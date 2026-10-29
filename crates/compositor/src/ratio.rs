//! RVM's downsample ratio: the network sees the frame shrunk by this factor, so
//! a smaller ratio is faster and sees less detail. It is fixed for a clip because
//! the recurrent state's shape depends on it.

use crate::error::CompositorError;

/// The longest side, in pixels, that RVM's own `auto_downsample_ratio` aims for.
pub const TARGET_SIDE: f32 = 512.0;

/// `min(512 / max(width, height), 1)`: RVM's rule (`inference.py`). A 1080p frame
/// gives 0.2667, and a frame of 512 pixels or fewer on its longest side gives 1.
pub fn auto_downsample_ratio(width: u32, height: u32) -> f32 {
    let longest = width.max(height).max(1) as f32;
    (TARGET_SIDE / longest).min(1.0)
}

/// A ratio the user gave: finite, above 0 and at most 1.
pub fn validate_ratio(ratio: f32) -> Result<f32, CompositorError> {
    if ratio.is_finite() && ratio > 0.0 && ratio <= 1.0 {
        Ok(ratio)
    } else {
        Err(CompositorError::setup(format!(
            "--downsample-ratio {ratio} is not usable: it must be above 0 and at most 1 \
(1920x1080 uses {:.4} by default)",
            auto_downsample_ratio(1920, 1080)
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn the_ratio_aims_for_a_longest_side_of_512() {
        assert!(close(auto_downsample_ratio(1920, 1080), 512.0 / 1920.0));
        // Portrait: the longest side is the height.
        assert!(close(auto_downsample_ratio(1080, 1920), 512.0 / 1920.0));
        assert!(close(auto_downsample_ratio(3840, 2160), 512.0 / 3840.0));
        assert!(close(auto_downsample_ratio(640, 360), 0.8));
        assert!(close(auto_downsample_ratio(1000, 563), 0.512));
    }

    #[test]
    fn a_small_frame_is_not_enlarged() {
        assert!(close(auto_downsample_ratio(512, 512), 1.0));
        assert!(close(auto_downsample_ratio(300, 200), 1.0));
        assert!(close(auto_downsample_ratio(513, 100), 512.0 / 513.0));
    }

    #[test]
    fn a_zero_size_does_not_divide_by_zero() {
        assert!(close(auto_downsample_ratio(0, 0), 1.0));
    }

    #[test]
    fn a_given_ratio_must_be_in_zero_to_one() {
        assert!(close(validate_ratio(0.25).unwrap(), 0.25));
        assert!(close(validate_ratio(1.0).unwrap(), 1.0));
        assert!(close(validate_ratio(0.001).unwrap(), 0.001));
        for bad in [0.0, -0.5, 1.0001, 2.0, f32::NAN, f32::INFINITY] {
            let error = validate_ratio(bad).unwrap_err();
            assert!(error.is_setup(), "{bad}");
            assert!(error.to_string().contains("--downsample-ratio"), "{bad}");
        }
    }
}
