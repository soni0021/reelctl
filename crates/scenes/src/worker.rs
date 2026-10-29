//! The manimgx worker: a separately installed program, started directly with an
//! argument array (never a shell), in a scratch directory, with a time limit. Every
//! failure is an error; nothing it writes is used before the caller has checked it.
//!
//! Only flags the tool documents are used (2026-10-29): `--version`; `render FILE
//! SCENE -o OUT.mp4 -r WxH --fps N`; `inspect FILE SCENE -t end -o OUT.png`.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Result, SceneError};

/// The environment variable that names the worker.
pub const WORKER_ENV: &str = "MANIMGX_BIN";
/// The documented install command, shown when the worker is missing.
pub const INSTALL_HINT: &str = "uv tool install --python 3.14 manimgx";
/// The default time limit of one worker run.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// How much of the end of a log is shown in an error.
const TAIL_BYTES: usize = 4000;
/// The longest a version line may be.
const VERSION_CHARS: usize = 100;

/// One scene to inspect or render. Every path is absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneJob {
    pub scene_file: PathBuf,
    pub scene_name: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_out: PathBuf,
    pub storyboard_out: PathBuf,
    /// The scratch directory the worker runs in.
    pub work_dir: PathBuf,
}

/// What `inspect` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inspection {
    Clean,
    /// The tool's exit code 1: 2D layout problems. The text is what it printed.
    Problems(String),
}

/// What the command calls. `ManimgxCli` is the real one; a test may use a fake.
pub trait SceneRenderer {
    /// The worker's version line. It must answer `--version` with exit 0.
    fn version(&self) -> Result<String>;
    /// Check the scene's 2D layout at its end, without making a video.
    fn inspect(&self, job: &SceneJob) -> Result<Inspection>;
    /// Make the video at `job.video_out`.
    fn render(&self, job: &SceneJob) -> Result<()>;
}

fn absolute(path: &Path, what: &str) -> Result<OsString> {
    if !path.is_absolute() {
        return Err(SceneError::usage(format!(
            "{what} {} is not an absolute path",
            path.display()
        )));
    }
    Ok(path.as_os_str().to_os_string())
}

/// The arguments of `manimgx render`.
pub fn render_args(job: &SceneJob) -> Result<Vec<OsString>> {
    Ok(vec![
        "render".into(),
        absolute(&job.scene_file, "the scene file")?,
        job.scene_name.clone().into(),
        "-o".into(),
        absolute(&job.video_out, "the video path")?,
        "-r".into(),
        format!("{}x{}", job.width, job.height).into(),
        "--fps".into(),
        job.fps.to_string().into(),
    ])
}

/// The arguments of `manimgx inspect`.
pub fn inspect_args(job: &SceneJob) -> Result<Vec<OsString>> {
    Ok(vec![
        "inspect".into(),
        absolute(&job.scene_file, "the scene file")?,
        job.scene_name.clone().into(),
        "-t".into(),
        "end".into(),
        "-o".into(),
        absolute(&job.storyboard_out, "the storyboard path")?,
    ])
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn not_found(name: &str) -> SceneError {
    SceneError::worker(format!(
        "the manimgx worker \"{name}\" was not found; install it with `{INSTALL_HINT}`, or give its path with --manimgx or {WORKER_ENV}"
    ))
}

/// The worker program: `flag` (`--manimgx`), else `env` (`MANIMGX_BIN`, when not
/// empty), else `manimgx` searched on `path_var`. A name with a `/` must be an
/// executable regular file; a bare name is looked up on the path. The result is an
/// absolute path, so it cannot be read as an option. A `://` in the name is refused.
pub fn resolve_worker(
    flag: Option<&str>,
    env: Option<&str>,
    path_var: Option<&OsStr>,
) -> Result<PathBuf> {
    let name = match (flag, env) {
        (Some(flag), _) => flag,
        (None, Some(env)) if !env.is_empty() => env,
        _ => "manimgx",
    };
    if name.trim().is_empty() {
        return Err(SceneError::usage("--manimgx is empty"));
    }
    if name.contains("://") {
        return Err(SceneError::usage(format!(
            "\"{name}\" is a URL; the manimgx worker must be a local program"
        )));
    }
    if name.contains('/') {
        let path = fs::canonicalize(name).map_err(|_| not_found(name))?;
        if !is_executable(&path) {
            return Err(SceneError::worker(format!(
                "\"{name}\" is not an executable file"
            )));
        }
        return Ok(path);
    }
    let dirs = path_var.map(std::env::split_paths).into_iter().flatten();
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return fs::canonicalize(&candidate)
                .map_err(|error| SceneError::worker(format!("{}: {error}", candidate.display())));
        }
    }
    Err(not_found(name))
}

