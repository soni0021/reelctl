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
