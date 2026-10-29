//! The record a dataset line holds, and the shapes of its parts. The format is
//! documented in `docs/finetune.md` and the spec; the JSON Schema is generated from
//! these types (`cargo run -p dataset --example dataset_schema`).

use edit_dna::{FrameSize, VisualEventKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use understand::Segment;

use crate::split::Split;

/// `dataset_schema_version` of every record and of the manifest.
pub const DATASET_SCHEMA_VERSION: u32 = 1;

/// `provenance.tool`.
pub const TOOL: &str = "reelctl-dataset";

/// `provenance.tool_version`: the workspace version.
pub const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    ClipPicks,
    EditEvents,
    Pacing,
}

impl Task {
    pub const ALL: [Task; 3] = [Task::ClipPicks, Task::EditEvents, Task::Pacing];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClipPicks => "clip_picks",
            Self::EditEvents => "edit_events",
            Self::Pacing => "pacing",
        }
    }

    /// Whether a model can be trained on this task's records today. `edit_events` and
    /// `pacing` have no input that does not give the answer away (see the spec), so
    /// they are for scoring only.
    pub fn trainable(self) -> bool {
        matches!(self, Self::ClipPicks)
    }

    /// The fixed instruction text of the task. It is part of the dataset schema: a
    /// change to it is a change to `DATASET_SCHEMA_VERSION`.
    pub fn instruction(self) -> &'static str {
        match self {
            Self::ClipPicks => {
                "The input is a numbered transcript of a long video: segments with an id, a start \
                 and end in milliseconds, the spoken text, and whether the segment ends a sentence. \
                 Choose clips that could stand as short vertical videos. For each clip give the \
                 segment id of its hook (the opening line), of its payoff (the line that delivers \
                 what the hook promised) and of its cutoff (the last segment, ending on an open \
                 loop before the answer is given). Use only segment ids that appear in the input. \
                 Answer with JSON: {\"picks\": [{\"hook_segment\": N, \"payoff_segment\": N, \
                 \"cutoff_segment\": N}]}, best first."
            }
            Self::EditEvents => {
                "The input describes a short video. List its measured edit events as JSON: \
                 {\"cuts\": [{\"time_ms\": N}], \"visual_events\": [{\"time_ms\": N, \"kind\": \
                 \"punch_in\" | \"zoom_out\" | \"overlay_change\", \"scale\": N | null, \"area\": \
                 N | null}], \"zoom_curves\": [{\"event_index\": N, \"bezier\": [x1, y1, x2, y2] | \
                 null}]}. Times are milliseconds from the start."
            }
            Self::Pacing => {
                "The input describes a short video. Report its measured pacing as JSON: \
                 {\"cut_count\": N, \"mean_shot_ms\": N, \"longest_shot_ms\": N, \
                 \"dead_air_count\": N, \"words_per_minute\": N | null, \"integrated_lufs\": N, \
                 \"payoff_has_cue\": true | false, \"payoff_cue_ms\": N | null}."
            }
        }
    }
}

/// What a model may see for a `clip_picks` record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptInput {
    pub duration_ms: u64,
    pub segments: Vec<Segment>,
}

/// What a model may see for an `edit_events` or `pacing` record: the facts of the
/// file, nothing about its edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MediaFacts {
    pub duration_ms: u64,
    pub fps: f64,
    pub frame_size: FrameSize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Input {
    Transcript(TranscriptInput),
    Media(MediaFacts),
}

