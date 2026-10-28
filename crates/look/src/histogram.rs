//! Per-channel 256-bin histograms of 8-bit RGB pixels.

/// Levels in an 8-bit channel.
pub const LEVELS: usize = 256;

/// How many pixels have each level of one channel.
pub type Histogram = [u64; LEVELS];

/// A histogram for each of R, G and B.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Histograms {
    pub channels: [Histogram; 3],
}

impl Default for Histograms {
    fn default() -> Self {
        Self {
            channels: [[0; LEVELS]; 3],
        }
    }
}

impl Histograms {
    /// Count the pixels of a packed `R, G, B, R, G, B, ...` buffer. A trailing
    /// partial pixel is ignored.
    pub fn add_rgb(&mut self, rgb: &[u8]) {
        let [red, green, blue] = &mut self.channels;
        let (pixels, _partial) = rgb.as_chunks::<3>();
        for &[r, g, b] in pixels {
            // A level is a `u8`, so it is always one of the 256 bins.
            red[usize::from(r)] += 1;
            green[usize::from(g)] += 1;
            blue[usize::from(b)] += 1;
        }
    }

    /// The histograms of one packed RGB buffer.
    pub fn from_rgb(rgb: &[u8]) -> Self {
        let mut histograms = Self::default();
        histograms.add_rgb(rgb);
        histograms
    }

    /// How many pixels were counted.
    pub fn pixels(&self) -> u64 {
        self.channels
            .first()
            .map_or(0, |histogram| histogram.iter().sum())
    }

    /// The mean level of channel `channel` (0 = R, 1 = G, 2 = B), or `None` when no
    /// pixel was counted or the channel is not 0 to 2.
    pub fn mean(&self, channel: usize) -> Option<f64> {
        let histogram = self.channels.get(channel)?;
        let total: u64 = histogram.iter().sum();
        if total == 0 {
            return None;
        }
        let weighted: f64 = histogram
            .iter()
            .enumerate()
            .map(|(level, &count)| level as f64 * count as f64)
            .sum();
        Some(weighted / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_are_counted_per_channel() {
        // (10, 20, 30), (10, 25, 30), (200, 20, 0)
        let h = Histograms::from_rgb(&[10, 20, 30, 10, 25, 30, 200, 20, 0]);
        assert_eq!(h.pixels(), 3);
        assert_eq!(h.channels[0][10], 2);
        assert_eq!(h.channels[0][200], 1);
        assert_eq!(h.channels[1][20], 2);
        assert_eq!(h.channels[1][25], 1);
        assert_eq!(h.channels[2][30], 2);
        assert_eq!(h.channels[2][0], 1);
        for channel in &h.channels {
            assert_eq!(channel.iter().sum::<u64>(), 3);
        }
    }

    #[test]
    fn a_trailing_partial_pixel_and_an_empty_buffer_count_nothing() {
        assert_eq!(Histograms::from_rgb(&[]).pixels(), 0);
        assert_eq!(Histograms::from_rgb(&[1, 2]).pixels(), 0);
        assert_eq!(Histograms::from_rgb(&[1, 2, 3, 4, 5]).pixels(), 1);
    }

    #[test]
    fn buffers_add_up() {
        let mut h = Histograms::default();
        h.add_rgb(&[1, 2, 3]);
        h.add_rgb(&[1, 2, 3, 255, 255, 255]);
        assert_eq!(h.pixels(), 3);
        assert_eq!(h.channels[0][1], 2);
        assert_eq!(h.channels[0][255], 1);
    }

    #[test]
    fn the_mean_is_per_channel_and_none_when_empty() {
        let h = Histograms::from_rgb(&[0, 100, 255, 100, 100, 255]);
        assert_eq!(h.mean(0), Some(50.0));
        assert_eq!(h.mean(1), Some(100.0));
        assert_eq!(h.mean(2), Some(255.0));
        assert_eq!(h.mean(3), None);
        assert_eq!(Histograms::default().mean(0), None);
    }
}
