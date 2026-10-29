//! The Smelter render backend's translation: a project becomes the ordered list of
//! HTTP requests that make Smelter render it (software-mansion/smelter, offline
//! mode). Pure: it runs no process, reads no file and opens no socket; the CLI
//! sends the requests. See `docs/specs/smelter-backend.md` for the evidence behind
//! every choice here (the half-frame lead of a scheduled update, the clock input,
//! one request per frame of a punch-in ramp).

use serde_json::{json, Value};

use crate::bridge::{CARD_KIND, SILENCE_KIND};
use crate::error::{Result, TimelineError};
use crate::init::{media_info, MediaInfo};
use crate::otio::{Item, MediaReference, Timeline, Track, AUDIO_KIND, VIDEO_KIND};
use crate::render::{
    card_of, continues, fade_ms, frame_rate_arg, zoom_of, CardSpec, RenderSize, Zoom,
    AUDIO_SAMPLE_RATE, MAX_PIXELS, MAX_SIDE,
};
use crate::tracks::{check_editable, item_range, track_frames};

/// Id of the output in every request.
pub const OUTPUT_ID: &str = "out";
/// Id of the extra input that ends an output whose last item has no input.
pub const CLOCK_ID: &str = "clock";
/// How many frames before a frame a scheduled scene update is scheduled. Smelter
/// applies an update to the frames after its time, so one scheduled at a frame's own
/// time lands a frame late (measured, spec evidence 3).
pub const UPDATE_LEAD_FRAMES: f64 = 0.5;
/// How many frames before its end an input is unregistered. An unregister exactly on
/// a frame's time is a coin toss (an extra frame was written when the time was not a
/// whole number of milliseconds); a quarter frame early gave the exact count, and so
/// did anything from a quarter frame early to half a frame late (spec evidence 4).
pub const END_LEAD_FRAMES: f64 = 0.25;
/// How many frames past the end of the timeline the clock input runs. The output
/// ends when the last input is unregistered, and the frame count at that moment
/// varied by a few frames from one run to the next (286 to 290 for 289, measured
/// under load), so Smelter renders a few frames too many and the CLI trims to the
/// exact count.
pub const CLOCK_MARGIN_FRAMES: f64 = 4.0;

/// One mp4 input Smelter registers: a range of one media file played from
/// `offset_ms` of the output, until it is unregistered at `end_ms`.
#[derive(Debug, Clone, PartialEq)]
pub struct SmelterInput {
    pub id: String,
    /// Index into [`SmelterPlan::media`].
    pub media: usize,
    pub seek_ms: f64,
    pub offset_ms: f64,
    pub end_ms: f64,
    /// Plays again from the start when the file ends (the clock input).
    pub looped: bool,
    /// The scene shows this input's picture.
    pub video: bool,
    /// The mixer plays this input's sound; Smelter must have heard audio in it.
    pub audio: bool,
}

/// From timeline frame `frame` on, the output shows `video` and mixes `audio`.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneStep {
    pub frame: i64,
    pub video: Value,
    /// Ids of the inputs the mixer plays. Empty when the project has no audio.
    pub audio: Vec<String>,
}

/// Everything needed to make Smelter render a project.
#[derive(Debug, Clone, PartialEq)]
pub struct SmelterPlan {
    /// Media files, referred to by index.
    pub media: Vec<String>,
    /// Card images (one PNG each, the size of the output), image ids `card0`, ...
    pub cards: Vec<CardSpec>,
    pub inputs: Vec<SmelterInput>,
    /// Scene changes in frame order; the first is at frame 0.
    pub steps: Vec<SceneStep>,
    /// Frame rate as Smelter's `SMELTER_OUTPUT_FRAMERATE` takes it.
    pub frame_rate: String,
    pub fps: f64,
    pub size: RenderSize,
    pub frames: i64,
    pub has_audio: bool,
    /// Things this backend cannot do for this project; the CLI prints them.
    pub notes: Vec<String>,
}

/// The files a plan refers to, by absolute path.
#[derive(Debug, Clone, Copy)]
pub struct SmelterFiles<'a> {
    pub media: &'a [String],
    pub cards: &'a [String],
    pub out: &'a str,
}

/// One HTTP request to Smelter's API.
#[derive(Debug, Clone, PartialEq)]
pub struct SmelterRequest {
    pub method: &'static str,
    pub path: String,
    pub body: Value,
}

/// Printed when the project changes what the output shows: Smelter applies a scheduled
/// scene update to frames it has already queued, so a change lands on its frame in
/// most runs and one or two frames early in others (spec evidence 3 and 5).
pub const JITTER_NOTE: &str = "render: Smelter applies a scheduled scene change out of step with the frames it has queued, so cuts, zoom steps and the card can land one or two frames early (measured in about a third of runs); use --backend ffmpeg when frame accuracy matters";

/// A stretch of one media file on the timeline: `frames` frames of `media` from
/// source frame `source`, playing from timeline frame `start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seg {
    media: usize,
    source: i64,
    start: i64,
    frames: i64,
}

impl Seg {
    fn end(&self) -> i64 {
        self.start + self.frames
    }

    /// Whether `other` lies inside this stretch, on the same footage.
    fn holds(&self, other: &Seg) -> bool {
        self.media == other.media
            && other.start >= self.start
            && other.end() <= self.end()
            && other.source - other.start == self.source - self.start
    }
}

/// Append `seg`, joined to the last one when it plays on from it.
fn push_merged(list: &mut Vec<Seg>, seg: Seg) {
    if let Some(last) = list.last_mut() {
        if last.media == seg.media
            && last.end() == seg.start
            && last.source + last.frames == seg.source
        {
            last.frames += seg.frames;
            return;
        }
    }
    list.push(seg);
}

enum What {
    Footage(Seg, Option<Zoom>),
    Card(usize),
    /// A gap, or the silence under a card.
    Empty,
}

struct Entry {
    start: i64,
    frames: i64,
    what: What,
}

