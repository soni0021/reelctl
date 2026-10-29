//! Errors of the library. `reelctl` turns them into messages and exit codes.

use thiserror::Error;

/// A metric was asked for something it cannot score.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum MetricError {
    /// A NaN or infinite number reached a metric. It is refused, never scored.
    #[error("{what} is not a finite number ({value})")]
    NotFinite { what: &'static str, value: f64 },
    /// A rate whose numerator is larger than its denominator.
    #[error("a rate of {numerator} out of {denominator} is not a share")]
    NotAShare { numerator: u64, denominator: u64 },
}

/// Why `dataset build` or `eval` stopped. Each is an error exit; nothing is written.
#[derive(Debug, Error)]
pub enum DatasetError {
    #[error("{file}: not a usable {kind}: {message}")]
    Parse {
        file: String,
        kind: &'static str,
        message: String,
    },
    #[error("{file}: unknown schema_version {found} (this build reads {supported})")]
    UnknownSchemaVersion {
        file: String,
        found: String,
        supported: String,
    },
    #[error("two edit_dna files give the reel id {0:?}")]
    DuplicateReelId(String),
    #[error("--val-percent {0} is not between 0 and 50")]
    ValPercent(u32),
    #[error("the answers give id {0:?} more than once")]
    DuplicateAnswer(String),
    #[error("{file}: line {line}: {message}")]
    BadRecord {
        file: String,
        line: usize,
        message: String,
    },
    #[error("{0}")]
    Truth(String),
    #[error("could not write a record as JSON: {0}")]
    Serialize(String),
    #[error(transparent)]
    Metric(#[from] MetricError),
}
