//! `transform reframe`: a narrower crop window (9:16 out of 16:9) whose left edge
//! follows the presenter, planned from one horizontal position per source frame
//! (the matte's centre of mass, measured outside this crate). Pure: the plan, the
//! keyframes in `.otio` metadata, and the `crop` expression `render` uses.
//!
//! The window only moves sideways. Keyframes are in the media's own frames, so the
//! plan stays right whatever is cut out of the timeline before or after it.

use serde_json::{json, Value};

use crate::error::{Result, TimelineError};
use crate::otio::{reelctl, update_reelctl, Item, Timeline, VIDEO_KIND};
use crate::tracks::{external_url, item_range, project_rate};

/// The key under the timeline's `metadata.reelctl` that holds the plan.
pub const REFRAME_KEY: &str = "reframe";
/// The presenter may move this share of the crop's width (centred) without the
/// window following: the middle 20%.
pub const DEAD_ZONE: f64 = 0.2;
/// The window moves at most this share of its own width per second.
pub const MAX_SPEED: f64 = 0.3;
/// The window takes this long to reach full speed and to come to rest.
pub const EASE_MS: f64 = 250.0;
/// At most one keyframe this often within a shot (plus one at each end).
pub const KEYFRAME_MS: f64 = 500.0;
/// A shot settles on the presenter's median position over its first this-long.
pub const SETTLE_MS: f64 = 500.0;

/// How the window is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Follow {
    Presenter,
    Centre,
}

impl Follow {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Presenter => "presenter",
            Self::Centre => "centre",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "presenter" => Some(Self::Presenter),
            "centre" => Some(Self::Centre),
            _ => None,
        }
    }
}

/// An aspect ratio `width:height`, both above 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aspect {
    pub width: u32,
    pub height: u32,
}

impl Aspect {
    /// `"9:16"` and the like.
    pub fn parse(text: &str) -> Result<Self> {
        let parsed = text.split_once(':').and_then(|(width, height)| {
            Some(Aspect {
                width: width.trim().parse().ok()?,
                height: height.trim().parse().ok()?,
            })
        });
        match parsed {
            Some(aspect) if aspect.width > 0 && aspect.height > 0 => Ok(aspect),
            _ => Err(TimelineError::invalid(format!(
                "--aspect \"{text}\" is not width:height (for example 9:16)"
            ))),
        }
    }

    pub fn label(self) -> String {
        format!("{}:{}", self.width, self.height)
    }
}

/// The largest window of `aspect` that fits a `width` x `height` picture, full
/// height, with an even width; `None` when the picture is already that narrow or
/// narrower (then nothing is cropped).
pub fn crop_size(width: u32, height: u32, aspect: Aspect) -> Option<(u32, u32)> {
    let want = u64::from(height) * u64::from(aspect.width) / u64::from(aspect.height);
    let even = (want / 2 * 2) as u32;
    (even >= 2 && even < width).then_some((even, height - height % 2))
}

/// The window's left edge at one source frame, as a share of the source width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    pub source_frame: i64,
    pub x: f64,
}

/// A reframe as recorded in the project.
#[derive(Debug, Clone, PartialEq)]
pub struct ReframeSpec {
    pub aspect: Aspect,
    pub follow: Follow,
    /// The window in source pixels.
    pub crop_width: u32,
    pub crop_height: u32,
    pub keyframes: Vec<Keyframe>,
    /// Why the plan is what it is (for example a fallback), or empty.
    pub note: String,
}

/// The window held at the centre.
pub fn centre_keyframes(first_frame: i64, source_width: u32, crop_width: u32) -> Vec<Keyframe> {
    vec![Keyframe {
        source_frame: first_frame,
        x: (f64::from(source_width) - f64::from(crop_width)) / 2.0 / f64::from(source_width),
    }]
}

