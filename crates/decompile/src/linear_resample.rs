//! Linear resampling of decoded audio to 48 kHz as one stream.
//!
//! Resampling each decoded frame on its own dropped the fractional output
//! sample at every frame boundary: 44.1 kHz AAC (1,024 samples per frame) lost
//! 0.56 samples per frame, so a pause planted at 50.0 s was found 10-40 ms
//! early (status.md). Here the read position and the last input sample carry
//! across frames, so output sample k always reads input position
//! `k * in_rate / 48,000`, however the input was cut into frames.

use crate::decode::SAMPLE_RATE;

const OUT_RATE: u64 = SAMPLE_RATE as u64;

pub(crate) struct LinearResampler {
    in_rate: u64,
    /// Input samples seen so far.
    consumed: u64,
    /// Output samples produced so far.
    produced: u64,
    /// The last input sample, the left neighbour of a position that spans a
    /// frame boundary.
    last: f32,
}

impl LinearResampler {
    pub(crate) fn new(in_rate: u32) -> Self {
        Self {
            in_rate: u64::from(in_rate),
            consumed: 0,
            produced: 0,
            last: 0.0,
        }
    }

    pub(crate) fn in_rate(&self) -> u32 {
        self.in_rate as u32
    }

    /// Resample `input`, the next samples of the stream, onto `output`.
    pub(crate) fn push(&mut self, input: &[f32], output: &mut Vec<f32>) {
        if input.is_empty() || self.in_rate == 0 {
            return;
        }
        let end = self.consumed + input.len() as u64;
        loop {
            let (base, rem) = self.position();
            // The right neighbour only matters when the position is between
            // two input samples.
            let needed = if rem == 0 { base } else { base + 1 };
            if needed >= end {
                break;
            }
            let left = self.sample(input, base);
            output.push(if rem == 0 {
                left
            } else {
                let right = self.sample(input, base + 1);
                left + (right - left) * (rem as f32 / OUT_RATE as f32)
            });
            self.produced += 1;
        }
        self.consumed = end;
        self.last = input[input.len() - 1];
    }

    /// Emit the output samples that wait for a neighbour that never came, so
    /// the stream ends with `floor(input samples * 48,000 / in_rate)` samples.
    pub(crate) fn finish(&mut self, output: &mut Vec<f32>) {
        if self.in_rate == 0 {
            return;
        }
        let total = self.consumed * OUT_RATE / self.in_rate;
        while self.produced < total {
            output.push(self.last);
            self.produced += 1;
        }
    }

    /// The input position of the next output sample: the index of the sample
    /// to its left, and how far past it, in 1/48,000ths of a sample.
    fn position(&self) -> (u64, u64) {
        let scaled = self.produced * self.in_rate;
        (scaled / OUT_RATE, scaled % OUT_RATE)
    }

    /// Input sample `index` of the stream, taken from `input` (which starts at
    /// `consumed`), or the last sample of the previous frame.
    fn sample(&self, input: &[f32], index: u64) -> f32 {
        match index.checked_sub(self.consumed) {
            Some(offset) => input[offset as usize],
            None => self.last,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(rate: u32, input: &[f32], frame: usize) -> Vec<f32> {
        let mut resampler = LinearResampler::new(rate);
        let mut out = Vec::new();
        for chunk in input.chunks(frame) {
            resampler.push(chunk, &mut out);
        }
        resampler.finish(&mut out);
        out
    }

    #[test]
    fn sixty_seconds_of_44k_in_1024_sample_frames_is_2_880_000_samples() {
        let input = vec![0.0f32; 44_100 * 60];
        let out = run(44_100, &input, 1_024);
        assert!(out.len().abs_diff(2_880_000) <= 1, "{}", out.len());
    }

    #[test]
    fn an_impulse_at_50_seconds_lands_at_50_seconds() {
        let mut input = vec![0.0f32; 44_100 * 60];
        input[44_100 * 50] = 1.0;
        let out = run(44_100, &input, 1_024);
        let peak = out
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(index, _)| index)
            .expect("output");
        assert!(peak.abs_diff(48_000 * 50) <= 1, "peak at {peak}");
    }

    #[test]
    fn the_frame_size_does_not_change_the_output() {
        let input: Vec<f32> = (0..30_000).map(|i| ((i as f32) * 0.013).sin()).collect();
        let whole = run(44_100, &input, input.len());
        for frame in [1, 7, 1_024, 4_410] {
            assert_eq!(run(44_100, &input, frame), whole, "frame {frame}");
        }
    }

    #[test]
    fn a_ramp_stays_a_ramp_across_frame_boundaries() {
        // Linear interpolation of a line is the line, so any seam shows up.
        let input: Vec<f32> = (0..4_000).map(|i| i as f32).collect();
        let out = run(32_000, &input, 1_000);
        for (index, value) in out.iter().enumerate().take(5_900) {
            let expected = index as f32 * 32_000.0 / 48_000.0;
            assert!(
                (value - expected).abs() < 1e-2,
                "{index}: {value} vs {expected}"
            );
        }
    }

    #[test]
    fn upsampling_from_8k_also_keeps_the_length() {
        let out = run(8_000, &vec![0.0; 8_000 * 3], 160);
        assert!(out.len().abs_diff(144_000) <= 1, "{}", out.len());
    }

    #[test]
    fn a_stream_shorter_than_one_output_sample_gives_nothing() {
        assert!(run(44_100, &[], 1_024).is_empty());
    }
}
