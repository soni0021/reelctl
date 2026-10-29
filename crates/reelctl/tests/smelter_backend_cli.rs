//! `reelctl render --backend` from the command line, without a Smelter: the default
//! backend is unchanged and never looks for Smelter, and `--backend smelter` fails
//! closed (exit 1) with a message that says how to point at one. The Smelter
//! backend's work with a server is tested in `smelter_backend.rs` against a fake on the
//! loopback; the measurements against a real Smelter are in docs/status.md.
//! Needs `ffmpeg` and `ffprobe` (as `fixtures/generate.sh` does).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model, weight and Smelter paths a shell profile may export.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("SMELTER_BIN");
    command
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("reelctl-smelter-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn run(command: &mut Command) -> Output {
    command.output().expect("spawn reelctl")
}

/// A project over a 12 s synthetic clip, in a fresh directory.
fn project(name: &str) -> (PathBuf, PathBuf) {
    let dir = scratch(name);
    let video = dir.join("draft.mp4");
    let status = Command::new("bash")
        .arg(repo_root().join("fixtures/generate.sh"))
        .arg("draft12")
        .arg(&video)
        .status()
        .expect("spawn the fixture generator");
    assert!(status.success());
    let project = dir.join("edit.otio");
    let output = run(reelctl().args([
        "timeline",
        "init",
        "--input",
        &s(&video),
        "--out",
        &s(&project),
    ]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    (dir, project)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn the_default_backend_is_ffmpeg_and_never_looks_for_smelter() {
    let (dir, project) = project("default");
    let out = dir.join("out.mp4");
    // SMELTER_BIN names nothing: if the default backend read it, the render would fail.
    let output = run(reelctl()
        .env("SMELTER_BIN", dir.join("no-such-smelter"))
        .args(["render", "--project", &s(&project), "--out", &s(&out)]));
    assert!(output.status.success(), "{}", text(&output.stderr));
    let line = text(&output.stdout);
    assert!(line.starts_with("render: out="), "{line}");
    assert!(
        line.contains("frames=360") && line.contains(" ffmpeg_ms="),
        "{line}"
    );
    assert!(!line.contains("smelter_ms"), "{line}");
    assert!(out.is_file());
}

#[test]
fn naming_ffmpeg_is_the_same_render() {
    let (dir, project) = project("named");
    let (plain, named) = (dir.join("a.mp4"), dir.join("b.mp4"));
    let a = run(reelctl().args(["render", "--project", &s(&project), "--out", &s(&plain)]));
    let b = run(reelctl().args([
        "render",
        "--project",
        &s(&project),
        "--out",
        &s(&named),
        "--backend",
        "ffmpeg",
    ]));
    assert!(a.status.success() && b.status.success());
    // The same filter graph, so the same file apart from the timing in the line.
    assert_eq!(fs::read(&plain).expect("a"), fs::read(&named).expect("b"));
    let strip = |bytes: &[u8]| {
        text(bytes)
            .split(" ffmpeg_ms=")
            .next()
            .unwrap_or_default()
            .replace("a.mp4", "X")
            .replace("b.mp4", "X")
    };
    assert_eq!(strip(&a.stdout), strip(&b.stdout));
}

#[test]
fn the_smelter_backend_without_smelter_fails_closed_and_says_how_to_fix_it() {
    let (dir, project) = project("absent");
    let out = dir.join("out.mp4");
    // SMELTER_BIN set to a file that is not there.
    let output = run(reelctl()
        .env("SMELTER_BIN", dir.join("no-such-smelter"))
        .args([
            "render",
            "--project",
            &s(&project),
            "--out",
            &s(&out),
            "--backend",
            "smelter",
        ]));
    assert_eq!(output.status.code(), Some(1));
    let error = text(&output.stderr);
    assert!(
        error.starts_with("error: Smelter was not found at "),
        "{error}"
    );
    assert!(
        error.contains("SMELTER_BIN") && error.contains("--smelter"),
        "{error}"
    );
    assert!(error.contains("--backend ffmpeg"), "{error}");
    assert!(!out.exists(), "nothing is written");

    // And with no SMELTER_BIN and no `smelter` on PATH (a PATH with only ffmpeg's tools).
    let tools = dir.join("tools");
    fs::create_dir_all(&tools).expect("tools dir");
    for tool in ["ffmpeg", "ffprobe"] {
        let found = Command::new("sh")
            .args(["-c", &format!("command -v {tool}")])
            .output()
            .expect("which");
        let path = text(&found.stdout).trim().to_string();
        std::os::unix::fs::symlink(&path, tools.join(tool)).expect("link");
    }
    let output = run(reelctl().env("PATH", &tools).args([
        "render",
        "--project",
        &s(&project),
        "--out",
        &s(&out),
        "--backend",
        "smelter",
    ]));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("Smelter was not found on PATH"),
        "{}",
        text(&output.stderr)
    );
    assert!(!out.exists());
}

#[test]
fn the_smelter_flag_names_the_server_and_a_file_that_is_not_one_fails() {
    let (dir, project) = project("flag");
    let out = dir.join("out.mp4");
    // A file that exists but is not Smelter: it is started, exits, and its output is reported.
    let fake = dir.join("fake-smelter");
    fs::write(&fake, "#!/bin/sh\necho 'not a smelter' >&2\nexit 7\n").expect("script");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
    let output = run(reelctl().args([
        "render",
        "--project",
        &s(&project),
        "--out",
        &s(&out),
        "--backend",
        "smelter",
        "--smelter",
        &s(&fake),
    ]));
    assert_eq!(output.status.code(), Some(1));
    let error = text(&output.stderr);
    assert!(error.contains("exited before it answered"), "{error}");
    assert!(error.contains("not a smelter"), "{error}");
    assert!(!out.exists());
}

#[test]
fn an_unknown_backend_is_a_usage_error_and_help_lists_the_flags() {
    let output = run(reelctl().args([
        "render",
        "--project",
        "x",
        "--out",
        "y",
        "--backend",
        "nonsense",
    ]));
    assert_eq!(output.status.code(), Some(2));
    assert!(
        text(&output.stderr).contains("possible values: ffmpeg, smelter"),
        "{}",
        text(&output.stderr)
    );
    let help = run(reelctl().args(["render", "--help"]));
    let help = text(&help.stdout);
    assert!(
        help.contains("--backend") && help.contains("--smelter") && help.contains("SMELTER_BIN"),
        "{help}"
    );
}
