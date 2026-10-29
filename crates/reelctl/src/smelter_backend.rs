//! The Smelter render backend: start a separately installed Smelter, send it the
//! requests `timeline::build_smelter_plan` made, wait for the output to finish, and
//! check what it wrote. Smelter is never downloaded, vendored or left running; one
//! server renders one project. Spec: `docs/specs/smelter-backend.md`; decision:
//! ADR-023.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use timeline::{build_smelter_plan, SmelterFiles, SmelterPlan, SmelterRequest, OUTPUT_ID};

use crate::render_backend::{draw_card_files, RenderBackend, RenderJob, RenderOutcome};
use crate::smelter_client::{Client, Event};
use crate::timeline_cmd::{
    local_media_file, render_temp_name, replace_file, spawn_error, usage, utf8, TempDir, TempFile,
};

/// Where to get Smelter, for the message when it is missing.
const WHERE_TO_GET: &str = "https://github.com/software-mansion/smelter/releases (on an Apple-silicon Mac, smelter_darwin_aarch64.tar.gz; it needs FFmpeg 8 from Homebrew)";

/// How the backend finds and drives Smelter.
#[derive(Debug, Clone)]
pub struct SmelterBackend {
    /// `--smelter` or `SMELTER_BIN`; else `smelter` on `PATH`.
    pub binary: Option<PathBuf>,
    /// A server that is already listening on this port is used instead of a free one
    /// (tests, with a fake). The process is still started.
    pub port: Option<u16>,
    /// How long the server has to answer `GET /status`.
    pub startup_timeout: Duration,
    /// The fixed part of the time a render may take.
    pub render_timeout: Duration,
    /// Seconds a render may take for each second of timeline, added to the fixed part.
    pub render_per_second: f64,
}

impl SmelterBackend {
    pub fn new(binary: Option<PathBuf>) -> Self {
        Self {
            binary,
            port: None,
            startup_timeout: Duration::from_secs(30),
            render_timeout: Duration::from_secs(60),
            render_per_second: 10.0,
        }
    }
}

/// The program to run: `--smelter` (a file, or a directory holding `smelter`), else the
/// first `smelter` on `path`.
pub fn resolve_binary(given: Option<&Path>, path: Option<OsString>) -> Result<PathBuf> {
    let missing = |why: String| {
        anyhow!(
            "{why}; point reelctl at the Smelter server with --smelter <path> or SMELTER_BIN=<path> (the `smelter` or `smelter_main` file), or render with --backend ffmpeg. Smelter is a separate program that reelctl never installs: {WHERE_TO_GET}"
        )
    };
    if let Some(given) = given {
        let candidate = if given.is_dir() {
            given.join("smelter")
        } else {
            given.to_path_buf()
        };
        return if candidate.is_file() {
            Ok(candidate)
        } else {
            Err(missing(format!(
                "Smelter was not found at {}",
                candidate.display()
            )))
        };
    }
    let found = path.and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join("smelter"))
            .find(|candidate| candidate.is_file())
    });
    found.ok_or_else(|| missing("Smelter was not found on PATH".to_string()))
}

/// A port that was free a moment ago.
fn free_port() -> Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("find a free port")?;
    Ok(listener.local_addr()?.port())
}

/// Kills and reaps the child when dropped, on every way out.
struct Server {
    child: Child,
    /// A cancel (SIGTERM to `reelctl`) reaches the server too.
    _tracked: progress::Tracked,
}

