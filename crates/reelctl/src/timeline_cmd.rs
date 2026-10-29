//! `reelctl timeline|transform|funnel|render|qa`: edit an OpenTimelineIO project and
//! render it.
//!
//! Exit codes: 2 for a bad argument, an unreadable or invalid project or DNA, a media
//! path that is not a plain local file, or a span outside the timeline; 1 for a
//! failed ffmpeg (or a missing one) or an output that cannot be written. The project
//! is read before anything is written, and written atomically (a temporary file in
//! the same directory, then a rename), so `--out` may be `--project`. `render` and
//! `init` run the `ffmpeg` and `ffprobe` command line tools; no command uses the
//! network or a model: a media path is checked to be a regular file and handed to
//! them as an absolute path with only the `file` protocol allowed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use decompile::{decode_frames_at, DecoderSetting};
use edit_dna::{BoundaryView, EditDna};
use serde_json::Value;
use timeline::{
    attach_bridge, bridge_of, build_render_plan, check_dna_length as check_project_dna,
    cut_silence, dna_ref, init, joins, media_info, punch_in, set_dna_ref, single_media,
    words_on_timeline, BridgeRequest, CutOptions, DnaRef, InitSpec, PunchRequest, RenderPlan,
    RenderSize, Timeline, TimelineError, MAX_PIXELS, MAX_SIDE,
};

use crate::card::{draw_card, parse_color};
use crate::render_backend::{
    result_line, BackendChoice, FfmpegBackend, RenderBackend, RenderJob, RenderOutcome,
};
use crate::smelter_backend::SmelterBackend;
use crate::understand_cmd::ExitWith;

/// The DNA and the media may differ by this much in length (see `timeline::cut_silence`).
const DNA_TOLERANCE_MS: u64 = 100;
/// Only this protocol may be opened by `ffmpeg` and `ffprobe`: a media file that is a
/// playlist or a concat script cannot make them fetch anything.
const PROTOCOLS: &str = "file";
/// Width of the thumbnails in `qa inspect-boundaries`.
const THUMB_WIDTH: u32 = 160;
/// Audio on each side of a join in `qa inspect-boundaries`.
const PCM_WINDOW_MS: u64 = 400;
const PCM_RATE: u32 = 16_000;

#[derive(Args)]
pub struct TimelineCommand {
    #[command(subcommand)]
    command: TimelineSub,
}

#[derive(Subcommand)]
enum TimelineSub {
    /// Make a project (.otio) with one clip spanning a video.
    Init(InitArgs),
    /// Cut silence gaps (and filler words) out of a project, from its edit DNA.
    CutSilence(CutSilenceArgs),
}

#[derive(Args)]
pub struct TransformCommand {
    #[command(subcommand)]
    command: TransformSub,
}

#[derive(Subcommand)]
enum TransformSub {
    /// Zoom into the centre of the picture over a span.
    PunchIn(PunchInArgs),
}

#[derive(Args)]
pub struct FunnelCommand {
    #[command(subcommand)]
    command: FunnelSub,
}

#[derive(Subcommand)]
enum FunnelSub {
    /// Cut the project at a tension point and end on a card with a link.
    AttachBridge(AttachBridgeArgs),
}

#[derive(Args)]
pub struct QaCommand {
    #[command(subcommand)]
    command: QaSub,
}

#[derive(Subcommand)]
enum QaSub {
    /// Write a filmstrip PNG for every join in the project.
    InspectBoundaries(InspectArgs),
}

