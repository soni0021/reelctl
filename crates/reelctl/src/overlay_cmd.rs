//! `reelctl overlay add-chart|remove`: draw a chart with the separately installed
//! `manimgx` and put it on the project as an overlay, which `render` composites.
//!
//! Exit codes: 2 for a bad spec or argument, a path that is not a plain local file, a
//! span that does not fit, or layout problems the worker's `inspect` found; 1 for a
//! worker that is missing, fails, is too slow or writes a video that fails the
//! checks, and for `ffmpeg` failures. The project is read first and written last, so
//! a failure leaves it as it was. The worker is started with an argument array and no
//! shell, in a scratch directory, with no chart text on its command line; the scene
//! file holds the chart text once, as data (see the `scenes` crate).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};
use scenes::{
    resolve_worker, scene_fps, scene_size, scene_source, ChartSpec, Inspection, ManimgxCli,
    SceneError, SceneJob, SceneRenderer, MAX_SPEC_BYTES, SCENE_NAME, WORKER_ENV,
};
use timeline::{
    add_captions, add_overlay, caption_preset, check_dna_length as check_project_dna, media_info,
    overlays, plan_overlay, project_rate, remove_captions, remove_overlay, CaptionCase,
    CaptionPosition, CaptionStyle, OverlayAsset, OverlayRequest, StyleOverrides, CAPTION_PRESETS,
    KEY_COLOR,
};

use crate::timeline_cmd::{
    invalid, load_dna, load_project, probe_media, replace_file, save_project, spawn_error, usage,
    utf8, Probe, TempDir, TempFile,
};

/// The most a rendered chart video may weigh.
const MAX_ASSET_BYTES: u64 = 256 * 1024 * 1024;
/// How far a corner pixel of the rendered video may be from the key colour, per
/// channel: the video is lossy.
const KEY_TOLERANCE: u8 = 40;
/// The longest `--timeout-s`.
const MAX_TIMEOUT_S: u64 = 3600;
/// Only `file` may be opened by `ffmpeg` and `ffprobe`.
const PROTOCOLS: &str = "file";

#[derive(Args)]
pub struct OverlayCommand {
    #[command(subcommand)]
    command: OverlaySub,
}

#[derive(Subcommand)]
enum OverlaySub {
    /// Draw a bar, line or number-callout chart with manimgx and lay it over the
    /// video for a span.
    AddChart(AddChartArgs),
    /// Take an overlay (or the captions) off the project.
    Remove(RemoveArgs),
    /// Burn word captions from the edit DNA's whisper words into the project.
    Captions(CaptionsArgs),
}

/// The caption style flags, shared with `make-reels`.
#[derive(Args, Clone, Debug, Default)]
pub struct CaptionStyleArgs {
    /// Start from this preset: bold, clean, boxed, karaoke or minimal.
    #[arg(long)]
    pub preset: Option<String>,
    /// A JSON file of style fields (the flag names with underscores), applied
    /// after the preset and before the flags.
    #[arg(long)]
    pub style: Option<PathBuf>,
    /// A .ttf or .otf font. Default: a bold system sans.
    #[arg(long)]
    pub font: Option<String>,
    /// Text height, percent of the output height (2 to 12).
    #[arg(long)]
    pub font_size: Option<f64>,
    /// Text colour, #rrggbb.
    #[arg(long)]
    pub color: Option<String>,
    /// The spoken word's colour, #rrggbb, or none.
    #[arg(long)]
    pub highlight_color: Option<String>,
    #[arg(long)]
    pub outline_color: Option<String>,
    /// Pixels at 1080 wide (0 to 12).
    #[arg(long)]
    pub outline_width: Option<u32>,
    /// Shadow offset, pixels at 1080 wide (0 to 12).
    #[arg(long)]
    pub shadow: Option<u32>,
    #[arg(long)]
    pub box_color: Option<String>,
    /// 0 (no box) to 1.
    #[arg(long)]
    pub box_opacity: Option<f64>,
    /// top, middle or bottom.
    #[arg(long)]
    pub position: Option<String>,
    /// Percent of the output height from that edge (0 to 40).
    #[arg(long)]
    pub margin: Option<f64>,
    /// 1 to 5.
    #[arg(long)]
    pub words_per_group: Option<u32>,
    /// Characters per line before a group wraps (8 to 40).
    #[arg(long)]
    pub max_chars: Option<u32>,
    /// as-is, upper or lower.
    #[arg(long)]
    pub case: Option<String>,
    /// A pause at least this long starts a new group (100 to 2000).
    #[arg(long)]
    pub min_gap_ms: Option<u64>,
}

