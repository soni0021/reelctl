//! Cut a presenter out of a video with RobustVideoMatting (RVM) and write the
//! presenter over a background, the alpha matte alone, or the presenter with alpha.
//!
//! The crate is a pure core and two edges. The core ([`ratio`], [`tensor`],
//! [`composite`], [`background`], [`pipeline`], [`model`]) turns raw frames into
//! raw frames with a [`MattingModel`] behind a trait, so tests use a fake. The
//! edges are [`ort_model`] (the ONNX Runtime session, loaded from
//! `ORT_DYLIB_PATH` at run time) and [`media`] (the `ffprobe` and `ffmpeg`
//! command line tools that decode and encode). No `unsafe` code, no network.

#![forbid(unsafe_code)]
#![warn(clippy::all)]

pub mod background;
pub mod composite;
pub mod error;
pub mod media;
pub mod model;
pub mod ort_model;
pub mod pipeline;
pub mod ratio;
pub mod tensor;

pub use background::{parse_hex_colour, Background, RawFrames, SolidColor};
pub use composite::OutputMode;
pub use error::CompositorError;
pub use media::{
    isolate_presenter, prepare, BackgroundSpec, IsolateReport, IsolateRequest, Prepared, VideoInfo,
};
pub use model::{MattingModel, ModelOutput};
pub use ort_model::{release_runtime, RvmModel, LIBRARY_ENV};
pub use pipeline::{isolate, Options, Stats};
pub use ratio::{auto_downsample_ratio, validate_ratio};
pub use tensor::{FrameRgb, RecurrentState, Tensor4};
