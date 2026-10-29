//! The `ffprobe` and `ffmpeg` command line tools around the pipeline: read the
//! input's size and rate, decode it (and a video or image background) to raw
//! `rgb24` frames on pipes, and encode the result to a temporary file that is
//! renamed to the output when the run is complete.
//!
//! Nothing here uses the network: every input is opened with only the `file`
//! protocol allowed, as an absolute path. The argument lists are built by pure
//! functions so they are tested without running `ffmpeg`.

use std::ffi::OsString;
use std::fs;
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::background::{Background, RawFrames, SolidColor};
use crate::composite::OutputMode;
use crate::error::CompositorError;
use crate::model::MattingModel;
use crate::pipeline::{isolate, measure_centroids, Options};
use crate::ratio::{auto_downsample_ratio, validate_ratio};

/// Only this protocol may be opened by `ffmpeg` and `ffprobe`: a media file that is
/// a playlist or a concat script cannot make them fetch anything.
const PROTOCOLS: &str = "file";
/// The smallest side RVM is asked to matte.
const MIN_SIDE: u32 = 16;
/// How much of a child's stderr is kept for an error message.
const STDERR_KEEP: usize = 8 * 1024;
/// The read buffer between the decoder and the loop.
const PIPE_BUFFER: usize = 1 << 20;
/// Transfer characteristics of HDR video, which this module does not tone-map.
const HDR_TRANSFERS: [&str; 2] = ["arib-std-b67", "smpte2084"];

/// What goes behind the presenter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackgroundSpec {
    Colour([u8; 3]),
    /// An image or a video, scaled to cover the frame and looped.
    File(PathBuf),
}

/// One run of `isolate-presenter`.
#[derive(Debug, Clone)]
pub struct IsolateRequest {
    pub input: PathBuf,
    /// Required for [`OutputMode::Composite`], refused otherwise.
    pub background: Option<BackgroundSpec>,
    pub out: PathBuf,
    pub mode: OutputMode,
    /// The downsample ratio; `None` is RVM's automatic one.
    pub ratio: Option<f32>,
}

/// What a finished run reports.
#[derive(Debug, Clone, PartialEq)]
pub struct IsolateReport {
    pub out: PathBuf,
    pub mode: OutputMode,
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub ratio: f32,
    pub model_time: Duration,
    pub total_time: Duration,
}

/// What `ffprobe` says about a video stream.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoInfo {
    /// After the rotation a phone clip carries is applied.
    pub width: u32,
    pub height: u32,
    /// The average rate as `ffmpeg` writes it (`30000/1001`), and as a number.
    pub rate: Option<(String, f64)>,
    pub has_audio: bool,
    /// The colour matrix tag, when the file has one.
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
}

impl VideoInfo {
    /// The `in_color_matrix` for the scaler that converts this video to RGB: the
    /// file's own tag when it has one, and otherwise the convention players use
    /// (BT.709 from 720 lines, BT.601 below).
    pub fn matrix(&self) -> &'static str {
        match self.color_space.as_deref() {
            Some(_) => "auto",
            None if self.height >= 720 => "bt709",
            None => "bt601",
        }
    }
}

/// A run that has been checked and probed, and is ready for a model.
#[derive(Debug)]
pub struct Prepared {
    request: IsolateRequest,
    input: PathBuf,
    info: VideoInfo,
    rate: (String, f64),
    background: Option<PreparedBackground>,
    ratio: f32,
}

#[derive(Debug)]
enum PreparedBackground {
    Colour([u8; 3]),
    File { path: PathBuf, info: VideoInfo },
}

impl Prepared {
    /// The input's size, rate and the ratio the model will be given.
    pub fn info(&self) -> (u32, u32, f64, f32) {
        (self.info.width, self.info.height, self.rate.1, self.ratio)
    }
}

fn tool_error(tool: &str, error: std::io::Error) -> CompositorError {
    if error.kind() == std::io::ErrorKind::NotFound {
        CompositorError::run(format!(
            "{tool} was not found on PATH; install ffmpeg (macOS: `brew install ffmpeg`), which provides ffmpeg and ffprobe"
        ))
    } else {
        CompositorError::run(format!("cannot run {tool}: {error}"))
    }
}

fn parse_rate(text: &str) -> Option<f64> {
    let (numerator, denominator) = text.split_once('/')?;
    let (numerator, denominator): (f64, f64) = (numerator.parse().ok()?, denominator.parse().ok()?);
    (denominator > 0.0 && numerator > 0.0).then(|| numerator / denominator)
}

