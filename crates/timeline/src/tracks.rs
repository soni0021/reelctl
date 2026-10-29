//! Frame arithmetic over the tracks of a project: ranges, durations, and splitting
//! an item at a timeline frame. Everything is in whole frames at the project rate.

use serde_json::{Map, Value};

use crate::error::{Result, TimelineError};
use crate::otio::{
    reelctl, update_reelctl, Clip, Item, MediaReference, RationalTime, TimeRange, Timeline, Track,
    AUDIO_KIND, VIDEO_KIND,
};

const FRAME_EPSILON: f64 = 1e-6;

/// The largest frame count or frame offset a project value may have. A hand-edited
/// project with a larger one is refused, so no sum of them can overflow.
pub const MAX_FRAMES: i64 = i32::MAX as i64;

/// The metadata key of a punch-in on a clip, under `metadata.reelctl`.
pub const PUNCH_IN_KEY: &str = "punch_in";
/// The metadata key of the audio fade length on a clip, under `metadata.reelctl`.
pub const FADE_KEY: &str = "fade_ms";
/// The fade `cut-silence` and `attach-bridge` use unless told another.
pub const DEFAULT_FADE_MS: u64 = 30;

/// The frame at or after `ms`, on the grid of `rate` frames per second.
pub fn ceil_frame(ms: u64, rate: f64) -> i64 {
    (ms as f64 * rate / 1000.0 - FRAME_EPSILON).ceil() as i64
}

/// The frame at or before `ms`.
pub fn floor_frame(ms: u64, rate: f64) -> i64 {
    (ms as f64 * rate / 1000.0 + FRAME_EPSILON).floor() as i64
}

/// The nearest frame to `ms`.
pub fn round_frame(ms: u64, rate: f64) -> i64 {
    (ms as f64 * rate / 1000.0).round() as i64
}

/// The time of `frame` in milliseconds, rounded.
pub fn frame_ms(frame: i64, rate: f64) -> u64 {
    (frame.max(0) as f64 * 1000.0 / rate).round() as u64
}

/// A time as a whole number of frames at `rate`.
pub(crate) fn frames_of(time: &RationalTime, rate: f64, what: &str) -> Result<i64> {
    if (time.rate - rate).abs() > 1e-9 {
        return Err(TimelineError::invalid(format!(
            "{what}: rate {} differs from the project rate {rate}",
            time.rate
        )));
    }
    let rounded = time.value.round();
    if !time.value.is_finite() || (time.value - rounded).abs() > FRAME_EPSILON {
        return Err(TimelineError::invalid(format!(
            "{what}: {} is not a whole number of frames",
            time.value
        )));
    }
    let frames = rounded as i64;
    if frames.abs() > MAX_FRAMES {
        return Err(TimelineError::invalid(format!(
            "{what}: {frames} frames is out of range (at most {MAX_FRAMES})"
        )));
    }
    Ok(frames)
}

/// `(start, duration)` of an item's `source_range`, in frames.
pub(crate) fn item_range(item: &Item, rate: f64) -> Result<(i64, i64)> {
    let name = match item {
        Item::Clip(clip) => format!("clip \"{}\"", clip.name),
        Item::Gap(_) => "gap".to_string(),
    };
    let range = item
        .source_range()
        .ok_or_else(|| TimelineError::invalid(format!("{name} has no source_range")))?;
    let start = frames_of(&range.start_time, rate, &format!("{name} start_time"))?;
    let duration = frames_of(&range.duration, rate, &format!("{name} duration"))?;
    if duration < 0 {
        return Err(TimelineError::invalid(format!(
            "{name} has a negative duration"
        )));
    }
    Ok((start, duration))
}

/// The frame rate of the project: the rate of the first video item.
pub fn project_rate(timeline: &Timeline) -> Result<f64> {
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let rate = video
        .children
        .iter()
        .find_map(|item| item.source_range().map(|range| range.start_time.rate))
        .ok_or_else(|| TimelineError::invalid("the video track has no clip with a source_range"))?;
    if !rate.is_finite() || rate <= 0.0 {
        return Err(TimelineError::invalid(format!(
            "the video track's frame rate is {rate}; it must be above 0"
        )));
    }
    Ok(rate)
}

/// Length of a track in frames.
pub fn track_frames(track: &Track, rate: f64) -> Result<i64> {
    let mut total: i64 = 0;
    for item in &track.children {
        total = total
            .checked_add(item_range(item, rate)?.1)
            .ok_or_else(|| {
                TimelineError::invalid("the track is out of range: its length overflows")
            })?;
    }
    Ok(total)
}

/// Length of the project (its video track) in frames.
pub fn timeline_frames(timeline: &Timeline) -> Result<i64> {
    let rate = project_rate(timeline)?;
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    track_frames(video, rate)
}

