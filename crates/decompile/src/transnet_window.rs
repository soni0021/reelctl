//! TransNetV2's windowing and post-processing, as pure functions.
//!
//! No `ort` types are used here: the model is a closure from one window of
//! `100 x 27 x 48 x 3` values to 100 probabilities, so everything below runs in
//! the unit tests without a model or an ONNX Runtime library.
//!
//! The layout is the official one (`TransNetV2.predict_frames`): the first 25
//! frames are padded with copies of the first frame and the end with
//! `25 + 50 - (n % 50 or 50)` copies of the last; windows of 100 frames start
//! every 50 padded frames; the middle 50 outputs of each window are kept, which
//! are the probabilities of the original frames `[50 k, 50 k + 50)`; the result
//! is cut to `n`. A frame is a transition when its probability is above 0.5.
//! On a hard cut the model marks the last frame of the old shot, so the cut is
//! the frame after a run of marked frames: the first frame of the new shot.

use edit_dna::Cut;

use crate::error::DecompileError;
use crate::scene::{cuts_with_scores, ScoredFrame};

pub(crate) const MODEL_WIDTH: usize = 48;
pub(crate) const MODEL_HEIGHT: usize = 27;
/// Bytes in one 48x27 RGB frame.
pub(crate) const FRAME_BYTES: usize = MODEL_WIDTH * MODEL_HEIGHT * 3;
/// Frames the model takes at once.
pub(crate) const WINDOW: usize = 100;
/// Frames between the starts of two windows, and the frames kept from each.
pub(crate) const STRIDE: usize = 50;
/// Frames of context on each side of the kept part.
const CONTEXT: usize = (WINDOW - STRIDE) / 2;
/// A probability above this is a transition (strictly, as in the official code).
pub(crate) const CUT_THRESHOLD: f32 = 0.5;
/// Points read along each axis of an output pixel's cell.
pub(crate) const SUBSAMPLES: usize = 4;
/// The most frames kept for the model: 777.6 MB of 48x27 RGB (2 h 4 min at 27 fps
/// is a little over 200,000). Past it the frames are counted and dropped.
pub(crate) const MAX_MODEL_FRAMES: usize = 200_000;
/// The most frames reserved up front from the container's duration (16 MB).
pub(crate) const MAX_PREALLOCATED_FRAMES: usize = 4_096;

/// The 48x27 RGB frames of a clip, one after the other, as bytes. At most
/// `limit` frames are kept: once a clip passes it the buffer is freed and the
/// frames are only counted, so a very long clip cannot exhaust memory and the
/// detector can say how long the clip was.
pub struct ModelFrames {
    data: Vec<u8>,
    count: usize,
    limit: usize,
}

impl ModelFrames {
    /// For about `expected` frames (from the container's duration, which a
    /// damaged file can make absurd, so the reservation is capped), keeping at
    /// most `MAX_MODEL_FRAMES`. The decode loop passes its own limit to `with_limit`.
    #[cfg(test)]
    pub(crate) fn with_capacity(expected: usize) -> Self {
        Self::with_limit(expected, MAX_MODEL_FRAMES)
    }

    pub(crate) fn with_limit(expected: usize, limit: usize) -> Self {
        let reserved = expected.min(limit).min(MAX_PREALLOCATED_FRAMES);
        Self {
            data: Vec::with_capacity(reserved * FRAME_BYTES),
            count: 0,
            limit,
        }
    }

    /// Append a frame, filled in place by `fill`; past the limit, only count it.
    pub(crate) fn push_with(&mut self, fill: impl FnOnce(&mut [u8])) {
        self.count += 1;
        if self.exceeds_limit() {
            self.data = Vec::new();
            return;
        }
        let start = self.data.len();
        self.data.resize(start + FRAME_BYTES, 0);
        fill(&mut self.data[start..]);
    }

    /// Whether more frames were pushed than the limit allows (and so dropped).
    pub(crate) fn exceeds_limit(&self) -> bool {
        self.count > self.limit
    }

    pub(crate) fn limit(&self) -> usize {
        self.limit
    }

