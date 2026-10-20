//! 9 second payoff window.
//!
//! A cue is a detected cut whose time falls inside 8_000..=10_000 ms.
//! The marker itself is always recorded, including when the reel is shorter
//! than the window.

use edit_dna::{Cut, PayoffWindow, PAYOFF_MARKER_MS, PAYOFF_WINDOW_END_MS, PAYOFF_WINDOW_START_MS};

pub fn from_cuts(cuts: &[Cut]) -> PayoffWindow {
    let cue = cuts
        .iter()
        .find(|cut| cut.time_ms >= PAYOFF_WINDOW_START_MS && cut.time_ms <= PAYOFF_WINDOW_END_MS);
    PayoffWindow {
        marker_ms: PAYOFF_MARKER_MS,
        window_start_ms: PAYOFF_WINDOW_START_MS,
        window_end_ms: PAYOFF_WINDOW_END_MS,
        has_cue: cue.is_some(),
        cue_ms: cue.map(|cut| cut.time_ms),
        cue_frame_index: cue.map(|cut| cut.frame_index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cut_at_nine_seconds_is_the_cue() {
        let cuts = [
            Cut {
                frame_index: 90,
                time_ms: 3_000,
                interval_ms: 3_000,
                scene_score: 1.0,
            },
            Cut {
                frame_index: 270,
                time_ms: 9_000,
                interval_ms: 6_000,
                scene_score: 1.0,
            },
        ];
        let window = from_cuts(&cuts);
        assert!(window.has_cue);
        assert_eq!(window.cue_ms, Some(9_000));
        assert_eq!(window.marker_ms, 9_000);
        assert!(!from_cuts(&cuts[..1]).has_cue);
    }
}