/// The first video stream of an `ffprobe -print_format json -show_streams` answer.
pub fn parse_video_info(json: &Value, path: &Path) -> Result<VideoInfo, CompositorError> {
    let streams = json
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let codec_type = |stream: &Value| {
        stream
            .get("codec_type")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let video = streams
        .iter()
        .find(|stream| codec_type(stream).as_deref() == Some("video"))
        .ok_or_else(|| CompositorError::setup(format!("{} has no video stream", path.display())))?;
    let has_audio = streams
        .iter()
        .any(|stream| codec_type(stream).as_deref() == Some("audio"));
    let text = |key: &str| {
        video
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty() && *value != "unknown" && *value != "unspecified")
            .map(str::to_string)
    };
    let rate = ["avg_frame_rate", "r_frame_rate"]
        .into_iter()
        .filter_map(text)
        .find_map(|text| parse_rate(&text).map(|rate| (text, rate)));
    let number = |key: &str| video.get(key).and_then(Value::as_u64).map(|n| n as u32);
    let (mut width, mut height) = match (number("width"), number("height")) {
        (Some(width), Some(height)) => (width, height),
        _ => {
            return Err(CompositorError::setup(format!(
                "{}: the video size is unknown",
                path.display()
            )))
        }
    };
    // A phone clip is stored sideways with a rotation to apply; ffmpeg applies it.
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
    Ok(VideoInfo {
        width,
        height,
        rate,
        has_audio,
        color_space: text("color_space"),
        color_transfer: text("color_transfer"),
    })
}

/// Refuse what this module cannot handle: HDR, and a frame too small to matte.
fn check_video(info: &VideoInfo, path: &Path, what: &str) -> Result<(), CompositorError> {
    if let Some(transfer) = info.color_transfer.as_deref() {
        if HDR_TRANSFERS.contains(&transfer) {
            return Err(CompositorError::setup(format!(
                "{} ({what}) is HDR video ({transfer}); isolate-presenter reads SDR only. Convert it \
to SDR first, for example with ffmpeg's zscale and tonemap filters",
                path.display()
            )));
        }
    }
    if info.width < MIN_SIDE || info.height < MIN_SIDE {
        return Err(CompositorError::setup(format!(
            "{} ({what}) is {}x{}; the smallest size is {MIN_SIDE}x{MIN_SIDE}",
            path.display(),
            info.width,
            info.height
        )));
    }
    Ok(())
}