/// A flag whose value is one of a style field's names (`top`, `as-is`, ...).
fn named<T: serde::de::DeserializeOwned>(field: &str, value: &Option<String>) -> Result<Option<T>> {
    value
        .as_ref()
        .map(|text| {
            serde_json::from_value(serde_json::Value::String(text.clone()))
                .map_err(|error| usage(format!("--{field} {text}: {error}")))
        })
        .transpose()
}

impl CaptionStyleArgs {
    /// The resolved style: the preset, then the file, then the flags.
    pub fn resolve(&self) -> Result<CaptionStyle> {
        let base = caption_preset(self.preset.as_deref().unwrap_or("bold")).map_err(invalid)?;
        let from_file = match &self.style {
            Some(path) => {
                let text = fs::read_to_string(path).map_err(|error| {
                    usage(format!(
                        "cannot read the caption style {}: {error}",
                        path.display()
                    ))
                })?;
                serde_json::from_str::<StyleOverrides>(&text)
                    .map_err(|error| usage(format!("{}: {error}", path.display())))?
            }
            None => StyleOverrides::default(),
        };
        let position: Option<CaptionPosition> = named("position", &self.position)?;
        let case: Option<CaptionCase> = named("case", &self.case)?;
        let flags = StyleOverrides {
            preset: None,
            font: self.font.clone(),
            font_size: self.font_size,
            color: self.color.clone(),
            highlight_color: self.highlight_color.clone(),
            outline_color: self.outline_color.clone(),
            outline_width: self.outline_width,
            shadow: self.shadow,
            box_color: self.box_color.clone(),
            box_opacity: self.box_opacity,
            position,
            margin: self.margin,
            words_per_group: self.words_per_group,
            max_chars: self.max_chars,
            case,
            min_gap_ms: self.min_gap_ms,
        };
        base.with(&from_file)
            .and_then(|style| style.with(&flags))
            .map_err(invalid)
    }
}

#[derive(Args)]
struct CaptionsArgs {
    #[arg(long, required_unless_present = "list_presets")]
    project: Option<PathBuf>,
    /// The edit_dna.json with whisper words of the project's video.
    #[arg(long, required_unless_present = "list_presets")]
    dna: Option<PathBuf>,
    #[arg(long, required_unless_present = "list_presets")]
    out: Option<PathBuf>,
    /// Print every preset's style as JSON and exit.
    #[arg(long)]
    list_presets: bool,
    #[command(flatten)]
    style: CaptionStyleArgs,
}

#[derive(Args)]
struct AddChartArgs {
    #[arg(long)]
    project: PathBuf,
    /// The chart spec (JSON): kind, title, series, start_ms, end_ms, position.
    #[arg(long)]
    spec: PathBuf,
    #[arg(long)]
    out: PathBuf,
    /// Where the rendered chart video is kept. Defaults to `<project name>.overlays/`
    /// beside the project.
    #[arg(long)]
    assets_dir: Option<PathBuf>,
    /// The manimgx program. Defaults to $MANIMGX_BIN, then `manimgx` on PATH.
    #[arg(long)]
    manimgx: Option<String>,
    /// Do not run `manimgx inspect` (the layout check) before rendering.
    #[arg(long)]
    skip_inspect: bool,
    /// Stop a worker run that takes longer than this many seconds.
    #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout_s: u64,
}

#[derive(Args)]
struct RemoveArgs {
    #[arg(long)]
    project: PathBuf,
    /// The overlay's id, as `add-chart` printed it.
    #[arg(
        long,
        required_unless_present = "captions",
        conflicts_with = "captions"
    )]
    id: Option<String>,
    /// Take the captions off instead.
    #[arg(long)]
    captions: bool,
    #[arg(long)]
    out: PathBuf,
}

pub fn overlay(command: OverlayCommand) -> Result<()> {
    match command.command {
        OverlaySub::AddChart(args) => add_chart(args),
        OverlaySub::Remove(args) => remove(args),
        OverlaySub::Captions(args) => captions(args),
    }
}