fn media_index(media: &mut Vec<String>, url: &str) -> usize {
    match media.iter().position(|known| known == url) {
        Some(index) => index,
        None => {
            media.push(url.to_string());
            media.len() - 1
        }
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

impl Zoom {
    /// The zoom of clip frame `frame`: the `zoompan` expression of the ffmpeg render.
    pub(crate) fn scale_at(&self, frame: i64) -> f64 {
        let ramp = self.ramp_frames.max(1) as f64;
        let n = frame as f64;
        let up = ((n - self.start) / ramp).clamp(0.0, 1.0);
        let down = ((self.end - 1.0 - n) / ramp).clamp(0.0, 1.0);
        1.0 + (self.scale - 1.0) * up.min(down)
    }
}

fn plain_scene(input: &str) -> Value {
    json!({"type": "rescaler", "mode": "fill",
           "child": {"type": "input_stream", "input_id": input}})
}

/// The picture zoomed by `zoom` about its centre: a rescaler `zoom` times the output,
/// placed so its middle is the output's middle, inside a view that clips it.
fn zoomed_scene(input: &str, size: RenderSize, zoom: f64) -> Value {
    let (width, height) = (f64::from(size.width), f64::from(size.height));
    let (w, h) = (round3(width * zoom), round3(height * zoom));
    json!({"type": "view", "children": [{
        "type": "rescaler", "mode": "fill", "width": w, "height": h,
        "top": round3(-(h - height) / 2.0), "left": round3(-(w - width) / 2.0),
        "child": {"type": "input_stream", "input_id": input}
    }]})
}

fn card_scene(index: usize, size: RenderSize) -> Value {
    json!({"type": "view", "children": [
        {"type": "image", "image_id": format!("card{index}"),
         "width": size.width, "height": size.height}
    ]})
}

fn black_scene() -> Value {
    json!({"type": "view", "background_color": "#000000FF"})
}

/// Record that from `frame` on the value is `value`, unless it already is.
fn change<T: PartialEq>(steps: &mut Vec<(i64, T)>, frame: i64, value: T) {
    if steps.last().map(|(_, last)| last) != Some(&value) {
        steps.push((frame, value));
    }
}

fn checked_size(size: Option<RenderSize>, info: &MediaInfo) -> Result<RenderSize> {
    let size = size.unwrap_or(RenderSize {
        width: info.width,
        height: info.height,
    });
    if size.width == 0
        || size.height == 0
        || !size.width.is_multiple_of(2)
        || !size.height.is_multiple_of(2)
    {
        return Err(TimelineError::invalid(format!(
            "the output size {}x{} must have even, non-zero sides",
            size.width, size.height
        )));
    }
    if size.width > MAX_SIDE
        || size.height > MAX_SIDE
        || u64::from(size.width) * u64::from(size.height) > MAX_PIXELS
    {
        return Err(TimelineError::invalid(format!(
            "the output size {}x{} is too big: at most {MAX_SIDE} pixels on a side and {MAX_PIXELS} in all",
            size.width, size.height
        )));
    }
    Ok(size)
}

fn video_entries(
    track: &Track,
    rate: f64,
    media: &mut Vec<String>,
    cards: &mut Vec<CardSpec>,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut position = 0;
    for item in &track.children {
        let (source, frames) = item_range(item, rate)?;
        let what = match item {
            Item::Gap(_) => What::Empty,
            Item::Clip(clip) => match clip.active_reference()? {
                MediaReference::External(external) => {
                    let media = media_index(media, &external.target_url);
                    let seg = Seg {
                        media,
                        source,
                        start: position,
                        frames,
                    };
                    What::Footage(seg, zoom_of(clip)?)
                }
                MediaReference::Generator(generator) if generator.generator_kind == CARD_KIND => {
                    cards.push(card_of(generator, frames)?);
                    What::Card(cards.len() - 1)
                }
                MediaReference::Generator(generator) => {
                    return Err(TimelineError::invalid(format!(
                        "clip \"{}\": cannot render a \"{}\" video generator",
                        clip.name, generator.generator_kind
                    )))
                }
                MediaReference::Other(_) => {
                    return Err(TimelineError::invalid(format!(
                        "clip \"{}\" has a media reference this tool cannot render",
                        clip.name
                    )))
                }
            },
        };
        if frames > 0 {
            entries.push(Entry {
                start: position,
                frames,
                what,
            });
        }
        position += frames;
    }
    Ok(entries)
}

fn audio_entries(track: &Track, rate: f64, media: &mut Vec<String>) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut position = 0;
    for item in &track.children {
        let (source, frames) = item_range(item, rate)?;
        let what = match item {
            Item::Gap(_) => What::Empty,
            Item::Clip(clip) => match clip.active_reference()? {
                MediaReference::External(external) => {
                    let media = media_index(media, &external.target_url);
                    What::Footage(
                        Seg {
                            media,
                            source,
                            start: position,
                            frames,
                        },
                        None,
                    )
                }
                MediaReference::Generator(generator)
                    if generator.generator_kind == SILENCE_KIND =>
                {
                    What::Empty
                }
                MediaReference::Generator(generator) => {
                    return Err(TimelineError::invalid(format!(
                        "clip \"{}\": cannot render a \"{}\" audio generator",
                        clip.name, generator.generator_kind
                    )))
                }
                MediaReference::Other(_) => {
                    return Err(TimelineError::invalid(format!(
                        "clip \"{}\" has a media reference this tool cannot render",
                        clip.name
                    )))
                }
            },
        };
        if frames > 0 {
            entries.push(Entry {
                start: position,
                frames,
                what,
            });
        }
        position += frames;
    }
    Ok(entries)
}

fn merged_segs(entries: &[Entry]) -> Vec<Seg> {
    let mut segs = Vec::new();
    for entry in entries {
        if let What::Footage(seg, _) = &entry.what {
            push_merged(&mut segs, *seg);
        }
    }
    segs
}

/// The audio dip joins the ffmpeg render makes and this backend cannot: a note, or
/// nothing.
fn fade_note(track: &Track, rate: f64) -> Result<Option<String>> {
    let fade = |item: &Item| match item {
        Item::Clip(clip) => fade_ms(clip),
        Item::Gap(_) => 0,
    };
    let (mut joins, mut longest) = (0, 0);
    let mut playing: Vec<&Item> = Vec::new();
    for item in &track.children {
        if item_range(item, rate)?.1 > 0 {
            playing.push(item);
        }
    }
    for pair in playing.windows(2) {
        let dip = fade(pair[0]).max(fade(pair[1]));
        if dip > 0 && !continues(pair[0], pair[1], rate)? {
            joins += 1;
            longest = longest.max(dip);
        }
    }
    Ok((joins > 0).then(|| {
        format!(
            "render: Smelter cannot fade audio, so the {longest} ms audio dip that the ffmpeg render makes is not applied at {joins} join{}; a cut through sound can click",
            if joins == 1 { "" } else { "s" }
        )
    }))
}

/// Build the plan for `timeline`. `size` is the output size; `None` is the size of
/// the source media.
pub fn build_smelter_plan(timeline: &Timeline, size: Option<RenderSize>) -> Result<SmelterPlan> {
    let rate = check_editable(timeline)?;
    let info = media_info(timeline)?;
    let size = checked_size(size, &info)?;
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let audio = timeline.track(AUDIO_KIND);
    if video.children.is_empty() {
        return Err(TimelineError::invalid("the video track is empty"));
    }
    let frames = track_frames(video, rate)?;
    if frames == 0 {
        return Err(TimelineError::invalid(
            "the project has no frames: every item on the video track is empty",
        ));
    }
    let ms = |frame: i64| frame as f64 * 1000.0 / rate;

    let mut media: Vec<String> = Vec::new();
    let mut cards: Vec<CardSpec> = Vec::new();
    let video_entries = video_entries(video, rate, &mut media, &mut cards)?;
    let audio_entries = match audio {
        Some(track) => audio_entries(track, rate, &mut media)?,
        None => Vec::new(),
    };

    // Inputs: one per stretch of footage the picture plays; the sound shares an
    // input when it plays exactly the same stretch, else has its own.
    let video_segs = merged_segs(&video_entries);
    let audio_segs = merged_segs(&audio_entries);
    let mut inputs: Vec<SmelterInput> = Vec::new();
    let input_of = |seg: &Seg, inputs: &mut Vec<SmelterInput>, video: bool| -> usize {
        inputs.push(SmelterInput {
            id: format!("i{}", inputs.len()),
            media: seg.media,
            seek_ms: ms(seg.source),
            offset_ms: ms(seg.start),
            end_ms: ms(seg.end()),
            looped: false,
            video,
            audio: !video,
        });
        inputs.len() - 1
    };
    let video_ids: Vec<usize> = video_segs
        .iter()
        .map(|seg| input_of(seg, &mut inputs, true))
        .collect();
    let mut audio_ids: Vec<usize> = Vec::new();
    for seg in &audio_segs {
        match video_segs.iter().position(|other| other == seg) {
            Some(shared) => {
                inputs[video_ids[shared]].audio = true;
                audio_ids.push(video_ids[shared]);
            }
            None => audio_ids.push(input_of(seg, &mut inputs, false)),
        }
    }
    // The clock: an input that outlasts the timeline by CLOCK_MARGIN_FRAMES and ends
    // the output when it is unregistered. The CLI trims the surplus frames.
    if media.is_empty() {
        return Err(TimelineError::invalid(
            "the project has no footage, so Smelter has no input to time the output by",
        ));
    }
    inputs.push(SmelterInput {
        id: CLOCK_ID.to_string(),
        media: 0,
        seek_ms: 0.0,
        offset_ms: 0.0,
        end_ms: ms(frames),
        looped: true,
        video: false,
        audio: false,
    });
    let id_in = |segs: &[Seg], ids: &[usize], seg: &Seg| -> String {
        let at = segs.iter().position(|other| other.holds(seg)).unwrap_or(0);
        inputs[ids[at]].id.clone()
    };

    // Scenes: the picture, frame by frame where it changes.
    let mut pictures: Vec<(i64, Value)> = Vec::new();
    for entry in &video_entries {
        match &entry.what {
            What::Footage(seg, None) => change(
                &mut pictures,
                entry.start,
                plain_scene(&id_in(&video_segs, &video_ids, seg)),
            ),
            What::Footage(seg, Some(zoom)) => {
                let id = id_in(&video_segs, &video_ids, seg);
                let mut last: Option<f64> = None;
                for n in 0..entry.frames {
                    let z = zoom.scale_at(n);
                    if last == Some(z) {
                        continue;
                    }
                    last = Some(z);
                    let scene = if z == 1.0 {
                        plain_scene(&id)
                    } else {
                        zoomed_scene(&id, size, z)
                    };
                    change(&mut pictures, entry.start + n, scene);
                }
            }
            What::Card(index) => change(&mut pictures, entry.start, card_scene(*index, size)),
            What::Empty => change(&mut pictures, entry.start, black_scene()),
        }
    }
    let mut sounds: Vec<(i64, Vec<String>)> = Vec::new();
    for entry in &audio_entries {
        let playing = match &entry.what {
            What::Footage(seg, _) => vec![id_in(&audio_segs, &audio_ids, seg)],
            What::Card(_) | What::Empty => Vec::new(),
        };
        change(&mut sounds, entry.start, playing);
    }
    let mut at: Vec<i64> = pictures
        .iter()
        .map(|(frame, _)| *frame)
        .chain(sounds.iter().map(|(frame, _)| *frame))
        .collect();
    at.sort_unstable();
    at.dedup();
    let steps: Vec<SceneStep> = at
        .into_iter()
        .map(|frame| SceneStep {
            frame,
            video: pictures
                .iter()
                .rfind(|(from, _)| *from <= frame)
                .map(|(_, scene)| scene.clone())
                .unwrap_or_else(black_scene),
            audio: sounds
                .iter()
                .rfind(|(from, _)| *from <= frame)
                .map(|(_, playing)| playing.clone())
                .unwrap_or_default(),
        })
        .collect();

    let mut notes: Vec<String> = match audio {
        Some(track) => fade_note(track, rate)?.into_iter().collect(),
        None => Vec::new(),
    };
    if steps.len() > 1 {
        notes.push(JITTER_NOTE.to_string());
    }
    Ok(SmelterPlan {
        media,
        cards,
        inputs,
        steps,
        frame_rate: frame_rate_arg(rate),
        fps: rate,
        size,
        frames,
        has_audio: audio.is_some(),
        notes,
    })
}

fn post(path: String, body: Value) -> SmelterRequest {
    SmelterRequest {
        method: "POST",
        path,
        body,
    }
}

fn mixer(inputs: &[String]) -> Value {
    json!({"inputs": inputs.iter().map(|id| json!({"input_id": id})).collect::<Vec<_>>()})
}

impl SmelterPlan {
    /// The requests, in the order they are sent: images, inputs, the output (with
    /// its first scene), the scheduled scene updates, the scheduled input
    /// unregisters, and `start`. An error if `files` does not hold what the plan
    /// refers to.
    pub fn requests(&self, files: &SmelterFiles<'_>) -> Result<Vec<SmelterRequest>> {
        if files.media.len() < self.media.len() || files.cards.len() < self.cards.len() {
            return Err(TimelineError::invalid(format!(
                "the plan needs {} media files and {} card images, but got {} and {}",
                self.media.len(),
                self.cards.len(),
                files.media.len(),
                files.cards.len()
            )));
        }
        let mut all = Vec::new();
        for (index, path) in files.cards.iter().enumerate().take(self.cards.len()) {
            all.push(post(
                format!("/api/image/card{index}/register"),
                json!({"asset_type": "png", "path": path}),
            ));
        }
        for input in &self.inputs {
            let mut body = json!({
                "type": "mp4",
                "path": files.media[input.media],
                "seek_ms": input.seek_ms,
                "offset_ms": input.offset_ms,
                "required": true,
            });
            if input.looped {
                body["loop"] = json!(true);
            }
            all.push(post(format!("/api/input/{}/register", input.id), body));
        }
        let first = &self.steps[0];
        let mut output = json!({
            "type": "mp4",
            "path": files.out,
            "video": {
                "resolution": {"width": self.size.width, "height": self.size.height},
                "encoder": {
                    "type": "ffmpeg_h264",
                    "pixel_format": "yuv420p",
                    "ffmpeg_options": {"preset": "veryfast", "crf": "18", "bf": "0"}
                },
                "initial": {"root": first.video},
                "send_eos_when": {"all_inputs": true}
            },
            "ffmpeg_options": {"movflags": "+faststart"}
        });
        if self.has_audio {
            output["audio"] = json!({
                "encoder": {"type": "aac", "sample_rate": AUDIO_SAMPLE_RATE},
                "channels": "stereo",
                "initial": mixer(&first.audio),
                "send_eos_when": {"all_inputs": true}
            });
        }
        all.push(post(format!("/api/output/{OUTPUT_ID}/register"), output));
        for step in &self.steps[1..] {
            let at = (step.frame as f64 - UPDATE_LEAD_FRAMES) * 1000.0 / self.fps;
            let mut body = json!({
                "schedule_time_ms": at,
                "video": {"root": step.video},
            });
            if self.has_audio {
                body["audio"] = mixer(&step.audio);
            }
            all.push(post(format!("/api/output/{OUTPUT_ID}/update"), body));
        }
        let frame_ms = 1000.0 / self.fps;
        for input in &self.inputs {
            let at = if input.id == CLOCK_ID {
                input.end_ms + CLOCK_MARGIN_FRAMES * frame_ms
            } else {
                input.end_ms - END_LEAD_FRAMES * frame_ms
            };
            all.push(post(
                format!("/api/input/{}/unregister", input.id),
                json!({"schedule_time_ms": at}),
            ));
        }
        all.push(post("/api/start".to_string(), json!({})));
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{attach_bridge, BridgeRequest};
    use crate::cut::{cut_silence, CutOptions};
    use crate::init::{init, InitSpec};
    use crate::otio::{Gap, Item, TimeRange, AUDIO_KIND, VIDEO_KIND};
    use crate::punch::{punch_in, PunchRequest};
    use crate::render::build_render_plan;
    use crate::testutil::{dna, project};
    use serde_json::json;

    const MEDIA: &str = "/media/take.mp4";

    fn plan(timeline: &Timeline) -> SmelterPlan {
        build_smelter_plan(timeline, None).expect("plan")
    }

    fn requests(plan: &SmelterPlan) -> Vec<SmelterRequest> {
        let cards: Vec<String> = (0..plan.cards.len())
            .map(|n| format!("/tmp/card{n}.png"))
            .collect();
        plan.requests(&SmelterFiles {
            media: &[MEDIA.to_string()],
            cards: &cards,
            out: "/tmp/out.mp4",
        })
        .expect("requests")
    }

    fn plain(id: &str) -> Value {
        json!({"type": "rescaler", "mode": "fill", "child": {"type": "input_stream", "input_id": id}})
    }

    fn zoomed(id: &str, width: f64, height: f64, zoom: f64) -> Value {
        let (w, h) = (round3(width * zoom), round3(height * zoom));
        json!({"type": "view", "children": [{
            "type": "rescaler", "mode": "fill", "width": w, "height": h,
            "top": round3(-(h - height) / 2.0), "left": round3(-(w - width) / 2.0),
            "child": {"type": "input_stream", "input_id": id}
        }]})
    }

    /// 10 s at 30 fps: a 500 ms pause cut at 1.0 s (frames 30..45 go), a punch-in over
    /// frames 60..120 of the result, and a bridge at 72 %.
    fn edited() -> Timeline {
        let mut timeline = project(30.0, 300);
        let gaps = dna(10_000, 30.0, &[(1_000, 1_500)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        punch_in(
            &mut timeline,
            &PunchRequest {
                start_ms: 2_000,
                end_ms: 4_000,
                scale: 1.18,
            },
        )
        .expect("punch");
        let bridge = BridgeRequest {
            target_url: "example.com/go".to_string(),
            cutoff_pct: 0.72,
            card_ms: 2_500,
        };
        attach_bridge(&mut timeline, &bridge, &[]).expect("bridge");
        timeline
    }

    fn cut_only() -> Timeline {
        let mut timeline = project(30.0, 300);
        let gaps = dna(10_000, 30.0, &[(1_000, 1_500)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        timeline
    }

    fn step_at(plan: &SmelterPlan, frame: i64) -> &SceneStep {
        plan.steps
            .iter()
            .rfind(|step| step.frame <= frame)
            .expect("a step")
    }

    #[test]
    fn one_clip_is_one_input_and_one_scene() {
        let plan = plan(&project(30.0, 300));
        // The footage, and the clock that ends the output.
        assert_eq!(plan.inputs.len(), 2);
        let input = &plan.inputs[0];
        assert_eq!(input.id, "i0");
        assert_eq!(
            (input.seek_ms, input.offset_ms, input.end_ms),
            (0.0, 0.0, 10_000.0)
        );
        assert!(input.video && input.audio && !input.looped);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].video, plain("i0"));
        assert_eq!(plan.steps[0].audio, vec!["i0".to_string()]);
        assert_eq!((plan.frames, plan.frame_rate.as_str()), (300, "30"));
        assert!(plan.has_audio && plan.notes.is_empty());
        assert_eq!(plan.media, vec![MEDIA.to_string()]);
    }

    #[test]
    fn a_cut_starts_a_second_input_at_its_timeline_time() {
        let plan = plan(&cut_only());
        assert_eq!(plan.frames, 285);
        assert_eq!(plan.inputs.len(), 3);
        let (first, second) = (&plan.inputs[0], &plan.inputs[1]);
        assert_eq!(
            (first.seek_ms, first.offset_ms, first.end_ms),
            (0.0, 0.0, 1_000.0)
        );
        // Source frame 45 at 30 fps is 1500 ms; it plays from timeline frame 30.
        assert_eq!(
            (second.seek_ms, second.offset_ms, second.end_ms),
            (1_500.0, 1_000.0, 9_500.0)
        );
        let frames: Vec<i64> = plan.steps.iter().map(|step| step.frame).collect();
        assert_eq!(frames, vec![0, 30]);
        assert_eq!(plan.steps[0].video, plain("i0"));
        assert_eq!(plan.steps[1].video, plain("i1"));
        assert_eq!(plan.steps[1].audio, vec!["i1".to_string()]);
    }

    #[test]
    fn a_scene_update_is_scheduled_half_a_frame_before_its_frame() {
        let all = requests(&plan(&cut_only()));
        let update = all
            .iter()
            .find(|request| request.path == "/api/output/out/update")
            .expect("an update");
        // Frame 30 at 30 fps is 1000 ms; half a frame earlier is 983.33.. ms.
        let at = update.body["schedule_time_ms"].as_f64().expect("a time");
        assert!((at - (30.0 - 0.5) * 1000.0 / 30.0).abs() < 1e-9, "{at}");
        assert_eq!(update.body["video"]["root"], plain("i1"));
        assert_eq!(
            update.body["audio"],
            json!({"inputs": [{"input_id": "i1"}]})
        );
    }

    #[test]
    fn the_requests_come_in_a_fixed_order() {
        let all = requests(&plan(&edited()));
        let kinds: Vec<String> = all
            .iter()
            .map(|request| {
                let parts: Vec<&str> = request.path.split('/').collect();
                match parts.as_slice() {
                    ["", "api", "image", _, action] => format!("image {action}"),
                    ["", "api", "input", _, action] => format!("input {action}"),
                    ["", "api", "output", _, action] => format!("output {action}"),
                    ["", "api", action] => (*action).to_string(),
                    _ => request.path.clone(),
                }
            })
            .collect();
        let mut seen = Vec::new();
        for kind in &kinds {
            if seen.last() != Some(kind) {
                seen.push(kind.clone());
            }
        }
        assert_eq!(
            seen,
            [
                "image register",
                "input register",
                "output register",
                "output update",
                "input unregister",
                "start"
            ]
        );
        assert!(all.iter().all(|request| request.method == "POST"));
        assert_eq!(all.last().expect("start").body, json!({}));
    }

    #[test]
    fn the_output_is_registered_with_the_encoder_the_size_and_the_first_scene() {
        let all = requests(&plan(&project(30.0, 300)));
        let output = all
            .iter()
            .find(|request| request.path == "/api/output/out/register")
            .expect("register");
        assert_eq!(
            output.body,
            json!({
                "type": "mp4",
                "path": "/tmp/out.mp4",
                "video": {
                    "resolution": {"width": 640, "height": 360},
                    "encoder": {
                        "type": "ffmpeg_h264",
                        "pixel_format": "yuv420p",
                        "ffmpeg_options": {"preset": "veryfast", "crf": "18", "bf": "0"}
                    },
                    "initial": {"root": plain("i0")},
                    "send_eos_when": {"all_inputs": true}
                },
                "audio": {
                    "encoder": {"type": "aac", "sample_rate": 48000},
                    "channels": "stereo",
                    "initial": {"inputs": [{"input_id": "i0"}]},
                    "send_eos_when": {"all_inputs": true}
                },
                "ffmpeg_options": {"movflags": "+faststart"}
            })
        );
    }

    #[test]
    fn an_input_is_registered_with_its_range_and_unregistered_at_its_end() {
        let all = requests(&plan(&cut_only()));
        let register = all
            .iter()
            .find(|request| request.path == "/api/input/i1/register")
            .expect("register");
        assert_eq!(
            register.body,
            json!({"type": "mp4", "path": MEDIA, "seek_ms": 1500.0, "offset_ms": 1000.0, "required": true})
        );
        let unregister = all
            .iter()
            .find(|request| request.path == "/api/input/i1/unregister")
            .expect("unregister");
        assert_eq!(
            unregister.body,
            json!({"schedule_time_ms": 9500.0 - 0.25 * 1000.0 / 30.0})
        );
    }

    #[test]
    fn a_gap_is_a_black_picture_and_silence() {
        let mut timeline = project(30.0, 300);
        for kind in [VIDEO_KIND, AUDIO_KIND] {
            let track = timeline.track_mut(kind).expect("track");
            track
                .children
                .insert(1, Item::Gap(Gap::new(TimeRange::new(0.0, 15.0, 30.0))));
        }
        let plan = plan(&timeline);
        let black = json!({"type": "view", "background_color": "#000000FF"});
        let gap_step = plan.steps.iter().find(|s| s.video == black).expect("black");
        assert!(gap_step.audio.is_empty());
        assert_eq!(gap_step.frame, 300);
        assert_eq!(plan.frames, 315);
    }

    #[test]
    fn the_card_is_an_image_the_size_of_the_output_with_no_sound() {
        let plan = plan(&edited());
        assert_eq!(plan.cards.len(), 1);
        let card_start = plan.frames - plan.cards[0].frames;
        let step = step_at(&plan, card_start);
        assert_eq!(step.frame, card_start);
        assert_eq!(
            step.video,
            json!({"type": "view", "children": [
                {"type": "image", "image_id": "card0", "width": 640, "height": 360}
            ]})
        );
        assert!(step.audio.is_empty());
        let all = requests(&plan);
        let image = all
            .iter()
            .find(|request| request.path == "/api/image/card0/register")
            .expect("image");
        assert_eq!(
            image.body,
            json!({"asset_type": "png", "path": "/tmp/card0.png"})
        );
    }

    #[test]
    fn the_clock_input_ends_the_output_and_outlasts_the_timeline() {
        let plan = plan(&edited());
        let clock = plan.inputs.last().expect("an input");
        assert_eq!(clock.id, "clock");
        assert_eq!(
            (clock.media, clock.seek_ms, clock.offset_ms, clock.looped),
            (0, 0.0, 0.0, true)
        );
        assert_eq!(clock.end_ms, plan.frames as f64 * 1000.0 / 30.0);
        assert!(!clock.video && !clock.audio);
        // It is unregistered four frames after the timeline ends; the CLI trims the rest.
        let unregister = requests(&plan)
            .into_iter()
            .find(|request| request.path == "/api/input/clock/unregister")
            .expect("unregister");
        let at = unregister.body["schedule_time_ms"].as_f64().expect("time");
        assert!(
            (at - (plan.frames as f64 + 4.0) * 1000.0 / 30.0).abs() < 1e-9,
            "{at}"
        );
        // It is registered with `loop`, so a short file cannot end the output early.
        let all = requests(&plan);
        let register = all
            .iter()
            .find(|request| request.path == "/api/input/clock/register")
            .expect("clock register");
        assert_eq!(register.body["loop"], json!(true));
        // And it is in no scene.
        assert!(plan
            .steps
            .iter()
            .all(|step| !step.audio.contains(&"clock".to_string())
                && !step.video.to_string().contains("clock")));
    }

    #[test]
    fn a_punch_in_is_sampled_on_every_frame_of_its_ramps() {
        let plan = plan(&edited());
        // The punch-in covers frames 60..120 of the edit (6 up, 48 held, 6 down).
        // Frame 60 is the first frame of the zoom: z(0) = 1.
        let id = |frame| match &step_at(&plan, frame).video {
            Value::Object(object) => object
                .get("children")
                .and_then(|children| children[0]["child"]["input_id"].as_str())
                .or_else(|| object["child"]["input_id"].as_str())
                .expect("an input")
                .to_string(),
            other => panic!("{other}"),
        };
        let source = id(60);
        assert_eq!(step_at(&plan, 60).video, plain(&source));
        for n in 1..=6 {
            let z = 1.0 + 0.18 * (f64::from(n) / 6.0).min(1.0);
            let step = step_at(&plan, 60 + i64::from(n));
            assert_eq!(step.frame, 60 + i64::from(n), "a step for ramp frame {n}");
            assert_eq!(step.video, zoomed(&source, 640.0, 360.0, z), "frame {n}");
        }
        // Held from n = 6 to n = 53: one scene, no steps between.
        assert_eq!(step_at(&plan, 66).video, step_at(&plan, 113).video);
        assert_eq!(
            plan.steps
                .iter()
                .filter(|step| (67..=113).contains(&step.frame))
                .count(),
            0
        );
        // Ramp down: n = 54..59, z = 1 + 0.18 * (59 - n) / 6, and 1.0 at n = 59.
        for n in 54..=59 {
            let z = 1.0 + 0.18 * f64::from(59 - n) / 6.0;
            let step = step_at(&plan, 60 + i64::from(n));
            let want = if n == 59 {
                plain(&source)
            } else {
                zoomed(&source, 640.0, 360.0, z)
            };
            assert_eq!(step.video, want, "down frame {n}");
        }
    }

    #[test]
    fn a_punch_in_that_began_in_an_earlier_clip_continues_at_the_zoom() {
        // A cut at frame 30 and a punch-in over 20..80: the clip from 30 starts
        // inside the hold (its zoom `start` is -10), and so is at 1.18 at once.
        let mut timeline = project(30.0, 300);
        let gaps = dna(10_000, 30.0, &[(1_000, 1_300)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        punch_in(
            &mut timeline,
            &PunchRequest {
                start_ms: 667,
                end_ms: 2_667,
                scale: 1.18,
            },
        )
        .expect("punch");
        let plan = plan(&timeline);
        let hold = step_at(&plan, 30);
        assert_eq!(hold.frame, 30, "the new clip is a new input");
        assert!(
            plan.inputs.len() >= 3,
            "two stretches of footage and the clock"
        );
        let width = hold.video["children"][0]["width"].as_f64().expect("width");
        assert!((width - round3(640.0 * 1.18)).abs() < 1e-9, "{width}");
    }

    #[test]
    fn the_zoom_uses_the_output_size() {
        let plan = build_smelter_plan(
            &edited(),
            Some(RenderSize {
                width: 1080,
                height: 1920,
            }),
        )
        .expect("plan");
        let step = step_at(&plan, 66);
        assert_eq!(step.video["children"][0]["width"], json!(1274.4));
        assert_eq!(step.video["children"][0]["height"], json!(2265.6));
        assert_eq!(step.video["children"][0]["top"], json!(-172.8));
        assert_eq!(step.video["children"][0]["left"], json!(-97.2));
        let all = requests(&plan);
        let output = all
            .iter()
            .find(|request| request.path == "/api/output/out/register")
            .expect("register");
        assert_eq!(
            output.body["video"]["resolution"],
            json!({"width": 1080, "height": 1920})
        );
        assert_eq!(
            plan.size,
            RenderSize {
                width: 1080,
                height: 1920
            }
        );
    }

    #[test]
    fn the_frame_rate_is_in_the_form_smelter_takes() {
        for (fps, text) in [(24.0, "24"), (30000.0 / 1001.0, "30000/1001"), (25.0, "25")] {
            let plan = plan(&project(fps, 120));
            assert_eq!(plan.frame_rate, text);
            assert_eq!(plan.fps, fps);
        }
        // At 30000/1001 fps, frame 30 is at 1001 ms; half a frame early is 984.65 ms.
        let timeline = {
            let mut timeline = project(30000.0 / 1001.0, 300);
            let gaps = dna(10_000, 29.97, &[(1_000, 1_500)]);
            cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
            timeline
        };
        let plan = plan(&timeline);
        let all = requests(&plan);
        let update = all
            .iter()
            .find(|request| request.path == "/api/output/out/update")
            .expect("update");
        let at = update.body["schedule_time_ms"].as_f64().expect("time");
        let frame = plan.steps[1].frame as f64;
        assert!((at - (frame - 0.5) * 1001.0 / 30.0).abs() < 1e-6, "{at}");
    }

    #[test]
    fn a_project_without_audio_has_no_audio_anywhere() {
        let timeline = init(&InitSpec {
            media_path: MEDIA.to_string(),
            fps: 30.0,
            frames: 300,
            width: 640,
            height: 360,
            has_audio: false,
            dna: None,
        })
        .expect("init");
        let plan = plan(&timeline);
        assert!(!plan.has_audio);
        assert!(plan.steps.iter().all(|step| step.audio.is_empty()));
        assert!(!plan.inputs[0].audio);
        let all = requests(&plan);
        let output = all
            .iter()
            .find(|request| request.path == "/api/output/out/register")
            .expect("register");
        assert!(output.body.get("audio").is_none());
        assert!(all
            .iter()
            .filter(|request| request.path == "/api/output/out/update")
            .all(|request| request.body.get("audio").is_none()));
    }

    #[test]
    fn audio_and_video_with_the_same_range_share_an_input_and_others_do_not() {
        let mut timeline = project(30.0, 300);
        timeline
            .track_mut(AUDIO_KIND)
            .expect("audio")
            .children
            .iter_mut()
            .for_each(|item| *item.source_range_mut() = Some(TimeRange::new(3.0, 297.0, 30.0)));
        timeline
            .track_mut(VIDEO_KIND)
            .expect("video")
            .children
            .iter_mut()
            .for_each(|item| *item.source_range_mut() = Some(TimeRange::new(0.0, 297.0, 30.0)));
        let plan = plan(&timeline);
        assert_eq!(plan.inputs.len(), 3);
        assert!(plan.inputs[0].video && !plan.inputs[0].audio);
        assert!(!plan.inputs[1].video && plan.inputs[1].audio);
        assert_eq!(plan.inputs[1].seek_ms, 100.0);
        assert_eq!(plan.steps[0].video, plain("i0"));
        assert_eq!(plan.steps[0].audio, vec!["i1".to_string()]);
    }

    #[test]
    fn pieces_that_play_on_are_one_input() {
        // The punch-in only splits a clip: its pieces are one stretch of footage.
        let mut timeline = project(30.0, 300);
        punch_in(
            &mut timeline,
            &PunchRequest {
                start_ms: 2_000,
                end_ms: 4_000,
                scale: 1.2,
            },
        )
        .expect("punch");
        let plan = plan(&timeline);
        // One stretch of footage (not three pieces), and the clock.
        assert_eq!(plan.inputs.len(), 2);
        assert_eq!(plan.inputs[0].end_ms, 10_000.0);
    }

    #[test]
    fn what_it_cannot_do_is_a_note() {
        assert!(plan(&project(30.0, 300)).notes.is_empty());
        let notes = plan(&cut_only()).notes;
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(
            notes[0].contains("audio dip") && notes[0].contains("is not applied at 1 join;"),
            "{}",
            notes[0]
        );
        // The cut and the card are two joins; 30 ms is the longest dip.
        let two = plan(&edited()).notes;
        assert!(two[0].contains("the 30 ms audio dip"), "{}", two[0]);
        assert!(two[0].contains("is not applied at 2 joins;"), "{}", two[0]);
        assert!(notes[0].starts_with("render: "));
        assert_eq!(notes[1], JITTER_NOTE);
    }

    #[test]
    fn it_refuses_what_the_ffmpeg_plan_refuses_with_the_same_words() {
        let odd = Some(RenderSize {
            width: 641,
            height: 360,
        });
        let big = Some(RenderSize {
            width: 8194,
            height: 8192,
        });
        // Each side is allowed, but 8192 x 8192 is more pixels than 8K UHD.
        let many = Some(RenderSize {
            width: 8192,
            height: 8192,
        });
        for size in [odd, big, many] {
            let ours = build_smelter_plan(&project(30.0, 300), size).expect_err("refused");
            let theirs = build_render_plan(&project(30.0, 300), size).expect_err("refused");
            assert_eq!(ours.to_string(), theirs.to_string());
        }
        let mut timeline = project(30.0, 300);
        timeline.track_mut(VIDEO_KIND).expect("v").children.clear();
        timeline.track_mut(AUDIO_KIND).expect("a").children.clear();
        let ours = build_smelter_plan(&timeline, None).expect_err("empty");
        let theirs = build_render_plan(&timeline, None).expect_err("empty");
        assert_eq!(ours.to_string(), theirs.to_string());
    }

    #[test]
    fn a_project_of_empty_items_has_no_frames_to_render() {
        let mut timeline = project(30.0, 300);
        for kind in [VIDEO_KIND, AUDIO_KIND] {
            for item in &mut timeline.track_mut(kind).expect("track").children {
                *item.source_range_mut() = Some(TimeRange::new(0.0, 0.0, 30.0));
            }
        }
        let ours = build_smelter_plan(&timeline, None).expect_err("no frames");
        let theirs = build_render_plan(&timeline, None).expect_err("no frames");
        assert_eq!(ours.to_string(), theirs.to_string());
        assert!(ours.to_string().contains("no frames"), "{ours}");
    }

    #[test]
    fn an_item_of_no_length_adds_nothing() {
        // A zero-length gap between two clips is not a scene, an input or a step.
        let mut timeline = cut_only();
        for kind in [VIDEO_KIND, AUDIO_KIND] {
            timeline
                .track_mut(kind)
                .expect("track")
                .children
                .insert(1, Item::Gap(Gap::new(TimeRange::new(0.0, 0.0, 30.0))));
        }
        assert_eq!(plan(&timeline), plan(&cut_only()));
    }

    #[test]
    fn the_same_project_gives_the_same_requests() {
        let a = serde_json::to_string(&requests_json(&edited())).expect("json");
        let b = serde_json::to_string(&requests_json(&edited())).expect("json");
        assert_eq!(a, b);
    }

    fn requests_json(timeline: &Timeline) -> Vec<Value> {
        requests(&plan(timeline))
            .iter()
            .map(|request| json!([request.method, request.path, request.body]))
            .collect()
    }

    /// The number after `key` in `text`, up to the next `:` or `,` or `[`.
    fn number_after(text: &str, key: &str) -> Option<i64> {
        let rest = &text[text.find(key)? + key.len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse().ok()
    }

    /// The stretches of footage the ffmpeg graph's picture trims, read from its text:
    /// each `[svN]` chain is a trim of the source (`trim=start_frame=A:end_frame=B`) or
    /// a generated span (`trim=end_frame=N`), played one after the other.
    fn ffmpeg_video_segs(timeline: &Timeline) -> Vec<Seg> {
        let graph = build_render_plan(timeline, None)
            .expect("plan")
            .filter_graph;
        let mut segs = Vec::new();
        let mut position = 0;
        for part in graph.split(';') {
            if !part.contains("setpts=PTS-STARTPTS") || !part.contains("[sv") {
                continue;
            }
            let (source, frames) = match number_after(part, "trim=start_frame=") {
                Some(from) => {
                    let to = number_after(part, ":end_frame=").expect("an end");
                    (Some(from), to - from)
                }
                None => (
                    None,
                    number_after(part, "trim=end_frame=").expect("a length"),
                ),
            };
            if let Some(source) = source {
                push_merged(
                    &mut segs,
                    Seg {
                        media: 0,
                        source,
                        start: position,
                        frames,
                    },
                );
            }
            position += frames;
        }
        segs
    }

    /// The same for the sound: `atrim=start_sample=S:end_sample=E` at 48 kHz.
    fn ffmpeg_audio_segs(timeline: &Timeline) -> Vec<Seg> {
        let plan = build_render_plan(timeline, None).expect("plan");
        let (graph, fps) = (plan.filter_graph, plan.fps);
        let frames_of = |samples: i64| (samples as f64 * fps / 48_000.0).round() as i64;
        let mut segs = Vec::new();
        let mut position = 0;
        for part in graph.split(';') {
            if !part.contains("asetpts=PTS-STARTPTS") || !part.contains("[sa") {
                continue;
            }
            match number_after(part, "atrim=start_sample=") {
                Some(from) => {
                    let to = number_after(part, ":end_sample=").expect("an end");
                    let frames = frames_of(to - from);
                    push_merged(
                        &mut segs,
                        Seg {
                            media: 0,
                            source: frames_of(from),
                            start: position,
                            frames,
                        },
                    );
                    position += frames;
                }
                None => position += frames_of(number_after(part, "atrim=end_sample=").expect("n")),
            }
        }
        segs
    }

    /// The stretches Smelter is asked to play, read from the request bodies.
    fn smelter_segs(timeline: &Timeline, want_audio: bool) -> Vec<Seg> {
        let plan = plan(timeline);
        let frame = |ms: f64| (ms * plan.fps / 1000.0).round() as i64;
        let all = requests(&plan);
        let mut segs = Vec::new();
        for input in &plan.inputs {
            if input.id == CLOCK_ID || !(if want_audio { input.audio } else { input.video }) {
                continue;
            }
            let register = all
                .iter()
                .find(|r| r.path == format!("/api/input/{}/register", input.id))
                .expect("register");
            let unregister = all
                .iter()
                .find(|r| r.path == format!("/api/input/{}/unregister", input.id))
                .expect("unregister");
            let start = frame(register.body["offset_ms"].as_f64().expect("offset"));
            let end = frame(unregister.body["schedule_time_ms"].as_f64().expect("end"));
            segs.push(Seg {
                media: 0,
                source: frame(register.body["seek_ms"].as_f64().expect("seek")),
                start,
                frames: end - start,
            });
        }
        segs.sort_by_key(|seg| seg.start);
        segs
    }

    #[test]
    fn the_two_backends_play_the_same_footage_at_the_same_times() {
        let mut projects = vec![project(30.0, 300), cut_only(), edited()];
        for fps in [24.0, 30000.0 / 1001.0, 60.0] {
            let mut timeline = project(fps, (fps * 10.0).round() as i64);
            let gaps = dna(10_000, fps, &[(1_000, 1_500), (3_000, 3_400)]);
            cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
            punch_in(
                &mut timeline,
                &PunchRequest {
                    start_ms: 1_500,
                    end_ms: 3_500,
                    scale: 1.3,
                },
            )
            .expect("punch");
            let bridge = BridgeRequest {
                target_url: "a.co".to_string(),
                cutoff_pct: 0.6,
                card_ms: 1_000,
            };
            attach_bridge(&mut timeline, &bridge, &[]).expect("bridge");
            projects.push(timeline);
        }
        for timeline in &projects {
            let wanted = ffmpeg_video_segs(timeline);
            assert!(!wanted.is_empty());
            assert_eq!(smelter_segs(timeline, false), wanted, "picture");
            assert_eq!(
                smelter_segs(timeline, true),
                ffmpeg_audio_segs(timeline),
                "sound"
            );
            // And every picture input takes over the output at the frame it starts.
            let plan = plan(timeline);
            for input in plan.inputs.iter().filter(|input| input.video) {
                let start = (input.offset_ms * plan.fps / 1000.0).round() as i64;
                let step = step_at(&plan, start);
                assert!(
                    step.video
                        .to_string()
                        .contains(&format!("\"{}\"", input.id)),
                    "input {} at frame {start}",
                    input.id
                );
            }
        }
    }
}
