//! `reelctl decompile` and `reelctl diff`.

#![warn(clippy::all)]

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use decompile::{
    decompile_file, run_whisper_worker, CutDetectorChoice, DecompileRequest, WhisperRunner,
};
use diff::compare;
use edit_dna::EditDna;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Decompile(args) => decompile(args),
        Command::Diff(args) => diff(args),
        Command::WhisperWorker(args) => whisper_worker(args),
        other => {
            let _never: std::convert::Infallible = match other {
                Command::Decompile(_) | Command::Diff(_) | Command::WhisperWorker(_) => {
                    unreachable!("Command is handled above")
                }
            };
            match _never {}
        }
    }
}

#[derive(Parser)]
#[command(
    name = "reelctl",
    version,
    about = "Decompile a reel into edit DNA and diff pacing against an outlier"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract millisecond edit DNA and timeline_view PNGs at each cut.
    Decompile(DecompileArgs),
    /// Flag dead air, static shots, and a missing 9s payoff on a draft.
    Diff(DiffArgs),
    /// Internal: runs whisper passes for a parent `reelctl`, one JSON line in
    /// and one out per pass, so a whisper.cpp abort cannot end the parent.
    #[command(hide = true)]
    WhisperWorker(WorkerArgs),
}

#[derive(Args)]
struct WorkerArgs {
    #[arg(long)]
    model: PathBuf,
    /// 16 kHz mono f32 audio written by the parent.
    #[arg(long)]
    audio: PathBuf,
    /// Load the model without DTW timestamps.
    #[arg(long)]
    no_dtw: bool,
}

#[derive(Args)]
struct DecompileArgs {
    /// Local reel to decompile.
    #[arg(long)]
    input: PathBuf,
    /// Where to write edit_dna.json.
    #[arg(long)]
    out: PathBuf,
    /// Directory for timeline_view PNGs. Defaults to `<out stem>_timeline`.
    #[arg(long)]
    filmstrip_dir: Option<PathBuf>,
    #[command(flatten)]
    signal: SignalArgs,
}

#[derive(Args)]
struct DiffArgs {
    /// Draft reel (mp4). Decompiled in-process, then compared.
    #[arg(long)]
    draft: PathBuf,
    /// Outlier edit_dna.json from a previous decompile.
    #[arg(long)]
    outlier: PathBuf,
    #[command(flatten)]
    signal: SignalArgs,
}

#[derive(Args)]
struct SignalArgs {
    /// TransNetV2 ONNX weights. Used by --cut-detector auto and transnet.
    #[arg(long, env = "REELCTL_TRANSNET_WEIGHTS")]
    transnet_weights: Option<PathBuf>,
    /// Cut detector. `auto` uses TransNetV2 when the weight file exists.
    #[arg(long, value_enum, default_value_t = DetectorArg::Auto)]
    cut_detector: DetectorArg,
    /// ggml whisper.cpp model. With one, words come from whisper.cpp; without
    /// one, from voiced bursts.
    #[arg(long, env = "REELCTL_WHISPER_MODEL")]
    whisper_model: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DetectorArg {
    Auto,
    Scene,
    Transnet,
}

impl DetectorArg {
    fn choice(self) -> CutDetectorChoice {
        match self {
            Self::Auto => CutDetectorChoice::Auto,
            Self::Scene => CutDetectorChoice::Scene,
            Self::Transnet => CutDetectorChoice::TransNet,
            other => {
                let _never: std::convert::Infallible = match other {
                    Self::Auto | Self::Scene | Self::Transnet => {
                        unreachable!("DetectorArg is handled above")
                    }
                };
                match _never {}
            }
        }
    }
}

/// whisper runs in a child `reelctl whisper-worker`. If this executable
/// cannot be found, it runs in this process instead.
fn whisper_runner() -> WhisperRunner {
    match std::env::current_exe() {
        Ok(program) => WhisperRunner::Worker {
            program,
            args: vec!["whisper-worker".to_string()],
        },
        Err(_) => WhisperRunner::InProcess,
    }
}

fn whisper_worker(args: WorkerArgs) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    run_whisper_worker(
        &args.model,
        &args.audio,
        !args.no_dtw,
        stdin.lock(),
        stdout.lock(),
    )
    .context("whisper worker")?;
    Ok(())
}

fn decompile(args: DecompileArgs) -> Result<()> {
    let request = DecompileRequest {
        input: args.input,
        output: Some(args.out.clone()),
        transnet_weights: args.signal.transnet_weights,
        whisper_model: args.signal.whisper_model,
        whisper_runner: whisper_runner(),
        cut_detector: args.signal.cut_detector.choice(),
    };
    let output = decompile_file(&request).context("decompile")?;
    let dir = filmstrip_dir(&args.out, args.filmstrip_dir);
    let paths = filmstrip::write_boundary_views(&dir, &output.boundaries, &output.dna.words)
        .context("write timeline views")?;
    print_timing(&output);
    if std::env::var_os("REELCTL_PROFILE").is_some() {
        eprintln!(
            "profile decode_ms={} analyze_ms={} silence_ms={} loudness_ms={} asr_ms={} asr_first_ms={} asr_second_ms={} json_ms={} filmstrips={}",
            output.profile.decode.as_millis(),
            output.profile.analyze.as_millis(),
            output.profile.silence.as_millis(),
            output.profile.loudness.as_millis(),
            output.profile.asr.as_millis(),
            output.profile.asr_first_pass.as_millis(),
            output.profile.asr_second_pass.as_millis(),
            output.profile.json.as_millis(),
            paths.len()
        );
    }
    Ok(())
}

fn diff(args: DiffArgs) -> Result<()> {
    let request = DecompileRequest {
        input: args.draft,
        output: None,
        transnet_weights: args.signal.transnet_weights,
        whisper_model: args.signal.whisper_model,
        whisper_runner: whisper_runner(),
        cut_detector: args.signal.cut_detector.choice(),
    };
    let draft = decompile_file(&request).context("decompile draft")?;
    let file =
        File::open(&args.outlier).with_context(|| format!("open {}", args.outlier.display()))?;
    let outlier: EditDna = serde_json::from_reader(file).context("parse outlier edit_dna.json")?;
    let report = compare(&draft.dna, &outlier);
    println!(
        "{}",
        serde_json::to_string_pretty(&report).context("format diff")?
    );
    Ok(())
}

fn print_timing(output: &decompile::DecompileOutput) {
    let under = output.signal < Duration::from_millis(500);
    println!(
        "signal_ms={} asr_ms={} asr_us={} structural_under_500ms={} cuts={} silence_gaps={} duration_ms={}",
        output.dna.signal_ms,
        output.dna.asr_ms,
        output.asr.as_micros(),
        under,
        output.dna.cuts.len(),
        output.dna.silence_gaps.len(),
        output.dna.duration_ms
    );
}

fn filmstrip_dir(out: &std::path::Path, explicit: Option<PathBuf>) -> PathBuf {
    if let Some(dir) = explicit {
        return dir;
    }
    let stem = out
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("edit_dna");
    let mut dir = out.to_path_buf();
    dir.set_file_name(format!("{stem}_timeline"));
    dir
}
