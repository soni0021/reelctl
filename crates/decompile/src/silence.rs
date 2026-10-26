//! Silence gaps from mono PCM.
//!
//! A 10 ms window whose mean-square level is below the recording's threshold
//! is silent. The threshold is `max(-40, min(p10 + 6, p90 - 12))` dB over all
//! window levels (see `threshold_db`), so a pause under room noise is still a
//! gap while a clean recording keeps -40 dBFS. A gap ends only after 30 ms of
//! sound, so a noise spike does not split a pause. Runs shorter than 80 ms are
//! dropped so encoder padding is not a gap. The diff flag for dead air is a
//! separate, higher limit (180 ms).
//!
//! The numbers come from docs/specs/relative-silence.md: on 60 s of a real
//! recording with room noise, thresholds up to -34 dB called no word start
//! silence, and requiring 40 ms of sound instead of 30 hid one word start.

use edit_dna::SilenceGap;

const WINDOW_MS: u64 = 10;
const MIN_GAP_MS: u64 = 80;
const MIN_SOUND_WINDOWS: usize = 3;
const FLOOR_DB: f64 = -40.0;
const NOISE_MARGIN_DB: f64 = 6.0;
const HEADROOM_DB: f64 = 12.0;

pub struct SilenceAnalysis {
    /// One level per 10 ms window, in dB.
    pub levels: Vec<f64>,
    pub gaps: Vec<SilenceGap>,
    pub threshold_db: f64,
    /// The 10th-percentile window level; `None` without audio.
    pub noise_floor_db: Option<f64>,
}

pub fn analyze(pcm: &[f32], sample_rate: u32) -> SilenceAnalysis {
    if sample_rate == 0 || pcm.is_empty() {
        return SilenceAnalysis {
            levels: Vec::new(),
            gaps: Vec::new(),
            threshold_db: FLOOR_DB,
            noise_floor_db: None,
        };
    }
    let levels = window_levels(pcm, sample_rate);
    let threshold_db = threshold_db(&levels);
    let gaps = detect_gaps(
        &levels,
        threshold_db,
        pcm_duration_ms(pcm.len(), sample_rate),
    );
    SilenceAnalysis {
        noise_floor_db: Some(percentile(&levels, 10)),
        levels,
        gaps,
        threshold_db,
    }
}

/// Gaps over window levels. A gap ends where at least `MIN_SOUND_WINDOWS`
/// windows in a row reach the threshold; a gap still open when the audio ends
/// closes at `duration_ms`, or at its last quiet window if a shorter sound
/// trails it.
pub fn detect_gaps(levels: &[f64], threshold_db: f64, duration_ms: u64) -> Vec<SilenceGap> {
    let mut gaps = Vec::new();
    let mut gap_start: Option<usize> = None;
    let mut quiet_end = 0usize;
    let mut loud_run = 0usize;
    for (index, level) in levels.iter().enumerate() {
        if *level < threshold_db {
            gap_start.get_or_insert(index);
            quiet_end = index + 1;
            loud_run = 0;
        } else if let Some(start) = gap_start {
            loud_run += 1;
            if loud_run >= MIN_SOUND_WINDOWS {
                push_gap(&mut gaps, window_ms(start), window_ms(quiet_end));
                gap_start = None;
                loud_run = 0;
            }
        }
    }
    if let Some(start) = gap_start {
        let end_ms = if loud_run == 0 {
            duration_ms
        } else {
            window_ms(quiet_end)
        };
        push_gap(&mut gaps, window_ms(start), end_ms);
    }
    gaps
}

