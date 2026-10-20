//! Word timing with whisper.cpp, through `whisper-rs`.
//!
//! Each word comes back as its own segment: token timestamps on, `max_len`
//! 1, and splits only at word boundaries. That is whisper.cpp's documented
//! word-level timestamp mode ("Word-level timestamp (experimental)" in its
//! README). In 1.8.3, `max_len` re-splits segments only when token
//! timestamps are on, so both are set.

use std::path::Path;

use edit_dna::Word;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::error::DecompileError;
use crate::resample::to_whisper_rate;

/// One whisper.cpp segment. Times are in centiseconds, as whisper.cpp reports them.
pub(crate) struct RawSegment {
    pub text: String,
    pub start_cs: i64,
    pub end_cs: i64,
}

pub(crate) fn words(
    model: &Path,
    pcm: &[f32],
    sample_rate: u32,
) -> Result<Vec<Word>, DecompileError> {
    if !model.is_file() {
        return Err(DecompileError::Whisper(format!(
            "model not found at {}",
            model.display()
        )));
    }
    if pcm.is_empty() {
        return Ok(Vec::new());
    }
    let audio = to_whisper_rate(pcm, sample_rate)?;
    // With neither of whisper-rs's log-backend features enabled, this
    // discards whisper.cpp's log output instead of printing it to stderr.
    whisper_rs::install_logging_hooks();
    let context = WhisperContext::new_with_params(model, WhisperContextParameters::default())
        .map_err(|err| failed("load", model, err))?;
    let mut state = context
        .create_state()
        .map_err(|err| failed("create a state for", model, err))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    if context.is_multilingual() {
        params.set_language(None);
    }
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    params.set_suppress_nst(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state
        .full(params, &audio)
        .map_err(|err| failed("transcribe with", model, err))?;
    let mut segments = Vec::new();
    for segment in state.as_iter() {
        let text = segment
            .to_str_lossy()
            .map_err(|err| failed("read a segment from", model, err))?;
        segments.push(RawSegment {
            text: text.into_owned(),
            start_cs: segment.start_timestamp(),
            end_cs: segment.end_timestamp(),
        });
    }
    Ok(words_from_segments(&segments))
}

fn failed(action: &str, model: &Path, err: whisper_rs::WhisperError) -> DecompileError {
    DecompileError::Whisper(format!(
        "could not {action} model {}: {err}",
        model.display()
    ))
}

pub(crate) fn words_from_segments(segments: &[RawSegment]) -> Vec<Word> {
    segments
        .iter()
        .filter_map(|segment| {
            let text = segment.text.trim();
            if !text.chars().any(char::is_alphanumeric) || is_marker(text) {
                return None;
            }
            let start_ms = centiseconds_to_ms(segment.start_cs);
            Some(Word {
                text: text.to_string(),
                start_ms,
                end_ms: centiseconds_to_ms(segment.end_cs).max(start_ms),
            })
        })
        .collect()
}

/// Non-speech annotations such as `[BLANK_AUDIO]` or `(music)`.
fn is_marker(text: &str) -> bool {
    (text.starts_with('[') && text.ends_with(']'))
        || (text.starts_with('(') && text.ends_with(')'))
}

fn centiseconds_to_ms(centiseconds: i64) -> u64 {
    u64::try_from(centiseconds).unwrap_or(0).saturating_mul(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(text: &str, start_cs: i64, end_cs: i64) -> RawSegment {
        RawSegment {
            text: text.to_string(),
            start_cs,
            end_cs,
        }
    }

    #[test]
    fn links_the_documented_whisper_cpp_version() {
        // docs/specs/whisper-asr.md and asr_note name this version.
        assert_eq!(whisper_rs::get_whisper_version(), "1.8.3");
    }

    #[test]
    fn segments_become_trimmed_words_in_milliseconds() {
        let words = words_from_segments(&[segment(" Cut", 0, 32), segment(" the", 32, 45)]);
        assert_eq!(
            words,
            vec![
                Word {
                    text: "Cut".to_string(),
                    start_ms: 0,
                    end_ms: 320,
                },
                Word {
                    text: "the".to_string(),
                    start_ms: 320,
                    end_ms: 450,
                },
            ]
        );
    }

    #[test]
    fn markers_blanks_and_bare_punctuation_are_not_words() {
        let words = words_from_segments(&[
            segment(" [BLANK_AUDIO]", 0, 100),
            segment(" (music)", 100, 200),
            segment(" ", 200, 210),
            segment(".", 210, 220),
            segment(" air.", 220, 260),
        ]);
        assert_eq!(words.len(), 1, "{words:?}");
        assert_eq!(words[0].text, "air.");
    }

    #[test]
    fn negative_or_reversed_times_are_clamped() {
        let words = words_from_segments(&[segment(" so", -3, 20), segment(" then", 50, 40)]);
        assert_eq!((words[0].start_ms, words[0].end_ms), (0, 200));
        assert_eq!((words[1].start_ms, words[1].end_ms), (500, 500));
    }

    #[test]
    fn a_missing_model_is_an_error_naming_the_path() {
        let path = Path::new("/nonexistent/ggml-missing.bin");
        let err = words(path, &[0.0; 4_800], 48_000).expect_err("no model file");
        assert!(err.to_string().contains("/nonexistent/ggml-missing.bin"), "{err}");
    }
}