impl Server {
    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The last lines of the server's log, for a message: the `message` of a JSON line,
/// else the line.
fn log_tail(log: &str, lines: usize) -> String {
    let shown: Vec<String> = log
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let text = serde_json::from_str::<Value>(line)
                .ok()
                .and_then(|json| {
                    json.pointer("/fields/message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| line.to_string());
            text.chars().take(400).collect::<String>()
        })
        .collect();
    let skip = shown.len().saturating_sub(lines);
    shown[skip..].join("\n")
}

/// The messages of every `ERROR` line of Smelter's JSON log. A scheduled update that
/// fails is only logged (the HTTP call that scheduled it had already succeeded).
pub fn smelter_errors(log: &str) -> Vec<String> {
    log.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|json| {
            json.get("level")
                .and_then(Value::as_str)
                .is_some_and(|level| level.eq_ignore_ascii_case("error"))
        })
        .map(|json| {
            json.pointer("/fields/message")
                .and_then(Value::as_str)
                .unwrap_or("(an error without a message)")
                .to_string()
        })
        .collect()
}

fn read_log(path: &Path) -> String {
    let mut text = String::new();
    if let Ok(mut file) = File::open(path) {
        let _ = file.read_to_string(&mut text);
    }
    text
}

/// Whether ffprobe's `color_space` says the source carries no colour tag.
pub fn is_untagged(color_space: &str) -> bool {
    matches!(
        color_space.trim().to_ascii_lowercase().as_str(),
        "" | "unknown" | "unspecified" | "n/a"
    )
}

/// ffprobe's `color_space` of the first video stream; `None` when ffprobe cannot say.
fn color_space(path: &Path) -> Option<String> {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-protocol_whitelist", "file"])
        .args([
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=color_space",
        ])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// The number of video frames in `path`, counted as packets (one per frame in mp4).
fn count_frames(path: &Path) -> Result<i64> {
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-protocol_whitelist",
            "file",
            "-count_packets",
        ])
        .args([
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=nb_read_packets",
        ])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .stdin(Stdio::null());
    let output =
        progress::tracked_output(&mut command).map_err(|error| spawn_error("ffprobe", error))?;
    if !output.status.success() {
        bail!(
            "ffprobe could not read what Smelter wrote ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .with_context(|| "ffprobe gave no frame count for what Smelter wrote")
}

/// Copy the first `frames` video frames of `from` (and the sound up to that time) into
/// `to`, without re-encoding. The video has no B-frames (the plan asks for none), so
/// the first `frames` packets are the first `frames` frames.
fn trim_to_frames(from: &Path, to: &Path, frames: i64, fps: f64) -> Result<()> {
    let length = format!("{:.6}", frames as f64 / fps);
    let mut command = Command::new("ffmpeg");
    command
        .args(["-y", "-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(["-protocol_whitelist", "file", "-i"])
        .arg(from)
        .args([
            "-c",
            "copy",
            "-frames:v",
            &frames.to_string(),
            "-t",
            &length,
        ])
        .args(["-movflags", "+faststart"])
        .arg(to)
        .stdin(Stdio::null());
    let output =
        progress::tracked_output(&mut command).map_err(|error| spawn_error("ffmpeg", error))?;
    if !output.status.success() {
        bail!(
            "ffmpeg could not trim what Smelter wrote ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

impl SmelterBackend {
    /// Start the server and wait until it answers.
    fn start_server(
        &self,
        binary: &Path,
        plan: &SmelterPlan,
        log: &Path,
    ) -> Result<(Server, Client)> {
        let port = match self.port {
            Some(port) => port,
            None => free_port()?,
        };
        let sink = File::create(log).with_context(|| format!("create {}", log.display()))?;
        let child = Command::new(binary)
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("REELCTL_WHISPER_MODEL")
            .env_remove("REELCTL_TRANSNET_WEIGHTS")
            .env("SMELTER_API_PORT", port.to_string())
            .env("SMELTER_OUTPUT_FRAMERATE", &plan.frame_rate)
            .env("SMELTER_OFFLINE_PROCESSING_ENABLE", "true")
            .env("SMELTER_START_WHIP_WHEP_SERVER", "false")
            .env("SMELTER_START_RTMP_SERVER", "false")
            .env("SMELTER_WEB_RENDERER_ENABLE", "false")
            .env("SMELTER_LOGGER_FORMAT", "json")
            .stdin(Stdio::null())
            .stdout(Stdio::from(sink.try_clone()?))
            .stderr(Stdio::from(sink))
            .spawn()
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => anyhow!(
                    "cannot run Smelter at {}: {error}; check that the file exists and is executable (SMELTER_BIN / --smelter)",
                    binary.display()
                ),
                _ => anyhow!("cannot run Smelter at {}: {error}", binary.display()),
            })?;
        let tracked = progress::track(child.id());
        let mut server = Server {
            child,
            _tracked: tracked,
        };
        let client = Client::new(port);
        let deadline = Instant::now() + self.startup_timeout;
        loop {
            if let Some(status) = server.child.try_wait()? {
                let text = read_log(log);
                let mut message = format!(
                    "Smelter ({}) exited before it answered ({status}). Its log ended:\n{}",
                    binary.display(),
                    log_tail(&text, 8)
                );
                if text.contains("Library not loaded") {
                    message.push_str("\nSmelter 0.6 links Homebrew's FFmpeg 8 (libavcodec 62): install it with `brew install ffmpeg`.");
                }
                bail!(message);
            }
            if client.get("/status").is_ok_and(|reply| reply.ok()) {
                return Ok((server, client));
            }
            if Instant::now() >= deadline {
                bail!(
                    "Smelter ({}) did not answer GET /status on port {port} within {} s",
                    binary.display(),
                    self.startup_timeout.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Send every request but the last (`start`); refuse what Smelter cannot read.
    fn register(
        &self,
        client: &Client,
        plan: &SmelterPlan,
        requests: &[SmelterRequest],
        media: &[PathBuf],
    ) -> Result<()> {
        for request in requests {
            let reply = client
                .post(&request.path, &request.body)
                .with_context(|| format!("send {} {}", request.method, request.path))?;
            let input = request
                .path
                .strip_prefix("/api/input/")
                .and_then(|rest| rest.strip_suffix("/register"))
                .and_then(|id| plan.inputs.iter().find(|input| input.id == id));
            if let Some(input) = input {
                let path = media[input.media].display();
                if !reply.ok() {
                    return Err(usage(format!(
                        "Smelter cannot read {path}: {}. Its mp4 input reads H.264 video and AAC audio only; convert the file first, for example: ffmpeg -i {path} -c:v libx264 -c:a aac converted.mp4",
                        reply.explain()
                    )));
                }
                let heard = serde_json::from_str::<Value>(&reply.body)
                    .ok()
                    .and_then(|json| json.get("audio_duration_ms").cloned())
                    .is_some_and(|value| !value.is_null());
                if input.audio && !heard {
                    return Err(usage(format!(
                        "Smelter heard no audio in {path}: its mp4 input reads AAC audio only, and the project plays this file's sound. Re-encode the audio: ffmpeg -i {path} -c:v copy -c:a aac converted.mp4"
                    )));
                }
            } else if !reply.ok() {
                bail!(
                    "Smelter rejected {} {}: {}",
                    request.method,
                    request.path,
                    reply.explain()
                );
            }
        }
        Ok(())
    }
}

impl RenderBackend for SmelterBackend {
    fn name(&self) -> &'static str {
        "smelter"
    }

    fn render(&self, job: &RenderJob<'_>) -> Result<RenderOutcome> {
        let binary = resolve_binary(self.binary.as_deref(), std::env::var_os("PATH"))?;
        let plan = build_smelter_plan(job.project, job.size)
            .map_err(|error| usage(format!("{}: {error}", job.project_path.display())))?;
        let mut media = Vec::with_capacity(plan.media.len());
        for url in &plan.media {
            media.push(
                local_media_file(url)
                    .map_err(|error| usage(format!("{}: {error}", job.project_path.display())))?,
            );
        }
        if job.out.is_dir() {
            return Err(usage(format!(
                "--out {} is a directory; name the file to write",
                job.out.display()
            )));
        }
        let temporary_name = render_temp_name(job.out)?;
        let trimmed_name = temporary_name.replacen(".tmp.", ".trim.", 1);
        let scratch = TempDir::create()?;
        let (cards, mut notes) = draw_card_files(&plan.cards, plan.size, &scratch.0)?;
        notes.splice(0..0, plan.notes.iter().cloned());
        for path in &media {
            if color_space(path).is_some_and(|space| is_untagged(&space)) {
                notes.push(format!(
                    "render: {} has no colour tags; Smelter reads it as BT.709, so its colour can differ from the ffmpeg render (tag the source, or use --backend ffmpeg)",
                    path.display()
                ));
            }
        }

        let dir = match job.out.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        };
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let dir = fs::canonicalize(&dir).with_context(|| format!("find {}", dir.display()))?;
        let temporary = TempFile::new(dir.join(temporary_name));

        let media_text: Vec<String> = media.iter().map(|path| utf8(path)).collect::<Result<_>>()?;
        let card_text: Vec<String> = cards.iter().map(|path| utf8(path)).collect::<Result<_>>()?;
        let out_text = utf8(&temporary.path)?;
        let requests = plan
            .requests(&SmelterFiles {
                media: &media_text,
                cards: &card_text,
                out: &out_text,
            })
            .map_err(|error| usage(error.to_string()))?;
        let Some((start, before_start)) = requests.split_last() else {
            bail!("the plan made no requests");
        };

        let started = Instant::now();
        let log = scratch.0.join("smelter.log");
        let (mut server, client) = self.start_server(&binary, &plan, &log)?;
        self.register(&client, &plan, before_start, &media)?;
        let mut events = client.events()?;
        let reply = client.post(&start.path, &start.body)?;
        if !reply.ok() {
            bail!("Smelter rejected start: {}", reply.explain());
        }
        let seconds = plan.frames as f64 / plan.fps;
        let allowed =
            self.render_timeout + Duration::from_secs_f64(self.render_per_second * seconds);
        let deadline = Instant::now() + allowed;
        loop {
            match events.next(deadline)? {
                Event::Text(text) => {
                    let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                    if json.get("type").and_then(Value::as_str) == Some("OUTPUT_DONE")
                        && json.get("output_id").and_then(Value::as_str) == Some(OUTPUT_ID)
                    {
                        break;
                    }
                }
                Event::Closed => bail!(
                    "Smelter closed its event stream before the output was done; its log ended:\n{}",
                    log_tail(&read_log(&log), 8)
                ),
                Event::Timeout => bail!(
                    "Smelter did not finish within {} s; its log ended:\n{}",
                    allowed.as_secs(),
                    log_tail(&read_log(&log), 8)
                ),
            }
        }
        let errors = smelter_errors(&read_log(&log));
        server.stop();
        if !errors.is_empty() {
            bail!(
                "Smelter logged {} error{} while rendering:\n{}",
                errors.len(),
                if errors.len() == 1 { "" } else { "s" },
                errors
                    .iter()
                    .take(3)
                    .map(|error| format!("  {error}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let written = count_frames(&temporary.path)?;
        if written < plan.frames {
            bail!(
                "Smelter wrote {written} video frames but the project has {}; the file is not kept (Smelter ends an output a few frames early now and then; run it again, or use --backend ffmpeg)",
                plan.frames
            );
        }
        // Smelter was asked to run a few frames long; cut the file to the exact length.
        let trimmed = TempFile::new(dir.join(trimmed_name));
        trim_to_frames(&temporary.path, &trimmed.path, plan.frames, plan.fps)?;
        let frames = count_frames(&trimmed.path)?;
        if frames != plan.frames {
            bail!(
                "after trimming, the file has {frames} video frames but the project has {}; the file is not kept",
                plan.frames
            );
        }
        let elapsed = started.elapsed().as_millis();
        replace_file(trimmed, job.out)?;
        Ok(RenderOutcome {
            frames: plan.frames,
            duration_ms: (plan.frames as f64 * 1000.0 / plan.fps).round() as u64,
            size: plan.size,
            backend_ms: elapsed,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::smelter_fake::{frame, FakeSmelter, Recorded};
    use crate::understand_cmd::ExitWith;
    use std::os::unix::fs::PermissionsExt;
    use timeline::{init, InitSpec, Timeline};

    const DONE: &[u8] = br#"{"type":"OUTPUT_DONE","output_id":"out"}"#;
    /// A line of Smelter 0.6.0's JSON log, as it wrote it for a scheduled update that
    /// had no `audio` (2026-10-29).
    const ERROR_LINE: &str = r#"{"timestamp":"2026-10-29T05:12:18.606565Z","level":"ERROR","fields":{"message":"Error while running scheduled output update for pts 200ms: Audio and video specification for output \"o\" doesn't match one provided in register output request.\n    If audio or video was specified on register, it has to be specified in update."},"target":"smelter::routes::update_output","span":{"name":"Queue"},"spans":[{"name":"Queue"}]}"#;
    const WARN_LINE: &str = r#"{"timestamp":"2026-10-29T05:12:18.642358Z","level":"WARN","fields":{"message":"Failed to send decoded video frame from decoder. Channel closed."},"target":"smelter_core::pipeline::decoder::decoder_thread_video"}"#;

    /// A directory under the system temp dir, removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "reelctl-smelter-{name}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("scratch dir");
            Self(path)
        }

        fn script(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
            path
        }

        fn leftovers(&self, dir: &str) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.0.join(dir))
                .map(|entries| {
                    entries
                        .filter_map(|entry| entry.ok())
                        .map(|entry| entry.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A small H.264 mp4 of `frames` frames (no B-frames), made by ffmpeg.
    fn make_mp4(path: &Path, frames: u32) {
        let status = Command::new("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-nostdin"])
            .args(["-f", "lavfi", "-i", "testsrc=s=64x64:r=30"])
            .args(["-frames:v", &frames.to_string()])
            .args(["-c:v", "libx264", "-bf", "0", "-pix_fmt", "yuv420p", "-an"])
            .arg(path)
            .status()
            .expect("ffmpeg is needed to run these tests");
        assert!(status.success());
    }

    /// A project of one clip of `frames` frames at 30 fps over `media`.
    fn project(media: &Path, frames: i64, audio: bool) -> Timeline {
        init(&InitSpec {
            media_path: media.to_str().expect("utf8").to_string(),
            fps: 30.0,
            frames,
            width: 64,
            height: 64,
            has_audio: audio,
            dna: None,
        })
        .expect("init")
    }

    /// A fake that answers `/status` once `ready` exists, puts `prepared` at the output's
    /// path when it is registered, and reports the output done after `start`.
    fn fake_writing(
        prepared: PathBuf,
        ready: PathBuf,
        extra: impl Fn(&Recorded) -> Option<(u16, String)> + Send + Sync + 'static,
    ) -> FakeSmelter {
        let fake = FakeSmelter::start(move |request| {
            // Not ready until the child has started, so a test cannot finish a render
            // before the child it spawned has run.
            if request.path == "/status" && !ready.exists() {
                return Some((503, "{}".to_string()));
            }
            if request.path == "/api/output/out/register" {
                if let Some(path) = request.body["path"].as_str() {
                    fs::copy(&prepared, path).expect("copy the prepared output");
                }
            }
            extra(request)
        });
        fake.events_after_start(vec![frame(0x1, true, DONE)]);
        fake
    }

    fn backend(script: PathBuf, port: u16) -> SmelterBackend {
        SmelterBackend {
            binary: Some(script),
            port: Some(port),
            startup_timeout: Duration::from_secs(10),
            render_timeout: Duration::from_secs(10),
            render_per_second: 0.0,
        }
    }

    fn render(backend: &SmelterBackend, timeline: &Timeline, out: &Path) -> Result<RenderOutcome> {
        backend.render(&RenderJob {
            project_path: Path::new("project.otio"),
            project: timeline,
            out,
            size: None,
        })
    }

    fn exit_code(error: &anyhow::Error) -> Option<i32> {
        error.downcast_ref::<ExitWith>().map(|exit| exit.code)
    }

    fn alive(pid: &str) -> bool {
        Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// No process is left running `sleep <marker>`, whether or not the child got as far
    /// as writing its pid.
    fn assert_none_left(marker: &str) {
        for _ in 0..60 {
            let found = Command::new("pgrep")
                .args(["-f", &format!("sleep {marker}")])
                .output()
                .map(|output| !output.stdout.is_empty())
                .unwrap_or(false);
            if !found {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("a child sleeping {marker} is still running");
    }

    fn assert_dies(pidfile: &Path) {
        let pid = fs::read_to_string(pidfile).expect("the child wrote its pid");
        let pid = pid.trim();
        for _ in 0..60 {
            if !alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the child {pid} is still running");
    }

    /// Everything a render test needs: a media file (never read: the fake does the
    /// rendering), a result for the fake to write, a child script that records its pid
    /// and waits.
    struct Rig {
        /// A sleep length no other test uses, so `pgrep` finds this rig's child alone.
        marker: String,
        dir: Scratch,
        media: PathBuf,
        prepared: PathBuf,
        child: PathBuf,
        pidfile: PathBuf,
        out: PathBuf,
    }

    fn rig(name: &str, written_frames: u32) -> Rig {
        let dir = Scratch::new(name);
        let media = dir.0.join("take.mp4");
        fs::write(&media, b"not a video; the fake renders").expect("media");
        let prepared = dir.0.join("prepared.mp4");
        make_mp4(&prepared, written_frames);
        let pidfile = dir.0.join("pid");
        let marker = format!(
            "120.{}",
            dir.0
                .file_name()
                .map(
                    |name| name.to_string_lossy().bytes().fold(7u64, |sum, byte| sum
                        .wrapping_mul(31)
                        .wrapping_add(u64::from(byte)))
                )
                .unwrap_or(0)
        );
        let child = dir.script(
            "smelter",
            &format!("echo $$ > {}\nexec sleep {marker}", pidfile.display()),
        );
        fs::create_dir_all(dir.0.join("out")).expect("out dir");
        let out = dir.0.join("out").join("result.mp4");
        Rig {
            marker,
            dir,
            media,
            prepared,
            child,
            pidfile,
            out,
        }
    }

    fn posts(fake: &FakeSmelter) -> Vec<String> {
        fake.requests()
            .into_iter()
            .filter(|request| request.method == "POST")
            .map(|request| request.path)
            .collect()
    }

    #[test]
    fn a_render_registers_in_order_starts_last_trims_and_leaves_nothing_behind() {
        let rig = rig("happy", 34);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        let timeline = project(&rig.media, 30, false);
        let started = Instant::now();
        let outcome = render(
            &backend(rig.child.clone(), fake.port()),
            &timeline,
            &rig.out,
        )
        .expect("render");
        // The child sleeps for 120 s: the render must kill it, not wait for it.
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{:?}",
            started.elapsed()
        );
        assert_eq!((outcome.frames, outcome.duration_ms), (30, 1000));
        assert!(outcome.notes.is_empty(), "{:?}", outcome.notes);
        // The fake wrote 34 frames (30 and the four of margin); the file has 30.
        assert_eq!(count_frames(&rig.out).expect("frames"), 30);
        assert_eq!(rig.dir.leftovers("out"), vec!["result.mp4".to_string()]);
        assert_eq!(
            posts(&fake),
            [
                "/api/input/i0/register",
                "/api/input/clock/register",
                "/api/output/out/register",
                "/api/input/i0/unregister",
                "/api/input/clock/unregister",
                "/api/start",
            ]
        );
        // The first thing asked was whether the server is up, and the child is gone.
        assert_eq!(fake.requests()[0].path, "/status");
        assert_dies(&rig.pidfile);
    }

    #[test]
    fn the_server_is_started_with_the_offline_settings_and_no_other_listeners() {
        let rig = rig("env", 34);
        let envfile = rig.dir.0.join("env.txt");
        let child = rig.dir.script(
            "smelter-env",
            &format!(
                "env | sort > {0}.tmp\nmv {0}.tmp {0}\nexec sleep 120",
                envfile.display()
            ),
        );
        let fake = fake_writing(rig.prepared.clone(), envfile.clone(), |_| None);
        let timeline = project(&rig.media, 30, false);
        render(&backend(child, fake.port()), &timeline, &rig.out).expect("render");
        let env = fs::read_to_string(&envfile).expect("env");
        for want in [
            format!("SMELTER_API_PORT={}", fake.port()),
            "SMELTER_OUTPUT_FRAMERATE=30".to_string(),
            "SMELTER_OFFLINE_PROCESSING_ENABLE=true".to_string(),
            "SMELTER_START_WHIP_WHEP_SERVER=false".to_string(),
            "SMELTER_START_RTMP_SERVER=false".to_string(),
            "SMELTER_WEB_RENDERER_ENABLE=false".to_string(),
            "SMELTER_LOGGER_FORMAT=json".to_string(),
        ] {
            assert!(env.lines().any(|line| line == want), "{want} in\n{env}");
        }
    }

    #[test]
    fn an_input_smelter_cannot_decode_is_a_usage_error_with_the_fix() {
        let rig = rig("hevc", 34);
        let body = serde_json::json!({
            "error_code": "INPUT_STREAM_INPUT_ERROR",
            "message": "Input initialization error while registering input for stream \"i0\".",
            "stack": ["Input initialization error", "No suitable track in the mp4 file"]
        })
        .to_string();
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), move |request| {
            request
                .path
                .ends_with("/i0/register")
                .then(|| (500, body.clone()))
        });
        let timeline = project(&rig.media, 30, true);
        let error = render(
            &backend(rig.child.clone(), fake.port()),
            &timeline,
            &rig.out,
        )
        .expect_err("refused");
        assert_eq!(exit_code(&error), Some(2));
        let text = error.to_string();
        assert!(text.contains("No suitable track in the mp4 file"), "{text}");
        assert!(text.contains("H.264 video and AAC audio only"), "{text}");
        assert!(text.contains("ffmpeg -i"), "{text}");
        assert!(!posts(&fake).contains(&"/api/start".to_string()));
        assert!(!rig.out.exists());
        assert_dies(&rig.pidfile);
    }

    fn silent_input(request: &Recorded) -> Option<(u16, String)> {
        let is_register =
            request.path.starts_with("/api/input/") && request.path.ends_with("/register");
        is_register.then(|| {
            (
                200,
                r#"{"video_duration_ms":12000,"audio_duration_ms":null}"#.to_string(),
            )
        })
    }

    #[test]
    fn audio_smelter_dropped_is_refused_when_the_project_plays_it() {
        let rig = rig("pcm", 34);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), silent_input);
        let with_audio = project(&rig.media, 30, true);
        let error = render(
            &backend(rig.child.clone(), fake.port()),
            &with_audio,
            &rig.out,
        )
        .expect_err("refused");
        assert_eq!(exit_code(&error), Some(2));
        assert!(error.to_string().contains("heard no audio"), "{error}");
        assert!(error.to_string().contains("-c:a aac"), "{error}");
        assert!(!posts(&fake).contains(&"/api/start".to_string()));
    }

    #[test]
    fn a_project_without_audio_does_not_need_it() {
        let rig = rig("noaudio", 34);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), silent_input);
        let without = project(&rig.media, 30, false);
        render(&backend(rig.child.clone(), fake.port()), &without, &rig.out).expect("render");
    }

    #[test]
    fn a_rejected_request_is_an_error_in_smelters_words() {
        let rig = rig("reject", 34);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |request| {
            (request.path == "/api/output/out/register").then(|| {
                (
                    400,
                    r#"{"error_code":"MALFORMED_REQUEST","message":"unknown field `bf`"}"#
                        .to_string(),
                )
            })
        });
        let timeline = project(&rig.media, 30, false);
        let error = render(
            &backend(rig.child.clone(), fake.port()),
            &timeline,
            &rig.out,
        )
        .expect_err("rejected");
        assert_eq!(exit_code(&error), None, "exit 1, not 2");
        let text = error.to_string();
        assert!(text.contains("POST /api/output/out/register"), "{text}");
        assert!(
            text.contains("MALFORMED_REQUEST: unknown field `bf`"),
            "{text}"
        );
        assert_dies(&rig.pidfile);
    }

    #[test]
    fn an_error_in_smelters_log_fails_the_render_and_keeps_no_file() {
        let rig = rig("logerr", 34);
        let lines = rig.dir.0.join("log-lines.txt");
        fs::write(&lines, format!("{WARN_LINE}\n{ERROR_LINE}\n")).expect("log lines");
        let child = rig.dir.script(
            "smelter-err",
            &format!(
                "cat {}\necho $$ > {}\nexec sleep 120",
                lines.display(),
                rig.pidfile.display()
            ),
        );
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        let timeline = project(&rig.media, 30, false);
        let error = render(&backend(child, fake.port()), &timeline, &rig.out).expect_err("error");
        let text = error.to_string();
        assert!(text.contains("logged 1 error"), "{text}");
        assert!(
            text.contains("If audio or video was specified on register"),
            "{text}"
        );
        assert!(!rig.out.exists());
        assert_eq!(rig.dir.leftovers("out"), Vec::<String>::new());
        assert_dies(&rig.pidfile);
    }

    #[test]
    fn an_event_stream_that_closes_early_is_an_error() {
        let rig = rig("closed", 34);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        fake.events_after_start(vec![
            frame(0x1, true, br#"{"type":"AUDIO_INPUT_PLAYING"}"#),
            frame(0x8, true, b""),
        ]);
        let timeline = project(&rig.media, 30, false);
        let error = render(
            &backend(rig.child.clone(), fake.port()),
            &timeline,
            &rig.out,
        )
        .expect_err("closed");
        assert!(
            error.to_string().contains("closed its event stream"),
            "{error}"
        );
        assert_eq!(rig.dir.leftovers("out"), Vec::<String>::new());
    }

    #[test]
    fn an_output_that_never_finishes_times_out_and_the_server_is_killed() {
        let rig = rig("timeout", 34);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        fake.events_after_start(vec![frame(0x1, true, br#"{"type":"VIDEO_INPUT_PLAYING"}"#)]);
        let timeline = project(&rig.media, 30, false);
        let mut slow = backend(rig.child.clone(), fake.port());
        slow.render_timeout = Duration::from_millis(400);
        let error = render(&slow, &timeline, &rig.out).expect_err("timeout");
        assert!(
            error.to_string().contains("did not finish within"),
            "{error}"
        );
        assert_dies(&rig.pidfile);
        assert_eq!(rig.dir.leftovers("out"), Vec::<String>::new());
    }

    #[test]
    fn a_file_with_too_few_frames_is_refused() {
        let rig = rig("short", 29);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        let timeline = project(&rig.media, 30, false);
        let error = render(
            &backend(rig.child.clone(), fake.port()),
            &timeline,
            &rig.out,
        )
        .expect_err("short");
        let text = error.to_string();
        assert!(
            text.contains("wrote 29 video frames but the project has 30"),
            "{text}"
        );
        assert!(!rig.out.exists());
        assert_eq!(rig.dir.leftovers("out"), Vec::<String>::new());
    }

    #[test]
    fn a_server_that_exits_at_once_is_an_error_with_its_log() {
        let rig = rig("exits", 34);
        let child = rig.dir.script(
            "smelter-dies",
            "echo 'dyld: Library not loaded: libavcodec.62.dylib' >&2\nexit 3",
        );
        let closed = {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let timeline = project(&rig.media, 30, false);
        let error = render(&backend(child, closed), &timeline, &rig.out).expect_err("exit");
        let text = error.to_string();
        assert!(text.contains("exited before it answered"), "{text}");
        assert!(text.contains("Library not loaded"), "{text}");
        assert!(text.contains("brew install ffmpeg"), "{text}");
    }

    #[test]
    fn a_server_that_never_answers_times_out_and_is_killed() {
        let rig = rig("silent", 34);
        let closed = {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let timeline = project(&rig.media, 30, false);
        let mut silent = backend(rig.child.clone(), closed);
        silent.startup_timeout = Duration::from_millis(400);
        let error = render(&silent, &timeline, &rig.out).expect_err("timeout");
        assert!(
            error.to_string().contains("did not answer GET /status"),
            "{error}"
        );
        assert_none_left(&rig.marker);
    }

    #[test]
    fn a_missing_media_file_is_a_usage_error_before_anything_starts() {
        let rig = rig("nomedia", 34);
        let timeline = project(&rig.dir.0.join("gone.mp4"), 30, false);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        let error = render(
            &backend(rig.child.clone(), fake.port()),
            &timeline,
            &rig.out,
        )
        .expect_err("missing");
        assert_eq!(exit_code(&error), Some(2));
        assert!(fake.requests().is_empty());
        assert!(!rig.pidfile.exists(), "the server was not started");
    }

    #[test]
    fn an_out_that_is_a_directory_is_a_usage_error() {
        let rig = rig("outdir", 34);
        let timeline = project(&rig.media, 30, false);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        let dir = rig.dir.0.join("out");
        let error = render(&backend(rig.child.clone(), fake.port()), &timeline, &dir)
            .expect_err("directory");
        assert_eq!(exit_code(&error), Some(2));
        assert!(!rig.pidfile.exists());
    }

    #[test]
    fn an_odd_size_is_a_usage_error_from_the_plan() {
        let rig = rig("size", 34);
        let timeline = project(&rig.media, 30, false);
        let fake = fake_writing(rig.prepared.clone(), rig.pidfile.clone(), |_| None);
        let error = backend(rig.child.clone(), fake.port())
            .render(&RenderJob {
                project_path: Path::new("project.otio"),
                project: &timeline,
                out: &rig.out,
                size: Some(timeline::RenderSize {
                    width: 641,
                    height: 360,
                }),
            })
            .expect_err("odd");
        assert_eq!(exit_code(&error), Some(2));
        assert!(
            error.to_string().contains("even, non-zero sides"),
            "{error}"
        );
        assert!(!rig.pidfile.exists());
    }

    #[test]
    fn the_binary_is_the_flag_then_a_directory_then_the_path() {
        let dir = Scratch::new("resolve");
        let exe = dir.script("smelter", "exit 0");
        let direct = resolve_binary(Some(&exe), None).expect("a file");
        assert_eq!(direct, exe);
        let in_dir = resolve_binary(Some(&dir.0), None).expect("a directory");
        assert_eq!(in_dir, exe);
        let on_path = resolve_binary(None, Some(dir.0.clone().into_os_string())).expect("on PATH");
        assert_eq!(on_path, exe);
        // The flag wins over PATH.
        let other = dir.script("other", "exit 0");
        assert_eq!(
            resolve_binary(Some(&other), Some(dir.0.clone().into_os_string())).expect("flag"),
            other
        );
    }

    #[test]
    fn a_missing_binary_says_how_to_point_at_one_and_that_ffmpeg_still_works() {
        let dir = Scratch::new("nobin");
        for error in [
            resolve_binary(Some(&dir.0.join("nope")), None).expect_err("missing"),
            resolve_binary(None, Some(dir.0.clone().into_os_string())).expect_err("not on PATH"),
            resolve_binary(None, None).expect_err("no PATH"),
        ] {
            let text = error.to_string();
            assert!(
                text.contains("SMELTER_BIN") && text.contains("--smelter"),
                "{text}"
            );
            assert!(text.contains("--backend ffmpeg"), "{text}");
            assert!(
                text.contains("github.com/software-mansion/smelter"),
                "{text}"
            );
            assert_eq!(exit_code(&error), None, "a missing tool is exit 1");
        }
    }

    #[test]
    fn only_error_lines_of_the_json_log_count() {
        let log = format!(
            "{WARN_LINE}\nplain text from ffmpeg, level ERROR\n{ERROR_LINE}\n\n{{\"level\":\"error\",\"fields\":{{\"message\":\"second\"}}}}\n"
        );
        let errors = smelter_errors(&log);
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors[0].starts_with("Error while running scheduled output update"));
        assert_eq!(errors[1], "second");
        assert!(smelter_errors(WARN_LINE).is_empty());
        assert!(smelter_errors("").is_empty());
    }

    #[test]
    fn the_log_tail_shows_messages_of_json_lines_and_the_last_lines_only() {
        let log = format!("one\n{WARN_LINE}\nthree\nfour\n");
        assert_eq!(
            log_tail(&log, 3),
            "Failed to send decoded video frame from decoder. Channel closed.\nthree\nfour"
        );
        assert_eq!(log_tail(&log, 1), "four");
        assert_eq!(log_tail("", 5), "");
    }

    #[test]
    fn a_source_without_colour_tags_is_noticed() {
        for text in ["", "unknown", "Unspecified\n", "N/A"] {
            assert!(is_untagged(text), "{text:?}");
        }
        for text in ["bt709", "smpte170m", "bt2020nc"] {
            assert!(!is_untagged(text), "{text:?}");
        }
    }
}
