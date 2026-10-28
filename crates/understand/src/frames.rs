//! Which frames to sample, how to lay them out for the model, and JPEG encoding.
//!
//! Decoding the frames is `reelctl`'s job (through `decompile`); this module only
//! plans times and arranges images it is given.

use image::RgbImage;

use crate::error::FrameError;

/// A decoded frame, already scaled to [`crate::FRAME_WIDTH`].
#[derive(Debug, Clone)]
pub struct FrameImage {
    pub time_ms: u64,
    pub image: RgbImage,
}

/// Several frames tiled into one image. `times_ms` is in reading order (rows, left to right).
#[derive(Debug, Clone)]
pub struct Sheet {
    pub times_ms: Vec<u64>,
    pub image: RgbImage,
}

/// What `--sheet` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetMode {
    Auto,
    Single,
    /// `n` by `n` tiles, with `n` of 2 or 3.
    Grid(u32),
}

/// What is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Single,
    Grid(u32),
}

/// Sample times in milliseconds, ascending and without duplicates: every `every_s`
/// seconds from 0 up to `duration_ms`, plus [`crate::FRAME_CUT_OFFSET_MS`] before
/// and after each cut. Times closer than [`crate::FRAME_MERGE_MS`] merge into the
/// earlier one, and times past the duration are dropped.
pub fn sample_times(duration_ms: u64, cut_times_ms: &[u64], every_s: u32) -> Vec<u64> {
    // Stub: the sampling plan is built in its own task.
    let _ = (duration_ms, cut_times_ms, every_s);
    Vec::new()
}

/// Tile `frames` into `cols` by `rows` sheets, in order. The last sheet may have empty tiles.
pub fn tile(frames: &[FrameImage], cols: u32, rows: u32) -> Result<Vec<Sheet>, FrameError> {
    // Stub: tiling is built in its own task.
    let _ = (frames, cols, rows);
    Err(FrameError::NoFrames)
}

/// With `Auto`, single frames when the request fits the model's context, otherwise the
/// smallest grid (2x2, then 3x3) that does; the fixed modes return themselves. A request
/// fits when `frame_count * IMAGE_TOKEN_ESTIMATE` (per sheet for a grid) plus
/// `text_tokens` plus `max_tokens` is within `context_tokens`.
pub fn choose_layout(
    mode: SheetMode,
    frame_count: usize,
    text_tokens: u64,
    max_tokens: u32,
    context_tokens: u64,
) -> Layout {
    // Stub: the layout choice is built in its own task.
    let _ = (frame_count, text_tokens, max_tokens, context_tokens);
    match mode {
        SheetMode::Grid(n) => Layout::Grid(n),
        SheetMode::Auto | SheetMode::Single => Layout::Single,
    }
}

/// Encode an image as JPEG.
pub fn encode_jpeg(image: &RgbImage) -> Result<Vec<u8>, FrameError> {
    // Stub: encoding is built in its own task.
    let _ = image;
    Err(FrameError::Encode("not implemented".to_string()))
}