    /// The frames pushed, kept or not.
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    pub(crate) fn frame(&self, index: usize) -> Option<&[u8]> {
        let start = index.checked_mul(FRAME_BYTES)?;
        self.data.get(start..start.checked_add(FRAME_BYTES)?)
    }
}

/// Frames after padding: 25 in front and `25 + 50 - (n % 50 or 50)` behind.
/// `fill_window` pads by clamping the index instead; the tests check the two agree.
#[cfg(test)]
pub(crate) fn padded_len(frames: usize) -> usize {
    if frames == 0 {
        return 0;
    }
    let tail = match frames % STRIDE {
        0 => STRIDE,
        rest => rest,
    };
    CONTEXT + frames + (CONTEXT + STRIDE - tail)
}

/// Windows needed for `frames` frames.
pub(crate) fn window_count(frames: usize) -> usize {
    frames.div_ceil(STRIDE)
}

/// The values of window `window`: its 100 padded frames as `f32` (raw 0 to 255).
pub(crate) fn fill_window(frames: &ModelFrames, window: usize, out: &mut Vec<f32>) {
    out.clear();
    let count = frames.len();
    if count == 0 {
        return;
    }
    out.reserve(WINDOW * FRAME_BYTES);
    for position in 0..WINDOW {
        let padded = window * STRIDE + position;
        let source = padded.saturating_sub(CONTEXT).min(count - 1);
        if let Some(bytes) = frames.frame(source) {
            out.extend(bytes.iter().map(|byte| f32::from(*byte)));
        }
    }
}

/// The transition probability of every frame, from `run` applied to each window.
pub(crate) fn predict(
    frames: &ModelFrames,
    mut run: impl FnMut(&[f32]) -> Result<Vec<f32>, DecompileError>,
) -> Result<Vec<f32>, DecompileError> {
    let count = frames.len();
    let mut probabilities = Vec::with_capacity(window_count(count) * STRIDE);
    let mut window = Vec::new();
    progress::stage("transnet");
    progress::set_total("transnet", window_count(count) as u64);
    for index in 0..window_count(count) {
        if progress::cancel_requested() {
            return Err(DecompileError::Message(
                progress::CANCELLED_MESSAGE.to_string(),
            ));
        }
        fill_window(frames, index, &mut window);
        let output = run(&window)?;
        if output.len() != WINDOW {
            return Err(DecompileError::Message(format!(
                "TransNetV2 returned {} probabilities for a window of {WINDOW} frames",
                output.len()
            )));
        }
        probabilities.extend_from_slice(&output[CONTEXT..CONTEXT + STRIDE]);
        progress::tick("transnet", index as u64 + 1);
    }
    probabilities.truncate(count);
    check_probabilities(&probabilities)?;
    Ok(probabilities)
}

/// Every probability must be a number from 0 to 1 (the graph ends in a sigmoid,
/// which can reach exactly 0 or 1). A NaN would otherwise end a run of frames
/// above the threshold without being one, and an infinity would become a cut's
/// `scene_score`.
fn check_probabilities(probabilities: &[f32]) -> Result<(), DecompileError> {
    match probabilities
        .iter()
        .enumerate()
        .find(|(_, value)| !(0.0..=1.0).contains(*value))
    {
        None => Ok(()),
        Some((frame, value)) => Err(DecompileError::Message(format!(
            "TransNetV2 gave {value:?} for frame {frame}; a transition probability is a number from 0 to 1"
        ))),
    }
}

/// A maximal stretch of frames above the threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Run {
    pub first: usize,
    pub last: usize,
    pub peak: f32,
}

pub(crate) fn runs_above(probabilities: &[f32], threshold: f32) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut open: Option<Run> = None;
    for (index, &probability) in probabilities.iter().enumerate() {
        if probability > threshold {
            match open.as_mut() {
                Some(run) => {
                    run.last = index;
                    run.peak = run.peak.max(probability);
                }
                None => {
                    open = Some(Run {
                        first: index,
                        last: index,
                        peak: probability,
                    });
                }
            }
        } else if let Some(run) = open.take() {
            runs.push(run);
        }
    }
    runs.extend(open);
    runs
}

