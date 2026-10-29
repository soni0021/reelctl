//! `manifest.json`: what was built, what was skipped and why, and the hashes of the
//! files. It holds reel ids and file names, never a directory.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::record::Task;
use crate::split::Split;

/// Why a record, or a whole input, was not built. The detail says which.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The reel id (the file name without `.edit_dna.json`) has characters that are not allowed.
    BadReelId,
    /// The `edit_dna.json` has a duration of 0.
    ZeroDuration,
    /// A number the record needs is NaN or infinite.
    NonFiniteValue,
    /// An `understanding.json` whose reel has no `edit_dna.json` among the inputs.
    NoMatchingEditDna,
    /// The understanding's `source.duration_ms` differs from the DNA's.
    DurationMismatch,
    /// Every clip of the run has flags, or the run has none.
    NoRuleCompliantClips,
    /// A kept clip names a segment ID that the run's segments do not have.
    InconsistentUnderstanding,
    /// A kept clip does not rebuild into a rule-compliant clip from the DNA's pauses.
    ClipsNotReproducible,
    /// The same `understanding.json` bytes were given twice.
    DuplicateUnderstanding,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BadReelId => "bad_reel_id",
            Self::ZeroDuration => "zero_duration",
            Self::NonFiniteValue => "non_finite_value",
            Self::NoMatchingEditDna => "no_matching_edit_dna",
            Self::DurationMismatch => "duration_mismatch",
            Self::NoRuleCompliantClips => "no_rule_compliant_clips",
            Self::InconsistentUnderstanding => "inconsistent_understanding",
            Self::ClipsNotReproducible => "clips_not_reproducible",
            Self::DuplicateUnderstanding => "duplicate_understanding",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Skip {
    /// A reel id, or an input file's name (never its directory).
    pub input: String,
    /// `None` when the whole input was skipped.
    pub task: Option<Task>,
    pub reason: SkipReason,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestReel {
    pub reel_id: String,
    pub split: Split,
    pub edit_dna_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileInfo {
    pub records: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestFiles {
    #[serde(rename = "train.jsonl")]
    pub train: FileInfo,
    #[serde(rename = "val.jsonl")]
    pub val: FileInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct SplitCounts {
    pub train: u64,
    pub val: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Manifest {
    pub dataset_schema_version: u32,
    pub tool_version: String,
    pub salt: String,
    pub val_percent: u32,
    pub reels: Vec<ManifestReel>,
    pub files: ManifestFiles,
    /// Per task, in task order, the records written to each split.
    pub records: BTreeMap<String, SplitCounts>,
    pub skipped: Vec<Skip>,
    /// Clips of an understanding run that were left out of the targets for having flags.
    pub flagged_clips_left_out: u64,
    pub warnings: Vec<String>,
}
