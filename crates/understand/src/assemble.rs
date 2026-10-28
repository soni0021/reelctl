//! The model's reply and the measured facts, put together as `understanding.json`.

use edit_dna::Cut;

use crate::backend::VlmReply;
use crate::clips::ClipOutcome;
use crate::types::{RunSettings, Segment, Understanding};

/// Everything `assemble` needs. All of it is measured or already checked.
#[derive(Debug, Clone)]
pub struct AssembleInput<'a> {
    pub source_path: &'a str,
    pub duration_ms: u64,
    pub model_id: &'a str,
    pub segments: &'a [Segment],
    /// `reelctl`'s cuts, for `vlm_label`.
    pub cuts: &'a [Cut],
    /// The times of the frames sent, ascending.
    pub frame_times_ms: &'a [u64],
    pub reply: &'a VlmReply,
    pub clips: ClipOutcome,
    pub settings: RunSettings,
    pub cost_upper_bound_usd: f64,
}

/// Map the reply to `understanding.json`: beats get their times from their segments,
/// each event's `t_s` snaps to the nearest sampled frame time (`seen_at_ms`), each cut
/// gets the `vlm_label` of the event seen in the frame at its time plus or minus
/// [`crate::FRAME_CUT_OFFSET_MS`], and every quote is copied from `segments`.
pub fn assemble(input: AssembleInput<'_>) -> Understanding {
    // Stub: the mapping is built in its own task.
    let _ = input;
    Understanding {
        schema_version: crate::types::SCHEMA_VERSION,
        source: crate::types::Source {
            path: String::new(),
            duration_ms: 0,
        },
        model: crate::types::ModelRun {
            id: String::new(),
            provider: String::new(),
            quantization: None,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: None,
            cost_upper_bound_usd: 0.0,
            elapsed_ms: 0,
        },
        settings: RunSettings {
            every_s: 0,
            sheet: String::new(),
            segment_pause_ms: 0,
            quant: Vec::new(),
        },
        segments: Vec::new(),
        summary: crate::types::Summary {
            topic: String::new(),
            language: String::new(),
            speaker_count: 0,
            visual_formats: Vec::new(),
        },
        beats: Vec::new(),
        visual_events: Vec::new(),
        cuts: Vec::new(),
        clips: Vec::new(),
        rejected: Vec::new(),
    }
}