struct Outcome {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn tail(path: &Path) -> String {
    let mut bytes = Vec::new();
    if let Ok(file) = fs::File::open(path) {
        let _ = file.take(8 * 1024 * 1024).read_to_end(&mut bytes);
    }
    let from = bytes.len().saturating_sub(TAIL_BYTES);
    let mut text = String::from_utf8_lossy(&bytes[from..]).trim().to_string();
    if from > 0 {
        text.insert_str(0, "...");
    }
    text
}

/// Run `program` with `args` in `work_dir`, its output in files there, and kill it
/// after `timeout`.
fn run(
    program: &Path,
    args: &[OsString],
    work_dir: &Path,
    label: &str,
    timeout: Duration,
) -> Result<Outcome> {
    let out_path = work_dir.join(format!("{label}.out.log"));
    let err_path = work_dir.join(format!("{label}.err.log"));
    let open = |path: &Path| {
        fs::File::create(path).map_err(|error| {
            SceneError::worker(format!("cannot write {}: {error}", path.display()))
        })
    };
    let mut child = Command::new(program)
        .args(args)
        .current_dir(work_dir)
        .env_remove("OPENROUTER_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::from(open(&out_path)?))
        .stderr(Stdio::from(open(&err_path)?))
        .spawn()
        .map_err(|error| {
            SceneError::worker(format!("cannot start {}: {error}", program.display()))
        })?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SceneError::worker(format!(
                    "the manimgx worker ({label}) did not finish in {} s and was stopped",
                    timeout.as_secs_f64()
                )));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                return Err(SceneError::worker(format!(
                    "cannot wait for the manimgx worker: {error}"
                )))
            }
        }
    };
    Ok(Outcome {
        code: status.code(),
        stdout: tail(&out_path),
        stderr: tail(&err_path),
    })
}

fn failure(label: &str, outcome: &Outcome) -> SceneError {
    let code = match outcome.code {
        Some(code) => format!("exit {code}"),
        None => "no exit code (stopped by a signal)".to_string(),
    };
    let mut message = format!("manimgx {label} failed ({code})");
    if !outcome.stderr.is_empty() {
        message.push_str(&format!(":\n{}", outcome.stderr));
    } else if !outcome.stdout.is_empty() {
        message.push_str(&format!(":\n{}", outcome.stdout));
    }
    SceneError::worker(message)
}

/// The real worker.
#[derive(Debug, Clone)]
pub struct ManimgxCli {
    program: PathBuf,
    timeout: Duration,
    work_dir: PathBuf,
}

impl ManimgxCli {
    /// `program` is an absolute path from [`resolve_worker`]; `work_dir` is where it
    /// runs and writes its logs.
    pub fn new(program: PathBuf, work_dir: PathBuf, timeout: Duration) -> Self {
        Self {
            program,
            timeout,
            work_dir,
        }
    }

    pub fn program(&self) -> &Path {
        &self.program
    }
}

/// A version line made safe to store: no control characters, at most 100 characters.
fn clean_version(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    let cleaned: String = line
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_control())
        .take(VERSION_CHARS)
        .collect();
    if cleaned.is_empty() {
        "manimgx (no version text)".to_string()
    } else {
        cleaned
    }
}

impl SceneRenderer for ManimgxCli {
    fn version(&self) -> Result<String> {
        let outcome = run(
            &self.program,
            &["--version".into()],
            &self.work_dir,
            "version",
            self.timeout,
        )?;
        if outcome.code != Some(0) {
            return Err(failure("--version", &outcome));
        }
        let text = if outcome.stdout.is_empty() {
            &outcome.stderr
        } else {
            &outcome.stdout
        };
        Ok(clean_version(text))
    }

    fn inspect(&self, job: &SceneJob) -> Result<Inspection> {
        let outcome = run(
            &self.program,
            &inspect_args(job)?,
            &job.work_dir,
            "inspect",
            self.timeout,
        )?;
        match outcome.code {
            Some(0) => Ok(Inspection::Clean),
            Some(1) => {
                let mut report = outcome.stdout.clone();
                if !outcome.stderr.is_empty() {
                    if !report.is_empty() {
                        report.push('\n');
                    }
                    report.push_str(&outcome.stderr);
                }
                Ok(Inspection::Problems(report))
            }
            _ => Err(failure("inspect", &outcome)),
        }
    }

