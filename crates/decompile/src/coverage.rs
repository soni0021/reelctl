//! Checking whisper's output against the audio.
//!
//! whisper.cpp decodes 30 s windows in turn and can skip part or all of a
//! window without saying so: a window judged to hold no speech is dropped,
//! and a window whose output ends on a single timestamp skips its remainder.
//! On real footage that lost up to 30 s of speech (docs/specs/whisper-coverage.md).
//!
//! A hole is a stretch between consecutive word starts, or between an end of
//! the audio and the nearest word, that is longer than 3 s and has at least
//! 1.5 s outside the silence gaps. On the seven transcripts measured for the
//! spec, that flagged every known loss and nothing in the runs without one.
//! Each hole is transcribed again on its own; its words replace the first
//! pass's words inside the hole.

use edit_dna::SilenceGap;

const MIN_HOLE_MS: u64 = 3_000;
const MIN_SOUND_MS: u64 = 1_500;

/// A word from one whisper pass, before its end is set from its neighbours.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WordStart {
    pub text: String,
    pub start_ms: u64,
    /// End of the whisper segment that holds the word.
    pub segment_end_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hole {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl Hole {
    fn contains(&self, time_ms: u64) -> bool {
        time_ms >= self.start_ms && time_ms < self.end_ms
    }
}

/// Holes in `words`, which must be sorted by start.
pub(crate) fn find_holes(words: &[WordStart], gaps: &[SilenceGap], duration_ms: u64) -> Vec<Hole> {
    let mut bounds = Vec::with_capacity(words.len() + 2);
    bounds.push(0);
    bounds.extend(words.iter().map(|word| word.start_ms));
    bounds.push(duration_ms);
    bounds
        .windows(2)
        .filter_map(|pair| {
            let (start_ms, end_ms) = (pair[0], pair[1]);
            let length = end_ms.saturating_sub(start_ms);
            let sound = length.saturating_sub(silent_ms(gaps, start_ms, end_ms));
            (length > MIN_HOLE_MS && sound >= MIN_SOUND_MS).then_some(Hole { start_ms, end_ms })
        })
        .collect()
}

fn silent_ms(gaps: &[SilenceGap], start_ms: u64, end_ms: u64) -> u64 {
    gaps.iter()
        .map(|gap| {
            gap.end_ms
                .min(end_ms)
                .saturating_sub(gap.start_ms.max(start_ms))
        })
        .sum()
}

/// Merges re-run words into the first pass. Re-run times are relative to the
/// start of their hole. A re-run's words that start inside its hole replace
/// the first pass's words there; a re-run with none leaves the hole as it was.
pub(crate) fn merge(first: Vec<WordStart>, reruns: Vec<(Hole, Vec<WordStart>)>) -> Vec<WordStart> {
    let mut words = first;
    for (hole, rerun) in reruns {
        let found = shift_into(hole, rerun);
        if found.is_empty() {
            continue;
        }
        words.retain(|word| !hole.contains(word.start_ms));
        words.extend(found);
    }
    words.sort_by_key(|word| word.start_ms);
    words
}

/// Shifts a re-run's words, timed from the start of `hole`, onto the
/// recording, and keeps those that start inside the hole.
pub(crate) fn shift_into(hole: Hole, rerun: Vec<WordStart>) -> Vec<WordStart> {
    rerun
        .into_iter()
        .map(|word| WordStart {
            start_ms: word.start_ms + hole.start_ms,
            segment_end_ms: word.segment_end_ms + hole.start_ms,
            text: word.text,
        })
        .filter(|word| hole.contains(word.start_ms))
        .collect()
}

/// Drops words that start at or after `end_ms`, the end of the real audio.
/// A pass is padded with silence, and whisper can write words there.
pub(crate) fn drop_after(mut words: Vec<WordStart>, end_ms: u64) -> Vec<WordStart> {
    words.retain(|word| word.start_ms < end_ms);
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_dna::SilenceGap;

    fn word(text: &str, start_ms: u64) -> WordStart {
        WordStart {
            text: text.to_string(),
            start_ms,
            segment_end_ms: start_ms + 200,
        }
    }

    fn starts(words: &[WordStart]) -> Vec<(String, u64)> {
        words
            .iter()
            .map(|word| (word.text.clone(), word.start_ms))
            .collect()
    }

    fn gap(start_ms: u64, end_ms: u64) -> SilenceGap {
        SilenceGap {
            start_ms,
            end_ms,
            duration_ms: end_ms - start_ms,
        }
    }

    #[test]
    fn a_long_stretch_of_sound_without_words_is_a_hole() {
        let words = [word("a", 1_000), word("b", 1_400), word("c", 30_000)];
        assert_eq!(
            find_holes(&words, &[], 31_000),
            vec![Hole {
                start_ms: 1_400,
                end_ms: 30_000
            }]
        );
    }

    #[test]
    fn a_stretch_that_is_mostly_silence_is_not_a_hole() {
        // 4 s between word starts, 3 s of it a silence gap: 1 s of sound.
        let words = [word("a", 1_000), word("b", 5_000)];
        assert!(find_holes(&words, &[gap(1_500, 4_500)], 5_500).is_empty());
    }

    #[test]
    fn stretches_before_the_first_word_and_after_the_last_count() {
        let holes = find_holes(&[word("a", 25_000)], &[], 60_000);
        assert_eq!(
            holes,
            vec![
                Hole {
                    start_ms: 0,
                    end_ms: 25_000
                },
                Hole {
                    start_ms: 25_000,
                    end_ms: 60_000
                }
            ]
        );
    }

    #[test]
    fn audio_without_words_is_one_hole_unless_it_is_silent() {
        assert_eq!(
            find_holes(&[], &[], 10_000),
            vec![Hole {
                start_ms: 0,
                end_ms: 10_000
            }]
        );
        assert!(find_holes(&[], &[gap(0, 10_000)], 10_000).is_empty());
    }

    #[test]
    fn rerun_words_replace_the_hole_shifted_and_sorted() {
        let first = vec![word("a", 1_000), word("ko?", 15_960), word("next", 30_860)];
        let hole = Hole {
            start_ms: 15_960,
            end_ms: 30_860,
        };
        // Times relative to the start of the re-run slice.
        let rerun = vec![word("ko?", 0), word("yahaan", 500), word("next", 14_900)];
        let merged = merge(first, vec![(hole, rerun)]);
        assert_eq!(
            starts(&merged),
            vec![
                ("a".to_string(), 1_000),
                ("ko?".to_string(), 15_960),
                ("yahaan".to_string(), 16_460),
                ("next".to_string(), 30_860),
            ]
        );
        assert_eq!(merged[2].segment_end_ms, 16_660);
    }

    #[test]
    fn an_empty_rerun_keeps_the_first_pass() {
        let first = vec![word("a", 1_000), word("b", 20_000)];
        let hole = Hole {
            start_ms: 1_000,
            end_ms: 20_000,
        };
        assert_eq!(
            starts(&merge(first.clone(), vec![(hole, Vec::new())])),
            starts(&first)
        );
    }

    #[test]
    fn words_from_the_padding_are_dropped() {
        let words = vec![word("a", 9_000), word("nan", 10_000), word("b", 10_500)];
        assert_eq!(
            starts(&drop_after(words, 10_000)),
            vec![("a".to_string(), 9_000)]
        );
    }
}
