//! The ffmpeg filter graph that renders a project: trims, concatenation, audio
//! fades at joins, the punch-in zoom, and the bridge card. Pure text; the CLI runs
//! it.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::bridge::CARD_KIND;
use crate::bridge::SILENCE_KIND;
use crate::error::{Result, TimelineError};
use crate::init::media_info;
use crate::otio::{
    reelctl, Clip, GeneratorReference, Item, MediaReference, Timeline, Track, AUDIO_KIND,
    VIDEO_KIND,
};
use crate::tracks::{check_editable, external_url, item_range, FADE_KEY, PUNCH_IN_KEY};

/// Sample rate of the rendered audio. Every segment is resampled to it.
pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
/// Label of the video the graph produces.
pub const VIDEO_LABEL: &str = "vout";
/// Label of the audio the graph produces, when the project has an audio track.
pub const AUDIO_LABEL: &str = "aout";

/// Output size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderSize {
    pub width: u32,
    pub height: u32,
}

/// A card to draw: an image the size of the output, looped for `frames` frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSpec {
    pub text: String,
    pub background: String,
    pub foreground: String,
    pub frames: i64,
}

/// Everything needed to run ffmpeg for a project.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderPlan {
    /// Media files, as inputs `0..media.len()`.
    pub media: Vec<String>,
    /// Card images, as inputs `media.len()..`, in order.
    pub cards: Vec<CardSpec>,
    pub filter_graph: String,
    pub video_label: String,
    pub audio_label: Option<String>,
    /// Frame rate as ffmpeg writes it (`30`, `30000/1001`).
    pub frame_rate: String,
    pub fps: f64,
    pub size: RenderSize,
    /// Length of the project in frames.
    pub frames: i64,
}

/// The frame rate as ffmpeg takes it: an integer, or `N000/1001` for the NTSC rates.
pub fn frame_rate_arg(rate: f64) -> String {
    let nearest = rate.round();
    if (rate - nearest).abs() < 1e-6 {
        return format!("{}", nearest as i64);
    }
    let ntsc = (rate * 1.001).round();
    if ((ntsc * 1000.0 / 1001.0) - rate).abs() < 1e-3 {
        return format!("{}000/1001", ntsc as i64);
    }
    format!("{rate}")
}

fn samples(frames: i64, rate: f64) -> i64 {
    (frames as f64 * f64::from(AUDIO_SAMPLE_RATE) / rate).round() as i64
}

fn seconds(samples: i64) -> f64 {
    samples as f64 / f64::from(AUDIO_SAMPLE_RATE)
}