/// The keyframes that follow the presenter. `centroids[i]` is the presenter's
/// horizontal centre at source frame `first_frame + i` as a share of the width
/// (`None` when no presenter was found; the window holds). `cuts` are source frames
/// where a new shot starts: the window jumps there instead of panning.
pub fn plan_path(
    centroids: &[Option<f64>],
    first_frame: i64,
    cuts: &[i64],
    fps: f64,
    source_width: u32,
    crop_width: u32,
) -> Result<Vec<Keyframe>> {
    if centroids.is_empty() {
        return Err(TimelineError::invalid("no frames were measured to reframe"));
    }
    if !fps.is_finite() || fps <= 0.0 {
        return Err(TimelineError::invalid(format!(
            "the frame rate {fps} is not usable"
        )));
    }
    if crop_width == 0 || crop_width >= source_width {
        return Err(TimelineError::invalid(format!(
            "a {crop_width} px window does not fit inside {source_width} px to move"
        )));
    }
    let width = f64::from(source_width);
    let crop = f64::from(crop_width);
    let half = crop / 2.0;
    let dead = DEAD_ZONE / 2.0 * crop;
    let ease_frames = (EASE_MS * fps / 1000.0).max(1.0);
    let max_step = MAX_SPEED * crop / fps;
    let accel = max_step / ease_frames;
    let settle_frames = ((SETTLE_MS * fps / 1000.0).round() as usize).max(1);
    let every = ((KEYFRAME_MS * fps / 1000.0).round() as usize).max(1);
    let clamp = |centre: f64| centre.clamp(half, width - half);

    // Shots: frame offsets where a shot starts.
    let mut starts: Vec<usize> = cuts
        .iter()
        .filter_map(|&cut| usize::try_from(cut - first_frame).ok())
        .filter(|&offset| offset > 0 && offset < centroids.len())
        .collect();
    starts.insert(0, 0);
    starts.sort_unstable();
    starts.dedup();

    let mut keyframes = Vec::new();
    let mut centre = width / 2.0;
    for (shot, &start) in starts.iter().enumerate() {
        let end = starts.get(shot + 1).copied().unwrap_or(centroids.len());
        // Settle on the shot's median position, or keep the last one.
        let mut opening: Vec<f64> = centroids[start..end.min(start + settle_frames)]
            .iter()
            .flatten()
            .map(|share| share * width)
            .collect();
        if !opening.is_empty() {
            opening.sort_by(f64::total_cmp);
            centre = clamp(opening[opening.len() / 2]);
        }
        let mut velocity = 0.0;
        let mut target = centre;
        for offset in start..end {
            if let Some(share) = centroids[offset] {
                target = share * width;
            }
            let error = target - centre;
            let beyond = error.signum() * (error.abs() - dead).max(0.0);
            let desired = (beyond / ease_frames).clamp(-max_step, max_step);
            velocity += (desired - velocity).clamp(-accel, accel);
            let moved = clamp(centre + velocity);
            if moved != centre + velocity {
                velocity = 0.0;
            }
            centre = moved;
            let last = offset + 1 == end;
            if (offset - start) % every == 0 || last {
                keyframes.push(Keyframe {
                    source_frame: first_frame + offset as i64,
                    x: (centre - half) / width,
                });
            }
        }
    }
    Ok(thin(keyframes))
}

/// Drop a keyframe that lies on the straight line between its neighbours (a held
/// or steadily moving window needs only its ends).
fn thin(keyframes: Vec<Keyframe>) -> Vec<Keyframe> {
    if keyframes.len() < 3 {
        return keyframes;
    }
    let mut kept = vec![keyframes[0]];
    for index in 1..keyframes.len() - 1 {
        let (before, here, after) = (kept[kept.len() - 1], keyframes[index], keyframes[index + 1]);
        let span = (after.source_frame - before.source_frame) as f64;
        let along = (here.source_frame - before.source_frame) as f64 / span;
        let line = before.x + (after.x - before.x) * along;
        if (line - here.x).abs() > 1e-4 {
            kept.push(here);
        }
    }
    kept.push(keyframes[keyframes.len() - 1]);
    kept
}

/// The window's left edge at `frame`, by straight lines between keyframes and held
/// before the first and after the last.
pub fn x_at(keyframes: &[Keyframe], frame: i64) -> f64 {
    let Some(first) = keyframes.first() else {
        return 0.0;
    };
    let mut x = first.x;
    for pair in keyframes.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let span = (b.source_frame - a.source_frame).max(1) as f64;
        let along = ((frame - a.source_frame) as f64 / span).clamp(0.0, 1.0);
        x += (b.x - a.x) * along;
    }
    x
}

fn number(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" || text.is_empty() {
        "0".to_string()
    } else {
        text.to_string()
    }
}

