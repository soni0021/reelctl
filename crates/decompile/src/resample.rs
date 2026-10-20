//! 48 kHz mono PCM to the 16 kHz mono that whisper.cpp reads.
//!
//! A windowed-sinc low-pass removes what would alias above the new 8 kHz
//! Nyquist frequency, then every third sample is kept.

use std::f64::consts::PI;

use crate::error::DecompileError;

const INPUT_RATE: u32 = 48_000;
const RATIO: usize = 3;
const TAPS: usize = 127;
const CUTOFF_HZ: f64 = 7_000.0;

pub(crate) fn to_whisper_rate(pcm: &[f32], sample_rate: u32) -> Result<Vec<f32>, DecompileError> {
    if sample_rate != INPUT_RATE {
        return Err(DecompileError::Message(format!(
            "whisper input is resampled from {INPUT_RATE} Hz audio, got {sample_rate} Hz"
        )));
    }
    let taps = low_pass_taps();
    let half = TAPS / 2;
    let mut padded = vec![0.0f32; pcm.len() + TAPS - 1];
    padded[half..half + pcm.len()].copy_from_slice(pcm);
    Ok((0..pcm.len().div_ceil(RATIO))
        .map(|index| {
            let start = index * RATIO;
            padded[start..start + TAPS]
                .iter()
                .zip(&taps)
                .map(|(sample, tap)| sample * tap)
                .sum()
        })
        .collect())
}

fn low_pass_taps() -> [f32; TAPS] {
    let cutoff = CUTOFF_HZ / f64::from(INPUT_RATE);
    let last = (TAPS - 1) as f64;
    let mut taps = [0.0f64; TAPS];
    for (index, tap) in taps.iter_mut().enumerate() {
        let offset = index as f64 - last / 2.0;
        let sinc = if offset == 0.0 {
            2.0 * cutoff
        } else {
            (2.0 * PI * cutoff * offset).sin() / (PI * offset)
        };
        let phase = 2.0 * PI * index as f64 / last;
        let blackman = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
        *tap = sinc * blackman;
    }
    let gain: f64 = taps.iter().sum();
    taps.map(|tap| (tap / gain) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frequency: f32, rate: u32) -> Vec<f32> {
        (0..rate)
            .map(|index| (index as f32 * frequency * std::f32::consts::TAU / rate as f32).sin() * 0.5)
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        let sum: f64 = samples.iter().map(|value| f64::from(*value).powi(2)).sum();
        (sum / samples.len() as f64).sqrt()
    }

    // Edges are skipped so the filter's start-up transient is not measured.
    fn level_ratio(input: &[f32], output: &[f32]) -> f64 {
        rms(&output[200..output.len() - 200]) / rms(&input[600..input.len() - 600])
    }

    #[test]
    fn one_second_at_48_khz_is_one_second_at_16_khz() {
        let output = to_whisper_rate(&[0.0; 48_000], 48_000).expect("resample");
        assert_eq!(output.len(), 16_000);
    }

    #[test]
    fn a_1_khz_tone_keeps_its_level() {
        let input = tone(1_000.0, 48_000);
        let output = to_whisper_rate(&input, 48_000).expect("resample");
        let ratio = level_ratio(&input, &output);
        assert!((ratio - 1.0).abs() < 0.05, "ratio={ratio}");
    }

    #[test]
    fn a_12_khz_tone_drops_by_at_least_30_db() {
        let input = tone(12_000.0, 48_000);
        let output = to_whisper_rate(&input, 48_000).expect("resample");
        let ratio = level_ratio(&input, &output);
        assert!(ratio < 0.0316, "ratio={ratio}");
    }

    #[test]
    fn input_that_is_not_48_khz_is_rejected() {
        let err = to_whisper_rate(&[0.0; 441], 44_100).expect_err("44.1 kHz input");
        assert!(err.to_string().contains("44100"), "{err}");
    }
}
