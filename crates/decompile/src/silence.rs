//! Silence gaps from mono PCM.
//!
//! A 10 ms window whose mean-square level is below -40 dBFS is silent.
//! Runs shorter than 80 ms are dropped so encoder padding is not a gap.
//! The diff flag for dead air is a separate, higher threshold (180 ms).

use edit_dna::SilenceGap;

const THRESHOLD_DB: f64 = -40.0;
const MIN_GAP_MS: u64 = 80;

pub fn detect_gaps(pcm: &[f32], sample_rate: u32) -> Vec<SilenceGap> {
    if sample_rate == 0 || pcm.is_empty() {
        return Vec::new();
    }
    let window = (sample_rate / 100).max(1) as usize;
    let window_ms = 10u64;
    let mut gaps = Vec::new();
    let mut gap_start: Option<u64> = None;
    let mut index = 0usize;
    let mut window_index = 0u64;
    while index < pcm.len() {
        let end = (index + window).min(pcm.len());
        let silent = window_db(&pcm[index..end]) < THRESHOLD_DB;
        let start_ms = window_index * window_ms;
        let next_ms = start_ms + window_ms;
        match (silent, gap_start) {
            (true, None) => gap_start = Some(start_ms),
            (false, Some(opened)) => {
                push_gap(&mut gaps, opened, start_ms);
                gap_start = None;
            }
            (true, Some(_)) | (false, None) => {}
        }
        index = end;
        window_index += 1;
        let _ = next_ms;
    }
    if let Some(opened) = gap_start {
        let end_ms = pcm_duration_ms(pcm.len(), sample_rate);
        push_gap(&mut gaps, opened, end_ms);
    }
    gaps
}

fn push_gap(gaps: &mut Vec<SilenceGap>, start_ms: u64, end_ms: u64) {
    let duration_ms = end_ms.saturating_sub(start_ms);
    if duration_ms >= MIN_GAP_MS {
        gaps.push(SilenceGap {
            start_ms,
            end_ms,
            duration_ms,
        });
    }
}

pub fn window_db(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return -120.0;
    }
    let mut sum = 0.0f64;
    for sample in samples {
        let value = f64::from(*sample);
        sum += value * value;
    }
    let mean = sum / samples.len() as f64;
    if mean <= 1e-20 {
        return -120.0;
    }
    10.0 * mean.log10()
}

fn pcm_duration_ms(samples: usize, sample_rate: u32) -> u64 {
    (samples as u128 * 1_000 / u128::from(sample_rate.max(1))) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize) -> Vec<f32> {
        (0..n)
            .map(|index| (index as f32 * 0.3).sin() * 0.2)
            .collect()
    }

    #[test]
    fn finds_a_planted_half_second_gap_and_ignores_a_short_dip() {
        let rate = 48_000u32;
        let mut pcm = tone(rate as usize);
        pcm.extend(std::iter::repeat(0.0).take(rate as usize / 2));
        pcm.extend(tone(rate as usize));
        pcm.extend(std::iter::repeat(0.0).take(rate as usize / 20));
        pcm.extend(tone(rate as usize / 10));
        let gaps = detect_gaps(&pcm, rate);
        assert!(
            gaps.iter().any(|gap| {
                gap.duration_ms > 180 && gap.start_ms <= 1_100 && gap.end_ms >= 1_400
            }),
            "gaps={gaps:?}"
        );
        assert!(
            gaps.iter().all(|gap| gap.duration_ms >= MIN_GAP_MS),
            "short dips stay under the record threshold: {gaps:?}"
        );
    }

    #[test]
    fn digital_silence_is_far_below_a_tone() {
        assert!(window_db(&[0.0; 48]) < THRESHOLD_DB);
        let tone = tone(480);
        assert!(window_db(&tone) > THRESHOLD_DB);
    }
}
