//! Structural extraction, then ASR, then the JSON write.
//!
//! `signal` covers decode, cuts, silence, loudness, histograms, and the write.
//! `asr` is timed on its own and is not part of the 500 ms gate.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use edit_dna::{BoundaryView, CutDetectorKind, EditDna, SCHEMA_VERSION};

use crate::asr::{self, AsrEstimate};
use crate::decode::{self, DecodedReel, DecoderSetting, RawBoundary};
use crate::error::DecompileError;
use crate::histogram::aggregate;
use crate::loudness::integrated_lufs;
use crate::payoff::from_cuts;
use crate::scene::{CutDetector, DetectorInput, SceneScoreDetector, SCENE_NOTE};
use crate::silence;
use crate::transnet::{
    long_run_warning, select_detector, CutDetectorChoice, SelectedDetector, TRANSNET_NOTE,
};
use crate::worker::WhisperRunner;

const PCM_HALF_WINDOW_MS: u64 = 1_500;

pub struct DecompileRequest {
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub transnet_weights: Option<PathBuf>,
    pub whisper_model: Option<PathBuf>,
    pub whisper_runner: WhisperRunner,
    pub cut_detector: CutDetectorChoice,
    pub decoder: DecoderSetting,
}

pub struct StageProfile {
    pub decode: Duration,
    pub analyze: Duration,
    pub silence: Duration,
    pub loudness: Duration,
    pub asr: Duration,
    /// Parts of `asr`: whisper's first pass and its re-runs of skipped stretches.
    pub asr_first_pass: Duration,
    pub asr_second_pass: Duration,
    pub json: Duration,
}

pub struct DecompileOutput {
    pub dna: EditDna,
    pub signal: Duration,
    pub asr: Duration,
    pub boundaries: Vec<BoundaryView>,
    pub profile: StageProfile,
}

pub fn decompile_file(request: &DecompileRequest) -> Result<DecompileOutput, DecompileError> {
    // The detector is chosen before decoding: `transnet` fails here, without
    // decoding, when the weights, the library or the model are missing, and the
    // 48x27 model frames are only collected when the model will run.
    let select_started = Instant::now();
    let selected = select_detector(request.cut_detector, request.transnet_weights.clone())?;
    let select_elapsed = select_started.elapsed();

    let decode_started = Instant::now();
    let decoded = decode::decode_file(&request.input, request.decoder, selected.runs_model())?;
    let decode_elapsed = decode_started.elapsed();

    let analyze_started = Instant::now();
    let resolved = resolve_cuts(&selected, request.cut_detector, &decoded)?;
    let ResolvedCuts {
        cuts,
        detector,
        note: detector_note,
        boundaries: model_boundaries,
    } = resolved;
    let histograms = aggregate(
        &decoded.colors,
        &cuts,
        decoded.duration_ms,
        decoded.frames.len() as u64,
    );
    let payoff = from_cuts(&cuts);
    let analyze_elapsed = select_elapsed + analyze_started.elapsed();

    let silence_started = Instant::now();
    let silence = silence::analyze(&decoded.pcm, decoded.sample_rate);
    let silence_elapsed = silence_started.elapsed();

    let loudness_started = Instant::now();
    let lufs = integrated_lufs(&decoded.pcm, decoded.sample_rate);
    let loudness_elapsed = loudness_started.elapsed();

    let boundaries = boundary_views(
        model_boundaries.as_deref().unwrap_or(&decoded.boundaries),
        &decoded.pcm,
        decoded.sample_rate,
    );
    let mut signal = decode_elapsed + analyze_elapsed + silence_elapsed + loudness_elapsed;

    let asr_started = Instant::now();
    let asr = asr::transcribe(
        &decoded.pcm,
        decoded.sample_rate,
        decoded.duration_ms,
        &silence,
        request.whisper_model.as_deref(),
        &request.whisper_runner,
    )?;
    let asr_elapsed = asr_started.elapsed();

    let mut dna = assemble(
        &decoded,
        cuts,
        histograms,
        silence,
        lufs,
        payoff,
        &asr,
        detector,
        &detector_note,
    );
    let json_elapsed = match &request.output {
        Some(path) => stamp_json(path, &mut dna, &mut signal, asr_elapsed)?,
        None => {
            dna.signal_ms = duration_ms(signal);
            dna.asr_ms = duration_ms(asr_elapsed);
            Duration::ZERO
        }
    };

    Ok(DecompileOutput {
        dna,
        signal,
        asr: asr_elapsed,
        boundaries,
        profile: StageProfile {
            decode: decode_elapsed,
            analyze: analyze_elapsed,
            silence: silence_elapsed,
            loudness: loudness_elapsed,
            asr: asr_elapsed,
            asr_first_pass: asr.first_pass,
            asr_second_pass: asr.second_pass,
            json: json_elapsed,
        },
    })
}

