//! Reel decompiler: decode, cuts, silence, loudness, per-shot color.
//!
//! Optical-flow Bézier curves and `.cube` LUT synthesis stay unimplemented.
//! TransNetV2 runs, through `ort` and an ONNX Runtime library named in
//! `ORT_DYLIB_PATH`, when weights are given and `--cut-detector` allows it;
//! otherwise the FFmpeg scene-score detector runs behind the same trait.

#![warn(clippy::all)]

mod asr;
mod coverage;
mod decode;
mod error;
mod histogram;
mod hwdecode;
mod linear_resample;
mod loudness;
mod motion;
mod payoff;
mod pipeline;
mod resample;
mod scene;
mod silence;
mod transnet;
mod transnet_window;
mod whisper;
mod worker;

pub use decode::{decode_frames_at, DecoderSetting, SampledFrame};
pub use error::DecompileError;
pub use pipeline::{decompile_file, DecompileOutput, DecompileRequest, StageProfile};
pub use scene::{CutDetector, SCENE_SCORE_THRESHOLD};
pub use transnet::CutDetectorChoice;
pub use worker::{run_whisper_worker, WhisperRunner};
