//! Which frames to sample, how to lay them out for the model, and JPEG encoding.
//!
//! Decoding the frames is `reelctl`'s job (through `decompile`); this module only
//! plans times and arranges images it is given.

use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, RgbImage};

use crate::error::FrameError;
use crate::{FRAME_CUT_OFFSET_MS, FRAME_MERGE_MS, FRAME_WIDTH, IMAGE_TOKEN_ESTIMATE};

const JPEG_QUALITY: u8 = 80;

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
/// earlier one, and times past the duration are dropped. A cut within 300 ms of the
/// start has no frame before it. An `every_s` of 0 turns the regular grid off, leaving
/// only the frames around the cuts.
pub fn sample_times(duration_ms: u64, cut_times_ms: &[u64], every_s: u32) -> Vec<u64> {
    let mut times = Vec::new();
    if every_s > 0 {
        let step = u64::from(every_s) * 1_000;
        let mut t = 0;
        while t <= duration_ms {
            times.push(t);
            match t.checked_add(step) {
                Some(next) => t = next,
                None => break,
            }
        }
    }
    for &cut in cut_times_ms {
        if let Some(before) = cut.checked_sub(FRAME_CUT_OFFSET_MS) {
            times.push(before);
        }
        times.push(cut.saturating_add(FRAME_CUT_OFFSET_MS));
    }
    times.retain(|&t| t <= duration_ms);
    times.sort_unstable();
    times.dedup();

    // Each time is measured from the last one kept, so a chain of close times
    // cannot drag the frames away from where they were asked for.
    let mut merged: Vec<u64> = Vec::with_capacity(times.len());
    for t in times {
        if merged.last().is_none_or(|&last| t - last >= FRAME_MERGE_MS) {
            merged.push(t);
        }
    }
    merged
}

/// Tile `frames` into `cols` by `rows` sheets, in order. The last sheet may have empty tiles.
/// Every sheet has the same size: `cols` tiles of [`crate::FRAME_WIDTH`] across and `rows`
/// tiles of the tallest frame down. A frame wider than its tile is cropped.
pub fn tile(frames: &[FrameImage], cols: u32, rows: u32) -> Result<Vec<Sheet>, FrameError> {
    if frames.is_empty() || cols == 0 || rows == 0 {
        return Err(FrameError::NoFrames);
    }
    let tile_height = frames.iter().map(|f| f.image.height()).max().unwrap_or(0);
    // JPEG cannot hold a side longer than 65,535 pixels.
    let side = |tile: u32, count: u32| {
        tile.checked_mul(count)
            .filter(|&px| px <= u32::from(u16::MAX))
            .ok_or_else(|| FrameError::Encode(format!("a {cols}x{rows} sheet is too large")))
    };
    let width = side(FRAME_WIDTH, cols)?;
    let height = side(tile_height, rows)?;
    let per_row = cols as usize;
    let per_sheet = per_row.saturating_mul(rows as usize);

    let sheets = frames
        .chunks(per_sheet)
        .map(|chunk| {
            let mut image = RgbImage::new(width, height);
            for (row, line) in chunk.chunks(per_row).enumerate() {
                for (col, frame) in line.iter().enumerate() {
                    // `col < cols` and `row < rows`, so these fit in a `u32`.
                    let (x0, y0) = (col as u32 * FRAME_WIDTH, row as u32 * tile_height);
                    for (x, y, pixel) in frame.image.enumerate_pixels() {
                        if x < FRAME_WIDTH {
                            image.put_pixel(x0 + x, y0 + y, *pixel);
                        }
                    }
                }
            }
            Sheet {
                times_ms: chunk.iter().map(|f| f.time_ms).collect(),
                image,
            }
        })
        .collect();
    Ok(sheets)
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
    // An overflow means the request is far past any context, so it does not fit.
    let fits = |images: usize| {
        u64::try_from(images)
            .ok()
            .and_then(|n| n.checked_mul(IMAGE_TOKEN_ESTIMATE))
            .and_then(|tokens| tokens.checked_add(text_tokens))
            .and_then(|tokens| tokens.checked_add(u64::from(max_tokens)))
            .is_some_and(|total| total <= context_tokens)
    };
    match mode {
        SheetMode::Single => Layout::Single,
        SheetMode::Grid(n) => Layout::Grid(n),
        SheetMode::Auto if fits(frame_count) => Layout::Single,
        SheetMode::Auto if fits(frame_count.div_ceil(4)) => Layout::Grid(2),
        SheetMode::Auto => Layout::Grid(3),
    }
}