fn window_ms(index: usize) -> u64 {
    index as u64 * WINDOW_MS
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
        let gaps = analyze(&pcm, rate).gaps;
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

    /// A sine at `amplitude`; its level is `20 log10(amplitude / sqrt 2)` dB.
    fn sine(amplitude: f32, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| (index as f32 * 0.3).sin() * amplitude)
            .collect()
    }

    fn spans(gaps: &[SilenceGap]) -> Vec<(u64, u64)> {
        gaps.iter().map(|gap| (gap.start_ms, gap.end_ms)).collect()
    }

    #[test]
    fn a_pause_under_room_noise_is_a_gap_at_the_recording_threshold() {
        let rate = 48_000u32;
        let mut pcm = sine(0.141, 2 * rate as usize); // about -20 dB
        pcm.extend(sine(0.0178, rate as usize / 2)); // about -38 dB
        pcm.extend(sine(0.141, 2 * rate as usize));
        let analysis = analyze(&pcm, rate);
        assert!(analysis.threshold_db > -38.0, "{}", analysis.threshold_db);
        assert_eq!(spans(&analysis.gaps), vec![(2_000, 2_500)]);
        assert!(detect_gaps(&analysis.levels, FLOOR_DB, 4_500).is_empty());
    }

    #[test]
    fn sounds_shorter_than_30_ms_stay_inside_a_gap() {
        for (loud_windows, expected) in [
            (1, vec![(0, 610)]),
            (2, vec![(0, 620)]),
            (3, vec![(0, 300), (330, 630)]),
        ] {
            let mut levels = repeat(-60.0, 30);
            levels.extend(repeat(-20.0, loud_windows));
            levels.extend(repeat(-60.0, 30));
            let duration_ms = levels.len() as u64 * 10;
            let gaps = detect_gaps(&levels, FLOOR_DB, duration_ms);
            assert_eq!(spans(&gaps), expected, "{loud_windows} loud windows");
        }
    }

    #[test]
    fn a_gap_still_open_at_the_end_closes_with_the_audio() {
        let mut levels = repeat(-20.0, 10);
        levels.extend(repeat(-60.0, 20));
        assert_eq!(
            spans(&detect_gaps(&levels, FLOOR_DB, 295)),
            vec![(100, 295)]
        );
        levels.extend(repeat(-20.0, 2));
        assert_eq!(
            spans(&detect_gaps(&levels, FLOOR_DB, 320)),
            vec![(100, 300)]
        );
    }

    #[test]
    fn no_audio_has_no_gaps_and_no_noise_floor() {
        let analysis = analyze(&[], 48_000);
        assert!(analysis.gaps.is_empty());
        assert_eq!(analysis.threshold_db, FLOOR_DB);
        assert_eq!(analysis.noise_floor_db, None);
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

    /// Window levels shaped like a very quiet download: p10 -56, p90 -44, p99 -42.
    fn quiet_levels() -> Vec<f64> {
        let mut levels = repeat(-56.0, 11);
        levels.extend(repeat(-49.0, 69));
        levels.extend(repeat(-44.0, 19));
        levels.extend(repeat(-42.0, 1));
        levels
    }

    #[test]
    fn a_recording_quieter_than_the_floor_gets_a_threshold_under_its_loud_part() {
        // The floor follows p99 - 12 = -54; the noise term p10 + 6 = -50 would
        // be above it but the cap p90 - 12 = -56 is below it.
        assert_eq!(threshold_db(&quiet_levels()), -54.0);
    }

    #[test]
    fn a_constant_quiet_tone_is_capped_below_itself() {
        assert_eq!(threshold_db(&repeat(-50.0, 100)), -62.0);
    }

    #[test]
    fn a_pause_in_a_very_quiet_tone_is_a_gap_and_the_tone_is_not() {
        let rate = 48_000u32;
        // 4 s of a tone at -46 dB peak (about -49 dB), 0.5 s of silence, 4 s of tone.
        let mut pcm = sine(0.005, 4 * rate as usize);
        pcm.extend(std::iter::repeat(0.0).take(rate as usize / 2));
        pcm.extend(sine(0.005, 4 * rate as usize));
        let analysis = analyze(&pcm, rate);
        assert!(
            analysis.threshold_db < FLOOR_DB,
            "{}",
            analysis.threshold_db
        );
        let long: Vec<(u64, u64)> = spans(&analysis.gaps)
            .into_iter()
            .filter(|(start, end)| end - start >= 180)
            .collect();
        assert_eq!(long.len(), 1, "{:?}", analysis.gaps);
        assert!(long[0].0.abs_diff(4_000) <= 20 && long[0].1.abs_diff(4_500) <= 20);
    }

    #[test]
    fn a_recording_that_is_mostly_silence_keeps_the_fixed_floor() {
        // The loud 5% is at -14 dB, so p99 is high enough to leave the floor alone.
        let mut levels = repeat(-120.0, 95);
        levels.extend(repeat(-14.0, 5));
        assert_eq!(threshold_db(&levels), -40.0);
        // p90 is the silence level here, which a cap applied last would follow.
        let mut levels = repeat(-60.0, 92);
        levels.extend(repeat(-20.0, 8));
        assert_eq!(threshold_db(&levels), -40.0);
    }

    #[test]
    fn under_one_percent_sound_cannot_push_the_floor_below_the_silence() {
        // p99 is the silence level (-120), so p99 - 12 = -132 would sit under
        // the silence. The lower bound holds the threshold at -70 instead.
        let mut levels = repeat(-120.0, 995);
        levels.extend(repeat(-14.0, 5));
        assert_eq!(threshold_db(&levels), -70.0);
        assert_eq!(spans(&detect_gaps(&levels, -70.0, 10_000)), [(0, 9_950)]);
        // Only digital silence is one gap, with a finite threshold.
        let analysis = analyze(&[0.0; 48_000], 48_000);
        assert_eq!(analysis.threshold_db, -70.0);
        assert_eq!(spans(&analysis.gaps), [(0, 1_000)]);
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
        assert!(window_db(&[0.0; 48]) < FLOOR_DB);
        let tone = tone(480);
        assert!(window_db(&tone) > FLOOR_DB);
    }
}
