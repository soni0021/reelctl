//! `timeline cut-silence`: remove silence gaps (and filler words) from both tracks.

use edit_dna::{AsrBackend, EditDna};
use serde_json::{json, Value};

use crate::error::{Result, TimelineError};
use crate::otio::{update_reelctl, Item, MediaReference, Timeline, Track, VIDEO_KIND};
use crate::tracks::{
    ceil_frame, check_editable, external_url, floor_frame, frame_ms, frames_of, item_range, piece,
    timeline_frames, FADE_KEY,
};

/// The key under the timeline's `metadata.reelctl` that lists the removed spans.
pub const CUTS_KEY: &str = "cuts";

/// Arguments of `cut-silence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutOptions {
    /// A gap is removed when it is at least this long in the DNA.
    pub min_gap_ms: u64,
    /// Length of the audio fade on each side of a join.
    pub fade_ms: u64,
    /// Whole words to remove (compared lowercase, without punctuation).
    pub fillers: Vec<String>,
}

impl Default for CutOptions {
    fn default() -> Self {
        Self {
            min_gap_ms: 180,
            fade_ms: 30,
            fillers: vec!["um".to_string(), "uh".to_string()],
        }
    }
}

/// One span taken out of the media, in source milliseconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedSpan {
    pub start_ms: u64,
    pub end_ms: u64,
    pub start_frame: i64,
    pub end_frame: i64,
    /// `silence` or `filler`.
    pub reason: String,
    /// The filler words in the span.
    pub text: Option<String>,
}

/// What a `cut-silence` run changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CutReport {
    pub removed: Vec<RemovedSpan>,
}

impl CutReport {
    pub fn frames_removed(&self) -> i64 {
        self.removed
            .iter()
            .map(|span| span.end_frame - span.start_frame)
            .sum()
    }
}

/// The DNA and the media may differ by this much in length before the DNA is
/// taken to be of another file.
const DURATION_TOLERANCE_MS: u64 = 100;

/// A span of the media to remove, in source milliseconds, before snapping.
struct Raw {
    start_ms: u64,
    end_ms: u64,
    /// The start comes from a silence gap (snaps inward) and not from a word.
    start_silence: bool,
    end_silence: bool,
    silence: bool,
    /// Indexes into `dna.words` of the filler words in the span.
    fillers: Vec<usize>,
}

/// A word compared as a filler: lowercase letters and digits only.
fn normalize(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn raw_spans(dna: &EditDna, options: &CutOptions) -> Vec<Raw> {
    let mut spans = Vec::new();
    for gap in &dna.silence_gaps {
        let length = gap.end_ms.saturating_sub(gap.start_ms);
        if length >= options.min_gap_ms && length > 0 {
            spans.push(Raw {
                start_ms: gap.start_ms,
                end_ms: gap.end_ms,
                start_silence: true,
                end_silence: true,
                silence: true,
                fillers: Vec::new(),
            });
        }
    }
    // Only a speech recognizer gives words. Voiced bursts are not words.
    if dna.asr_backend == AsrBackend::WhisperCpp {
        let fillers: Vec<String> = options
            .fillers
            .iter()
            .map(|word| normalize(word))
            .filter(|word| !word.is_empty())
            .collect();
        for (index, word) in dna.words.iter().enumerate() {
            if word.end_ms > word.start_ms && fillers.contains(&normalize(&word.text)) {
                spans.push(Raw {
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                    start_silence: false,
                    end_silence: false,
                    silence: false,
                    fillers: vec![index],
                });
            }
        }
    }
    // Earliest first; at a tie the silence, whose edge snaps inward, leads.
    spans.sort_by_key(|span| (span.start_ms, !span.start_silence));
    let mut merged: Vec<Raw> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start_ms <= last.end_ms => {
                if span.end_ms > last.end_ms || (span.end_ms == last.end_ms && span.end_silence) {
                    last.end_ms = span.end_ms;
                    last.end_silence = span.end_silence;
                }
                last.silence |= span.silence;
                last.fillers.extend(span.fillers);
            }
            _ => merged.push(span),
        }
    }
    merged
}

