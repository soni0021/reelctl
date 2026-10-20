//! Word timing.
//!
//! whisper.cpp was not linked. `whisper-rs` 0.16 builds whisper.cpp through
//! CMake (`whisper-rs-sys`). On this machine `cmake` is not on PATH
//! (`cmake: command not found`), and P0 does not download ggml weights.
//! Without a compiler for the bindings and a model file, linking them would
//! not produce word timestamps in this session.
//!
//! `asr_ms` still times the estimator below. Each contiguous voiced burst
//! separated by at least 40 ms of silence is one word. Words per minute is
//! `word_count / (duration_ms / 60_000)` over the whole reel.

use std::path::Path;

use edit_dna::{AsrBackend, Word};

use crate::silence::window_db;

const THRESHOLD_DB: f64 = -40.0;
const MIN_BURST_MS: u64 = 40;
const MIN_SPLIT_MS: u64 = 40;

pub const WHISPER_NOT_LINKED: &str = "\
whisper.cpp was not linked. whisper-rs 0.16 compiles whisper.cpp through CMake \
(whisper-rs-sys). On this machine cmake is not installed (`cmake: command not found`), \
and P0 does not download ggml weights. Without both, the bindings cannot produce word \
timestamps in this session. asr_ms times this silence-derived estimator: each contiguous \
voiced burst separated by at least 40ms of silence is one word, and words_per_minute is \
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
    model: Option<&Path>,
) -> AsrEstimate {
    let mut note = WHISPER_NOT_LINKED.to_string();
    if let Some(path) = model {
        note.push_str(&format!(
            " A model path was provided ({}) but whisper.cpp is not linked, so it was not used.",
            path.display()
        ));
    }
    let words = bursts(pcm, sample_rate);
    let words_per_minute = if duration_ms == 0 {
        0.0
    } else {
        let minutes = duration_ms as f64 / 60_000.0;
        round2(words.len() as f64 / minutes)
    };
    AsrEstimate {
        words,
        words_per_minute,
        backend: AsrBackend::SilenceBursts,
        note,
    }
}

fn bursts(pcm: &[f32], sample_rate: u32) -> Vec<Word> {
    if sample_rate == 0 || pcm.is_empty() {
        return Vec::new();
    }
    let window = (sample_rate / 100).max(1) as usize;
    let window_ms = 10u64;
    let mut words = Vec::new();
    let mut burst_start: Option<u64> = None;
    let mut silent_run_ms = 0u64;
    let mut index = 0usize;
    let mut window_index = 0u64;
    while index < pcm.len() {
        let end = (index + window).min(pcm.len());
        let silent = window_db(&pcm[index..end]) < THRESHOLD_DB;
        let start_ms = window_index * window_ms;
        if silent {
            silent_run_ms = silent_run_ms.saturating_add(window_ms);
            if silent_run_ms >= MIN_SPLIT_MS {
                if let Some(opened) = burst_start.take() {
                    let end_ms = start_ms + window_ms - silent_run_ms;
                    push_word(&mut words, opened, end_ms.max(opened));
                }
            }
        } else {
            if burst_start.is_none() {
                burst_start = Some(start_ms);
            }
            silent_run_ms = 0;
        }
        index = end;
        window_index += 1;
    }
    if let Some(opened) = burst_start {
        let end_ms = (pcm.len() as u128 * 1_000 / u128::from(sample_rate)) as u64;
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
        let estimate = transcribe(&pcm, rate, 500, None);
        assert_eq!(estimate.backend, AsrBackend::SilenceBursts);
        assert_eq!(estimate.words.len(), 2, "words={:?}", estimate.words);
        assert!(estimate.words_per_minute > 0.0);
        assert!(estimate.note.contains("cmake"));
        assert!(estimate.note.contains("not linked"));
    }
}
