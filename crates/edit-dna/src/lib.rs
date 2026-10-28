//! Types for `edit_dna.json`, the millisecond telemetry a reel decompiles into.
//!
//! The JSON schema is [`json_schema`]. Zoom easing curves are measured
//! ([`ZoomCurves`]); `.cube` LUT synthesis stays on [`StubbedPass`] until a
//! later pass.

#![warn(clippy::all)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Bump when the format changes. Version 2 added `silence_threshold_db` and
/// `noise_floor_db`; version 3 added `visual_events`; version 4 turned
/// `zoom_curves` from a stub into measured curves. Older files still parse,
/// with those fields' defaults.
pub const SCHEMA_VERSION: u32 = 4;

/// The silence threshold every version 1 file used.
pub const LEGACY_SILENCE_THRESHOLD_DB: f64 = -40.0;

/// Octupie payoff sits on a 9 second marker. The diff window is 8–10 seconds.
pub const PAYOFF_MARKER_MS: u64 = 9_000;
pub const PAYOFF_WINDOW_START_MS: u64 = 8_000;
pub const PAYOFF_WINDOW_END_MS: u64 = 10_000;

/// Decoded frame size in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// A detected scene change. `interval_ms` is the time since the previous cut,
/// or since the start of the reel for the first cut.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cut {
    pub frame_index: u64,
    pub time_ms: u64,
    pub interval_ms: u64,
    /// FFmpeg scene score in `0..=1` for the scene-score detector.
    /// With TransNetV2, the highest cut probability in the run of frames that gave this cut.
    pub scene_score: f64,
}

/// What a [`VisualEvent`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualEventKind {
    /// A fast scale-up of the picture (`scale` above 1).
    PunchIn,
    /// A fast scale-down of the picture (`scale` below 1).
    ZoomOut,
    /// A block of the frame changed and stayed changed (a caption, a graphic).
    OverlayChange,
}

impl VisualEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PunchIn => "punch_in",
            Self::ZoomOut => "zoom_out",
            Self::OverlayChange => "overlay_change",
        }
    }
}

/// A change inside a shot that is not a cut. `time_ms` and `frame_index` are
/// the first frame that shows the change in full, as for a cut. A field that
/// does not apply to the kind is `null`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VisualEvent {
    pub time_ms: u64,
    pub frame_index: u64,
    pub kind: VisualEventKind,
    /// Factor between the picture before and after a zoom move. `null` for
    /// `overlay_change`.
    pub scale: Option<f64>,
    /// Fixed point of a zoom move as a share of the frame width, `0..=1`.
    pub center_x: Option<f64>,
    /// Fixed point of a zoom move as a share of the frame height, `0..=1`.
    pub center_y: Option<f64>,
    /// Changed share of the frame for `overlay_change`, `0..=1`. `null` for zooms.
    pub area: Option<f64>,
}

/// A run of audio below the silence threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SilenceGap {
    pub start_ms: u64,
    pub end_ms: u64,
    pub duration_ms: u64,
}

/// One word, or one silence-delimited voiced burst when no whisper model was given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Word {
    pub text: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Per-shot RGB summary. Bin values are fractions of the samples in the shot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ShotHistogram {
    pub shot_index: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub start_frame: u64,
    pub end_frame: u64,
    pub mean_r: f64,
    pub mean_g: f64,
    pub mean_b: f64,
    pub bins_r: [f64; 8],
    pub bins_g: [f64; 8],
    pub bins_b: [f64; 8],
}

/// The 9 second payoff window and whether a cue was found inside 8–10 seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PayoffWindow {
    pub marker_ms: u64,
    pub window_start_ms: u64,
    pub window_end_ms: u64,
    pub has_cue: bool,
    pub cue_ms: Option<u64>,
    pub cue_frame_index: Option<u64>,
}

impl PayoffWindow {
    pub fn empty() -> Self {
        Self {
            marker_ms: PAYOFF_MARKER_MS,
            window_start_ms: PAYOFF_WINDOW_START_MS,
            window_end_ms: PAYOFF_WINDOW_END_MS,
            has_cue: false,
            cue_ms: None,
            cue_frame_index: None,
        }
    }
}

/// Pass 1b signals that are named in the blueprint and intentionally not computed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PassStatus {
    Unimplemented,
}

impl PassStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unimplemented => "unimplemented",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StubbedPass {
    pub status: PassStatus,
    pub reason: String,
}

impl StubbedPass {
    pub fn lut_unimplemented() -> Self {
        Self {
            status: PassStatus::Unimplemented,
            reason: ".cube LUT synthesis needs two videos, so it is not part of decompile: use `reelctl look --draft <video> --outlier <video> --out <file.cube>`. Per-shot RGB histograms are recorded here.".to_string(),
        }
    }
}

