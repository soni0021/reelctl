//! Word timing.
//!
//! With a model file, whisper.cpp transcribes the audio (see `whisper.rs`).
//! Without one, each contiguous voiced burst separated by at least 40 ms of
//! silence is one word, named `burst-N`. Silence here is the same per-recording
//! threshold that `silence.rs` uses for gaps, read from the same window levels. Either way, words per minute is
//! `word_count / (duration_ms / 60_000)` over the whole reel, and `asr_ms`
//! times this stage.

use std::path::Path;

use edit_dna::{AsrBackend, Word};

use crate::error::DecompileError;
use crate::silence::SilenceAnalysis;
use crate::whisper;

const WINDOW_MS: u64 = 10;
const MIN_BURST_MS: u64 = 40;
const MIN_SPLIT_MS: u64 = 40;

pub const NO_MODEL: &str = "\
No whisper model was given (--whisper-model or REELCTL_WHISPER_MODEL), so whisper.cpp did \
not run. asr_ms times the silence-derived estimator instead: each contiguous voiced burst \
separated by at least 40ms below silence_threshold_db is one word, and words_per_minute is \
word_count / (duration_ms / 60000) over the whole reel.";

pub struct AsrEstimate {
    pub words: Vec<Word>,
    pub words_per_minute: f64,
    pub backend: AsrBackend,
    pub note: String,
}

pub fn transcribe(
    pcm: &[f32],
    sample_rate: u32,
    duration_ms: u64,
    silence: &SilenceAnalysis,
    model: Option<&Path>,
) -> Result<AsrEstimate, DecompileError> {
    let Some(path) = model else {
        let end_ms = (pcm.len() as u128 * 1_000 / u128::from(sample_rate.max(1))) as u64;
        let words = bursts(&silence.levels, silence.threshold_db, end_ms);
        return Ok(AsrEstimate {
            words_per_minute: per_minute(words.len(), duration_ms),
            words,
            backend: AsrBackend::SilenceBursts,
            note: NO_MODEL.to_string(),
        });
    };
    let transcript = whisper::transcribe(path, pcm, sample_rate, &silence.gaps)?;
    let file_name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let note = format!(
        "whisper.cpp {} transcribed the audio with {file_name} (DTW preset {}, language {}). \
Each word starts at its DTW timestamp and ends at the next word's start or the next silence \
gap, whichever is earlier. words_per_minute is word_count / (duration_ms / 60000) over the \
whole reel.",
        whisper_rs::get_whisper_version(),
        transcript.preset,
        transcript.language,
    );
    Ok(AsrEstimate {
        words_per_minute: per_minute(transcript.words.len(), duration_ms),
        words: transcript.words,
        backend: AsrBackend::WhisperCpp,
        note,
    })
}

fn per_minute(count: usize, duration_ms: u64) -> f64 {
    if duration_ms == 0 {
        return 0.0;
    }
    let minutes = duration_ms as f64 / 60_000.0;
    round2(count as f64 / minutes)
}

fn bursts(levels: &[f64], threshold_db: f64, end_ms: u64) -> Vec<Word> {
    let mut words = Vec::new();
    let mut burst_start: Option<u64> = None;
    let mut silent_run_ms = 0u64;
    for (window_index, level) in levels.iter().enumerate() {
        let start_ms = window_index as u64 * WINDOW_MS;
        if *level < threshold_db {
            silent_run_ms = silent_run_ms.saturating_add(WINDOW_MS);
            if silent_run_ms >= MIN_SPLIT_MS {
                if let Some(opened) = burst_start.take() {
                    let end_ms = start_ms + WINDOW_MS - silent_run_ms;
                    push_word(&mut words, opened, end_ms.max(opened));
                }
            }
        } else {
            if burst_start.is_none() {
                burst_start = Some(start_ms);
            }
            silent_run_ms = 0;
        }
    }
    if let Some(opened) = burst_start {
        push_word(&mut words, opened, end_ms);
    }
    words
}

fn push_word(words: &mut Vec<Word>, start_ms: u64, end_ms: u64) {
    if end_ms.saturating_sub(start_ms) < MIN_BURST_MS {
        return;
    }
    let index = words.len();
    words.push(Word {
        text: format!("burst-{index}"),
        start_ms,
        end_ms,
    });
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silence_gap_splits_one_tone_into_two_words() {
        let rate = 48_000u32;
        let mut pcm = Vec::new();
        for index in 0..(rate / 5) {
            pcm.push((index as f32 * 0.3).sin() * 0.2);
        }
        pcm.extend(std::iter::repeat(0.0).take(rate as usize / 10));
        for index in 0..(rate / 5) {
            pcm.push((index as f32 * 0.3).sin() * 0.2);
        }
        let silence = crate::silence::analyze(&pcm, rate);
        let estimate = transcribe(&pcm, rate, 500, &silence, None).expect("no model");
        assert_eq!(estimate.backend, AsrBackend::SilenceBursts);
        assert_eq!(estimate.words.len(), 2, "words={:?}", estimate.words);
        assert!(estimate.words_per_minute > 0.0);
        assert!(estimate.note.starts_with("No whisper model was given"));
    }
}
