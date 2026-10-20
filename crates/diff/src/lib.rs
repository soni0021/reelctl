//! reel-diff pacing flags.
//!
//! Dead air, static shots, and a missing 9 second payoff are properties of the
//! draft. The payoff flag is raised only when the outlier DNA has a cue inside
//! the 8–10 second window and the draft does not.

#![warn(clippy::all)]

use edit_dna::{EditDna, PAYOFF_WINDOW_END_MS, PAYOFF_WINDOW_START_MS};
use serde::{Deserialize, Serialize};

/// Silence longer than this is dead air.
pub const DEAD_AIR_LIMIT_MS: u64 = 180;

/// A stretch of picture with no cut longer than this is a static window.
pub const STATIC_WINDOW_LIMIT_MS: u64 = 2_500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffFlag {
    DeadAir {
        start_ms: u64,
        end_ms: u64,
        duration_ms: u64,
    },
    StaticWindow {
        start_ms: u64,
        end_ms: u64,
        duration_ms: u64,
    },
    MissingPayoffCue {
        window_start_ms: u64,
        window_end_ms: u64,
    },
}

impl DiffFlag {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::DeadAir { .. } => "dead_air",
            Self::StaticWindow { .. } => "static_window",
            Self::MissingPayoffCue { .. } => "missing_payoff_cue",
            other => {
                let _never: std::convert::Infallible = match other {
                    Self::DeadAir { .. }
                    | Self::StaticWindow { .. }
                    | Self::MissingPayoffCue { .. } => {
                        unreachable!("DiffFlag is handled above")
                    }
                };
                match _never {}
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffReport {
    pub flags: Vec<DiffFlag>,
}

impl DiffReport {
    pub fn has_kind(&self, kind: &str) -> bool {
        self.flags.iter().any(|flag| flag.kind_name() == kind)
    }
}

/// Compare a decompiled draft against an outlier blueprint.
pub fn compare(draft: &EditDna, outlier: &EditDna) -> DiffReport {
    let mut flags = Vec::new();
    push_dead_air(&mut flags, draft);
    push_static_windows(&mut flags, draft);
    push_missing_payoff(&mut flags, draft, outlier);
    DiffReport { flags }
}

fn push_dead_air(flags: &mut Vec<DiffFlag>, draft: &EditDna) {
    for gap in &draft.silence_gaps {
        if gap.duration_ms > DEAD_AIR_LIMIT_MS {
            flags.push(DiffFlag::DeadAir {
                start_ms: gap.start_ms,
                end_ms: gap.end_ms,
                duration_ms: gap.duration_ms,
            });
        }
    }
}

fn push_static_windows(flags: &mut Vec<DiffFlag>, draft: &EditDna) {
    let mut bounds = Vec::with_capacity(draft.cuts.len() + 2);
    bounds.push(0);
    for cut in &draft.cuts {
        if cut.time_ms > 0 && cut.time_ms < draft.duration_ms {
            bounds.push(cut.time_ms);
        }
    }
    if draft.duration_ms > 0 {
        bounds.push(draft.duration_ms);
    }
    bounds.sort_unstable();
    bounds.dedup();
    for pair in bounds.windows(2) {
        let start_ms = pair[0];
        let end_ms = pair[1];
        let duration_ms = end_ms.saturating_sub(start_ms);
        if duration_ms > STATIC_WINDOW_LIMIT_MS {
            flags.push(DiffFlag::StaticWindow {
                start_ms,
                end_ms,
                duration_ms,
            });
        }
    }
}

fn push_missing_payoff(flags: &mut Vec<DiffFlag>, draft: &EditDna, outlier: &EditDna) {
    if outlier.payoff_window.has_cue && !draft.payoff_window.has_cue {
        flags.push(DiffFlag::MissingPayoffCue {
            window_start_ms: PAYOFF_WINDOW_START_MS,
            window_end_ms: PAYOFF_WINDOW_END_MS,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_dna::{Cut, SilenceGap};

    fn dna(duration_ms: u64) -> EditDna {
        EditDna::empty(duration_ms, 30.0, 640, 360)
    }

    #[test]
    fn flags_planted_silence_static_shot_and_missing_payoff() {
        let mut draft = dna(12_000);
        draft.silence_gaps.push(SilenceGap {
            start_ms: 2_000,
            end_ms: 2_500,
            duration_ms: 500,
        });
        draft.silence_gaps.push(SilenceGap {
            start_ms: 4_000,
            end_ms: 4_100,
            duration_ms: 100,
        });
        draft.cuts.push(Cut {
            frame_index: 180,
            time_ms: 6_000,
            interval_ms: 6_000,
            scene_score: 1.0,
        });

        let mut outlier = dna(12_000);
        outlier.payoff_window.has_cue = true;
        outlier.payoff_window.cue_ms = Some(9_000);
        outlier.cuts.push(Cut {
            frame_index: 270,
            time_ms: 9_000,
            interval_ms: 9_000,
            scene_score: 1.0,
        });

        let report = compare(&draft, &outlier);
        assert!(report.has_kind("dead_air"));
        assert!(report.has_kind("static_window"));
        assert!(report.has_kind("missing_payoff_cue"));
        let dead = report
            .flags
            .iter()
            .find(|flag| flag.kind_name() == "dead_air")
            .expect("dead air");
        match dead {
            DiffFlag::DeadAir { duration_ms, .. } => assert_eq!(*duration_ms, 500),
            other => {
                let _never: std::convert::Infallible = match other {
                    DiffFlag::DeadAir { .. } => unreachable!("kind was dead_air"),
                    DiffFlag::StaticWindow { .. } | DiffFlag::MissingPayoffCue { .. } => {
                        unreachable!("kind was dead_air")
                    }
                };
                match _never {}
            }
        }
        assert!(report.flags.iter().any(|flag| matches!(
            flag,
            DiffFlag::StaticWindow { duration_ms, .. } if *duration_ms > STATIC_WINDOW_LIMIT_MS
        )));
        assert_eq!(
            report
                .flags
                .iter()
                .filter(|flag| flag.kind_name() == "dead_air")
                .count(),
            1,
            "a 100ms gap is under the 180ms dead-air limit"
        );
    }

    #[test]
    fn payoff_present_on_both_sides_is_not_a_flag() {
        let mut draft = dna(12_000);
        draft.payoff_window.has_cue = true;
        let mut outlier = dna(12_000);
        outlier.payoff_window.has_cue = true;
        let report = compare(&draft, &outlier);
        assert!(!report.has_kind("missing_payoff_cue"));
    }
}
