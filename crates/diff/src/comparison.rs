//! A draft against one to three outliers: per-reel metrics, cut rhythm in 5 s
//! windows, and two relative findings.
//!
//! The findings live in [`Comparison::flags`], never in [`Report::flags`], so
//! everything that reads the older flags keeps working.

use edit_dna::{EditDna, PAYOFF_WINDOW_END_MS, PAYOFF_WINDOW_START_MS};
use serde::{Deserialize, Serialize};

use crate::{push_dead_air, push_static_windows, DiffFlag, DEAD_AIR_LIMIT_MS};

/// Length of one cut-rhythm window.
pub const RHYTHM_WINDOW_MS: u64 = 5_000;

/// The draft's pauses per minute must exceed the outliers' median by this factor...
pub const PAUSE_DENSITY_RATIO: f64 = 1.5;

/// ...and by at least this many pauses per minute.
pub const PAUSE_DENSITY_MIN_EXTRA_PER_MINUTE: f64 = 3.0;

/// A decompiled reel and the name it is shown under.
#[derive(Debug, Clone, Copy)]
pub struct NamedReel<'a> {
    pub name: &'a str,
    pub dna: &'a EditDna,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReelMetrics {
    pub name: String,
    pub duration_ms: u64,
    pub cut_count: usize,
    pub mean_shot_ms: u64,
    pub longest_shot_ms: u64,
    /// Silence gaps longer than the dead-air limit.
    pub dead_air_count: usize,
    pub dead_air_per_minute: f64,
    /// Silence gap time as a share of the duration, in `0..=1`.
    pub silent_share: f64,
    pub words_per_minute: f64,
    /// `whisper_cpp`, or `silence_bursts` when the rate is an estimate.
    pub words_source: String,
    pub integrated_lufs: f64,
}