/// The cut list, which detector made it and why, and, when the model made it,
/// the filmstrip boundaries rebuilt at those cuts (the decode loop captured
/// them at the scene-score cuts).
struct ResolvedCuts {
    cuts: Vec<edit_dna::Cut>,
    detector: CutDetectorKind,
    note: String,
    boundaries: Option<Vec<RawBoundary>>,
}

fn resolve_cuts(
    selected: &SelectedDetector,
    choice: CutDetectorChoice,
    decoded: &DecodedReel,
) -> Result<ResolvedCuts, DecompileError> {
    let scores = DetectorInput::scores(&decoded.frames);
    match selected {
        SelectedDetector::Scene(detector, skipped) => {
            let note = match skipped {
                None => SCENE_NOTE.to_string(),
                Some(reason) => format!("Fell back to the FFmpeg scene-score detector: {reason}"),
            };
            Ok(ResolvedCuts {
                cuts: detector.detect(&scores)?,
                detector: detector.kind(),
                note,
                boundaries: None,
            })
        }
        SelectedDetector::TransNet(detector) => {
            let input = DetectorInput {
                frames: &decoded.frames,
                model_frames: decoded.model_frames.as_ref(),
            };
            match detector.detect(&input) {
                Ok(cuts) => {
                    let boundaries = decoded
                        .model_frames
                        .as_ref()
                        .map(|model| decode::boundaries_from_model_frames(model, &cuts));
                    Ok(ResolvedCuts {
                        cuts,
                        detector: detector.kind(),
                        note: format!(
                            "{TRANSNET_NOTE}{}",
                            long_run_warning(decoded.frames.len()).unwrap_or_default()
                        ),
                        boundaries,
                    })
                }
                Err(err) if choice == CutDetectorChoice::Auto => {
                    let scene = SceneScoreDetector::default();
                    Ok(ResolvedCuts {
                        cuts: scene.detect(&scores)?,
                        detector: scene.kind(),
                        note: format!("Fell back to the FFmpeg scene-score detector: {err}"),
                        boundaries: None,
                    })
                }
                Err(err) => Err(err),
            }
        }
    }
}

fn assemble(
    decoded: &DecodedReel,
    cuts: Vec<edit_dna::Cut>,
    histograms: Vec<edit_dna::ShotHistogram>,
    silence: silence::SilenceAnalysis,
    lufs: f64,
    payoff: edit_dna::PayoffWindow,
    asr: &AsrEstimate,
    detector: CutDetectorKind,
    detector_note: &str,
) -> EditDna {
    let mut dna = EditDna::empty(
        decoded.duration_ms,
        decoded.fps,
        decoded.width,
        decoded.height,
    );
    dna.schema_version = SCHEMA_VERSION;
    dna.frame_count = decoded.frames.len() as u64;
    dna.visual_events = visual_events(&decoded.motion_events, &cuts);
    dna.cuts = cuts;
    dna.silence_gaps = silence.gaps;
    dna.silence_threshold_db = round2(silence.threshold_db);
    dna.noise_floor_db = silence.noise_floor_db.map(round2);
    dna.words = asr.words.clone();
    dna.words_per_minute = asr.words_per_minute;
    dna.integrated_lufs = lufs;
    dna.shot_histograms = histograms;
    dna.payoff_window = payoff;
    dna.cut_detector = detector;
    dna.cut_detector_note = detector_note.to_string();
    dna.asr_backend = asr.backend;
    dna.asr_note = asr.note.clone();
    dna.decode_note = decode::decode_note(decoded.format, &decoded.decoder);
    dna
}

