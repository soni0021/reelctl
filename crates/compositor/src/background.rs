//! Where the pixels behind the presenter come from: one `rgb24` frame at a time,
//! the size of the output.

use std::io::Read;

use crate::error::CompositorError;

/// A source of background frames.
pub trait Background {
    /// Write the next frame (`width * height * 3` bytes of `rgb24`) into `out`.
    fn next_frame(&mut self, out: &mut [u8]) -> Result<(), CompositorError>;
}

/// One colour, everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolidColor(pub [u8; 3]);

impl Background for SolidColor {
    fn next_frame(&mut self, out: &mut [u8]) -> Result<(), CompositorError> {
        for pixel in out.chunks_exact_mut(3) {
            pixel.copy_from_slice(&self.0);
        }
        Ok(())
    }
}

/// Frames that `ffmpeg` has already scaled, cropped and looped to the output's
/// size and rate, read as raw `rgb24` from a pipe (or anything that reads).
#[derive(Debug)]
pub struct RawFrames<R: Read> {
    reader: R,
    frames: usize,
}

impl<R: Read> RawFrames<R> {
    pub fn new(reader: R) -> Self {
        Self { reader, frames: 0 }
    }
}

impl<R: Read> Background for RawFrames<R> {
    fn next_frame(&mut self, out: &mut [u8]) -> Result<(), CompositorError> {
        self.reader.read_exact(out).map_err(|error| {
            if error.kind() == std::io::ErrorKind::UnexpectedEof {
                CompositorError::run(format!(
                    "the background gave only {} frames and the video needs more",
                    self.frames
                ))
            } else {
                CompositorError::run(format!("cannot read the background: {error}"))
            }
        })?;
        self.frames += 1;
        Ok(())
    }
}

/// `#rrggbb` as a colour. The `#` is required so that a file named `abcdef` is a
/// file and not a colour.
pub fn parse_hex_colour(text: &str) -> Result<[u8; 3], CompositorError> {
    let bad = || {
        CompositorError::setup(format!(
            "\"{text}\" is not a #rrggbb colour (for a file, give the path of an image or video that exists)"
        ))
    };
    let digits = text.strip_prefix('#').ok_or_else(bad)?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).map_err(|_| bad());
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_solid_colour_fills_every_pixel() {
        let mut out = [0u8; 12];
        SolidColor([1, 2, 3]).next_frame(&mut out).unwrap();
        assert_eq!(out, [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3]);
    }

    #[test]
    fn hex_colours_are_parsed() {
        assert_eq!(parse_hex_colour("#000000").unwrap(), [0, 0, 0]);
        assert_eq!(parse_hex_colour("#ff8000").unwrap(), [255, 128, 0]);
        assert_eq!(parse_hex_colour("#1A2b3C").unwrap(), [26, 43, 60]);
    }

    #[test]
    fn anything_else_is_not_a_colour() {
        for bad in [
            "ff8000", "#ff800", "#ff80000", "#gg0000", "#", "", "red", "#ff 800", "#+f0000",
        ] {
            let error = parse_hex_colour(bad).unwrap_err();
            assert!(error.is_setup(), "{bad}");
            assert!(error.to_string().contains("#rrggbb"), "{bad}");
        }
    }

    #[test]
    fn raw_frames_are_read_one_after_another() {
        let bytes: Vec<u8> = (0..12).collect();
        let mut source = RawFrames::new(&bytes[..]);
        let mut frame = [0u8; 6];
        source.next_frame(&mut frame).unwrap();
        assert_eq!(frame, [0, 1, 2, 3, 4, 5]);
        source.next_frame(&mut frame).unwrap();
        assert_eq!(frame, [6, 7, 8, 9, 10, 11]);
    }

    #[test]
    fn a_background_that_ends_early_is_an_error_with_the_count() {
        let bytes = [0u8; 9];
        let mut source = RawFrames::new(&bytes[..]);
        let mut frame = [0u8; 6];
        source.next_frame(&mut frame).unwrap();
        let error = source.next_frame(&mut frame).unwrap_err();
        assert!(error.to_string().contains("only 1 frames"), "{error}");
        assert!(!error.is_setup());
    }
}