/// Encode an image as JPEG.
pub fn encode_jpeg(image: &RgbImage) -> Result<Vec<u8>, FrameError> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return Err(FrameError::Encode("the image is empty".to_string()));
    }
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode(image.as_raw(), width, height, ExtendedColorType::Rgb8)
        .map_err(|e| FrameError::Encode(e.to_string()))?;
    Ok(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FRAME_WIDTH, IMAGE_TOKEN_ESTIMATE};
    use image::Rgb;

    fn frame(time_ms: u64, height: u32, shade: u8) -> FrameImage {
        FrameImage {
            time_ms,
            image: RgbImage::from_pixel(FRAME_WIDTH, height, Rgb([shade, shade, shade])),
        }
    }

    #[test]
    fn samples_every_n_seconds_from_zero_through_the_duration() {
        assert_eq!(sample_times(12_000, &[], 5), vec![0, 5_000, 10_000]);
        // The duration itself is not past the duration.
        assert_eq!(sample_times(10_000, &[], 5), vec![0, 5_000, 10_000]);
    }

    #[test]
    fn samples_before_and_after_each_cut() {
        assert_eq!(
            sample_times(10_000, &[7_000], 5),
            vec![0, 5_000, 6_700, 7_300, 10_000]
        );
    }

    #[test]
    fn output_is_ascending_without_duplicates_for_unsorted_cuts() {
        let times = sample_times(20_000, &[12_000, 3_000, 12_000], 5);
        assert_eq!(
            times,
            vec![0, 2_700, 3_300, 5_000, 10_000, 11_700, 12_300, 15_000, 20_000]
        );
    }

    #[test]
    fn times_merge_into_the_earlier_one_only_when_closer_than_the_window() {
        // A cut at 5_250 samples 4_950; the grid's 5_000 is 50 ms later and folds into it.
        assert_eq!(
            sample_times(10_000, &[5_250], 5),
            vec![0, 4_950, 5_550, 10_000]
        );
        // 4_901 and 5_000 are 99 ms apart: they merge into the earlier.
        assert_eq!(
            sample_times(10_000, &[5_201], 5),
            vec![0, 4_901, 5_501, 10_000]
        );
        // 4_900 and 5_000 are exactly 100 ms apart: both stay.
        assert_eq!(
            sample_times(10_000, &[5_200], 5),
            vec![0, 4_900, 5_000, 5_500, 10_000]
        );
        // A cut at 4_699 samples 4_999, and the grid's 5_000 folds into it.
        assert_eq!(
            sample_times(10_000, &[4_699], 5),
            vec![0, 4_399, 4_999, 10_000]
        );
    }

    #[test]
    fn a_chain_of_close_times_measures_from_the_one_kept() {
        // 1_000, 1_080 and 1_160 are each 80 ms apart; 1_160 is 160 from the kept 1_000.
        // The cuts' later samples (1_600, 1_680, 1_760) chain the same way.
        assert_eq!(
            sample_times(5_000, &[1_300, 1_380, 1_460], 5),
            vec![0, 1_000, 1_160, 1_600, 1_760, 5_000]
        );
    }

    #[test]
    fn times_before_zero_are_dropped_not_clamped() {
        assert_eq!(sample_times(10_000, &[200], 5), vec![0, 500, 5_000, 10_000]);
        assert_eq!(sample_times(10_000, &[0], 5), vec![0, 300, 5_000, 10_000]);
        // Without the grid's 0, a clamped frame would show up as a 0.
        assert_eq!(sample_times(10_000, &[200], 0), vec![500]);
        assert_eq!(sample_times(10_000, &[0], 0), vec![300]);
    }

    #[test]
    fn times_past_the_duration_are_dropped() {
        let times = sample_times(10_000, &[9_800, 11_000, u64::MAX], 5);
        assert!(times.iter().all(|&t| t <= 10_000), "{times:?}");
        assert_eq!(times, vec![0, 5_000, 9_500, 10_000]);
        assert_eq!(
            sample_times(10_000, &[10_000], 5),
            vec![0, 5_000, 9_700, 10_000]
        );
    }

    #[test]
    fn a_zero_duration_samples_only_time_zero() {
        assert_eq!(sample_times(0, &[], 5), vec![0]);
        assert_eq!(sample_times(0, &[0, 500], 5), vec![0]);
    }

    #[test]
    fn an_every_s_of_zero_samples_only_around_the_cuts() {
        assert_eq!(sample_times(10_000, &[], 0), Vec::<u64>::new());
        assert_eq!(sample_times(10_000, &[5_000], 0), vec![4_700, 5_300]);
    }

    #[test]
    fn a_huge_every_s_does_not_overflow() {
        assert_eq!(sample_times(10_000, &[], u32::MAX), vec![0]);
    }

    #[test]
    fn tiles_in_order_with_times_in_reading_order() {
        let frames: Vec<FrameImage> = (0..5u8)
            .map(|i| frame(u64::from(i) * 1_000, 252, 10 * (i + 1)))
            .collect();
        let sheets = tile(&frames, 2, 2).expect("tiles");
        assert_eq!(sheets.len(), 2);
        assert_eq!(sheets[0].times_ms, vec![0, 1_000, 2_000, 3_000]);
        assert_eq!(sheets[1].times_ms, vec![4_000]);
        assert_eq!(sheets[0].image.dimensions(), (2 * FRAME_WIDTH, 2 * 252));
        // Reading order: top-left, top-right, bottom-left, bottom-right.
        assert_eq!(sheets[0].image.get_pixel(0, 0).0, [10, 10, 10]);
        assert_eq!(sheets[0].image.get_pixel(FRAME_WIDTH, 0).0, [20, 20, 20]);
        assert_eq!(sheets[0].image.get_pixel(0, 252).0, [30, 30, 30]);
        assert_eq!(sheets[0].image.get_pixel(FRAME_WIDTH, 252).0, [40, 40, 40]);
    }

    #[test]
    fn the_last_sheet_has_empty_tiles_and_the_same_size() {
        let frames: Vec<FrameImage> = (0..5).map(|i| frame(i * 1_000, 252, 200)).collect();
        let sheets = tile(&frames, 2, 2).expect("tiles");
        let last = &sheets[1];
        assert_eq!(last.image.dimensions(), sheets[0].image.dimensions());
        assert_eq!(last.image.get_pixel(0, 0).0, [200, 200, 200]);
        assert_eq!(
            last.image.get_pixel(FRAME_WIDTH, 0).0,
            [0, 0, 0],
            "empty tile is black"
        );
        assert_eq!(last.image.get_pixel(FRAME_WIDTH, 252).0, [0, 0, 0]);
    }

    #[test]
    fn a_full_last_sheet_is_not_followed_by_an_empty_one() {
        let frames: Vec<FrameImage> = (0..9).map(|i| frame(i, 100, 50)).collect();
        let sheets = tile(&frames, 3, 3).expect("tiles");
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].times_ms.len(), 9);
        assert_eq!(sheets[0].image.dimensions(), (3 * FRAME_WIDTH, 300));
    }

    #[test]
    fn a_non_square_grid_tiles_by_columns_then_rows() {
        let frames: Vec<FrameImage> = (0..3u8)
            .map(|i| frame(u64::from(i), 100, 10 * (i + 1)))
            .collect();
        let sheets = tile(&frames, 3, 1).expect("tiles");
        assert_eq!(sheets.len(), 1);
        assert_eq!(sheets[0].image.dimensions(), (3 * FRAME_WIDTH, 100));
        assert_eq!(
            sheets[0].image.get_pixel(2 * FRAME_WIDTH, 0).0,
            [30, 30, 30]
        );
    }

    #[test]
    fn tile_with_no_frames_or_a_zero_grid_is_an_error() {
        assert!(matches!(tile(&[], 2, 2), Err(FrameError::NoFrames)));
        let frames = vec![frame(0, 100, 1)];
        assert!(matches!(tile(&frames, 0, 2), Err(FrameError::NoFrames)));
        assert!(matches!(tile(&frames, 2, 0), Err(FrameError::NoFrames)));
    }

    #[test]
    fn a_frame_wider_or_taller_than_its_tile_is_cropped_not_a_panic() {
        let wide = FrameImage {
            time_ms: 0,
            image: RgbImage::from_pixel(FRAME_WIDTH + 50, 300, Rgb([9, 9, 9])),
        };
        let short = frame(1, 100, 7);
        let sheets = tile(&[wide, short], 2, 1).expect("tiles");
        assert_eq!(sheets[0].image.dimensions(), (2 * FRAME_WIDTH, 300));
        assert_eq!(
            sheets[0].image.get_pixel(FRAME_WIDTH, 0).0,
            [7, 7, 7],
            "the neighbour is not overwritten"
        );
    }

    #[test]
    fn encodes_a_jpeg_that_decodes_to_the_same_size() {
        let image = RgbImage::from_fn(64, 48, |x, y| Rgb([(x * 4) as u8, (y * 5) as u8, 128]));
        let bytes = encode_jpeg(&image).expect("encodes");
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "JPEG magic");
        let back =
            image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).expect("decodes");
        assert_eq!((back.width(), back.height()), (64, 48));
    }

    #[test]
    fn encoding_an_empty_image_is_an_error() {
        assert!(matches!(
            encode_jpeg(&RgbImage::new(0, 0)),
            Err(FrameError::Encode(_))
        ));
    }

    const CONTEXT: u64 = 262_144;
    const TEXT: u64 = 30_000;
    const MAX: u32 = 16_000;

    #[test]
    fn auto_keeps_single_frames_when_they_fit() {
        assert_eq!(
            choose_layout(SheetMode::Auto, 100, TEXT, MAX, CONTEXT),
            Layout::Single
        );
    }

    #[test]
    fn auto_picks_2x2_for_the_lecture_case() {
        // 344 single frames are 497,080 image tokens: over a 262,144-token context.
        assert_eq!(
            choose_layout(SheetMode::Auto, 344, TEXT, MAX, CONTEXT),
            Layout::Grid(2)
        );
        // 86 sheets fit: 86 * 1,445 + 30,000 + 16,000 = 170,270.
        assert!(86 * IMAGE_TOKEN_ESTIMATE + TEXT + u64::from(MAX) <= CONTEXT);
    }

    #[test]
    fn auto_picks_3x3_when_2x2_does_not_fit_either() {
        assert_eq!(
            choose_layout(SheetMode::Auto, 1_000, TEXT, MAX, CONTEXT),
            Layout::Grid(3)
        );
    }

    #[test]
    fn auto_boundaries_are_inclusive() {
        let exact = 10 * IMAGE_TOKEN_ESTIMATE + TEXT + u64::from(MAX);
        assert_eq!(
            choose_layout(SheetMode::Auto, 10, TEXT, MAX, exact),
            Layout::Single
        );
        assert_eq!(
            choose_layout(SheetMode::Auto, 10, TEXT, MAX, exact - 1),
            Layout::Grid(2)
        );
        // 2x2 of 10 frames is 3 sheets.
        let grid2 = 3 * IMAGE_TOKEN_ESTIMATE + TEXT + u64::from(MAX);
        assert_eq!(
            choose_layout(SheetMode::Auto, 10, TEXT, MAX, grid2),
            Layout::Grid(2)
        );
        assert_eq!(
            choose_layout(SheetMode::Auto, 10, TEXT, MAX, grid2 - 1),
            Layout::Grid(3)
        );
    }

    #[test]
    fn auto_rounds_the_sheet_count_up() {
        // 5 frames make 2 sheets of 2x2, not 1.
        let two_sheets = 2 * IMAGE_TOKEN_ESTIMATE + TEXT + u64::from(MAX);
        assert_eq!(
            choose_layout(SheetMode::Auto, 5, TEXT, MAX, two_sheets),
            Layout::Grid(2)
        );
        assert_eq!(
            choose_layout(SheetMode::Auto, 5, TEXT, MAX, two_sheets - 1),
            Layout::Grid(3)
        );
    }

    #[test]
    fn fixed_modes_return_themselves() {
        assert_eq!(
            choose_layout(SheetMode::Single, 10_000, TEXT, MAX, 1),
            Layout::Single
        );
        assert_eq!(
            choose_layout(SheetMode::Grid(2), 1, TEXT, MAX, u64::MAX),
            Layout::Grid(2)
        );
        assert_eq!(
            choose_layout(SheetMode::Grid(3), 1, TEXT, MAX, u64::MAX),
            Layout::Grid(3)
        );
    }

    #[test]
    fn auto_does_not_overflow() {
        assert_eq!(
            choose_layout(SheetMode::Auto, usize::MAX, u64::MAX, u32::MAX, u64::MAX),
            Layout::Grid(3)
        );
        assert_eq!(
            choose_layout(SheetMode::Auto, usize::MAX, 0, 0, u64::MAX),
            Layout::Grid(3)
        );
    }

    #[test]
    fn auto_with_no_frames_and_a_text_that_fits_is_single() {
        assert_eq!(
            choose_layout(SheetMode::Auto, 0, TEXT, MAX, CONTEXT),
            Layout::Single
        );
    }
}