#[derive(Args)]
struct InitArgs {
    /// The video to edit. Its absolute path is written to the project.
    #[arg(long)]
    input: PathBuf,
    /// Its edit_dna.json from `decompile`. Its path and length are recorded.
    #[arg(long)]
    dna: Option<PathBuf>,
    /// Where to write the project.
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
struct CutSilenceArgs {
    #[arg(long)]
    project: PathBuf,
    /// The edit_dna.json of the project's video.
    #[arg(long)]
    dna: PathBuf,
    /// Remove silence gaps at least this long.
    #[arg(long, default_value_t = 180)]
    min_gap_ms: u64,
    /// Audio fade on each side of a join.
    #[arg(long, default_value_t = 30)]
    fade_ms: u64,
    /// Filler words to remove (needs whisper words in the DNA), comma-separated.
    #[arg(long, value_delimiter = ',', default_values_t = ["um".to_string(), "uh".to_string()])]
    fillers: Vec<String>,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
struct PunchInArgs {
    #[arg(long)]
    project: PathBuf,
    /// Start of the zoom, in milliseconds of the edited timeline.
    #[arg(long)]
    start_ms: u64,
    #[arg(long)]
    end_ms: u64,
    /// Zoom factor: above 1, at most 2.
    #[arg(long, allow_negative_numbers = true)]
    scale: f64,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
struct AttachBridgeArgs {
    #[arg(long)]
    project: PathBuf,
    /// The link shown on the card.
    #[arg(long)]
    target_url: String,
    /// Cut at this fraction of the timeline (0 to 1, exclusive).
    #[arg(long, default_value_t = 0.72, allow_negative_numbers = true)]
    cutoff_pct: f64,
    /// Length of the card.
    #[arg(long, default_value_t = 2500)]
    card_ms: u64,
    /// The edit DNA, to cut in a pause. Defaults to the one `init` recorded.
    #[arg(long)]
    dna: Option<PathBuf>,
    #[arg(long)]
    out: PathBuf,
}

#[derive(Args)]
pub struct RenderArgs {
    #[arg(long)]
    project: PathBuf,
    #[arg(long)]
    out: PathBuf,
    /// Output size, `WIDTHxHEIGHT`, scaled and cropped to fill. Defaults to the
    /// source's size.
    #[arg(long, value_parser = parse_size)]
    size: Option<RenderSize>,
    /// What renders the project. `smelter` drives a separately installed Smelter
    /// server over its HTTP API and fails (exit 1) if it cannot find one.
    #[arg(long, value_enum, default_value = "ffmpeg")]
    backend: BackendChoice,
    /// The Smelter server (`smelter` or `smelter_main`), for `--backend smelter`.
    /// Without it, `smelter` on `PATH`.
    #[arg(long, env = "SMELTER_BIN")]
    smelter: Option<PathBuf>,
}

#[derive(Args)]
struct InspectArgs {
    #[arg(long)]
    project: PathBuf,
    #[arg(long)]
    out_dir: PathBuf,
}

fn parse_size(text: &str) -> std::result::Result<RenderSize, String> {
    let (width, height) = text
        .split_once('x')
        .ok_or_else(|| format!("\"{text}\" is not WIDTHxHEIGHT"))?;
    let side = |value: &str| {
        value
            .parse::<u32>()
            .ok()
            .filter(|side| *side > 0 && side % 2 == 0)
            .ok_or_else(|| format!("\"{text}\": {value} is not an even, positive number"))
    };
    let size = RenderSize {
        width: side(width)?,
        height: side(height)?,
    };
    if size.width > MAX_SIDE
        || size.height > MAX_SIDE
        || u64::from(size.width) * u64::from(size.height) > MAX_PIXELS
    {
        return Err(format!(
            "\"{text}\" is too big: at most {MAX_SIDE} pixels on a side and {MAX_PIXELS} in all"
        ));
    }
    Ok(size)
}

pub fn timeline(command: TimelineCommand) -> Result<()> {
    match command.command {
        TimelineSub::Init(args) => init_cmd(args),
        TimelineSub::CutSilence(args) => cut_silence_cmd(args),
    }
}

pub fn transform(command: TransformCommand) -> Result<()> {
    match command.command {
        TransformSub::PunchIn(args) => punch_in_cmd(args),
    }
}

pub fn funnel(command: FunnelCommand) -> Result<()> {
    match command.command {
        FunnelSub::AttachBridge(args) => attach_bridge_cmd(args),
    }
}

pub fn qa(command: QaCommand) -> Result<()> {
    match command.command {
        QaSub::InspectBoundaries(args) => inspect_cmd(args),
    }
}

/// An error that exits with code 2.
pub(crate) fn usage(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ExitWith {
        code: 2,
        message: message.into(),
    })
}

/// A timeline-crate error: the project or an argument is unusable (exit 2).
pub(crate) fn invalid(error: TimelineError) -> anyhow::Error {
    usage(error.to_string())
}

pub(crate) fn load_project(path: &Path) -> Result<Timeline> {
    let text = fs::read_to_string(path)
        .map_err(|error| usage(format!("cannot read project {}: {error}", path.display())))?;
    Timeline::from_json(&text).map_err(|error| usage(format!("{}: {error}", path.display())))
}

fn load_dna(path: &Path) -> Result<EditDna> {
    let text = fs::read_to_string(path)
        .map_err(|error| usage(format!("cannot read edit DNA {}: {error}", path.display())))?;
    serde_json::from_str(&text)
        .map_err(|error| usage(format!("{}: not an edit_dna.json: {error}", path.display())))
}

pub(crate) fn absolute(path: &Path, what: &str) -> Result<PathBuf> {
    fs::canonicalize(path)
        .map_err(|error| usage(format!("cannot find {what} {}: {error}", path.display())))
}

/// A media path from the command line or a project, as the absolute path of a
/// regular file. A URL, a `concat:` or `pipe:` string, a missing file, a directory
/// and a device all exit 2, so `ffmpeg` is never handed anything but a file (and the
/// absolute path cannot be read as a protocol or an option, even if it began with a
/// dash).
pub(crate) fn local_media_file(media: &str) -> Result<PathBuf> {
    if media.contains("://") {
        return Err(usage(format!(
            "\"{media}\" is a URL; reelctl reads local files only"
        )));
    }
    let path = fs::canonicalize(media)
        .map_err(|error| usage(format!("cannot find the media file \"{media}\": {error}")))?;
    if !fs::metadata(&path).is_ok_and(|meta| meta.is_file()) {
        return Err(usage(format!(
            "\"{media}\" is not a regular file; reelctl reads local media files only"
        )));
    }
    Ok(path)
}

pub(crate) fn utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| usage(format!("{} is not valid UTF-8", path.display())))
}

