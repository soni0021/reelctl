//! The types shared by every module: transcript segments, what the model
//! returns (by segment ID), and `understanding.json`.

use edit_dna::SilenceGap;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `understanding.json`'s `schema_version`.
pub const SCHEMA_VERSION: u32 = 1;

/// A run of whisper words split at sentence ends or measured pauses. `id` is 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Segment {
    pub id: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Whisper's exact words, joined by single spaces.
    pub text: String,
    /// The text ends with `.`, `?` or `!`. A sentence starts at segment 1 and after
    /// every segment with this set.
    pub ends_sentence: bool,
}

/// The editing events the model may name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualEventKind {
    HardCut,
    PunchIn,
    ZoomOut,
    OverlayOn,
    OverlayOff,
    BRollOn,
    BRollOff,
    SlideChange,
    CaptionChange,
}

// What the model returns. Boundaries are segment IDs; the one time it gives
// (`t_s` of an event) is snapped to a sampled frame time before it is stored.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawSummary {
    pub topic: String,
    pub language: String,
    pub speaker_count: u32,
    pub visual_formats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawBeat {
    pub start_segment: u32,
    pub end_segment: u32,
    pub kind: String,
    pub summary: String,
    pub visual: String,
    pub insight_density: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawVisualEvent {
    /// Seconds: the time of the first frame that shows the change.
    pub t_s: f64,
    pub kind: VisualEventKind,
    pub note: String,
}

/// One clip the model proposes, by segment ID, best first in `RawReply::reel_candidates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipPick {
    pub start_segment: u32,
    pub end_segment: u32,
    pub hook_segment: u32,
    pub payoff_segment: u32,
    pub cutoff_segment: u32,
    pub title: String,
    pub open_loop: String,
    pub visual_suggestion: String,
    pub why: String,
}

/// The model's whole answer, as the strict schema asks for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RawReply {
    pub video_summary: RawSummary,
    pub beats: Vec<RawBeat>,
    pub edit_events: Vec<RawVisualEvent>,
    pub reel_candidates: Vec<ClipPick>,
}

// `understanding.json`.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Source {
    pub path: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRun {
    pub id: String,
    pub provider: String,
    pub quantization: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// The cost the response reports, when it does.
    pub cost_usd: Option<f64>,
    /// An upper bound computed before the call; it overestimates image tokens.
    pub cost_upper_bound_usd: f64,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RunSettings {
    pub every_s: u32,
    /// `single`, `2x2`, or `3x3`: what was sent.
    pub sheet: String,
    pub segment_pause_ms: u64,
    pub quant: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub topic: String,
    pub language: String,
    pub speaker_count: u32,
    pub visual_formats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Beat {
    pub start_segment: u32,
    pub end_segment: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub kind: String,
    pub summary: String,
    pub visual: String,
    pub insight_density: u8,
}

/// An event the model saw. `seen_at_ms` is a sampled frame time: the event
/// happened at or before it, and after the previous frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VisualEvent {
    pub seen_at_ms: u64,
    pub kind: VisualEventKind,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VlmLabel {
    pub kind: VisualEventKind,
    pub note: String,
}

/// A `reelctl` cut and what the model saw there, if anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CutLabel {
    pub frame_index: u64,
    pub time_ms: u64,
    pub vlm_label: Option<VlmLabel>,
}

/// A segment's ID and its exact text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SegmentRef {
    pub id: u32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Clip {
    /// The model's ranking, 1 = best. Rejected picks keep their rank too.
    pub rank: u32,
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
    pub length_ms: u64,
    /// `length_ms` minus every pause of at least 180 ms, counting only the part of a pause
    /// that lies inside the clip.
    pub tight_length_ms: u64,
    pub dead_air: Vec<SilenceGap>,
    pub hook: SegmentRef,
    pub payoff: SegmentRef,
    pub last: SegmentRef,
    /// The payoff's time into the clip once dead air is cut, or `None` when the payoff
    /// segment is not inside the clip (which is also flagged).
    pub payoff_at_ms: Option<u64>,
    pub open_loop: String,
    pub visual_suggestion: String,
    pub why: String,
    /// What the clip rules changed (for example, material dropped before the hook).
    pub adjustments: Vec<String>,
    /// The rules the clip breaks. Empty means it meets all of them.
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rejected {
    pub rank: u32,
    pub title: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Understanding {
    pub schema_version: u32,
    pub source: Source,
    pub model: ModelRun,
    pub settings: RunSettings,
    pub segments: Vec<Segment>,
    pub summary: Summary,
    pub beats: Vec<Beat>,
    pub visual_events: Vec<VisualEvent>,
    pub cuts: Vec<CutLabel>,
    pub clips: Vec<Clip>,
    pub rejected: Vec<Rejected>,
}