/// A cut: the first frame of the new shot, and the run's highest probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CutPoint {
    pub frame_index: usize,
    pub score: f32,
}

/// One cut after each run that has a frame before it and a frame after it.
pub(crate) fn cut_points(probabilities: &[f32], threshold: f32) -> Vec<CutPoint> {
    runs_above(probabilities, threshold)
        .into_iter()
        .filter(|run| run.first > 0 && run.last + 1 < probabilities.len())
        .map(|run| CutPoint {
            frame_index: run.last + 1,
            score: run.peak,
        })
        .collect()
}

/// The `Cut` records for these points: times and intervals from the frames, the
/// probability as `scene_score`.
pub(crate) fn cuts_at(frames: &[ScoredFrame], points: &[CutPoint]) -> Vec<Cut> {
    let scored: Vec<(usize, f64)> = points
        .iter()
        .map(|point| (point.frame_index, f64::from(point.score)))
        .collect();
    cuts_with_scores(frames, &scored)
}

/// The coordinate along one axis of sample `point` of `points`, in a frame of
/// `size` pixels: the middle of the point's equal share of the axis.
pub(crate) fn sample_coordinate(point: usize, points: usize, size: u32) -> u32 {
    let numerator = (2 * point as u64 + 1) * u64::from(size);
    let coordinate = numerator / (2 * (points.max(1) as u64));
    coordinate.min(u64::from(size.saturating_sub(1))) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` frames whose first byte is `i % 251` and whose other bytes are 0, so a
    /// fake window function can read back which frame it was given.
    fn numbered(n: usize) -> ModelFrames {
        let mut frames = ModelFrames::with_capacity(n);
        for i in 0..n {
            frames.push_with(|slot| {
                slot.fill(0);
                slot[0] = (i % 251) as u8;
            });
        }
        frames
    }

    /// A window function that answers, for each of the 100 frames, the number it
    /// carries (first byte) divided by 255, so the output says which frame sat where.
    fn echo(window: &[f32]) -> Result<Vec<f32>, DecompileError> {
        assert_eq!(window.len(), WINDOW * FRAME_BYTES);
        Ok((0..WINDOW)
            .map(|index| window[index * FRAME_BYTES] / 255.0)
            .collect())
    }

    fn echoed_numbers(n: usize) -> Vec<usize> {
        let out = predict(&numbered(n), echo).expect("predict");
        out.iter()
            .map(|value| (value * 255.0).round() as usize)
            .collect()
    }

    #[test]
    fn the_model_input_is_48_by_27_rgb() {
        assert_eq!(MODEL_WIDTH, 48);
        assert_eq!(MODEL_HEIGHT, 27);
        assert_eq!(FRAME_BYTES, 48 * 27 * 3);
    }

    #[test]
    fn padding_and_window_counts_follow_the_official_code() {
        // (frames, padded frames, windows): 25 + n + (75 - (n % 50 or 50)).
        for (n, padded, windows) in [
            (1, 100, 1),
            (49, 100, 1),
            (50, 100, 1),
            (51, 150, 2),
            (100, 150, 2),
            (101, 200, 3),
            (1000, 1050, 20),
        ] {
            assert_eq!(padded_len(n), padded, "padded length for {n} frames");
            assert_eq!(window_count(n), windows, "windows for {n} frames");
            assert_eq!((padded - WINDOW) / STRIDE + 1, windows);
        }
        assert_eq!(window_count(0), 0);
    }

    #[test]
    fn every_frame_is_covered_once_for_each_length() {
        for n in [
            1, 2, 24, 25, 26, 49, 50, 51, 74, 75, 76, 99, 100, 101, 149, 150, 151, 1000,
        ] {
            let expected: Vec<usize> = (0..n).map(|i| i % 251).collect();
            assert_eq!(echoed_numbers(n), expected, "{n} frames");
        }
    }

    #[test]
    fn the_first_window_starts_with_25_copies_of_the_first_frame() {
        let frames = numbered(120);
        let mut first = Vec::new();
        fill_window(&frames, 0, &mut first);
        assert_eq!(first.len(), WINDOW * FRAME_BYTES);
        let number = |window: &[f32], index: usize| window[index * FRAME_BYTES] as usize;
        for index in 0..25 {
            assert_eq!(number(&first, index), 0, "padding at {index}");
        }
        for index in 25..WINDOW {
            assert_eq!(number(&first, index), index - 25, "frame at {index}");
        }
    }

    #[test]
    fn the_last_window_ends_with_copies_of_the_last_frame() {
        // 120 frames: padded to 25 + 120 + 55 = 200, three windows; the last one
        // holds padded frames 100..200, which are frames 75..119 and then 55 copies of 119.
        let frames = numbered(120);
        let mut last = Vec::new();
        fill_window(&frames, 2, &mut last);
        let number = |index: usize| last[index * FRAME_BYTES] as usize;
        for index in 0..45 {
            assert_eq!(number(index), 75 + index, "frame at {index}");
        }
        for index in 45..WINDOW {
            assert_eq!(number(index), 119, "padding at {index}");
        }
    }

    #[test]
    fn samples_are_the_raw_byte_values() {
        let mut frames = ModelFrames::with_capacity(1);
        frames.push_with(|slot| slot.fill(255));
        let mut window = Vec::new();
        fill_window(&frames, 0, &mut window);
        assert!(window
            .iter()
            .all(|value| (*value - 255.0).abs() < f32::EPSILON));
    }

    #[test]
    fn a_window_function_that_returns_too_few_values_is_an_error() {
        let err = predict(&numbered(10), |_| Ok(vec![0.0; 10])).expect_err("short output");
        assert!(err.to_string().contains("100"), "{err}");
    }

    #[test]
    fn a_window_function_error_stops_the_run() {
        let mut calls = 0;
        let err = predict(&numbered(500), |_| {
            calls += 1;
            Err(DecompileError::Message("model failed".to_string()))
        })
        .expect_err("model failed");
        assert_eq!(calls, 1);
        assert!(err.to_string().contains("model failed"));
    }

    /// A window function that answers 0.01 everywhere except `value` at output `at`.
    fn with_value(at: usize, value: f32) -> impl FnMut(&[f32]) -> Result<Vec<f32>, DecompileError> {
        move |_| {
            let mut out = vec![0.01f32; WINDOW];
            out[at] = value;
            Ok(out)
        }
    }

    #[test]
    fn a_nan_in_the_middle_of_a_run_is_an_error_naming_the_frame() {
        // Output 40 of the first window is the probability of frame 15.
        let probs = vec![0.9f32; WINDOW];
        let err = predict(&numbered(100), |_| {
            let mut out = probs.clone();
            out[40] = f32::NAN;
            Ok(out)
        })
        .expect_err("NaN");
        let text = err.to_string();
        assert!(text.contains("NaN"), "{text}");
        assert!(text.contains("frame 15"), "{text}");
    }

    #[test]
    fn infinite_negative_and_too_large_probabilities_are_errors() {
        for (value, word) in [
            (f32::INFINITY, "inf"),
            (f32::NEG_INFINITY, "-inf"),
            (1e30, "1e30"),
            (-0.01, "-0.01"),
            (1.000_1, "1.0001"),
        ] {
            let err = predict(&numbered(100), with_value(30, value)).expect_err(word);
            let text = err.to_string();
            assert!(text.contains(word), "{word}: {text}");
            assert!(text.contains("frame 5"), "{word}: {text}");
            assert!(text.contains("0 to 1"), "{word}: {text}");
        }
    }

    #[test]
    fn an_all_nan_output_is_an_error_not_no_cuts() {
        let err = predict(&numbered(120), |_| Ok(vec![f32::NAN; WINDOW])).expect_err("all NaN");
        assert!(err.to_string().contains("NaN"), "{err}");
    }

    #[test]
    fn probabilities_of_exactly_zero_and_one_are_valid() {
        let out = predict(&numbered(100), |_| {
            let mut out = vec![0.0f32; WINDOW];
            out[40] = 1.0;
            Ok(out)
        })
        .expect("0 and 1 are probabilities");
        assert_eq!(out[15], 1.0);
    }

    #[test]
    fn a_bad_value_outside_the_kept_part_of_a_window_is_ignored() {
        // Outputs 0..25 and 75..100 are context the official code drops.
        predict(&numbered(100), |_| {
            let mut out = vec![0.0f32; WINDOW];
            out[3] = f32::NAN;
            out[90] = 7.0;
            Ok(out)
        })
        .expect("context outputs are not read");
    }

    #[test]
    fn a_huge_expected_frame_count_does_not_reserve_memory() {
        let frames = ModelFrames::with_capacity(usize::MAX);
        assert_eq!(frames.len(), 0);
        assert!(frames.data.capacity() <= MAX_PREALLOCATED_FRAMES * FRAME_BYTES);
    }

    #[test]
    fn the_frame_limit_is_about_780_megabytes() {
        assert_eq!(MAX_MODEL_FRAMES, 200_000);
        assert_eq!(MAX_MODEL_FRAMES * FRAME_BYTES, 777_600_000);
    }

    #[test]
    fn frames_past_the_limit_are_counted_and_not_kept() {
        let mut frames = ModelFrames::with_limit(10, 5);
        for index in 0..5u8 {
            frames.push_with(|slot| slot.fill(index));
        }
        assert!(!frames.exceeds_limit());
        assert_eq!(frames.frame(4).map(|f| f[0]), Some(4));
        frames.push_with(|slot| slot.fill(9));
        frames.push_with(|slot| slot.fill(9));
        assert!(frames.exceeds_limit());
        assert_eq!(frames.len(), 7, "every frame seen is counted");
        assert_eq!(frames.data.capacity(), 0, "the buffer is freed");
        assert!(frames.frame(0).is_none());
    }

    #[test]
    fn an_empty_clip_has_no_probabilities() {
        let out = predict(&ModelFrames::with_capacity(0), echo).expect("predict");
        assert!(out.is_empty());
    }

    #[test]
    fn a_hard_cut_is_one_frame_and_the_cut_is_the_next_frame() {
        let mut probs = vec![0.01f32; 300];
        probs[71] = 0.9649;
        let cuts = cut_points(&probs, CUT_THRESHOLD);
        assert_eq!(
            cuts,
            vec![CutPoint {
                frame_index: 72,
                score: 0.9649
            }]
        );
    }

    #[test]
    fn a_long_run_is_one_cut_after_the_run_with_the_peak_score() {
        let mut probs = vec![0.0f32; 300];
        probs[108] = 0.8372;
        probs[109] = 0.8093;
        probs[110] = 0.4;
        let cuts = cut_points(&probs, CUT_THRESHOLD);
        assert_eq!(
            cuts,
            vec![CutPoint {
                frame_index: 110,
                score: 0.8372
            }]
        );
        let runs = runs_above(&probs, CUT_THRESHOLD);
        assert_eq!(
            runs,
            vec![Run {
                first: 108,
                last: 109,
                peak: 0.8372
            }]
        );
    }

    #[test]
    fn a_probability_of_exactly_the_threshold_is_not_a_transition() {
        let mut probs = vec![0.0f32; 100];
        probs[40] = 0.5;
        assert!(cut_points(&probs, CUT_THRESHOLD).is_empty());
        probs[40] = 0.500_001;
        assert_eq!(cut_points(&probs, CUT_THRESHOLD).len(), 1);
    }

    #[test]
    fn a_run_that_starts_at_the_first_frame_gives_no_cut() {
        let mut probs = vec![0.0f32; 100];
        probs[0] = 0.9;
        probs[1] = 0.9;
        assert!(cut_points(&probs, CUT_THRESHOLD).is_empty());
        assert_eq!(runs_above(&probs, CUT_THRESHOLD).len(), 1);
    }

    #[test]
    fn a_run_that_reaches_the_last_frame_gives_no_cut() {
        let mut probs = vec![0.0f32; 100];
        probs[98] = 0.9;
        probs[99] = 0.9;
        assert!(cut_points(&probs, CUT_THRESHOLD).is_empty());
    }

    #[test]
    fn a_single_frame_clip_has_no_cut() {
        assert!(cut_points(&[0.99], CUT_THRESHOLD).is_empty());
        assert!(cut_points(&[], CUT_THRESHOLD).is_empty());
    }

    #[test]
    fn two_runs_one_frame_apart_give_two_cuts() {
        let mut probs = vec![0.0f32; 100];
        probs[10] = 0.9;
        probs[12] = 0.8;
        let cuts = cut_points(&probs, CUT_THRESHOLD);
        assert_eq!(
            cuts,
            vec![
                CutPoint {
                    frame_index: 11,
                    score: 0.9
                },
                CutPoint {
                    frame_index: 13,
                    score: 0.8
                },
            ]
        );
    }

    #[test]
    fn cuts_carry_the_frame_time_interval_and_rounded_score() {
        let frames: Vec<ScoredFrame> = (0..10)
            .map(|index| ScoredFrame {
                time_ms: index * 100,
                scene_score: 0.0,
            })
            .collect();
        let points = [
            CutPoint {
                frame_index: 3,
                score: 0.964_93,
            },
            CutPoint {
                frame_index: 7,
                score: 0.5,
            },
        ];
        let cuts = cuts_at(&frames, &points);
        assert_eq!(cuts.len(), 2);
        assert_eq!(
            (cuts[0].frame_index, cuts[0].time_ms, cuts[0].interval_ms),
            (3, 300, 300)
        );
        assert!((cuts[0].scene_score - 0.9649).abs() < 1e-9);
        assert_eq!(
            (cuts[1].frame_index, cuts[1].time_ms, cuts[1].interval_ms),
            (7, 700, 400)
        );
    }

    #[test]
    fn a_probability_track_from_windows_finds_a_cut_across_a_window_edge() {
        // A fake model that flags the frame whose number is 49, which is the last frame
        // of the first window's kept range: the cut is frame 50, the next window's first.
        let out = predict(&numbered(120), |window| {
            Ok((0..WINDOW)
                .map(|index| {
                    if window[index * FRAME_BYTES] as usize == 49 {
                        0.95
                    } else {
                        0.0
                    }
                })
                .collect())
        })
        .expect("predict");
        assert_eq!(out.len(), 120);
        assert_eq!(
            cut_points(&out, CUT_THRESHOLD),
            vec![CutPoint {
                frame_index: 50,
                score: 0.95
            }]
        );
    }

    #[test]
    fn sample_positions_are_the_middles_of_equal_shares_of_the_axis() {
        // Four points along 8 pixels: the middles of the shares [0,2) [2,4) [4,6) [6,8).
        let at = |size: u32| -> Vec<u32> {
            (0..4)
                .map(|point| sample_coordinate(point, 4, size))
                .collect()
        };
        assert_eq!(at(8), vec![1, 3, 5, 7]);
        // 10 pixels: the middles are 1.25, 3.75, 6.25 and 8.75, rounded down.
        assert_eq!(at(10), vec![1, 3, 6, 8]);
        // One point in one pixel, and one point in many: the middle.
        assert_eq!(sample_coordinate(0, 1, 1), 0);
        assert_eq!(sample_coordinate(0, 1, 9), 4);
        // As many points as pixels: each point is its own pixel.
        let own: Vec<u32> = (0..48)
            .map(|point| sample_coordinate(point, 48, 48))
            .collect();
        assert_eq!(own, (0..48).collect::<Vec<u32>>());
    }

    #[test]
    fn sample_positions_stay_inside_the_frame_and_spread_across_it() {
        for size in [10u32, 27, 48, 360, 640, 1080, 1920] {
            for cells in [MODEL_WIDTH, MODEL_HEIGHT] {
                let mut previous = 0u32;
                for point in 0..cells * SUBSAMPLES {
                    let at = sample_coordinate(point, cells * SUBSAMPLES, size);
                    assert!(at < size, "size {size}: {at}");
                    assert!(at >= previous, "size {size}: not increasing at {point}");
                    previous = at;
                }
                if size >= 360 {
                    assert!(sample_coordinate(0, cells * SUBSAMPLES, size) < size / 20);
                    assert!(previous > size - size / 20);
                }
            }
        }
    }
}