fn boundary_views(boundaries: &[RawBoundary], pcm: &[f32], sample_rate: u32) -> Vec<BoundaryView> {
    boundaries
        .iter()
        .map(|boundary| boundary_view(boundary, pcm, sample_rate))
        .collect()
}

fn boundary_view(boundary: &RawBoundary, pcm: &[f32], sample_rate: u32) -> BoundaryView {
    let mut thumbs = Vec::with_capacity(boundary.before.len() + boundary.after.len());
    thumbs.extend(boundary.before.iter().cloned());
    let cut_thumb_index = thumbs.len();
    thumbs.extend(boundary.after.iter().cloned());
    let (pcm_start_ms, window) = pcm_window(boundary.time_ms, pcm, sample_rate);
    BoundaryView {
        frame_index: boundary.frame_index,
        time_ms: boundary.time_ms,
        thumb_width: boundary.thumb_w,
        thumb_height: boundary.thumb_h,
        thumbs,
        cut_thumb_index,
        sample_rate,
        pcm_start_ms,
        pcm: window,
    }
}

fn pcm_window(time_ms: u64, pcm: &[f32], sample_rate: u32) -> (u64, Vec<f32>) {
    if sample_rate == 0 || pcm.is_empty() {
        return (time_ms.saturating_sub(PCM_HALF_WINDOW_MS), Vec::new());
    }
    let start_ms = time_ms.saturating_sub(PCM_HALF_WINDOW_MS);
    let end_ms = time_ms.saturating_add(PCM_HALF_WINDOW_MS);
    let start = ((u128::from(start_ms) * u128::from(sample_rate)) / 1_000) as usize;
    let end = ((u128::from(end_ms) * u128::from(sample_rate)) / 1_000) as usize;
    let end = end.min(pcm.len());
    let start = start.min(end);
    (start_ms, pcm[start..end].to_vec())
}

fn stamp_json(
    path: &std::path::Path,
    dna: &mut EditDna,
    signal: &mut Duration,
    asr: Duration,
) -> Result<Duration, DecompileError> {
    dna.asr_ms = duration_ms(asr);
    let mut json_total = Duration::ZERO;
    for _ in 0..4 {
        dna.signal_ms = duration_ms(*signal);
        let started = Instant::now();
        write_dna(path, dna)?;
        let write = started.elapsed();
        *signal += write;
        json_total += write;
        if duration_ms(*signal) == dna.signal_ms {
            break;
        }
    }
    dna.signal_ms = duration_ms(*signal);
    Ok(json_total)
}

fn write_dna(path: &std::path::Path, dna: &EditDna) -> Result<(), DecompileError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, dna)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// The measured events as `edit_dna.json` records. An event never lies on a frame that
/// is also a resolved cut (the detector was told about the scene-score cuts; a different
/// cut detector can disagree), and the list stays sorted by time.
fn visual_events(
    events: &[crate::motion::DetectedEvent],
    cuts: &[edit_dna::Cut],
) -> Vec<edit_dna::VisualEvent> {
    use crate::motion::MotionKind;
    use edit_dna::VisualEventKind;
    let mut out: Vec<edit_dna::VisualEvent> = events
        .iter()
        .filter(|event| !cuts.iter().any(|cut| cut.frame_index == event.frame_index))
        .map(|event| edit_dna::VisualEvent {
            time_ms: event.time_ms,
            frame_index: event.frame_index,
            kind: match event.kind {
                MotionKind::PunchIn => VisualEventKind::PunchIn,
                MotionKind::ZoomOut => VisualEventKind::ZoomOut,
                MotionKind::OverlayChange => VisualEventKind::OverlayChange,
            },
            scale: event.scale.map(round4),
            center_x: event.center_x.map(round4),
            center_y: event.center_y.map(round4),
            area: event.area.map(round4),
        })
        .collect();
    out.sort_by_key(|event| (event.time_ms, event.frame_index));
    out
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}
