//! TransNetV2 cut detector, run through the `ort` crate.
//!
//! ONNX Runtime is not linked at build time: `ort` is built with `load-dynamic`
//! and this module loads the library the user names in `ORT_DYLIB_PATH` when a
//! run asks for the model, so the default build and `cargo test` need no
//! download and no library. The weights are a file the user supplies
//! (`--transnet-weights`); neither is vendored. Everything that does not need
//! the model (windows, padding, runs, cuts) is in `transnet_window.rs`.
//!
//! Never call `ort::init()` or let `ort` load the library lazily: its default
//! loader panics when the library is missing. The path is always resolved here
//! and given to `ort::init_from`, which returns an error. `ort` allows one
//! environment per process, and `reelctl diff` decompiles several files in one
//! process, so the environment is built once and shared.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use edit_dna::{Cut, CutDetectorKind};
use ort::environment::Environment;
use ort::session::Session;
use ort::value::Tensor;

use crate::error::DecompileError;
use crate::scene::{CutDetector, DetectorInput, SceneScoreDetector};
use crate::transnet_window::{
    cut_points, cuts_at, predict, CUT_THRESHOLD, FRAME_BYTES, MODEL_HEIGHT, MODEL_WIDTH, WINDOW,
};

/// The environment variable that names the ONNX Runtime library (`ort`'s own name).
pub const LIBRARY_ENV: &str = "ORT_DYLIB_PATH";

/// Frames from which the note warns that the run was long: 5 minutes at 30 fps.
const LONG_RUN_FRAMES: usize = 9_000;
/// Milliseconds per video frame measured on an Apple M3 (7 at a load average of
/// about 9, 13 at about 26); other machines differ.
const MS_PER_FRAME: (usize, usize) = (7, 13);

/// What the note adds when the model ran over `frames` frames and that is long.
pub(crate) fn long_run_warning(frames: usize) -> Option<String> {
    if frames < LONG_RUN_FRAMES {
        return None;
    }
    let (fast, slow) = MS_PER_FRAME;
    Some(format!(
        " The model ran on {frames} frames; at the 7 to 13 ms per frame measured on an Apple M3 \
that is {} to {} s, and other machines differ. --cut-detector scene is much faster.",
        frames * fast / 1_000,
        frames * slow / 1_000,
    ))
}

fn too_long_error(seen: usize, limit: usize) -> DecompileError {
    DecompileError::Message(format!(
        "This clip has {seen} frames, which is too long for TransNetV2: it keeps at most \
{limit} frames in memory ({} MB of 48x27 frames). Use --cut-detector scene for a clip this long.",
        limit * FRAME_BYTES / 1_000_000
    ))
}

/// The single-frame transition probability. The graph also has `535` (many-hot),
/// which the official post-processing does not use.
const OUTPUT_NAME: &str = "534";

pub const TRANSNET_NOTE: &str = "\
TransNetV2 (ONNX, run by ort) on 48x27 RGB frames: windows of 100 frames every \
50, the middle 50 outputs kept. A cut is the first frame after a run of frames \
whose transition probability is above 0.5; scene_score is the highest \
probability in the run.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutDetectorChoice {
    Auto,
    Scene,
    TransNet,
}

impl CutDetectorChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Scene => "scene",
            Self::TransNet => "transnet",
        }
    }
}

/// A model session, ready to run. Opening one checks the weights file, the
/// library and the model, so a bad setup fails before the video is decoded.
pub struct TransNetV2Detector {
    session: Mutex<Session>,
    input_name: String,
}

impl std::fmt::Debug for TransNetV2Detector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransNetV2Detector")
            .field("input_name", &self.input_name)
            .finish_non_exhaustive()
    }
}

impl TransNetV2Detector {
    /// Open the weights with the ONNX Runtime library at `library` (the value of
    /// `ORT_DYLIB_PATH`). The error is the reason, ready to show.
    pub fn open_with(weights: Option<&Path>, library: Option<&OsStr>) -> Result<Self, String> {
        let weights = check_weights(weights)?;
        let environment = environment(library)?;
        let session = Session::builder(&environment)
            .and_then(|mut builder| builder.commit_from_file(weights))
            .map_err(|err| {
                format!(
                    "TransNetV2 weights at {} could not be loaded by ONNX Runtime: {err}",
                    weights.display()
                )
            })?;
        let input_name = check_model(&session)?;
        Ok(Self {
            session: Mutex::new(session),
            input_name,
        })
    }

