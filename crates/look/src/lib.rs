//! Colour matching for `reelctl look`: per-channel histograms of RGB pixels, a
//! monotonic curve per channel that moves one distribution onto another, an n^3
//! `.cube` LUT built from the three curves, and a writer and reader for the
//! Adobe/Resolve `.cube` text format.
//!
//! Everything here is pure: no decoding, no files, no clock. The matching is
//! separable (each channel on its own), so the cube can tone-map and shift a colour
//! cast and cannot reproduce a look that mixes channels.

#![warn(clippy::all)]

mod cube;
mod curve;
mod error;
mod histogram;
mod times;

pub use cube::{Cube, CUBE_SIZES};
pub use curve::{identity_curve, match_curve, Curve};
pub use error::LookError;
pub use histogram::{Histogram, Histograms, LEVELS};
pub use times::sample_times_ms;
