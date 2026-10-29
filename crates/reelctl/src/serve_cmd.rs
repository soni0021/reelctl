//! `reelctl serve`: the studio service on a Unix socket (spec
//! `docs/specs/studio-service.md`, ADR-028). It holds the workspace lock, checks
//! each connection's peer uid, reads JSON-RPC requests one per line, and runs jobs
//! as child `reelctl` processes with `--progress json`. The request handlers are
//! `studio::service`; this file is the socket, `doctor.check`, `events.subscribe`,
//! `shutdown`, and the runner that spawns the children.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::Args;
use nix::fcntl::{Flock, FlockArg};
use serde_json::{json, Value};
use studio::commands;
use studio::jobs::Job;
use studio::protocol::{self, RpcError};
use studio::queue::{Limits, Notification, RunContext, RunOutcome, Runner};
use studio::service::{Config, Secrets, Service, KEY_NAME};

use crate::make_reels_cmd::PASS_THROUGH;
use crate::understand_cmd::ExitWith;

/// How long a cancelled child has between SIGTERM and SIGKILL.
const KILL_AFTER: Duration = Duration::from_secs(5);
/// How often the accept loop and the runner look at their stop flags.
const TICK: Duration = Duration::from_millis(50);
/// The longest socket path the platforms accept (`sun_path` is 104 bytes on macOS).
const MAX_SOCKET_PATH: usize = 100;
/// How much of a failed child's plain stderr is kept as its error.
const STDERR_LINES: usize = 12;

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// The workspace folder (made if missing). Default: ~/Movies/ReelStudio on
    /// macOS, ~/ReelStudio elsewhere.
    #[arg(long)]
    workspace: Option<PathBuf>,
    /// Internal: a recorded model reply for every `understand` child (tests only).
    #[arg(long, hide = true)]
    understand_replay: Option<PathBuf>,
    /// Internal: a model listing for every `understand` child (tests only).
    #[arg(long, hide = true)]
    understand_catalog: Option<PathBuf>,
}

fn exit(code: i32, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ExitWith {
        code,
        message: message.into(),
    })
}

fn default_workspace() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| exit(2, "HOME is not set; pass --workspace"))?;
    Ok(if cfg!(target_os = "macos") {
        home.join("Movies").join("ReelStudio")
    } else {
        home.join("ReelStudio")
    })
}

/// The uid of the process at the other end of `stream`.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn peer_uid(stream: &UnixStream) -> nix::Result<u32> {
    nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
        .map(|credentials| credentials.uid())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn peer_uid(stream: &UnixStream) -> nix::Result<u32> {
    nix::unistd::getpeereid(stream).map(|(uid, _)| uid.as_raw())
}

/// Hold `serve.lock` for the life of the process: one `serve` per workspace.
fn lock_workspace(studio_dir: &Path) -> Result<Flock<fs::File>> {
    let path = studio_dir.join("serve.lock");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(mut lock) => {
            lock.set_len(0)
                .and_then(|()| writeln!(lock, "{}", std::process::id()))
                .with_context(|| format!("write {}", path.display()))?;
            Ok(lock)
        }
        Err((_, nix::errno::Errno::EWOULDBLOCK)) => {
            let pid = fs::read_to_string(&path).unwrap_or_default();
            Err(exit(
                2,
                format!(
                    "another reelctl serve (pid {}) is using this workspace",
                    pid.trim()
                ),
            ))
        }
        Err((_, errno)) => Err(anyhow!("lock {}: {errno}", path.display())),
    }
}

