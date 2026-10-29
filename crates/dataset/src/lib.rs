//! Instruction-tuning records from decompiled reels, and the metrics that score a
//! model's answers against them. See `docs/specs/p3-dataset-eval.md`.
//!
//! The crate is pure: it takes bytes and returns bytes, so it needs no ffmpeg and no
//! network. `reelctl dataset build` and `reelctl eval` read and write the files.

#![warn(clippy::all)]

pub mod builder;
#[cfg(test)]
mod builder_tests;
pub mod clips;
pub mod error;
pub mod eval;
#[cfg(test)]
mod eval_tests;
pub mod manifest;
pub mod metrics;
pub mod record;
pub mod sha256;
pub mod split;

pub use error::{DatasetError, MetricError};
pub use split::{Split, DEFAULT_SALT, DEFAULT_VAL_PERCENT};

/// JSON Schema of one dataset record (a line of `train.jsonl` or `val.jsonl`).
pub fn record_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(record::Record)).unwrap_or(serde_json::Value::Null)
}

/// JSON Schema of `manifest.json`.
pub fn manifest_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(manifest::Manifest))
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed schema files are what the types generate. Regenerate them with
    /// `cargo run -p dataset --example dataset_schema`.
    #[test]
    fn the_schema_files_are_up_to_date() {
        for (committed, generated) in [
            (
                include_str!("../schema/record.schema.json"),
                record_schema(),
            ),
            (
                include_str!("../schema/manifest.schema.json"),
                manifest_schema(),
            ),
        ] {
            let text = serde_json::to_string_pretty(&generated).unwrap_or_default();
            assert_eq!(committed, format!("{text}\n"));
        }
    }

    #[test]
    fn the_record_schema_names_the_fields_of_a_record() {
        let schema = record_schema();
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|object| object.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for field in [
            "dataset_schema_version",
            "id",
            "task",
            "reel_id",
            "split",
            "trainable",
            "instruction",
            "input",
            "target",
            "context",
            "provenance",
        ] {
            assert!(properties.iter().any(|name| name == field), "{field}");
        }
    }
}
