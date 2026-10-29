//! RobustVideoMatting through the `ort` crate.
//!
//! ONNX Runtime is not linked at build time: `ort` is built with `load-dynamic`
//! and this module loads the library named in `ORT_DYLIB_PATH` when a run opens
//! the model, so the default build and `cargo test` need no download and no
//! library. The weights are a file the user supplies (`--weights`); neither is
//! vendored.
//!
//! Never call `ort::init()` or let `ort` load the library lazily: its default
//! loader panics when the library is missing. The path is resolved here and given
//! to `ort::init_from`, which returns an error. `ort` allows one environment per
//! process, for good, and ONNX Runtime 1.21 aborts at exit (status 134) if one is
//! still alive, so the process must call [`release_runtime`] before it exits.
//! This environment is the compositor's own: it is separate from the one
//! `decompile` keeps for TransNetV2, and one process must not use both.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use half::f16;
use ort::environment::Environment;
use ort::session::Session;
use ort::value::{DynValue, Tensor, TensorElementType};

use crate::error::CompositorError;
use crate::model::{MattingModel, ModelOutput};
use crate::tensor::{FrameRgb, RecurrentState, Tensor4};

/// The environment variable that names the ONNX Runtime library (`ort`'s own name).
pub const LIBRARY_ENV: &str = "ORT_DYLIB_PATH";

const INPUT_FRAME: &str = "src";
const INPUT_STATES: [&str; 4] = ["r1i", "r2i", "r3i", "r4i"];
const INPUT_RATIO: &str = "downsample_ratio";
const OUTPUT_FOREGROUND: &str = "fgr";
const OUTPUT_ALPHA: &str = "pha";
const OUTPUT_STATES: [&str; 4] = ["r1o", "r2o", "r3o", "r4o"];

/// The element type the model's frame and states use. The ratio is `float32` in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    Float32,
    Float16,
}

impl Precision {
    pub fn name(self) -> &'static str {
        match self {
            Self::Float32 => "float32",
            Self::Float16 => "float16",
        }
    }
}

/// An opened RVM model, ready to run frames. Opening one checks the weights file,
/// the library and the model's signature, so a bad setup fails before the video
/// is decoded.
pub struct RvmModel {
    session: Session,
    precision: Precision,
}

impl std::fmt::Debug for RvmModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RvmModel")
            .field("precision", &self.precision)
            .finish_non_exhaustive()
    }
}

impl RvmModel {
    /// Open the weights with the library named by `ORT_DYLIB_PATH`.
    pub fn open(weights: Option<&Path>) -> Result<Self, CompositorError> {
        let library = std::env::var_os(LIBRARY_ENV);
        Self::open_with(weights, library.as_deref())
    }

    /// Open the weights with the ONNX Runtime library at `library` (the value of
    /// `ORT_DYLIB_PATH`). Every failure is a setup error: the reason is ready to show.
    pub fn open_with(
        weights: Option<&Path>,
        library: Option<&OsStr>,
    ) -> Result<Self, CompositorError> {
        let weights = check_weights(weights).map_err(CompositorError::Setup)?;
        let environment = environment(library).map_err(CompositorError::Setup)?;
        let session = Session::builder(&environment)
            .and_then(|mut builder| builder.commit_from_file(weights))
            .map_err(|error| {
                CompositorError::setup(format!(
                    "RobustVideoMatting weights at {} could not be loaded by ONNX Runtime: {error}",
                    weights.display()
                ))
            })?;
        let precision = check_signature(&ports(&session)).map_err(CompositorError::Setup)?;
        Ok(Self { session, precision })
    }

    pub fn precision(&self) -> Precision {
        self.precision
    }
}

/// A named input or output of the model, with its element type if it is a tensor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Port {
    pub name: String,
    pub element: Option<TensorElementType>,
}

/// The model's inputs then outputs, as `(inputs, outputs)`.
pub(crate) struct Ports {
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
}

fn ports(session: &Session) -> Ports {
    let port = |name: &str, dtype: &ort::value::ValueType| Port {
        name: name.to_string(),
        element: dtype.tensor_type(),
    };
    Ports {
        inputs: session
            .inputs()
            .iter()
            .map(|input| port(input.name(), input.dtype()))
            .collect(),
        outputs: session
            .outputs()
            .iter()
            .map(|output| port(output.name(), output.dtype()))
            .collect(),
    }
}