    fn run_window(&self, window: &[f32]) -> Result<Vec<f32>, DecompileError> {
        let shape = [1usize, WINDOW, MODEL_HEIGHT, MODEL_WIDTH, 3];
        let tensor = Tensor::from_array((shape, window.to_vec())).map_err(model_error)?;
        let mut session = self.session.lock().unwrap_or_else(PoisonError::into_inner);
        let outputs = session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(model_error)?;
        let value = outputs.get(OUTPUT_NAME).ok_or_else(|| {
            DecompileError::Message(format!("TransNetV2 gave no output named {OUTPUT_NAME}"))
        })?;
        let (_, probabilities) = value.try_extract_tensor::<f32>().map_err(model_error)?;
        Ok(probabilities.to_vec())
    }
}

fn model_error(err: ort::Error) -> DecompileError {
    DecompileError::Message(format!("TransNetV2 failed while running: {err}"))
}

impl CutDetector for TransNetV2Detector {
    fn kind(&self) -> CutDetectorKind {
        CutDetectorKind::TransNetV2
    }

    fn detect(&self, input: &DetectorInput<'_>) -> Result<Vec<Cut>, DecompileError> {
        detect_with(input, |window| self.run_window(window))
    }
}

/// Cuts from `input`'s 48x27 frames, with `run` standing in for the model.
pub(crate) fn detect_with(
    input: &DetectorInput<'_>,
    run: impl FnMut(&[f32]) -> Result<Vec<f32>, DecompileError>,
) -> Result<Vec<Cut>, DecompileError> {
    let frames = input.model_frames.ok_or_else(|| {
        DecompileError::Message("TransNetV2 was not given the 48x27 frames".to_string())
    })?;
    if frames.exceeds_limit() {
        return Err(too_long_error(frames.len(), frames.limit()));
    }
    if frames.len() != input.frames.len() {
        return Err(DecompileError::Message(format!(
            "TransNetV2 got {} model frames for {} decoded frames",
            frames.len(),
            input.frames.len()
        )));
    }
    let probabilities = predict(frames, run)?;
    Ok(cuts_at(
        input.frames,
        &cut_points(&probabilities, CUT_THRESHOLD),
    ))
}

/// The weights path, if it names a file.
fn check_weights(weights: Option<&Path>) -> Result<&Path, String> {
    match weights {
        None => Err("\
TransNetV2 ONNX weights were not provided. Pass --transnet-weights or set \
REELCTL_TRANSNET_WEIGHTS to the path of transnetv2.onnx. The weights are not \
vendored or downloaded."
            .to_string()),
        Some(path) if !path.is_file() => Err(format!(
            "TransNetV2 ONNX weights were not found at {}. The path is \
configurable (--transnet-weights or REELCTL_TRANSNET_WEIGHTS), but the file is \
not on disk and is not vendored or downloaded.",
            path.display()
        )),
        Some(path) => Ok(path),
    }
}

fn library_hint() -> String {
    format!(
        "set {LIBRARY_ENV} to the full path of an ONNX Runtime library, version 1.17 or newer \
(macOS: brew install onnxruntime, then /opt/homebrew/lib/libonnxruntime.dylib)"
    )
}

/// The process-wide environment; kept after the first success only.
static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);

/// Release the shared ONNX Runtime environment. A process that ran the model
/// must call this before it exits: with the environment still alive at exit,
/// ONNX Runtime 1.21's static destructors abort the process ("mutex lock
/// failed") and the exit status becomes 134 after the output was written.
/// `ort` builds one environment per process for good, so the model cannot be
/// opened again afterwards.
pub fn release_runtime() {
    let mut slot = ENVIRONMENT.lock().unwrap_or_else(PoisonError::into_inner);
    *slot = None;
}

fn environment(library: Option<&OsStr>) -> Result<Environment, String> {
    let mut slot = ENVIRONMENT.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(environment) = slot.as_ref() {
        return Ok(environment.clone());
    }
    let path = library_path(library)?;
    let environment = ort::init_from(&path)
        .map_err(|err| {
            format!(
                "TransNetV2 needs the ONNX Runtime library: {err}; {}",
                library_hint()
            )
        })?
        .build()
        .map_err(|err| format!("ONNX Runtime could not start: {err}"))?;
    *slot = Some(environment.clone());
    Ok(environment)
}