/// Frames `[start, end)` to take out for `span`. A silence edge snaps into the
/// pause (start up, end down); a word edge snaps out (start down, end up). Either is
/// then held off the neighbouring words, so a cut never lands inside one.
fn snap(span: &Raw, dna: &EditDna, rate: f64) -> (i64, i64) {
    let start = if span.start_silence {
        ceil_frame(span.start_ms, rate)
    } else {
        floor_frame(span.start_ms, rate)
    };
    let end = if span.end_silence {
        floor_frame(span.end_ms, rate)
    } else {
        ceil_frame(span.end_ms, rate)
    };
    let neighbour = |index: usize| span.fillers.contains(&index);
    let previous_end = dna
        .words
        .iter()
        .enumerate()
        .filter(|(index, word)| !neighbour(*index) && word.start_ms < span.start_ms)
        .map(|(_, word)| word.end_ms)
        .max();
    let next_start = dna
        .words
        .iter()
        .enumerate()
        .filter(|(index, word)| !neighbour(*index) && word.end_ms > span.end_ms)
        .map(|(_, word)| word.start_ms)
        .min();
    let start = previous_end.map_or(start, |ms| start.max(ceil_frame(ms, rate)));
    let end = next_start.map_or(end, |ms| end.min(floor_frame(ms, rate)));
    (start, end)
}

/// Sorted, non-overlapping, non-touching ranges.
fn union(mut ranges: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    ranges.sort_unstable();
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// `(start, duration)` pieces of a clip's source range `[start, start + duration)`
/// that are left when `removals` (sorted, disjoint) are taken out.
fn kept_pieces(start: i64, duration: i64, removals: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let end = start + duration;
    let mut pieces = Vec::new();
    let mut cursor = start;
    for &(from, to) in removals {
        let from = from.max(start);
        let to = to.min(end);
        if from < to {
            if from > cursor {
                pieces.push((cursor, from - cursor));
            }
            cursor = to;
        }
    }
    if cursor < end {
        pieces.push((cursor, end - cursor));
    }
    pieces
}

/// Source ranges of the clips that play media, in the video track.
fn media_ranges(track: &Track, rate: f64) -> Result<Vec<(i64, i64)>> {
    let mut ranges = Vec::new();
    for item in &track.children {
        if let Item::Clip(clip) = item {
            if external_url(clip).is_some() {
                let (start, duration) = item_range(item, rate)?;
                ranges.push((start, start + duration));
            }
        }
    }
    Ok(ranges)
}

fn remove_from_track(track: &mut Track, removals: &[(i64, i64)], rate: f64) -> Result<()> {
    let mut rebuilt = Vec::with_capacity(track.children.len());
    for item in &track.children {
        let plays_media = matches!(item, Item::Clip(clip) if external_url(clip).is_some());
        if !plays_media {
            rebuilt.push(item.clone());
            continue;
        }
        let (start, duration) = item_range(item, rate)?;
        for (piece_start, piece_duration) in kept_pieces(start, duration, removals) {
            rebuilt.push(piece(item, piece_start - start, piece_duration, rate)?);
        }
    }
    track.children = rebuilt;
    Ok(())
}

/// Remove the silence gaps of `dna` that are at least `min_gap_ms` long, and the
/// filler words, from every track of `timeline`. Times are the DNA's, in the media's
/// own time, snapped to the frame grid; a span already removed is not removed twice,
/// so running this again with the same arguments changes nothing.
pub fn cut_silence(
    timeline: &mut Timeline,
    dna: &EditDna,
    options: &CutOptions,
) -> Result<CutReport> {
    let rate = check_editable(timeline)?;
    let media_ms = media_length_ms(timeline, rate)?;
    if dna.duration_ms.abs_diff(media_ms) > DURATION_TOLERANCE_MS {
        return Err(TimelineError::invalid(format!(
            "the edit DNA is {} ms long and the project's media is {media_ms} ms; they are not the same file",
            dna.duration_ms
        )));
    }

    let snapped: Vec<(Raw, (i64, i64))> = raw_spans(dna, options)
        .into_iter()
        .map(|span| {
            let frames = snap(&span, dna, rate);
            (span, frames)
        })
        .filter(|(_, (start, end))| end > start)
        .collect();

    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::invalid("the project has no video track"))?;
    let clips = media_ranges(video, rate)?;
    let mut removed = Vec::new();
    for (span, (start, end)) in &snapped {
        // Only what is still in the project counts: a span already cut earlier
        // has nothing left to remove.
        let present = union(
            clips
                .iter()
                .map(|&(from, to)| (from.max(*start), to.min(*end)))
                .filter(|(from, to)| from < to)
                .collect(),
        );
        for (from, to) in present {
            let text: Vec<&str> = span
                .fillers
                .iter()
                .filter_map(|&index| dna.words.get(index))
                .map(|word| word.text.as_str())
                .collect();
            removed.push(RemovedSpan {
                start_ms: frame_ms(from, rate),
                end_ms: frame_ms(to, rate),
                start_frame: from,
                end_frame: to,
                reason: if span.silence { "silence" } else { "filler" }.to_string(),
                text: if text.is_empty() {
                    None
                } else {
                    Some(text.join(" "))
                },
            });
        }
    }
    if removed.is_empty() {
        return Ok(CutReport::default());
    }

    let total = timeline_frames(timeline)?;
    let removed_frames: i64 = removed
        .iter()
        .map(|span| span.end_frame - span.start_frame)
        .sum();
    if removed_frames >= total {
        return Err(TimelineError::invalid(
            "the cuts would remove all of the video; nothing would be left to render",
        ));
    }

    let ranges = union(
        removed
            .iter()
            .map(|span| (span.start_frame, span.end_frame))
            .collect(),
    );
    let mut work = timeline.clone();
    for track in &mut work.tracks.children {
        remove_from_track(track, &ranges, rate)?;
    }
    record_cuts(&mut work, &removed);
    set_fades(&mut work, options.fade_ms);
    check_editable(&work)?;
    *timeline = work;
    Ok(CutReport { removed })
}