/// The `crop` filter's `x` for a piece that starts at source frame `start` and
/// lasts `duration` frames: the same lines as [`x_at`], in pixels, over ffmpeg's
/// frame counter `n` (which starts at 0 after the piece's `trim`). Only the
/// keyframes that bear on the piece are written, as a flat sum (no nesting).
pub fn crop_x_expression(
    keyframes: &[Keyframe],
    start: i64,
    duration: i64,
    source_width: u32,
) -> String {
    let width = f64::from(source_width);
    let end = start + duration;
    let first = keyframes
        .iter()
        .rposition(|key| key.source_frame <= start)
        .unwrap_or(0);
    let last = keyframes
        .iter()
        .position(|key| key.source_frame >= end)
        .unwrap_or(keyframes.len().saturating_sub(1));
    if keyframes.is_empty() {
        return "0".to_string();
    }
    let relevant = &keyframes[first..=last.max(first)];
    let mut expression = number(x_at(keyframes, start) * width);
    let at_start = x_at(keyframes, start);
    let mut previous = (start, at_start);
    for key in relevant.iter().skip_while(|key| key.source_frame <= start) {
        let (from_frame, from_x) = previous;
        let to_frame = key.source_frame.min(end);
        let to_x = if key.source_frame <= end {
            key.x
        } else {
            x_at(keyframes, end)
        };
        let delta = (to_x - from_x) * width;
        if delta.abs() >= 1e-3 && to_frame > from_frame {
            expression.push_str(&format!(
                "+{}*clip((n-{})/{},0,1)",
                number(delta),
                from_frame - start,
                to_frame - from_frame
            ));
        }
        previous = (to_frame, to_x);
        if key.source_frame >= end {
            break;
        }
    }
    expression
}

/// The source frames the project's footage spans: the first frame any video clip
/// plays to one past the last. A reframe measures this span.
pub fn footage_span(timeline: &Timeline) -> Result<(i64, i64)> {
    let rate = project_rate(timeline)?;
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let mut span: Option<(i64, i64)> = None;
    for item in &video.children {
        if matches!(item, Item::Clip(clip) if external_url(clip).is_some()) {
            let (start, duration) = item_range(item, rate)?;
            if duration > 0 {
                span = Some(match span {
                    None => (start, start + duration),
                    Some((from, to)) => (from.min(start), to.max(start + duration)),
                });
            }
        }
    }
    span.ok_or_else(|| TimelineError::invalid("the project has no footage to reframe"))
}

/// Record `spec` in the project, replacing any earlier reframe.
pub fn set_reframe(timeline: &mut Timeline, spec: &ReframeSpec) {
    let keyframes: Vec<Value> = spec
        .keyframes
        .iter()
        .map(|key| json!({"source_frame": key.source_frame, "x": key.x}))
        .collect();
    update_reelctl(&mut timeline.metadata, |map| {
        map.insert(
            REFRAME_KEY.to_string(),
            json!({
                "aspect": spec.aspect.label(),
                "follow": spec.follow.as_str(),
                "crop_width": spec.crop_width,
                "crop_height": spec.crop_height,
                "keyframes": keyframes,
                "note": spec.note,
            }),
        );
    });
}

