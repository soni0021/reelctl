//! From the model's foreground and alpha to the bytes that are encoded.
//!
//! `fgr` is planar `[3, n]` and `pha` is `[n]`, both already clamped to 0..1
//! ([`crate::tensor::clamp_unit`]); `n` is width times height. Output pixels are
//! interleaved.

/// What the command writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    /// The presenter over a background (`rgb24`).
    Composite,
    /// The alpha matte alone, as grey (`gray`).
    Matte,
    /// The presenter with straight alpha (`rgba`).
    Rgba,
}

impl OutputMode {
    /// Bytes per pixel of the raw frames sent to the encoder.
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Composite => 3,
            Self::Matte => 1,
            Self::Rgba => 4,
        }
    }

    /// The `ffmpeg` pixel format name of those raw frames.
    pub fn raw_pixel_format(self) -> &'static str {
        match self {
            Self::Composite => "rgb24",
            Self::Matte => "gray",
            Self::Rgba => "rgba",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Composite => "composite",
            Self::Matte => "matte",
            Self::Rgba => "rgba",
        }
    }
}

/// An 8-bit value for a 0..1 value.
fn byte(unit: f32) -> u8 {
    (unit * 255.0).round() as u8
}

/// `fgr * alpha + background * (1 - alpha)` per channel, rounded. `background`
/// and `out` are interleaved `rgb24`.
pub fn composite_frame(fgr: &[f32], pha: &[f32], background: &[u8], out: &mut [u8]) {
    let n = pha.len();
    for (i, alpha) in pha.iter().enumerate() {
        for channel in 0..3 {
            let foreground = fgr[channel * n + i] * 255.0;
            let behind = f32::from(background[3 * i + channel]);
            out[3 * i + channel] = (foreground * alpha + behind * (1.0 - alpha)).round() as u8;
        }
    }
}

/// The alpha as one grey byte per pixel.
pub fn matte_frame(pha: &[f32], out: &mut [u8]) {
    for (alpha, slot) in pha.iter().zip(out.iter_mut()) {
        *slot = byte(*alpha);
    }
}

/// The foreground with straight (not premultiplied) alpha, interleaved `rgba`.
pub fn rgba_frame(fgr: &[f32], pha: &[f32], out: &mut [u8]) {
    let n = pha.len();
    for (i, alpha) in pha.iter().enumerate() {
        for channel in 0..3 {
            out[4 * i + channel] = byte(fgr[channel * n + i]);
        }
        out[4 * i + 3] = byte(*alpha);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_say_how_big_their_frames_are() {
        assert_eq!(OutputMode::Composite.bytes_per_pixel(), 3);
        assert_eq!(OutputMode::Matte.bytes_per_pixel(), 1);
        assert_eq!(OutputMode::Rgba.bytes_per_pixel(), 4);
        assert_eq!(OutputMode::Composite.raw_pixel_format(), "rgb24");
        assert_eq!(OutputMode::Matte.raw_pixel_format(), "gray");
        assert_eq!(OutputMode::Rgba.raw_pixel_format(), "rgba");
        assert_eq!(OutputMode::Composite.name(), "composite");
        assert_eq!(OutputMode::Matte.name(), "matte");
        assert_eq!(OutputMode::Rgba.name(), "rgba");
    }

    /// Two pixels; planes R, G, B of two values each.
    const FGR: [f32; 6] = [1.0, 0.2, 0.0, 0.4, 0.5, 1.0];

    #[test]
    fn alpha_one_shows_the_foreground_and_zero_shows_the_background() {
        let background = [10, 20, 30, 200, 100, 50];
        let mut out = [0u8; 6];
        composite_frame(&FGR, &[1.0, 0.0], &background, &mut out);
        // Pixel 0 is the foreground (1.0, 0.0, 0.5); pixel 1 is the background.
        assert_eq!(out, [255, 0, 128, 200, 100, 50]);
    }

    #[test]
    fn alpha_half_mixes_foreground_and_background() {
        let background = [100, 100, 100, 100, 100, 100];
        let mut out = [0u8; 6];
        composite_frame(&FGR, &[0.5, 0.5], &background, &mut out);
        // Pixel 0: 0.5*255 + 0.5*100 = 177.5 -> 178; 0 + 50 = 50; 0.5*127.5+50 = 113.75 -> 114.
        assert_eq!(out[..3], [178, 50, 114]);
        // Pixel 1: 0.2 -> 0.5*51 + 50 = 75.5 -> 76; 0.4 -> 0.5*102+50 = 101; 1.0 -> 177.5 -> 178.
        assert_eq!(out[3..], [76, 101, 178]);
    }

    #[test]
    fn the_foreground_estimate_is_used_not_the_source() {
        // With alpha 1 the output is `fgr`, whatever the background held.
        let background = [9, 9, 9];
        let mut out = [0u8; 3];
        composite_frame(&[0.0, 1.0, 0.0], &[1.0], &background, &mut out);
        assert_eq!(out, [0, 255, 0]);
    }

    #[test]
    fn the_matte_is_alpha_in_eight_bits() {
        let mut out = [0u8; 4];
        matte_frame(&[0.0, 1.0, 0.5, 0.2], &mut out);
        assert_eq!(out, [0, 255, 128, 51]);
    }

    #[test]
    fn rgba_is_foreground_then_straight_alpha() {
        let mut out = [0u8; 8];
        rgba_frame(&FGR, &[1.0, 0.25], &mut out);
        assert_eq!(out, [255, 0, 128, 255, 51, 102, 255, 64]);
    }
}
