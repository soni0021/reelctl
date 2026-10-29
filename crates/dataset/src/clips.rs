//! Truth and answers for `clip_picks` go through the same clip rules
//! (`understand::build_clips`), so a pick means the same thing on both sides.

use edit_dna::SilenceGap;
use understand::{build_clips, Clip, ClipOutcome, ClipPick, Segment};

use crate::record::{AnswerPick, PickIds};

/// A pick as `build_clips` takes it. The model's prose is empty: the rules never read it.
/// A missing `start_segment` is the hook and a missing `end_segment` is the cutoff,
/// which are what decide the clip.
pub fn clip_pick(pick: &AnswerPick) -> ClipPick {
    ClipPick {
        start_segment: pick.start_segment.unwrap_or(pick.hook_segment),
        end_segment: pick.end_segment.unwrap_or(pick.cutoff_segment),
        hook_segment: pick.hook_segment,
        payoff_segment: pick.payoff_segment,
        cutoff_segment: pick.cutoff_segment,
        title: String::new(),
        open_loop: String::new(),
        visual_suggestion: String::new(),
        why: String::new(),
    }
}

impl From<PickIds> for AnswerPick {
    fn from(ids: PickIds) -> Self {
        Self {
            hook_segment: ids.hook_segment,
            payoff_segment: ids.payoff_segment,
            cutoff_segment: ids.cutoff_segment,
            start_segment: None,
            end_segment: None,
        }
    }
}

/// Apply the clip rules to a list of picks (`picks[i]` has rank `i + 1`).
pub fn rebuild(
    picks: &[AnswerPick],
    segments: &[Segment],
    pauses: &[SilenceGap],
    duration_ms: u64,
) -> ClipOutcome {
    let picks: Vec<ClipPick> = picks.iter().map(clip_pick).collect();
    build_clips(&picks, segments, pauses, duration_ms)
}

/// The truth picks that rebuild into a clip that breaks no rule, with the position of
/// each in the truth list, and how many did not.
#[derive(Debug, Clone, PartialEq)]
pub struct Reproduced {
    pub kept: Vec<(usize, Clip)>,
    pub unreproducible: usize,
}