/// The library file named by the environment variable's value.
fn library_path(library: Option<&OsStr>) -> Result<PathBuf, String> {
    match library {
        None => Err(format!(
            "TransNetV2 needs the ONNX Runtime library: {LIBRARY_ENV} is not set; {}",
            library_hint()
        )),
        Some(value) if value.is_empty() => Err(format!(
            "TransNetV2 needs the ONNX Runtime library: {LIBRARY_ENV} is empty; {}",
            library_hint()
        )),
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_file() {
                Ok(path)
            } else {
                Err(format!(
                    "TransNetV2 needs the ONNX Runtime library: {LIBRARY_ENV} names {}, which is not a file; {}",
                    path.display(),
                    library_hint()
                ))
            }
        }
    }
}

/// The model's input name, after checking its shapes.
fn check_model(session: &Session) -> Result<String, String> {
    let expected = [
        1i64,
        WINDOW as i64,
        MODEL_HEIGHT as i64,
        MODEL_WIDTH as i64,
        3,
    ];
    let input = session
        .inputs()
        .first()
        .ok_or_else(|| "The TransNetV2 model has no input".to_string())?;
    let matches = input
        .dtype()
        .tensor_shape()
        .is_some_and(|shape| shape[..] == expected[..]);
    if !matches {
        return Err(format!(
            "The model is not TransNetV2: its input is {:?}, not [1, 100, 27, 48, 3]",
            input.dtype()
        ));
    }
    if !session
        .outputs()
        .iter()
        .any(|output| output.name() == OUTPUT_NAME)
    {
        let names: Vec<&str> = session
            .outputs()
            .iter()
            .map(|output| output.name())
            .collect();
        return Err(format!(
            "The model is not the expected TransNetV2 export: it has no output named {OUTPUT_NAME} (it has {names:?})"
        ));
    }
    Ok(input.name().to_string())
}

#[derive(Debug)]
pub enum SelectedDetector {
    /// The scene-score detector, and why TransNetV2 was passed over when it was asked for.
    Scene(SceneScoreDetector, Option<String>),
    TransNet(TransNetV2Detector),
}

impl SelectedDetector {
    pub fn runs_model(&self) -> bool {
        matches!(self, Self::TransNet(_))
    }
}

/// Choose the detector before the file is decoded. `transnet` fails here when
/// the weights, the library or the model are missing; `auto` falls back to the
/// scene score, with the reason, when a weights path was given and the model
/// cannot run.
pub fn select_detector(
    choice: CutDetectorChoice,
    weights: Option<PathBuf>,
) -> Result<SelectedDetector, DecompileError> {
    let library = std::env::var_os(LIBRARY_ENV);
    select_with(choice, weights.as_deref(), library.as_deref())
}

