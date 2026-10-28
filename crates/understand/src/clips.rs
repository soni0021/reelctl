//! The clip rules: open on the hook's sentence, end on the cliffhanger, check the
//! length once pauses are cut, and place cut points inside measured pauses.
//!
//! Priority: hook first, then keep the cliffhanger, then length. A rule never breaks
//! a higher-priority one; when it would have to, it flags instead.

use edit_dna::SilenceGap;

use crate::types::{Clip, ClipPick, Rejected, Segment, SegmentRef};
use crate::{
    CLIP_MAX_MS, CLIP_MIN_MS, DEAD_AIR_MS, END_PAUSE_INTO_MS, END_TAIL_MS, HOOK_BACKUP_MAX_MS,
    PAYOFF_MAX_MS, PAYOFF_MIN_MS, START_LEAD_MS, START_PAUSE_REACH_MS,
};

/// The start of a flag for a clip that opens on a hook that is not at a sentence start.
/// Callers may match on this prefix; the numbers follow it.
pub const FLAG_STARTS_MID_SENTENCE: &str = "starts mid-sentence";

/// The flag for a clip that opens on its hook because the hook's sentence starts
/// `back_ms` before it, more than `HOOK_BACKUP_MAX_MS`.
fn flag_starts_mid_sentence(back_ms: u64) -> String {
    format!(
        "{FLAG_STARTS_MID_SENTENCE}: its sentence begins {back_ms} ms before the hook, over the {HOOK_BACKUP_MAX_MS} ms limit"
    )
}

/// The flag for a tight length under `CLIP_MIN_MS`; it names the tight and the raw length.
fn flag_tight_under(tight_ms: u64, length_ms: u64) -> String {
    format!(
        "tight length {tight_ms} ms is under the {CLIP_MIN_MS} ms minimum (clip length {length_ms} ms)"
    )
}

/// The flag for a tight length over `CLIP_MAX_MS`; it names the tight and the raw length.
fn flag_tight_over(tight_ms: u64, length_ms: u64) -> String {
    format!(
        "tight length {tight_ms} ms is over the {CLIP_MAX_MS} ms maximum (clip length {length_ms} ms)"
    )
}

/// The flag for a payoff that lands `at_ms` into the clip on tight time, outside
/// `PAYOFF_MIN_MS..=PAYOFF_MAX_MS`.
fn flag_payoff_window(at_ms: u64) -> String {
    format!(
        "payoff lands {at_ms} ms into the clip, outside {PAYOFF_MIN_MS}-{PAYOFF_MAX_MS} ms (tight time)"
    )
}

/// The flag for a payoff segment that is not one of the clip's segments.
fn flag_payoff_outside(payoff: u32, first: u32, last: u32) -> String {
    format!("payoff segment {payoff} is outside the clip (segments {first}-{last})")
}

/// The flag for a clip that overlaps the higher-ranked clip `rank`, which spans
/// `start_ms..end_ms`.
fn flag_overlap(rank: u32, start_ms: u64, end_ms: u64) -> String {
    format!("overlaps higher-ranked clip {rank} ({start_ms}-{end_ms} ms)")
}

/// "segment 4" or "segments 4-6".
fn segment_span(first: u32, last: u32) -> String {
    if first == last {
        format!("segment {first}")
    } else {
        format!("segments {first}-{last}")
    }
}

/// The adjustment for material the model put before the opening segment `opening`.
fn adjust_dropped_before(first: u32, last: u32, ms: u64, opening: u32) -> String {
    format!(
        "dropped {} ({ms} ms) before segment {opening}",
        segment_span(first, last)
    )
}

/// The adjustment for a start the model put after the opening segment `opening`.
fn adjust_moved_start(from: u32, opening: u32) -> String {
    format!("moved the start back from segment {from} to segment {opening} to open on the hook")
}

/// The adjustment for material the model put after the cutoff.
fn adjust_dropped_after(first: u32, last: u32) -> String {
    format!("dropped {} after the cutoff", segment_span(first, last))
}

/// The adjustment for an end the model put before the cutoff.
fn adjust_extended_to_cutoff(end: u32, cutoff: u32) -> String {
    format!("extended the end from segment {end} to the cutoff, segment {cutoff}")
}

/// The reason for a pick naming a segment that does not exist.
fn reason_missing(field: &str, id: u32, count: usize) -> String {
    format!("{field} {id} is not a segment (the transcript has {count})")
}

/// The reason for a cutoff before the hook.
fn reason_cutoff_before_hook(cutoff: u32, hook: u32) -> String {
    format!("cutoff_segment {cutoff} is before hook_segment {hook}")
}

/// The reason for a start after the end.
fn reason_start_after_end(start: u32, end: u32) -> String {
    format!("start_segment {start} is after end_segment {end}")
}

/// The picks that became clips (in the model's order) and the ones that did not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipOutcome {
    pub clips: Vec<Clip>,
    pub rejected: Vec<Rejected>,
}

/// Apply the clip rules to the model's picks. `picks[i]` has rank `i + 1`.
///
/// `pauses` are the measured silence gaps of the video; `duration_ms` bounds the cut
/// points. See the spec's "Clip rules" for the six rules. Decisions the spec leaves open:
///
/// - `segments` are in time order; a segment is found by its `id`, and "before" and
///   "after" compare positions in `segments`.
/// - `pauses` do not overlap one another.
/// - All five IDs of a pick must exist, `payoff_segment` and `end_segment` included.
/// - The model's `start_segment` never decides the start: the hook does. Segments the
///   model put before the opening segment are dropped; a start after it is moved back.
/// - `end_segment` never decides the end: the cutoff does. A later `end_segment` is
///   dropped, an earlier one is extended to the cutoff. Both are recorded.
/// - A payoff outside the clip's segments is flagged, not rejected; `payoff_at_ms` is
///   then `None`.
/// - Overlap compares the final cut points, and clips that only touch do not overlap.
///   It is checked against kept clips only, so a rejected pick is not one to overlap.
pub fn build_clips(
    picks: &[ClipPick],
    segments: &[Segment],
    pauses: &[SilenceGap],
    duration_ms: u64,
) -> ClipOutcome {
    let mut outcome = ClipOutcome::default();
    for (rank, pick) in (1u32..).zip(picks) {
        match plan_clip(rank, pick, segments, pauses, duration_ms, &outcome.clips) {
            Ok(clip) => outcome.clips.push(clip),
            Err(reason) => outcome.rejected.push(Rejected {
                rank,
                title: pick.title.clone(),
                reason,
            }),
        }
    }
    outcome
}