fn captions(args: CaptionsArgs) -> Result<()> {
    if args.list_presets {
        let mut all = serde_json::Map::new();
        for name in CAPTION_PRESETS {
            all.insert(
                name.to_string(),
                serde_json::to_value(caption_preset(name).map_err(invalid)?)?,
            );
        }
        out!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(());
    }
    let (Some(project_path), Some(dna_path), Some(out)) = (&args.project, &args.dna, &args.out)
    else {
        return Err(usage("--project, --dna and --out are required"));
    };
    let mut style = args.style.resolve()?;
    // The font is found now, and a given one is kept as an absolute path, so the
    // project renders from any directory.
    let font = crate::captions_draw::find_font(&style)?;
    crate::captions_draw::load_font(&font)?;
    if style.font.is_some() {
        let absolute = fs::canonicalize(&font).map_err(|error| {
            usage(format!(
                "cannot find the caption font {}: {error}",
                font.display()
            ))
        })?;
        style.font = Some(utf8(&absolute)?);
    }
    let mut project = load_project(project_path)?;
    let dna = load_dna(dna_path)?;
    check_project_dna(&project, dna.duration_ms)
        .map_err(|error| usage(format!("{}: {error}", dna_path.display())))?;
    if dna.asr_backend != edit_dna::AsrBackend::WhisperCpp {
        return Err(usage(format!(
            "{}: its words are voiced bursts, not whisper words; decompile with --whisper-model to caption",
            dna_path.display()
        )));
    }
    let report = add_captions(&mut project, &dna.words, &style).map_err(invalid)?;
    save_project(&project, out)?;
    out!(
        "captions: out={} groups={} words={} dropped={} preset={} font={} unchanged={}",
        out.display(),
        report.groups,
        report.words,
        report.dropped,
        style.preset,
        font.display(),
        report.unchanged
    );
    Ok(())
}

/// A scenes error as a command error with the right exit code.
fn scene_error(error: SceneError) -> anyhow::Error {
    match error {
        SceneError::Usage(message) => usage(message),
        SceneError::Worker(message) => anyhow!(message),
    }
}

/// The text of a spec file: a plain local file of at most [`MAX_SPEC_BYTES`].
fn read_spec(path: &Path) -> Result<String> {
    let shown = path.display();
    if path.to_string_lossy().contains("://") {
        return Err(usage(format!(
            "--spec {shown} is a URL; reelctl reads local files only"
        )));
    }
    let meta = fs::metadata(path)
        .map_err(|error| usage(format!("cannot read the spec {shown}: {error}")))?;
    if !meta.is_file() {
        return Err(usage(format!("--spec {shown} is not a regular file")));
    }
    if meta.len() > MAX_SPEC_BYTES {
        return Err(usage(format!(
            "the spec {shown} is {} bytes; at most {MAX_SPEC_BYTES}",
            meta.len()
        )));
    }
    fs::read_to_string(path)
        .map_err(|error| usage(format!("cannot read the spec {shown}: {error}")))
}

/// `<project dir>/<project name>.overlays`.
fn default_assets_dir(project: &Path) -> PathBuf {
    let stem = project
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    match project.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(format!("{stem}.overlays")),
        _ => PathBuf::from(format!("{stem}.overlays")),
    }
}

/// Whether a corner pixel is the key colour, within [`KEY_TOLERANCE`] per channel.
fn is_key_pixel(rgb: [u8; 3]) -> bool {
    let key = key_rgb();
    rgb.iter()
        .zip(key)
        .all(|(got, want)| got.abs_diff(want) <= KEY_TOLERANCE)
}

fn key_rgb() -> [u8; 3] {
    let hex = KEY_COLOR.trim_start_matches('#');
    let channel = |at: usize| u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0);
    [channel(0), channel(2), channel(4)]
}

/// Whether a rendered video lasts as long as the span: within one frame of the
/// slower of its own rate and the project's.
fn length_ok(video_frames: i64, video_fps: f64, expected_s: f64, project_fps: f64) -> bool {
    let actual = video_frames as f64 / video_fps;
    let frame = 1.0 / video_fps.min(project_fps);
    (actual - expected_s).abs() <= frame + 1e-9
}

