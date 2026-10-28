//! FFmpeg scene-score cut detector.
//!
//! The score is the libavfilter `select`/`scene` formula: mean absolute
//! difference of luma, then `clip(min(mafd, |mafd - prev_mafd|) / 100)`.
//! The decompiler evaluates it on a 160×90 grid. On a flat field that grid
//! matches a full-frame score, and it keeps the 60-second path inside the
//! structural budget.

use edit_dna::{Cut, CutDetectorKind};

use crate::error::DecompileError;
use crate::transnet_window::ModelFrames;

/// Same default as `select='gt(scene,0.3)'`.
pub const SCENE_SCORE_THRESHOLD: f64 = 0.30;

pub const SCENE_NOTE: &str = "\
FFmpeg scene score (libavfilter select/scene mafd formula) on a 160x90 luma \
grid. A hard cut is a frame whose score is greater than 0.30. A dissolve or \
fade is not a cut to this detector. TransNetV2 runs instead when \
--transnet-weights names an ONNX file and ORT_DYLIB_PATH names an ONNX \
Runtime library; neither is vendored.";

#[derive(Debug, Clone, Copy)]
pub struct ScoredFrame {
    pub time_ms: u64,
    pub scene_score: f64,
}

/// What a detector may read: the per-frame scores, and, when the model runs,
/// the 48x27 RGB frames.
pub struct DetectorInput<'a> {
    pub frames: &'a [ScoredFrame],
    pub model_frames: Option<&'a ModelFrames>,
}

impl<'a> DetectorInput<'a> {
    /// Scores only, as the scene-score detector needs.
    pub fn scores(frames: &'a [ScoredFrame]) -> Self {
        Self {
            frames,
            model_frames: None,
        }
    }
}

pub trait CutDetector {
    fn kind(&self) -> CutDetectorKind;
    fn detect(&self, input: &DetectorInput<'_>) -> Result<Vec<Cut>, DecompileError>;
}

#[derive(Debug, Clone, Copy)]
pub struct SceneScoreDetector {
    pub threshold: f64,
}

impl Default for SceneScoreDetector {
    fn default() -> Self {
        Self {
            threshold: SCENE_SCORE_THRESHOLD,
        }
    }
}

impl CutDetector for SceneScoreDetector {
    fn kind(&self) -> CutDetectorKind {
        CutDetectorKind::FfmpegSceneScore
    }

    fn detect(&self, input: &DetectorInput<'_>) -> Result<Vec<Cut>, DecompileError> {
        let frames = input.frames;
        let mut indexes = Vec::new();
        for (index, frame) in frames.iter().enumerate() {
            if index > 0 && frame.scene_score > self.threshold {
                indexes.push(index);
            }
        }
        Ok(finalize_cuts(frames, &indexes))
    }
}

pub fn finalize_cuts(frames: &[ScoredFrame], indexes: &[usize]) -> Vec<Cut> {
    let scored: Vec<(usize, f64)> = indexes
        .iter()
        .map(|&index| {
            let score = frames.get(index).map(|item| item.scene_score);
            (index, score.unwrap_or(0.0))
        })
        .collect();
    cuts_with_scores(frames, &scored)
}

/// The cuts at these frames with these scores; times and intervals come from `frames`.
pub fn cuts_with_scores(frames: &[ScoredFrame], cuts_at: &[(usize, f64)]) -> Vec<Cut> {
    let mut cuts = Vec::with_capacity(cuts_at.len());
    let mut previous_ms = 0u64;
    for &(index, scene_score) in cuts_at {
        let time_ms = frames.get(index).map(|item| item.time_ms).unwrap_or(0);
        cuts.push(Cut {
            frame_index: index as u64,
            time_ms,
            interval_ms: time_ms.saturating_sub(previous_ms),
            scene_score: round4(scene_score),
        });
        previous_ms = time_ms;
    }
    cuts
}

pub fn ffmpeg_scene_score(mafd: f64, previous_mafd: f64) -> f64 {
    let diff = (mafd - previous_mafd).abs();
    (mafd.min(diff) / 100.0).clamp(0.0, 1.0)
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_pair_scores_zero_and_hard_jump_saturates() {
        assert!(ffmpeg_scene_score(0.0, 0.0).abs() < 1e-9);
        assert!((ffmpeg_scene_score(150.0, 0.0) - 1.0).abs() < 1e-9);
        assert!(ffmpeg_scene_score(150.0, 0.0) > SCENE_SCORE_THRESHOLD);
        assert!(ffmpeg_scene_score(0.0, 150.0).abs() < 1e-9);
    }

    #[test]
    fn detector_keeps_only_frames_above_the_threshold() {
        let frames = [
            ScoredFrame {
                time_ms: 0,
                scene_score: 0.0,
            },
            ScoredFrame {
                time_ms: 3_000,
                scene_score: 0.92,
            },
            ScoredFrame {
                time_ms: 3_033,
                scene_score: 0.01,
            },
            ScoredFrame {
                time_ms: 9_000,
                scene_score: 0.8,
            },
        ];
        let cuts = SceneScoreDetector::default()
            .detect(&DetectorInput::scores(&frames))
            .expect("cuts");
        assert_eq!(cuts.len(), 2);
        assert_eq!(cuts[0].frame_index, 1);
        assert_eq!(cuts[0].time_ms, 3_000);
        assert_eq!(cuts[0].interval_ms, 3_000);
        assert_eq!(cuts[1].frame_index, 3);
        assert_eq!(cuts[1].interval_ms, 6_000);
        assert_eq!(
            SceneScoreDetector::default().kind(),
            CutDetectorKind::FfmpegSceneScore
        );
    }
}