/// A file that is removed when the guard drops, unless it was disarmed: a failed
/// write, rename or ffmpeg run leaves nothing behind.
pub(crate) struct TempFile {
    pub(crate) path: PathBuf,
    armed: bool,
}

impl TempFile {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Move `temporary` over `path`; on failure the temporary file is removed.
pub(crate) fn replace_file(mut temporary: TempFile, path: &Path) -> Result<()> {
    fs::rename(&temporary.path, path).with_context(|| format!("replace {}", path.display()))?;
    temporary.disarm();
    Ok(())
}

/// Write `bytes` to `path` through a temporary file in the same directory.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("{} has no file name", path.display()))?;
    let temporary = TempFile::new(dir.join(format!(".{name}.{}.tmp", std::process::id())));
    fs::write(&temporary.path, bytes)
        .with_context(|| format!("write {}", temporary.path.display()))?;
    replace_file(temporary, path)
}

pub(crate) fn save_project(timeline: &Timeline, out: &Path) -> Result<()> {
    let text = timeline.to_json().map_err(invalid)?;
    write_atomic(out, text.as_bytes())
}

/// What `ffprobe` says about the first video stream and the audio.
#[derive(Debug, PartialEq)]
pub(crate) struct Probe {
    pub(crate) fps: f64,
    pub(crate) frames: i64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) has_audio: bool,
}

fn parse_rate(text: &str) -> Option<f64> {
    let (numerator, denominator) = text.split_once('/')?;
    let (numerator, denominator): (f64, f64) = (numerator.parse().ok()?, denominator.parse().ok()?);
    (denominator > 0.0 && numerator > 0.0).then(|| numerator / denominator)
}

fn parse_probe(json: &Value, path: &Path) -> Result<Probe> {
    let streams = json
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let video = streams
        .iter()
        .find(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("video"))
        .ok_or_else(|| usage(format!("{} has no video stream", path.display())))?;
    let text = |key: &str| video.get(key).and_then(Value::as_str);
    let fps = text("avg_frame_rate")
        .and_then(parse_rate)
        .or_else(|| text("r_frame_rate").and_then(parse_rate))
        .ok_or_else(|| usage(format!("{}: the frame rate is unknown", path.display())))?;
    let seconds = text("duration")
        .or_else(|| {
            json.get("format")
                .and_then(|format| format.get("duration"))
                .and_then(Value::as_str)
        })
        .and_then(|duration| duration.parse::<f64>().ok());
    let frames = text("nb_frames")
        .and_then(|count| count.parse::<i64>().ok())
        .or_else(|| seconds.map(|seconds| (seconds * fps).round() as i64))
        .filter(|frames| *frames > 0)
        .ok_or_else(|| usage(format!("{}: the video length is unknown", path.display())))?;
    let number = |key: &str| video.get(key).and_then(Value::as_u64).map(|n| n as u32);
    let (mut width, mut height) = match (number("width"), number("height")) {
        (Some(width), Some(height)) => (width, height),
        _ => {
            return Err(usage(format!(
                "{}: the video size is unknown",
                path.display()
            )))
        }
    };
    // A phone clip is stored sideways with a rotation to apply.
    let rotation = video
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find_map(|data| data.get("rotation")))
        .and_then(Value::as_f64)
        .or_else(|| {
            video
                .get("tags")
                .and_then(|tags| tags.get("rotate"))
                .and_then(Value::as_str)
                .and_then(|rotate| rotate.parse().ok())
        })
        .unwrap_or(0.0);
    if (rotation.abs().round() as i64) % 180 == 90 {
        std::mem::swap(&mut width, &mut height);
    }
    Ok(Probe {
        fps,
        frames,
        width,
        height,
        has_audio: streams
            .iter()
            .any(|stream| stream.get("codec_type").and_then(Value::as_str) == Some("audio")),
    })
}

/// A command that cannot start because the tool is missing is exit 1 with a message
/// that names it.
pub(crate) fn spawn_error(tool: &str, error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        anyhow!("{tool} was not found on PATH; install ffmpeg (macOS: `brew install ffmpeg`), which provides ffmpeg and ffprobe")
    } else {
        anyhow!("cannot run {tool}: {error}")
    }
}