/// Whether [`ZoomCurves::curves`] was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CurveStatus {
    /// The curves were fitted; an empty list means no zoom move was found.
    Measured,
    /// The file is from a version before 4, which did not fit curves.
    Unimplemented,
}

impl CurveStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Unimplemented => "unimplemented",
        }
    }
}

/// The easing of one zoom move: `cubic-bezier(1/3, y1, 2/3, y2)` fitted to the
/// scale measured at each frame of a [`VisualEvent`] of kind `punch_in` or
/// `zoom_out`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ZoomCurve {
    /// Index into [`EditDna::visual_events`] of the move.
    pub event_index: u32,
    /// The frame before the move (the anchor, or the last frame still within a
    /// hair of it), as measured.
    pub start_ms: u64,
    /// The frame where the picture has reached its final scale. This can be a
    /// little later than the event's `time_ms`, which is the first frame within
    /// the noise floor of the last one.
    pub end_ms: u64,
    /// The scale at `start_ms`, relative to itself: always 1.
    pub from_scale: f64,
    /// The total factor of the move, as in the event's `scale`.
    pub to_scale: f64,
    /// `[x1, y1, x2, y2]` with `x1 = 1/3` and `x2 = 2/3`; `y1` and `y2` lie in
    /// `-0.5..=1.5` (above 1 for an overshoot). `null` when too few frames were
    /// measured to fit one (a one-frame snap).
    pub bezier: Option<[f64; 4]>,
    /// Frames from `start_ms` to `end_ms`, both included.
    pub samples: u32,
    /// Root mean square of the measured progress minus the curve, in units of
    /// progress (0 at the start, 1 at the end). `null` when `bezier` is.
    pub fit_rms: Option<f64>,
}

/// Easing curves of the zoom moves. Version 3 and earlier files hold the stub
/// they always had (`unimplemented`) and read with an empty list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ZoomCurves {
    pub status: CurveStatus,
    pub reason: String,
    /// One entry per `punch_in` or `zoom_out` in `visual_events`, in order.
    #[serde(default)]
    pub curves: Vec<ZoomCurve>,
}

impl ZoomCurves {
    pub fn measured(curves: Vec<ZoomCurve>) -> Self {
        Self {
            status: CurveStatus::Measured,
            reason: String::new(),
            curves,
        }
    }

    pub fn unimplemented() -> Self {
        Self {
            status: CurveStatus::Unimplemented,
            reason: "This file was written before zoom easing curves were measured (schema version 3 or earlier).".to_string(),
            curves: Vec::new(),
        }
    }
}

/// Which cut detector produced [`EditDna::cuts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CutDetectorKind {
    FfmpegSceneScore,
    TransNetV2,
}

impl CutDetectorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FfmpegSceneScore => "ffmpeg_scene_score",
            Self::TransNetV2 => "transnet_v2",
        }
    }
}

/// Which word timer produced [`EditDna::words`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AsrBackend {
    WhisperCpp,
    SilenceBursts,
}

impl AsrBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WhisperCpp => "whisper_cpp",
            Self::SilenceBursts => "silence_bursts",
        }
    }
}

/// Millisecond edit telemetry for one reel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EditDna {
    pub schema_version: u32,
    pub duration_ms: u64,
    pub fps: f64,
    pub frame_size: FrameSize,
    pub frame_count: u64,
    pub cuts: Vec<Cut>,
    /// Punch-ins, zoom-outs and overlay changes inside shots, in time order.
    /// Empty in a version 1 or 2 file, which did not look for them.
    #[serde(default)]
    pub visual_events: Vec<VisualEvent>,
    pub silence_gaps: Vec<SilenceGap>,
    /// Level in dB below which a 10 ms window counts as silent in this recording.
    /// A version 1 file, which has no such field, used a fixed -40 dBFS.
    #[serde(default = "legacy_silence_threshold_db")]
    pub silence_threshold_db: f64,
    /// 10th-percentile window level in dB, the noise-floor estimate. `null`
    /// without audio, and in version 1 files, which did not measure it.
    #[serde(default)]
    pub noise_floor_db: Option<f64>,
    pub words: Vec<Word>,
    pub words_per_minute: f64,
    pub integrated_lufs: f64,
    pub shot_histograms: Vec<ShotHistogram>,
    pub payoff_window: PayoffWindow,
    /// Decode, cuts, silence, loudness, histograms, and the JSON write.
    pub signal_ms: u64,
    /// Word timing. Not part of the structural budget.
    pub asr_ms: u64,
    #[serde(default = "ZoomCurves::unimplemented")]
    pub zoom_curves: ZoomCurves,
    pub cube_lut: StubbedPass,
    pub cut_detector: CutDetectorKind,
    pub cut_detector_note: String,
    pub asr_backend: AsrBackend,
    pub asr_note: String,
    pub decode_note: String,
}