/// Check that the project has the shape the edit commands handle: one video track
/// and at most one audio track whose items match the video items one for one in
/// duration. Returns the project rate.
pub fn check_editable(timeline: &Timeline) -> Result<f64> {
    let rate = project_rate(timeline)?;
    let mut videos = 0;
    let mut audios = 0;
    for track in timeline
        .tracks
        .children
        .iter()
        .filter(|track| !track.is_overlay())
    {
        match track.kind.as_str() {
            VIDEO_KIND => videos += 1,
            AUDIO_KIND => audios += 1,
            other => {
                return Err(TimelineError::invalid(format!(
                    "track \"{}\" has kind \"{other}\"; only one Video and one Audio track are supported",
                    track.name
                )))
            }
        }
    }
    if videos != 1 || audios > 1 {
        return Err(TimelineError::invalid(format!(
            "the project has {videos} video and {audios} audio tracks; one video and at most one audio track are supported"
        )));
    }
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    for item in &video.children {
        item_range(item, rate)?;
    }
    track_frames(video, rate)?;
    if let Some(audio) = timeline.track(AUDIO_KIND) {
        track_frames(audio, rate)?;
        if audio.children.len() != video.children.len() {
            return Err(TimelineError::invalid(format!(
                "the audio track has {} items and the video track {}; they must match one for one",
                audio.children.len(),
                video.children.len()
            )));
        }
        for (index, (a, v)) in audio.children.iter().zip(&video.children).enumerate() {
            if item_range(a, rate)?.1 != item_range(v, rate)?.1 {
                return Err(TimelineError::invalid(format!(
                    "audio item {index} and video item {index} differ in duration"
                )));
            }
        }
    }
    Ok(rate)
}

/// A copy of `item` that keeps `duration` frames starting `offset` frames into it.
/// A punch-in recorded on a clip is re-based so it stays at the same place in the
/// footage.
pub(crate) fn piece(item: &Item, offset: i64, duration: i64, rate: f64) -> Result<Item> {
    let (start, _) = item_range(item, rate)?;
    let mut copy = item.clone();
    *copy.source_range_mut() = Some(TimeRange::new(
        (start + offset) as f64,
        duration as f64,
        rate,
    ));
    if offset != 0 {
        shift_punch_in(copy.metadata_mut(), -offset, rate);
    }
    Ok(copy)
}

fn shift_punch_in(metadata: &mut Map<String, Value>, by: i64, rate: f64) {
    if reelctl(metadata)
        .and_then(|map| map.get(PUNCH_IN_KEY))
        .is_none()
    {
        return;
    }
    update_reelctl(metadata, |map| {
        if let Some(Value::Object(punch)) = map.get_mut(PUNCH_IN_KEY) {
            for key in ["start", "end"] {
                let Some(value) = punch
                    .get(key)
                    .and_then(|time| time.get("value"))
                    .and_then(Value::as_f64)
                else {
                    continue;
                };
                if let Ok(time) = serde_json::to_value(RationalTime::new(value + by as f64, rate)) {
                    punch.insert(key.to_string(), time);
                }
            }
        }
    });
}

/// Make a boundary between two items at timeline `frame`, splitting the item that
/// spans it. Does nothing at an existing boundary or outside the track.
pub(crate) fn split_at(track: &mut Track, frame: i64, rate: f64) -> Result<()> {
    let mut position = 0;
    for index in 0..track.children.len() {
        let (_, duration) = item_range(&track.children[index], rate)?;
        if position < frame && frame < position + duration {
            let cut = frame - position;
            let left = piece(&track.children[index], 0, cut, rate)?;
            let right = piece(&track.children[index], cut, duration - cut, rate)?;
            track.children.splice(index..=index, [left, right]);
            return Ok(());
        }
        position += duration;
    }
    Ok(())
}

/// The media an external-reference clip plays, if it is one.
pub(crate) fn external_url(clip: &Clip) -> Option<&str> {
    match clip.active_reference() {
        Ok(MediaReference::External(external)) => Some(external.target_url.as_str()),
        _ => None,
    }
}

