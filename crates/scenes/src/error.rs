//! Errors of the scenes crate, split by the exit code the CLI gives them.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SceneError {
    /// The spec, an argument or a path cannot be used as given (exit 2).
    #[error("{0}")]
    Usage(String),
    /// The worker is missing, failed, timed out, or wrote something unusable (exit 1).
    #[error("{0}")]
    Worker(String),
}

impl SceneError {
    pub(crate) fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    pub(crate) fn worker(message: impl Into<String>) -> Self {
        Self::Worker(message.into())
    }
}

pub type Result<T> = std::result::Result<T, SceneError>;