impl EditDna {
    pub fn empty(duration_ms: u64, fps: f64, width: u32, height: u32) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            duration_ms,
            fps,
            frame_size: FrameSize { width, height },
            frame_count: 0,
            cuts: Vec::new(),
            visual_events: Vec::new(),
            silence_gaps: Vec::new(),
            silence_threshold_db: LEGACY_SILENCE_THRESHOLD_DB,
            noise_floor_db: None,
            words: Vec::new(),
            words_per_minute: 0.0,
            integrated_lufs: -70.0,
            shot_histograms: Vec::new(),
            payoff_window: PayoffWindow::empty(),
            signal_ms: 0,
            asr_ms: 0,
            zoom_curves: ZoomCurves::measured(Vec::new()),
            cube_lut: StubbedPass::lut_unimplemented(),
            cut_detector: CutDetectorKind::FfmpegSceneScore,
            cut_detector_note: String::new(),
            asr_backend: AsrBackend::SilenceBursts,
            asr_note: String::new(),
            decode_note: String::new(),
        }
    }
}

fn legacy_silence_threshold_db() -> f64 {
    LEGACY_SILENCE_THRESHOLD_DB
}

/// Thumbnails and a PCM window around one cut, for a `timeline_view` PNG.
/// This is not part of `edit_dna.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryView {
    pub frame_index: u64,
    pub time_ms: u64,
    pub thumb_width: u32,
    pub thumb_height: u32,
    pub thumbs: Vec<Vec<u8>>,
    /// Index into `thumbs` of the first frame of the new shot.
    pub cut_thumb_index: usize,
    pub sample_rate: u32,
    pub pcm_start_ms: u64,
    pub pcm: Vec<f32>,
}