/// The path of the one file the project's external clips play. An error if there
/// are none or several.
pub fn single_media(timeline: &Timeline) -> Result<&str> {
    let mut found: Option<&str> = None;
    for track in timeline
        .tracks
        .children
        .iter()
        .filter(|track| !track.is_overlay())
    {
        for item in &track.children {
            if let Item::Clip(clip) = item {
                if let Some(url) = external_url(clip) {
                    match found {
                        None => found = Some(url),
                        Some(first) if first == url => {}
                        Some(first) => {
                            return Err(TimelineError::invalid(format!(
                                "the project plays more than one file (\"{first}\" and \"{url}\"); one is supported"
                            )))
                        }
                    }
                }
            }
        }
    }
    found.ok_or_else(|| TimelineError::invalid("the project has no clip with a media file"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::project;

    #[test]
    fn snapping_goes_to_the_grid_in_each_direction() {
        // 30 fps: a frame is 33.33 ms.
        assert_eq!(ceil_frame(1010, 30.0), 31);
        assert_eq!(floor_frame(1010, 30.0), 30);
        assert_eq!(round_frame(1010, 30.0), 30);
        // On the grid, all three agree even when the float is a hair off.
        assert_eq!(ceil_frame(1000, 30.0), 30);
        assert_eq!(floor_frame(1000, 30.0), 30);
        assert_eq!(ceil_frame(2000, 29.97002997), 60);
        assert_eq!(floor_frame(1001, 30000.0 / 1001.0 * 1.0), 30);
    }

    #[test]
    fn a_frame_converts_back_to_milliseconds() {
        assert_eq!(frame_ms(30, 30.0), 1000);
        assert_eq!(frame_ms(1, 30.0), 33);
        assert_eq!(frame_ms(-3, 30.0), 0);
    }

    #[test]
    fn splitting_makes_a_boundary_and_keeps_the_footage() {
        let mut timeline = project(100.0, 1000);
        let rate = 100.0;
        let video = timeline.track_mut(VIDEO_KIND).expect("video");
        split_at(video, 250, rate).expect("split");
        assert_eq!(video.children.len(), 2);
        assert_eq!(
            item_range(&video.children[0], rate).expect("range"),
            (0, 250)
        );
        assert_eq!(
            item_range(&video.children[1], rate).expect("range"),
            (250, 750)
        );
        // Splitting on the boundary, or past the end, changes nothing.
        split_at(video, 250, rate).expect("split");
        split_at(video, 5000, rate).expect("split");
        split_at(video, 0, rate).expect("split");
        assert_eq!(video.children.len(), 2);
    }

    #[test]
    fn a_project_with_mismatched_tracks_is_refused() {
        let mut timeline = project(100.0, 1000);
        let rate = check_editable(&timeline).expect("editable");
        assert_eq!(rate, 100.0);
        let audio = timeline.track_mut(AUDIO_KIND).expect("audio");
        split_at(audio, 10, rate).expect("split");
        let error = check_editable(&timeline).expect_err("mismatch");
        assert!(error.to_string().contains("audio"), "{error}");
    }

    #[test]
    fn a_fractional_frame_is_refused_not_rounded() {
        let mut timeline = project(100.0, 1000);
        let video = timeline.track_mut(VIDEO_KIND).expect("video");
        if let Some(Item::Clip(clip)) = video.children.first_mut() {
            clip.source_range = Some(TimeRange::new(0.5, 10.0, 100.0));
        }
        let error = check_editable(&timeline).expect_err("fraction");
        assert!(error.to_string().contains("whole number"), "{error}");
    }

    #[test]
    fn single_media_names_the_file() {
        let timeline = project(100.0, 1000);
        assert_eq!(single_media(&timeline).expect("media"), "/media/take.mp4");
    }

    /// A project whose video and audio tracks each hold two clips of `frames`.
    fn two_long_clips(frames: f64) -> Timeline {
        let mut timeline = project(100.0, 1000);
        for kind in [VIDEO_KIND, AUDIO_KIND] {
            let track = timeline.track_mut(kind).expect("track");
            let Some(Item::Clip(first)) = track.children.first_mut() else {
                panic!("clip expected")
            };
            first.source_range = Some(TimeRange::new(0.0, frames, 100.0));
            let second = Item::Clip(first.clone());
            track.children.push(second);
        }
        timeline
    }

    #[test]
    fn durations_that_would_overflow_the_sum_are_an_error_not_a_panic() {
        let timeline = two_long_clips(5.0e18);
        let error = timeline_frames(&timeline).expect_err("overflow");
        assert!(error.to_string().contains("out of range"), "{error}");
        assert!(check_editable(&timeline).is_err());
    }

    #[test]
    fn a_duration_above_the_cap_is_out_of_range_and_the_cap_itself_is_fine() {
        let timeline = two_long_clips(2_147_483_648.0);
        let error = check_editable(&timeline).expect_err("above the cap");
        assert!(error.to_string().contains("2147483647"), "{error}");
        let timeline = two_long_clips(2_147_483_647.0);
        assert_eq!(
            timeline_frames(&timeline).expect("frames"),
            2 * 2_147_483_647
        );
        let mut negative = project(100.0, 1000);
        let video = negative.track_mut(VIDEO_KIND).expect("video");
        if let Some(Item::Clip(clip)) = video.children.first_mut() {
            clip.source_range = Some(TimeRange::new(-3.0e18, 10.0, 100.0));
        }
        assert!(check_editable(&negative).is_err());
    }

    #[test]
    fn a_frame_rate_of_zero_is_named_as_such() {
        let mut timeline = project(100.0, 1000);
        let video = timeline.track_mut(VIDEO_KIND).expect("video");
        if let Some(Item::Clip(clip)) = video.children.first_mut() {
            clip.source_range = Some(TimeRange::new(0.0, 1000.0, 0.0));
        }
        let error = project_rate(&timeline).expect_err("rate 0");
        let text = error.to_string();
        assert!(text.contains("frame rate") && text.contains('0'), "{text}");
        assert!(!text.contains("no clip with a source_range"), "{text}");
    }
}
