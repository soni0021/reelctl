//! Which moments of a video to sample.

/// `samples` times in milliseconds, evenly spaced over `duration_ms`: the middle of
/// each of `samples` equal slices, so the first frame (often a fade-in) and the
/// last (often a fade-out or an end card) are not the ones chosen. Ascending, all
/// below `duration_ms` unless the duration is 0, and the same for the same inputs.
pub fn sample_times_ms(duration_ms: u64, samples: usize) -> Vec<u64> {
    let slices = samples as u128;
    (0..slices)
        .map(|k| {
            let at = u128::from(duration_ms) * (2 * k + 1) / (2 * slices);
            // Below `duration_ms` (or 0), so it always fits.
            u64::try_from(at).unwrap_or(u64::MAX)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_samples_is_no_times_and_one_is_the_middle() {
        assert!(sample_times_ms(10_000, 0).is_empty());
        assert_eq!(sample_times_ms(10_000, 1), [5_000]);
    }

    #[test]
    fn times_are_slice_middles_ascending_and_inside_the_video() {
        assert_eq!(sample_times_ms(12_000, 4), [1_500, 4_500, 7_500, 10_500]);
        for n in [1usize, 2, 3, 7, 60, 600] {
            let times = sample_times_ms(12_345, n);
            assert_eq!(times.len(), n);
            assert!(times.windows(2).all(|pair| pair[0] <= pair[1]));
            assert!(times.iter().all(|&t| t < 12_345));
        }
    }

    #[test]
    fn a_zero_duration_samples_time_zero_and_huge_ones_do_not_overflow() {
        assert_eq!(sample_times_ms(0, 3), [0, 0, 0]);
        let times = sample_times_ms(u64::MAX, 600);
        assert_eq!(times.len(), 600);
        assert!(times.iter().all(|&t| t < u64::MAX));
    }
}
