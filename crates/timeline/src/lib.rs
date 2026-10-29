//! OpenTimelineIO editing for `reelctl`: the JSON subset of the format, the edit
//! operations (`init`, `cut_silence`, `punch_in`, `attach_bridge`), and the ffmpeg
//! filter graph that renders a project. Pure: nothing here runs a process or reads
//! a file, so all of it is tested without video.

#![warn(clippy::all)]

mod auto_punch;
mod bridge;
mod captions;
mod cut;
mod error;
mod from_clip;
mod init;
mod otio;
mod overlay;
mod punch;
mod reframe;
mod render;
mod smelter;
mod tracks;

#[cfg(test)]
mod testutil;

pub use auto_punch::{
    auto_punch_in, auto_punch_in_like, outlier_scale, AutoPunchReport,
    DEFAULT_SCALE as DEFAULT_PUNCH_SCALE, STATIC_WINDOW_MS,
};
pub use bridge::{
    append_bridge, attach_bridge, bridge_of, BridgeReport, BridgeRequest, BridgeSpec,
    CARD_BACKGROUND, CARD_FOREGROUND, CARD_KIND, MAX_CARD_MS, PAUSE_SEARCH_MS, SILENCE_KIND,
};
pub use captions::{
    add_captions, caption_frames, captions_of, has_captions, preset as caption_preset,
    refuse_captions, remove_captions, CaptionCase, CaptionGroup, CaptionPosition, CaptionState,
    CaptionStyle, Captions, CaptionsReport, PlacedWord, StyleOverrides, CAPTIONS_KEY,
    PRESETS as CAPTION_PRESETS,
};
pub use cut::{check_dna_length, cut_silence, CutOptions, CutReport, RemovedSpan, CUTS_KEY};
pub use error::{Result, TimelineError};
pub use from_clip::{clip_record, from_clip, ClipPick, ClipRecord, CLIP_KEY, MIN_CLIP_FRAMES};
pub use init::{
    dna_ref, init, media_info, set_dna_ref, DnaRef, InitSpec, MediaInfo, DNA_KEY, MEDIA_KEY,
};
pub use otio::{
    reelctl, update_reelctl, Clip, ExternalReference, Gap, GeneratorReference, Item,
    MediaReference, RationalTime, Stack, TimeRange, Timeline, Track, AUDIO_KIND, REELCTL_KEY,
    VIDEO_KIND,
};
pub use overlay::{
    add_overlay, has_overlays, overlays, plan_overlay, refuse_overlays, remove_overlay, Overlay,
    OverlayAsset, OverlayPosition, OverlayReport, OverlayRequest, Slot, EDGE_MARGIN, KEY_BLEND,
    KEY_COLOR, KEY_SIMILARITY, MAX_OVERLAY_MS, MAX_WIDTH_PCT, MIN_OVERLAY_MS, MIN_WIDTH_PCT,
    OVERLAY_KEY, OVERLAY_TRACK_NAME,
};
pub use punch::{punch_in, PunchReport, PunchRequest, MAX_SCALE, RAMP_FRAMES};
pub use reframe::{
    centre_keyframes, crop_size, crop_x_expression, footage_span, plan_path, reframe_of,
    set_reframe, x_at, Aspect, Follow, Keyframe, ReframeSpec, REFRAME_KEY,
};
pub use render::{
    build_render_plan, frame_rate_arg, CaptionPlan, CardSpec, OverlayInput, RenderPlan, RenderSize,
    AUDIO_SAMPLE_RATE, MAX_PIXELS, MAX_SIDE,
};
pub use smelter::{
    build_smelter_plan, SceneStep, SmelterFiles, SmelterInput, SmelterPlan, SmelterRequest,
    CLOCK_ID, CLOCK_MARGIN_FRAMES, END_LEAD_FRAMES, JITTER_NOTE, OUTPUT_ID, UPDATE_LEAD_FRAMES,
};
pub use tracks::{
    ceil_frame, check_editable, floor_frame, frame_ms, project_rate, round_frame, single_media,
    timeline_frames, DEFAULT_FADE_MS, FADE_KEY, PUNCH_IN_KEY,
};

/// A join in the edited timeline: a place where two neighbouring items of the video
/// track meet, for `qa inspect-boundaries`.
#[derive(Debug, Clone, PartialEq)]
pub struct Join {
    /// Timeline frame of the first frame after the join.
    pub frame: i64,
    /// Source file and source frame of the last frame before it (when external).
    pub before: Option<(String, i64)>,
    /// Source file and source frame of the first frame after it (when external).
    pub after: Option<(String, i64)>,
}

/// Every join between neighbouring video items, in order.
pub fn joins(timeline: &Timeline) -> Result<Vec<Join>> {
    let rate = project_rate(timeline)?;
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::Invalid("the project has no video track".to_string()))?;
    let mut position = 0;
    let mut previous: Option<Option<(String, i64)>> = None;
    let mut found = Vec::new();
    for item in &video.children {
        let (start, duration) = tracks::item_range(item, rate)?;
        let url = match item {
            Item::Clip(clip) => tracks::external_url(clip).map(str::to_string),
            Item::Gap(_) => None,
        };
        if let Some(before) = previous.take() {
            found.push(Join {
                frame: position,
                before,
                after: url.clone().map(|url| (url, start)),
            });
        }
        previous = Some(url.map(|url| (url, start + duration - 1)));
        position += duration;
    }
    Ok(found)
}

/// The words of `words` placed on the edited timeline, in milliseconds of the
/// timeline. A word that is not wholly inside a clip's footage is left out.
pub fn words_on_timeline(
    timeline: &Timeline,
    words: &[edit_dna::Word],
) -> Result<Vec<edit_dna::Word>> {
    let rate = project_rate(timeline)?;
    let video = timeline
        .track(VIDEO_KIND)
        .ok_or_else(|| TimelineError::Invalid("the project has no video track".to_string()))?;
    let mut placed = Vec::new();
    let mut position = 0;
    for item in &video.children {
        let (start, duration) = tracks::item_range(item, rate)?;
        if matches!(item, Item::Clip(clip) if tracks::external_url(clip).is_some()) {
            let from_ms = frame_ms(start, rate);
            let to_ms = frame_ms(start + duration, rate);
            let offset_ms = frame_ms(position, rate);
            for word in words {
                if word.start_ms >= from_ms && word.end_ms <= to_ms {
                    placed.push(edit_dna::Word {
                        text: word.text.clone(),
                        start_ms: word.start_ms - from_ms + offset_ms,
                        end_ms: word.end_ms - from_ms + offset_ms,
                    });
                }
            }
        }
        position += duration;
    }
    Ok(placed)
}
