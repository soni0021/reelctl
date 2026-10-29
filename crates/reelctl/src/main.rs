//! `reelctl decompile`, `reelctl diff`, `reelctl understand`, and `reelctl look`.

#![warn(clippy::all)]

use std::fs::File;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use decompile::{
    decompile_file, run_whisper_worker, CutDetectorChoice, DecoderSetting, DecompileRequest,
    WhisperRunner,
};
use diff::{compare_many, render_text, NamedReel};
use edit_dna::EditDna;

mod captions_draw;
mod card;
mod compositor_cmd;
mod dataset_cmd;
mod look_cmd;
mod make_reels_cmd;
mod overlay_cmd;
mod render_backend;
mod smelter_backend;
mod smelter_client;
#[cfg(test)]
mod smelter_fake;
mod timeline_cmd;
mod understand_cmd;

fn main() {
    let result = run();
    // Before any exit: a live ONNX Runtime environment makes the process abort at exit.
    decompile::release_onnx_runtime();
    compositor::release_runtime();
    if let Err(err) = result {
        // Some failures have their own exit code (3: no API key, 4: over the cost cap).
        if let Some(exit) = err.downcast_ref::<understand_cmd::ExitWith>() {
            eprintln!("error: {exit}");
            std::process::exit(exit.code);
        }
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Decompile(args) => decompile(args),
        Command::Diff(args) => diff(args),
        Command::Understand(args) => understand_cmd::understand(args),
        Command::Look(args) => look_cmd::look(args),
        Command::Timeline(command) => timeline_cmd::timeline(command),
        Command::Transform(command) => timeline_cmd::transform(command),
        Command::Funnel(command) => timeline_cmd::funnel(command),
        Command::Overlay(command) => overlay_cmd::overlay(command),
        Command::Render(args) => timeline_cmd::render(args),
        Command::MakeReels(args) => make_reels_cmd::make_reels(args),
        Command::Qa(command) => timeline_cmd::qa(command),
        Command::Compositor(command) => compositor_cmd::compositor(command),
        Command::Dataset(command) => dataset_cmd::run_dataset(command),
        Command::Eval(args) => dataset_cmd::run_eval(args),
        Command::WhisperWorker(args) => whisper_worker(args),
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
    /// Pick Reel-sized clips from a long video and label its cuts, with a hosted
    /// vision-language model (needs OPENROUTER_API_KEY).
    Understand(understand_cmd::UnderstandArgs),
    /// Write a .cube 3D LUT that moves a draft's colour toward an outlier's, by
    /// per-channel histogram matching of sampled frames.
    Look(look_cmd::LookArgs),
    /// Make and edit an OpenTimelineIO project: `init`, `cut-silence`.
    Timeline(timeline_cmd::TimelineCommand),
    /// Change the picture of a project: `punch-in`.
    Transform(timeline_cmd::TransformCommand),
    /// Add a funnel step to a project: `attach-bridge`.
    Funnel(timeline_cmd::FunnelCommand),
    /// Lay a chart drawn by manimgx over a project: `add-chart`, `remove`.
    Overlay(overlay_cmd::OverlayCommand),
    /// Render a project to an mp4 with ffmpeg.
    Render(timeline_cmd::RenderArgs),
    /// One long video in, finished vertical reels out: decompile, understand (paid,
    /// after a yes), then for each clip cut, pace, reframe, card, caption and render.
    MakeReels(make_reels_cmd::MakeReelsArgs),
    /// Check a project: `inspect-boundaries`.
    Qa(timeline_cmd::QaCommand),
    /// Cut a presenter out of a video: `isolate-presenter`.
    Compositor(compositor_cmd::CompositorCommand),
    /// Build instruction-tuning records from decompiled reels: `build`.
    Dataset(dataset_cmd::DatasetCommand),
    /// Score a model's answers against measured truth (an understanding.json or a
    /// dataset). Offline: it reads an answers file and never calls a model.
    Eval(dataset_cmd::EvalArgs),
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
    /// Outlier edit_dna.json from a previous decompile. Give it one to three
    /// times.
    #[arg(long, required = true)]
    outlier: Vec<PathBuf>,
    /// How to print the report.
    #[arg(long, value_enum, default_value_t = FormatArg::Json)]
    format: FormatArg,
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
    /// Video decoder. `auto` decodes HEVC on the Apple media engine (VideoToolbox)
    /// where it works, and everything else in software; `software` forces software.
    #[arg(long, value_enum, default_value_t = DecoderArg::Auto)]
    decoder: DecoderArg,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FormatArg {
    Json,
    Text,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DecoderArg {
    Auto,
    Software,
}

impl DecoderArg {
    fn setting(self) -> DecoderSetting {
        match self {
            Self::Auto => DecoderSetting::Auto,
            Self::Software => DecoderSetting::Software,
        }
    }
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
        decoder: args.signal.decoder.setting(),
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

/// The most outliers `diff` compares a draft with.
const MAX_OUTLIERS: usize = 3;

/// A reel's name in the report: its file name without the directory and without
/// a trailing `.edit_dna.json` or extension.
fn reel_name(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("reel");
    match name.strip_suffix(".edit_dna.json") {
        Some(stem) => stem.to_string(),
        None => path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or(name)
            .to_string(),
    }
}

fn diff(args: DiffArgs) -> Result<()> {
    if args.outlier.len() > MAX_OUTLIERS {
        // An argument error, so it exits with code 2 like clap's own, and shows
        // `diff`'s usage.
        let message = format!(
            "at most three --outlier files are allowed, got {}",
            args.outlier.len()
        );
        let mut command = Cli::command();
        let error = match command.find_subcommand_mut("diff") {
            Some(diff) => diff.error(ErrorKind::TooManyValues, message),
            None => Cli::command().error(ErrorKind::TooManyValues, message),
        };
        error.exit();
    }
    let draft_name = reel_name(&args.draft);
    let request = DecompileRequest {
        input: args.draft,
        output: None,
        transnet_weights: args.signal.transnet_weights,
        whisper_model: args.signal.whisper_model,
        whisper_runner: whisper_runner(),
        cut_detector: args.signal.cut_detector.choice(),
        decoder: args.signal.decoder.setting(),
    };
    let draft = decompile_file(&request).context("decompile draft")?;
    let mut outliers: Vec<(String, EditDna)> = Vec::with_capacity(args.outlier.len());
    for path in &args.outlier {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let dna: EditDna = serde_json::from_reader(file)
            .with_context(|| format!("parse outlier {}", path.display()))?;
        outliers.push((reel_name(path), dna));
    }
    let named: Vec<NamedReel<'_>> = outliers
        .iter()
        .map(|(name, dna)| NamedReel { name, dna })
        .collect();
    let report = compare_many(
        NamedReel {
            name: &draft_name,
            dna: &draft.dna,
        },
        &named,
    );
    match args.format {
        FormatArg::Json => println!(
            "{}",
            serde_json::to_string_pretty(&report).context("format diff")?
        ),
        FormatArg::Text => print!("{}", render_text(&report)),
    }
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::reel_name;

    #[test]
    fn a_reel_is_named_by_its_file_without_directory_or_suffix() {
        assert_eq!(reel_name(Path::new("out/reel.edit_dna.json")), "reel");
        assert_eq!(reel_name(Path::new("dir/x.mp4")), "x");
        // Dots inside the name stay.
        assert_eq!(reel_name(Path::new("a.v1.edit_dna.json")), "a.v1");
        assert_eq!(reel_name(Path::new("my.video.mp4")), "my.video");
        assert_eq!(reel_name(Path::new("plain")), "plain");
    }
}
