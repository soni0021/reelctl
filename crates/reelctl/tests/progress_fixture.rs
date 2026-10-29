//! `--progress json` and clean cancel, end to end on synthetic fixtures.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("REELCTL_RVM_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
        .env_remove("MANIMGX_BIN")
        .env_remove("SMELTER_BIN")
        .env_remove("OPENROUTER_API_KEY")
        .stdin(Stdio::null());
    command
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reelctl-progress-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn generate(kind: &str, path: &Path) {
    let output = Command::new("bash")
        .arg(repo_root().join("fixtures/generate.sh"))
        .args([kind, &s(path)])
        .output()
        .expect("spawn fixture generator");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Every stderr line as an event; panics on a line that is not one.
fn events(stderr: &str) -> Vec<Value> {
    stderr
        .lines()
        .map(|line| {
            let event: Value =
                serde_json::from_str(line).unwrap_or_else(|_| panic!("not an event: {line}"));
            assert_eq!(event["v"], 1, "{line}");
            event
        })
        .collect()
}

/// The `key=value` fields of a result line.
fn fields(line: &str) -> Vec<(String, String)> {
    let rest = line.split_once(": ").map_or(
        line,
        |(head, rest)| if head.contains('=') { line } else { rest },
    );
    rest.split_whitespace()
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

#[test]
fn decompile_and_render_report_progress_and_a_result_equal_to_stdout() {
    let root = scratch("events");
    let video = root.join("draft.mp4");
    generate("draft12", &video);
    let dna = root.join("draft.edit_dna.json");

    // Without the flag stderr is empty, as before.
    let plain = reelctl()
        .args([
            "decompile",
            "--input",
            &s(&video),
            "--out",
            &s(&dna),
            "--filmstrip-dir",
            &s(&root.join("a")),
        ])
        .output()
        .unwrap();
    assert!(plain.status.success());
    assert!(
        plain.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );

    let run = reelctl()
        .args([
            "--progress",
            "json",
            "decompile",
            "--input",
            &s(&video),
            "--out",
            &s(&dna),
            "--filmstrip-dir",
            &s(&root.join("b")),
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    let stdout = String::from_utf8(run.stdout).unwrap();
    let list = events(&String::from_utf8(run.stderr).unwrap());
    assert!(list
        .iter()
        .any(|e| e["event"] == "stage" && e["name"] == "decode"));
    let decode: Vec<&Value> = list
        .iter()
        .filter(|e| e["event"] == "progress" && e["stage"] == "decode")
        .collect();
    assert!(!decode.is_empty());
    let last = decode.last().unwrap();
    assert_eq!(
        (last["done"].as_u64(), last["total"].as_u64()),
        (Some(360), Some(360)),
        "{last}"
    );
    let result = list
        .iter()
        .find(|e| e["event"] == "result")
        .expect("a result event");
    assert_eq!(result["command"], "decompile");
    for (key, value) in fields(stdout.trim()) {
        assert_eq!(result["fields"][&key], value.as_str(), "{key}");
    }
    // The same keys as the plain run's line (signal_ms differs between runs).
    let plain_keys: Vec<String> = fields(String::from_utf8(plain.stdout).unwrap().trim())
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    let keys: Vec<String> = fields(stdout.trim()).into_iter().map(|(k, _)| k).collect();
    assert_eq!(plain_keys, keys);

    let project = root.join("draft.otio");
    let init = reelctl()
        .args([
            "timeline",
            "init",
            "--input",
            &s(&video),
            "--out",
            &s(&project),
        ])
        .output()
        .unwrap();
    assert!(init.status.success());
    let out = root.join("draft.render.mp4");
    let render = reelctl()
        .args([
            "--progress",
            "json",
            "render",
            "--project",
            &s(&project),
            "--out",
            &s(&out),
        ])
        .output()
        .unwrap();
    assert!(
        render.status.success(),
        "{}",
        String::from_utf8_lossy(&render.stderr)
    );
    let list = events(&String::from_utf8(render.stderr).unwrap());
    let frames: Vec<&Value> = list
        .iter()
        .filter(|e| e["event"] == "progress" && e["stage"] == "render")
        .collect();
    assert!(!frames.is_empty());
    assert_eq!(frames.last().unwrap()["total"], 360);
    let result = list.iter().find(|e| e["event"] == "result").unwrap();
    assert_eq!(result["command"], "render");
    assert_eq!(result["fields"]["frames"], "360");
}

#[test]
fn a_failure_is_an_error_event_with_the_exit_code() {
    let output = reelctl()
        .args([
            "--progress",
            "json",
            "timeline",
            "init",
            "--input",
            "/no/such/file.mp4",
            "--out",
            "/tmp/x.otio",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let list = events(
        &String::from_utf8(output.stderr)
            .unwrap()
            .lines()
            .filter(|l| l.starts_with('{'))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    let error = list
        .iter()
        .find(|e| e["event"] == "error")
        .expect("an error event");
    assert_eq!(error["code"], 2);
}

#[test]
fn sigterm_during_a_render_exits_130_and_leaves_no_temporary_file() {
    let root = scratch("cancel");
    let video = root.join("bench.mp4");
    generate("bench60", &video);
    let project = root.join("bench.otio");
    let init = reelctl()
        .args([
            "timeline",
            "init",
            "--input",
            &s(&video),
            "--out",
            &s(&project),
        ])
        .output()
        .unwrap();
    assert!(init.status.success());
    let out_dir = root.join("out");
    fs::create_dir_all(&out_dir).unwrap();
    let out = out_dir.join("bench.render.mp4");
    let mut child = reelctl()
        .args([
            "--progress",
            "json",
            "render",
            "--project",
            &s(&project),
            "--out",
            &s(&out),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    // Wait for the render to be under way, then ask it to stop.
    let started = Instant::now();
    loop {
        let line = lines
            .next()
            .expect("stderr ended before any render progress")
            .unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        if event["event"] == "progress"
            && event["stage"] == "render"
            && event["done"].as_u64().unwrap_or(0) > 0
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "no render progress"
        );
    }
    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let rest: Vec<String> = lines.map_while(Result::ok).collect();
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "{rest:?}");
    assert!(
        rest.iter().any(|line| line.contains("\"code\":130")),
        "{rest:?}"
    );
    let left: Vec<String> = fs::read_dir(&out_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(left.is_empty(), "left behind: {left:?}");
}
