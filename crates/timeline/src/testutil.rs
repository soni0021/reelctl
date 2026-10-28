//! Shared builders for the unit tests.

use edit_dna::{AsrBackend, EditDna, SilenceGap, Word};

use crate::init::{init, InitSpec};
use crate::otio::Timeline;

/// A project for `/media/take.mp4`: 640x360 with audio, `frames` frames at `fps`.
pub(crate) fn project(fps: f64, frames: i64) -> Timeline {
    init(&InitSpec {
        media_path: "/media/take.mp4".to_string(),
        fps,
        frames,
        width: 640,
        height: 360,
        has_audio: true,
        dna: None,
    })
    .expect("init")
}

/// An edit DNA with these silence gaps `(start_ms, end_ms)`.
pub(crate) fn dna(duration_ms: u64, fps: f64, gaps: &[(u64, u64)]) -> EditDna {
    let mut dna = EditDna::empty(duration_ms, fps, 640, 360);
    dna.silence_gaps = gaps
        .iter()
        .map(|&(start_ms, end_ms)| SilenceGap {
            start_ms,
            end_ms,
            duration_ms: end_ms - start_ms,
        })
        .collect();
    dna
}

/// `dna` with whisper words `(text, start_ms, end_ms)`.
pub(crate) fn with_words(mut dna: EditDna, words: &[(&str, u64, u64)]) -> EditDna {
    dna.asr_backend = AsrBackend::WhisperCpp;
    dna.words = words
        .iter()
        .map(|&(text, start_ms, end_ms)| Word {
            text: text.to_string(),
            start_ms,
            end_ms,
        })
        .collect();
    dna
}