pub(crate) fn select_with(
    choice: CutDetectorChoice,
    weights: Option<&Path>,
    library: Option<&OsStr>,
) -> Result<SelectedDetector, DecompileError> {
    let scene = SceneScoreDetector::default();
    match choice {
        CutDetectorChoice::Scene => Ok(SelectedDetector::Scene(scene, None)),
        CutDetectorChoice::TransNet => TransNetV2Detector::open_with(weights, library)
            .map(SelectedDetector::TransNet)
            .map_err(|reason| DecompileError::CutDetectorUnavailable { reason }),
        CutDetectorChoice::Auto => match weights {
            None => Ok(SelectedDetector::Scene(scene, None)),
            Some(path) => match TransNetV2Detector::open_with(Some(path), library) {
                Ok(detector) => Ok(SelectedDetector::TransNet(detector)),
                Err(reason) => Ok(SelectedDetector::Scene(scene, Some(reason))),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::ScoredFrame;
    use crate::transnet_window::{ModelFrames, FRAME_BYTES};
    use std::ffi::OsString;

    /// A path that is certainly not a file.
    fn no_library() -> OsString {
        OsString::from("/nonexistent/reelctl-test/libonnxruntime.dylib")
    }

    fn some_file() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
    }

    fn reason(result: Result<TransNetV2Detector, String>) -> String {
        result.err().unwrap_or_default()
    }

    #[test]
    fn missing_weights_say_how_to_pass_them() {
        let text = reason(TransNetV2Detector::open_with(None, None));
        assert!(text.contains("--transnet-weights"), "{text}");
        assert!(text.contains("REELCTL_TRANSNET_WEIGHTS"), "{text}");
        assert!(text.contains("not vendored"), "{text}");
    }

    #[test]
    fn weights_that_are_not_on_disk_are_named() {
        let path = Path::new("/nonexistent/reelctl-test/transnetv2.onnx");
        let text = reason(TransNetV2Detector::open_with(Some(path), None));
        assert!(
            text.contains("/nonexistent/reelctl-test/transnetv2.onnx"),
            "{text}"
        );
        assert!(text.contains("not found"), "{text}");
    }

    #[test]
    fn an_unset_library_variable_says_what_to_set() {
        let weights = some_file();
        let text = reason(TransNetV2Detector::open_with(Some(&weights), None));
        assert!(text.contains(LIBRARY_ENV), "{text}");
        assert!(text.contains("is not set"), "{text}");
        assert!(text.contains("brew install onnxruntime"), "{text}");
    }

    #[test]
    fn an_empty_library_variable_is_an_error() {
        let weights = some_file();
        let text = reason(TransNetV2Detector::open_with(
            Some(&weights),
            Some(OsStr::new("")),
        ));
        assert!(text.contains("is empty"), "{text}");
    }

    #[test]
    fn a_library_path_that_is_not_a_file_is_named() {
        let weights = some_file();
        let library = no_library();
        let text = reason(TransNetV2Detector::open_with(
            Some(&weights),
            Some(&library),
        ));
        assert!(
            text.contains("/nonexistent/reelctl-test/libonnxruntime.dylib"),
            "{text}"
        );
        assert!(text.contains("not a file"), "{text}");
    }

    #[test]
    fn a_file_that_is_not_a_library_is_an_error_not_a_panic() {
        // Cargo.toml exists but is not a shared library: the loader refuses it.
        let weights = some_file();
        let library = some_file();
        let text = reason(TransNetV2Detector::open_with(
            Some(&weights),
            Some(library.as_os_str()),
        ));
        assert!(text.contains("ONNX Runtime library"), "{text}");
        assert!(text.contains(LIBRARY_ENV), "{text}");
    }

    #[test]
    fn scene_never_looks_at_the_weights_or_the_library() {
        let selected = select_with(CutDetectorChoice::Scene, None, None).expect("select");
        assert!(matches!(selected, SelectedDetector::Scene(_, None)));
    }

    #[test]
    fn transnet_without_what_it_needs_is_an_error() {
        let weights = some_file();
        let library = no_library();
        for (weights, library) in [
            (None, None),
            (Some(weights.as_path()), None),
            (Some(weights.as_path()), Some(library.as_os_str())),
        ] {
            match select_with(CutDetectorChoice::TransNet, weights, library) {
                Err(DecompileError::CutDetectorUnavailable { reason }) => {
                    assert!(!reason.is_empty());
                }
                other => panic!("expected an error, got {other:?}"),
            }
        }
    }

    #[test]
    fn auto_without_a_weights_path_is_the_scene_score_with_no_reason() {
        let selected = select_with(CutDetectorChoice::Auto, None, None).expect("select");
        assert!(matches!(selected, SelectedDetector::Scene(_, None)));
    }

    #[test]
    fn auto_with_weights_but_no_library_falls_back_and_says_why() {
        let weights = some_file();
        let selected = select_with(CutDetectorChoice::Auto, Some(&weights), None).expect("select");
        match selected {
            SelectedDetector::Scene(_, Some(reason)) => {
                assert!(reason.contains(LIBRARY_ENV), "{reason}");
            }
            other => panic!("expected a fallback, got {other:?}"),
        }
    }

    #[test]
    fn auto_with_a_weights_path_that_is_missing_falls_back_and_says_why() {
        let weights = PathBuf::from("/nonexistent/reelctl-test/transnetv2.onnx");
        let selected = select_with(CutDetectorChoice::Auto, Some(&weights), None).expect("select");
        match selected {
            SelectedDetector::Scene(_, Some(reason)) => assert!(reason.contains("not found")),
            other => panic!("expected a fallback, got {other:?}"),
        }
    }

    fn frames_for(count: usize) -> (Vec<ScoredFrame>, ModelFrames) {
        let scored = (0..count)
            .map(|index| ScoredFrame {
                time_ms: index as u64 * 40,
                scene_score: 0.0,
            })
            .collect();
        let mut model = ModelFrames::with_capacity(count);
        for index in 0..count {
            model.push_with(|slot| {
                slot.fill(0);
                slot[0] = (index % 251) as u8;
            });
        }
        (scored, model)
    }

    #[test]
    fn a_fake_model_that_marks_two_frames_gives_two_cuts() {
        let (scored, model) = frames_for(300);
        let input = DetectorInput {
            frames: &scored,
            model_frames: Some(&model),
        };
        // Marks the frames numbered 71 and 215 (the frame values are i % 251).
        let cuts = detect_with(&input, |window| {
            Ok((0..WINDOW)
                .map(|index| {
                    let number = window[index * FRAME_BYTES] as usize;
                    if number == 71 || number == 215 {
                        0.96
                    } else {
                        0.01
                    }
                })
                .collect())
        })
        .expect("cuts");
        assert_eq!(cuts.len(), 2);
        assert_eq!(
            (cuts[0].frame_index, cuts[0].time_ms, cuts[0].interval_ms),
            (72, 2_880, 2_880)
        );
        assert_eq!(
            (cuts[1].frame_index, cuts[1].time_ms, cuts[1].interval_ms),
            (216, 8_640, 5_760)
        );
        assert!((cuts[0].scene_score - 0.96).abs() < 1e-6);
    }

    #[test]
    fn a_clip_over_the_frame_limit_is_an_error_before_the_model_runs() {
        let (scored, _) = frames_for(8);
        let mut model = ModelFrames::with_limit(8, 5);
        for index in 0..8 {
            model.push_with(|slot| slot.fill(index));
        }
        let input = DetectorInput {
            frames: &scored,
            model_frames: Some(&model),
        };
        let mut calls = 0;
        let err = detect_with(&input, |_| {
            calls += 1;
            Ok(vec![0.0; WINDOW])
        })
        .expect_err("too long");
        assert_eq!(calls, 0, "the model must not run");
        let text = err.to_string();
        assert!(text.contains("too long for TransNetV2"), "{text}");
        assert!(text.contains("--cut-detector scene"), "{text}");
        assert!(text.contains("8 frames"), "{text}");
        assert!(text.contains('5'), "{text}");
    }

    #[test]
    fn the_limit_is_checked_before_the_frame_count_mismatch() {
        // 8 model frames seen but only 3 decoded-frame scores: still "too long".
        let (scored, _) = frames_for(3);
        let mut model = ModelFrames::with_limit(8, 5);
        for index in 0..8 {
            model.push_with(|slot| slot.fill(index));
        }
        let input = DetectorInput {
            frames: &scored,
            model_frames: Some(&model),
        };
        let err = detect_with(&input, |_| Ok(vec![0.0; WINDOW])).expect_err("too long");
        assert!(err.to_string().contains("too long"), "{err}");
    }

    #[test]
    fn a_long_run_is_warned_about_with_its_frame_count_and_the_measured_rate() {
        assert_eq!(long_run_warning(LONG_RUN_FRAMES - 1), None);
        let text = long_run_warning(LONG_RUN_FRAMES).expect("a warning");
        assert!(text.contains(&format!("{LONG_RUN_FRAMES} frames")), "{text}");
        assert!(text.contains("7 to 13 ms"), "{text}");
        assert!(text.contains("--cut-detector scene"), "{text}");
        // 9,000 frames at 7 and 13 ms: 63 and 117 s.
        assert!(text.contains("63 to 117 s"), "{text}");
    }

    #[test]
    fn detecting_without_model_frames_is_an_error() {
        let (scored, _) = frames_for(10);
        let err = detect_with(&DetectorInput::scores(&scored), |_| Ok(vec![0.0; WINDOW]))
            .expect_err("no frames");
        assert!(err.to_string().contains("48x27"), "{err}");
    }

    #[test]
    fn a_frame_count_mismatch_is_an_error() {
        let (scored, _) = frames_for(10);
        let (_, model) = frames_for(9);
        let input = DetectorInput {
            frames: &scored,
            model_frames: Some(&model),
        };
        let err = detect_with(&input, |_| Ok(vec![0.0; WINDOW])).expect_err("mismatch");
        assert!(err.to_string().contains('9'), "{err}");
    }

    #[test]
    fn a_run_error_from_the_model_is_returned() {
        let (scored, model) = frames_for(10);
        let input = DetectorInput {
            frames: &scored,
            model_frames: Some(&model),
        };
        let err = detect_with(&input, |_| Err(DecompileError::Message("boom".to_string())))
            .expect_err("boom");
        assert!(err.to_string().contains("boom"));
    }
}
