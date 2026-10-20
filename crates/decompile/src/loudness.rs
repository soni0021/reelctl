//! Integrated loudness, ITU-R BS.1770 / EBU R128, for mono PCM at 48 kHz.
//!
//! Two biquads (K-weighting high shelf, then the RLB high-pass) use the
//! widely published 48 kHz coefficients. Gating is 400 ms blocks, 100 ms hop,
//! absolute gate -70 LUFS, relative gate -10 LU.

const SHELF_B0: f64 = 1.535_124_859_586_97;
const SHELF_B1: f64 = -2.691_696_189_406_38;
const SHELF_B2: f64 = 1.198_392_810_852_85;
const SHELF_A1: f64 = -1.690_659_293_182_41;
const SHELF_A2: f64 = 0.732_480_774_215_85;

const HIGHPASS_B0: f64 = 1.0;
const HIGHPASS_B1: f64 = -2.0;
const HIGHPASS_B2: f64 = 1.0;
const HIGHPASS_A1: f64 = -1.990_047_454_833_98;
const HIGHPASS_A2: f64 = 0.990_072_250_366_21;

struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    fn new(b0: f64, b1: f64, b2: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0,
            b1,
            b2,
            a1,
            a2,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x0: f64) -> f64 {
        let y0 = self.b0 * x0 + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x0;
        self.y2 = self.y1;
        self.y1 = y0;
        y0
    }
}

pub fn integrated_lufs(samples: &[f32], sample_rate: u32) -> f64 {
    if sample_rate == 0 || samples.is_empty() {
        return -70.0;
    }
    let mut shelf = Biquad::new(SHELF_B0, SHELF_B1, SHELF_B2, SHELF_A1, SHELF_A2);
    let mut highpass = Biquad::new(
        HIGHPASS_B0,
        HIGHPASS_B1,
        HIGHPASS_B2,
        HIGHPASS_A1,
        HIGHPASS_A2,
    );
    let block = ((u64::from(sample_rate) * 400) / 1_000).max(1) as usize;
    let hop = ((u64::from(sample_rate) * 100) / 1_000).max(1) as usize;
    let mut ring = vec![0.0f64; block];
    let mut ring_index = 0usize;
    let mut filled = 0usize;
    let mut sum_squares = 0.0f64;
    let mut since_hop = 0usize;
    let mut blocks = Vec::new();
    for sample in samples {
        let filtered = highpass.process(shelf.process(f64::from(*sample)));
        let square = filtered * filtered;
        if filled == block {
            sum_squares -= ring[ring_index];
        } else {
            filled += 1;
        }
        ring[ring_index] = square;
        sum_squares += square;
        ring_index = (ring_index + 1) % block;
        if filled == block {
            since_hop += 1;
            if since_hop == 1 || since_hop > hop {
                blocks.push(sum_squares / block as f64);
                since_hop = 1;
            }
        }
    }
    let gated = gate(&blocks);
    round2(gated)
}

fn gate(blocks: &[f64]) -> f64 {
    let absolute: Vec<f64> = blocks
        .iter()
        .copied()
        .filter(|mean| energy_to_lufs(*mean) > -70.0)
        .collect();
    if absolute.is_empty() {
        return -70.0;
    }
    let ungated = absolute.iter().sum::<f64>() / absolute.len() as f64;
    let relative = energy_to_lufs(ungated) - 10.0;
    let kept: Vec<f64> = absolute
        .into_iter()
        .filter(|mean| energy_to_lufs(*mean) >= relative)
        .collect();
    if kept.is_empty() {
        return -70.0;
    }
    let mean = kept.iter().sum::<f64>() / kept.len() as f64;
    energy_to_lufs(mean)
}

fn energy_to_lufs(mean_square: f64) -> f64 {
    if mean_square <= 1e-12 {
        return -70.0;
    }
    -0.691 + 10.0 * mean_square.log10()
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_at_the_floor_and_a_sine_is_above_it() {
        let quiet = vec![0.0f32; 48_000];
        assert_eq!(integrated_lufs(&quiet, 48_000), -70.0);
        let mut tone = Vec::with_capacity(48_000 * 3);
        let amp = 0.070_794_58f32;
        for n in 0..(48_000 * 3) {
            let t = n as f32 / 48_000.0;
            tone.push((t * 997.0 * std::f32::consts::TAU).sin() * amp);
        }
        let measured = integrated_lufs(&tone, 48_000);
        assert!(measured > -40.0 && measured < -5.0, "lufs={measured}");
    }
}
