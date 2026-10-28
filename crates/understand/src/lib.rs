//! Long-form video understanding for `reelctl understand`.
//!
//! `decompile` and whisper measure a video exactly but do not understand it. A
//! hosted vision-language model understands it but gets times wrong and rewrites
//! speech. This crate combines them: the model sees sampled frames and numbered
//! transcript segments and answers with segment IDs, labels, and the frame it saw
//! an event in. Every quote in the output is whisper's text, and every time is a
//! whisper, `reelctl`, or frame-sample time.
//!
//! This crate does not depend on `decompile`, ffmpeg, or whisper.cpp. Frames
//! arrive as images with their times, and `reelctl` connects the two.

#![warn(clippy::all)]

pub mod assemble;
pub mod backend;
pub mod catalog;
pub mod clips;
pub mod error;
pub mod frames;
pub mod openrouter;
pub mod request;
pub mod segments;
pub mod types;

pub use assemble::{assemble, AssembleInput};
pub use backend::{Part, RecordedBackend, VlmBackend, VlmReply, VlmRequest};
pub use catalog::{EndpointInfo, FixedCatalog, ModelCatalog};
pub use clips::{build_clips, ClipOutcome};
pub use error::{BackendError, CatalogError, FrameError};
pub use frames::{
    choose_layout, encode_jpeg, sample_times, tile, FrameImage, Layout, Sheet, SheetMode,
};
pub use openrouter::{key_from_env, OpenRouterBackend, OpenRouterCatalog, KEY_VARIABLE};
pub use request::{
    cost_upper_bound, eligible_endpoints, estimate_input_tokens, response_schema, segment_lines,
    system_prompt,
};
pub use segments::segment_words;
pub use types::{
    Beat, Clip, ClipPick, CutLabel, ModelRun, RawBeat, RawReply, RawSummary, RawVisualEvent,
    Rejected, RunSettings, Segment, Source, Summary, Understanding, VisualEvent, VisualEventKind,
    VlmLabel, SCHEMA_VERSION,
};

/// A transcript segment ends at a measured pause at least this long.
pub const SEGMENT_PAUSE_MS: u64 = 400;

/// Slack allowed on whisper's word times when matching a word boundary to a pause.
pub const SEGMENT_SLACK_MS: u64 = 150;

/// A clip starts at its hook's sentence start if that is at most this far before the hook.
pub const HOOK_BACKUP_MAX_MS: u64 = 8_000;

/// A clip's tight length (pauses cut) must be at least this long.
pub const CLIP_MIN_MS: u64 = 30_000;

/// A clip's tight length (pauses cut) must be at most this long.
pub const CLIP_MAX_MS: u64 = 60_000;

/// A pause at least this long inside a clip is dead air for the tight length.
pub const DEAD_AIR_MS: u64 = 180;

/// The payoff should land this far into the clip, measured on tight time.
pub const PAYOFF_MIN_MS: u64 = 6_000;
pub const PAYOFF_MAX_MS: u64 = 12_000;

/// A clip starts at the end of a pause just before it if the pause ends within this long.
pub const START_PAUSE_REACH_MS: u64 = 600;

/// With no pause to start in, a clip starts this long before its first word.
pub const START_LEAD_MS: u64 = 100;

/// A clip ends this far into the pause after its last word (or half the pause, if shorter).
pub const END_PAUSE_INTO_MS: u64 = 150;

/// With no pause to end in, a clip ends this long after its last word.
pub const END_TAIL_MS: u64 = 150;

/// Frames are also sampled this far before and after every `reelctl` cut.
pub const FRAME_CUT_OFFSET_MS: u64 = 300;

/// Sample times closer than this are merged.
pub const FRAME_MERGE_MS: u64 = 100;

/// Frames are scaled to this width.
pub const FRAME_WIDTH: u32 = 448;

/// The default spacing of sampled frames, in seconds.
pub const DEFAULT_EVERY_S: u32 = 5;

/// OpenRouter's pre-flight check counted about this many tokens per 448 px image.
pub const IMAGE_TOKEN_ESTIMATE: u64 = 1_445;

/// The default cap on the model's output.
pub const DEFAULT_MAX_TOKENS: u32 = 16_000;

/// JSON Schema for [`Understanding`] (`understanding.json`).
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Understanding)).unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed schema file is what the types generate. Regenerate it with
    /// `cargo run -p understand --example schema`.
    #[test]
    fn the_schema_file_is_up_to_date() {
        let committed = include_str!("../schema/understanding.schema.json");
        let generated = serde_json::to_string_pretty(&json_schema()).unwrap_or_default();
        assert_eq!(committed, format!("{generated}\n"));
    }

    #[test]
    fn the_schema_names_the_top_level_fields() {
        let schema = json_schema();
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|object| object.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for field in [
            "schema_version",
            "source",
            "model",
            "settings",
            "segments",
            "summary",
            "beats",
            "visual_events",
            "cuts",
            "clips",
            "rejected",
        ] {
            assert!(properties.iter().any(|name| name == field), "{field}");
        }
    }
}