fn probe(path: &Path) -> Result<VideoInfo, CompositorError> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            PROTOCOLS,
            "-print_format",
            "json",
            "-show_streams",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| tool_error("ffprobe", error))?;
    if !output.status.success() {
        return Err(CompositorError::setup(format!(
            "ffprobe cannot read {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| CompositorError::run(format!("cannot parse ffprobe's answer: {error}")))?;
    parse_video_info(&json, path)
}

fn local_file(path: &Path, what: &str) -> Result<PathBuf, CompositorError> {
    if !path.is_file() {
        return Err(CompositorError::setup(format!(
            "{what} {} is not a file",
            path.display()
        )));
    }
    fs::canonicalize(path).map_err(|error| {
        CompositorError::setup(format!("cannot find {what} {}: {error}", path.display()))
    })
}

/// The name of the temporary file written before the rename: the output's own name
/// with the process id, and the extension last, because `ffmpeg` picks the
/// container from it.
fn temporary_name(out: &Path) -> Result<String, CompositorError> {
    let stem = out.file_stem().and_then(|name| name.to_str());
    let extension = out.extension().and_then(|ext| ext.to_str());
    match (stem, extension) {
        (Some(stem), Some(extension)) => {
            Ok(format!(".{stem}.{}.tmp.{extension}", std::process::id()))
        }
        _ => Err(CompositorError::setup(format!(
            "--out {} needs a file name with an extension (for example out.mp4): ffmpeg picks the container from it",
            out.display()
        ))),
    }
}

/// Check the request and probe the input and the background. Nothing is written and
/// no model is needed, so a bad command fails before the weights are touched.
pub fn prepare(request: &IsolateRequest) -> Result<Prepared, CompositorError> {
    match (request.mode, &request.background) {
        (OutputMode::Composite, None) => return Err(CompositorError::setup(
            "--background is required for composite output (a #rrggbb colour, an image or a video)",
        )),
        (OutputMode::Matte | OutputMode::Rgba, Some(_)) => {
            return Err(CompositorError::setup(format!(
                "--background is not used with --mode {}",
                request.mode.name()
            )))
        }
        _ => {}
    }
    if request.out.is_dir() {
        return Err(CompositorError::setup(format!(
            "--out {} is a directory; name the file to write",
            request.out.display()
        )));
    }
    let extension = request
        .out
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    if request.mode == OutputMode::Rgba && extension.as_deref() != Some("mov") {
        return Err(CompositorError::setup(format!(
            "--mode rgba writes ProRes 4444, which needs a .mov file, not {}",
            request.out.display()
        )));
    }
    temporary_name(&request.out)?;
    let ratio = request.ratio.map(validate_ratio).transpose()?;

    let input = local_file(&request.input, "--input")?;
    let info = probe(&input)?;
    check_video(&info, &request.input, "the input")?;
    let rate = info.rate.clone().ok_or_else(|| {
        CompositorError::setup(format!(
            "{}: the frame rate is unknown",
            request.input.display()
        ))
    })?;
    let background = match &request.background {
        None => None,
        Some(BackgroundSpec::Colour(colour)) => Some(PreparedBackground::Colour(*colour)),
        Some(BackgroundSpec::File(path)) => {
            let path_abs = local_file(path, "--background")?;
            let background_info = probe(&path_abs)?;
            check_video(&background_info, path, "the background")?;
            Some(PreparedBackground::File {
                path: path_abs,
                info: background_info,
            })
        }
    };
    let ratio = ratio.unwrap_or_else(|| auto_downsample_ratio(info.width, info.height));
    Ok(Prepared {
        request: request.clone(),
        input,
        info,
        rate,
        background,
        ratio,
    })
}

fn base_args() -> Vec<OsString> {
    ["-hide_banner", "-loglevel", "error", "-nostdin"]
        .iter()
        .map(Into::into)
        .collect()
}

fn push_all(args: &mut Vec<OsString>, parts: &[&str]) {
    args.extend(parts.iter().map(OsString::from));
}

/// The decoder for the input: raw `rgb24` on stdout, at a constant `rate`, upright.
pub(crate) fn decoder_args(input: &Path, info: &VideoInfo, rate: &str) -> Vec<OsString> {
    let mut args = base_args();
    push_all(&mut args, &["-protocol_whitelist", PROTOCOLS, "-i"]);
    args.push(input.into());
    let filter = format!(
        "fps={rate},scale=in_color_matrix={}:out_range=full,format=rgb24",
        info.matrix()
    );
    push_all(
        &mut args,
        &[
            "-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &filter, "-f", "rawvideo", "-pix_fmt",
            "rgb24", "pipe:1",
        ],
    );
    args
}

/// The decoder for a background: looped without end, scaled to cover `width` by
/// `height` and cropped at the centre, at `rate`.
pub(crate) fn background_args(
    path: &Path,
    info: &VideoInfo,
    rate: &str,
    width: u32,
    height: u32,
) -> Vec<OsString> {
    let mut args = base_args();
    push_all(
        &mut args,
        &["-protocol_whitelist", PROTOCOLS, "-stream_loop", "-1", "-i"],
    );
    args.push(path.into());
    let filter = format!(
        "fps={rate},scale={width}:{height}:force_original_aspect_ratio=increase:in_color_matrix={}:out_range=full,\
format=rgb24,crop={width}:{height},setsar=1",
        info.matrix()
    );
    push_all(
        &mut args,
        &[
            "-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &filter, "-f", "rawvideo", "-pix_fmt",
            "rgb24", "pipe:1",
        ],
    );
    args
}

/// The encoder: raw frames of `mode` on stdin, the input's audio mapped from the
/// original file, written to `output`. Colour goes back to YUV with BT.709 limited
/// range and is tagged so.
pub(crate) fn encoder_args(
    mode: OutputMode,
    width: u32,
    height: u32,
    rate: &str,
    audio_from: Option<&Path>,
    output: &Path,
) -> Vec<OsString> {
    let mut args = base_args();
    args.push("-y".into());
    // The raw input is stdin: no protocol whitelist here (that would forbid `pipe`).
    push_all(
        &mut args,
        &[
            "-f",
            "rawvideo",
            "-pixel_format",
            mode.raw_pixel_format(),
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            rate,
            "-i",
            "pipe:0",
        ],
    );
    if let Some(path) = audio_from {
        push_all(&mut args, &["-protocol_whitelist", PROTOCOLS, "-i"]);
        args.push(path.into());
    }
    push_all(&mut args, &["-map", "0:v:0"]);
    if audio_from.is_some() {
        push_all(&mut args, &["-map", "1:a:0"]);
    }
    // Even sides: yuv420p needs them, and a crop of one pixel is cheaper than a failure.
    let even = "crop=trunc(iw/2)*2:trunc(ih/2)*2";
    // The encoder's own tag options write only the matrix and range here (ffmpeg 8.1
    // takes the primaries and transfer from the frames), so the frames carry all four.
    let tags = "setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv";
    let (filter, video, audio): (String, &[&str], &[&str]) = match mode {
        OutputMode::Rgba => (
            format!("{even},scale=out_color_matrix=bt709:out_range=tv,format=yuva444p10le,{tags}"),
            &[
                "-c:v",
                "prores_ks",
                "-profile:v",
                "4444",
                "-alpha_bits",
                "16",
                "-vendor",
                "apl0",
            ],
            &["-c:a", "pcm_s16le"],
        ),
        _ => (
            format!("{even},scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,{tags}"),
            &["-c:v", "libx264", "-preset", "veryfast", "-crf", "18"],
            &["-c:a", "aac", "-b:a", "192k"],
        ),
    };
    push_all(&mut args, &["-vf", &filter]);
    push_all(&mut args, video);
    push_all(
        &mut args,
        &[
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-color_range",
            "tv",
        ],
    );
    if audio_from.is_some() {
        push_all(&mut args, audio);
    }
    if mode != OutputMode::Rgba {
        push_all(&mut args, &["-movflags", "+faststart"]);
    }
    args.push(output.into());
    args
}

/// A child's stderr, read to the end on its own thread so a full pipe cannot stop it.
struct Drain(Option<JoinHandle<String>>);

impl Drain {
    fn start(child: &mut Child) -> Self {
        let handle = child.stderr.take().map(|mut stderr| {
            std::thread::spawn(move || {
                let mut text = Vec::new();
                let _ = stderr.read_to_end(&mut text);
                let start = text.len().saturating_sub(STDERR_KEEP);
                String::from_utf8_lossy(&text[start..]).into_owned()
            })
        });
        Self(handle)
    }

    fn finish(self) -> String {
        self.0
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default()
    }
}

/// A child process with its stderr drained.
struct Process {
    child: Child,
    stderr: Drain,
}

impl Process {
    fn spawn(args: &[OsString], stdin: Stdio, stdout: Stdio) -> Result<Self, CompositorError> {
        let mut child = Command::new("ffmpeg")
            .args(args)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| tool_error("ffmpeg", error))?;
        let stderr = Drain::start(&mut child);
        Ok(Self { child, stderr })
    }

    /// Stop it and collect what it said.
    fn stop(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stderr.finish()
    }

    /// Wait for it to end by itself: whether it succeeded, and what it said.
    fn finish(mut self) -> (bool, String) {
        let success = self.child.wait().is_ok_and(|status| status.success());
        (success, self.stderr.finish())
    }
}

/// A temporary output file, removed on every way out but a kill.
struct TempFile {
    path: PathBuf,
    keep: bool,
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().rev().take(12).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

impl Prepared {
    /// Run the frames of the input through `model` and write the output. The file
    /// appears at `--out` only when the whole run succeeded.
    pub fn run(self, model: &mut dyn MattingModel) -> Result<IsolateReport, CompositorError> {
        let started = Instant::now();
        let request = &self.request;
        let (width, height) = (self.info.width, self.info.height);
        let rate = self.rate.0.as_str();
        let dir = match request.out.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        fs::create_dir_all(&dir).map_err(|error| {
            CompositorError::run(format!("cannot create {}: {error}", dir.display()))
        })?;
        let dir = fs::canonicalize(&dir).map_err(|error| {
            CompositorError::run(format!("cannot find {}: {error}", dir.display()))
        })?;
        let temporary = TempFile {
            path: dir.join(temporary_name(&request.out)?),
            keep: false,
        };

        let mut decoder = Process::spawn(
            &decoder_args(&self.input, &self.info, rate),
            Stdio::null(),
            Stdio::piped(),
        )?;
        let audio = (self.info.has_audio && request.mode != OutputMode::Matte)
            .then_some(self.input.as_path());
        let encoder_result = Process::spawn(
            &encoder_args(request.mode, width, height, rate, audio, &temporary.path),
            Stdio::piped(),
            Stdio::null(),
        );
        let mut encoder = match encoder_result {
            Ok(encoder) => encoder,
            Err(error) => {
                decoder.stop();
                return Err(error);
            }
        };
        let mut background_process = match &self.background {
            Some(PreparedBackground::File { path, info }) => {
                let spawned = Process::spawn(
                    &background_args(path, info, rate, width, height),
                    Stdio::null(),
                    Stdio::piped(),
                );
                match spawned {
                    Ok(process) => Some(process),
                    Err(error) => {
                        decoder.stop();
                        encoder.stop();
                        return Err(error);
                    }
                }
            }
            _ => None,
        };

        let decoder_out = decoder.child.stdout.take();
        let encoder_in = encoder.child.stdin.take();
        let background_out = background_process
            .as_mut()
            .and_then(|process| process.child.stdout.take());
        let (Some(decoder_out), Some(encoder_in)) = (decoder_out, encoder_in) else {
            decoder.stop();
            encoder.stop();
            if let Some(process) = background_process {
                process.stop();
            }
            return Err(CompositorError::run("cannot connect to ffmpeg's pipes"));
        };

        let options = Options {
            width: width as usize,
            height: height as usize,
            ratio: self.ratio,
            mode: request.mode,
        };
        let outcome = {
            let mut solid = None;
            let mut frames = None;
            let source: Option<&mut dyn Background> = match (&self.background, background_out) {
                (Some(PreparedBackground::Colour(colour)), _) => {
                    Some(solid.insert(SolidColor(*colour)))
                }
                (Some(PreparedBackground::File { .. }), Some(out)) => {
                    Some(frames.insert(RawFrames::new(BufReader::with_capacity(PIPE_BUFFER, out))))
                }
                _ => None,
            };
            isolate(
                BufReader::with_capacity(PIPE_BUFFER, decoder_out),
                source,
                BufWriter::with_capacity(PIPE_BUFFER, encoder_in),
                model,
                &options,
            )
        };
        // The loop owns the encoder's stdin, so the encoder has seen end of input.
        // The background loops without end and the decoder may still be running if
        // the loop failed: stop them. The encoder ends by itself.
        if let Some(process) = background_process {
            process.stop();
        }
        let failed = outcome.is_err();
        let (decoder_ok, decoder_said) = if failed {
            (false, decoder.stop())
        } else {
            decoder.finish()
        };
        let (encoder_ok, encoder_said) = encoder.finish();

        let stats = match outcome {
            Ok(stats) => stats,
            Err(error) => {
                // The encoder dying first shows up as a broken pipe: its own words
                // are the reason.
                if !encoder_ok {
                    return Err(CompositorError::run(format!(
                        "ffmpeg failed while encoding {}:\n{}",
                        request.out.display(),
                        tail(&encoder_said)
                    )));
                }
                if !decoder_said.trim().is_empty() {
                    return Err(CompositorError::run(format!(
                        "{error}\nffmpeg said while decoding {}:\n{}",
                        request.input.display(),
                        tail(&decoder_said)
                    )));
                }
                return Err(error);
            }
        };
        if !decoder_ok {
            return Err(CompositorError::run(format!(
                "ffmpeg failed while decoding {}:\n{}",
                request.input.display(),
                tail(&decoder_said)
            )));
        }
        if !encoder_ok {
            return Err(CompositorError::run(format!(
                "ffmpeg failed while encoding {}:\n{}",
                request.out.display(),
                tail(&encoder_said)
            )));
        }
        fs::rename(&temporary.path, &request.out).map_err(|error| {
            CompositorError::run(format!("cannot write {}: {error}", request.out.display()))
        })?;
        let mut temporary = temporary;
        temporary.keep = true;
        Ok(IsolateReport {
            out: request.out.clone(),
            mode: request.mode,
            frames: stats.frames,
            width,
            height,
            fps: self.rate.1,
            ratio: self.ratio,
            model_time: stats.model_time,
            total_time: started.elapsed(),
        })
    }
}

/// The default downsample ratio when measuring where the presenter is: the matte
/// only has to be good enough to find its centre of mass.
pub const MEASURE_RATIO: f32 = 0.25;
/// Frames are decoded at most this wide to be measured.
pub const MEASURE_MAX_WIDTH: u32 = 1280;

/// One measuring run: the presenter's horizontal position on source frames
/// `start_frame..end_frame` of `input`.
#[derive(Debug, Clone)]
pub struct MeasureRequest {
    pub input: PathBuf,
    pub start_frame: i64,
    pub end_frame: i64,
    /// `None` for [`MEASURE_RATIO`].
    pub ratio: Option<f32>,
}

/// What a measuring run found.
#[derive(Debug, Clone)]
pub struct MeasureReport {
    /// One entry per frame from `start_frame`: the centre of mass as a share of the
    /// width, or `None` where no presenter was found (and for frames the decoder
    /// did not give, at the end).
    pub centroids: Vec<Option<f64>>,
    /// Frames that went through the model.
    pub frames: usize,
    pub width: u32,
    pub height: u32,
    pub ratio: f32,
    pub model_time: Duration,
    pub total_time: Duration,
}

/// A measuring run that has been checked and probed, and is ready for a model.
#[derive(Debug)]
pub struct PreparedMeasure {
    request: MeasureRequest,
    input: PathBuf,
    info: VideoInfo,
    size: (u32, u32),
    ratio: f32,
}

/// The size frames are decoded at to be measured: the source, scaled down to at
/// most [`MEASURE_MAX_WIDTH`] wide, both sides even.
pub fn measure_size(width: u32, height: u32) -> (u32, u32) {
    if width <= MEASURE_MAX_WIDTH {
        return (width - width % 2, height - height % 2);
    }
    let scaled =
        (f64::from(height) * f64::from(MEASURE_MAX_WIDTH) / f64::from(width)).round() as u32;
    (MEASURE_MAX_WIDTH, (scaled - scaled % 2).max(2))
}

/// The decoder for a measuring run: source frames `start..end` (counted as
/// `render` counts them, before any rate change), scaled to `width` x `height`,
/// raw `rgb24` on stdout.
pub(crate) fn measure_args(
    input: &Path,
    info: &VideoInfo,
    (start, end): (i64, i64),
    (width, height): (u32, u32),
) -> Vec<OsString> {
    let mut args = base_args();
    push_all(&mut args, &["-protocol_whitelist", PROTOCOLS, "-i"]);
    args.push(input.into());
    let filter = format!(
        "trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS,scale={width}:{height}:in_color_matrix={}:out_range=full,format=rgb24",
        info.matrix()
    );
    push_all(
        &mut args,
        &[
            "-map",
            "0:v:0",
            "-an",
            "-sn",
            "-dn",
            "-vf",
            &filter,
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ],
    );
    args
}

/// Check the request and probe the input; no model is needed yet, so a bad
/// command fails before the weights are touched.
pub fn prepare_measure(request: &MeasureRequest) -> Result<PreparedMeasure, CompositorError> {
    if request.start_frame < 0 || request.end_frame <= request.start_frame {
        return Err(CompositorError::setup(format!(
            "frames {}..{} are not a span to measure",
            request.start_frame, request.end_frame
        )));
    }
    let ratio = validate_ratio(request.ratio.unwrap_or(MEASURE_RATIO))?;
    let input = local_file(&request.input, "the project's media")?;
    let info = probe(&input)?;
    check_video(&info, &request.input, "the project's media")?;
    let size = measure_size(info.width, info.height);
    Ok(PreparedMeasure {
        request: request.clone(),
        input,
        info,
        size,
        ratio,
    })
}

impl PreparedMeasure {
    /// Decode the span and run every frame through `model`, keeping only where the
    /// presenter is.
    pub fn run(self, model: &mut dyn MattingModel) -> Result<MeasureReport, CompositorError> {
        let started = Instant::now();
        let span = (self.request.start_frame, self.request.end_frame);
        let mut decoder = Process::spawn(
            &measure_args(&self.input, &self.info, span, self.size),
            Stdio::null(),
            Stdio::piped(),
        )?;
        let Some(out) = decoder.child.stdout.take() else {
            decoder.stop();
            return Err(CompositorError::run("cannot connect to ffmpeg's pipe"));
        };
        let (width, height) = self.size;
        let outcome = measure_centroids(
            BufReader::with_capacity(PIPE_BUFFER, out),
            model,
            width as usize,
            height as usize,
            self.ratio,
        );
        let (decoder_ok, decoder_said) = if outcome.is_err() {
            (false, decoder.stop())
        } else {
            decoder.finish()
        };
        let (mut centroids, stats) = match outcome {
            Ok(result) => result,
            Err(error) if !decoder_said.trim().is_empty() => {
                return Err(CompositorError::run(format!(
                    "{error}\nffmpeg said while decoding {}:\n{}",
                    self.request.input.display(),
                    tail(&decoder_said)
                )))
            }
            Err(error) => return Err(error),
        };
        if !decoder_ok {
            return Err(CompositorError::run(format!(
                "ffmpeg failed while decoding {}:\n{}",
                self.request.input.display(),
                tail(&decoder_said)
            )));
        }
        let wanted = usize::try_from(span.1 - span.0).unwrap_or(0);
        centroids.truncate(wanted);
        centroids.resize(wanted, None);
        Ok(MeasureReport {
            centroids,
            frames: stats.frames,
            width,
            height,
            ratio: self.ratio,
            model_time: stats.model_time,
            total_time: started.elapsed(),
        })
    }
}

/// Check, probe and run in one call, for a model that is already open.
pub fn isolate_presenter(
    request: &IsolateRequest,
    model: &mut dyn MattingModel,
) -> Result<IsolateReport, CompositorError> {
    prepare(request)?.run(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn info(width: u32, height: u32) -> VideoInfo {
        VideoInfo {
            width,
            height,
            rate: Some(("30/1".to_string(), 30.0)),
            has_audio: true,
            color_space: None,
            color_transfer: None,
        }
    }

    fn stream_json() -> Value {
        json!({"streams": [
            {"codec_type": "video", "width": 1080, "height": 1920,
             "avg_frame_rate": "30000/1001", "r_frame_rate": "30/1",
             "color_space": "bt709", "color_transfer": "bt709"},
            {"codec_type": "audio"}
        ]})
    }

    #[test]
    fn a_probe_gives_size_rate_audio_and_colour() {
        let got = parse_video_info(&stream_json(), Path::new("a.mp4")).unwrap();
        assert_eq!((got.width, got.height), (1080, 1920));
        let (text, rate) = got.rate.clone().unwrap();
        assert_eq!(text, "30000/1001");
        assert!((rate - 29.97).abs() < 0.001);
        assert!(got.has_audio);
        assert_eq!(got.color_space.as_deref(), Some("bt709"));
        assert_eq!(got.matrix(), "auto");
    }

    #[test]
    fn a_rotated_phone_clip_swaps_its_sides() {
        let mut json = stream_json();
        json["streams"][0]["side_data_list"] = json!([{"rotation": -90}]);
        let got = parse_video_info(&json, Path::new("a.mp4")).unwrap();
        assert_eq!((got.width, got.height), (1920, 1080));
        // 180 degrees keeps the sides; the old tag is read too.
        let mut json = stream_json();
        json["streams"][0]["side_data_list"] = json!([{"rotation": 180}]);
        let got = parse_video_info(&json, Path::new("a.mp4")).unwrap();
        assert_eq!((got.width, got.height), (1080, 1920));
        let mut json = stream_json();
        json["streams"][0]["tags"] = json!({"rotate": "90"});
        let got = parse_video_info(&json, Path::new("a.mp4")).unwrap();
        assert_eq!((got.width, got.height), (1920, 1080));
    }

    #[test]
    fn a_zero_rate_falls_back_to_the_container_rate_and_unknown_is_none() {
        let mut json = stream_json();
        json["streams"][0]["avg_frame_rate"] = json!("0/0");
        let got = parse_video_info(&json, Path::new("a.mp4")).unwrap();
        assert_eq!(got.rate.unwrap().0, "30/1");
        json["streams"][0]["r_frame_rate"] = json!("0/0");
        assert!(parse_video_info(&json, Path::new("a.mp4"))
            .unwrap()
            .rate
            .is_none());
    }

    #[test]
    fn untagged_video_uses_the_players_convention() {
        assert_eq!(info(1280, 720).matrix(), "bt709");
        assert_eq!(info(1080, 1920).matrix(), "bt709");
        assert_eq!(info(640, 480).matrix(), "bt601");
        assert_eq!(info(1280, 719).matrix(), "bt601");
        let mut json = stream_json();
        json["streams"][0]["color_space"] = json!("unknown");
        let got = parse_video_info(&json, Path::new("a.mp4")).unwrap();
        assert_eq!(got.color_space, None);
        assert_eq!(got.matrix(), "bt709");
    }

    #[test]
    fn no_video_stream_or_size_is_a_setup_error() {
        let audio_only = json!({"streams": [{"codec_type": "audio"}]});
        let error = parse_video_info(&audio_only, Path::new("a.mp3")).unwrap_err();
        assert!(error.is_setup());
        assert!(error.to_string().contains("no video stream"));
        let no_size = json!({"streams": [{"codec_type": "video"}]});
        assert!(parse_video_info(&no_size, Path::new("a.mp4"))
            .unwrap_err()
            .to_string()
            .contains("size is unknown"));
        assert!(parse_video_info(&json!({}), Path::new("a.mp4")).is_err());
    }

    #[test]
    fn hdr_and_tiny_video_are_refused() {
        // Spelled out, not read from the constant, so that changing it fails here.
        for transfer in ["arib-std-b67", "smpte2084"] {
            let mut hdr = info(1920, 1080);
            hdr.color_transfer = Some(transfer.to_string());
            let error = check_video(&hdr, Path::new("hdr.mov"), "the input").unwrap_err();
            assert!(error.is_setup());
            assert!(error.to_string().contains("HDR"), "{error}");
            assert!(error.to_string().contains(transfer), "{error}");
        }
        let mut sdr = info(1920, 1080);
        sdr.color_transfer = Some("bt709".to_string());
        assert!(check_video(&sdr, Path::new("a.mp4"), "the input").is_ok());
        for (w, h) in [(15, 100), (100, 15), (8, 8)] {
            let error = check_video(&info(w, h), Path::new("a.mp4"), "the input").unwrap_err();
            assert!(error.to_string().contains("smallest size"), "{error}");
        }
        assert!(check_video(&info(16, 16), Path::new("a.mp4"), "the input").is_ok());
    }

    #[test]
    fn the_decoder_reads_one_local_file_to_rgb_at_a_constant_rate() {
        let args = strings(&decoder_args(
            Path::new("/v/in.mp4"),
            &info(1080, 1920),
            "30/1",
        ));
        let text = args.join(" ");
        assert!(
            text.contains("-protocol_whitelist file -i /v/in.mp4"),
            "{text}"
        );
        assert!(text.contains("fps=30/1,scale=in_color_matrix=bt709:out_range=full,format=rgb24"));
        assert!(
            text.ends_with("-f rawvideo -pix_fmt rgb24 pipe:1"),
            "{text}"
        );
        assert!(text.contains("-an"));
    }

    #[test]
    fn the_background_loops_and_covers_the_frame() {
        let args = strings(&background_args(
            Path::new("/v/bg.png"),
            &info(100, 60),
            "30/1",
            640,
            360,
        ));
        let text = args.join(" ");
        assert!(text.contains("-stream_loop -1 -i /v/bg.png"), "{text}");
        assert!(text.contains("scale=640:360:force_original_aspect_ratio=increase"));
        assert!(
            text.contains("format=rgb24,crop=640:360,setsar=1"),
            "{text}"
        );
        assert!(text.contains("in_color_matrix=bt601"), "{text}");
    }

    #[test]
    fn the_encoder_takes_raw_frames_on_stdin_without_a_protocol_whitelist_before_them() {
        let args = strings(&encoder_args(
            OutputMode::Composite,
            640,
            360,
            "30/1",
            Some(Path::new("/v/in.mp4")),
            Path::new("/o/.out.1.tmp.mp4"),
        ));
        let text = args.join(" ");
        let pipe = args.iter().position(|arg| arg == "pipe:0").unwrap();
        assert!(
            !args[..pipe].contains(&"-protocol_whitelist".to_string()),
            "{text}"
        );
        assert!(text.contains("-pixel_format rgb24 -video_size 640x360 -framerate 30/1 -i pipe:0"));
        // The audio file is the second input, with the whitelist.
        assert!(
            text.contains("-protocol_whitelist file -i /v/in.mp4"),
            "{text}"
        );
        assert!(text.contains("-map 0:v:0 -map 1:a:0"), "{text}");
        assert!(text.contains("out_color_matrix=bt709:out_range=tv,format=yuv420p,setparams="));
        assert!(text.contains("colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv"));
        assert!(text.contains("-colorspace bt709 -color_primaries bt709 -color_trc bt709"));
        assert!(text.contains("-c:v libx264"));
        assert!(text.contains("-c:a aac"));
        assert!(text.ends_with("/o/.out.1.tmp.mp4"), "{text}");
    }

    #[test]
    fn matte_has_no_audio_and_rgba_is_prores_with_alpha() {
        let matte = strings(&encoder_args(
            OutputMode::Matte,
            64,
            48,
            "25/1",
            None,
            Path::new("/o/m.mp4"),
        ))
        .join(" ");
        assert!(matte.contains("-pixel_format gray"), "{matte}");
        assert!(!matte.contains("1:a:0"), "{matte}");
        assert!(!matte.contains("-c:a"), "{matte}");
        let rgba = strings(&encoder_args(
            OutputMode::Rgba,
            64,
            48,
            "25/1",
            Some(Path::new("/v/in.mp4")),
            Path::new("/o/r.mov"),
        ))
        .join(" ");
        assert!(rgba.contains("-pixel_format rgba"), "{rgba}");
        assert!(rgba.contains("prores_ks"), "{rgba}");
        assert!(rgba.contains("yuva444p10le"), "{rgba}");
        assert!(rgba.contains("-c:a pcm_s16le"), "{rgba}");
        assert!(!rgba.contains("faststart"), "{rgba}");
    }

    #[test]
    fn the_temporary_name_ends_with_the_extension() {
        let name = temporary_name(Path::new("dir/out.mp4")).unwrap();
        assert!(name.starts_with(".out."), "{name}");
        assert!(name.ends_with(".tmp.mp4"), "{name}");
        assert!(temporary_name(Path::new("dir/noext"))
            .unwrap_err()
            .is_setup());
    }

    fn request(mode: OutputMode, background: Option<BackgroundSpec>) -> IsolateRequest {
        IsolateRequest {
            input: PathBuf::from("/no/such/input.mp4"),
            background,
            out: PathBuf::from("/tmp/out.mp4"),
            mode,
            ratio: None,
        }
    }

    #[test]
    fn the_background_rules_are_checked_before_any_file_is_read() {
        let error = prepare(&request(OutputMode::Composite, None)).unwrap_err();
        assert!(error.is_setup());
        assert!(error.to_string().contains("--background is required"));
        let colour = Some(BackgroundSpec::Colour([0, 0, 0]));
        for mode in [OutputMode::Matte, OutputMode::Rgba] {
            let error = prepare(&request(mode, colour.clone())).unwrap_err();
            assert!(error.to_string().contains("not used with"), "{error}");
        }
    }

    #[test]
    fn the_output_path_and_ratio_are_checked_before_any_file_is_read() {
        let colour = Some(BackgroundSpec::Colour([0, 0, 0]));
        let mut bad = request(OutputMode::Composite, colour.clone());
        bad.out = std::env::temp_dir();
        assert!(prepare(&bad)
            .unwrap_err()
            .to_string()
            .contains("is a directory"));
        let mut bad = request(OutputMode::Rgba, None);
        bad.out = PathBuf::from("/tmp/x.mp4");
        assert!(prepare(&bad).unwrap_err().to_string().contains(".mov"));
        let mut bad = request(OutputMode::Composite, colour.clone());
        bad.ratio = Some(1.5);
        assert!(prepare(&bad)
            .unwrap_err()
            .to_string()
            .contains("--downsample-ratio"));
        let mut bad = request(OutputMode::Composite, colour);
        bad.out = PathBuf::from("/tmp/noextension");
        assert!(prepare(&bad).unwrap_err().to_string().contains("extension"));
    }

    #[test]
    fn a_missing_input_is_a_setup_error() {
        let colour = Some(BackgroundSpec::Colour([0, 0, 0]));
        let error = prepare(&request(OutputMode::Composite, colour)).unwrap_err();
        assert!(error.is_setup());
        assert!(error.to_string().contains("--input"), "{error}");
    }

    #[test]
    fn only_the_last_lines_of_ffmpegs_complaint_are_kept() {
        let text: String = (0..30).map(|i| format!("line {i}\n")).collect();
        let kept = tail(&text);
        assert_eq!(kept.lines().count(), 12);
        assert!(kept.starts_with("line 18"));
        assert!(kept.ends_with("line 29"));
    }

    #[test]
    fn measuring_decodes_the_span_scaled_and_untouched_in_rate() {
        let args = strings(&measure_args(
            Path::new("/media/take.mp4"),
            &info(1920, 1080),
            (300, 450),
            measure_size(1920, 1080),
        ));
        let filter = &args[args.iter().position(|arg| arg == "-vf").unwrap() + 1];
        assert_eq!(
            filter,
            "trim=start_frame=300:end_frame=450,setpts=PTS-STARTPTS,scale=1280:720:in_color_matrix=bt709:out_range=full,format=rgb24"
        );
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-protocol_whitelist", "file"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-fps_mode", "passthrough"]));
        assert_eq!(args.last().map(String::as_str), Some("pipe:1"));
    }

    #[test]
    fn the_measuring_size_is_even_and_at_most_1280_wide() {
        assert_eq!(measure_size(1280, 720), (1280, 720));
        assert_eq!(measure_size(641, 361), (640, 360));
        assert_eq!(measure_size(3840, 2160), (1280, 720));
        assert_eq!(measure_size(1920, 1081), (1280, 720));
    }

    #[test]
    fn a_bad_span_is_refused_before_anything_runs() {
        let request = |start, end| MeasureRequest {
            input: PathBuf::from("/no/such/file.mp4"),
            start_frame: start,
            end_frame: end,
            ratio: None,
        };
        for (start, end) in [(10, 10), (10, 5), (-1, 5)] {
            match prepare_measure(&request(start, end)) {
                Err(CompositorError::Setup(message)) => {
                    assert!(message.contains("not a span"), "{message}")
                }
                other => panic!("{other:?}"),
            }
        }
        match prepare_measure(&request(0, 5)) {
            Err(CompositorError::Setup(message)) => {
                assert!(message.contains("not a file"), "{message}")
            }
            other => panic!("{other:?}"),
        }
    }
}