/// The top-left pixel of the first frame of `video`, as RGB.
fn corner_pixel(video: &Path) -> Result<[u8; 3]> {
    let mut command = Command::new("ffmpeg");
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            PROTOCOLS,
            "-i",
        ])
        .arg(video)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "format=rgb24,crop=1:1:0:0",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-",
        ])
        .stdin(Stdio::null());
    let output =
        progress::tracked_output(&mut command).map_err(|error| spawn_error("ffmpeg", error))?;
    match output.stdout.as_slice() {
        [r, g, b] if output.status.success() => Ok([*r, *g, *b]),
        _ => Err(anyhow!(
            "ffmpeg cannot read a frame of the rendered chart {}: {}",
            video.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// Check the worker's video against what was asked for. Any difference is an error:
/// the project is not touched with a video that is not the chart.
fn check_video(
    video: &Path,
    probe: &Probe,
    asked: (u32, u32),
    expected_s: f64,
    project_fps: f64,
) -> Result<()> {
    let bytes = fs::metadata(video)
        .with_context(|| format!("read {}", video.display()))?
        .len();
    if bytes == 0 || bytes > MAX_ASSET_BYTES {
        return Err(anyhow!(
            "the rendered chart is {bytes} bytes; it must be 1 to {MAX_ASSET_BYTES}"
        ));
    }
    if (probe.width, probe.height) != asked {
        return Err(anyhow!(
            "the rendered chart is {}x{} and {}x{} was asked for",
            probe.width,
            probe.height,
            asked.0,
            asked.1
        ));
    }
    if !length_ok(probe.frames, probe.fps, expected_s, project_fps) {
        return Err(anyhow!(
            "the rendered chart lasts {:.3} s and the span is {expected_s:.3} s; it must be within one frame",
            probe.frames as f64 / probe.fps
        ));
    }
    let corner = corner_pixel(video)?;
    if !is_key_pixel(corner) {
        return Err(anyhow!(
            "the rendered chart's top-left pixel is rgb({}, {}, {}) and the key colour {KEY_COLOR} was expected; the worker did not draw on the key colour, so it cannot be keyed out",
            corner[0],
            corner[1],
            corner[2]
        ));
    }
    Ok(())
}

/// Put `video` at `target` through a temporary file in the same directory.
fn store_asset(video: &Path, target: &Path) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("{} has no directory", target.display()))?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("{} has no file name", target.display()))?;
    let temporary = TempFile::new(dir.join(format!(".{name}.{}.tmp", std::process::id())));
    fs::copy(video, &temporary.path)
        .with_context(|| format!("copy the chart to {}", temporary.path.display()))?;
    replace_file(temporary, target)
}