/// The reframe recorded in the project, if any. A record that does not parse is an
/// error, not "none": rendering it uncropped would be the wrong picture.
pub fn reframe_of(timeline: &Timeline) -> Result<Option<ReframeSpec>> {
    let Some(value) = reelctl(&timeline.metadata).and_then(|map| map.get(REFRAME_KEY)) else {
        return Ok(None);
    };
    let bad = || {
        TimelineError::invalid(format!(
            "metadata.reelctl.{REFRAME_KEY} is not a reframe this tool wrote"
        ))
    };
    let aspect = Aspect::parse(
        value
            .get("aspect")
            .and_then(Value::as_str)
            .ok_or_else(bad)?,
    )?;
    let follow = value
        .get("follow")
        .and_then(Value::as_str)
        .and_then(Follow::parse)
        .ok_or_else(bad)?;
    let side = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|side| u32::try_from(side).ok())
            .filter(|side| *side > 0)
            .ok_or_else(bad)
    };
    let (crop_width, crop_height) = (side("crop_width")?, side("crop_height")?);
    let mut keyframes = Vec::new();
    for key in value
        .get("keyframes")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
    {
        let source_frame = key
            .get("source_frame")
            .and_then(Value::as_i64)
            .ok_or_else(bad)?;
        let x = key
            .get("x")
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && (0.0..=1.0).contains(x))
            .ok_or_else(bad)?;
        if keyframes
            .last()
            .is_some_and(|last: &Keyframe| last.source_frame >= source_frame)
        {
            return Err(bad());
        }
        keyframes.push(Keyframe { source_frame, x });
    }
    if keyframes.is_empty() {
        return Err(bad());
    }
    Ok(Some(ReframeSpec {
        aspect,
        follow,
        crop_width,
        crop_height,
        keyframes,
        note: value
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::project;

    const W: u32 = 1280;
    const CROP: u32 = 404; // 720 * 9 / 16 = 405, made even

    fn shares(positions: impl Iterator<Item = f64>) -> Vec<Option<f64>> {
        positions.map(Some).collect()
    }

    #[test]
    fn the_window_is_full_height_and_even() {
        let nine_sixteen = Aspect::parse("9:16").unwrap();
        assert_eq!(crop_size(1280, 720, nine_sixteen), Some((404, 720)));
        assert_eq!(crop_size(1920, 1080, nine_sixteen), Some((606, 1080)));
        // Already vertical: nothing to crop.
        assert_eq!(crop_size(1080, 1920, nine_sixteen), None);
        assert_eq!(crop_size(404, 720, nine_sixteen), None);
        assert!(Aspect::parse("9x16").is_err());
        assert!(Aspect::parse("0:16").is_err());
    }

    #[test]
    fn a_still_presenter_off_centre_gets_one_settled_window() {
        let centroids = vec![Some(0.25); 300];
        let keys = plan_path(&centroids, 100, &[], 30.0, W, CROP).unwrap();
        // Centre at 320 px, left edge 320 - 202 = 118 px.
        let want = (0.25 * f64::from(W) - f64::from(CROP) / 2.0) / f64::from(W);
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(
            keys.iter().all(|key| (key.x - want).abs() < 1e-9),
            "{keys:?}"
        );
        assert_eq!(keys[0].source_frame, 100);
        assert_eq!(keys[1].source_frame, 399);
    }

    #[test]
    fn small_moves_inside_the_dead_zone_are_ignored() {
        // +-30 px around the centre is inside 20% of 404 px (+-40 px).
        let centroids = shares((0..300).map(|frame| {
            let wobble = if (frame / 10) % 2 == 0 { 30.0 } else { -30.0 };
            (640.0 + wobble) / f64::from(W)
        }));
        let keys = plan_path(&centroids, 0, &[], 30.0, W, CROP).unwrap();
        let first = keys[0].x;
        assert!(
            keys.iter().all(|key| (key.x - first).abs() < 0.04),
            "{keys:?}"
        );
    }

    /// Per-frame path through the keyframes, in source pixels of the window centre.
    fn centres(keys: &[Keyframe], frames: i64) -> Vec<f64> {
        (0..frames)
            .map(|frame| x_at(keys, frame) * f64::from(W) + f64::from(CROP) / 2.0)
            .collect()
    }

    #[test]
    fn a_walking_presenter_stays_in_the_middle_40_percent_and_the_pan_is_limited() {
        // From 300 px to 980 px over 10 s at 30 fps: 68 px/s, about 0.17 of the
        // window per second, under the 0.3 limit.
        let frames = 300;
        let position = |frame: i64| 300.0 + 680.0 * frame as f64 / frames as f64;
        let centroids = shares((0..frames).map(|frame| position(frame) / f64::from(W)));
        let keys = plan_path(&centroids, 0, &[], 30.0, W, CROP).unwrap();
        let path = centres(&keys, frames);
        for (frame, centre) in path.iter().enumerate() {
            let off = (position(frame as i64) - centre).abs();
            assert!(off <= 0.2 * f64::from(CROP), "frame {frame}: {off} px off");
        }
        let limit = MAX_SPEED * f64::from(CROP) / 30.0 + 1e-6;
        for pair in path.windows(2) {
            assert!(
                (pair[1] - pair[0]).abs() <= limit,
                "{} px in one frame",
                pair[1] - pair[0]
            );
        }
        assert!(keys.len() < 30, "{} keyframes", keys.len());
    }

    #[test]
    fn at_a_cut_the_window_jumps_instead_of_panning() {
        // Left third for 3 s, then right third after a cut at frame 90.
        let centroids = shares((0..180).map(|frame| if frame < 90 { 0.2 } else { 0.8 }));
        let keys = plan_path(&centroids, 0, &[90], 30.0, W, CROP).unwrap();
        let before = x_at(&keys, 89) * f64::from(W) + f64::from(CROP) / 2.0;
        let after = x_at(&keys, 90) * f64::from(W) + f64::from(CROP) / 2.0;
        assert!((before - 0.2 * f64::from(W)).abs() < 1.0, "{before}");
        assert!((after - 0.8 * f64::from(W)).abs() < 1.0, "{after}");
    }

    #[test]
    fn without_a_cut_the_same_move_is_a_limited_pan() {
        let centroids = shares((0..180).map(|frame| if frame < 90 { 0.2 } else { 0.8 }));
        let keys = plan_path(&centroids, 0, &[], 30.0, W, CROP).unwrap();
        let path = centres(&keys, 180);
        let limit = MAX_SPEED * f64::from(CROP) / 30.0 + 1e-6;
        assert!(path
            .windows(2)
            .all(|pair| (pair[1] - pair[0]).abs() <= limit));
        assert!(path[95] < 0.5 * f64::from(W));
    }

    #[test]
    fn the_window_never_leaves_the_picture_and_holds_when_nobody_is_there() {
        let mut centroids = shares((0..120).map(|_| 0.01));
        centroids.extend((0..60).map(|_| None));
        let keys = plan_path(&centroids, 0, &[], 30.0, W, CROP).unwrap();
        for frame in 0..180 {
            let x = x_at(&keys, frame);
            assert!(x >= -1e-9 && x * f64::from(W) + f64::from(CROP) <= f64::from(W) + 1e-6);
        }
        assert!((x_at(&keys, 179) - x_at(&keys, 119)).abs() < 1e-9);
        // Nobody at all: the centre.
        let keys = plan_path(&[None; 30], 0, &[], 30.0, W, CROP).unwrap();
        assert!((x_at(&keys, 0) - centre_keyframes(0, W, CROP)[0].x).abs() < 1e-9);
    }

    #[test]
    fn the_expression_matches_the_lines_at_every_frame() {
        let keys = vec![
            Keyframe {
                source_frame: 100,
                x: 0.1,
            },
            Keyframe {
                source_frame: 130,
                x: 0.4,
            },
            Keyframe {
                source_frame: 131,
                x: 0.6,
            },
            Keyframe {
                source_frame: 200,
                x: 0.5,
            },
        ];
        // A piece from source frame 110 to 160.
        let expression = crop_x_expression(&keys, 110, 50, W);
        assert!(!expression.contains("if("), "{expression}");
        for n in 0..50i64 {
            let want = x_at(&keys, 110 + n) * f64::from(W);
            let got = eval(&expression, n as f64);
            assert!(
                (want - got).abs() < 0.01,
                "n={n}: {want} vs {got} in {expression}"
            );
        }
        // A piece after every keyframe holds the last.
        assert_eq!(
            crop_x_expression(&keys, 300, 10, W),
            number(0.5 * f64::from(W))
        );
    }

    /// Evaluates the `a+b*clip((n-c)/d,0,1)+...` sums `crop_x_expression` writes.
    fn eval(expression: &str, n: f64) -> f64 {
        let mut parts = expression.split('+');
        let mut total: f64 = parts.next().unwrap().parse().unwrap();
        for part in parts {
            let (scale, rest) = part.split_once("*clip((n-").unwrap();
            let (offset, rest) = rest.split_once(")/").unwrap();
            let (span, _) = rest.split_once(",0,1)").unwrap();
            let (scale, offset, span): (f64, f64, f64) = (
                scale.parse().unwrap(),
                offset.parse().unwrap(),
                span.parse().unwrap(),
            );
            total += scale * ((n - offset) / span).clamp(0.0, 1.0);
        }
        total
    }

    #[test]
    fn the_footage_span_covers_every_clip_after_edits() {
        let mut timeline = project(100.0, 1000);
        let gaps = crate::testutil::dna(10_000, 100.0, &[(3_000, 4_000)]);
        crate::cut_silence(&mut timeline, &gaps, &crate::CutOptions::default()).unwrap();
        crate::append_bridge(&mut timeline, "https://example.com", 1_000).unwrap();
        assert_eq!(footage_span(&timeline).unwrap(), (0, 1000));
    }

    #[test]
    fn the_plan_reads_back_and_a_bad_record_is_an_error() {
        let mut timeline = project(30.0, 300);
        assert_eq!(reframe_of(&timeline).unwrap(), None);
        let spec = ReframeSpec {
            aspect: Aspect::parse("9:16").unwrap(),
            follow: Follow::Presenter,
            crop_width: 202,
            crop_height: 360,
            keyframes: vec![
                Keyframe {
                    source_frame: 0,
                    x: 0.1,
                },
                Keyframe {
                    source_frame: 15,
                    x: 0.2,
                },
            ],
            note: String::new(),
        };
        set_reframe(&mut timeline, &spec);
        assert_eq!(reframe_of(&timeline).unwrap(), Some(spec));
        update_reelctl(&mut timeline.metadata, |map| {
            map.insert(REFRAME_KEY.to_string(), json!({"aspect": "9:16"}));
        });
        assert!(reframe_of(&timeline).is_err());
    }
}