/// Cuts in one window, for the draft and for each outlier. An outlier that had
/// ended before the window started has `None` and does not vote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RhythmWindow {
    pub start_ms: u64,
    pub end_ms: u64,
    pub draft_cuts: usize,
    pub outlier_cuts: Vec<Option<usize>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelativeFlag {
    /// The draft has no cut in these windows, and more than half of the
    /// outliers still running have one.
    RhythmGap {
        window_start_ms: u64,
        window_end_ms: u64,
        outliers_with_cut: usize,
        outliers_counted: usize,
    },
    /// The draft pauses much more often than the outliers do.
    PauseDensity {
        draft_per_minute: f64,
        outlier_per_minute: f64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub draft: ReelMetrics,
    pub outliers: Vec<ReelMetrics>,
    pub windows: Vec<RhythmWindow>,
    pub flags: Vec<RelativeFlag>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub flags: Vec<DiffFlag>,
    pub comparison: Comparison,
}

/// More than half of `counted`, the rule for an outlier majority.
fn more_than_half(with: usize, counted: usize) -> bool {
    counted > 0 && 2 * with > counted
}

/// Shot lengths in milliseconds, from the start to each cut and to the end.
fn shot_lengths(dna: &EditDna) -> Vec<u64> {
    if dna.duration_ms == 0 {
        return Vec::new();
    }
    let mut bounds = vec![0];
    bounds.extend(
        dna.cuts
            .iter()
            .map(|cut| cut.time_ms)
            .filter(|time_ms| *time_ms > 0 && *time_ms < dna.duration_ms),
    );
    bounds.push(dna.duration_ms);
    bounds.sort_unstable();
    bounds.dedup();
    bounds.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

/// Metrics of one reel.
pub fn reel_metrics(reel: NamedReel<'_>) -> ReelMetrics {
    let dna = reel.dna;
    let shots = shot_lengths(dna);
    let mean_shot_ms = match u64::try_from(shots.len()) {
        Ok(count) if count > 0 => shots.iter().sum::<u64>() / count,
        _ => 0,
    };
    let dead_air_count = dna
        .silence_gaps
        .iter()
        .filter(|gap| gap.duration_ms > DEAD_AIR_LIMIT_MS)
        .count();
    let silent_ms: u64 = dna.silence_gaps.iter().map(|gap| gap.duration_ms).sum();
    let (dead_air_per_minute, silent_share) = if dna.duration_ms == 0 {
        (0.0, 0.0)
    } else {
        let duration = dna.duration_ms as f64;
        (
            dead_air_count as f64 * 60_000.0 / duration,
            silent_ms as f64 / duration,
        )
    };
    ReelMetrics {
        name: reel.name.to_string(),
        duration_ms: dna.duration_ms,
        cut_count: dna.cuts.len(),
        mean_shot_ms,
        longest_shot_ms: shots.iter().copied().max().unwrap_or(0),
        dead_air_count,
        dead_air_per_minute,
        silent_share,
        words_per_minute: dna.words_per_minute,
        words_source: dna.asr_backend.as_str().to_string(),
        integrated_lufs: dna.integrated_lufs,
    }
}

fn cuts_between(dna: &EditDna, start_ms: u64, end_ms: u64) -> usize {
    dna.cuts
        .iter()
        .filter(|cut| cut.time_ms >= start_ms && cut.time_ms < end_ms)
        .count()
}

/// Windows over the draft's length. An outlier that ended before a window starts
/// has no count there.
fn rhythm_windows(draft: &EditDna, outliers: &[NamedReel<'_>]) -> Vec<RhythmWindow> {
    let mut windows = Vec::new();
    let mut start_ms = 0;
    while start_ms < draft.duration_ms {
        let end_ms = (start_ms + RHYTHM_WINDOW_MS).min(draft.duration_ms);
        windows.push(RhythmWindow {
            start_ms,
            end_ms,
            draft_cuts: cuts_between(draft, start_ms, end_ms),
            outlier_cuts: outliers
                .iter()
                .map(|outlier| {
                    (outlier.dna.duration_ms > start_ms)
                        .then(|| cuts_between(outlier.dna, start_ms, end_ms))
                })
                .collect(),
        });
        start_ms = end_ms;
    }
    windows
}

/// Windows where the draft has no cut and most of the outliers still running do,
/// with adjacent ones joined. A joined span carries the counts of its weakest
/// window, the one with the fewest outliers cutting.
fn rhythm_gaps(windows: &[RhythmWindow]) -> Vec<RelativeFlag> {
    let mut spans: Vec<(u64, u64, usize, usize)> = Vec::new();
    let mut open = false;
    for window in windows {
        let counted = window.outlier_cuts.iter().flatten().count();
        let with_cut = window
            .outlier_cuts
            .iter()
            .flatten()
            .filter(|n| **n > 0)
            .count();
        if window.draft_cuts > 0 || !more_than_half(with_cut, counted) {
            open = false;
            continue;
        }
        match spans.last_mut() {
            Some(span) if open => {
                span.1 = window.end_ms;
                if with_cut < span.2 {
                    span.2 = with_cut;
                    span.3 = counted;
                }
            }
            _ => spans.push((window.start_ms, window.end_ms, with_cut, counted)),
        }
        open = true;
    }
    spans
        .into_iter()
        .map(
            |(window_start_ms, window_end_ms, outliers_with_cut, outliers_counted)| {
                RelativeFlag::RhythmGap {
                    window_start_ms,
                    window_end_ms,
                    outliers_with_cut,
                    outliers_counted,
                }
            },
        )
        .collect()
}

/// The middle value of `values`, or the mean of the two middle ones.
fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 1 => Some(values[middle]),
        _ => Some((values[middle - 1] + values[middle]) / 2.0),
    }
}

fn pause_density(draft: &ReelMetrics, outliers: &[ReelMetrics]) -> Option<RelativeFlag> {
    let outlier_per_minute = median(outliers.iter().map(|m| m.dead_air_per_minute).collect())?;
    let draft_per_minute = draft.dead_air_per_minute;
    (draft_per_minute > outlier_per_minute * PAUSE_DENSITY_RATIO
        && draft_per_minute - outlier_per_minute >= PAUSE_DENSITY_MIN_EXTRA_PER_MINUTE)
        .then_some(RelativeFlag::PauseDensity {
            draft_per_minute,
            outlier_per_minute,
        })
}

/// The payoff flag: the draft has no cue in the window, and more than half of
/// the outliers do. With one outlier this is [`compare`]'s rule.
fn push_missing_payoff(flags: &mut Vec<DiffFlag>, draft: &EditDna, outliers: &[NamedReel<'_>]) {
    let with_cue = outliers
        .iter()
        .filter(|outlier| outlier.dna.payoff_window.has_cue)
        .count();
    if !draft.payoff_window.has_cue && more_than_half(with_cue, outliers.len()) {
        flags.push(DiffFlag::MissingPayoffCue {
            window_start_ms: PAYOFF_WINDOW_START_MS,
            window_end_ms: PAYOFF_WINDOW_END_MS,
        });
    }
}

/// Compare a draft with one to three outliers.
pub fn compare_many(draft: NamedReel<'_>, outliers: &[NamedReel<'_>]) -> Report {
    let mut flags = Vec::new();
    push_dead_air(&mut flags, draft.dna);
    push_static_windows(&mut flags, draft.dna);
    push_missing_payoff(&mut flags, draft.dna, outliers);

    let draft_metrics = reel_metrics(draft);
    let outlier_metrics: Vec<ReelMetrics> = outliers.iter().copied().map(reel_metrics).collect();
    let windows = rhythm_windows(draft.dna, outliers);
    let mut relative = rhythm_gaps(&windows);
    relative.extend(pause_density(&draft_metrics, &outlier_metrics));
    Report {
        flags,
        comparison: Comparison {
            draft: draft_metrics,
            outliers: outlier_metrics,
            windows,
            flags: relative,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare;
    use edit_dna::{AsrBackend, Cut, SilenceGap};

    fn reel(duration_ms: u64, cuts: &[u64]) -> EditDna {
        let mut dna = EditDna::empty(duration_ms, 30.0, 640, 360);
        for &time_ms in cuts {
            dna.cuts.push(Cut {
                frame_index: time_ms * 30 / 1_000,
                time_ms,
                interval_ms: time_ms,
                scene_score: 1.0,
            });
        }
        dna
    }

    fn with_gaps(mut dna: EditDna, durations_ms: &[u64]) -> EditDna {
        let mut start_ms = 1_000;
        for &duration_ms in durations_ms {
            dna.silence_gaps.push(SilenceGap {
                start_ms,
                end_ms: start_ms + duration_ms,
                duration_ms,
            });
            start_ms += duration_ms + 1_000;
        }
        dna
    }

    fn with_cue(mut dna: EditDna) -> EditDna {
        dna.payoff_window.has_cue = true;
        dna.payoff_window.cue_ms = Some(9_000);
        dna
    }

    fn run(draft: &EditDna, outliers: &[&EditDna]) -> Report {
        let names = ["a", "b", "c"];
        let named: Vec<NamedReel<'_>> = outliers
            .iter()
            .zip(names)
            .map(|(dna, name)| NamedReel { name, dna })
            .collect();
        compare_many(
            NamedReel {
                name: "draft",
                dna: draft,
            },
            &named,
        )
    }

    fn metrics(dna: &EditDna) -> ReelMetrics {
        reel_metrics(NamedReel { name: "reel", dna })
    }

    #[test]
    fn a_reel_with_no_cut_is_one_shot() {
        let m = metrics(&reel(15_000, &[]));
        assert_eq!(m.cut_count, 0);
        assert_eq!(m.mean_shot_ms, 15_000);
        assert_eq!(m.longest_shot_ms, 15_000);
        assert_eq!((m.name.as_str(), m.duration_ms), ("reel", 15_000));
    }

    #[test]
    fn cuts_split_a_reel_into_shots() {
        let m = metrics(&reel(12_000, &[3_000, 9_000]));
        assert_eq!(m.cut_count, 2);
        assert_eq!(m.mean_shot_ms, 4_000);
        assert_eq!(m.longest_shot_ms, 6_000);
    }

    #[test]
    fn pauses_are_counted_per_minute_over_the_dead_air_limit() {
        // 500 and 300 ms are dead air; 100 ms is not, but it is still silence.
        let m = metrics(&with_gaps(reel(60_000, &[]), &[500, 100, 300]));
        assert_eq!(m.dead_air_count, 2);
        assert_eq!(m.dead_air_per_minute, 2.0);
        assert_eq!(m.silent_share, 0.015);
    }

    #[test]
    fn the_words_source_says_how_the_rate_was_found() {
        let mut dna = reel(10_000, &[]);
        dna.words_per_minute = 144.9;
        dna.integrated_lufs = -22.4;
        let bursts = metrics(&dna);
        assert_eq!(bursts.words_source, "silence_bursts");
        assert_eq!(
            (bursts.words_per_minute, bursts.integrated_lufs),
            (144.9, -22.4)
        );
        dna.asr_backend = AsrBackend::WhisperCpp;
        assert_eq!(metrics(&dna).words_source, "whisper_cpp");
    }

    #[test]
    fn an_empty_reel_has_finite_zero_metrics() {
        let m = metrics(&reel(0, &[]));
        assert_eq!((m.cut_count, m.mean_shot_ms, m.longest_shot_ms), (0, 0, 0));
        assert_eq!(
            (m.dead_air_per_minute, m.silent_share),
            (0.0, 0.0),
            "no division by zero"
        );
    }

    #[test]
    fn windows_count_cuts_for_the_draft_and_each_outlier() {
        let draft = reel(12_000, &[6_000]);
        let long = reel(12_000, &[3_000, 9_000]);
        let short = reel(8_000, &[]);
        let report = run(&draft, &[&long, &short]);
        let windows = &report.comparison.windows;
        let spans: Vec<(u64, u64)> = windows.iter().map(|w| (w.start_ms, w.end_ms)).collect();
        assert_eq!(spans, [(0, 5_000), (5_000, 10_000), (10_000, 12_000)]);
        let draft_counts: Vec<usize> = windows.iter().map(|w| w.draft_cuts).collect();
        assert_eq!(draft_counts, [0, 1, 0]);
        let long_counts: Vec<Option<usize>> = windows.iter().map(|w| w.outlier_cuts[0]).collect();
        assert_eq!(long_counts, [Some(1), Some(1), Some(0)]);
        // The short outlier ended at 8 s, so it has no count for 10-12 s.
        let short_counts: Vec<Option<usize>> = windows.iter().map(|w| w.outlier_cuts[1]).collect();
        assert_eq!(short_counts, [Some(0), Some(0), None]);
    }

    fn rhythm_gaps(report: &Report) -> Vec<(u64, u64, usize, usize)> {
        report
            .comparison
            .flags
            .iter()
            .filter_map(|flag| match flag {
                RelativeFlag::RhythmGap {
                    window_start_ms,
                    window_end_ms,
                    outliers_with_cut,
                    outliers_counted,
                } => Some((
                    *window_start_ms,
                    *window_end_ms,
                    *outliers_with_cut,
                    *outliers_counted,
                )),
                RelativeFlag::PauseDensity { .. } => None,
            })
            .collect()
    }

    #[test]
    fn one_outlier_that_cuts_where_the_draft_does_not_is_a_rhythm_gap() {
        // The real pair: a draft with no cut, an outlier with one at 8,466 ms.
        let draft = reel(15_092, &[]);
        let outlier = reel(38_498, &[8_466]);
        let report = run(&draft, &[&outlier]);
        assert_eq!(rhythm_gaps(&report), [(5_000, 10_000, 1, 1)]);
    }

    #[test]
    fn a_rhythm_gap_needs_more_than_half_of_the_outliers() {
        let draft = reel(10_000, &[]);
        let cuts = reel(10_000, &[1_000]);
        let still = reel(10_000, &[]);
        // 2 of 3 cut in 0-5 s: a gap. 1 of 2: not a gap.
        let two_of_three = run(&draft, &[&cuts, &cuts, &still]);
        assert_eq!(rhythm_gaps(&two_of_three), [(0, 5_000, 2, 3)]);
        let one_of_two = run(&draft, &[&cuts, &still]);
        assert!(rhythm_gaps(&one_of_two).is_empty());
    }

    #[test]
    fn adjacent_windows_without_a_cut_are_one_gap() {
        let draft = reel(15_000, &[]);
        let outlier = reel(15_000, &[1_000, 7_000]);
        let report = run(&draft, &[&outlier]);
        assert_eq!(rhythm_gaps(&report), [(0, 10_000, 1, 1)]);
    }

    #[test]
    fn a_merged_gap_reports_its_weakest_window() {
        // 3 of 3 cut in 0-5 s, and 2 of 3 in 5-10 s: one span, with the 2 of 3.
        let draft = reel(10_000, &[]);
        let both = reel(10_000, &[1_000, 7_000]);
        let first_only = reel(10_000, &[1_000]);
        let report = run(&draft, &[&both, &both, &first_only]);
        assert_eq!(rhythm_gaps(&report), [(0, 10_000, 2, 3)]);
    }

    #[test]
    fn a_draft_that_cuts_as_often_has_no_gap() {
        let draft = reel(10_000, &[2_000, 7_000]);
        let outlier = reel(10_000, &[1_000, 8_000]);
        assert!(rhythm_gaps(&run(&draft, &[&outlier])).is_empty());
    }

    #[test]
    fn windows_after_every_outlier_has_ended_raise_nothing() {
        let draft = reel(20_000, &[]);
        let outlier = reel(10_000, &[1_000, 6_000]);
        let report = run(&draft, &[&outlier]);
        assert_eq!(rhythm_gaps(&report), [(0, 10_000, 1, 1)]);
    }

    fn pause_density(report: &Report) -> Vec<(f64, f64)> {
        report
            .comparison
            .flags
            .iter()
            .filter_map(|flag| match flag {
                RelativeFlag::PauseDensity {
                    draft_per_minute,
                    outlier_per_minute,
                } => Some((*draft_per_minute, *outlier_per_minute)),
                RelativeFlag::RhythmGap { .. } => None,
            })
            .collect()
    }

    fn paused(per_minute: usize) -> EditDna {
        with_gaps(reel(60_000, &[]), &vec![300; per_minute])
    }

    #[test]
    fn pause_density_needs_both_the_ratio_and_the_extra_pauses() {
        // 10 against 5: 2x and 5 extra. 6 against 5: only 1.2x. 3 against 1: 2 extra.
        assert_eq!(
            pause_density(&run(&paused(10), &[&paused(5)])),
            [(10.0, 5.0)]
        );
        assert!(pause_density(&run(&paused(6), &[&paused(5)])).is_empty());
        assert!(pause_density(&run(&paused(3), &[&paused(1)])).is_empty());
    }

    #[test]
    fn the_median_of_two_outliers_is_their_mean() {
        // Outliers at 2 and 6 per minute: median 4. 7 is 1.75x and 3 more; 6 is exactly 1.5x.
        assert_eq!(
            pause_density(&run(&paused(7), &[&paused(2), &paused(6)])),
            [(7.0, 4.0)]
        );
        assert!(pause_density(&run(&paused(6), &[&paused(2), &paused(6)])).is_empty());
    }

    #[test]
    fn the_median_of_three_outliers_is_the_middle_one() {
        assert_eq!(
            pause_density(&run(&paused(10), &[&paused(1), &paused(5), &paused(30)])),
            [(10.0, 5.0)]
        );
    }

    #[test]
    fn the_payoff_follows_more_than_half_of_the_outliers() {
        let draft = reel(12_000, &[]);
        let cue = with_cue(reel(12_000, &[9_000]));
        let none = reel(12_000, &[]);
        let has = |report: &Report| {
            report
                .flags
                .iter()
                .any(|f| f.kind_name() == "missing_payoff_cue")
        };
        // One outlier: its own cue decides, as `compare` does.
        assert!(has(&run(&draft, &[&cue])));
        assert!(!has(&run(&draft, &[&none])));
        // Two outliers: both must have one.
        assert!(!has(&run(&draft, &[&cue, &none])));
        assert!(has(&run(&draft, &[&cue, &cue])));
        // Three outliers: two are enough, one is not.
        assert!(has(&run(&draft, &[&cue, &cue, &none])));
        assert!(!has(&run(&draft, &[&cue, &none, &none])));
        // A draft with its own cue is never flagged.
        let ok = with_cue(reel(12_000, &[9_000]));
        assert!(!has(&run(&ok, &[&cue, &cue, &cue])));
    }

    #[test]
    fn with_one_outlier_the_flags_are_exactly_the_older_ones() {
        let draft = with_gaps(reel(12_000, &[6_000]), &[500, 100]);
        let outlier = with_cue(reel(12_000, &[3_000, 9_000]));
        let report = run(&draft, &[&outlier]);
        assert_eq!(report.flags, compare(&draft, &outlier).flags);
        assert!(!report.flags.is_empty());
    }

    #[test]
    fn the_report_serializes_with_flags_first_and_a_comparison() {
        let draft = reel(15_092, &[]);
        let outlier = reel(38_498, &[8_466]);
        let report = run(&draft, &[&outlier]);
        // A `Value` sorts its keys, so read the order from the text.
        let text = serde_json::to_string(&report).expect("json");
        let flags_at = text.find("\"flags\"").expect("flags");
        let comparison_at = text.find("\"comparison\"").expect("comparison");
        assert!(flags_at < comparison_at, "{text}");
        let value = serde_json::to_value(&report).expect("json");
        assert_eq!(value["comparison"]["flags"][0]["kind"], "rhythm_gap");
        assert_eq!(value["comparison"]["draft"]["name"], "draft");
        assert_eq!(value["comparison"]["outliers"][0]["name"], "a");
    }
}
