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
use crate::decode::{self, DecodedReel, DecoderSetting, RawBoundary, THUMB_H, THUMB_W};
use crate::error::DecompileError;
use crate::histogram::aggregate;
use crate::loudness::integrated_lufs;
use crate::payoff::from_cuts;
use crate::scene::{CutDetector, SceneScoreDetector, SCENE_NOTE};
use crate::silence;
use crate::transnet::{select_detector, CutDetectorChoice, SelectedDetector};
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
    let decode_started = Instant::now();
    let decoded = decode::decode_file(&request.input, request.decoder)?;
    let decode_elapsed = decode_started.elapsed();

    let analyze_started = Instant::now();
    let selected = select_detector(request.cut_detector, request.transnet_weights.clone())?;
    let (cuts, detector, detector_note) = resolve_cuts(&selected, request.cut_detector, &decoded)?;
    let histograms = aggregate(
        &decoded.colors,
        &cuts,
        decoded.duration_ms,
        decoded.frames.len() as u64,
    );
    let payoff = from_cuts(&cuts);
    let analyze_elapsed = analyze_started.elapsed();

    let silence_started = Instant::now();
    let silence = silence::analyze(&decoded.pcm, decoded.sample_rate);
    let silence_elapsed = silence_started.elapsed();

    let loudness_started = Instant::now();
    let lufs = integrated_lufs(&decoded.pcm, decoded.sample_rate);
    let loudness_elapsed = loudness_started.elapsed();

    let boundaries = boundary_views(&decoded);
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

fn resolve_cuts(
    selected: &SelectedDetector,
    choice: CutDetectorChoice,
    decoded: &DecodedReel,
) -> Result<(Vec<edit_dna::Cut>, CutDetectorKind, String), DecompileError> {
    match selected {
        SelectedDetector::Scene(detector) => Ok((
            detector.detect(&decoded.frames)?,
            detector.kind(),
            SCENE_NOTE.to_string(),
        )),
        SelectedDetector::TransNet(detector) => match detector.detect(&decoded.frames) {
            Ok(cuts) => Ok((
                cuts,
                detector.kind(),
                "TransNetV2 ONNX produced the cut list.".to_string(),
            )),
            Err(err) => match choice {
                CutDetectorChoice::Auto => {
                    let scene = SceneScoreDetector::default();
                    let cuts = scene.detect(&decoded.frames)?;
                    Ok((
                        cuts,
                        scene.kind(),
                        format!("Fell back to the FFmpeg scene-score detector: {err}"),
                    ))
                }
                CutDetectorChoice::TransNet => Err(err),
                CutDetectorChoice::Scene => Err(err),
            },
        },
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

fn boundary_views(decoded: &DecodedReel) -> Vec<BoundaryView> {
    decoded
        .boundaries
        .iter()
        .map(|boundary| boundary_view(boundary, &decoded.pcm, decoded.sample_rate))
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
        thumb_width: THUMB_W,
        thumb_height: THUMB_H,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{DetectedEvent, MotionKind};
    use edit_dna::{Cut, VisualEventKind};

    fn event(frame_index: u64, time_ms: u64, kind: MotionKind) -> DetectedEvent {
        DetectedEvent {
            time_ms,
            frame_index,
            kind,
            scale: None,
            center_x: None,
            center_y: None,
            area: None,
        }
    }

    fn cut_at(frame_index: u64) -> Cut {
        Cut {
            frame_index,
            time_ms: frame_index * 1000 / 24,
            interval_ms: 0,
            scene_score: 0.5,
        }
    }

    #[test]
    fn an_event_on_a_resolved_cut_frame_is_dropped_and_the_others_are_kept() {
        let events = [
            event(30, 1250, MotionKind::OverlayChange),
            event(60, 2500, MotionKind::OverlayChange),
            event(90, 3750, MotionKind::OverlayChange),
        ];
        let kept = visual_events(&events, &[cut_at(60), cut_at(61), cut_at(5)]);
        let frames: Vec<u64> = kept.iter().map(|e| e.frame_index).collect();
        assert_eq!(frames, vec![30, 90]);
        // A cut elsewhere drops nothing; no cuts drop nothing.
        assert_eq!(visual_events(&events, &[cut_at(59)]).len(), 3);
        assert_eq!(visual_events(&events, &[]).len(), 3);
        assert!(visual_events(&[], &[cut_at(1)]).is_empty());
    }

    #[test]
    fn events_come_out_sorted_by_time_then_frame() {
        let events = [
            event(90, 3750, MotionKind::OverlayChange),
            event(31, 1300, MotionKind::PunchIn),
            event(30, 1300, MotionKind::ZoomOut),
            event(12, 500, MotionKind::OverlayChange),
        ];
        let out = visual_events(&events, &[]);
        let order: Vec<(u64, u64)> = out.iter().map(|e| (e.time_ms, e.frame_index)).collect();
        assert_eq!(order, vec![(500, 12), (1300, 30), (1300, 31), (3750, 90)]);
    }

    #[test]
    fn each_kind_maps_to_its_own_record_kind() {
        let out = visual_events(
            &[
                event(1, 10, MotionKind::PunchIn),
                event(2, 20, MotionKind::ZoomOut),
                event(3, 30, MotionKind::OverlayChange),
            ],
            &[],
        );
        let kinds: Vec<VisualEventKind> = out.iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                VisualEventKind::PunchIn,
                VisualEventKind::ZoomOut,
                VisualEventKind::OverlayChange
            ]
        );
    }

    #[test]
    fn measures_are_rounded_to_four_decimals_and_absent_ones_stay_absent() {
        let mut zoom = event(10, 400, MotionKind::PunchIn);
        zoom.scale = Some(1.180_049_9);
        zoom.center_x = Some(0.500_06);
        zoom.center_y = Some(0.123_449_9);
        let mut overlay = event(20, 800, MotionKind::OverlayChange);
        overlay.area = Some(0.075_849_99);
        let out = visual_events(&[zoom, overlay], &[]);
        assert_eq!(out[0].time_ms, 400);
        assert_eq!(out[0].frame_index, 10);
        assert_eq!(out[0].scale, Some(1.18));
        assert_eq!(out[0].center_x, Some(0.5001));
        assert_eq!(out[0].center_y, Some(0.1234));
        assert_eq!(out[0].area, None);
        assert_eq!(out[1].area, Some(0.0758));
        assert_eq!(out[1].scale, None);
        assert_eq!(out[1].center_x, None);
        assert_eq!(out[1].center_y, None);
    }
}