fn number(value: f64) -> String {
    let text = format!("{value:.6}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    text.to_string()
}

/// One input stream used `uses` times: `[k:v]` itself, or `[k_v0]`, `[k_v1]` ... after
/// a `split`.
struct Taps {
    next: BTreeMap<usize, usize>,
    uses: BTreeMap<usize, usize>,
    kind: char,
}

impl Taps {
    fn new(kind: char) -> Self {
        Self {
            next: BTreeMap::new(),
            uses: BTreeMap::new(),
            kind,
        }
    }

    fn count(&mut self, input: usize) {
        *self.uses.entry(input).or_insert(0) += 1;
    }

    /// The stream before any fan-out: the input's video, or its normalized audio.
    fn base(&self, input: usize) -> String {
        if self.kind == 'v' {
            format!("{input}:v")
        } else {
            format!("na{input}")
        }
    }

    fn label(&mut self, input: usize) -> String {
        let uses = self.uses.get(&input).copied().unwrap_or(1);
        if uses <= 1 {
            return self.base(input);
        }
        let counter = self.next.entry(input).or_insert(0);
        let label = format!("t{}{input}_{counter}", self.kind);
        *counter += 1;
        label
    }

    /// The `split` / `asplit` filters that fan an input out.
    fn splits(&self) -> Vec<String> {
        let filter = if self.kind == 'v' { "split" } else { "asplit" };
        self.uses
            .iter()
            .filter(|(_, &uses)| uses > 1)
            .map(|(&input, &uses)| {
                let outputs: String = (0..uses)
                    .map(|n| format!("[t{}{input}_{n}]", self.kind))
                    .collect();
                format!("[{}]{filter}={uses}{outputs}", self.base(input))
            })
            .collect()
    }
}

struct Zoom {
    scale: f64,
    ramp_frames: i64,
    start: f64,
    end: f64,
}

fn zoom_of(clip: &Clip) -> Result<Option<Zoom>> {
    let Some(map) = reelctl(&clip.metadata).and_then(|map| map.get(PUNCH_IN_KEY)) else {
        return Ok(None);
    };
    let field = |name: &str| -> Result<f64> {
        let value = match name {
            "start" | "end" => map.get(name).and_then(|time| time.get("value")),
            _ => map.get(name),
        };
        value.and_then(Value::as_f64).ok_or_else(|| {
            TimelineError::invalid(format!(
                "clip \"{}\": metadata.reelctl.{PUNCH_IN_KEY}.{name} is missing or not a number",
                clip.name
            ))
        })
    };
    Ok(Some(Zoom {
        scale: field("scale")?,
        ramp_frames: field("ramp_frames")? as i64,
        start: field("start")?,
        end: field("end")?,
    }))
}

fn fade_ms(clip: &Clip) -> u64 {
    reelctl(&clip.metadata)
        .and_then(|map| map.get(FADE_KEY))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

fn card_of(reference: &GeneratorReference, frames: i64) -> Result<CardSpec> {
    let text = |key: &str| -> Result<String> {
        reference
            .parameters
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                TimelineError::invalid(format!(
                    "the {CARD_KIND} generator has no \"{key}\" parameter"
                ))
            })
    };
    Ok(CardSpec {
        text: text("text")?,
        background: text("background")?,
        foreground: text("foreground")?,
        frames,
    })
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

/// Whether `next` plays on from where `previous` stopped, with no cut between.
fn continues(previous: &Item, next: &Item, rate: f64) -> Result<bool> {
    let (Item::Clip(a), Item::Clip(b)) = (previous, next) else {
        return Ok(false);
    };
    let (Some(url_a), Some(url_b)) = (external_url(a), external_url(b)) else {
        return Ok(false);
    };
    let (start_a, duration_a) = item_range(previous, rate)?;
    let (start_b, _) = item_range(next, rate)?;
    Ok(url_a == url_b && start_a + duration_a == start_b)
}

/// Build the ffmpeg inputs and filter graph for `timeline`. `size` is the output
/// size; `None` is the size of the source media.
pub fn build_render_plan(timeline: &Timeline, size: Option<RenderSize>) -> Result<RenderPlan> {
    let rate = check_editable(timeline)?;
    let info = media_info(timeline)?;
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
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let audio = timeline.track(AUDIO_KIND);
    if video.children.is_empty() {
        return Err(TimelineError::invalid("the video track is empty"));
    }
    let frame_rate = frame_rate_arg(rate);

    let mut media: Vec<String> = Vec::new();
    let mut cards: Vec<CardSpec> = Vec::new();
    // First pass: which inputs, and how many times each is used.
    let mut video_taps = Taps::new('v');
    let mut audio_taps = Taps::new('a');
    for item in &video.children {
        if let Item::Clip(clip) = item {
            if let Some(url) = external_url(clip) {
                let index = media_index(&mut media, url);
                video_taps.count(index);
            }
        }
    }
    if let Some(track) = audio {
        for item in &track.children {
            if let Item::Clip(clip) = item {
                if let Some(url) = external_url(clip) {
                    let index = media_index(&mut media, url);
                    audio_taps.count(index);
                }
            }
        }
    }

    let mut parts: Vec<String> = Vec::new();
    for index in 0..media.len() {
        if audio.is_some() && audio_taps.uses.contains_key(&index) {
            parts.push(format!(
                "[{index}:a]aresample={AUDIO_SAMPLE_RATE},aformat=sample_fmts=fltp:channel_layouts=stereo[na{index}]"
            ));
        }
    }
    parts.extend(video_taps.splits());
    parts.extend(audio_taps.splits());

    let (width, height) = (size.width, size.height);
    let fit = format!("scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1");
    let finish = format!("fps={frame_rate},format=yuv420p");

    let mut video_labels = String::new();
    for (position, item) in video.children.iter().enumerate() {
        let (start, duration) = item_range(item, rate)?;
        let label = format!("sv{position}");
        let Item::Clip(clip) = item else {
            parts.push(format!(
                "color=c=black:s={width}x{height}:r={frame_rate},trim=end_frame={duration},setpts=PTS-STARTPTS,setsar=1,{finish}[{label}]"
            ));
            video_labels.push_str(&format!("[{label}]"));
            continue;
        };
        match clip.active_reference()? {
            MediaReference::External(external) => {
                let index = media_index(&mut media, &external.target_url);
                let tap = video_taps.label(index);
                let mut chain = format!(
                    "[{tap}]trim=start_frame={start}:end_frame={},setpts=PTS-STARTPTS,{fit}",
                    start + duration
                );
                if let Some(zoom) = zoom_of(clip)? {
                    chain.push_str(&format!(",{}", zoompan(&zoom, width, height, &frame_rate)));
                }
                chain.push_str(&format!(",{finish}[{label}]"));
                parts.push(chain);
            }
            MediaReference::Generator(generator) if generator.generator_kind == CARD_KIND => {
                let input = media.len() + cards.len();
                cards.push(card_of(generator, duration)?);
                parts.push(format!(
                    "[{input}:v]trim=end_frame={duration},setpts=PTS-STARTPTS,scale={width}:{height},setsar=1,{finish}[{label}]"
                ));
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
        }
        video_labels.push_str(&format!("[{label}]"));
    }
    parts.push(format!(
        "{video_labels}concat=n={}:v=1:a=0[{VIDEO_LABEL}]",
        video.children.len()
    ));

    let audio_label = match audio {
        Some(track) => {
            let labels = audio_chains(track, &mut media, &mut audio_taps, rate, &mut parts)?;
            parts.push(format!(
                "{labels}concat=n={}:v=0:a=1[{AUDIO_LABEL}]",
                track.children.len()
            ));
            Some(AUDIO_LABEL.to_string())
        }
        None => None,
    };

    Ok(RenderPlan {
        media,
        cards,
        filter_graph: parts.join(";"),
        video_label: VIDEO_LABEL.to_string(),
        audio_label,
        frame_rate,
        fps: rate,
        size,
        frames: crate::tracks::track_frames(video, rate)?,
    })
}

/// `zoompan` that scales by 1 to `scale` and back: up over `ramp_frames` frames from
/// the zoom's start, down over `ramp_frames` frames ending at its last frame.
fn zoompan(zoom: &Zoom, width: u32, height: u32, frame_rate: &str) -> String {
    let ramp = zoom.ramp_frames.max(1);
    let up = format!("clip((on-({}))/{ramp},0,1)", number(zoom.start));
    let down = format!("clip(({}-1-on)/{ramp},0,1)", number(zoom.end));
    format!(
        "zoompan=z='1+({}-1)*min({up},{down})':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=1:s={width}x{height}:fps={frame_rate}",
        number(zoom.scale)
    )
}

fn audio_chains(
    track: &Track,
    media: &mut Vec<String>,
    taps: &mut Taps,
    rate: f64,
    parts: &mut Vec<String>,
) -> Result<String> {
    let mut labels = String::new();
    let format = "aformat=sample_fmts=fltp:channel_layouts=stereo";
    for (position, item) in track.children.iter().enumerate() {
        let (start, duration) = item_range(item, rate)?;
        let label = format!("sa{position}");
        let length = samples(duration, rate);
        let Item::Clip(clip) = item else {
            parts.push(format!(
                "anullsrc=r={AUDIO_SAMPLE_RATE}:cl=stereo,atrim=end_sample={length},asetpts=PTS-STARTPTS,{format}[{label}]"
            ));
            labels.push_str(&format!("[{label}]"));
            continue;
        };
        match clip.active_reference()? {
            MediaReference::External(external) => {
                let index = media_index(media, &external.target_url);
                let tap = taps.label(index);
                let from = samples(start, rate);
                let to = from + length;
                let mut chain = format!(
                    "[{tap}]atrim=start_sample={from}:end_sample={to},asetpts=PTS-STARTPTS"
                );
                let fade = fade_ms(clip);
                if fade > 0 {
                    let fade_s = (fade as f64 / 1000.0).min(seconds(length) / 2.0);
                    let join_before = match position.checked_sub(1) {
                        Some(before) => !continues(&track.children[before], item, rate)?,
                        None => false,
                    };
                    let join_after = match track.children.get(position + 1) {
                        Some(after) => !continues(item, after, rate)?,
                        None => false,
                    };
                    if join_before {
                        chain.push_str(&format!(",afade=t=in:st=0:d={}", number(fade_s)));
                    }
                    if join_after {
                        chain.push_str(&format!(
                            ",afade=t=out:st={}:d={}",
                            number(seconds(length) - fade_s),
                            number(fade_s)
                        ));
                    }
                }
                chain.push_str(&format!(",{format}[{label}]"));
                parts.push(chain);
            }
            MediaReference::Generator(generator) if generator.generator_kind == SILENCE_KIND => {
                parts.push(format!(
                    "anullsrc=r={AUDIO_SAMPLE_RATE}:cl=stereo,atrim=end_sample={length},asetpts=PTS-STARTPTS,{format}[{label}]"
                ));
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
        }
        labels.push_str(&format!("[{label}]"));
    }
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{attach_bridge, BridgeRequest};
    use crate::cut::{cut_silence, CutOptions};
    use crate::punch::{punch_in, PunchRequest};
    use crate::testutil::{dna, project};

    /// 10 s at 30 fps, 640x360: a 500 ms pause cut out at 1.0 s, a punch-in over
    /// 2.0 to 4.0 s of the result, and a bridge at 72 %.
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

    #[test]
    fn the_filter_graph_of_an_edited_project_is_this_text() {
        let plan = build_render_plan(&edited(), None).expect("plan");
        let fit = "scale=640:360:force_original_aspect_ratio=increase,crop=640:360,setsar=1";
        let finish = "fps=30,format=yuv420p";
        let audio = "aformat=sample_fmts=fltp:channel_layouts=stereo";
        let expected = [
            format!("[0:a]aresample=48000,{audio}[na0]"),
            "[0:v]split=4[tv0_0][tv0_1][tv0_2][tv0_3]".to_string(),
            "[na0]asplit=4[ta0_0][ta0_1][ta0_2][ta0_3]".to_string(),
            format!("[tv0_0]trim=start_frame=0:end_frame=30,setpts=PTS-STARTPTS,{fit},{finish}[sv0]"),
            format!("[tv0_1]trim=start_frame=45:end_frame=75,setpts=PTS-STARTPTS,{fit},{finish}[sv1]"),
            format!("[tv0_2]trim=start_frame=75:end_frame=135,setpts=PTS-STARTPTS,{fit},zoompan=z='1+(1.18-1)*min(clip((on-(0))/6,0,1),clip((60-1-on)/6,0,1))':x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':d=1:s=640x360:fps=30,{finish}[sv2]"),
            format!("[tv0_3]trim=start_frame=135:end_frame=220,setpts=PTS-STARTPTS,{fit},{finish}[sv3]"),
            format!("[1:v]trim=end_frame=75,setpts=PTS-STARTPTS,scale=640:360,setsar=1,{finish}[sv4]"),
            "[sv0][sv1][sv2][sv3][sv4]concat=n=5:v=1:a=0[vout]".to_string(),
            format!("[ta0_0]atrim=start_sample=0:end_sample=48000,asetpts=PTS-STARTPTS,afade=t=out:st=0.97:d=0.03,{audio}[sa0]"),
            format!("[ta0_1]atrim=start_sample=72000:end_sample=120000,asetpts=PTS-STARTPTS,afade=t=in:st=0:d=0.03,{audio}[sa1]"),
            format!("[ta0_2]atrim=start_sample=120000:end_sample=216000,asetpts=PTS-STARTPTS,{audio}[sa2]"),
            format!("[ta0_3]atrim=start_sample=216000:end_sample=352000,asetpts=PTS-STARTPTS,afade=t=out:st=2.803333:d=0.03,{audio}[sa3]"),
            format!("anullsrc=r=48000:cl=stereo,atrim=end_sample=120000,asetpts=PTS-STARTPTS,{audio}[sa4]"),
            "[sa0][sa1][sa2][sa3][sa4]concat=n=5:v=0:a=1[aout]".to_string(),
        ]
        .join(";");
        assert_eq!(plan.filter_graph, expected);
        assert_eq!(plan.media, vec!["/media/take.mp4".to_string()]);
        assert_eq!(
            plan.cards,
            vec![CardSpec {
                text: "example.com/go".to_string(),
                background: "#3a1c71".to_string(),
                foreground: "#ffffff".to_string(),
                frames: 75,
            }]
        );
        assert_eq!((plan.frames, plan.frame_rate.as_str()), (280, "30"));
        assert_eq!(plan.audio_label.as_deref(), Some("aout"));
    }

    #[test]
    fn a_join_that_plays_on_has_no_fade() {
        // The punch-in only splits a clip: its pieces meet without a cut.
        let mut timeline = project(30.0, 300);
        let gaps = dna(10_000, 30.0, &[(1_000, 1_500)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        punch_in(
            &mut timeline,
            &PunchRequest {
                start_ms: 2_000,
                end_ms: 4_000,
                scale: 1.5,
            },
        )
        .expect("punch");
        let graph = build_render_plan(&timeline, None)
            .expect("plan")
            .filter_graph;
        assert_eq!(graph.matches("afade=t=in").count(), 1, "{graph}");
        assert_eq!(graph.matches("afade=t=out").count(), 1, "{graph}");
    }

    #[test]
    fn without_cuts_there_are_no_fades() {
        let timeline = project(30.0, 300);
        let graph = build_render_plan(&timeline, None)
            .expect("plan")
            .filter_graph;
        assert!(!graph.contains("afade"), "{graph}");
        assert!(graph.contains("concat=n=1:v=1:a=0[vout]"), "{graph}");
    }

    #[test]
    fn a_fade_longer_than_half_a_clip_is_capped() {
        let mut timeline = project(100.0, 1000);
        let gaps = dna(10_000, 100.0, &[(100, 3_000), (3_050, 9_000)]);
        let options = CutOptions {
            fade_ms: 500,
            ..CutOptions::default()
        };
        cut_silence(&mut timeline, &gaps, &options).expect("cut");
        // The 50 ms of footage between the two cuts cannot take a 500 ms fade.
        let graph = build_render_plan(&timeline, None)
            .expect("plan")
            .filter_graph;
        assert!(graph.contains("afade=t=in:st=0:d=0.025"), "{graph}");
    }

    #[test]
    fn the_size_argument_scales_and_crops_to_fill() {
        let plan = build_render_plan(
            &project(30.0, 300),
            Some(RenderSize {
                width: 1080,
                height: 1920,
            }),
        )
        .expect("plan");
        assert!(plan
            .filter_graph
            .contains("scale=1080:1920:force_original_aspect_ratio=increase,crop=1080:1920"));
        assert_eq!((plan.size.width, plan.size.height), (1080, 1920));
        let odd = build_render_plan(
            &project(30.0, 300),
            Some(RenderSize {
                width: 1081,
                height: 1920,
            }),
        );
        assert!(odd.is_err());
    }

    #[test]
    fn a_project_without_an_audio_track_renders_video_only() {
        let mut timeline = project(30.0, 300);
        timeline
            .tracks
            .children
            .retain(|track| track.kind == VIDEO_KIND);
        let plan = build_render_plan(&timeline, None).expect("plan");
        assert_eq!(plan.audio_label, None);
        assert!(
            !plan.filter_graph.contains("atrim"),
            "{}",
            plan.filter_graph
        );
    }

    #[test]
    fn the_frame_rate_is_written_as_ffmpeg_reads_it() {
        assert_eq!(frame_rate_arg(30.0), "30");
        assert_eq!(frame_rate_arg(25.0), "25");
        assert_eq!(frame_rate_arg(29.97002997), "30000/1001");
        assert_eq!(frame_rate_arg(23.976023976), "24000/1001");
        assert_eq!(frame_rate_arg(59.94005994), "60000/1001");
    }

    #[test]
    fn a_generator_it_does_not_know_is_an_error_that_names_it() {
        let mut timeline = edited();
        let json = timeline
            .to_json()
            .expect("json")
            .replace("reelctl.card", "acme.sparkles");
        timeline = Timeline::from_json(&json).expect("parse");
        let error = build_render_plan(&timeline, None).expect_err("unknown");
        assert!(error.to_string().contains("acme.sparkles"), "{error}");
    }

    #[test]
    fn a_project_from_another_tool_has_no_size_to_render_with() {
        let timeline = Timeline::from_json(include_str!("../tests/reference.otio")).expect("parse");
        assert!(build_render_plan(&timeline, None).is_err());
    }
}
