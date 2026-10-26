//! TransNetV2 cut detector.
//!
//! The trait is the same one [`crate::scene::SceneScoreDetector`] implements.
//! ONNX weights are not in this repo, and `ort` is not linked: there is no
//! weight file to run, and P0 does not vendor the reference models. When a
//! path is configured and the file exists, detection still fails with an
//! explicit reason until an ort-backed build is wired to that file.

use std::path::{Path, PathBuf};

use edit_dna::CutDetectorKind;

use crate::error::DecompileError;
use crate::scene::{CutDetector, SceneScoreDetector, ScoredFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutDetectorChoice {
    Auto,
    Scene,
    TransNet,
}

impl CutDetectorChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Scene => "scene",
            Self::TransNet => "transnet",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TransNetV2Detector {
    weights: Option<PathBuf>,
}

impl TransNetV2Detector {
    pub fn new(weights: Option<PathBuf>) -> Self {
        Self { weights }
    }
}

impl CutDetector for TransNetV2Detector {
    fn kind(&self) -> CutDetectorKind {
        CutDetectorKind::TransNetV2
    }

    fn detect(&self, _frames: &[ScoredFrame]) -> Result<Vec<edit_dna::Cut>, DecompileError> {
        Err(DecompileError::CutDetectorUnavailable {
            reason: unavailable_reason(self.weights.as_deref()),
        })
    }
}

pub fn unavailable_reason(weights: Option<&Path>) -> String {
    match weights {
        None => "\
TransNetV2 ONNX weights were not provided. Pass --transnet-weights or set \
REELCTL_TRANSNET_WEIGHTS. Weights are not vendored (P0 does not download or \
vendor the reference repos), and the ort runtime is not linked because there \
is no offline weight file to execute. The FFmpeg scene-score detector \
implements the same CutDetector trait."
            .to_string(),
        Some(path) if !path.is_file() => format!(
            "TransNetV2 ONNX weights were not found at {}. The path is \
configurable, but the file is not on disk and is not vendored. The FFmpeg \
scene-score detector implements the same CutDetector trait.",
            path.display()
        ),
        Some(path) => format!(
            "TransNetV2 weights were found at {}, but the ort runtime is not \
linked. ONNX Runtime is not bundled: the reference weights are not part of \
this repo, and P0 does not vendor them. Rebuild with an ort-backed detector \
pointed at this file. The FFmpeg scene-score detector implements the same \
CutDetector trait and remains the active offline path.",
            path.display()
        ),
    }
}

#[derive(Debug)]
pub enum SelectedDetector {
    Scene(SceneScoreDetector),
    TransNet(TransNetV2Detector),
}

pub fn select_detector(
    choice: CutDetectorChoice,
    weights: Option<PathBuf>,
) -> Result<SelectedDetector, DecompileError> {
    match choice {
        CutDetectorChoice::Scene => Ok(SelectedDetector::Scene(SceneScoreDetector::default())),
        CutDetectorChoice::TransNet => {
            Ok(SelectedDetector::TransNet(TransNetV2Detector::new(weights)))
        }
        CutDetectorChoice::Auto => {
            if weights.as_ref().is_some_and(|path| path.is_file()) {
                Ok(SelectedDetector::TransNet(TransNetV2Detector::new(weights)))
            } else {
                Ok(SelectedDetector::Scene(SceneScoreDetector::default()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_weights_name_the_scene_score_fallback() {
        let reason = unavailable_reason(None);
        assert!(reason.contains("ort"));
        assert!(reason.contains("scene-score"));
        let err = TransNetV2Detector::new(None)
            .detect(&[])
            .expect_err("weights are absent");
        assert!(err.to_string().contains("scene-score"));
    }

    #[test]
    fn auto_without_a_file_selects_scene_score() {
        let selected = select_detector(CutDetectorChoice::Auto, None).expect("select");
        match selected {
            SelectedDetector::Scene(_) => {}
            SelectedDetector::TransNet(_) => panic!("auto should stay on scene score"),
        }
    }
}
