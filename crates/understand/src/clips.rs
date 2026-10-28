//! The clip rules: open on the hook's sentence, end on the cliffhanger, check the
//! length once pauses are cut, and place cut points inside measured pauses.
//!
//! Priority: hook first, then keep the cliffhanger, then length. A rule never breaks
//! a higher-priority one; when it would have to, it flags instead.

use edit_dna::SilenceGap;

use crate::types::{Clip, ClipPick, Rejected, Segment};

/// The picks that became clips (in the model's order) and the ones that did not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClipOutcome {
    pub clips: Vec<Clip>,
    pub rejected: Vec<Rejected>,
}

/// Apply the clip rules to the model's picks. `picks[i]` has rank `i + 1`.
///
/// `pauses` are the measured silence gaps of the video; `duration_ms` bounds the cut
/// points. See the spec's "Clip rules" for the six rules.
pub fn build_clips(
    picks: &[ClipPick],
    segments: &[Segment],
    pauses: &[SilenceGap],
    duration_ms: u64,
) -> ClipOutcome {
    // Stub: the rules are built in their own task.
    let _ = (picks, segments, pauses, duration_ms);
    ClipOutcome::default()
}