/// A segment and its position in the transcript.
#[derive(Clone, Copy)]
struct At<'a> {
    index: usize,
    seg: &'a Segment,
}

/// The five segments a pick names.
struct Located<'a> {
    start: At<'a>,
    end: At<'a>,
    hook: At<'a>,
    payoff: At<'a>,
    cutoff: At<'a>,
}

/// Rule 1: find the pick's segments, or say why it is invalid. Nothing is repaired.
fn locate<'a>(pick: &ClipPick, segments: &'a [Segment]) -> Result<Located<'a>, String> {
    let find = |field: &str, id: u32| {
        segments
            .iter()
            .enumerate()
            .find(|(_, seg)| seg.id == id)
            .map(|(index, seg)| At { index, seg })
            .ok_or_else(|| reason_missing(field, id, segments.len()))
    };
    let located = Located {
        start: find("start_segment", pick.start_segment)?,
        end: find("end_segment", pick.end_segment)?,
        hook: find("hook_segment", pick.hook_segment)?,
        payoff: find("payoff_segment", pick.payoff_segment)?,
        cutoff: find("cutoff_segment", pick.cutoff_segment)?,
    };
    if located.cutoff.index < located.hook.index {
        return Err(reason_cutoff_before_hook(
            pick.cutoff_segment,
            pick.hook_segment,
        ));
    }
    if located.start.index > located.end.index {
        return Err(reason_start_after_end(pick.start_segment, pick.end_segment));
    }
    Ok(located)
}

/// Rule 2: the segment the clip opens on, and how far before the hook its sentence starts
/// when that is too far (the clip then opens on the hook and is flagged).
///
/// A sentence starts at the first segment and after every segment with `ends_sentence`.
fn open_on_hook<'a>(segments: &'a [Segment], hook: At<'a>) -> (At<'a>, Option<u64>) {
    let mut sentence_start = hook;
    while let Some(index) = sentence_start.index.checked_sub(1) {
        match segments.get(index) {
            Some(seg) if !seg.ends_sentence => sentence_start = At { index, seg },
            _ => break,
        }
    }
    let back_ms = hook
        .seg
        .start_ms
        .saturating_sub(sentence_start.seg.start_ms);
    if back_ms <= HOOK_BACKUP_MAX_MS {
        (sentence_start, None)
    } else {
        (hook, Some(back_ms))
    }
}

/// The length of a measured pause.
fn pause_len(pause: &SilenceGap) -> u64 {
    pause.end_ms.saturating_sub(pause.start_ms)
}

/// Rule 5, start: the end of the latest pause that lies between the previous segment and
/// the first one and ends within `START_PAUSE_REACH_MS` before the first word; otherwise
/// `START_LEAD_MS` before the first word. A pause that ends after the first word starts,
/// or before the previous segment ends (speech lies between), is not a start.
fn start_cut(first: &Segment, previous: Option<&Segment>, pauses: &[SilenceGap]) -> u64 {
    let floor = previous.map_or(0, |seg| seg.end_ms);
    let pause_end = pauses
        .iter()
        .map(|pause| pause.end_ms)
        .filter(|&end| end <= first.start_ms && end >= floor)
        .max();
    match pause_end {
        Some(end) if first.start_ms.saturating_sub(end) <= START_PAUSE_REACH_MS => end,
        _ => first.start_ms.saturating_sub(START_LEAD_MS),
    }
}

/// Rule 5, end: `END_PAUSE_INTO_MS` into the first pause that starts after the last word
/// and before the next segment, or half the pause if that is shorter; otherwise
/// `END_TAIL_MS` after the last word.
fn end_cut(last: &Segment, next: Option<&Segment>, pauses: &[SilenceGap]) -> u64 {
    let pause = pauses
        .iter()
        .filter(|pause| {
            pause.start_ms >= last.end_ms && next.is_none_or(|seg| pause.start_ms < seg.start_ms)
        })
        .min_by_key(|pause| pause.start_ms);
    match pause {
        Some(pause) => pause
            .start_ms
            .saturating_add(END_PAUSE_INTO_MS.min(pause_len(pause) / 2)),
        None => last.end_ms.saturating_add(END_TAIL_MS),
    }
}

/// Rule 4: the pauses inside `start_ms..end_ms`, each clipped to that span, whose clipped
/// length is at least `DEAD_AIR_MS`, in time order. The clipped length decides, so the
/// 150 ms the clip ends into a long pause is never dead air.
fn dead_air_in(pauses: &[SilenceGap], start_ms: u64, end_ms: u64) -> Vec<SilenceGap> {
    let mut dead: Vec<SilenceGap> = pauses
        .iter()
        .filter_map(|pause| {
            let from = pause.start_ms.max(start_ms);
            let to = pause.end_ms.min(end_ms);
            let duration_ms = to.saturating_sub(from);
            (duration_ms >= DEAD_AIR_MS).then_some(SilenceGap {
                start_ms: from,
                end_ms: to,
                duration_ms,
            })
        })
        .collect();
    dead.sort_by_key(|gap| gap.start_ms);
    dead
}

/// The time of `at_ms` into a clip that starts at `start_ms`, once `dead_air` before it is cut.
fn tight_offset(at_ms: u64, start_ms: u64, dead_air: &[SilenceGap]) -> u64 {
    let cut: u64 = dead_air
        .iter()
        .map(|gap| gap.end_ms.min(at_ms).saturating_sub(gap.start_ms))
        .sum();
    at_ms.saturating_sub(start_ms).saturating_sub(cut)
}

/// What the rules changed about the clip's first and last segment.
fn adjustments(segments: &[Segment], at: &Located, first: At) -> Vec<String> {
    let mut changes = Vec::new();
    match at.start.index.cmp(&first.index) {
        std::cmp::Ordering::Less => {
            let last_dropped = first
                .index
                .checked_sub(1)
                .and_then(|index| segments.get(index))
                .unwrap_or(at.start.seg);
            changes.push(adjust_dropped_before(
                at.start.seg.id,
                last_dropped.id,
                first.seg.start_ms.saturating_sub(at.start.seg.start_ms),
                first.seg.id,
            ));
        }
        std::cmp::Ordering::Greater => {
            changes.push(adjust_moved_start(at.start.seg.id, first.seg.id));
        }
        std::cmp::Ordering::Equal => {}
    }
    match at.end.index.cmp(&at.cutoff.index) {
        std::cmp::Ordering::Greater => {
            let first_dropped = segments.get(at.cutoff.index + 1).unwrap_or(at.end.seg);
            changes.push(adjust_dropped_after(first_dropped.id, at.end.seg.id));
        }
        std::cmp::Ordering::Less => {
            changes.push(adjust_extended_to_cutoff(at.end.seg.id, at.cutoff.seg.id));
        }
        std::cmp::Ordering::Equal => {}
    }
    changes
}