/// Length of the media in milliseconds: the `available_range` of its reference.
fn media_length_ms(timeline: &Timeline, rate: f64) -> Result<u64> {
    for track in &timeline.tracks.children {
        for item in &track.children {
            let Item::Clip(clip) = item else { continue };
            if let Ok(MediaReference::External(external)) = clip.active_reference() {
                let range = external.available_range.as_ref().ok_or_else(|| {
                    TimelineError::invalid(format!(
                        "\"{}\" has no available_range, so its length is unknown",
                        external.target_url
                    ))
                })?;
                let frames = frames_of(&range.duration, rate, "available_range duration")?;
                return Ok(frame_ms(frames, rate));
            }
        }
    }
    Err(TimelineError::invalid(
        "the project has no clip with a media file",
    ))
}

fn record_cuts(timeline: &mut Timeline, removed: &[RemovedSpan]) {
    update_reelctl(&mut timeline.metadata, |map| {
        let mut cuts: Vec<Value> = match map.remove(CUTS_KEY) {
            Some(Value::Array(existing)) => existing,
            _ => Vec::new(),
        };
        for span in removed {
            let mut entry = json!({
                "reason": span.reason,
                "source_start_ms": span.start_ms,
                "source_end_ms": span.end_ms,
                "start_frame": span.start_frame,
                "end_frame": span.end_frame,
            });
            if let (Some(text), Some(object)) = (&span.text, entry.as_object_mut()) {
                object.insert("text".to_string(), Value::String(text.clone()));
            }
            cuts.push(entry);
        }
        cuts.sort_by_key(|entry| {
            entry
                .get("start_frame")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        });
        map.insert(CUTS_KEY.to_string(), Value::Array(cuts));
    });
}

