//! `transform auto-punch-in`: punch-ins wherever the edited timeline would hold one
//! picture longer than `reelctl diff` allows (a static window), at the outliers'
//! zoom scale.

use edit_dna::{AsrBackend, EditDna, VisualEventKind};

use crate::error::{Result, TimelineError};
use crate::otio::{reelctl, Item, Timeline, VIDEO_KIND};
use crate::punch::{punch_in, PunchRequest, MAX_SCALE, RAMP_FRAMES};
use crate::tracks::{
    check_editable, external_url, frame_ms, item_range, round_frame, timeline_frames, PUNCH_IN_KEY,
};

/// A stretch longer than this with no cut and no visual event is a static window.
/// The same limit as `reelctl diff` (`diff::STATIC_WINDOW_LIMIT_MS`).
pub const STATIC_WINDOW_MS: u64 = 2_500;
/// A punch-in starts this long after the start of a window, or after the last one.
pub const LEAD_MS: u64 = 1_000;
/// And lasts this long. Punch-ins are placed until what is left of the window is
/// at most this long, so every piece is at most 1.5 s, well inside the limit even
/// if a detector places the move's event a few frames late.
pub const PUNCH_MS: u64 = 1_500;
/// A punch-in moves to a word that starts this soon after its planned start.
pub const WORD_SNAP_MS: u64 = 300;
/// The scale when the outliers have no measured punch-in.
pub const DEFAULT_SCALE: f64 = 1.15;
/// The outliers' scale is clamped to this range: a smaller zoom is not seen, a
/// bigger one crops a talking head too hard.
pub const MIN_SCALE: f64 = 1.08;
pub const MAX_AUTO_SCALE: f64 = 1.35;

/// What an `auto-punch-in` run did.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoPunchReport {
    /// Static windows found, as `(start_ms, end_ms)` of the timeline.
    pub windows: Vec<(u64, u64)>,
    /// Punch-ins added, as `(start_ms, end_ms)` of the timeline.
    pub punches: Vec<(u64, u64)>,
    pub scale: f64,
    /// The scale came from the outliers (`true`) or is [`DEFAULT_SCALE`].
    pub measured: bool,
}

/// The median scale of the outliers' punch-ins, clamped, or the default.
pub fn outlier_scale(outliers: &[&EditDna]) -> (f64, bool) {
    let mut scales: Vec<f64> = outliers
        .iter()
        .flat_map(|dna| &dna.visual_events)
        .filter(|event| event.kind == VisualEventKind::PunchIn)
        .filter_map(|event| event.scale)
        .filter(|scale| scale.is_finite() && *scale > 1.0)
        .collect();
    if scales.is_empty() {
        return (DEFAULT_SCALE, false);
    }
    scales.sort_by(f64::total_cmp);
    let middle = scales.len() / 2;
    let median = if scales.len() % 2 == 0 {
        (scales[middle - 1] + scales[middle]) / 2.0
    } else {
        scales[middle]
    };
    (median.clamp(MIN_SCALE, MAX_AUTO_SCALE.min(MAX_SCALE)), true)
}

/// Timeline frames where the picture changes: every cut and visual event of `dna`
/// that is still in the edit, the start and end of every punch-in already in the
/// project, and the edges of anything that is not footage (a card).
fn boundaries(timeline: &Timeline, dna: &EditDna, rate: f64) -> Result<Vec<i64>> {
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let source_times: Vec<u64> = dna
        .cuts
        .iter()
        .map(|cut| cut.time_ms)
        .chain(dna.visual_events.iter().map(|event| event.time_ms))
        .collect();
    let mut found = Vec::new();
    let mut position = 0;
    for item in &video.children {
        let (start, duration) = item_range(item, rate)?;
        let footage = matches!(item, Item::Clip(clip) if external_url(clip).is_some());
        if !footage {
            found.push(position);
            found.push(position + duration);
        } else {
            for &ms in &source_times {
                let frame = round_frame(ms, rate);
                // A cut on a piece's first frame is a cut at the splice.
                if frame >= start && frame < start + duration {
                    found.push(position + frame - start);
                }
            }
            let punch = reelctl(item.metadata()).and_then(|map| map.get(PUNCH_IN_KEY));
            for key in ["start", "end"] {
                let value = punch
                    .and_then(|punch| punch.get(key))
                    .and_then(|time| time.get("value"))
                    .and_then(serde_json::Value::as_f64);
                if let Some(value) = value {
                    found.push(position + value.round() as i64);
                }
            }
        }
        position += duration;
    }
    Ok(found)
}