pub(crate) fn probe_media(path: &Path) -> Result<Probe> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            PROTOCOLS,
            "-print_format",
            "json",
            "-show_streams",
            "-show_format",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| spawn_error("ffprobe", error))?;
    if !output.status.success() {
        return Err(usage(format!(
            "ffprobe cannot read {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let json: Value = serde_json::from_slice(&output.stdout).context("parse ffprobe output")?;
    parse_probe(&json, path)
}

fn check_dna_length(dna: &EditDna, dna_path: &Path, probe: &Probe) -> Result<()> {
    let media_ms = (probe.frames as f64 * 1000.0 / probe.fps).round() as u64;
    if dna.duration_ms.abs_diff(media_ms) > DNA_TOLERANCE_MS {
        return Err(usage(format!(
            "{}: the edit DNA is {} ms long and the video is {media_ms} ms; they are not the same file",
            dna_path.display(),
            dna.duration_ms
        )));
    }
    Ok(())
}

fn init_cmd(args: InitArgs) -> Result<()> {
    let media = local_media_file(&utf8(&args.input)?)?;
    let probe = probe_media(&media)?;
    let dna = match &args.dna {
        Some(path) => {
            let dna = load_dna(path)?;
            check_dna_length(&dna, path, &probe)?;
            Some(DnaRef {
                path: utf8(&absolute(path, "edit DNA")?)?,
                duration_ms: dna.duration_ms,
            })
        }
        None => None,
    };
    let project = init(&InitSpec {
        media_path: utf8(&media)?,
        fps: probe.fps,
        frames: probe.frames,
        width: probe.width,
        height: probe.height,
        has_audio: probe.has_audio,
        dna,
    })
    .map_err(invalid)?;
    save_project(&project, &args.out)?;
    println!(
        "init: out={} frames={} fps={:.3} size={}x{} audio={}",
        args.out.display(),
        probe.frames,
        probe.fps,
        probe.width,
        probe.height,
        probe.has_audio
    );
    Ok(())
}

fn cut_silence_cmd(args: CutSilenceArgs) -> Result<()> {
    let mut project = load_project(&args.project)?;
    let dna = load_dna(&args.dna)?;
    let options = CutOptions {
        min_gap_ms: args.min_gap_ms,
        fade_ms: args.fade_ms,
        fillers: args.fillers,
    };
    let report = cut_silence(&mut project, &dna, &options).map_err(|error| {
        usage(format!(
            "{} with {}: {error}",
            args.project.display(),
            args.dna.display()
        ))
    })?;
    set_dna_ref(
        &mut project,
        &DnaRef {
            path: utf8(&absolute(&args.dna, "edit DNA")?)?,
            duration_ms: dna.duration_ms,
        },
    );
    save_project(&project, &args.out)?;
    println!(
        "cut-silence: out={} removed_spans={} removed_ms={} timeline_ms={}",
        args.out.display(),
        report.removed.len(),
        report
            .removed
            .iter()
            .map(|span| span.end_ms - span.start_ms)
            .sum::<u64>(),
        timeline_ms(&project)?
    );
    Ok(())
}

fn timeline_ms(project: &Timeline) -> Result<u64> {
    let rate = timeline::project_rate(project).map_err(invalid)?;
    let frames = timeline::timeline_frames(project).map_err(invalid)?;
    Ok(timeline::frame_ms(frames, rate))
}

fn punch_in_cmd(args: PunchInArgs) -> Result<()> {
    let mut project = load_project(&args.project)?;
    let report = punch_in(
        &mut project,
        &PunchRequest {
            start_ms: args.start_ms,
            end_ms: args.end_ms,
            scale: args.scale,
        },
    )
    .map_err(invalid)?;
    save_project(&project, &args.out)?;
    println!(
        "punch-in: out={} frames={}..{} clips={} scale={}",
        args.out.display(),
        report.start_frame,
        report.end_frame,
        report.clips,
        args.scale
    );
    Ok(())
}

fn attach_bridge_cmd(args: AttachBridgeArgs) -> Result<()> {
    let mut project = load_project(&args.project)?;
    // The project's media must be a local file even though this command only reads
    // the project: a bridge on a project that points elsewhere is not one to keep.
    let media = single_media(&project)
        .map_err(|error| usage(format!("{}: {error}", args.project.display())))?;
    local_media_file(media)
        .map_err(|error| usage(format!("{}: {error}", args.project.display())))?;
    // A project that already has a bridge does not look at pauses again.
    let pauses: Vec<(u64, u64)> = if bridge_of(&project).is_some() {
        Vec::new()
    } else {
        let dna_path = match (&args.dna, dna_ref(&project)) {
            (Some(path), _) => Some(path.clone()),
            (None, Some(recorded)) => Some(PathBuf::from(recorded.path)),
            (None, None) => None,
        };
        match dna_path {
            Some(path) => {
                let dna = load_dna(&path)?;
                check_project_dna(&project, dna.duration_ms)
                    .map_err(|error| usage(format!("{}: {error}", path.display())))?;
                dna.silence_gaps
                    .iter()
                    .map(|gap| (gap.start_ms, gap.end_ms))
                    .collect()
            }
            None => Vec::new(),
        }
    };
    let report = attach_bridge(
        &mut project,
        &BridgeRequest {
            target_url: args.target_url,
            cutoff_pct: args.cutoff_pct,
            card_ms: args.card_ms,
        },
        &pauses,
    )
    .map_err(invalid)?;
    save_project(&project, &args.out)?;
    let rate = timeline::project_rate(&project).map_err(invalid)?;
    println!(
        "attach-bridge: out={} cut_ms={} in_pause={} card_ms={} unchanged={}",
        args.out.display(),
        timeline::frame_ms(report.spec.cut_frame, rate),
        report.in_pause,
        report.spec.card_ms,
        report.unchanged
    );
    Ok(())
}

/// Removes its directory when dropped.
pub(crate) struct TempDir(pub(crate) PathBuf);

impl TempDir {
    pub(crate) fn create() -> Result<Self> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("reelctl-render-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&path).with_context(|| format!("create {}", path.display()))?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn seconds(frames: i64, fps: f64) -> String {
    format!("{:.6}", frames as f64 / fps)
}

/// The `ffmpeg` argument list that renders `plan` to `output`, with the media files
/// at `media` (absolute paths, in the plan's order) and card images at `card_paths`.
fn ffmpeg_args(
    plan: &RenderPlan,
    media: &[PathBuf],
    card_paths: &[PathBuf],
    overlay_paths: &[PathBuf],
    output: &Path,
) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> =
        ["-y", "-hide_banner", "-loglevel", "error", "-nostdin"]
            .iter()
            .map(Into::into)
            .collect();
    for path in media {
        for part in ["-protocol_whitelist", PROTOCOLS, "-i"] {
            args.push(part.into());
        }
        args.push(path.into());
    }
    for (card, path) in plan.cards.iter().zip(card_paths) {
        for part in [
            "-protocol_whitelist",
            PROTOCOLS,
            "-loop",
            "1",
            "-framerate",
            plan.frame_rate.as_str(),
            "-t",
            seconds(card.frames, plan.fps).as_str(),
            "-i",
        ] {
            args.push(part.into());
        }
        args.push(path.into());
    }
    // Overlay videos: inputs after the cards, opened like the media.
    for path in overlay_paths {
        for part in ["-protocol_whitelist", PROTOCOLS, "-i"] {
            args.push(part.into());
        }
        args.push(path.into());
    }
    args.push("-filter_complex".into());
    args.push(plan.filter_graph.clone().into());
    args.push("-map".into());
    args.push(format!("[{}]", plan.video_label).into());
    if let Some(audio) = &plan.audio_label {
        args.push("-map".into());
        args.push(format!("[{audio}]").into());
    }
    for part in [
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "18", "-pix_fmt", "yuv420p",
    ] {
        args.push(part.into());
    }
    if plan.audio_label.is_some() {
        for part in ["-c:a", "aac", "-b:a", "192k"] {
            args.push(part.into());
        }
    }
    args.push("-movflags".into());
    args.push("+faststart".into());
    args.push(output.into());
    args
}

/// The name of the temporary file `render` writes before it renames: the output's
/// own name with the process id, and the extension last, because `ffmpeg` picks the
/// container from it.
pub(crate) fn render_temp_name(out: &Path) -> Result<String> {
    let stem = out.file_stem().and_then(|name| name.to_str());
    let extension = out.extension().and_then(|ext| ext.to_str());
    match (stem, extension) {
        (Some(stem), Some(extension)) => {
            Ok(format!(".{stem}.{}.tmp.{extension}", std::process::id()))
        }
        _ => Err(usage(format!(
            "--out {} needs a file name with an extension (for example out.mp4): ffmpeg picks the container from it",
            out.display()
        ))),
    }
}

pub fn render(args: RenderArgs) -> Result<()> {
    let project = load_project(&args.project)?;
    let job = RenderJob {
        project_path: &args.project,
        project: &project,
        out: &args.out,
        size: args.size,
    };
    let backend: Box<dyn RenderBackend> = match args.backend {
        BackendChoice::Ffmpeg => Box::new(FfmpegBackend),
        BackendChoice::Smelter => Box::new(SmelterBackend::new(args.smelter.clone())),
    };
    let outcome = backend.render(&job)?;
    println!("{}", result_line(backend.name(), &args.out, &outcome));
    for note in &outcome.notes {
        println!("{note}");
    }
    Ok(())
}

/// The ffmpeg backend: one filter graph for the whole project.
pub(crate) fn render_ffmpeg(job: &RenderJob<'_>) -> Result<RenderOutcome> {
    let plan = build_render_plan(job.project, job.size)
        .map_err(|error| usage(format!("{}: {error}", job.project_path.display())))?;
    let mut media = Vec::with_capacity(plan.media.len());
    for url in &plan.media {
        media.push(
            local_media_file(url)
                .map_err(|error| usage(format!("{}: {error}", job.project_path.display())))?,
        );
    }
    // The overlay videos are rendered files named by the project; they are held to
    // the same rule as the media.
    let mut overlay_paths = Vec::with_capacity(plan.overlays.len());
    for overlay in &plan.overlays {
        overlay_paths.push(
            local_media_file(&overlay.path)
                .map_err(|error| usage(format!("{}: {error}", job.project_path.display())))?,
        );
    }
    // The output is checked before anything is made.
    if job.out.is_dir() {
        return Err(usage(format!(
            "--out {} is a directory; name the file to write",
            job.out.display()
        )));
    }
    let temporary_name = render_temp_name(job.out)?;

    let scratch = TempDir::create()?;
    let mut card_paths = Vec::with_capacity(plan.cards.len());
    let mut notes = Vec::new();
    for (index, card) in plan.cards.iter().enumerate() {
        let drawn = draw_card(
            &card.text,
            parse_color(&card.background)?,
            parse_color(&card.foreground)?,
            plan.size.width,
            plan.size.height,
        );
        if drawn.truncated() {
            notes.push(format!(
                "render: the card text was too long for the frame; it shows {} of {} characters and ends in ...",
                drawn.shown.chars().count(),
                drawn.total
            ));
        }
        let path = scratch.0.join(format!("card{index}.png"));
        drawn
            .image
            .save(&path)
            .with_context(|| format!("write {}", path.display()))?;
        card_paths.push(path);
    }

    // Render beside the output, then rename. The temporary file is removed on every
    // way out but a kill, so a failed run leaves nothing behind. Its directory is
    // made absolute, so the path cannot be read as an option.
    let dir = match job.out.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    };
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let dir = fs::canonicalize(&dir).with_context(|| format!("find {}", dir.display()))?;
    let temporary = TempFile::new(dir.join(temporary_name));

    let started = Instant::now();
    let output = Command::new("ffmpeg")
        .args(ffmpeg_args(
            &plan,
            &media,
            &card_paths,
            &overlay_paths,
            &temporary.path,
        ))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| spawn_error("ffmpeg", error))?;
    if !output.status.success() {
        let log = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = log.lines().rev().take(12).collect();
        return Err(anyhow!(
            "ffmpeg failed ({}) rendering {}:\n{}",
            output.status,
            job.project_path.display(),
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        ));
    }
    replace_file(temporary, job.out)?;
    Ok(RenderOutcome {
        frames: plan.frames,
        duration_ms: (plan.frames as f64 * 1000.0 / plan.fps).round() as u64,
        size: plan.size,
        backend_ms: started.elapsed().as_millis(),
        notes,
    })
}

/// Mono f32 audio of `media` from `from_ms` for `length_ms`, at `PCM_RATE`.
fn decode_pcm(media: &Path, from_ms: u64, length_ms: u64) -> Result<Vec<f32>> {
    let output = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            PROTOCOLS,
            "-ss",
        ])
        .arg(format!("{:.3}", from_ms as f64 / 1000.0))
        .arg("-t")
        .arg(format!("{:.3}", length_ms as f64 / 1000.0))
        .arg("-i")
        .arg(media)
        .args(["-vn", "-f", "f32le", "-ac", "1", "-ar"])
        .arg(PCM_RATE.to_string())
        .arg("-")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| spawn_error("ffmpeg", error))?;
    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg cannot read the audio of {}: {}",
            media.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect())
}