/// Names of the variables a job may get that are set here (never their values).
fn available_env() -> Vec<String> {
    let mut names: Vec<String> = commands::COMMANDS
        .iter()
        .flat_map(|spec| spec.env.iter())
        .filter(|name| **name != KEY_NAME && std::env::var_os(name).is_some())
        .map(|name| name.to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

pub fn serve(args: ServeArgs) -> Result<()> {
    let workspace = match args.workspace {
        Some(path) => path,
        None => default_workspace()?,
    };
    fs::create_dir_all(&workspace)
        .with_context(|| format!("create the workspace {}", workspace.display()))?;
    let workspace = fs::canonicalize(&workspace)
        .with_context(|| format!("find the workspace {}", workspace.display()))?;
    let studio_dir = studio::store::Store::open(&workspace)
        .map_err(|error| exit(2, error.to_string()))?
        .root()
        .to_path_buf();
    let _lock = lock_workspace(&studio_dir)?;

    let socket = studio_dir.join("reelctl.sock");
    if socket.as_os_str().len() > MAX_SOCKET_PATH {
        return Err(exit(
            2,
            format!(
                "the socket path {} is over {MAX_SOCKET_PATH} bytes; use a shorter workspace path",
                socket.display()
            ),
        ));
    }
    let secrets = Arc::new(Secrets::default());
    let runner = Arc::new(ProcessRunner {
        exe: std::env::current_exe().context("find the reelctl executable")?,
        secrets: Arc::clone(&secrets),
        understand_replay: args.understand_replay,
        understand_catalog: args.understand_catalog,
    });
    let service = Arc::new(
        Service::open(
            Config {
                workspace: workspace.clone(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                limits: Limits::default(),
                env_available: available_env(),
            },
            runner,
            secrets,
        )
        .map_err(|error| exit(2, error.to_string()))?,
    );

    // The lock is held, so a socket file here is a stale one.
    let _ = fs::remove_file(&socket);
    let listener =
        UnixListener::bind(&socket).with_context(|| format!("listen on {}", socket.display()))?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set the mode of {}", socket.display()))?;
    listener
        .set_nonblocking(true)
        .context("make the socket non-blocking")?;
    out!(
        "serve: workspace={} socket={} pid={}",
        workspace.display(),
        socket.display(),
        std::process::id()
    );
    let _ = std::io::stdout().flush();

    let stopping = Arc::new(AtomicBool::new(false));
    let uid = nix::unistd::getuid().as_raw();
    while !stopping.load(Ordering::SeqCst) && !progress::cancel_requested() {
        match listener.accept() {
            Ok((stream, _)) => {
                if !matches!(peer_uid(&stream), Ok(peer) if peer == uid) {
                    // Another user (or an unknown one): closed without a word.
                    continue;
                }
                let service = Arc::clone(&service);
                let stopping = Arc::clone(&stopping);
                std::thread::spawn(move || {
                    let _ = connection(stream, &service, &stopping);
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(TICK);
            }
            Err(error) => return Err(anyhow!("accept on {}: {error}", socket.display())),
        }
    }
    // Running jobs are cancelled cleanly (SIGTERM, then SIGKILL after 5 s).
    let _ = service.queue().shutdown();
    let _ = fs::remove_file(&socket);
    Ok(())
}

type Writer = Arc<Mutex<UnixStream>>;

fn send(writer: &Writer, line: &str) -> std::io::Result<()> {
    let mut stream = writer
        .lock()
        .map_err(|_| std::io::Error::other("the writer lock is poisoned"))?;
    stream.write_all(line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()
}

/// Serve one connection until it closes.
fn connection(stream: UnixStream, service: &Arc<Service>, stopping: &AtomicBool) -> Result<()> {
    stream.set_nonblocking(false)?;
    let writer: Writer = Arc::new(Mutex::new(stream.try_clone()?));
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match protocol::parse_request(&line) {
            Ok(request) => request,
            Err(error) => {
                send(&writer, &protocol::response(&Value::Null, Err(error)))?;
                continue;
            }
        };
        // The subscription's notifications start only after its reply is sent.
        let mut start_events: Option<Box<dyn FnOnce() + Send>> = None;
        let result = match request.method.as_str() {
            "doctor.check" => Ok(doctor()),
            "events.subscribe" => {
                subscribe(service, &request.params, &writer).map(|(reply, start)| {
                    start_events = Some(start);
                    reply
                })
            }
            "shutdown" => {
                stopping.store(true, Ordering::SeqCst);
                Ok(json!({}))
            }
            method => service.handle(method, &request.params),
        };
        if let Some(id) = &request.id {
            send(&writer, &protocol::response(id, result))?;
        }
        if let Some(start) = start_events.take() {
            start();
        }
    }
    Ok(())
}

fn notification_line(notification: &Notification) -> String {
    match notification {
        Notification::JobStatus { job, status } => {
            protocol::notification("job.status", json!({ "job": job, "status": status }))
        }
        Notification::JobEvent { job, seq, event } => {
            let method = if event["event"] == "progress" {
                "job.progress"
            } else {
                "job.event"
            };
            protocol::notification(method, json!({ "job": job, "seq": seq, "event": event }))
        }
        Notification::CheckpointCreated {
            subject,
            checkpoint,
            job,
            cached,
        } => protocol::notification(
            "checkpoint.created",
            json!({ "subject": subject, "checkpoint": checkpoint, "job": job, "cached": cached }),
        ),
    }
}

fn notification_job(notification: &Notification) -> i64 {
    match notification {
        Notification::JobStatus { job, .. }
        | Notification::JobEvent { job, .. }
        | Notification::CheckpointCreated { job, .. } => *job,
    }
}

/// `events.subscribe {job?, since_seq?}`: with `job`, that job's stored events
/// after `since_seq` are sent first, then its live ones; without, every live
/// notification. A replayed event can come again live; `seq` tells them apart.
fn subscribe(
    service: &Arc<Service>,
    params: &Value,
    writer: &Writer,
) -> std::result::Result<(Value, Box<dyn FnOnce() + Send>), RpcError> {
    let job = match params.get("job") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            value
                .as_i64()
                .ok_or_else(|| RpcError::params("\"job\" must be a whole number"))?,
        ),
    };
    let since = params.get("since_seq").and_then(Value::as_i64).unwrap_or(0);
    // Subscribe before the replay, so nothing falls between the two.
    let receiver = service.queue().subscribe()?;
    let mut replay = Vec::new();
    if let Some(job) = job {
        let catalogue =
            studio::catalogue::Catalogue::open(&service.store().root().join("studio.db"))?;
        catalogue.require_job(job)?;
        for event in catalogue.events(job, since)? {
            replay.push(notification_line(&Notification::JobEvent {
                job,
                seq: event.seq,
                event: event.event,
            }));
        }
    }
    let writer = Arc::clone(writer);
    let start = move || {
        std::thread::spawn(move || {
            for line in replay {
                if send(&writer, &line).is_err() {
                    return;
                }
            }
            for notification in receiver {
                if job.is_some_and(|job| notification_job(&notification) != job) {
                    continue;
                }
                if send(&writer, &notification_line(&notification)).is_err() {
                    return;
                }
            }
        });
    };
    Ok((json!({ "subscribed": true, "job": job }), Box::new(start)))
}

/// The first line `tool -version` prints, if it runs.
fn tool_version(tool: &Path) -> Option<String> {
    let mut command = Command::new(tool);
    command.arg("-version").stdin(Stdio::null());
    let output = progress::tracked_output(&mut command).ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    })
}

/// Where `name` is on PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}

/// `doctor.check`: each tool and file the commands use, as found by this service.
fn doctor() -> Value {
    let tool = |name: &str| match on_path(name) {
        Some(path) => json!({ "found": true, "path": path, "version": tool_version(&path) }),
        None => json!({ "found": false, "path": null, "version": null }),
    };
    let named = |variable: &str, fallback: Option<&str>| {
        let path = std::env::var_os(variable)
            .map(PathBuf::from)
            .or_else(|| fallback.and_then(on_path));
        match path {
            Some(path) => json!({ "found": path.is_file(), "path": path, "variable": variable }),
            None => json!({ "found": false, "path": null, "variable": variable }),
        }
    };
    json!({
        "ffmpeg": tool("ffmpeg"),
        "ffprobe": tool("ffprobe"),
        "ffmpeg_libraries": { "found": true, "version": decompile::ffmpeg_library_versions() },
        "manimgx": named("MANIMGX_BIN", Some("manimgx")),
        "smelter": named("SMELTER_BIN", None),
        "onnx_runtime": named("ORT_DYLIB_PATH", None),
        "whisper_model": named("REELCTL_WHISPER_MODEL", None),
        "transnet_weights": named("REELCTL_TRANSNET_WEIGHTS", None),
        "rvm_weights": named("REELCTL_RVM_WEIGHTS", None),
    })
}

/// Runs each job as a child `reelctl <command> --progress json` in its scratch
/// folder, with a from-scratch environment (ADR-027).
struct ProcessRunner {
    exe: PathBuf,
    secrets: Arc<Secrets>,
    understand_replay: Option<PathBuf>,
    understand_catalog: Option<PathBuf>,
}

fn failed(message: impl Into<String>) -> RunOutcome {
    RunOutcome {
        exit_code: None,
        result: None,
        outputs: BTreeMap::new(),
        error: Some(message.into()),
    }
}

/// Signal a job's whole process group (the child `reelctl` and the ffmpeg,
/// whisper or manimgx processes it started), through `nix`'s safe `killpg`.
fn signal_group(pid: u32, signal: nix::sys::signal::Signal) {
    if let Ok(pid) = i32::try_from(pid) {
        let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), signal);
    }
}