/// Static windows of the edited timeline, as `(start_frame, end_frame)`.
fn static_windows(timeline: &Timeline, dna: &EditDna, rate: f64) -> Result<Vec<(i64, i64)>> {
    let total = timeline_frames(timeline)?;
    let mut bounds = boundaries(timeline, dna, rate)?;
    bounds.retain(|frame| *frame > 0 && *frame < total);
    bounds.push(0);
    bounds.push(total);
    bounds.sort_unstable();
    bounds.dedup();
    Ok(bounds
        .windows(2)
        .map(|pair| (pair[0], pair[1]))
        .filter(|&(start, end)| frame_ms(end - start, rate) > STATIC_WINDOW_MS)
        .map(|window| Ok((window, footage_only(timeline, window.0, window.1, rate)?)))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|(window, footage)| footage.then_some(window))
        .collect())
}

/// Whether timeline frames `start..end` are all footage (not a card or a gap).
fn footage_only(timeline: &Timeline, start: i64, end: i64, rate: f64) -> Result<bool> {
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let mut position = 0;
    for item in &video.children {
        let (_, duration) = item_range(item, rate)?;
        let overlaps = position < end && position + duration > start;
        let footage = matches!(item, Item::Clip(clip) if external_url(clip).is_some());
        if overlaps && !footage {
            return Ok(false);
        }
        position += duration;
    }
    Ok(true)
}

/// Punch-ins for one window, in timeline milliseconds: one [`PUNCH_MS`] long
/// every [`LEAD_MS`], each moved to a word that starts within [`WORD_SNAP_MS`].
fn plan_window(start_ms: u64, end_ms: u64, word_starts: &[u64], min_ms: u64) -> Vec<(u64, u64)> {
    let mut planned = Vec::new();
    let mut last = start_ms;
    while end_ms.saturating_sub(last) > PUNCH_MS {
        let mut from = last + LEAD_MS;
        if let Some(&word) = word_starts
            .iter()
            .find(|&&word| word >= from && word <= from + WORD_SNAP_MS)
        {
            from = word;
        }
        let to = (from + PUNCH_MS).min(end_ms);
        if to.saturating_sub(from) < min_ms {
            break;
        }
        planned.push((from, to));
        last = to;
    }
    planned
}

/// Add punch-ins at `scale` to every static window of `timeline`. Cuts and visual
/// events come from `dna`, the edit DNA of the project's media, and are placed on
/// the edited timeline; word starts (when `dna` has whisper words) place each zoom
/// on a word. Running it again on its own output adds nothing.
pub fn auto_punch_in(
    timeline: &mut Timeline,
    dna: &EditDna,
    scale: f64,
) -> Result<Vec<(u64, u64)>> {
    let rate = check_editable(timeline)?;
    let windows = static_windows(timeline, dna, rate)?;
    // Only whisper's words: the fallback estimator's voiced bursts are not words.
    let word_starts: Vec<u64> = if dna.asr_backend == AsrBackend::WhisperCpp {
        crate::words_on_timeline(timeline, &dna.words)?
            .iter()
            .map(|word| word.start_ms)
            .collect()
    } else {
        Vec::new()
    };
    let min_ms = frame_ms(2 * RAMP_FRAMES, rate) + 1;
    let mut added = Vec::new();
    for (start, end) in windows {
        let (start_ms, end_ms) = (frame_ms(start, rate), frame_ms(end, rate));
        for (from, to) in plan_window(start_ms, end_ms, &word_starts, min_ms) {
            punch_in(
                timeline,
                &PunchRequest {
                    start_ms: from,
                    end_ms: to,
                    scale,
                },
            )?;
            added.push((from, to));
        }
    }
    Ok(added)
}