/// Check that `ports` are RobustVideoMatting's, and say which precision it is.
pub(crate) fn check_signature(ports: &Ports) -> Result<Precision, String> {
    let find = |list: &[Port],
                name: &str,
                kind: &str|
     -> Result<Option<TensorElementType>, String> {
        list.iter()
            .find(|port| port.name == name)
            .map(|port| port.element)
            .ok_or_else(|| {
                let names: Vec<&str> = list.iter().map(|port| port.name.as_str()).collect();
                format!(
                    "The model is not RobustVideoMatting (rvm_mobilenetv3_*.onnx): it has no {kind} \
named {name} (it has {names:?})"
                )
            })
    };
    let frame = find(&ports.inputs, INPUT_FRAME, "input")?;
    let precision = match frame {
        Some(TensorElementType::Float32) => Precision::Float32,
        Some(TensorElementType::Float16) => Precision::Float16,
        other => {
            return Err(format!(
                "The model's {INPUT_FRAME} input is {other:?}, not float32 or float16"
            ))
        }
    };
    let expected = frame;
    for name in INPUT_STATES {
        if find(&ports.inputs, name, "input")? != expected {
            return Err(format!(
                "The model's {name} input is not the same type as {INPUT_FRAME}"
            ));
        }
    }
    if find(&ports.inputs, INPUT_RATIO, "input")? != Some(TensorElementType::Float32) {
        return Err(format!("The model's {INPUT_RATIO} input is not float32"));
    }
    for name in [OUTPUT_FOREGROUND, OUTPUT_ALPHA]
        .into_iter()
        .chain(OUTPUT_STATES)
    {
        if find(&ports.outputs, name, "output")? != expected {
            return Err(format!(
                "The model's {name} output is not the same type as the {INPUT_FRAME} input"
            ));
        }
    }
    Ok(precision)
}

fn run_error(error: ort::Error) -> CompositorError {
    CompositorError::run(format!("RobustVideoMatting failed while running: {error}"))
}

/// An input tensor in the model's precision.
fn tensor_in(
    precision: Precision,
    shape: [usize; 4],
    data: &[f32],
) -> Result<DynValue, CompositorError> {
    match precision {
        Precision::Float32 => Tensor::from_array((shape, data.to_vec()))
            .map(Tensor::into_dyn)
            .map_err(run_error),
        Precision::Float16 => {
            let halves: Vec<f16> = data.iter().map(|value| f16::from_f32(*value)).collect();
            Tensor::from_array((shape, halves))
                .map(Tensor::into_dyn)
                .map_err(run_error)
        }
    }
}

/// An output as its dimensions and `f32` values.
fn tensor_out(
    value: &DynValue,
    precision: Precision,
) -> Result<(Vec<usize>, Vec<f32>), CompositorError> {
    let dims = |shape: &[i64]| shape.iter().map(|dim| *dim as usize).collect::<Vec<_>>();
    match precision {
        Precision::Float32 => {
            let (shape, data) = value.try_extract_tensor::<f32>().map_err(run_error)?;
            Ok((dims(shape), data.to_vec()))
        }
        Precision::Float16 => {
            let (shape, data) = value.try_extract_tensor::<f16>().map_err(run_error)?;
            Ok((
                dims(shape),
                data.iter().map(|value| value.to_f32()).collect(),
            ))
        }
    }
}

impl MattingModel for RvmModel {
    fn matte(
        &mut self,
        frame: &FrameRgb,
        ratio: f32,
        state: &RecurrentState,
    ) -> Result<ModelOutput, CompositorError> {
        let precision = self.precision;
        let ratio_tensor = Tensor::from_array(([1usize], vec![ratio])).map_err(run_error)?;
        let mut inputs: Vec<(&str, DynValue)> = vec![
            (
                INPUT_FRAME,
                tensor_in(precision, [1, 3, frame.height, frame.width], &frame.data)?,
            ),
            (INPUT_RATIO, ratio_tensor.into_dyn()),
        ];
        for (name, tensor) in INPUT_STATES.iter().zip(&state.0) {
            inputs.push((name, tensor_in(precision, tensor.shape, &tensor.data)?));
        }
        let outputs = self.session.run(inputs).map_err(run_error)?;
        let take = |name: &str| -> Result<(Vec<usize>, Vec<f32>), CompositorError> {
            let value = outputs.get(name).ok_or_else(|| {
                CompositorError::run(format!("RobustVideoMatting gave no output named {name}"))
            })?;
            tensor_out(value, precision)
        };
        let (_, fgr) = take(OUTPUT_FOREGROUND)?;
        let (_, pha) = take(OUTPUT_ALPHA)?;
        let mut states = Vec::with_capacity(4);
        for name in OUTPUT_STATES {
            let (dims, data) = take(name)?;
            let shape: [usize; 4] = dims.as_slice().try_into().map_err(|_| {
                CompositorError::run(format!(
                    "RobustVideoMatting's {name} has {} dimensions, not 4",
                    dims.len()
                ))
            })?;
            states.push(Tensor4::new(shape, data)?);
        }
        let [r1, r2, r3, r4]: [Tensor4; 4] = states
            .try_into()
            .map_err(|_| CompositorError::run("RobustVideoMatting returned the wrong states"))?;
        Ok(ModelOutput {
            fgr,
            pha,
            state: RecurrentState([r1, r2, r3, r4]),
        })
    }
}