/// One clip the teacher run picked, by segment ID, in the teacher's rank order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PickIds {
    pub hook_segment: u32,
    pub payoff_segment: u32,
    pub cutoff_segment: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClipPicksTarget {
    pub picks: Vec<PickIds>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CutTarget {
    pub time_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventTarget {
    pub time_ms: u64,
    pub kind: VisualEventKind,
    pub scale: Option<f64>,
    pub area: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CurveTarget {
    /// Index into the same record's `visual_events`.
    pub event_index: u32,
    pub bezier: Option<[f64; 4]>,
}

/// Measured edit events. `visual_events` is `null` when the file predates schema
/// version 3 and `zoom_curves` is `null` unless curves were measured: not measured is
/// not the same as none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct EditEventsTarget {
    #[serde(default)]
    pub cuts: Vec<CutTarget>,
    #[serde(default)]
    pub visual_events: Option<Vec<EventTarget>>,
    #[serde(default)]
    pub zoom_curves: Option<Vec<CurveTarget>>,
}

/// Measured pacing, from `diff::reel_metrics` and the payoff window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PacingTarget {
    pub cut_count: u64,
    pub mean_shot_ms: u64,
    pub longest_shot_ms: u64,
    pub dead_air_count: u64,
    /// `null` unless the words came from whisper; a burst rate is not a word rate.
    pub words_per_minute: Option<f64>,
    pub integrated_lufs: f64,
    pub payoff_has_cue: bool,
    pub payoff_cue_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Target {
    ClipPicks(ClipPicksTarget),
    EditEvents(EditEventsTarget),
    Pacing(PacingTarget),
}

/// Scoring-only data that a trainer must not show to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Context {
    /// The reel's measured pauses, which the clip rules need.
    ClipPicks {
        silence_gaps: Vec<edit_dna::SilenceGap>,
    },
    Empty {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnderstandingProvenance {
    pub schema_version: u32,
    pub sha256: String,
    /// The model that made the picks. Picks are that teacher's, checked by rules.
    pub teacher_model: String,
    pub teacher_provider: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Provenance {
    pub tool: String,
    pub tool_version: String,
    pub reel_id: String,
    pub edit_dna_schema_version: u32,
    pub edit_dna_sha256: String,
    pub cut_detector: String,
    pub asr_backend: String,
    pub understanding: Option<UnderstandingProvenance>,
}

/// One line of `train.jsonl` or `val.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Record {
    pub dataset_schema_version: u32,
    pub id: String,
    pub task: Task,
    pub reel_id: String,
    pub split: Split,
    pub trainable: bool,
    pub instruction: String,
    pub input: Input,
    pub target: Target,
    pub context: Context,
    pub provenance: Provenance,
}

/// The wire shape, read before the task says which types the parts have.
#[derive(Deserialize)]
struct WireRecord {
    dataset_schema_version: u32,
    id: String,
    task: Task,
    reel_id: String,
    split: Split,
    trainable: bool,
    instruction: String,
    input: serde_json::Value,
    target: serde_json::Value,
    context: serde_json::Value,
    provenance: Provenance,
}

impl Record {
    /// Parse one line. The version must be this build's and the parts must have the
    /// task's shapes.
    pub fn parse(line: &str) -> Result<Record, String> {
        let wire: WireRecord = serde_json::from_str(line).map_err(|err| err.to_string())?;
        if wire.dataset_schema_version != DATASET_SCHEMA_VERSION {
            return Err(format!(
                "dataset_schema_version {} (this build reads {DATASET_SCHEMA_VERSION})",
                wire.dataset_schema_version
            ));
        }
        let part = |what: &str, err: serde_json::Error| format!("{what}: {err}");
        let (input, target, context) = match wire.task {
            Task::ClipPicks => (
                Input::Transcript(
                    serde_json::from_value(wire.input).map_err(|e| part("input", e))?,
                ),
                Target::ClipPicks(
                    serde_json::from_value(wire.target).map_err(|e| part("target", e))?,
                ),
                serde_json::from_value::<ClipContext>(wire.context)
                    .map(|context| Context::ClipPicks {
                        silence_gaps: context.silence_gaps,
                    })
                    .map_err(|e| part("context", e))?,
            ),
            Task::EditEvents => (
                Input::Media(serde_json::from_value(wire.input).map_err(|e| part("input", e))?),
                Target::EditEvents(
                    serde_json::from_value(wire.target).map_err(|e| part("target", e))?,
                ),
                Context::Empty {},
            ),
            Task::Pacing => (
                Input::Media(serde_json::from_value(wire.input).map_err(|e| part("input", e))?),
                Target::Pacing(serde_json::from_value(wire.target).map_err(|e| part("target", e))?),
                Context::Empty {},
            ),
        };
        Ok(Record {
            dataset_schema_version: wire.dataset_schema_version,
            id: wire.id,
            task: wire.task,
            reel_id: wire.reel_id,
            split: wire.split,
            trainable: wire.trainable,
            instruction: wire.instruction,
            input,
            target,
            context,
            provenance: wire.provenance,
        })
    }
}

#[derive(Deserialize)]
struct ClipContext {
    silence_gaps: Vec<edit_dna::SilenceGap>,
}

/// One pick in an answer. `start_segment` and `end_segment` default to the hook and the
/// cutoff, which are what decide the clip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnswerPick {
    pub hook_segment: u32,
    pub payoff_segment: u32,
    pub cutoff_segment: u32,
    #[serde(default)]
    pub start_segment: Option<u32>,
    #[serde(default)]
    pub end_segment: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct ClipPicksAnswer {
    #[serde(default)]
    pub picks: Vec<AnswerPick>,
}

/// A pacing answer: every field may be left out, and a missing field is counted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
pub struct PacingAnswer {
    #[serde(default)]
    pub cut_count: Option<u64>,
    #[serde(default)]
    pub mean_shot_ms: Option<u64>,
    #[serde(default)]
    pub longest_shot_ms: Option<u64>,
    #[serde(default)]
    pub dead_air_count: Option<u64>,
    #[serde(default)]
    pub words_per_minute: Option<f64>,
    #[serde(default)]
    pub integrated_lufs: Option<f64>,
    #[serde(default)]
    pub payoff_has_cue: Option<bool>,
    #[serde(default)]
    pub payoff_cue_ms: Option<u64>,
}

/// One line of an answers file for a dataset: the record id and a value shaped like
/// that task's target.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct AnswerLine {
    pub id: String,
    pub answer: serde_json::Value,
}
