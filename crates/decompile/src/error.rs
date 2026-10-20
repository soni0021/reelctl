use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecompileError {
    #[error("ffmpeg: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("no video stream in {0}")]
    NoVideo(String),
    #[error("decoded zero video frames from {0}")]
    NoFrames(String),
    #[error("{reason}")]
    CutDetectorUnavailable { reason: String },
    #[error("whisper: {0}")]
    Whisper(String),
    #[error("{0}")]
    Message(String),
}