fn inspect_cmd(args: InspectArgs) -> Result<()> {
    let project = load_project(&args.project)?;
    let info = media_info(&project)
        .map_err(|error| usage(format!("{}: {error}", args.project.display())))?;
    // The one media file of the project, checked before any join is decoded: it is
    // the only file opened below (`media_info` refuses a project with several).
    let media_path = local_media_file(&info.path)
        .map_err(|error| usage(format!("{}: {error}", args.project.display())))?;
    let rate = timeline::project_rate(&project).map_err(invalid)?;
    let all = joins(&project).map_err(invalid)?;
    let words = match dna_ref(&project).map(|recorded| PathBuf::from(recorded.path)) {
        Some(path) if path.is_file() => {
            let dna = load_dna(&path)?;
            words_on_timeline(&project, &dna.words).map_err(invalid)?
        }
        _ => Vec::new(),
    };

    let frame_time = |frame: i64| ((frame as f64 + 0.5) * 1000.0 / rate).round() as u64;
    let mut views = Vec::with_capacity(all.len());
    for join in &all {
        // Two frames before the join and two after, from where they come from.
        let before = join
            .before
            .as_ref()
            .map(|(_, last)| [(*last - 1).max(0), *last]);
        let after = join.after.as_ref().map(|(_, first)| [*first, *first + 1]);
        let mut sides: Vec<Option<Vec<Vec<u8>>>> = Vec::with_capacity(2);
        let mut size: Option<(u32, u32)> = None;
        for side in [before, after] {
            let Some(frames) = side else {
                sides.push(None);
                continue;
            };
            let times = frames.map(frame_time);
            let decoded = decode_frames_at(&media_path, &times, THUMB_WIDTH, DecoderSetting::Auto)
                .with_context(|| format!("decode frames of {}", media_path.display()))?;
            if let Some(first) = decoded.first() {
                size = Some((first.width, first.height));
            }
            sides.push(Some(decoded.into_iter().map(|frame| frame.rgb).collect()));
        }
        let (thumb_width, thumb_height) = size.unwrap_or_else(|| {
            let height =
                u64::from(THUMB_WIDTH) * u64::from(info.height) / u64::from(info.width.max(1));
            (THUMB_WIDTH, (height + height % 2) as u32)
        });
        // A side that is not footage (a card) is shown as a dark frame.
        let blank_len = (thumb_width as usize)
            .checked_mul(thumb_height as usize)
            .and_then(|pixels| pixels.checked_mul(3))
            .ok_or_else(|| {
                usage(format!(
                    "{}: the thumbnail size overflows",
                    args.project.display()
                ))
            })?;
        let blank = vec![0x14u8; blank_len];
        let mut thumbs: Vec<Vec<u8>> = Vec::with_capacity(4);
        for side in sides {
            match side {
                Some(frames) if frames.len() == 2 => thumbs.extend(frames),
                _ => thumbs.extend([blank.clone(), blank.clone()]),
            }
        }

        let join_ms = timeline::frame_ms(join.frame, rate);
        // 400 ms of audio on each side; silence where a side is the card, so the
        // waveform lines up with the two halves of the filmstrip.
        let silence = || vec![0.0f32; (u64::from(PCM_RATE) * PCM_WINDOW_MS / 1000) as usize];
        let mut pcm: Vec<f32> = match (&join.before, info.has_audio) {
            (Some((_, last)), true) => {
                let end = timeline::frame_ms(last.saturating_add(1), rate);
                let from = end.saturating_sub(PCM_WINDOW_MS);
                decode_pcm(&media_path, from, end - from)?
            }
            _ => silence(),
        };
        let before_len = pcm.len() as u64 * 1000 / u64::from(PCM_RATE);
        match (&join.after, info.has_audio) {
            (Some((_, first)), true) => {
                let from = timeline::frame_ms(*first, rate);
                pcm.extend(decode_pcm(&media_path, from, PCM_WINDOW_MS)?);
            }
            _ => pcm.extend(silence()),
        }
        views.push(BoundaryView {
            frame_index: join.frame as u64,
            time_ms: join_ms,
            thumb_width,
            thumb_height,
            thumbs,
            cut_thumb_index: 2,
            sample_rate: PCM_RATE,
            pcm_start_ms: join_ms.saturating_sub(before_len),
            pcm,
        });
    }
    let paths = filmstrip::write_boundary_views(&args.out_dir, &views, &words)
        .with_context(|| format!("write filmstrips to {}", args.out_dir.display()))?;
    println!(
        "inspect-boundaries: out_dir={} joins={}",
        args.out_dir.display(),
        paths.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({
            "streams": [
                {"codec_type": "video", "width": 1920, "height": 1080,
                 "avg_frame_rate": "30000/1001", "r_frame_rate": "30000/1001",
                 "nb_frames": "300", "duration": "10.01"},
                {"codec_type": "audio", "duration": "10.0"}
            ],
            "format": {"duration": "10.02"}
        })
    }

    #[test]
    fn a_probe_gives_rate_frames_size_and_audio() {
        let probe = parse_probe(&sample(), Path::new("a.mp4")).expect("probe");
        assert_eq!(probe.frames, 300);
        assert!((probe.fps - 29.97002997).abs() < 1e-6);
        assert_eq!(
            (probe.width, probe.height, probe.has_audio),
            (1920, 1080, true)
        );
    }

    #[test]
    fn without_a_frame_count_the_duration_gives_it() {
        let mut json = sample();
        json["streams"][0]["nb_frames"] = Value::Null;
        let probe = parse_probe(&json, Path::new("a.mp4")).expect("probe");
        assert_eq!(probe.frames, 300);
    }

    #[test]
    fn a_rotated_phone_clip_swaps_its_sides() {
        let mut json = sample();
        json["streams"][0]["side_data_list"] = json!([{"rotation": -90}]);
        let probe = parse_probe(&json, Path::new("a.mp4")).expect("probe");
        assert_eq!((probe.width, probe.height), (1080, 1920));
    }

    #[test]
    fn a_file_without_video_is_a_usage_error() {
        let json = json!({"streams": [{"codec_type": "audio"}]});
        let error = parse_probe(&json, Path::new("a.mp3")).expect_err("no video");
        assert!(error.to_string().contains("a.mp3"));
    }

    #[test]
    fn the_size_is_width_x_height_with_even_sides() {
        assert_eq!(
            parse_size("1080x1920").expect("size"),
            RenderSize {
                width: 1080,
                height: 1920
            }
        );
        assert!(parse_size("1080").is_err());
        assert!(parse_size("1081x1920").is_err());
        assert!(parse_size("0x10").is_err());
        assert!(parse_size("axb").is_err());
    }

    #[test]
    fn an_atomic_write_leaves_only_the_file() {
        let dir = std::env::temp_dir().join(format!("reelctl-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("p.otio");
        write_atomic(&path, b"one").expect("write");
        write_atomic(&path, b"two").expect("overwrite");
        assert_eq!(fs::read(&path).expect("read"), b"two");
        let names: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    fn exit_code(error: &anyhow::Error) -> Option<i32> {
        error.downcast_ref::<ExitWith>().map(|exit| exit.code)
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reelctl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn only_a_plain_existing_file_is_media() {
        let dir = scratch_dir("media-path");
        let file = dir.join("a.mp4");
        fs::write(&file, b"x").expect("write");
        let found = local_media_file(file.to_str().expect("utf8")).expect("a file");
        assert!(found.is_absolute() && found.is_file());
        let missing = dir.join("missing.mp4");
        for bad in [
            "http://127.0.0.1:1/x.mp4",
            "https://example.com/a.mp4",
            "file:///etc/hosts",
            "concat:http://127.0.0.1:1/a|b",
            "concat:/etc/hosts|/etc/hosts",
            "pipe:0",
            "subfile,,start,0,end,0,,:/etc/hosts",
            "",
            dir.to_str().expect("utf8"),
            missing.to_str().expect("utf8"),
        ] {
            let error = local_media_file(bad).expect_err(bad);
            assert_eq!(exit_code(&error), Some(2), "{bad}: {error}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_render_temporary_keeps_the_extension_last() {
        let name = render_temp_name(Path::new("/some/dir/out.mp4")).expect("name");
        assert_eq!(name, format!(".out.{}.tmp.mp4", std::process::id()));
        let name = render_temp_name(Path::new("take.v2.mov")).expect("name");
        assert_eq!(name, format!(".take.v2.{}.tmp.mov", std::process::id()));
    }

    #[test]
    fn an_output_without_an_extension_is_a_usage_error() {
        let error = render_temp_name(Path::new("/some/dir/out")).expect_err("no extension");
        assert_eq!(exit_code(&error), Some(2));
        assert!(error.to_string().contains("extension"), "{error}");
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .expect("dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    #[test]
    fn a_failed_replace_removes_the_temporary_file() {
        let dir = scratch_dir("replace-fails");
        // The destination is a directory, so the rename cannot replace it.
        let target = dir.join("p.otio");
        fs::create_dir(&target).expect("dir");
        let error = write_atomic(&target, b"one").expect_err("a directory");
        assert!(error.to_string().contains("p.otio"), "{error}");
        assert_eq!(leftovers(&dir), vec!["p.otio".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_guard_removes_its_file_unless_it_is_disarmed() {
        let dir = scratch_dir("guard");
        let kept = dir.join("kept");
        let dropped = dir.join("dropped");
        fs::write(&kept, b"x").expect("write");
        fs::write(&dropped, b"x").expect("write");
        let mut guard = TempFile::new(kept.clone());
        guard.disarm();
        drop(guard);
        drop(TempFile::new(dropped.clone()));
        assert!(kept.exists());
        assert!(!dropped.exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_size_has_a_cap_on_each_side_and_on_the_pixels() {
        assert!(parse_size("7680x4320").is_ok());
        assert!(parse_size("8192x2").is_ok());
        for text in [
            "8194x2",
            "2x8194",
            "8192x4096",
            "4096x8192",
            "99999999999x2",
        ] {
            assert!(parse_size(text).is_err(), "{text}");
        }
        let message = parse_size("8192x4096").expect_err("too big");
        assert!(
            message.contains("8192") && message.contains("33177600"),
            "{message}"
        );
    }
}
