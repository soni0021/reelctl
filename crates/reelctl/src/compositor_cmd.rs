//! `reelctl compositor isolate-presenter`: cut a presenter out of a video with
//! RobustVideoMatting and write them over a background (or the matte alone).
//!
//! Exit codes: 2 for a command or setup that is unusable (a missing or mistyped
//! input, background or output, HDR input, weights or `ORT_DYLIB_PATH` that are
//! missing or do not load); 1 for a failure while running (`ffmpeg` or the model
//! failed, the output cannot be written). Nothing is written to `--out` unless
//! the whole run succeeds. The command runs the `ffprobe` and `ffmpeg` command line
//! tools and, with the weights, ONNX Runtime from `ORT_DYLIB_PATH`; it does not use
//! the network.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::{Args, Subcommand, ValueEnum};
use compositor::{
    parse_hex_colour, prepare, BackgroundSpec, CompositorError, IsolateRequest, OutputMode,
    RvmModel,
};

use crate::understand_cmd::ExitWith;

#[derive(Args)]
pub struct CompositorCommand {
    #[command(subcommand)]
    command: CompositorSub,
}

#[derive(Subcommand)]
enum CompositorSub {
    /// Cut the presenter out of a video with RobustVideoMatting and put them over
    /// a colour, an image or a video (needs the RVM ONNX weights and ONNX Runtime).
    IsolatePresenter(IsolateArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModeArg {
    /// The presenter over --background (mp4).
    Composite,
    /// The alpha matte alone, as a grey video (mp4).
    Matte,
    /// The presenter with its alpha, ProRes 4444 (mov).
    Rgba,
}

impl ModeArg {
    fn mode(self) -> OutputMode {
        match self {
            Self::Composite => OutputMode::Composite,
            Self::Matte => OutputMode::Matte,
            Self::Rgba => OutputMode::Rgba,
        }
    }
}

#[derive(Args)]
struct IsolateArgs {
    /// Video with the presenter (SDR; HDR must be converted first).
    #[arg(long)]
    input: PathBuf,
    /// What goes behind the presenter: a #rrggbb colour, or an image or video
    /// (scaled to cover the frame, cropped at the centre, looped). Required for
    /// --mode composite; not used otherwise.
    #[arg(long)]
    background: Option<String>,
    /// RobustVideoMatting ONNX weights (rvm_mobilenetv3_fp32.onnx or _fp16.onnx).
    /// ONNX Runtime comes from the ORT_DYLIB_PATH environment variable.
    #[arg(long, env = "REELCTL_RVM_WEIGHTS")]
    weights: Option<PathBuf>,
    /// Where to write the result.
    #[arg(long)]
    out: PathBuf,
    /// What to write.
    #[arg(long, value_enum, default_value_t = ModeArg::Composite)]
    mode: ModeArg,
    /// The model's downsample ratio, above 0 and at most 1. Default: 512 divided
    /// by the longest side, at most 1.
    #[arg(long)]
    downsample_ratio: Option<f32>,
}

pub fn compositor(command: CompositorCommand) -> Result<()> {
    match command.command {
        CompositorSub::IsolatePresenter(args) => isolate_presenter(args),
    }
}

/// A setup error exits with code 2; a failure while running with 1.
pub(crate) fn exit_for(error: CompositorError) -> anyhow::Error {
    match error {
        CompositorError::Setup(message) => anyhow::Error::new(ExitWith { code: 2, message }),
        CompositorError::Run(message) => anyhow::anyhow!(message),
    }
}

fn usage(message: impl Into<String>) -> anyhow::Error {
    exit_for(CompositorError::setup(message))
}

/// `#rrggbb` is a colour; anything else must be an existing file.
fn background_spec(text: &str) -> Result<BackgroundSpec> {
    if text.starts_with('#') {
        return parse_hex_colour(text)
            .map(BackgroundSpec::Colour)
            .map_err(exit_for);
    }
    let path = Path::new(text);
    if path.is_file() {
        Ok(BackgroundSpec::File(path.to_path_buf()))
    } else {
        Err(usage(format!(
            "--background {text} is neither a #rrggbb colour nor an existing image or video file"
        )))
    }
}

fn isolate_presenter(args: IsolateArgs) -> Result<()> {
    let background = args
        .background
        .as_deref()
        .map(background_spec)
        .transpose()?;
    let request = IsolateRequest {
        input: args.input,
        background,
        out: args.out,
        mode: args.mode.mode(),
        ratio: args.downsample_ratio,
    };
    // The input, background and output are checked before the weights are opened,
    // and the weights before the first frame is decoded.
    let prepared = prepare(&request).map_err(exit_for)?;
    let mut model = RvmModel::open(args.weights.as_deref()).map_err(exit_for)?;
    let precision = model.precision().name();
    let report = prepared.run(&mut model).map_err(exit_for)?;
    let model_ms = report.model_time.as_secs_f64() * 1000.0;
    println!(
        "isolate-presenter: out={} mode={} frames={} size={}x{} fps={:.3} downsample_ratio={:.4} \
model={precision} model_ms={:.0} ms_per_frame={:.1} total_ms={}",
        report.out.display(),
        report.mode.name(),
        report.frames,
        report.width,
        report.height,
        report.fps,
        report.ratio,
        model_ms,
        model_ms / report.frames.max(1) as f64,
        report.total_time.as_millis()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(error: &anyhow::Error) -> Option<i32> {
        error.downcast_ref::<ExitWith>().map(|exit| exit.code)
    }

    #[test]
    fn a_colour_with_a_hash_is_a_colour() {
        match background_spec("#ff8000").unwrap() {
            BackgroundSpec::Colour(colour) => assert_eq!(colour, [255, 128, 0]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_bad_colour_and_a_missing_file_exit_with_2() {
        let bad = background_spec("#12345").unwrap_err();
        assert_eq!(code(&bad), Some(2));
        let missing = background_spec("/no/such/background.png").unwrap_err();
        assert_eq!(code(&missing), Some(2));
        assert!(missing.to_string().contains("#rrggbb"));
        // A colour without its # is a path that does not exist.
        assert_eq!(code(&background_spec("ff8000").unwrap_err()), Some(2));
    }

    #[test]
    fn an_existing_file_is_a_file_background() {
        let file = std::env::current_exe().unwrap();
        match background_spec(file.to_str().unwrap()).unwrap() {
            BackgroundSpec::File(path) => assert_eq!(path, file),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn setup_errors_exit_with_2_and_run_errors_with_1() {
        assert_eq!(code(&exit_for(CompositorError::setup("a"))), Some(2));
        // A run error is a plain error, which `main` turns into exit 1.
        assert_eq!(code(&exit_for(CompositorError::run("a"))), None);
    }
}
