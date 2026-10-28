//! Errors of the timeline crate. Every one is a problem with the project file or
//! the arguments, so the CLI reports them as usage errors (exit 2).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TimelineError {
    /// The project, the DNA, or an argument cannot be used as given.
    #[error("{0}")]
    Invalid(String),
    /// The text is not JSON of the expected shape.
    #[error("invalid OTIO JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl TimelineError {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

pub type Result<T> = std::result::Result<T, TimelineError>;