/// Apply the rules to one pick. `kept` are the higher-ranked clips already built.
fn plan_clip(
    rank: u32,
    pick: &ClipPick,
    segments: &[Segment],
    pauses: &[SilenceGap],
    duration_ms: u64,
    kept: &[Clip],
) -> Result<Clip, String> {
    let at = locate(pick, segments)?;
    let (first, mid_sentence_ms) = open_on_hook(segments, at.hook);
    let previous = first
        .index
        .checked_sub(1)
        .and_then(|index| segments.get(index));
    let next = segments.get(at.cutoff.index + 1);

    let start_ms = start_cut(first.seg, previous, pauses).min(duration_ms);
    let end_ms = end_cut(at.cutoff.seg, next, pauses).min(duration_ms);
    let length_ms = end_ms.saturating_sub(start_ms);
    let dead_air = dead_air_in(pauses, start_ms, end_ms);
    let dead_ms: u64 = dead_air.iter().map(|gap| gap.duration_ms).sum();
    let tight_length_ms = length_ms.saturating_sub(dead_ms);

    let payoff_inside = (first.index..=at.cutoff.index).contains(&at.payoff.index);
    let payoff_at_ms =
        payoff_inside.then(|| tight_offset(at.payoff.seg.start_ms, start_ms, &dead_air));

    let mut flags = Vec::new();
    if let Some(back_ms) = mid_sentence_ms {
        flags.push(flag_starts_mid_sentence(back_ms));
    }
    if tight_length_ms < CLIP_MIN_MS {
        flags.push(flag_tight_under(tight_length_ms, length_ms));
    } else if tight_length_ms > CLIP_MAX_MS {
        flags.push(flag_tight_over(tight_length_ms, length_ms));
    }
    match payoff_at_ms {
        None => flags.push(flag_payoff_outside(
            at.payoff.seg.id,
            first.seg.id,
            at.cutoff.seg.id,
        )),
        Some(at_ms) if !(PAYOFF_MIN_MS..=PAYOFF_MAX_MS).contains(&at_ms) => {
            flags.push(flag_payoff_window(at_ms));
        }
        Some(_) => {}
    }
    for higher in kept {
        if start_ms < higher.end_ms && higher.start_ms < end_ms {
            flags.push(flag_overlap(higher.rank, higher.start_ms, higher.end_ms));
        }
    }

    Ok(Clip {
        rank,
        title: pick.title.clone(),
        start_ms,
        end_ms,
        length_ms,
        tight_length_ms,
        dead_air,
        hook: quote(at.hook.seg),
        payoff: quote(at.payoff.seg),
        last: quote(at.cutoff.seg),
        payoff_at_ms,
        open_loop: pick.open_loop.clone(),
        visual_suggestion: pick.visual_suggestion.clone(),
        why: pick.why.clone(),
        adjustments: adjustments(segments, &at, first),
        flags,
    })
}

