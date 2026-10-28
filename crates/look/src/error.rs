use thiserror::Error;

/// Why a cube could not be built or read.
#[derive(Debug, Error, PartialEq)]
pub enum LookError {
    /// A cube needs at least two nodes per edge.
    #[error("a LUT needs a size of at least 2, got {0}")]
    Size(usize),
    /// A `.cube` file that does not follow the format this crate reads.
    #[error("line {line}: {reason}")]
    Parse { line: usize, reason: String },
}
