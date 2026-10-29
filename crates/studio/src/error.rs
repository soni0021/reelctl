//! Errors of the studio core.

use std::path::Path;

pub type Result<T> = std::result::Result<T, StudioError>;

#[derive(Debug, thiserror::Error)]
pub enum StudioError {
    /// A request or an input that cannot be used (exit 2 on the command line).
    #[error("{0}")]
    Invalid(String),
    /// A file could not be read or written.
    #[error("{0}")]
    Io(String),
    /// Something stored does not read back as it was written.
    #[error("{0}")]
    Corrupt(String),
    /// The catalogue failed.
    #[error("catalogue: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl StudioError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn corrupt(message: impl Into<String>) -> Self {
        Self::Corrupt(message.into())
    }

    pub fn io(path: &Path, error: std::io::Error) -> Self {
        Self::Io(format!("{}: {error}", path.display()))
    }
}