/// The weights path, if it names a file.
fn check_weights(weights: Option<&Path>) -> Result<&Path, String> {
    match weights {
        None => Err("\
RobustVideoMatting ONNX weights were not provided. Pass --weights or set \
REELCTL_RVM_WEIGHTS to the path of rvm_mobilenetv3_fp32.onnx or \
rvm_mobilenetv3_fp16.onnx (from the PeterL1n/RobustVideoMatting releases). The \
weights are not vendored or downloaded."
            .to_string()),
        Some(path) if path.is_dir() => Err(format!(
            "RobustVideoMatting weights path {} is a directory, not a file. Pass the path of \
the .onnx file itself (--weights or REELCTL_RVM_WEIGHTS).",
            path.display()
        )),
        Some(path) if !path.is_file() => Err(format!(
            "RobustVideoMatting weights were not found at {}. The path is configurable \
(--weights or REELCTL_RVM_WEIGHTS), but the file is not on disk and is not vendored or downloaded.",
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

/// Release the shared ONNX Runtime environment. A process that ran the model must
/// call this before it exits: with the environment still alive at exit, ONNX
/// Runtime 1.21's static destructors abort the process and the exit status becomes
/// 134 after the output was written. `ort` builds one environment per process for
/// good, so the model cannot be opened again afterwards.
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
        .map_err(|error| {
            format!(
                "RobustVideoMatting needs the ONNX Runtime library: {error}; {}",
                library_hint()
            )
        })?
        .build()
        .map_err(|error| format!("ONNX Runtime could not start: {error}"))?;
    *slot = Some(environment.clone());
    Ok(environment)
}

/// The library file named by the environment variable's value.
fn library_path(library: Option<&OsStr>) -> Result<PathBuf, String> {
    match library {
        None => Err(format!(
            "RobustVideoMatting needs the ONNX Runtime library: {LIBRARY_ENV} is not set; {}",
            library_hint()
        )),
        Some(value) if value.is_empty() => Err(format!(
            "RobustVideoMatting needs the ONNX Runtime library: {LIBRARY_ENV} is empty; {}",
            library_hint()
        )),
        Some(value) => {
            let path = PathBuf::from(value);
            // `ort` tries a relative path beside the executable before the
            // working directory, so it could load a different file than the
            // one checked below.
            if !path.is_absolute() {
                return Err(format!(
                    "RobustVideoMatting needs the ONNX Runtime library: {LIBRARY_ENV} is {}, which is \
not an absolute path (a relative path may load a different file than the one checked); {}",
                    path.display(),
                    library_hint()
                ));
            }
            if path.is_file() {
                Ok(path)
            } else {
                Err(format!(
                    "RobustVideoMatting needs the ONNX Runtime library: {LIBRARY_ENV} names {}, \
which is not a file; {}",
                    path.display(),
                    library_hint()
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(name: &str, element: TensorElementType) -> Port {
        Port {
            name: name.to_string(),
            element: Some(element),
        }
    }

    /// RVM's signature at `element`.
    fn rvm_ports(element: TensorElementType) -> Ports {
        let mut inputs = vec![port("src", element)];
        inputs.extend(INPUT_STATES.iter().map(|name| port(name, element)));
        inputs.push(port("downsample_ratio", TensorElementType::Float32));
        let mut outputs = vec![port("fgr", element), port("pha", element)];
        outputs.extend(OUTPUT_STATES.iter().map(|name| port(name, element)));
        Ports { inputs, outputs }
    }

    #[test]
    fn both_rvm_files_are_recognised() {
        assert_eq!(
            check_signature(&rvm_ports(TensorElementType::Float32)),
            Ok(Precision::Float32)
        );
        assert_eq!(
            check_signature(&rvm_ports(TensorElementType::Float16)),
            Ok(Precision::Float16)
        );
        assert_eq!(Precision::Float32.name(), "float32");
        assert_eq!(Precision::Float16.name(), "float16");
    }

    #[test]
    fn a_model_missing_an_input_or_output_is_not_rvm() {
        let mut ports = rvm_ports(TensorElementType::Float32);
        ports.inputs.retain(|port| port.name != "r3i");
        let error = check_signature(&ports).unwrap_err();
        assert!(error.contains("not RobustVideoMatting"), "{error}");
        assert!(error.contains("r3i"), "{error}");

        let mut ports = rvm_ports(TensorElementType::Float32);
        ports.outputs.retain(|port| port.name != "pha");
        assert!(check_signature(&ports).unwrap_err().contains("pha"));

        let mut ports = rvm_ports(TensorElementType::Float32);
        ports.outputs.retain(|port| port.name != "r4o");
        assert!(check_signature(&ports).unwrap_err().contains("r4o"));
    }

    #[test]
    fn mixed_or_unsupported_element_types_are_refused() {
        // A float16 frame with a float32 state.
        let mut ports = rvm_ports(TensorElementType::Float16);
        ports.inputs[2] = port("r2i", TensorElementType::Float32);
        assert!(check_signature(&ports).unwrap_err().contains("r2i"));
        // A float64 model.
        let ports = rvm_ports(TensorElementType::Float64);
        assert!(check_signature(&ports)
            .unwrap_err()
            .contains("float32 or float16"));
        // An output of another type than the input (fgr, then a state).
        for name in ["fgr", "r3o"] {
            let mut ports = rvm_ports(TensorElementType::Float16);
            let at = ports.outputs.iter().position(|p| p.name == name).unwrap();
            ports.outputs[at] = port(name, TensorElementType::Float32);
            assert!(
                check_signature(&ports).unwrap_err().contains(name),
                "{name}"
            );
        }
        // The ratio must be float32 even in the float16 file.
        let mut ports = rvm_ports(TensorElementType::Float16);
        let last = ports.inputs.len() - 1;
        ports.inputs[last] = port("downsample_ratio", TensorElementType::Float16);
        assert!(check_signature(&ports)
            .unwrap_err()
            .contains("downsample_ratio"));
    }

    #[test]
    fn weights_must_be_a_file() {
        let none = check_weights(None).unwrap_err();
        assert!(none.contains("--weights") && none.contains("REELCTL_RVM_WEIGHTS"));
        let dir = std::env::temp_dir();
        let error = check_weights(Some(&dir)).unwrap_err();
        assert!(error.contains("is a directory"), "{error}");
        let missing = dir.join("no-such-rvm-weights.onnx");
        let error = check_weights(Some(&missing)).unwrap_err();
        assert!(error.contains("were not found"), "{error}");
        assert!(error.contains("no-such-rvm-weights.onnx"), "{error}");
        let file = std::env::current_exe().unwrap();
        assert_eq!(check_weights(Some(&file)).unwrap(), file);
    }

    #[test]
    fn the_library_must_be_an_absolute_existing_file() {
        let unset = library_path(None).unwrap_err();
        assert!(
            unset.contains("is not set") && unset.contains(LIBRARY_ENV),
            "{unset}"
        );
        let empty = library_path(Some(OsStr::new(""))).unwrap_err();
        assert!(empty.contains("is empty"), "{empty}");
        let relative = library_path(Some(OsStr::new("lib/libonnxruntime.dylib"))).unwrap_err();
        assert!(relative.contains("not an absolute path"), "{relative}");
        let dir = std::env::temp_dir();
        let directory = library_path(Some(dir.as_os_str())).unwrap_err();
        assert!(directory.contains("not a file"), "{directory}");
        let file = std::env::current_exe().unwrap();
        assert_eq!(library_path(Some(file.as_os_str())).unwrap(), file);
    }

    #[test]
    fn a_missing_setup_is_a_setup_error_and_does_not_load_anything() {
        // Weights are checked first, then the library; neither is touched on failure.
        let error = RvmModel::open_with(None, None).unwrap_err();
        assert!(error.is_setup());
        assert!(error.to_string().contains("--weights"));
        let file = std::env::current_exe().unwrap();
        let error = RvmModel::open_with(Some(&file), None).unwrap_err();
        assert!(error.is_setup());
        assert!(error.to_string().contains(LIBRARY_ENV));
    }
}
