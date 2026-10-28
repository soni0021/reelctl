//! Whisper words into numbered segments, split at sentence ends and measured pauses.

use edit_dna::{SilenceGap, Word};

use crate::types::Segment;

/// Split `words` into segments with 1-based IDs.
///
/// A segment ends after a word ending in `.`, `?` or `!`, or where a silence gap of at
/// least [`crate::SEGMENT_PAUSE_MS`] lies between two words (allowing
/// [`crate::SEGMENT_SLACK_MS`] on whisper's word times). `text` joins the words with
/// single spaces and is never changed; `ends_sentence` is set when the last word
/// ends a sentence.
pub fn segment_words(words: &[Word], silence_gaps: &[SilenceGap]) -> Vec<Segment> {
    // Stub: the segmenter is built in its own task.
    let _ = (words, silence_gaps);
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SEGMENT_PAUSE_MS, SEGMENT_SLACK_MS};

    fn word(text: &str, start_ms: u64, end_ms: u64) -> Word {
        Word {
            text: text.to_string(),
            start_ms,
            end_ms,
        }
    }

    fn gap(start_ms: u64, end_ms: u64) -> SilenceGap {
        SilenceGap {
            start_ms,
            end_ms,
            duration_ms: end_ms - start_ms,
        }
    }

    fn texts(segments: &[Segment]) -> Vec<&str> {
        segments.iter().map(|s| s.text.as_str()).collect()
    }

    // Two words whose boundary is a silence gap of `pause_ms`, with whisper's word times
    // exactly at the gap's edges.
    fn two_words_around(pause_ms: u64) -> (Vec<Word>, Vec<SilenceGap>) {
        let words = vec![word("one", 0, 1_000), word("two", 1_000 + pause_ms, 2_500)];
        (words, vec![gap(1_000, 1_000 + pause_ms)])
    }

    #[test]
    fn splits_after_each_sentence_ending_mark() {
        let words = vec![
            word("Why", 0, 300),
            word("now?", 300, 600),
            word("Go!", 600, 900),
            word("Done.", 900, 1_200),
            word("Then", 1_200, 1_500),
        ];
        let segments = segment_words(&words, &[]);
        assert_eq!(texts(&segments), vec!["Why now?", "Go!", "Done.", "Then"]);
    }

    #[test]
    fn a_comma_or_bare_word_does_not_split() {
        let words = vec![
            word("so,", 0, 300),
            word("then", 300, 600),
            word("this", 600, 900),
        ];
        assert_eq!(segment_words(&words, &[]).len(), 1);
    }

    #[test]
    fn splits_at_a_pause_of_exactly_the_minimum() {
        assert_eq!(SEGMENT_PAUSE_MS, 400);
        let (words, gaps) = two_words_around(SEGMENT_PAUSE_MS);
        let segments = segment_words(&words, &gaps);
        assert_eq!(texts(&segments), vec!["one", "two"]);
        assert!(!segments[0].ends_sentence);
    }

    #[test]
    fn does_not_split_at_a_pause_one_millisecond_short() {
        let (words, gaps) = two_words_around(SEGMENT_PAUSE_MS - 1);
        let segments = segment_words(&words, &gaps);
        assert_eq!(texts(&segments), vec!["one two"]);
    }

    #[test]
    fn the_gap_may_start_slack_before_the_word_ends() {
        assert_eq!(SEGMENT_SLACK_MS, 150);
        // Whisper ends "one" at 1,150; the measured silence began at 1,000.
        let words = vec![word("one", 0, 1_150), word("two", 1_400, 2_000)];
        let at_slack = [gap(1_150 - SEGMENT_SLACK_MS, 1_400)];
        assert_eq!(segment_words(&words, &at_slack).len(), 2);
        // One millisecond earlier is past the slack.
        let past_slack = [gap(1_150 - SEGMENT_SLACK_MS - 1, 1_400)];
        assert_eq!(segment_words(&words, &past_slack).len(), 1);
    }

    #[test]
    fn a_gap_starting_after_the_word_end_still_counts() {
        let words = vec![word("one", 0, 1_000), word("two", 1_900, 2_500)];
        let segments = segment_words(&words, &[gap(1_300, 1_900)]);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn the_gap_may_end_slack_after_the_next_word_starts() {
        // Whisper's onset for "two" is 1,250 but the audio stays silent until 1,400.
        let words = vec![word("one", 0, 1_000), word("two", 1_250, 2_000)];
        let at_slack = [gap(1_000, 1_250 + SEGMENT_SLACK_MS)];
        assert_eq!(segment_words(&words, &at_slack).len(), 2);
        let past_slack = [gap(1_000, 1_250 + SEGMENT_SLACK_MS + 1)];
        assert_eq!(segment_words(&words, &past_slack).len(), 1);
    }

    #[test]
    fn a_gap_splits_only_at_the_boundary_it_lies_at() {
        // The gap opens where "two" ends, so it is the pause after "two", not before it.
        let words = vec![
            word("one", 0, 1_000),
            word("two", 1_000, 1_600),
            word("three", 2_200, 2_800),
        ];
        let segments = segment_words(&words, &[gap(1_600, 2_200)]);
        assert_eq!(texts(&segments), vec!["one two", "three"]);
    }

    #[test]
    fn a_long_gap_after_a_short_one_inside_the_word_still_splits() {
        // Whisper ends "one" at the first (short) gap; the long one follows before "two".
        let words = vec![word("one", 0, 500), word("two", 1_800, 2_400)];
        let gaps = [gap(500, 600), gap(1_100, 1_800)];
        assert_eq!(segment_words(&words, &gaps).len(), 2);
    }

    #[test]
    fn a_pause_at_the_start_or_end_of_the_words_adds_no_segment() {
        let words = vec![word("one", 1_000, 1_400), word("two", 1_400, 1_800)];
        let gaps = [gap(0, 1_000), gap(1_800, 3_000)];
        let segments = segment_words(&words, &gaps);
        assert_eq!(texts(&segments), vec!["one two"]);
    }

    #[test]
    fn a_pause_and_a_sentence_end_at_one_boundary_split_once() {
        let words = vec![word("end.", 0, 1_000), word("Next", 2_000, 2_500)];
        let segments = segment_words(&words, &[gap(1_000, 2_000)]);
        assert_eq!(texts(&segments), vec!["end.", "Next"]);
        assert!(segments[0].ends_sentence);
    }

    #[test]
    fn keeps_the_first_and_last_words() {
        let words = vec![
            word("first", 100, 400),
            word("middle", 400, 900),
            word("last", 2_000, 2_600),
        ];
        let segments = segment_words(&words, &[gap(900, 2_000)]);
        assert_eq!(
            segments.first().map(|s| s.text.as_str()),
            Some("first middle")
        );
        assert_eq!(segments.last().map(|s| s.text.as_str()), Some("last"));
        assert_eq!(segments[0].start_ms, 100);
        assert_eq!(segments[1].end_ms, 2_600);
    }

    #[test]
    fn a_segment_spans_its_first_word_start_to_its_last_word_end() {
        let words = vec![
            word("a", 100, 200),
            word("b", 200, 350),
            word("c.", 350, 500),
            word("d", 700, 800),
        ];
        let segments = segment_words(&words, &[]);
        assert_eq!((segments[0].start_ms, segments[0].end_ms), (100, 500));
        assert_eq!((segments[1].start_ms, segments[1].end_ms), (700, 800));
    }

    #[test]
    fn no_words_give_no_segments() {
        assert!(segment_words(&[], &[]).is_empty());
        assert!(segment_words(&[], &[gap(0, 5_000)]).is_empty());
    }

    #[test]
    fn no_pause_data_still_splits_at_punctuation() {
        let words = vec![word("Hi.", 0, 400), word("Bye.", 5_000, 5_400)];
        assert_eq!(segment_words(&words, &[]).len(), 2);
    }

    #[test]
    fn ids_are_one_based_and_contiguous() {
        let words: Vec<Word> = (0..5).map(|i| word("w.", i * 500, i * 500 + 500)).collect();
        let ids: Vec<u32> = segment_words(&words, &[]).iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn sentence_starts_follow_ends_sentence() {
        let words = vec![
            word("and", 0, 300),
            word("then", 1_000, 1_300),
            word("it", 1_300, 1_500),
            word("ended.", 1_500, 1_900),
            word("New", 1_900, 2_200),
        ];
        let segments = segment_words(&words, &[gap(300, 1_000)]);
        let ends: Vec<bool> = segments.iter().map(|s| s.ends_sentence).collect();
        assert_eq!(texts(&segments), vec!["and", "then it ended.", "New"]);
        assert_eq!(ends, vec![false, true, false]);
    }

    #[test]
    fn words_with_no_gaps_and_no_punctuation_give_one_segment() {
        let words = vec![word("a", 0, 100), word("b", 100, 200), word("c", 200, 300)];
        let segments = segment_words(&words, &[]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].id, 1);
        assert_eq!(segments[0].text, "a b c");
        assert!(!segments[0].ends_sentence);
    }

    #[test]
    fn text_is_whispers_words_exactly() {
        let words = vec![
            word("Café", 0, 300),
            word("नमस्ते,", 300, 700),
            word("kaise", 700, 900),
            word("हो?", 900, 1_200),
            word("“Ok”", 1_200, 1_500),
            word("Don't", 1_500, 1_700),
        ];
        let segments = segment_words(&words, &[]);
        assert_eq!(texts(&segments), vec!["Café नमस्ते, kaise हो?", "“Ok” Don't"]);
        assert!(segments[0].ends_sentence);
    }

    #[test]
    fn a_closing_quote_after_the_mark_is_not_a_sentence_end() {
        // `ends_sentence` is the spec's literal last-character test.
        let words = vec![word("said.\"", 0, 400), word("then", 400, 800)];
        let segments = segment_words(&words, &[]);
        assert_eq!(segments.len(), 1);
        assert!(!segments[0].ends_sentence);
    }
}