impl Runner for ProcessRunner {
    fn run(&self, job: &Job, context: RunContext<'_>) -> RunOutcome {
        // Taken first, so the key is forgotten here even if the job fails to start.
        let key = (job.command == "understand")
            .then(|| self.secrets.take(job.id))
            .flatten();
        let spec = match commands::command(&job.command) {
            Ok(spec) => spec,
            Err(error) => return failed(error.to_string()),
        };
        let planned = commands::outputs_for(spec, &job.args, context.scratch);
        let built = match commands::build(&job.command, &job.args, context.inputs, &planned) {
            Ok(built) => built,
            Err(error) => return failed(error.to_string()),
        };
        let dry_run = job.command == "understand" && job.args["dry_run"] == json!(true);

        let mut command = Command::new(&self.exe);
        command
            .arg("--progress")
            .arg("json")
            .args(&built.argv)
            .current_dir(context.scratch)
            // Its own process group, so a cancel reaches its children too.
            .process_group(0)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if job.command == "understand" {
            if let Some(path) = &self.understand_catalog {
                command.arg("--catalog").arg(path);
            }
            if let (Some(path), false) = (&self.understand_replay, dry_run) {
                command.arg("--replay").arg(path);
            }
        }
        for name in PASS_THROUGH {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        for name in &built.env {
            if *name == KEY_NAME {
                if let Some(key) = &key {
                    command.env(name, key);
                }
            } else if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => return failed(format!("cannot run {}: {error}", self.exe.display())),
        };
        drop(key);
        // Registered, so serve's own forced exit stops it as well.
        let _tracked = progress::track(child.id());

        let stdout = child.stdout.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let _ = pipe.read_to_end(&mut bytes);
                bytes
            })
        });
        let (lines_tx, lines) = channel::<String>();
        if let Some(pipe) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if lines_tx.send(line).is_err() {
                        return;
                    }
                }
            });
        }

        let mut result: Option<Value> = None;
        let mut error: Option<String> = None;
        let mut plain: Vec<String> = Vec::new();
        let mut take = |line: String| match serde_json::from_str::<Value>(&line) {
            Ok(event) if event.get("v").is_some() && event.get("event").is_some() => {
                match event["event"].as_str() {
                    Some("result") if result.is_none() => result = Some(event["fields"].clone()),
                    Some("error") => error = event["message"].as_str().map(str::to_string),
                    _ => {}
                }
                (context.emit)(event);
            }
            _ => {
                (context.emit)(json!({ "v": 1, "event": "log", "line": line }));
                plain.push(line);
                if plain.len() > STDERR_LINES {
                    plain.remove(0);
                }
            }
        };
        let mut terminated_at: Option<Instant> = None;
        let status = loop {
            match lines.recv_timeout(TICK) {
                Ok(line) => take(line),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => std::thread::sleep(TICK),
            }
            if context.cancel.load(Ordering::SeqCst) {
                match terminated_at {
                    None => {
                        signal_group(child.id(), nix::sys::signal::Signal::SIGTERM);
                        terminated_at = Some(Instant::now());
                    }
                    Some(at) if at.elapsed() >= KILL_AFTER => {
                        signal_group(child.id(), nix::sys::signal::Signal::SIGKILL);
                        let _ = child.kill();
                    }
                    Some(_) => {}
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
        };
        // What the child wrote before it ended.
        while let Ok(line) = lines.recv_timeout(Duration::from_millis(200)) {
            take(line);
        }
        let stdout = stdout
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        let exit_code = status.and_then(|status| status.code());
        // Stopped because serve itself is stopping (its signal reached the child
        // first): a cancel, not a failure.
        if exit_code == Some(progress::CANCELLED_EXIT) && progress::cancel_requested() {
            context.cancel.store(true, Ordering::SeqCst);
        }
        if job.command == "understand"
            && !dry_run
            && exit_code != Some(0)
            && context.cancel.load(Ordering::SeqCst)
        {
            (context.emit)(json!({
                "v": 1,
                "event": "warning",
                "message": format!(
                    "the model call was stopped; it may already have been billed (up to ${})",
                    job.args["max_cost_usd"]
                ),
            }));
        }

        let mut outputs = if dry_run {
            BTreeMap::new()
        } else {
            built.outputs.clone()
        };
        if exit_code == Some(0) {
            if let Some(role) = &built.stdout {
                if let Some(path) = outputs.get(role) {
                    if let Err(write) = fs::write(path, &stdout) {
                        return failed(format!("write {}: {write}", path.display()));
                    }
                }
            }
        } else {
            outputs.clear();
        }
        let error = match exit_code {
            Some(0) => None,
            _ => Some(error.unwrap_or_else(|| {
                if plain.is_empty() {
                    format!("{} ended without a word ({exit_code:?})", job.command)
                } else {
                    plain.join("\n")
                }
            })),
        };
        RunOutcome {
            exit_code,
            result,
            outputs,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_name_their_method_and_job() {
        let progress = Notification::JobEvent {
            job: 4,
            seq: 2,
            event: json!({"v": 1, "event": "progress", "stage": "render", "done": 3}),
        };
        let line: Value = serde_json::from_str(&notification_line(&progress)).unwrap();
        assert_eq!(line["method"], "job.progress");
        assert_eq!(line["params"]["seq"], 2);
        assert_eq!(notification_job(&progress), 4);
        let created = Notification::CheckpointCreated {
            subject: 1,
            checkpoint: 9,
            job: 4,
            cached: true,
        };
        let line: Value = serde_json::from_str(&notification_line(&created)).unwrap();
        assert_eq!(line["method"], "checkpoint.created");
        assert_eq!(line["params"]["cached"], true);
    }

    #[test]
    fn the_default_workspace_is_under_home() {
        let path = default_workspace().unwrap();
        assert!(path.ends_with("ReelStudio"));
    }

    #[test]
    fn the_only_variables_recorded_are_names_set_here_and_never_the_key() {
        assert!(!available_env().iter().any(|name| name == KEY_NAME));
    }
}