/// A segment's ID and its text, copied exactly.
fn quote(seg: &Segment) -> SegmentRef {
    SegmentRef {
        id: seg.id,
        text: seg.text.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SegmentRef;

    fn seg(id: u32, start_ms: u64, end_ms: u64, ends_sentence: bool) -> Segment {
        Segment {
            id,
            start_ms,
            end_ms,
            text: format!("text of segment {id}"),
            ends_sentence,
        }
    }

    fn gap(start_ms: u64, end_ms: u64) -> SilenceGap {
        SilenceGap {
            start_ms,
            end_ms,
            duration_ms: end_ms - start_ms,
        }
    }

    fn pick(start: u32, end: u32, hook: u32, payoff: u32, cutoff: u32) -> ClipPick {
        ClipPick {
            start_segment: start,
            end_segment: end,
            hook_segment: hook,
            payoff_segment: payoff,
            cutoff_segment: cutoff,
            title: format!("pick {hook}-{cutoff}"),
            open_loop: "the open loop".to_string(),
            visual_suggestion: "show the chart".to_string(),
            why: "it hooks".to_string(),
        }
    }

    /// The only clip of an outcome; the test fails if there is not exactly one.
    fn one_clip(outcome: ClipOutcome) -> Clip {
        assert!(
            outcome.rejected.is_empty(),
            "unexpected rejection: {:?}",
            outcome.rejected
        );
        assert_eq!(outcome.clips.len(), 1, "expected one clip");
        outcome.clips.into_iter().next().expect("one clip")
    }

    fn has_flag(clip: &Clip, needle: &str) -> bool {
        clip.flags.iter().any(|f| f.contains(needle))
    }

    fn has_adjustment(clip: &Clip, needle: &str) -> bool {
        clip.adjustments.iter().any(|a| a.contains(needle))
    }

    /// Segment 1 ends a sentence, segment 2 starts the hook's sentence at 1,000 ms,
    /// and segment 3 (the hook) starts `distance` ms after segment 2.
    fn backup_case(distance: u64) -> Clip {
        let hook_start = 1_000 + distance;
        let segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, false),
            seg(3, hook_start, hook_start + 1_000, true),
        ];
        one_clip(build_clips(&[pick(2, 3, 3, 3, 3)], &segs, &[], 100_000))
    }

    // Empty and invalid input.

    #[test]
    fn no_picks_give_an_empty_outcome() {
        let segs = vec![seg(1, 0, 1_000, true)];
        let outcome = build_clips(&[], &segs, &[], 10_000);
        assert!(outcome.clips.is_empty());
        assert!(outcome.rejected.is_empty());
    }

    #[test]
    fn no_segments_reject_every_pick() {
        let outcome = build_clips(
            &[pick(1, 1, 1, 1, 1), pick(1, 2, 1, 2, 2)],
            &[],
            &[],
            10_000,
        );
        assert!(outcome.clips.is_empty());
        let ranks: Vec<u32> = outcome.rejected.iter().map(|r| r.rank).collect();
        assert_eq!(ranks, vec![1, 2]);
    }

    #[test]
    fn an_out_of_range_id_is_rejected_whatever_its_field() {
        let segs = vec![
            seg(1, 0, 1_000, true),
            seg(2, 2_000, 3_000, true),
            seg(3, 4_000, 5_000, true),
        ];
        // Each field set to a segment that does not exist, the others valid.
        let cases = [
            ("start_segment", pick(4, 3, 1, 2, 3)),
            ("end_segment", pick(1, 4, 1, 2, 3)),
            ("hook_segment", pick(1, 3, 4, 2, 3)),
            ("payoff_segment", pick(1, 3, 1, 4, 3)),
            ("cutoff_segment", pick(1, 3, 1, 2, 4)),
            ("hook_segment", pick(1, 3, 0, 2, 3)),
        ];
        for (field, bad) in cases {
            let outcome = build_clips(&[bad], &segs, &[], 10_000);
            assert!(outcome.clips.is_empty(), "{field} accepted");
            assert_eq!(outcome.rejected.len(), 1, "{field}");
            let rejected = &outcome.rejected[0];
            assert_eq!(rejected.rank, 1);
            assert!(rejected.reason.contains(field), "{}", rejected.reason);
        }
    }

    #[test]
    fn a_cutoff_before_the_hook_is_rejected_not_repaired() {
        let segs = vec![
            seg(1, 0, 1_000, true),
            seg(2, 2_000, 3_000, true),
            seg(3, 4_000, 5_000, true),
        ];
        let outcome = build_clips(&[pick(1, 3, 3, 3, 2)], &segs, &[], 10_000);
        assert!(outcome.clips.is_empty());
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].title, "pick 3-2");
        assert!(outcome.rejected[0].reason.contains("cutoff_segment"));
        assert!(outcome.rejected[0].reason.contains("hook_segment"));
    }

    #[test]
    fn a_start_after_the_end_is_rejected_not_repaired() {
        let segs = vec![
            seg(1, 0, 1_000, true),
            seg(2, 2_000, 3_000, true),
            seg(3, 4_000, 5_000, true),
        ];
        let outcome = build_clips(&[pick(3, 2, 2, 2, 2)], &segs, &[], 10_000);
        assert!(outcome.clips.is_empty());
        assert_eq!(outcome.rejected.len(), 1);
        assert!(outcome.rejected[0].reason.contains("start_segment"));
        assert!(outcome.rejected[0].reason.contains("end_segment"));
    }

    #[test]
    fn equal_hook_and_cutoff_and_equal_start_and_end_are_valid() {
        let segs = vec![seg(1, 0, 1_000, true), seg(2, 2_000, 3_000, true)];
        let clip = one_clip(build_clips(&[pick(2, 2, 2, 2, 2)], &segs, &[], 10_000));
        assert_eq!(clip.hook.id, 2);
        assert_eq!(clip.last.id, 2);
    }

    #[test]
    fn a_rejected_pick_keeps_its_rank_and_does_not_shift_the_others() {
        let segs = vec![
            seg(1, 0, 1_000, true),
            seg(2, 2_000, 3_000, true),
            seg(3, 4_000, 5_000, true),
        ];
        let picks = [
            pick(1, 1, 1, 1, 1),
            pick(1, 9, 1, 1, 9),
            pick(3, 3, 3, 3, 3),
        ];
        let outcome = build_clips(&picks, &segs, &[], 10_000);
        let ranks: Vec<u32> = outcome.clips.iter().map(|c| c.rank).collect();
        assert_eq!(ranks, vec![1, 3]);
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].rank, 2);
    }

    // Rule 2: open on the hook.

    #[test]
    fn the_start_backs_up_to_the_sentence_start_7999_ms_before_the_hook() {
        let clip = backup_case(7_999);
        assert_eq!(clip.start_ms, 900);
        assert!(!has_flag(&clip, "starts mid-sentence"));
        assert!(clip.adjustments.is_empty());
    }

    #[test]
    fn the_start_backs_up_to_the_sentence_start_8000_ms_before_the_hook() {
        let clip = backup_case(8_000);
        assert_eq!(clip.start_ms, 900);
        assert!(!has_flag(&clip, "starts mid-sentence"));
    }

    #[test]
    fn the_clip_starts_at_the_hook_and_is_flagged_8001_ms_after_its_sentence_start() {
        let clip = backup_case(8_001);
        assert_eq!(clip.start_ms, 1_000 + 8_001 - 100);
        let flag = clip
            .flags
            .iter()
            .find(|f| f.contains("starts mid-sentence"))
            .expect("flag");
        assert!(flag.contains("8001"), "{flag}");
    }

    #[test]
    fn a_hook_that_starts_a_sentence_needs_no_backup_and_no_flag() {
        let segs = vec![seg(1, 0, 900, true), seg(2, 1_000, 1_900, true)];
        let clip = one_clip(build_clips(&[pick(2, 2, 2, 2, 2)], &segs, &[], 100_000));
        assert_eq!(clip.start_ms, 900);
        assert!(!has_flag(&clip, "starts mid-sentence"));
        assert!(clip.adjustments.is_empty());
    }

    #[test]
    fn segment_one_is_a_sentence_start() {
        // No segment before the hook ends a sentence, so the sentence starts at segment 1.
        let segs = vec![
            seg(1, 500, 1_500, false),
            seg(2, 2_000, 3_000, false),
            seg(3, 3_500, 4_500, true),
        ];
        let clip = one_clip(build_clips(&[pick(3, 3, 3, 3, 3)], &segs, &[], 100_000));
        assert_eq!(clip.start_ms, 400);
        assert!(!has_flag(&clip, "starts mid-sentence"));
    }

    #[test]
    fn what_came_before_the_hooks_sentence_is_dropped_and_recorded() {
        let segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, true),
            seg(3, 2_000, 2_900, false),
            seg(4, 3_000, 4_000, true),
        ];
        let clip = one_clip(build_clips(&[pick(1, 4, 4, 4, 4)], &segs, &[], 100_000));
        // The hook (segment 4) is mid-sentence; its sentence starts at segment 3.
        assert_eq!(clip.start_ms, 1_900);
        assert!(
            has_adjustment(&clip, "dropped segments 1-2"),
            "{:?}",
            clip.adjustments
        );
        assert!(has_adjustment(&clip, "2000 ms"), "{:?}", clip.adjustments);
    }

    #[test]
    fn a_single_dropped_segment_is_named_in_the_singular() {
        let segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, true),
            seg(3, 2_000, 2_900, true),
        ];
        let clip = one_clip(build_clips(&[pick(2, 3, 3, 3, 3)], &segs, &[], 100_000));
        assert!(
            has_adjustment(&clip, "dropped segment 2 "),
            "{:?}",
            clip.adjustments
        );
        assert!(has_adjustment(&clip, "before segment 3"));
    }

    #[test]
    fn material_before_a_mid_sentence_hook_is_dropped_too() {
        let segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, false),
            seg(3, 10_000, 11_000, true),
        ];
        let clip = one_clip(build_clips(&[pick(1, 3, 3, 3, 3)], &segs, &[], 100_000));
        assert!(has_flag(&clip, "starts mid-sentence"));
        assert_eq!(clip.start_ms, 9_900);
        assert!(
            has_adjustment(&clip, "dropped segments 1-2"),
            "{:?}",
            clip.adjustments
        );
    }

    #[test]
    fn a_model_start_after_the_hooks_sentence_start_is_moved_back_and_recorded() {
        let segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, false),
            seg(3, 2_000, 2_900, true),
        ];
        let clip = one_clip(build_clips(&[pick(3, 3, 3, 3, 3)], &segs, &[], 100_000));
        assert_eq!(clip.start_ms, 900);
        assert!(
            has_adjustment(&clip, "moved the start back from segment 3 to segment 2"),
            "{:?}",
            clip.adjustments
        );
    }

    // Rule 3: end on the cliffhanger.

    fn end_segments() -> Vec<Segment> {
        vec![
            seg(1, 1_000, 2_000, true),
            seg(2, 3_000, 4_000, false),
            seg(3, 5_000, 6_000, true),
            seg(4, 7_000, 8_000, true),
        ]
    }

    #[test]
    fn the_clip_never_ends_after_the_cutoff() {
        let clip = one_clip(build_clips(
            &[pick(1, 4, 1, 2, 3)],
            &end_segments(),
            &[],
            100_000,
        ));
        assert_eq!(clip.last.id, 3);
        assert_eq!(clip.end_ms, 6_150);
        assert!(
            has_adjustment(&clip, "dropped segment 4 after the cutoff"),
            "{:?}",
            clip.adjustments
        );
    }

    #[test]
    fn the_clip_never_ends_before_the_cutoff() {
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 3)],
            &end_segments(),
            &[],
            100_000,
        ));
        assert_eq!(clip.last.id, 3);
        assert_eq!(clip.end_ms, 6_150);
        assert!(
            has_adjustment(
                &clip,
                "extended the end from segment 2 to the cutoff, segment 3"
            ),
            "{:?}",
            clip.adjustments
        );
    }

    #[test]
    fn an_end_at_the_cutoff_changes_nothing() {
        let clip = one_clip(build_clips(
            &[pick(1, 3, 1, 2, 3)],
            &end_segments(),
            &[],
            100_000,
        ));
        assert_eq!(clip.end_ms, 6_150);
        assert!(clip.adjustments.is_empty());
    }

    // Rule 4: length.

    /// Segment 1 (the hook) starts at 1,000 ms; segment 2 ends so that the clip is exactly
    /// `length_ms` long with the fallback cut points. A pause of `dead_ms` (if any) lies
    /// between them.
    fn sized(length_ms: u64, dead_ms: u64) -> Clip {
        let segs = vec![
            seg(1, 1_000, 2_000, true),
            seg(2, 3_000 + dead_ms, length_ms + 750, true),
        ];
        let pauses = if dead_ms > 0 {
            vec![gap(2_100, 2_100 + dead_ms)]
        } else {
            Vec::new()
        };
        one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &pauses,
            1_000_000,
        ))
    }

    #[test]
    fn a_pause_of_180_ms_is_dead_air() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 4_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(2_100, 2_280)],
            100_000,
        ));
        assert_eq!(clip.dead_air, vec![gap(2_100, 2_280)]);
        assert_eq!(clip.length_ms, 3_250);
        assert_eq!(clip.tight_length_ms, 3_250 - 180);
    }

    #[test]
    fn a_pause_of_179_ms_is_not_dead_air() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 4_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(2_100, 2_279)],
            100_000,
        ));
        assert!(clip.dead_air.is_empty());
        assert_eq!(clip.tight_length_ms, clip.length_ms);
    }

    #[test]
    fn every_dead_pause_inside_the_clip_is_listed_in_order_and_subtracted() {
        let segs = vec![
            seg(1, 1_000, 2_000, true),
            seg(2, 4_000, 5_000, false),
            seg(3, 7_000, 8_000, true),
            seg(4, 9_000, 9_500, true),
        ];
        // Out of order on purpose; the third lies outside the clip (after segment 4 starts).
        let pauses = [gap(5_100, 6_000), gap(2_100, 3_000), gap(10_000, 11_000)];
        let clip = one_clip(build_clips(&[pick(1, 3, 1, 3, 3)], &segs, &pauses, 100_000));
        assert_eq!(clip.dead_air, vec![gap(2_100, 3_000), gap(5_100, 6_000)]);
        assert_eq!(clip.length_ms, 7_250);
        assert_eq!(clip.tight_length_ms, 7_250 - 900 - 900);
    }

    #[test]
    fn dead_air_is_clipped_to_the_clips_span() {
        // A pause straddling the clip's end (whisper's word end is before the pause starts
        // on the audio): only the part inside the clip counts, 250 ms here.
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 10_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(9_900, 11_000)],
            100_000,
        ));
        assert_eq!(clip.end_ms, 10_150);
        assert_eq!(clip.dead_air, vec![gap(9_900, 10_150)]);
        assert_eq!(clip.dead_air[0].duration_ms, 250);
    }

    #[test]
    fn dead_air_is_clipped_to_the_clips_start_too() {
        // A pause that ends after the first word starts is not a start, so the clip starts
        // 100 ms before the word (at 900 ms) and only 200 ms of the pause lie inside it.
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 4_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(500, 1_100)],
            100_000,
        ));
        assert_eq!(clip.start_ms, 900);
        assert_eq!(clip.dead_air, vec![gap(900, 1_100)]);
        assert_eq!(clip.tight_length_ms, clip.length_ms - 200);
    }

    #[test]
    fn a_sliver_of_a_long_pause_under_180_ms_inside_the_span_is_not_dead_air() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 10_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(9_971, 11_000)],
            100_000,
        ));
        assert!(clip.dead_air.is_empty());
    }

    #[test]
    fn the_150_ms_tail_in_the_pause_after_the_clip_is_never_dead_air() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 4_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(4_000, 6_000)],
            100_000,
        ));
        assert_eq!(clip.end_ms, 4_150);
        assert!(clip.dead_air.is_empty());
    }

    #[test]
    fn a_tight_length_of_29999_ms_is_flagged_under_the_minimum() {
        let clip = sized(29_999, 0);
        let flag = clip
            .flags
            .iter()
            .find(|f| f.contains("tight length"))
            .expect("flag");
        assert!(flag.contains("under"), "{flag}");
        assert!(flag.contains("29999"), "{flag}");
        // Never changed.
        assert_eq!(clip.length_ms, 29_999);
        assert_eq!(clip.tight_length_ms, 29_999);
    }

    #[test]
    fn a_tight_length_of_30000_ms_is_in_range() {
        let clip = sized(30_000, 0);
        assert_eq!(clip.tight_length_ms, 30_000);
        assert!(!has_flag(&clip, "tight length"));
    }

    #[test]
    fn a_tight_length_of_60000_ms_is_in_range() {
        let clip = sized(60_000, 0);
        assert_eq!(clip.tight_length_ms, 60_000);
        assert!(!has_flag(&clip, "tight length"));
    }

    #[test]
    fn a_tight_length_of_60001_ms_is_flagged_over_the_maximum() {
        let clip = sized(60_001, 0);
        let flag = clip
            .flags
            .iter()
            .find(|f| f.contains("tight length"))
            .expect("flag");
        assert!(flag.contains("over"), "{flag}");
        assert!(flag.contains("60001"), "{flag}");
        assert_eq!(clip.length_ms, 60_001);
    }

    #[test]
    fn the_length_rule_uses_the_tight_length_not_the_raw_length() {
        // 65 s raw, 59 s once 6 s of pauses are cut: in range.
        let clip = sized(65_000, 6_000);
        assert_eq!(clip.length_ms, 65_000);
        assert_eq!(clip.tight_length_ms, 59_000);
        assert!(!has_flag(&clip, "tight length"));
        // 35 s raw, 29 s tight: flagged, and the flag names both lengths.
        let clip = sized(35_000, 6_000);
        let flag = clip
            .flags
            .iter()
            .find(|f| f.contains("tight length"))
            .expect("flag");
        assert!(flag.contains("29000") && flag.contains("35000"), "{flag}");
    }

    // Rule 5: cut points.

    /// Segment 1 ends at `prev_end`, segment 2 (the hook) starts at 5,300, segment 3
    /// (the cutoff) spans 7,000-8,000, segment 4 starts at 9,000.
    fn cut_segments(prev_end: u64) -> Vec<Segment> {
        vec![
            seg(1, 0, prev_end, true),
            seg(2, 5_300, 6_000, false),
            seg(3, 7_000, 8_000, true),
            seg(4, 9_000, 10_000, true),
        ]
    }

    fn cut_clip(prev_end: u64, pauses: &[SilenceGap], duration_ms: u64) -> Clip {
        one_clip(build_clips(
            &[pick(2, 3, 2, 3, 3)],
            &cut_segments(prev_end),
            pauses,
            duration_ms,
        ))
    }

    #[test]
    fn the_start_is_the_end_of_a_pause_ending_600_ms_before_the_first_word() {
        assert_eq!(
            cut_clip(4_000, &[gap(4_000, 4_700)], 100_000).start_ms,
            4_700
        );
    }

    #[test]
    fn a_pause_ending_601_ms_before_the_first_word_is_out_of_reach() {
        assert_eq!(
            cut_clip(4_000, &[gap(4_000, 4_699)], 100_000).start_ms,
            5_200
        );
    }

    #[test]
    fn a_pause_ending_at_the_first_word_starts_the_clip_there() {
        assert_eq!(
            cut_clip(4_000, &[gap(4_500, 5_300)], 100_000).start_ms,
            5_300
        );
    }

    #[test]
    fn the_start_uses_the_latest_pause_before_the_first_word() {
        let pauses = [gap(4_500, 4_800), gap(4_000, 4_200)];
        assert_eq!(cut_clip(4_000, &pauses, 100_000).start_ms, 4_800);
    }

    #[test]
    fn a_pause_that_ends_after_the_first_word_starts_is_not_a_start() {
        assert_eq!(
            cut_clip(4_000, &[gap(5_000, 5_400)], 100_000).start_ms,
            5_200
        );
    }

    #[test]
    fn a_pause_before_the_previous_segments_end_is_not_a_start() {
        // Speech (segment 1, to 5,000 ms) lies between the pause and the first word.
        assert_eq!(
            cut_clip(5_000, &[gap(4_400, 4_800)], 100_000).start_ms,
            5_200
        );
    }

    #[test]
    fn without_a_pause_the_start_is_100_ms_before_the_first_word() {
        assert_eq!(cut_clip(4_000, &[], 100_000).start_ms, 5_200);
    }

    #[test]
    fn the_end_is_150_ms_into_a_long_pause_after_the_last_word() {
        assert_eq!(cut_clip(4_000, &[gap(8_000, 8_400)], 100_000).end_ms, 8_150);
        assert_eq!(cut_clip(4_000, &[gap(8_100, 8_900)], 100_000).end_ms, 8_250);
    }

    #[test]
    fn the_end_is_150_ms_into_a_pause_of_300_ms_and_149_into_one_of_299() {
        assert_eq!(cut_clip(4_000, &[gap(8_000, 8_300)], 100_000).end_ms, 8_150);
        assert_eq!(cut_clip(4_000, &[gap(8_000, 8_299)], 100_000).end_ms, 8_149);
    }

    #[test]
    fn the_end_is_half_a_short_pause() {
        assert_eq!(cut_clip(4_000, &[gap(8_000, 8_200)], 100_000).end_ms, 8_100);
        assert_eq!(cut_clip(4_000, &[gap(8_000, 8_100)], 100_000).end_ms, 8_050);
    }

    #[test]
    fn the_end_uses_the_first_pause_after_the_last_word() {
        let pauses = [gap(8_600, 8_900), gap(8_100, 8_400)];
        assert_eq!(cut_clip(4_000, &pauses, 100_000).end_ms, 8_250);
    }

    #[test]
    fn without_a_pause_the_end_is_150_ms_after_the_last_word() {
        assert_eq!(cut_clip(4_000, &[], 100_000).end_ms, 8_150);
    }

    #[test]
    fn a_pause_that_starts_before_the_last_word_ends_is_not_an_end() {
        assert_eq!(cut_clip(4_000, &[gap(7_900, 8_400)], 100_000).end_ms, 8_150);
    }

    #[test]
    fn a_pause_beyond_the_next_segments_start_is_not_an_end() {
        assert_eq!(cut_clip(4_000, &[gap(9_100, 9_600)], 100_000).end_ms, 8_150);
    }

    #[test]
    fn after_the_last_segment_of_the_video_any_later_pause_is_the_end() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 8_000, true)];
        let clip = one_clip(build_clips(
            &[pick(1, 2, 1, 2, 2)],
            &segs,
            &[gap(8_200, 9_000)],
            100_000,
        ));
        assert_eq!(clip.end_ms, 8_350);
    }

    #[test]
    fn the_start_is_clamped_at_zero() {
        let segs = vec![seg(1, 50, 1_000, true), seg(2, 2_000, 3_000, true)];
        let clip = one_clip(build_clips(&[pick(1, 2, 1, 2, 2)], &segs, &[], 100_000));
        assert_eq!(clip.start_ms, 0);
        assert_eq!(clip.length_ms, 3_150);
    }

    #[test]
    fn the_end_is_clamped_at_the_duration() {
        assert_eq!(cut_clip(4_000, &[], 8_100).end_ms, 8_100);
        assert_eq!(cut_clip(4_000, &[gap(8_000, 8_600)], 8_100).end_ms, 8_100);
    }

    #[test]
    fn length_is_the_distance_between_the_cut_points() {
        let clip = cut_clip(4_000, &[gap(4_000, 4_700), gap(8_000, 8_400)], 100_000);
        assert_eq!((clip.start_ms, clip.end_ms), (4_700, 8_150));
        assert_eq!(clip.length_ms, 3_450);
    }

    // Rule 6: other flags.

    /// Segment 1 (hook) starts at 1,000 ms so the clip starts at 900 ms; segment 2 (payoff)
    /// starts at `payoff_start`; segment 3 is the cutoff.
    fn payoff_case(payoff_start: u64, pauses: &[SilenceGap]) -> Clip {
        let segs = vec![
            seg(1, 1_000, 2_000, true),
            seg(2, payoff_start, payoff_start + 1_000, false),
            seg(3, payoff_start + 2_000, payoff_start + 3_000, true),
        ];
        one_clip(build_clips(
            &[pick(1, 3, 1, 2, 3)],
            &segs,
            pauses,
            1_000_000,
        ))
    }

    #[test]
    fn a_payoff_6000_ms_into_the_clip_is_in_range() {
        let clip = payoff_case(6_900, &[]);
        assert_eq!(clip.payoff_at_ms, Some(6_000));
        assert!(!has_flag(&clip, "payoff"));
    }

    #[test]
    fn a_payoff_5999_ms_into_the_clip_is_flagged() {
        let clip = payoff_case(6_899, &[]);
        assert_eq!(clip.payoff_at_ms, Some(5_999));
        assert!(has_flag(&clip, "payoff"));
    }

    #[test]
    fn a_payoff_12000_ms_into_the_clip_is_in_range() {
        let clip = payoff_case(12_900, &[]);
        assert_eq!(clip.payoff_at_ms, Some(12_000));
        assert!(!has_flag(&clip, "payoff"));
    }

    #[test]
    fn a_payoff_12001_ms_into_the_clip_is_flagged_with_its_time() {
        let clip = payoff_case(12_901, &[]);
        let flag = clip
            .flags
            .iter()
            .find(|f| f.contains("payoff"))
            .expect("flag");
        assert!(flag.contains("12001"), "{flag}");
    }

    #[test]
    fn the_payoff_time_is_measured_after_dead_air_before_it_is_cut() {
        // 7,000 ms of raw time, of which a 1,000 ms pause is dead air: 6,000 tight.
        let clip = payoff_case(7_900, &[gap(2_100, 3_100)]);
        assert_eq!(clip.payoff_at_ms, Some(6_000));
        assert!(!has_flag(&clip, "payoff"));
        // 6,500 raw is in range, but 5,500 tight is not.
        let clip = payoff_case(7_400, &[gap(2_100, 3_100)]);
        assert_eq!(clip.payoff_at_ms, Some(5_500));
        assert!(has_flag(&clip, "payoff"));
        // 13,000 raw is out of range, but 11,000 tight is in.
        let clip = payoff_case(13_900, &[gap(2_100, 4_100)]);
        assert_eq!(clip.payoff_at_ms, Some(11_000));
        assert!(!has_flag(&clip, "payoff"));
    }

    #[test]
    fn dead_air_after_the_payoff_start_does_not_move_it() {
        let clip = payoff_case(7_900, &[gap(9_000, 9_800)]);
        assert_eq!(clip.payoff_at_ms, Some(7_000));
    }

    #[test]
    fn a_payoff_on_the_first_segment_lands_at_the_start_lead_and_is_flagged() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 3_000, 4_000, true)];
        let clip = one_clip(build_clips(&[pick(1, 2, 1, 1, 2)], &segs, &[], 100_000));
        // The clip starts 100 ms before the first word.
        assert_eq!(clip.payoff_at_ms, Some(100));
        assert!(has_flag(&clip, "payoff"));
    }

    #[test]
    fn a_payoff_before_the_clip_is_flagged_as_outside_and_has_no_time() {
        let segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, true),
            seg(3, 3_000, 4_000, true),
        ];
        let clip = one_clip(build_clips(&[pick(2, 3, 2, 1, 3)], &segs, &[], 100_000));
        assert_eq!(clip.payoff_at_ms, None);
        assert!(has_flag(&clip, "outside the clip"));
        assert!(!has_flag(&clip, "payoff lands"));
        // The quote is still segment 1's own text.
        assert_eq!(clip.payoff.text, segs[0].text);
    }

    #[test]
    fn a_payoff_after_the_cutoff_is_flagged_as_outside_and_has_no_time() {
        let segs = vec![
            seg(1, 1_000, 2_000, true),
            seg(2, 3_000, 4_000, true),
            seg(3, 5_000, 6_000, true),
        ];
        let clip = one_clip(build_clips(&[pick(1, 2, 1, 3, 2)], &segs, &[], 100_000));
        assert_eq!(clip.payoff_at_ms, None);
        assert!(has_flag(&clip, "outside the clip"));
    }

    /// Sentence segments one second long every two seconds: segment `i` spans
    /// `(2i - 1)`-`2i` thousand ms.
    fn six_sentences() -> Vec<Segment> {
        (1..=6u32)
            .map(|i| {
                let start = (u64::from(i) * 2 - 1) * 1_000;
                seg(i, start, start + 1_000, true)
            })
            .collect()
    }

    #[test]
    fn a_clip_overlapping_a_higher_ranked_clip_is_flagged_and_the_higher_one_is_not() {
        let picks = [pick(1, 3, 1, 2, 3), pick(3, 5, 3, 4, 5)];
        let outcome = build_clips(&picks, &six_sentences(), &[], 100_000);
        assert_eq!(outcome.clips.len(), 2);
        let (first, second) = (&outcome.clips[0], &outcome.clips[1]);
        assert!(!has_flag(first, "overlaps"));
        let flag = second
            .flags
            .iter()
            .find(|f| f.contains("overlaps"))
            .expect("flag");
        assert!(flag.contains("clip 1"), "{flag}");
    }

    #[test]
    fn clips_that_only_touch_do_not_overlap() {
        // The first ends at 2,150; the second starts at 2,250 - 100 = 2,150.
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 2_250, 3_000, true)];
        let picks = [pick(1, 1, 1, 1, 1), pick(2, 2, 2, 2, 2)];
        let outcome = build_clips(&picks, &segs, &[], 100_000);
        assert_eq!(outcome.clips.len(), 2);
        assert_eq!(outcome.clips[0].end_ms, 2_150);
        assert_eq!(outcome.clips[1].start_ms, 2_150);
        assert!(!has_flag(&outcome.clips[1], "overlaps"));
    }

    #[test]
    fn clips_that_share_one_millisecond_overlap() {
        let segs = vec![seg(1, 1_000, 2_000, true), seg(2, 2_249, 3_000, true)];
        let picks = [pick(1, 1, 1, 1, 1), pick(2, 2, 2, 2, 2)];
        let outcome = build_clips(&picks, &segs, &[], 100_000);
        assert_eq!(outcome.clips.len(), 2);
        assert_eq!(outcome.clips[1].start_ms, 2_149);
        assert!(has_flag(&outcome.clips[1], "overlaps"));
    }

    #[test]
    fn a_rejected_pick_is_not_a_clip_to_overlap_and_the_flag_names_the_right_rank() {
        let segs = six_sentences();
        // Rank 2 is invalid; rank 3 overlaps rank 1 only.
        let picks = [
            pick(1, 3, 1, 2, 3),
            pick(1, 9, 1, 2, 9),
            pick(3, 5, 3, 4, 5),
        ];
        let outcome = build_clips(&picks, &segs, &[], 100_000);
        assert_eq!(outcome.clips.len(), 2);
        let third = &outcome.clips[1];
        assert_eq!(third.rank, 3);
        let flag = third
            .flags
            .iter()
            .find(|f| f.contains("overlaps"))
            .expect("flag");
        assert!(flag.contains("clip 1"), "{flag}");
    }

    #[test]
    fn a_clip_is_flagged_only_for_the_higher_ranked_clips_it_overlaps() {
        let segs = six_sentences();
        let picks = [
            pick(1, 1, 1, 1, 1),
            pick(4, 5, 4, 5, 5),
            pick(5, 6, 5, 6, 6),
        ];
        let outcome = build_clips(&picks, &segs, &[], 100_000);
        assert_eq!(outcome.clips.len(), 3);
        assert!(!has_flag(&outcome.clips[1], "overlaps"));
        let flag = outcome.clips[2]
            .flags
            .iter()
            .find(|f| f.contains("overlaps"))
            .expect("flag");
        assert!(
            flag.contains("clip 2") && !flag.contains("clip 1"),
            "{flag}"
        );
    }

    // Output.

    #[test]
    fn quotes_are_the_segments_exact_text() {
        let mut segs = vec![
            seg(1, 0, 900, true),
            seg(2, 1_000, 1_900, false),
            seg(3, 2_000, 3_000, false),
            seg(4, 3_500, 4_500, true),
        ];
        segs[0].text = "pehle ki baat.".to_string();
        segs[1].text = "  Hello,   दुनिया… \"quoted\" ".to_string();
        segs[2].text = "payoff: 98%?".to_string();
        segs[3].text = "to dekhte hain...".to_string();
        let clip = one_clip(build_clips(&[pick(1, 4, 3, 3, 4)], &segs, &[], 100_000));
        // The hook is the model's hook (segment 3) although the clip opens at segment 2.
        assert_eq!(clip.start_ms, 900);
        assert_eq!(
            clip.hook,
            SegmentRef {
                id: 3,
                text: segs[2].text.clone()
            }
        );
        assert_eq!(
            clip.payoff,
            SegmentRef {
                id: 3,
                text: segs[2].text.clone()
            }
        );
        assert_eq!(
            clip.last,
            SegmentRef {
                id: 4,
                text: segs[3].text.clone()
            }
        );
    }

    #[test]
    fn the_models_words_pass_through_and_clips_keep_the_models_order() {
        let segs = six_sentences();
        let mut first = pick(1, 1, 1, 1, 1);
        first.title = "First".to_string();
        let mut second = pick(5, 6, 5, 6, 6);
        second.title = "Second".to_string();
        second.open_loop = "but what about X?".to_string();
        second.visual_suggestion = "zoom on the slide".to_string();
        second.why = "a strong claim".to_string();
        let outcome = build_clips(&[first, second], &segs, &[], 100_000);
        let titles: Vec<&str> = outcome.clips.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["First", "Second"]);
        let c = &outcome.clips[1];
        assert_eq!(c.rank, 2);
        assert_eq!(c.open_loop, "but what about X?");
        assert_eq!(c.visual_suggestion, "zoom on the slide");
        assert_eq!(c.why, "a strong claim");
    }

    #[test]
    fn a_clip_that_meets_every_rule_has_no_flags_and_no_adjustments() {
        // 40 s clip with the payoff 9 s in, cut points inside pauses.
        let segs = vec![
            seg(1, 0, 4_000, true),
            seg(2, 5_000, 6_000, false),
            seg(3, 14_000, 15_000, false),
            seg(4, 44_000, 45_000, true),
        ];
        let pauses = [gap(4_000, 5_000), gap(45_000, 46_000)];
        let clip = one_clip(build_clips(&[pick(2, 4, 2, 3, 4)], &segs, &pauses, 100_000));
        assert_eq!(clip.start_ms, 5_000);
        assert_eq!(clip.end_ms, 45_150);
        assert_eq!(clip.payoff_at_ms, Some(9_000));
        assert!(clip.flags.is_empty(), "{:?}", clip.flags);
        assert!(clip.adjustments.is_empty());
    }
}
