//! `transform punch-in`: a centred zoom on a span of the video.

use serde_json::{json, Value};

use crate::error::{Result, TimelineError};
use crate::otio::{reelctl, update_reelctl, Clip, Item, RationalTime, Timeline, VIDEO_KIND};
use crate::tracks::{
    check_editable, external_url, frame_ms, item_range, round_frame, split_at, timeline_frames,
    PUNCH_IN_KEY,
};

/// The largest zoom factor.
pub const MAX_SCALE: f64 = 2.0;
/// Frames the zoom takes to reach its scale, and to leave it.
pub const RAMP_FRAMES: i64 = 6;

/// Arguments of `punch-in`, in milliseconds of the edited timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct PunchRequest {
    pub start_ms: u64,
    pub end_ms: u64,
    pub scale: f64,
}

/// What a punch-in changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PunchReport {
    /// First and one-past-last timeline frame of the zoom.
    pub start_frame: i64,
    pub end_frame: i64,
    /// Number of video clips that carry the zoom.
    pub clips: usize,
}

/// Zoom the video from 1.0 to `scale` over [`RAMP_FRAMES`] frames at `start_ms`,
/// hold it, and bring it back over the last [`RAMP_FRAMES`] frames before `end_ms`.
/// The clips under the span are split at its ends, so the zoom is a property of
/// whole clips; the audio is split in step and left alone.
///
/// A punch-in replaces any earlier one it overlaps, on the frames they share. The
/// earlier one ends (or, past the new span, starts) cleanly at the new span's edge:
/// it ramps back to 1.0 there, so the zoom never jumps.
pub fn punch_in(timeline: &mut Timeline, request: &PunchRequest) -> Result<PunchReport> {
    let rate = check_editable(timeline)?;
    if !request.scale.is_finite() || request.scale <= 1.0 || request.scale > MAX_SCALE {
        return Err(TimelineError::invalid(format!(
            "--scale {} must be above 1 and at most {MAX_SCALE}",
            request.scale
        )));
    }
    let start = round_frame(request.start_ms, rate);
    let end = round_frame(request.end_ms, rate);
    if end - start < 2 * RAMP_FRAMES {
        return Err(TimelineError::invalid(format!(
            "the span {}..{} ms is {} frames at {rate} fps; a punch-in needs at least {} (a ramp in and a ramp out)",
            request.start_ms,
            request.end_ms,
            (end - start).max(0),
            2 * RAMP_FRAMES
        )));
    }
    let total = timeline_frames(timeline)?;
    if end > total {
        return Err(TimelineError::invalid(format!(
            "the span ends at {} ms but the timeline is {} ms long",
            request.end_ms,
            frame_ms(total, rate)
        )));
    }

    let mut work = timeline.clone();
    for track in &mut work.tracks.children {
        split_at(track, start, rate)?;
        split_at(track, end, rate)?;
    }
    let mut clips = 0;
    let mut position = 0;
    if let Some(video) = work.track_mut(VIDEO_KIND) {
        for index in 0..video.children.len() {
            let (_, duration) = item_range(&video.children[index], rate)?;
            let inside = position >= start && position + duration <= end;
            if let Item::Clip(clip) = &mut video.children[index] {
                if inside && external_url(clip).is_some() {
                    let zoom = json!({
                        "scale": request.scale,
                        "ramp_frames": RAMP_FRAMES,
                        "start": time_value(start - position, rate)?,
                        "end": time_value(end - position, rate)?,
                    });
                    update_reelctl(&mut clip.metadata, |map| {
                        map.insert(PUNCH_IN_KEY.to_string(), zoom);
                    });
                    clips += 1;
                } else if !inside {
                    trim_earlier(clip, position, duration, (start, end), rate)?;
                }
            }
            position += duration;
        }
    }
    if clips == 0 {
        return Err(TimelineError::invalid(format!(
            "no footage lies in {}..{} ms (a card or a gap does not zoom)",
            request.start_ms, request.end_ms
        )));
    }
    check_editable(&work)?;
    *timeline = work;
    Ok(PunchReport {
        start_frame: start,
        end_frame: end,
        clips,
    })
}