fn add_chart(args: AddChartArgs) -> Result<()> {
    let mut project = load_project(&args.project)?;
    let spec = ChartSpec::parse(&read_spec(&args.spec)?).map_err(scene_error)?;
    let id = spec.id().map_err(scene_error)?;
    let info = media_info(&project)
        .map_err(|error| usage(format!("{}: {error}", args.project.display())))?;
    let rate = project_rate(&project).map_err(invalid)?;
    // The span is checked before anything is rendered.
    let slot = plan_overlay(&project, &id, spec.start_ms, spec.end_ms)
        .map_err(|error| usage(format!("{}: {error}", args.project.display())))?;

    if slot.existing {
        let recorded = overlays(&project)
            .map_err(invalid)?
            .into_iter()
            .find(|overlay| overlay.id == id);
        if let Some(overlay) = recorded {
            if fs::metadata(&overlay.asset.path).is_ok_and(|meta| meta.is_file()) {
                save_project(&project, &args.out)?;
                out!(
                    "overlay add-chart: out={} id={id} frames={}..{} asset={} unchanged=true",
                    args.out.display(),
                    slot.start_frame,
                    slot.end_frame,
                    overlay.asset.path
                );
                return Ok(());
            }
        }
        // The overlay is in the project but its video is gone: draw it again and
        // record the new video in its place.
        remove_overlay(&mut project, &id).map_err(invalid)?;
    }

    let assets_dir = args
        .assets_dir
        .clone()
        .unwrap_or_else(|| default_assets_dir(&args.project));
    if assets_dir.to_string_lossy().contains("://") {
        return Err(usage(format!(
            "--assets-dir {} is a URL; reelctl writes local files only",
            assets_dir.display()
        )));
    }

    let program = resolve_worker(
        args.manimgx.as_deref(),
        std::env::var(WORKER_ENV).ok().as_deref(),
        std::env::var_os("PATH").as_deref(),
    )
    .map_err(scene_error)?;
    let scratch = TempDir::create()?;
    let renderer = ManimgxCli::new(
        program,
        scratch.0.clone(),
        Duration::from_secs(args.timeout_s.min(MAX_TIMEOUT_S)),
    );
    let version = renderer.version().map_err(scene_error)?;

    let frames = slot.end_frame - slot.start_frame;
    let duration_s = frames as f64 / rate;
    let (width, height) = scene_size(info.width, spec.width_pct);
    let scene_file = scratch.0.join("scene.py");
    fs::write(&scene_file, scene_source(&spec.payload(duration_s)))
        .with_context(|| format!("write {}", scene_file.display()))?;
    let job = SceneJob {
        scene_file,
        scene_name: SCENE_NAME.to_string(),
        width,
        height,
        fps: scene_fps(rate),
        video_out: scratch.0.join("scene.mp4"),
        storyboard_out: scratch.0.join("inspect.png"),
        work_dir: scratch.0.clone(),
    };

    if !args.skip_inspect {
        if let Inspection::Problems(report) = renderer.inspect(&job).map_err(scene_error)? {
            let kept = keep_storyboard(&job.storyboard_out, &assets_dir, &id);
            return Err(usage(format!(
                "manimgx inspect found layout problems in the chart, so it is not rendered:\n{report}{}",
                match kept {
                    Some(path) => format!("\nThe annotated storyboard is at {}", path.display()),
                    None => String::new(),
                }
            )));
        }
    }
    renderer.render(&job).map_err(scene_error)?;
    let probe = probe_media(&job.video_out)?;
    check_video(&job.video_out, &probe, (width, height), duration_s, rate)?;

    fs::create_dir_all(&assets_dir).with_context(|| format!("create {}", assets_dir.display()))?;
    let assets_dir =
        fs::canonicalize(&assets_dir).with_context(|| format!("find {}", assets_dir.display()))?;
    let asset_path = assets_dir.join(format!("{id}.mp4"));
    store_asset(&job.video_out, &asset_path)?;

    let report = add_overlay(
        &mut project,
        &OverlayRequest {
            id: id.clone(),
            spec: serde_json::to_value(&spec).context("write the spec")?,
            start_ms: spec.start_ms,
            end_ms: spec.end_ms,
            position: spec.position,
            width_pct: spec.width_pct,
            asset: OverlayAsset {
                path: utf8(&asset_path)?,
                width: probe.width,
                height: probe.height,
                frames: probe.frames,
                fps: probe.fps,
            },
            renderer: version.clone(),
        },
    )
    .map_err(invalid)?;
    save_project(&project, &args.out)?;
    out!(
        "overlay add-chart: out={} id={id} frames={}..{} asset={} unchanged={} renderer=\"{version}\"",
        args.out.display(),
        report.start_frame,
        report.end_frame,
        asset_path.display(),
        report.unchanged
    );
    Ok(())
}

/// Copy the annotated storyboard `inspect` wrote to `<assets>/<id>.inspect.png`, so a
/// person can look at what it found. `None` if there is none or it cannot be kept.
fn keep_storyboard(storyboard: &Path, assets_dir: &Path, id: &str) -> Option<PathBuf> {
    if !fs::metadata(storyboard).is_ok_and(|meta| meta.is_file()) {
        return None;
    }
    fs::create_dir_all(assets_dir).ok()?;
    let target = assets_dir.join(format!("{id}.inspect.png"));
    fs::copy(storyboard, &target).ok()?;
    Some(target)
}

