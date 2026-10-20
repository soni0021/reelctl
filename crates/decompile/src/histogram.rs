//! Per-shot RGB histogram summaries from subsampled YUV pixels.

use edit_dna::{Cut, ShotHistogram};

pub(crate) const HISTOGRAM_BINS: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct FrameColor {
    pub frame_index: u64,
    pub time_ms: u64,
    pub samples: u32,
    pub sum_r: u64,
    pub sum_g: u64,
    pub sum_b: u64,
    pub bins_r: [u32; HISTOGRAM_BINS],
    pub bins_g: [u32; HISTOGRAM_BINS],
    pub bins_b: [u32; HISTOGRAM_BINS],
}

impl FrameColor {
    pub(crate) fn new(frame_index: u64, time_ms: u64) -> Self {
        Self {
            frame_index,
            time_ms,
            samples: 0,
            sum_r: 0,
            sum_g: 0,
            sum_b: 0,
            bins_r: [0; HISTOGRAM_BINS],
            bins_g: [0; HISTOGRAM_BINS],
            bins_b: [0; HISTOGRAM_BINS],
        }
    }

    pub(crate) fn push_rgb(&mut self, rgb: [u8; 3]) {
        self.samples += 1;
        self.sum_r += u64::from(rgb[0]);
        self.sum_g += u64::from(rgb[1]);
        self.sum_b += u64::from(rgb[2]);
        self.bins_r[(rgb[0] >> 5) as usize] += 1;
        self.bins_g[(rgb[1] >> 5) as usize] += 1;
        self.bins_b[(rgb[2] >> 5) as usize] += 1;
    }
}

pub(crate) fn yuv601_to_rgb(y: u8, u: u8, v: u8, full_range: bool) -> [u8; 3] {
    let (c, d, e) = if full_range {
        (i32::from(y), i32::from(u) - 128, i32::from(v) - 128)
    } else {
        (i32::from(y) - 16, i32::from(u) - 128, i32::from(v) - 128)
    };
    let (r, g, b) = if full_range {
        (
            c + ((359 * e) >> 8),
            c - ((88 * d + 183 * e) >> 8),
            c + ((454 * d) >> 8),
        )
    } else {
        (
            (298 * c + 409 * e + 128) >> 8,
            (298 * c - 100 * d - 208 * e + 128) >> 8,
            (298 * c + 516 * d + 128) >> 8,
        )
    };
    [clamp_u8(r), clamp_u8(g), clamp_u8(b)]
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

pub(crate) fn aggregate(
    frames: &[FrameColor],
    cuts: &[Cut],
    duration_ms: u64,
    frame_count: u64,
) -> Vec<ShotHistogram> {
    let mut bounds = Vec::with_capacity(cuts.len() + 2);
    bounds.push(0u64);
    for cut in cuts {
        if cut.time_ms > 0 && cut.time_ms < duration_ms {
            bounds.push(cut.time_ms);
        }
    }
    if duration_ms > 0 {
        bounds.push(duration_ms);
    } else if let Some(last) = frames.last() {
        bounds.push(last.time_ms.saturating_add(1));
    }
    bounds.sort_unstable();
    bounds.dedup();
    if bounds.len() < 2 {
        bounds = vec![0, duration_ms.max(1)];
    }

    let mut shots = Vec::with_capacity(bounds.len() - 1);
    for (shot_index, pair) in bounds.windows(2).enumerate() {
        let start_ms = pair[0];
        let end_ms = pair[1];
        let mut samples = 0u64;
        let mut sum_r = 0u64;
        let mut sum_g = 0u64;
        let mut sum_b = 0u64;
        let mut bins_r = [0u64; HISTOGRAM_BINS];
        let mut bins_g = [0u64; HISTOGRAM_BINS];
        let mut bins_b = [0u64; HISTOGRAM_BINS];
        let mut start_frame = frame_count;
        let mut end_frame = 0u64;
        for frame in frames {
            let in_shot = if shot_index + 1 == bounds.len() - 1 {
                frame.time_ms >= start_ms && frame.time_ms <= end_ms
            } else {
                frame.time_ms >= start_ms && frame.time_ms < end_ms
            };
            if !in_shot {
                continue;
            }
            if frame.frame_index < start_frame {
                start_frame = frame.frame_index;
            }
            end_frame = end_frame.max(frame.frame_index.saturating_add(1));
            samples += u64::from(frame.samples);
            sum_r += frame.sum_r;
            sum_g += frame.sum_g;
            sum_b += frame.sum_b;
            for bin in 0..HISTOGRAM_BINS {
                bins_r[bin] += u64::from(frame.bins_r[bin]);
                bins_g[bin] += u64::from(frame.bins_g[bin]);
                bins_b[bin] += u64::from(frame.bins_b[bin]);
            }
        }
        if start_frame == frame_count {
            start_frame = 0;
        }
        shots.push(ShotHistogram {
            shot_index: shot_index as u32,
            start_ms,
            end_ms,
            start_frame,
            end_frame,
            mean_r: mean(sum_r, samples),
            mean_g: mean(sum_g, samples),
            mean_b: mean(sum_b, samples),
            bins_r: fractions(&bins_r, samples),
            bins_g: fractions(&bins_g, samples),
            bins_b: fractions(&bins_b, samples),
        });
    }
    shots
}

fn mean(sum: u64, samples: u64) -> f64 {
    if samples == 0 {
        return 0.0;
    }
    round2(sum as f64 / samples as f64)
}

fn fractions(bins: &[u64; HISTOGRAM_BINS], samples: u64) -> [f64; HISTOGRAM_BINS] {
    let mut out = [0.0; HISTOGRAM_BINS];
    if samples == 0 {
        return out;
    }
    for (index, bin) in bins.iter().enumerate() {
        out[index] = round4(*bin as f64 / samples as f64);
    }
    out
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limited_range_red_is_red() {
        let [r, g, b] = yuv601_to_rgb(81, 90, 240, false);
        assert!(r > 200, "rgb={r},{g},{b}");
        assert!(g < 40, "rgb={r},{g},{b}");
        assert!(b < 40, "rgb={r},{g},{b}");
    }

    #[test]
    fn shots_follow_cut_bounds() {
        let mut red = FrameColor::new(0, 0);
        red.push_rgb([220, 10, 10]);
        let mut white = FrameColor::new(90, 3_000);
        white.push_rgb([240, 240, 240]);
        let cuts = [Cut {
            frame_index: 90,
            time_ms: 3_000,
            interval_ms: 3_000,
            scene_score: 1.0,
        }];
        let shots = aggregate(&[red, white], &cuts, 6_000, 180);
        assert_eq!(shots.len(), 2);
        assert!(shots[0].mean_r > shots[0].mean_b);
        assert!(shots[1].mean_r > 200.0 && shots[1].mean_b > 200.0);
    }
}