/// JSON Schema for [`EditDna`].
pub fn json_schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(EditDna)).expect("EditDna schema serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lists_structural_fields() {
        let schema = json_schema();
        let props = schema
            .get("properties")
            .and_then(|value| value.as_object())
            .expect("schema properties");
        for key in [
            "duration_ms",
            "fps",
            "frame_size",
            "cuts",
            "visual_events",
            "silence_gaps",
            "silence_threshold_db",
            "noise_floor_db",
            "words_per_minute",
            "integrated_lufs",
            "shot_histograms",
            "payoff_window",
            "signal_ms",
            "asr_ms",
            "zoom_curves",
            "cube_lut",
        ] {
            assert!(props.contains_key(key), "schema missing {key}");
        }
    }

    #[test]
    fn a_version_1_file_reads_with_the_fixed_silence_threshold() {
        let mut value =
            serde_json::to_value(EditDna::empty(1_000, 30.0, 64, 36)).expect("serialize");
        let object = value.as_object_mut().expect("object");
        object.remove("silence_threshold_db");
        object.remove("noise_floor_db");
        object.insert("schema_version".to_string(), 1.into());
        let dna: EditDna = serde_json::from_value(value).expect("a version 1 file parses");
        assert_eq!(dna.silence_threshold_db, -40.0);
        assert_eq!(dna.noise_floor_db, None);
    }

    #[test]
    fn the_schema_file_matches_the_types() {
        let file: serde_json::Value =
            serde_json::from_str(include_str!("../schema/edit_dna.schema.json"))
                .expect("schema file parses");
        assert_eq!(
            file,
            json_schema(),
            "regenerate with `cargo run -p edit-dna --example schema`"
        );
    }

    #[test]
    fn version_1_and_2_files_read_with_no_visual_events() {
        for version in [1, 2] {
            let mut value =
                serde_json::to_value(EditDna::empty(1_000, 30.0, 64, 36)).expect("serialize");
            let object = value.as_object_mut().expect("object");
            object.remove("visual_events");
            object.insert("schema_version".to_string(), version.into());
            let dna: EditDna = serde_json::from_value(value).expect("an old file parses");
            assert!(dna.visual_events.is_empty(), "version {version}");
        }
    }

    #[test]
    fn a_new_file_has_an_empty_visual_event_list_and_measured_curves_at_version_4() {
        let dna = EditDna::empty(1_000, 30.0, 64, 36);
        assert_eq!(SCHEMA_VERSION, 4);
        assert_eq!(dna.schema_version, 4);
        let value = serde_json::to_value(&dna).expect("serialize");
        assert_eq!(value["visual_events"], serde_json::json!([]));
        assert_eq!(
            value["zoom_curves"],
            serde_json::json!({"status": "measured", "reason": "", "curves": []})
        );
    }

    #[test]
    fn a_version_3_file_reads_with_unimplemented_zoom_curves_and_an_empty_list() {
        for version in [1, 2, 3] {
            let mut value =
                serde_json::to_value(EditDna::empty(1_000, 30.0, 64, 36)).expect("serialize");
            let object = value.as_object_mut().expect("object");
            object.insert("schema_version".to_string(), version.into());
            object.insert(
                "zoom_curves".to_string(),
                serde_json::json!({
                    "status": "unimplemented",
                    "reason": "Optical-flow Bézier zoom curves are Pass 1b and are not computed on the timed structural path."
                }),
            );
            let dna: EditDna = serde_json::from_value(value).expect("an old file parses");
            assert_eq!(dna.zoom_curves.status, CurveStatus::Unimplemented);
            assert_eq!(dna.zoom_curves.status.as_str(), "unimplemented");
            assert!(dna.zoom_curves.curves.is_empty(), "version {version}");
            assert!(
                dna.zoom_curves.reason.contains("Pass 1b"),
                "version {version}"
            );
        }
    }

    #[test]
    fn zoom_curves_round_trip_with_a_fitted_curve_and_a_snap() {
        let json = serde_json::json!({"status": "measured", "reason": "", "curves": [
            {"event_index": 0, "start_ms": 4000, "end_ms": 4250, "from_scale": 1.0,
             "to_scale": 1.1794, "bezier": [0.3333, 0.02, 0.6667, 1.08], "samples": 7,
             "fit_rms": 0.012},
            {"event_index": 2, "start_ms": 6000, "end_ms": 6042, "from_scale": 1.0,
             "to_scale": 0.85, "bezier": null, "samples": 2, "fit_rms": null}
        ]});
        let curves: ZoomCurves = serde_json::from_value(json.clone()).expect("parse");
        assert_eq!(curves.status, CurveStatus::Measured);
        assert_eq!(curves.curves.len(), 2);
        assert_eq!(curves.curves[0].bezier, Some([0.3333, 0.02, 0.6667, 1.08]));
        assert_eq!(curves.curves[1].bezier, None);
        assert_eq!(curves.curves[1].fit_rms, None);
        assert_eq!(curves.curves[1].samples, 2);
        assert_eq!(serde_json::to_value(&curves).expect("serialize"), json);
    }

    #[test]
    fn the_schema_describes_the_curve_entries() {
        let schema = serde_json::to_string(&json_schema()).expect("schema text");
        for key in ["event_index", "from_scale", "to_scale", "bezier", "fit_rms"] {
            assert!(schema.contains(key), "schema missing {key}");
        }
    }

    #[test]
    fn visual_events_round_trip_with_snake_case_kinds_and_nulls() {
        let json = serde_json::json!([
            {"time_ms": 4000, "frame_index": 96, "kind": "punch_in",
             "scale": 1.18, "center_x": 0.5, "center_y": 0.5, "area": null},
            {"time_ms": 6000, "frame_index": 144, "kind": "zoom_out",
             "scale": 0.85, "center_x": 0.5, "center_y": 0.5, "area": null},
            {"time_ms": 8000, "frame_index": 192, "kind": "overlay_change",
             "scale": null, "center_x": null, "center_y": null, "area": 0.12}
        ]);
        let events: Vec<VisualEvent> = serde_json::from_value(json.clone()).expect("parse");
        assert_eq!(events[0].kind, VisualEventKind::PunchIn);
        assert_eq!(events[1].kind, VisualEventKind::ZoomOut);
        assert_eq!(events[2].kind, VisualEventKind::OverlayChange);
        assert_eq!(events[2].area, Some(0.12));
        assert_eq!(events[2].scale, None);
        assert_eq!(serde_json::to_value(&events).expect("serialize"), json);
    }

    #[test]
    fn stubs_say_unimplemented() {
        assert_eq!(ZoomCurves::unimplemented().status.as_str(), "unimplemented");
        assert_eq!(ZoomCurves::measured(Vec::new()).status.as_str(), "measured");
        assert!(StubbedPass::lut_unimplemented().reason.contains(".cube"));
        assert_eq!(
            CutDetectorKind::FfmpegSceneScore.as_str(),
            "ffmpeg_scene_score"
        );
        assert_eq!(AsrBackend::SilenceBursts.as_str(), "silence_bursts");
    }
}