/// A piece outside the new span `(start, end)` that carries an earlier punch-in
/// running into the span: the earlier one now ends at `start` (for a piece before
/// the span) or starts at `end` (for a piece after it). The times are in the
/// piece's own frames.
fn trim_earlier(
    clip: &mut Clip,
    position: i64,
    duration: i64,
    (start, end): (i64, i64),
    rate: f64,
) -> Result<()> {
    let time_of = |key: &str| -> Option<f64> {
        reelctl(&clip.metadata)?
            .get(PUNCH_IN_KEY)?
            .get(key)?
            .get("value")?
            .as_f64()
    };
    let (Some(old_start), Some(old_end)) = (time_of("start"), time_of("end")) else {
        return Ok(());
    };
    let (key, frames) = if position + duration <= start && position as f64 + old_end > start as f64
    {
        ("end", start - position)
    } else if position >= end && (position as f64 + old_start) < end as f64 {
        ("start", end - position)
    } else {
        return Ok(());
    };
    let value = time_value(frames, rate)?;
    update_reelctl(&mut clip.metadata, |map| {
        if let Some(Value::Object(punch)) = map.get_mut(PUNCH_IN_KEY) {
            punch.insert(key.to_string(), value);
        }
    });
    Ok(())
}

fn time_value(frames: i64, rate: f64) -> Result<Value> {
    Ok(serde_json::to_value(RationalTime::new(
        frames as f64,
        rate,
    ))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cut::{cut_silence, CutOptions};
    use crate::otio::{reelctl, AUDIO_KIND};
    use crate::testutil::{dna, project};

    fn request(start_ms: u64, end_ms: u64, scale: f64) -> PunchRequest {
        PunchRequest {
            start_ms,
            end_ms,
            scale,
        }
    }

    fn zoom_of(item: &Item) -> Option<Value> {
        reelctl(item.metadata())
            .and_then(|map| map.get(PUNCH_IN_KEY))
            .cloned()
    }

    fn ranges(timeline: &Timeline, kind: &str) -> Vec<(i64, i64)> {
        timeline
            .track(kind)
            .expect("track")
            .children
            .iter()
            .map(|item| item_range(item, 100.0).expect("range"))
            .collect()
    }

    #[test]
    fn a_span_in_the_middle_splits_the_clip_in_three_and_marks_the_middle() {
        let mut timeline = project(100.0, 1000);
        let report = punch_in(&mut timeline, &request(2_000, 4_000, 1.18)).expect("punch");
        assert_eq!(
            (report.start_frame, report.end_frame, report.clips),
            (200, 400, 1)
        );
        assert_eq!(
            ranges(&timeline, VIDEO_KIND),
            vec![(0, 200), (200, 200), (400, 600)]
        );
        assert_eq!(ranges(&timeline, AUDIO_KIND), ranges(&timeline, VIDEO_KIND));
        let video = timeline.track(VIDEO_KIND).expect("video");
        assert_eq!(zoom_of(&video.children[0]), None);
        assert_eq!(zoom_of(&video.children[2]), None);
        let zoom = zoom_of(&video.children[1]).expect("zoom");
        assert_eq!(zoom["scale"], 1.18);
        assert_eq!(zoom["ramp_frames"], 6);
        assert_eq!(zoom["start"]["value"], 0.0);
        assert_eq!(zoom["end"]["value"], 200.0);
        let audio = timeline.track(AUDIO_KIND).expect("audio");
        assert!(audio.children.iter().all(|item| zoom_of(item).is_none()));
    }

    #[test]
    fn a_span_at_the_start_makes_two_pieces() {
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(0, 1_000, 1.5)).expect("punch");
        assert_eq!(ranges(&timeline, VIDEO_KIND), vec![(0, 100), (100, 900)]);
    }

    #[test]
    fn a_span_over_a_join_marks_each_clip_with_the_same_zoom_in_its_own_time() {
        let mut timeline = project(100.0, 1000);
        let gaps = dna(10_000, 100.0, &[(2_000, 2_500)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        // Clips: 0..200 and 250..1000 of the media, at timeline 0..200 and 200..950.
        let report = punch_in(&mut timeline, &request(1_500, 3_000, 1.2)).expect("punch");
        assert_eq!(report.clips, 2);
        let video = timeline.track(VIDEO_KIND).expect("video");
        // [0,150) plain, [150,200) zoomed, [200,300) zoomed, [300,..) plain.
        assert_eq!(video.children.len(), 4);
        let first = zoom_of(&video.children[1]).expect("first zoom");
        let second = zoom_of(&video.children[2]).expect("second zoom");
        // The span is 150..300 on the timeline. In the first piece (at 150) it
        // starts at 0 and ends at 150; in the second (at 200) it started at -50
        // and ends at 100: one ramp across the join.
        assert_eq!(first["start"]["value"], 0.0);
        assert_eq!(first["end"]["value"], 150.0);
        assert_eq!(second["start"]["value"], -50.0);
        assert_eq!(second["end"]["value"], 100.0);
    }

    #[test]
    fn a_zoom_on_a_clip_survives_a_later_cut_in_the_footage() {
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(1_000, 6_000, 1.3)).expect("punch");
        let gaps = dna(10_000, 100.0, &[(3_000, 3_500)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        let video = timeline.track(VIDEO_KIND).expect("video");
        // 0..100 plain, 100..300 zoomed, 350..600 zoomed, 600.. plain.
        let left = zoom_of(&video.children[1]).expect("left");
        let right = zoom_of(&video.children[2]).expect("right");
        assert_eq!(left["start"]["value"], 0.0);
        assert_eq!(left["end"]["value"], 500.0);
        // The right piece starts 250 frames into the original zoomed clip.
        assert_eq!(right["start"]["value"], -250.0);
        assert_eq!(right["end"]["value"], 250.0);
    }

    #[test]
    fn the_scale_must_be_above_one_and_at_most_two() {
        for scale in [1.0, 0.9, 2.01, f64::NAN, f64::INFINITY] {
            let mut timeline = project(100.0, 1000);
            let error =
                punch_in(&mut timeline, &request(1_000, 3_000, scale)).expect_err("bad scale");
            assert!(error.to_string().contains("--scale"), "{scale}: {error}");
        }
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(1_000, 3_000, 2.0)).expect("two is allowed");
    }

    #[test]
    fn a_span_shorter_than_twelve_frames_is_refused() {
        let mut timeline = project(100.0, 1000);
        let before = timeline.to_json().expect("json");
        let error = punch_in(&mut timeline, &request(1_000, 1_110, 1.2)).expect_err("short");
        assert!(error.to_string().contains("12"), "{error}");
        assert_eq!(timeline.to_json().expect("json"), before);
        punch_in(&mut timeline, &request(1_000, 1_120, 1.2)).expect("12 frames is enough");
    }

    #[test]
    fn a_span_that_ends_before_it_starts_or_past_the_end_is_refused() {
        let mut timeline = project(100.0, 1000);
        assert!(punch_in(&mut timeline, &request(3_000, 1_000, 1.2)).is_err());
        let error = punch_in(&mut timeline, &request(9_000, 10_500, 1.2)).expect_err("past");
        assert!(error.to_string().contains("10500"), "{error}");
    }

    #[test]
    fn a_second_run_with_the_same_arguments_changes_nothing() {
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(2_000, 4_000, 1.18)).expect("punch");
        let once = timeline.to_json().expect("json");
        punch_in(&mut timeline, &request(2_000, 4_000, 1.18)).expect("again");
        assert_eq!(timeline.to_json().expect("json"), once);
    }

    #[test]
    fn times_round_to_the_nearest_frame() {
        let mut timeline = project(100.0, 1000);
        let report = punch_in(&mut timeline, &request(2_004, 3_996, 1.2)).expect("punch");
        assert_eq!((report.start_frame, report.end_frame), (200, 400));
    }

    /// The zoom factor at timeline `frame`, from the recorded punch-ins, with the
    /// formula the renderer's `zoompan` uses (`on` counts frames of the piece).
    fn scale_at(timeline: &Timeline, frame: i64) -> f64 {
        let video = timeline.track(VIDEO_KIND).expect("video");
        let mut position = 0;
        for item in &video.children {
            let (_, duration) = item_range(item, 100.0).expect("range");
            if frame < position + duration {
                let Some(zoom) = zoom_of(item) else {
                    return 1.0;
                };
                let number = |value: &Value| value.as_f64().expect("number");
                let scale = number(&zoom["scale"]);
                let ramp = number(&zoom["ramp_frames"]);
                let start = number(&zoom["start"]["value"]);
                let end = number(&zoom["end"]["value"]);
                let on = (frame - position) as f64;
                let up = ((on - start) / ramp).clamp(0.0, 1.0);
                let down = ((end - 1.0 - on) / ramp).clamp(0.0, 1.0);
                return 1.0 + (scale - 1.0) * up.min(down);
            }
            position += duration;
        }
        1.0
    }

    /// The largest change in zoom between two neighbouring frames.
    fn largest_step(timeline: &Timeline) -> f64 {
        let total = timeline_frames(timeline).expect("frames");
        (1..total)
            .map(|frame| (scale_at(timeline, frame) - scale_at(timeline, frame - 1)).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn a_partly_overlapping_second_punch_in_does_not_jump() {
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(2_000, 4_000, 1.5)).expect("first");
        punch_in(&mut timeline, &request(3_000, 5_000, 1.5)).expect("second");
        // No frame-to-frame step is bigger than one ramp step, (1.5 - 1) / 6.
        let step = largest_step(&timeline);
        assert!(step <= 0.5 / 6.0 + 1e-9, "a step of {step}");
        // The first ends cleanly where the second begins, and the second holds
        // its full scale.
        assert_eq!(scale_at(&timeline, 299), 1.0);
        assert_eq!(scale_at(&timeline, 300), 1.0);
        assert_eq!(scale_at(&timeline, 400), 1.5);
        assert_eq!(scale_at(&timeline, 499), 1.0);
        // The first still holds its scale before its own ramp out.
        assert_eq!(scale_at(&timeline, 250), 1.5);
        assert_eq!(scale_at(&timeline, 204), 1.0 + 0.5 * 4.0 / 6.0);
    }

    #[test]
    fn a_later_punch_in_in_the_middle_of_an_earlier_one_splits_it_cleanly() {
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(2_000, 6_000, 1.4)).expect("first");
        punch_in(&mut timeline, &request(3_000, 4_000, 1.8)).expect("second");
        let step = largest_step(&timeline);
        assert!(step <= 0.8 / 6.0 + 1e-9, "a step of {step}");
        assert_eq!(scale_at(&timeline, 250), 1.4);
        assert_eq!(scale_at(&timeline, 299), 1.0);
        assert_eq!(scale_at(&timeline, 350), 1.8);
        assert_eq!(scale_at(&timeline, 400), 1.0);
        assert_eq!(scale_at(&timeline, 500), 1.4);
        assert_eq!(scale_at(&timeline, 599), 1.0);
    }

    #[test]
    fn a_later_punch_in_over_an_earlier_one_replaces_it_where_they_overlap() {
        // The same span with another scale replaces the first on every frame.
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(2_000, 4_000, 1.5)).expect("first");
        punch_in(&mut timeline, &request(2_000, 4_000, 1.2)).expect("second");
        assert_eq!(scale_at(&timeline, 300), 1.2);
        assert!(largest_step(&timeline) <= 0.2 / 6.0 + 1e-9);
    }

    #[test]
    fn overlapping_punch_ins_across_a_join_do_not_jump_either() {
        let mut timeline = project(100.0, 1000);
        let gaps = dna(10_000, 100.0, &[(2_500, 3_000)]);
        cut_silence(&mut timeline, &gaps, &CutOptions::default()).expect("cut");
        punch_in(&mut timeline, &request(2_000, 4_000, 1.5)).expect("first");
        punch_in(&mut timeline, &request(3_000, 5_000, 1.3)).expect("second");
        let step = largest_step(&timeline);
        assert!(step <= 0.5 / 6.0 + 1e-9, "a step of {step}");
    }

    #[test]
    fn the_same_overlapping_pair_twice_gives_the_same_project() {
        let mut timeline = project(100.0, 1000);
        punch_in(&mut timeline, &request(2_000, 4_000, 1.5)).expect("first");
        punch_in(&mut timeline, &request(3_000, 5_000, 1.5)).expect("second");
        let once = timeline.to_json().expect("json");
        punch_in(&mut timeline, &request(3_000, 5_000, 1.5)).expect("again");
        assert_eq!(timeline.to_json().expect("json"), once);
    }
}
