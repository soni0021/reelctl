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
use crate::easing;
use crate::error::DecompileError;
use crate::histogram::aggregate;
use crate::loudness::integrated_lufs;
use crate::payoff::from_cuts;
use crate::scene::{CutDetector, DetectorInput, SceneScoreDetector, SCENE_NOTE};
use crate::silence;
use crate::transnet::{select_detector, transnet_note, CutDetectorChoice, SelectedDetector};
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
    decompile_selected(request, &selected, select_elapsed)
}

/// Everything after the choice of detector. Tests give it a detector whose
/// model is a fake.
pub(crate) fn decompile_selected(
    request: &DecompileRequest,
    selected: &SelectedDetector,
    select_elapsed: Duration,
) -> Result<DecompileOutput, DecompileError> {
    let decode_started = Instant::now();
    let model_frame_limit = selected.runs_model().then(|| selected.model_frame_limit());
    let decoded = decode::decode_file(&request.input, request.decoder, model_frame_limit)?;
    let decode_elapsed = decode_started.elapsed();

    let analyze_started = Instant::now();
    let resolved = resolve_cuts(selected, request.cut_detector, &decoded)?;
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

    if progress::cancel_requested() {
        return Err(DecompileError::Message(
            progress::CANCELLED_MESSAGE.to_string(),
        ));
    }
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

    if progress::cancel_requested() {
        return Err(DecompileError::Message(
            progress::CANCELLED_MESSAGE.to_string(),
        ));
    }
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
    // A cancel during whisper (its worker was stopped) writes no edit DNA.
    if progress::cancel_requested() {
        return Err(DecompileError::Message(
            progress::CANCELLED_MESSAGE.to_string(),
        ));
    }
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
                        note: transnet_note(decoded.frames.len()),
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
    (dna.visual_events, dna.zoom_curves) = visual_events_and_curves(&decoded.motion_events, &cuts);
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

/// The measured events as `edit_dna.json` records, and one easing curve for each
/// punch-in or zoom-out. An event never lies on a frame that is also a resolved cut
/// (the detector was told about the scene-score cuts; a different cut detector can
/// disagree), and the list stays sorted by time; `event_index` of a curve points
/// into the list as returned.
fn visual_events_and_curves(
    events: &[crate::motion::DetectedEvent],
    cuts: &[edit_dna::Cut],
) -> (Vec<edit_dna::VisualEvent>, edit_dna::ZoomCurves) {
    use crate::motion::MotionKind;
    use edit_dna::VisualEventKind;
    let mut kept: Vec<&crate::motion::DetectedEvent> = events
        .iter()
        .filter(|event| !cuts.iter().any(|cut| cut.frame_index == event.frame_index))
        .collect();
    kept.sort_by_key(|event| (event.time_ms, event.frame_index));
    let mut out = Vec::with_capacity(kept.len());
    let mut curves = Vec::new();
    for (index, event) in kept.into_iter().enumerate() {
        out.push(edit_dna::VisualEvent {
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
        });
        if event.kind != MotionKind::OverlayChange {
            curves.push(zoom_curve(index, event));
        }
    }
    (out, edit_dna::ZoomCurves::measured(curves))
}

/// The curve entry of one zoom move. A move with no measurable change in its
/// track keeps the event's time and has no samples and no curve.
fn zoom_curve(index: usize, event: &crate::motion::DetectedEvent) -> edit_dna::ZoomCurve {
    let curve = easing::curve_from_track(&event.track);
    edit_dna::ZoomCurve {
        event_index: u32::try_from(index).unwrap_or(u32::MAX),
        start_ms: curve.as_ref().map_or(event.time_ms, |c| c.start_ms),
        end_ms: curve.as_ref().map_or(event.time_ms, |c| c.end_ms),
        from_scale: 1.0,
        to_scale: event.scale.map_or(1.0, round4),
        bezier: curve.as_ref().and_then(|c| c.bezier).map(|b| b.map(round4)),
        samples: curve
            .as_ref()
            .map_or(0, |c| u32::try_from(c.samples).unwrap_or(u32::MAX)),
        fit_rms: curve.as_ref().and_then(|c| c.fit_rms).map(round4),
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::decode::DecoderSetting;
    use crate::transnet::{TransNetV2Detector, WindowModel, TRANSNET_NOTE};
    use crate::transnet_window::{window_count, FRAME_BYTES, WINDOW};
    use crate::worker::WhisperRunner;

    /// What a fake model was given: the length of each window and the sum of its values.
    type Calls = Arc<Mutex<Vec<(usize, f64)>>>;

    /// A model that records its windows and answers with `answer`: a probability
    /// of 0.9 at output 40 (frame 15 of each window's kept part, so a cut at
    /// frames 16, 66, 116, ...) and 0.01 elsewhere, or an error.
    #[derive(Debug)]
    struct FakeModel {
        calls: Calls,
        fail_with: Option<&'static str>,
    }

    impl WindowModel for FakeModel {
        fn run_window(&self, window: &[f32]) -> Result<Vec<f32>, DecompileError> {
            let sum: f64 = window.iter().map(|value| f64::from(*value)).sum();
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((window.len(), sum));
            if let Some(message) = self.fail_with {
                return Err(DecompileError::Message(message.to_string()));
            }
            let mut out = vec![0.01f32; WINDOW];
            out[40] = 0.9;
            Ok(out)
        }
    }

    fn fake_detector(
        fail_with: Option<&'static str>,
        max_frames: usize,
    ) -> (SelectedDetector, Calls) {
        let calls: Calls = Arc::default();
        let model = FakeModel {
            calls: Arc::clone(&calls),
            fail_with,
        };
        let detector = TransNetV2Detector::new(Box::new(model), max_frames);
        (SelectedDetector::TransNet(detector), calls)
    }

    fn work_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("reelctl-pipeline-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A 5 s, 160x90, 24 fps clip with a tone: 120 frames, so 3 windows.
    fn clip(dir: &Path) -> PathBuf {
        let path = dir.join("clip.mp4");
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "lavfi", "-i", "testsrc2=s=160x90:r=24:d=5"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=5"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&path)
            .status()
            .expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg could not make the clip");
        path
    }

    fn request(input: &Path, out: &Path, choice: CutDetectorChoice) -> DecompileRequest {
        DecompileRequest {
            input: input.to_path_buf(),
            output: Some(out.to_path_buf()),
            transnet_weights: None,
            whisper_model: None,
            whisper_runner: WhisperRunner::InProcess,
            cut_detector: choice,
            decoder: DecoderSetting::Software,
        }
    }

    fn run(
        choice: CutDetectorChoice,
        selected: &SelectedDetector,
        dir: &Path,
    ) -> (Result<DecompileOutput, DecompileError>, PathBuf) {
        let out = dir.join("out.edit_dna.json");
        let result = decompile_selected(
            &request(&dir.join("clip.mp4"), &out, choice),
            selected,
            Duration::ZERO,
        );
        (result, out)
    }

    #[test]
    fn the_frames_the_decode_loop_collects_reach_the_model_window_by_window() {
        let dir = work_dir("reach");
        clip(&dir);
        let (selected, calls) = fake_detector(None, 200_000);
        let (result, _) = run(CutDetectorChoice::TransNet, &selected, &dir);
        let output = result.expect("decompile");
        let calls = calls.lock().expect("calls");
        assert_eq!(output.dna.frame_count, 120);
        assert_eq!(calls.len(), window_count(120), "one call per window");
        for (length, sum) in calls.iter() {
            assert_eq!(*length, WINDOW * FRAME_BYTES);
            assert!(*sum > 0.0, "the window held real picture data");
        }
        // The clip changes from frame to frame, so each window holds other frames.
        for pair in calls.windows(2) {
            assert_ne!(pair[0].1, pair[1].1, "two windows held the same frames");
        }
        assert_eq!(output.dna.cut_detector, CutDetectorKind::TransNetV2);
        assert!(output.dna.cut_detector_note.starts_with(TRANSNET_NOTE));
        let cuts: Vec<u64> = output.dna.cuts.iter().map(|cut| cut.frame_index).collect();
        assert_eq!(cuts, vec![16, 66, 116]);
        // The filmstrip comes from the 48x27 frames when the model made the cuts.
        assert_eq!(output.boundaries.len(), 3);
        assert_eq!(output.boundaries[0].thumb_width, 48);
        assert_eq!(output.boundaries[0].thumb_height, 27);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transnet_never_returns_scene_score_cuts_when_the_model_fails() {
        let dir = work_dir("fail-transnet");
        clip(&dir);
        let (selected, calls) = fake_detector(Some("model exploded"), 200_000);
        let (result, out) = run(CutDetectorChoice::TransNet, &selected, &dir);
        let err = result.err().expect("an error");
        assert!(err.to_string().contains("model exploded"), "{err}");
        assert_eq!(
            calls.lock().expect("calls").len(),
            1,
            "it stopped at the first window"
        );
        assert!(!out.exists(), "nothing is written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_falls_back_to_the_scene_score_when_the_model_fails_and_says_why() {
        let dir = work_dir("fail-auto");
        clip(&dir);
        let (selected, _) = fake_detector(Some("model exploded"), 200_000);
        let (result, out) = run(CutDetectorChoice::Auto, &selected, &dir);
        let output = result.expect("auto falls back");
        assert_eq!(output.dna.cut_detector, CutDetectorKind::FfmpegSceneScore);
        let note = &output.dna.cut_detector_note;
        assert!(
            note.starts_with("Fell back to the FFmpeg scene-score detector"),
            "{note}"
        );
        assert!(note.contains("model exploded"), "{note}");
        assert!(out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clip_over_the_frame_limit_is_not_run_and_transnet_says_to_use_scene() {
        let dir = work_dir("limit-transnet");
        clip(&dir);
        let (selected, calls) = fake_detector(None, 50);
        let (result, out) = run(CutDetectorChoice::TransNet, &selected, &dir);
        let err = result.err().expect("too long");
        let text = err.to_string();
        assert!(text.contains("too long for TransNetV2"), "{text}");
        assert!(text.contains("120 frames"), "{text}");
        assert!(text.contains("--cut-detector scene"), "{text}");
        assert!(
            calls.lock().expect("calls").is_empty(),
            "the model must not run"
        );
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_over_the_frame_limit_falls_back_without_running_the_model() {
        let dir = work_dir("limit-auto");
        clip(&dir);
        let (selected, calls) = fake_detector(None, 50);
        let (result, _) = run(CutDetectorChoice::Auto, &selected, &dir);
        let output = result.expect("auto falls back");
        assert_eq!(output.dna.cut_detector, CutDetectorKind::FfmpegSceneScore);
        assert!(output
            .dna
            .cut_detector_note
            .contains("too long for TransNetV2"));
        assert!(calls.lock().expect("calls").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    use crate::motion::{DetectedEvent, MotionKind, ScalePoint};
    use edit_dna::{Cut, VisualEventKind};

    fn point(time_ms: u64, scale: f64) -> ScalePoint {
        ScalePoint { time_ms, scale }
    }

    /// A zoom event at `frame` whose track runs from 1 to `total` over 12 frames
    /// of a straight line, then holds for two.
    fn zoom(kind: MotionKind, frame: u64, total: f64) -> DetectedEvent {
        let mut track = vec![point(frame * 42 - 500, 1.0)];
        for k in 1..=14u64 {
            let share = (k.min(12)) as f64 / 12.0;
            track.push(point(
                frame * 42 - 500 + k * 42,
                1.0 + (total - 1.0) * share,
            ));
        }
        DetectedEvent {
            time_ms: frame * 42,
            frame_index: frame,
            kind,
            scale: Some(total),
            center_x: Some(0.5),
            center_y: Some(0.5),
            area: None,
            track,
        }
    }

    fn overlay(frame: u64) -> DetectedEvent {
        DetectedEvent {
            time_ms: frame * 42,
            frame_index: frame,
            kind: MotionKind::OverlayChange,
            scale: None,
            center_x: None,
            center_y: None,
            area: Some(0.12),
            track: Vec::new(),
        }
    }

    fn event(frame_index: u64, time_ms: u64, kind: MotionKind) -> DetectedEvent {
        DetectedEvent {
            time_ms,
            frame_index,
            kind,
            scale: None,
            center_x: None,
            center_y: None,
            area: None,
            track: Vec::new(),
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

    /// The events alone, as `visual_events_and_curves` returns them.
    fn visual_events(events: &[DetectedEvent], cuts: &[Cut]) -> Vec<edit_dna::VisualEvent> {
        visual_events_and_curves(events, cuts).0
    }

    #[test]
    fn a_curve_for_each_zoom_in_event_order_and_none_for_an_overlay() {
        let events = vec![
            zoom(MotionKind::ZoomOut, 300, 0.85),
            overlay(200),
            zoom(MotionKind::PunchIn, 100, 1.18),
        ];
        let (visual, curves) = visual_events_and_curves(&events, &[]);
        assert_eq!(visual.len(), 3);
        assert_eq!(curves.status, edit_dna::CurveStatus::Measured);
        assert_eq!(curves.reason, "");
        let indexes: Vec<u32> = curves.curves.iter().map(|c| c.event_index).collect();
        assert_eq!(
            indexes,
            vec![0, 2],
            "events sorted: punch-in, overlay, zoom-out"
        );
        assert_eq!(visual[0].kind, edit_dna::VisualEventKind::PunchIn);
        assert_eq!(visual[2].kind, edit_dna::VisualEventKind::ZoomOut);
        let first = &curves.curves[0];
        assert_eq!((first.from_scale, first.to_scale), (1.0, 1.18));
        assert_eq!(curves.curves[1].to_scale, 0.85);
        let [x1, y1, x2, y2] = first.bezier.expect("a fit");
        assert_eq!((x1, x2), (0.3333, 0.6667));
        assert!(
            (y1 - 1.0 / 3.0).abs() < 0.01 && (y2 - 2.0 / 3.0).abs() < 0.01,
            "{y1} {y2}"
        );
        assert!(first.fit_rms.expect("rms") < 0.01);
        assert!(first.samples >= 12);
        assert!(first.start_ms < first.end_ms);
    }

    #[test]
    fn an_event_on_a_cut_is_dropped_with_its_curve_and_indexes_follow() {
        let events = vec![
            zoom(MotionKind::PunchIn, 100, 1.18),
            zoom(MotionKind::PunchIn, 200, 1.2),
        ];
        let cut = edit_dna::Cut {
            frame_index: 100,
            time_ms: 4_200,
            interval_ms: 4_200,
            scene_score: 0.9,
        };
        let (visual, curves) = visual_events_and_curves(&events, &[cut]);
        assert_eq!(visual.len(), 1);
        assert_eq!(visual[0].frame_index, 200);
        assert_eq!(curves.curves.len(), 1);
        assert_eq!(curves.curves[0].event_index, 0);
        assert_eq!(curves.curves[0].to_scale, 1.2);
    }

    #[test]
    fn no_events_gives_measured_curves_with_an_empty_list() {
        let (visual, curves) = visual_events_and_curves(&[], &[]);
        assert!(visual.is_empty());
        assert_eq!(curves, edit_dna::ZoomCurves::measured(Vec::new()));
    }

    #[test]
    fn a_snap_keeps_its_entry_with_a_null_curve() {
        let mut snap = zoom(MotionKind::PunchIn, 100, 1.18);
        snap.track = vec![point(3_700, 1.0), point(3_742, 1.18), point(3_784, 1.18)];
        let (_, curves) = visual_events_and_curves(&[snap], &[]);
        let entry = &curves.curves[0];
        assert_eq!(
            (entry.start_ms, entry.end_ms, entry.samples),
            (3_700, 3_742, 2)
        );
        assert!(entry.bezier.is_none() && entry.fit_rms.is_none());
    }

    #[test]
    fn a_zoom_with_no_track_keeps_its_entry_without_samples() {
        let mut bare = zoom(MotionKind::PunchIn, 100, 1.18);
        bare.track.clear();
        let (_, curves) = visual_events_and_curves(&[bare], &[]);
        let entry = &curves.curves[0];
        assert_eq!(
            (entry.start_ms, entry.end_ms, entry.samples),
            (4_200, 4_200, 0)
        );
        assert!(entry.bezier.is_none() && entry.fit_rms.is_none());
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
        let mut punch = event(10, 400, MotionKind::PunchIn);
        punch.scale = Some(1.180_049_9);
        punch.center_x = Some(0.500_06);
        punch.center_y = Some(0.123_449_9);
        let mut cover = event(20, 800, MotionKind::OverlayChange);
        cover.area = Some(0.075_849_99);
        let out = visual_events(&[punch, cover], &[]);
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