    fn render(&self, job: &SceneJob) -> Result<()> {
        let outcome = run(
            &self.program,
            &render_args(job)?,
            &job.work_dir,
            "render",
            self.timeout,
        )?;
        if outcome.code != Some(0) {
            return Err(failure("render", &outcome));
        }
        let wrote = fs::metadata(&job.video_out).is_ok_and(|meta| meta.is_file() && meta.len() > 0);
        if !wrote {
            return Err(SceneError::worker(format!(
                "manimgx render exited 0 but wrote no video at {}",
                job.video_out.display()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A new empty directory.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "scenes-worker-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        fs::canonicalize(&dir).expect("canonical")
    }

    /// An executable shell script with `body`.
    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn job(dir: &Path) -> SceneJob {
        SceneJob {
            scene_file: dir.join("scene.py"),
            scene_name: "ReelctlChart".to_string(),
            width: 576,
            height: 432,
            fps: 30,
            video_out: dir.join("scene.mp4"),
            storyboard_out: dir.join("inspect.png"),
            work_dir: dir.to_path_buf(),
        }
    }

    fn cli(dir: &Path, program: PathBuf, secs: u64) -> ManimgxCli {
        ManimgxCli::new(program, dir.to_path_buf(), Duration::from_secs(secs))
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn render_and_inspect_use_only_the_documented_flags() {
        let dir = PathBuf::from("/s");
        let job = job(&dir);
        assert_eq!(
            strings(&render_args(&job).expect("args")),
            [
                "render",
                "/s/scene.py",
                "ReelctlChart",
                "-o",
                "/s/scene.mp4",
                "-r",
                "576x432",
                "--fps",
                "30"
            ]
        );
        assert_eq!(
            strings(&inspect_args(&job).expect("args")),
            [
                "inspect",
                "/s/scene.py",
                "ReelctlChart",
                "-t",
                "end",
                "-o",
                "/s/inspect.png"
            ]
        );
    }

    #[test]
    fn a_relative_path_is_refused_so_it_cannot_be_read_as_an_option() {
        let mut bad = job(Path::new("/s"));
        bad.scene_file = PathBuf::from("-x.py");
        assert!(render_args(&bad).is_err());
        assert!(inspect_args(&bad).is_err());
        let mut bad = job(Path::new("/s"));
        bad.video_out = PathBuf::from("out.mp4");
        assert!(render_args(&bad).is_err());
        let mut bad = job(Path::new("/s"));
        bad.storyboard_out = PathBuf::from("out.png");
        assert!(inspect_args(&bad).is_err());
    }

    #[test]
    fn the_flag_beats_the_environment_beats_the_path() {
        let dir = scratch("resolve");
        let by_flag = script(&dir, "flagged", "exit 0");
        let by_env = script(&dir, "envied", "exit 0");
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        let on_path = script(&bin, "manimgx", "exit 0");
        let flag = by_flag.to_str().expect("utf8");
        let env = by_env.to_str().expect("utf8");
        let path = std::env::join_paths([&bin]).expect("path");
        assert_eq!(
            resolve_worker(Some(flag), Some(env), Some(&path)).expect("flag"),
            by_flag
        );
        assert_eq!(
            resolve_worker(None, Some(env), Some(&path)).expect("env"),
            by_env
        );
        assert_eq!(
            resolve_worker(None, None, Some(&path)).expect("path"),
            on_path
        );
        // An empty environment variable is no variable.
        assert_eq!(
            resolve_worker(None, Some(""), Some(&path)).expect("empty env"),
            on_path
        );
    }

    #[test]
    fn a_missing_worker_names_the_documented_install_command() {
        let dir = scratch("missing");
        let path = std::env::join_paths([&dir]).expect("path");
        let error = resolve_worker(None, None, Some(&path)).expect_err("none");
        assert!(matches!(error, SceneError::Worker(_)));
        assert!(error.to_string().contains(INSTALL_HINT), "{error}");
        assert!(resolve_worker(None, None, None).is_err());
        let gone = dir.join("gone");
        let error =
            resolve_worker(Some(gone.to_str().expect("utf8")), None, None).expect_err("gone");
        assert!(matches!(error, SceneError::Worker(_)), "{error}");
    }

    #[test]
    fn a_url_an_empty_name_a_directory_and_a_plain_file_are_refused() {
        let dir = scratch("refused");
        for url in ["https://x.example/manimgx", "file:///bin/sh", "x://y"] {
            let error = resolve_worker(Some(url), None, None).expect_err(url);
            assert!(matches!(error, SceneError::Usage(_)), "{url}: {error}");
            let error = resolve_worker(None, Some(url), None).expect_err(url);
            assert!(matches!(error, SceneError::Usage(_)), "{url}: {error}");
        }
        assert!(matches!(
            resolve_worker(Some(""), None, None),
            Err(SceneError::Usage(_))
        ));
        assert!(matches!(
            resolve_worker(Some("  "), None, None),
            Err(SceneError::Usage(_))
        ));
        assert!(resolve_worker(Some(dir.to_str().expect("utf8")), None, None).is_err());
        let plain = dir.join("plain");
        fs::write(&plain, "x").expect("write");
        let error =
            resolve_worker(Some(plain.to_str().expect("utf8")), None, None).expect_err("mode");
        assert!(error.to_string().contains("not an executable"), "{error}");
    }

    #[test]
    fn the_version_line_comes_from_a_worker_that_answers_with_exit_0() {
        let dir = scratch("version");
        let good = script(&dir, "good", "echo 'manimgx 1.2.3'; echo ignored");
        assert_eq!(
            cli(&dir, good, 30).version().expect("version"),
            "manimgx 1.2.3"
        );
        let errput = script(&dir, "errput", "echo 'manimgx 4.5.6' >&2");
        assert_eq!(
            cli(&dir, errput, 30).version().expect("version"),
            "manimgx 4.5.6"
        );
        let silent = script(&dir, "silent", "exit 0");
        assert_eq!(
            cli(&dir, silent, 30).version().expect("version"),
            "manimgx (no version text)"
        );
        let broken = script(&dir, "broken", "echo 'Traceback: no module' >&2; exit 3");
        let error = cli(&dir, broken, 30).version().expect_err("broken");
        assert!(matches!(error, SceneError::Worker(_)));
        assert!(error.to_string().contains("exit 3") && error.to_string().contains("Traceback"));
    }

    #[test]
    fn a_version_line_is_cleaned_and_cut() {
        assert_eq!(clean_version("\n  manimgx 1.0\u{7} \n"), "manimgx 1.0");
        assert_eq!(clean_version(&"v".repeat(500)).chars().count(), 100);
        assert_eq!(clean_version("\u{1b}[31m\n"), "[31m");
        assert_eq!(clean_version(""), "manimgx (no version text)");
        assert_eq!(clean_version(&"v".repeat(100)), "v".repeat(100));
    }

    #[test]
    fn inspect_is_clean_on_exit_0_problems_on_exit_1_and_a_failure_otherwise() {
        let dir = scratch("inspect");
        let job = job(&dir);
        let clean = script(&dir, "clean", "exit 0");
        assert_eq!(
            cli(&dir, clean, 30).inspect(&job).expect("inspect"),
            Inspection::Clean
        );
        let problems = script(
            &dir,
            "problems",
            "echo 'text overlaps at 1.0s'; echo detail >&2; exit 1",
        );
        match cli(&dir, problems, 30).inspect(&job).expect("inspect") {
            Inspection::Problems(report) => {
                assert_eq!(report, "text overlaps at 1.0s\ndetail");
            }
            other => panic!("expected problems, got {other:?}"),
        }
        for code in [2, 3, 127] {
            let other = script(&dir, "other", &format!("echo boom >&2; exit {code}"));
            let error = cli(&dir, other, 30).inspect(&job).expect_err("failure");
            assert!(matches!(error, SceneError::Worker(_)));
            assert!(
                error.to_string().contains(&format!("exit {code}")),
                "{error}"
            );
        }
    }

    #[test]
    fn render_logs_exactly_the_arguments_and_needs_the_video() {
        let dir = scratch("render");
        let job = job(&dir);
        let log = dir.join("argv.log");
        let writes = script(
            &dir,
            "writes",
            &format!(
                "echo \"$@\" > '{}'\nwhile [ $# -gt 0 ]; do if [ \"$1\" = -o ]; then printf video > \"$2\"; fi; shift; done",
                log.display()
            ),
        );
        cli(&dir, writes, 30).render(&job).expect("render");
        let argv = fs::read_to_string(&log).expect("log");
        assert_eq!(
            argv.trim(),
            format!(
                "render {d}/scene.py ReelctlChart -o {d}/scene.mp4 -r 576x432 --fps 30",
                d = dir.display()
            )
        );
        assert_eq!(fs::read(&job.video_out).expect("video"), b"video");

        fs::remove_file(&job.video_out).expect("rm");
        let nothing = script(&dir, "nothing", "exit 0");
        let error = cli(&dir, nothing, 30).render(&job).expect_err("no video");
        assert!(error.to_string().contains("wrote no video"), "{error}");

        let empty = script(&dir, "empty", &format!(": > '{}'", job.video_out.display()));
        assert!(
            cli(&dir, empty, 30).render(&job).is_err(),
            "an empty file is no video"
        );

        let fails = script(&dir, "fails", "echo 'no GPU adapter' >&2; exit 1");
        let error = cli(&dir, fails, 30).render(&job).expect_err("fails");
        assert!(error.to_string().contains("no GPU adapter"), "{error}");
        assert!(error.to_string().contains("exit 1"), "{error}");
    }

    #[test]
    fn a_worker_that_does_not_finish_is_stopped() {
        let dir = scratch("timeout");
        let slow = script(&dir, "slow", "sleep 30");
        let started = Instant::now();
        let error = cli(&dir, slow, 1).render(&job(&dir)).expect_err("timeout");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "it was not stopped"
        );
        assert!(
            error.to_string().contains("did not finish in 1 s"),
            "{error}"
        );
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let dir = scratch("nostart");
        let missing = dir.join("missing");
        let error = cli(&dir, missing, 30).version().expect_err("no program");
        assert!(error.to_string().contains("cannot start"), "{error}");
    }

    #[test]
    fn a_long_log_is_cut_to_its_end() {
        let dir = scratch("tail");
        let noisy = script(
            &dir,
            "noisy",
            "i=0; while [ $i -lt 3000 ]; do echo \"line $i of noise\" >&2; i=$((i+1)); done; echo THE-END >&2; exit 2",
        );
        let error = cli(&dir, noisy, 60).inspect(&job(&dir)).expect_err("noisy");
        let text = error.to_string();
        assert!(text.contains("THE-END") && text.contains("..."), "{text}");
        assert!(text.len() < 5000, "{}", text.len());
        assert!(!text.contains("line 0 of noise"), "the start is cut");
    }

    #[test]
    fn the_worker_runs_in_its_scratch_directory_with_no_input() {
        let dir = scratch("cwd");
        let probe = script(
            &dir,
            "probe",
            "pwd > cwd.txt; if read line; then echo input > stdin.txt; fi; echo ok",
        );
        cli(&dir, probe, 30).version().expect("version");
        assert_eq!(
            fs::read_to_string(dir.join("cwd.txt")).expect("cwd").trim(),
            dir.to_str().expect("utf8")
        );
        assert!(!dir.join("stdin.txt").exists());
    }

    #[test]
    fn the_layout_report_joins_what_the_tool_printed_on_each_stream() {
        let dir = scratch("report");
        let job = job(&dir);
        let only_out = script(&dir, "only_out", "echo 'overlap at 1 s'; exit 1");
        assert_eq!(
            cli(&dir, only_out, 30).inspect(&job).expect("inspect"),
            Inspection::Problems("overlap at 1 s".to_string())
        );
        let only_err = script(&dir, "only_err", "echo 'overlap at 2 s' >&2; exit 1");
        assert_eq!(
            cli(&dir, only_err, 30).inspect(&job).expect("inspect"),
            Inspection::Problems("overlap at 2 s".to_string())
        );
        let neither = script(&dir, "neither", "exit 1");
        assert_eq!(
            cli(&dir, neither, 30).inspect(&job).expect("inspect"),
            Inspection::Problems(String::new())
        );
    }

    #[test]
    fn a_failure_shows_stdout_when_there_is_no_stderr_and_nothing_when_neither() {
        let dir = scratch("failure");
        let job = job(&dir);
        let out = script(&dir, "out", "echo 'only on stdout'; exit 2");
        let error = cli(&dir, out, 30).inspect(&job).expect_err("failure");
        assert!(
            error.to_string().ends_with("(exit 2):\nonly on stdout"),
            "{error}"
        );
        let both = script(
            &dir,
            "both",
            "echo 'on stdout'; echo 'on stderr' >&2; exit 2",
        );
        let error = cli(&dir, both, 30).inspect(&job).expect_err("failure");
        assert!(
            error.to_string().contains("on stderr") && !error.to_string().contains("on stdout"),
            "{error}"
        );
        let none = script(&dir, "none", "exit 2");
        let error = cli(&dir, none, 30).inspect(&job).expect_err("failure");
        assert_eq!(error.to_string(), "manimgx inspect failed (exit 2)");
    }
}