/// Rebuild the truth picks. They were rule-compliant when they were collected, so one
/// that is not now (different pauses, a different duration) shows that the measured
/// data does not belong to the run, and it is left out of the agreement metrics.
pub fn reproduce(
    picks: &[PickIds],
    segments: &[Segment],
    pauses: &[SilenceGap],
    duration_ms: u64,
) -> Reproduced {
    let picks: Vec<AnswerPick> = picks.iter().copied().map(AnswerPick::from).collect();
    let outcome = rebuild(&picks, segments, pauses, duration_ms);
    let kept: Vec<(usize, Clip)> = outcome
        .clips
        .into_iter()
        .filter(|clip| clip.flags.is_empty())
        .map(|clip| ((clip.rank as usize).saturating_sub(1), clip))
        .collect();
    Reproduced {
        unreproducible: picks.len() - kept.len(),
        kept,
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use edit_dna::{AsrBackend, EditDna, SilenceGap};
    use understand::{
        build_clips, ClipPick, ModelRun, RunSettings, Segment, Source, Summary, Understanding,
    };

    /// A synthetic lecture: `count` sentences of 3.2 s starting at 1 s every 3.5 s, each
    /// its own segment ("Sentence N is about topic."), with a 300 ms pause between them.
    pub fn lecture(count: u32) -> (Vec<Segment>, Vec<SilenceGap>, u64) {
        let mut segments = Vec::new();
        let mut gaps = Vec::new();
        for n in 0..count {
            let start = 1_000 + 3_500 * u64::from(n);
            segments.push(Segment {
                id: n + 1,
                start_ms: start,
                end_ms: start + 3_200,
                text: format!("Sentence {n} is about topic."),
                ends_sentence: true,
            });
            if n + 1 < count {
                gaps.push(SilenceGap {
                    start_ms: start + 3_200,
                    end_ms: start + 3_500,
                    duration_ms: 300,
                });
            }
        }
        let duration = 1_000 + 3_500 * u64::from(count);
        (segments, gaps, duration)
    }

    /// The `edit_dna.json` of [`lecture`]: its duration and pauses, whisper words.
    pub fn lecture_dna(count: u32) -> EditDna {
        let (_, gaps, duration) = lecture(count);
        let mut dna = EditDna::empty(duration, 30.0, 540, 960);
        dna.silence_gaps = gaps;
        dna.asr_backend = AsrBackend::WhisperCpp;
        dna
    }

    /// A pick with prose that must never reach a dataset.
    pub fn pick(hook: u32, payoff: u32, cutoff: u32) -> ClipPick {
        ClipPick {
            start_segment: hook,
            end_segment: cutoff,
            hook_segment: hook,
            payoff_segment: payoff,
            cutoff_segment: cutoff,
            title: format!("SECRET-TITLE-{hook}"),
            open_loop: "SECRET-OPEN-LOOP".to_string(),
            visual_suggestion: "SECRET-VISUAL".to_string(),
            why: "SECRET-WHY".to_string(),
        }
    }

    /// An `understanding.json` of [`lecture`] whose clips are what the rules make of
    /// `picks`, as `reelctl understand` would have written it.
    pub fn understanding(source_path: &str, count: u32, picks: &[ClipPick]) -> Understanding {
        let (segments, gaps, duration) = lecture(count);
        let outcome = build_clips(picks, &segments, &gaps, duration);
        Understanding {
            schema_version: understand::SCHEMA_VERSION,
            source: Source {
                path: source_path.to_string(),
                duration_ms: duration,
            },
            model: ModelRun {
                id: "teacher/model-x".to_string(),
                provider: "ProviderY".to_string(),
                quantization: None,
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: None,
                cost_upper_bound_usd: 0.0,
                elapsed_ms: 1,
            },
            settings: RunSettings {
                every_s: 5,
                sheet: "single".to_string(),
                segment_pause_ms: 400,
                quant: Vec::new(),
            },
            segments,
            summary: Summary {
                topic: "SECRET-TOPIC".to_string(),
                language: "en".to_string(),
                speaker_count: 1,
                visual_formats: Vec::new(),
            },
            beats: Vec::new(),
            visual_events: Vec::new(),
            cuts: Vec::new(),
            clips: outcome.clips,
            rejected: outcome.rejected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::lecture;
    use super::*;

    fn ids(hook: u32, payoff: u32, cutoff: u32) -> PickIds {
        PickIds {
            hook_segment: hook,
            payoff_segment: payoff,
            cutoff_segment: cutoff,
        }
    }

    #[test]
    fn a_pick_without_start_and_end_uses_the_hook_and_the_cutoff() {
        let pick = clip_pick(&AnswerPick::from(ids(3, 5, 12)));
        assert_eq!((pick.start_segment, pick.end_segment), (3, 12));
        assert_eq!(
            (pick.hook_segment, pick.payoff_segment, pick.cutoff_segment),
            (3, 5, 12)
        );
    }

    #[test]
    fn a_given_start_and_end_are_kept() {
        let pick = clip_pick(&AnswerPick {
            hook_segment: 3,
            payoff_segment: 5,
            cutoff_segment: 12,
            start_segment: Some(1),
            end_segment: Some(14),
        });
        assert_eq!((pick.start_segment, pick.end_segment), (1, 14));
    }

    #[test]
    fn a_compliant_pick_is_reproduced_with_its_position() {
        // 3.5 s per sentence: segments 1..=12 are 42 s of speech, 11 pauses of 300 ms
        // are cut, so the tight length is 42 - 3.3 s... and the payoff (segment 3) is
        // about 7 s in.
        let (segments, gaps, duration) = lecture(40);
        let truth = [ids(1, 3, 13), ids(20, 22, 32)];
        let found = reproduce(&truth, &segments, &gaps, duration);
        assert_eq!(found.unreproducible, 0, "{:?}", found.kept);
        assert_eq!(found.kept.len(), 2);
        assert_eq!(found.kept[0].0, 0);
        assert_eq!(found.kept[1].0, 1);
        assert!(found.kept.iter().all(|(_, clip)| clip.flags.is_empty()));
    }

    #[test]
    fn a_pick_that_breaks_a_rule_is_counted_not_kept() {
        let (segments, gaps, duration) = lecture(40);
        // Segments 1..=2 are 7 s: under the 30 s minimum. 99 does not exist.
        let truth = [ids(1, 2, 2), ids(1, 99, 13), ids(20, 22, 32)];
        let found = reproduce(&truth, &segments, &gaps, duration);
        assert_eq!(found.unreproducible, 2);
        assert_eq!(found.kept.len(), 1);
        assert_eq!(found.kept[0].0, 2, "the third pick keeps its position");
    }

    #[test]
    fn no_picks_reproduce_to_nothing() {
        let (segments, gaps, duration) = lecture(5);
        let found = reproduce(&[], &segments, &gaps, duration);
        assert_eq!(found.unreproducible, 0);
        assert!(found.kept.is_empty());
    }
}