/// [`auto_punch_in`] with the scale taken from `outliers`, and the windows reported.
pub fn auto_punch_in_like(
    timeline: &mut Timeline,
    dna: &EditDna,
    outliers: &[&EditDna],
) -> Result<AutoPunchReport> {
    crate::overlay::refuse_overlays(timeline)?;
    let rate = check_editable(timeline)?;
    let windows = static_windows(timeline, dna, rate)?
        .into_iter()
        .map(|(start, end)| (frame_ms(start, rate), frame_ms(end, rate)))
        .collect();
    let (scale, measured) = outlier_scale(outliers);
    let punches = auto_punch_in(timeline, dna, scale)?;
    Ok(AutoPunchReport {
        windows,
        punches,
        scale,
        measured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{dna, project, with_words};
    use crate::{cut_silence, CutOptions};
    use edit_dna::{Cut, VisualEvent};

    fn cut_at(time_ms: u64) -> Cut {
        Cut {
            frame_index: time_ms / 10,
            time_ms,
            interval_ms: 0,
            scene_score: 1.0,
        }
    }

    fn punch_event(time_ms: u64, scale: f64) -> VisualEvent {
        VisualEvent {
            time_ms,
            frame_index: time_ms / 10,
            kind: VisualEventKind::PunchIn,
            scale: Some(scale),
            center_x: Some(0.5),
            center_y: Some(0.5),
            area: None,
        }
    }

    fn punches(timeline: &Timeline) -> Vec<(i64, i64)> {
        let video = timeline.track(VIDEO_KIND).unwrap();
        let mut position = 0;
        let mut found = Vec::new();
        for item in &video.children {
            let (_, duration) = item_range(item, 100.0).unwrap();
            if reelctl(item.metadata())
                .and_then(|map| map.get(PUNCH_IN_KEY))
                .is_some()
            {
                found.push((position, position + duration));
            }
            position += duration;
        }
        found
    }

    #[test]
    fn every_piece_of_a_long_still_window_ends_up_under_the_limit() {
        // 10 s at 100 fps with no cut: one static window.
        let mut timeline = project(100.0, 1000);
        let media = dna(10_000, 100.0, &[]);
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.windows, vec![(0, 10_000)]);
        assert_eq!(
            report.punches,
            vec![
                (1_000, 2_500),
                (3_500, 5_000),
                (6_000, 7_500),
                (8_500, 10_000)
            ]
        );
        assert_eq!((report.scale, report.measured), (DEFAULT_SCALE, false));
        assert_eq!(
            punches(&timeline),
            vec![(100, 250), (350, 500), (600, 750), (850, 1000)]
        );
        let rate = check_editable(&timeline).unwrap();
        assert!(static_windows(&timeline, &media, rate).unwrap().is_empty());
    }

    #[test]
    fn a_second_run_adds_nothing() {
        let mut timeline = project(100.0, 1000);
        let media = dna(10_000, 100.0, &[]);
        auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        let before = timeline.clone();
        let again = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert!(again.windows.is_empty() && again.punches.is_empty());
        assert_eq!(timeline, before);
    }

    #[test]
    fn cuts_and_events_split_windows_so_short_shots_are_left_alone() {
        let mut timeline = project(100.0, 1000);
        let mut media = dna(10_000, 100.0, &[]);
        // Shots of 2.0, 2.5, 2.4 s, then 3.1 s with an event at 8.5 s.
        media.cuts = vec![cut_at(2_000), cut_at(4_500), cut_at(6_900)];
        media.visual_events = vec![punch_event(8_500, 1.2)];
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert!(report.windows.is_empty(), "{:?}", report.windows);
        assert!(report.punches.is_empty());
    }

    #[test]
    fn windows_are_found_on_the_edited_timeline() {
        // A 2.0 s silence at 3..5 s is cut out: the 2 s before and 5 s after
        // join into one 7 s still stretch with no cut.
        let mut timeline = project(100.0, 1000);
        let mut media = dna(10_000, 100.0, &[(3_000, 5_000)]);
        media.cuts = vec![cut_at(1_000)];
        cut_silence(&mut timeline, &media, &CutOptions::default()).unwrap();
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        // Timeline: 0..1 s, cut, 1..8 s (media 1..3 s and 5..10 s).
        assert_eq!(report.windows, vec![(1_000, 8_000)]);
        assert_eq!(
            report.punches,
            vec![(2_000, 3_500), (4_500, 6_000), (7_000, 8_000)]
        );
    }

    #[test]
    fn a_punch_moves_to_a_word_that_starts_just_after_its_planned_start() {
        let mut timeline = project(100.0, 1000);
        let media = with_words(
            dna(10_000, 100.0, &[]),
            &[("now", 1_200, 1_500), ("later", 4_100, 4_400)],
        );
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        // 1.2 s is within 300 ms of 1.0 s; 4.1 s is more than 300 ms after 3.7 s.
        assert_eq!(report.punches[0], (1_200, 2_700));
        assert_eq!(report.punches[1], (3_700, 5_200));
    }

    #[test]
    fn no_piece_of_a_window_is_left_longer_than_a_punch() {
        // 5.0 s: 1.0..2.5 s leaves 2.5 s, so a second punch goes in at 3.5 s.
        let mut timeline = project(100.0, 500);
        let media = dna(5_000, 100.0, &[]);
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.punches, vec![(1_000, 2_500), (3_500, 5_000)]);
        for length in [2_600u64, 4_100, 5_900, 7_300, 12_000] {
            let planned = plan_window(0, length, &[], 130);
            let mut last = 0;
            for &(from, to) in &planned {
                assert!(
                    from - last <= PUNCH_MS && to > from,
                    "{length}: {planned:?}"
                );
                last = to;
            }
            assert!(length - last <= PUNCH_MS, "{length}: {planned:?}");
        }
    }

    #[test]
    fn voiced_bursts_are_not_words_to_snap_to() {
        let mut timeline = project(100.0, 1000);
        let mut media = with_words(dna(10_000, 100.0, &[]), &[("burst-1", 1_200, 1_500)]);
        media.asr_backend = edit_dna::AsrBackend::SilenceBursts;
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.punches[0], (1_000, 2_500));
    }

    #[test]
    fn on_a_trimmed_clip_cuts_and_punches_are_placed_by_the_clip_start() {
        // Media frames 300..800 at 100 fps (3..8 s); a cut at 5.5 s is timeline
        // 2.5 s, splitting the 5 s clip into 2.5 s and 2.5 s: no window.
        let spec = crate::InitSpec {
            media_path: "/media/take.mp4".to_string(),
            fps: 100.0,
            frames: 1000,
            width: 640,
            height: 360,
            has_audio: true,
            dna: None,
        };
        let pick = crate::ClipPick {
            understanding_path: "/u.json".to_string(),
            index: 1,
            start_ms: 3_000,
            end_ms: 8_000,
            hook_id: 0,
            last_id: 0,
        };
        let mut timeline = crate::from_clip(&spec, &pick).unwrap();
        let mut media = dna(10_000, 100.0, &[]);
        media.cuts = vec![cut_at(5_500)];
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert!(report.windows.is_empty(), "{:?}", report.windows);
        // A cut at media 1 s (before the clip) does not count: one 5 s window.
        let mut timeline = crate::from_clip(&spec, &pick).unwrap();
        media.cuts = vec![cut_at(1_000)];
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.windows, vec![(0, 5_000)]);
        assert_eq!(punches(&timeline)[0], (100, 250));
    }

    #[test]
    fn a_cut_on_the_first_frame_after_a_splice_ends_the_window() {
        // A silence 3..5 s is cut out and the shot changes at 5 s, right at the
        // splice: the 3 s before and 5 s after are two windows, not one of 8 s.
        let mut timeline = project(100.0, 1000);
        let mut media = dna(10_000, 100.0, &[(3_000, 5_000)]);
        media.cuts = vec![cut_at(5_000)];
        cut_silence(&mut timeline, &media, &CutOptions::default()).unwrap();
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.windows, vec![(0, 3_000), (3_000, 8_000)]);
    }

    #[test]
    fn the_scale_is_the_outliers_median_clamped() {
        let mut one = dna(10_000, 100.0, &[]);
        one.visual_events = vec![punch_event(1_000, 1.1), punch_event(2_000, 1.3)];
        let mut two = dna(10_000, 100.0, &[]);
        two.visual_events = vec![punch_event(1_000, 1.25)];
        assert_eq!(outlier_scale(&[&one, &two]), (1.25, true));
        let mut huge = dna(10_000, 100.0, &[]);
        huge.visual_events = vec![punch_event(1_000, 1.9)];
        assert_eq!(outlier_scale(&[&huge]), (MAX_AUTO_SCALE, true));
        let mut tiny = dna(10_000, 100.0, &[]);
        tiny.visual_events = vec![punch_event(1_000, 1.02)];
        assert_eq!(outlier_scale(&[&tiny]), (MIN_SCALE, true));
        assert_eq!(outlier_scale(&[]), (DEFAULT_SCALE, false));
    }

    #[test]
    fn a_card_at_the_end_is_not_a_static_window() {
        let mut timeline = project(100.0, 200);
        crate::append_bridge(&mut timeline, "https://example.com", 5_000).unwrap();
        let media = dna(2_000, 100.0, &[]);
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert!(report.windows.is_empty(), "{:?}", report.windows);
    }

    #[test]
    fn a_window_too_short_for_another_punch_is_left_as_is() {
        // 2.6 s: just over the limit, so one punch at 1.0..2.5 s fits.
        let mut timeline = project(100.0, 260);
        let media = dna(2_600, 100.0, &[]);
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.punches, vec![(1_000, 2_500)]);
        // 3.6 s: 1.0..2.5, then 1.1 s remains, which is under the limit.
        let mut timeline = project(100.0, 360);
        let media = dna(3_600, 100.0, &[]);
        let report = auto_punch_in_like(&mut timeline, &media, &[]).unwrap();
        assert_eq!(report.punches, vec![(1_000, 2_500)]);
    }
}