fn remove(args: RemoveArgs) -> Result<()> {
    let mut project = load_project(&args.project)?;
    let Some(id) = args.id else {
        if !remove_captions(&mut project) {
            return Err(usage(format!("{} has no captions", args.project.display())));
        }
        save_project(&project, &args.out)?;
        out!(
            "overlay remove: out={} captions=true removed=true",
            args.out.display()
        );
        return Ok(());
    };
    let removed = remove_overlay(&mut project, &id).map_err(invalid)?;
    if !removed {
        return Err(usage(format!(
            "{} has no overlay with id \"{id}\"",
            args.project.display()
        )));
    }
    save_project(&project, &args.out)?;
    out!(
        "overlay remove: out={} id={id} removed=true",
        args.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_colour_is_read_from_its_hex() {
        assert_eq!(key_rgb(), [255, 0, 255]);
    }

    #[test]
    fn a_corner_pixel_must_be_near_the_key_colour_in_every_channel() {
        assert!(is_key_pixel([255, 0, 255]));
        assert!(is_key_pixel([250, 8, 247]));
        assert!(is_key_pixel([215, 40, 255]));
        assert!(!is_key_pixel([214, 0, 255]));
        assert!(!is_key_pixel([255, 41, 255]));
        assert!(!is_key_pixel([255, 0, 214]));
        assert!(!is_key_pixel([0, 0, 0]));
        assert!(!is_key_pixel([255, 255, 255]));
    }

    #[test]
    fn a_video_lasts_as_long_as_the_span_within_one_frame_of_the_slower_rate() {
        // A 4.004 s span (120 frames at 29.97): a 4.0 s video at 30 fps is fine.
        assert!(length_ok(120, 30.0, 4.004, 29.97));
        assert!(length_ok(121, 30.0, 4.004, 29.97));
        assert!(!length_ok(119, 30.0, 4.004, 29.97), "1.1 frames short");
        assert!(!length_ok(117, 30.0, 4.004, 29.97));
        assert!(!length_ok(125, 30.0, 4.004, 29.97));
        // Judged by the slower rate: 1/24 s at 24 fps.
        assert!(length_ok(95, 24.0, 4.0, 30.0));
        assert!(!length_ok(94, 24.0, 4.1, 30.0));
        assert!(!length_ok(0, 30.0, 4.0, 30.0));
    }

    #[test]
    fn the_assets_go_beside_the_project_by_default() {
        assert_eq!(
            default_assets_dir(Path::new("/p/edit.otio")),
            PathBuf::from("/p/edit.overlays")
        );
        assert_eq!(
            default_assets_dir(Path::new("edit.otio")),
            PathBuf::from("edit.overlays")
        );
    }

    #[test]
    fn a_spec_path_that_is_a_url_or_a_directory_is_exit_2() {
        for path in ["https://example.com/spec.json", "file:///etc/hosts"] {
            let error = read_spec(Path::new(path)).expect_err(path);
            assert!(error.to_string().contains("URL"), "{error}");
        }
        let dir = std::env::temp_dir();
        let error = read_spec(&dir).expect_err("directory");
        assert!(error.to_string().contains("not a regular file"), "{error}");
        assert!(read_spec(Path::new("/definitely/not/here.json")).is_err());
    }

    fn temp_file(name: &str, len: u64) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reelctl-overlay-unit-{}", std::process::id()));
        fs::create_dir_all(&dir).expect("dir");
        let path = dir.join(name);
        let file = fs::File::create(&path).expect("create");
        file.set_len(len).expect("sparse length");
        path
    }

    #[test]
    fn a_spec_file_is_read_up_to_the_cap_and_refused_one_byte_over() {
        let at_cap = temp_file("at-cap.json", MAX_SPEC_BYTES);
        assert_eq!(
            read_spec(&at_cap).expect("at the cap").len() as u64,
            MAX_SPEC_BYTES
        );
        let over = temp_file("over.json", MAX_SPEC_BYTES + 1);
        let error = read_spec(&over).expect_err("one over");
        assert!(error.to_string().contains("at most 65536"), "{error}");
    }

    #[test]
    fn a_chart_video_is_one_byte_to_the_cap_and_the_size_and_length_asked_for() {
        let probe = Probe {
            fps: 30.0,
            frames: 120,
            width: 320,
            height: 240,
            has_audio: false,
        };
        // A video of the cap's size passes the size check and is judged on its
        // picture: the wrong size is named, not the weight.
        let at_cap = temp_file("video-at-cap.mp4", MAX_ASSET_BYTES);
        let error = check_video(&at_cap, &probe, (640, 480), 4.0, 30.0).expect_err("size");
        assert!(error.to_string().contains("320x240 and 640x480"), "{error}");
        let over = temp_file("video-over.mp4", MAX_ASSET_BYTES + 1);
        let error = check_video(&over, &probe, (320, 240), 4.0, 30.0).expect_err("weight");
        assert!(error.to_string().contains("268435457 bytes"), "{error}");
        let empty = temp_file("video-empty.mp4", 0);
        let error = check_video(&empty, &probe, (320, 240), 4.0, 30.0).expect_err("empty");
        assert!(error.to_string().contains("0 bytes"), "{error}");
        // The right size and the wrong length.
        let one = temp_file("video-one.mp4", 1);
        let error = check_video(&one, &probe, (320, 240), 9.0, 30.0).expect_err("length");
        assert!(error.to_string().contains("within one frame"), "{error}");
    }
}
