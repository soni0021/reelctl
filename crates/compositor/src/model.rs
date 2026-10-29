//! The model, behind a trait. The real one is an ONNX Runtime session
//! ([`crate::ort_model::RvmModel`]); tests supply fakes, so the pipeline's use of
//! the model (state feedback, ratio, frame size) is tested without one.

use crate::error::CompositorError;
use crate::tensor::{FrameRgb, RecurrentState};

/// What the model returns for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelOutput {
    /// The foreground estimate, planar `[3, height, width]`, about 0..1.
    pub fgr: Vec<f32>,
    /// The alpha matte, `[height, width]`, about 0..1.
    pub pha: Vec<f32>,
    /// The recurrent state for the next frame.
    pub state: RecurrentState,
}

/// A matting model that runs one frame at a time, carrying state from frame to frame.
pub trait MattingModel {
    /// Matte `frame` at `ratio` (the downsample ratio), given the state the
    /// previous frame returned (zeros for the first).
    fn matte(
        &mut self,
        frame: &FrameRgb,
        ratio: f32,
        state: &RecurrentState,
    ) -> Result<ModelOutput, CompositorError>;
}
