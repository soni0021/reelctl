//! Reel decompiler: decode, cuts, silence, loudness, per-shot color.
//!
//! Optical-flow Bézier curves and `.cube` LUT synthesis stay unimplemented.
//! TransNetV2 is the preferred cut detector when weights exist; otherwise the
//! FFmpeg scene-score detector runs behind the same trait.

#![warn(clippy::all)]

mod asr;
mod decode;
mod error;
mod histogram;
mod loudness;
mod payoff;
mod pipeline;
mod scene;
mod silence;
mod transnet;
mod whisper;

pub use error::DecompileError;
pub use pipeline::{decompile_file, DecompileOutput, DecompileRequest, StageProfile};
pub use scene::{CutDetector, SCENE_SCORE_THRESHOLD};
pub use transnet::CutDetectorChoice;
