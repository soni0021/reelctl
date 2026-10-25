//! Silence gaps from mono PCM.
//!
//! A 10 ms window whose mean-square level is below -40 dBFS is silent.
//! Runs shorter than 80 ms are dropped so encoder padding is not a gap.
//! The diff flag for dead air is a separate, higher threshold (180 ms).

use edit_dna::SilenceGap;

const THRESHOLD_DB: f64 = -40.0;
const MIN_GAP_MS: u64 = 80;
const FLOOR_DB: f64 = -40.0;
const NOISE_MARGIN_DB: f64 = 6.0;
const HEADROOM_DB: f64 = 12.0;

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

/// One level per 10 ms window, as `window_db` measures it.
pub fn window_levels(pcm: &[f32], sample_rate: u32) -> Vec<f64> {
    let window = (sample_rate / 100).max(1) as usize;
    pcm.chunks(window).map(window_db).collect()
}

/// The silence threshold for one recording: `max(-40, min(p10 + 6, p90 - 12))` dB.
/// p10 estimates the noise floor. The cap keeps the threshold 12 dB under the
/// loud part, so a recording with no quiet stretch is not marked silent. The
/// -40 floor keeps clean recordings at the fixed threshold used before.
pub fn threshold_db(levels: &[f64]) -> f64 {
    if levels.is_empty() {
        return FLOOR_DB;
    }
    let sorted = sorted(levels);
    let noise = rank(&sorted, 10);
    let loud = rank(&sorted, 90);
    (noise + NOISE_MARGIN_DB)
        .min(loud - HEADROOM_DB)
        .max(FLOOR_DB)
}

/// Nearest-rank percentile: `sorted[min(n - 1, p * n / 100)]`. `levels` must not be empty.
pub fn percentile(levels: &[f64], p: usize) -> f64 {
    rank(&sorted(levels), p)
}

fn sorted(levels: &[f64]) -> Vec<f64> {
    let mut sorted = levels.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted
}

fn rank(sorted: &[f64], p: usize) -> f64 {
    sorted[(p * sorted.len() / 100).min(sorted.len() - 1)]
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

    fn repeat(level_db: f64, count: usize) -> Vec<f64> {
        vec![level_db; count]
    }

    #[test]
    fn a_clean_recording_keeps_the_fixed_floor() {
        let mut levels = repeat(-120.0, 60);
        levels.extend(repeat(-14.0, 40));
        assert_eq!(threshold_db(&levels), -40.0);
    }

    #[test]
    fn room_noise_raises_the_threshold_above_the_noise_floor() {
        // p10 + 6 = -34 is below the cap p90 - 12 = -26.
        let mut levels = repeat(-40.0, 30);
        levels.extend(repeat(-14.0, 70));
        assert_eq!(threshold_db(&levels), -34.0);
    }

    #[test]
    fn a_constant_level_is_capped_below_the_loud_part() {
        // p10 + 6 = -15 would mark the tone silent; the cap p90 - 12 = -33 wins.
        assert_eq!(threshold_db(&repeat(-21.0, 100)), -33.0);
    }

    #[test]
    fn no_audio_keeps_the_fixed_floor() {
        assert_eq!(threshold_db(&[]), -40.0);
    }

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let levels: Vec<f64> = (0..10).map(f64::from).collect();
        assert_eq!(percentile(&levels, 10), 1.0);
        assert_eq!(percentile(&levels, 90), 9.0);
        assert_eq!(percentile(&[3.0, 1.0, 2.0], 0), 1.0);
    }

    #[test]
    fn one_level_per_10_ms_window() {
        let levels = window_levels(&vec![0.1; 48_000 + 100], 48_000);
        assert_eq!(levels.len(), 101);
        assert!(window_levels(&[], 48_000).is_empty());
    }

    #[test]
    fn digital_silence_is_far_below_a_tone() {
        assert!(window_db(&[0.0; 48]) < THRESHOLD_DB);
        let tone = tone(480);
        assert!(window_db(&tone) > THRESHOLD_DB);
    }
}
