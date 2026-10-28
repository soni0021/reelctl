//! Errors of the library. `reelctl` turns them into messages and exit codes.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    /// No endpoint of the model satisfies the provider settings.
    #[error("no provider for {model} satisfies the settings: {reason}")]
    NoProvider { model: String, reason: String },
    #[error("request failed: {0}")]
    Http(String),
    #[error("the backend answered with status {status}: {body}")]
    Status { status: u16, body: String },
    /// The reply was not the JSON the schema asks for.
    #[error("unusable reply: {0}")]
    BadReply(String),
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("unknown model {0}")]
    UnknownModel(String),
    #[error("could not read the model listing: {0}")]
    Listing(String),
}

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("no frames to tile")]
    NoFrames,
    #[error("could not encode a JPEG: {0}")]
    Encode(String),
}