/// Record the fade length on every clip that plays media. The renderer applies it
/// on each side of a join where the footage is not continuous.
fn set_fades(timeline: &mut Timeline, fade_ms: u64) {
    for track in &mut timeline.tracks.children {
        for item in &mut track.children {
            if let Item::Clip(clip) = item {
                if external_url(clip).is_some() {
                    update_reelctl(&mut clip.metadata, |map| {
                        map.insert(FADE_KEY.to_string(), json!(fade_ms));
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::otio::{reelctl, Item, AUDIO_KIND, VIDEO_KIND};
    use crate::testutil::{dna, project, with_words};
    use crate::tracks::{check_editable, item_range, timeline_frames};
    use edit_dna::AsrBackend;

    /// 10 s at 100 fps: a frame is 10 ms, so milliseconds and frames line up.
    fn ten_seconds() -> Timeline {
        project(100.0, 1000)
    }

    fn options() -> CutOptions {
        CutOptions::default()
    }

    fn kept(timeline: &Timeline) -> Vec<(i64, i64)> {
        let video = timeline.track(VIDEO_KIND).expect("video");
        video
            .children
            .iter()
            .map(|item| item_range(item, 100.0).expect("range"))
            .collect()
    }

    #[test]
    fn a_gap_is_removed_from_the_180_ms_boundary_up() {
        let mut timeline = ten_seconds();
        let gaps = dna(
            10_000,
            100.0,
            &[(1_000, 1_179), (2_000, 2_180), (3_000, 3_181)],
        );
        let report = cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        // 179 ms stays. 180 ms goes: frames 200..218 (the end snaps down to 218).
        // 181 ms goes: frames 300..318.
        let spans: Vec<(i64, i64)> = report
            .removed
            .iter()
            .map(|span| (span.start_frame, span.end_frame))
            .collect();
        assert_eq!(spans, vec![(200, 218), (300, 318)]);
        assert_eq!(
            kept(&timeline),
            vec![(0, 200), (218, 82), (318, 682)],
            "kept ranges"
        );
        assert_eq!(timeline_frames(&timeline).expect("frames"), 964);
    }

    #[test]
    fn min_gap_ms_moves_the_boundary() {
        let mut timeline = ten_seconds();
        let gaps = dna(10_000, 100.0, &[(1_000, 1_179)]);
        let opts = CutOptions {
            min_gap_ms: 100,
            ..options()
        };
        let report = cut_silence(&mut timeline, &gaps, &opts).expect("cut");
        assert_eq!(report.removed.len(), 1);
    }

    #[test]
    fn snapping_goes_into_the_pause_never_into_a_word() {
        // 30 fps: the start of a gap snaps up to the next frame, the end down to
        // the previous one, so the kept footage grows into the pause.
        let mut timeline = project(30.0, 300);
        let gaps = dna(10_000, 30.0, &[(1_010, 1_500)]);
        let report = cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        // 1010 ms is frame 30.3 -> 31; 1500 ms is frame 45.0 -> 45.
        assert_eq!(
            (report.removed[0].start_frame, report.removed[0].end_frame),
            (31, 45)
        );
        let (start_ms, end_ms) = (report.removed[0].start_ms, report.removed[0].end_ms);
        assert!(start_ms >= 1_010, "start {start_ms} is inside the word");
        assert!(end_ms <= 1_500, "end {end_ms} is inside the next word");
    }

    #[test]
    fn a_gap_that_collapses_after_snapping_is_dropped() {
        // 180 ms at 5 fps (200 ms frames) holds no whole frame once snapped inward.
        let mut timeline = project(5.0, 50);
        let gaps = dna(10_000, 5.0, &[(1_100, 1_300)]);
        let report = cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(timeline_frames(&timeline).expect("frames"), 50);
    }

    #[test]
    fn a_gap_at_the_start_and_one_at_the_end_trim_the_clip() {
        let mut timeline = ten_seconds();
        let gaps = dna(10_000, 100.0, &[(0, 500), (9_500, 10_000)]);
        cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        assert_eq!(kept(&timeline), vec![(50, 900)]);
    }

    #[test]
    fn overlapping_gaps_are_one_removal() {
        let mut timeline = ten_seconds();
        let gaps = dna(10_000, 100.0, &[(1_000, 1_300), (1_200, 1_500)]);
        let report = cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        assert_eq!(report.removed.len(), 1);
        assert_eq!(
            (report.removed[0].start_frame, report.removed[0].end_frame),
            (100, 150)
        );
    }

    #[test]
    fn both_tracks_stay_the_same_length_and_the_join_is_recorded() {
        let mut timeline = ten_seconds();
        let gaps = dna(10_000, 100.0, &[(2_000, 2_500)]);
        cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        check_editable(&timeline).expect("tracks match");
        let video = timeline.track(VIDEO_KIND).expect("video");
        let audio = timeline.track(AUDIO_KIND).expect("audio");
        assert_eq!(video.children.len(), 2);
        assert_eq!(audio.children.len(), 2);
        let cuts = reelctl(&timeline.metadata)
            .and_then(|map| map.get(CUTS_KEY))
            .and_then(|value| value.as_array())
            .expect("cuts");
        assert_eq!(cuts.len(), 1);
        assert_eq!(cuts[0]["reason"], "silence");
        assert_eq!(cuts[0]["source_start_ms"], 2_000);
        assert_eq!(cuts[0]["source_end_ms"], 2_500);
        for item in video.children.iter().chain(audio.children.iter()) {
            let Item::Clip(clip) = item else {
                panic!("clip expected")
            };
            let fade = reelctl(&clip.metadata).and_then(|map| map.get("fade_ms"));
            assert_eq!(fade.and_then(|v| v.as_u64()), Some(30));
        }
    }

    #[test]
    fn the_fade_length_is_the_argument() {
        let mut timeline = ten_seconds();
        let gaps = dna(10_000, 100.0, &[(2_000, 2_500)]);
        let opts = CutOptions {
            fade_ms: 45,
            ..options()
        };
        cut_silence(&mut timeline, &gaps, &opts).expect("cut");
        let video = timeline.track(VIDEO_KIND).expect("video");
        let Item::Clip(clip) = &video.children[0] else {
            panic!("clip expected")
        };
        let fade = reelctl(&clip.metadata).and_then(|map| map.get("fade_ms"));
        assert_eq!(fade.and_then(|v| v.as_u64()), Some(45));
    }

    #[test]
    fn a_filler_word_is_removed_with_its_punctuation_and_case_ignored() {
        let mut timeline = ten_seconds();
        let spoken = with_words(
            dna(10_000, 100.0, &[]),
            &[
                ("So", 1_000, 1_300),
                ("Um,", 1_500, 1_800),
                ("umbrella", 2_000, 2_400),
                ("UH.", 3_000, 3_200),
            ],
        );
        let report = cut_silence(&mut timeline, &spoken, &options()).expect("cut");
        let spans: Vec<(i64, i64, &str)> = report
            .removed
            .iter()
            .map(|span| (span.start_frame, span.end_frame, span.reason.as_str()))
            .collect();
        assert_eq!(spans, vec![(150, 180, "filler"), (300, 320, "filler")]);
        assert_eq!(report.removed[0].text.as_deref(), Some("Um,"));
    }

    #[test]
    fn fillers_come_from_the_option() {
        let mut timeline = ten_seconds();
        let spoken = with_words(
            dna(10_000, 100.0, &[]),
            &[("like", 1_000, 1_300), ("um", 2_000, 2_300)],
        );
        let opts = CutOptions {
            fillers: vec!["like".to_string()],
            ..options()
        };
        let report = cut_silence(&mut timeline, &spoken, &opts).expect("cut");
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].start_frame, 100);
    }

    #[test]
    fn a_filler_next_to_a_pause_is_one_removal() {
        let mut timeline = ten_seconds();
        let spoken = with_words(
            dna(10_000, 100.0, &[(2_300, 2_700)]),
            &[("um", 2_000, 2_300), ("so", 2_700, 3_000)],
        );
        let report = cut_silence(&mut timeline, &spoken, &options()).expect("cut");
        assert_eq!(report.removed.len(), 1);
        assert_eq!(
            (report.removed[0].start_frame, report.removed[0].end_frame),
            (200, 270)
        );
    }

    #[test]
    fn a_filler_never_cuts_into_its_neighbour() {
        // 30 fps. "so" runs to 2010 ms (frame 60.3), "um" starts at 2000 ms: the
        // filler's start would floor to frame 60, inside "so"; it is held at 61.
        let mut timeline = project(30.0, 300);
        let spoken = with_words(
            dna(10_000, 30.0, &[]),
            &[
                ("so", 1_500, 2_010),
                ("um", 2_000, 2_500),
                ("go", 2_520, 3_000),
            ],
        );
        let report = cut_silence(&mut timeline, &spoken, &options()).expect("cut");
        assert_eq!(report.removed.len(), 1);
        assert_eq!(report.removed[0].start_frame, 61);
        // "go" starts at 2520 ms = frame 75.6; the end is held at or before it.
        assert!(report.removed[0].end_frame <= 75, "{report:?}");
    }

    #[test]
    fn bursts_are_not_words_so_no_filler_is_removed() {
        let mut timeline = ten_seconds();
        let mut bursts = with_words(dna(10_000, 100.0, &[]), &[("um", 1_000, 1_300)]);
        bursts.asr_backend = AsrBackend::SilenceBursts;
        let report = cut_silence(&mut timeline, &bursts, &options()).expect("cut");
        assert!(report.removed.is_empty());
    }

    #[test]
    fn running_it_twice_changes_nothing_the_second_time() {
        let mut timeline = ten_seconds();
        let gaps = with_words(
            dna(10_000, 100.0, &[(0, 300), (2_000, 2_500), (9_700, 10_000)]),
            &[("um", 4_000, 4_300)],
        );
        let first = cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        assert!(!first.removed.is_empty());
        let after_first = timeline.to_json().expect("json");
        let second = cut_silence(&mut timeline, &gaps, &options()).expect("cut");
        assert!(second.removed.is_empty(), "{second:?}");
        assert_eq!(timeline.to_json().expect("json"), after_first);
    }

    #[test]
    fn a_dna_of_another_length_is_refused() {
        let mut timeline = ten_seconds();
        let other = dna(30_000, 100.0, &[(2_000, 2_500)]);
        let error = cut_silence(&mut timeline, &other, &options()).expect_err("wrong dna");
        assert!(error.to_string().contains("30000"), "{error}");
        assert_eq!(timeline_frames(&timeline).expect("frames"), 1000);
    }

    #[test]
    fn removing_everything_is_an_error_and_leaves_the_project_alone() {
        let mut timeline = ten_seconds();
        let gaps = dna(10_000, 100.0, &[(0, 10_000)]);
        let before = timeline.to_json().expect("json");
        let error = cut_silence(&mut timeline, &gaps, &options()).expect_err("empty");
        assert!(error.to_string().contains("nothing"), "{error}");
        assert_eq!(timeline.to_json().expect("json"), before);
    }
}
