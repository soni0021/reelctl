//! The numbers that go to and come from the model: a frame as planar `f32`, the
//! four recurrent states, and the check on what the model returns.

use crate::error::CompositorError;

/// A four-dimensional `f32` tensor, `[batch, channels, height, width]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tensor4 {
    pub shape: [usize; 4],
    pub data: Vec<f32>,
}

impl Tensor4 {
    /// A tensor whose data fills `shape` exactly.
    pub fn new(shape: [usize; 4], data: Vec<f32>) -> Result<Self, CompositorError> {
        let expected: usize = shape.iter().product();
        if data.len() != expected {
            return Err(CompositorError::run(format!(
                "a tensor of shape {shape:?} needs {expected} values, got {}",
                data.len()
            )));
        }
        Ok(Self { shape, data })
    }

    /// The `[1, 1, 1, 1]` zero that RVM takes as a recurrent state on the first frame.
    pub fn zero() -> Self {
        Self {
            shape: [1, 1, 1, 1],
            data: vec![0.0],
        }
    }
}

/// RVM's four recurrent states (`r1`..`r4`). The first frame gets zeros; every
/// later frame gets exactly what the previous frame returned.
#[derive(Debug, Clone, PartialEq)]
pub struct RecurrentState(pub [Tensor4; 4]);

impl RecurrentState {
    pub fn initial() -> Self {
        Self([
            Tensor4::zero(),
            Tensor4::zero(),
            Tensor4::zero(),
            Tensor4::zero(),
        ])
    }
}

/// One frame for the model: RGB, planar (`[3, height, width]`), scaled to 0..1.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameRgb {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl FrameRgb {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; 3 * width * height],
        }
    }

    /// Fill from interleaved 8-bit RGB (`rgb24`, row by row): channel `c` of
    /// pixel `i` goes to plane `c` at `i`, scaled by 1/255.
    pub fn load_rgb8(&mut self, bytes: &[u8]) -> Result<(), CompositorError> {
        let pixels = self.width * self.height;
        if bytes.len() != 3 * pixels {
            return Err(CompositorError::run(format!(
                "a {}x{} frame is {} bytes of rgb24, got {}",
                self.width,
                self.height,
                3 * pixels,
                bytes.len()
            )));
        }
        for (i, pixel) in bytes.chunks_exact(3).enumerate() {
            for (channel, value) in pixel.iter().enumerate() {
                self.data[channel * pixels + i] = f32::from(*value) / 255.0;
            }
        }
        Ok(())
    }
}

/// Refuse a non-finite value and clamp the rest to 0..1. The network overshoots
/// a little (more so in float16), so a finite value outside 0..1 is not an
/// error; NaN and infinity mean the model or the input is broken. `what` and
/// `frame` name the plane and frame in the message.
pub fn clamp_unit(values: &mut [f32], what: &str, frame: usize) -> Result<(), CompositorError> {
    for (index, value) in values.iter_mut().enumerate() {
        if !value.is_finite() {
            return Err(CompositorError::run(format!(
                "the model gave a non-finite {what} ({value}) at index {index} of frame {frame}"
            )));
        }
        *value = value.clamp(0.0, 1.0);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tensor_must_fill_its_shape() {
        assert!(Tensor4::new([1, 2, 3, 4], vec![0.0; 24]).is_ok());
        let error = Tensor4::new([1, 2, 3, 4], vec![0.0; 23]).unwrap_err();
        assert!(error.to_string().contains("24"), "{error}");
        assert_eq!(Tensor4::zero().shape, [1, 1, 1, 1]);
        assert_eq!(Tensor4::zero().data, vec![0.0]);
    }

    #[test]
    fn the_first_state_is_four_scalar_zeros() {
        let state = RecurrentState::initial();
        for tensor in &state.0 {
            assert_eq!(tensor, &Tensor4::zero());
        }
    }

    #[test]
    fn rgb8_becomes_planar_and_scaled() {
        // 2x1 frame: pixel 0 is (255, 0, 51), pixel 1 is (0, 255, 102).
        let mut frame = FrameRgb::new(2, 1);
        frame.load_rgb8(&[255, 0, 51, 0, 255, 102]).unwrap();
        assert_eq!(frame.data, vec![1.0, 0.0, 0.0, 1.0, 0.2, 0.4]);
    }

    #[test]
    fn rows_are_kept_in_order() {
        // 1x2 frame: the top pixel is red, the bottom is blue.
        let mut frame = FrameRgb::new(1, 2);
        frame.load_rgb8(&[255, 0, 0, 0, 0, 255]).unwrap();
        // Planes are R, G, B, each with the top pixel first.
        assert_eq!(frame.data, vec![1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_frame_of_the_wrong_length_is_refused() {
        let mut frame = FrameRgb::new(2, 2);
        let error = frame.load_rgb8(&[0; 11]).unwrap_err();
        assert!(error.to_string().contains("12"), "{error}");
        assert!(frame.load_rgb8(&[0; 13]).is_err());
    }

    #[test]
    fn values_outside_zero_to_one_are_clamped() {
        let mut values = [-0.2, 0.0, 0.5, 1.0, 1.3];
        clamp_unit(&mut values, "alpha", 0).unwrap();
        assert_eq!(values, [0.0, 0.0, 0.5, 1.0, 1.0]);
        // Only values past the bounds change.
        let mut inside = [0.25, 0.75];
        clamp_unit(&mut inside, "alpha", 0).unwrap();
        assert_eq!(inside, [0.25, 0.75]);
    }

    #[test]
    fn a_non_finite_value_is_an_error_naming_the_plane_and_frame() {
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut values = [0.5, bad, 0.5];
            let error = clamp_unit(&mut values, "alpha", 7).unwrap_err();
            let text = error.to_string();
            assert!(text.contains("alpha"), "{text}");
            assert!(text.contains("frame 7"), "{text}");
            assert!(text.contains("index 1"), "{text}");
            assert!(!error.is_setup());
        }
    }
}
