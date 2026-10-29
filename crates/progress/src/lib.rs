//! Progress events and cancellation for `reelctl`, shared by every crate that has
//! a long loop or runs a child process.
//!
//! With `--progress json`, `reelctl` calls [`enable_json`], and the functions here
//! write one JSON object per line to stderr (schema version 1):
//! `{"v":1,"event":"stage","name":...}`, `{"v":1,"event":"progress","stage":...,
//! "done":...,"total":...}`, `{"v":1,"event":"warning","message":...}`,
//! `{"v":1,"event":"result","command":...,"fields":{...}}` and
//! `{"v":1,"event":"error","code":...,"message":...}`. Without it every call is a
//! no-op, so stderr is exactly as it was. Progress is throttled to at most
//! [`MAX_PER_SECOND`] events per stage, and the last one of a stage is always written.
//!
//! Cancellation: a signal handler calls [`request_cancel`]; loops poll
//! [`cancel_requested`] and stop with an error, so the normal clean-up (temporary
//! files removed by their guards) runs; child processes registered with [`track`]
//! are sent SIGTERM by [`terminate_tracked`].

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

/// The event schema version.
pub const SCHEMA_VERSION: u32 = 1;
/// At most this many progress events per second for one stage.
pub const MAX_PER_SECOND: u32 = 10;
/// The exit code of a cancelled run (128 + SIGINT, as shells report it).
pub const CANCELLED_EXIT: i32 = 130;

static JSON: AtomicBool = AtomicBool::new(false);
static CANCELLED: AtomicBool = AtomicBool::new(false);

struct Stage {
    total: Option<u64>,
    last: Option<Instant>,
}

fn stages() -> &'static Mutex<HashMap<String, Stage>> {
    static STAGES: OnceLock<Mutex<HashMap<String, Stage>>> = OnceLock::new();
    STAGES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn children() -> &'static Mutex<BTreeMap<u32, ()>> {
    static CHILDREN: OnceLock<Mutex<BTreeMap<u32, ()>>> = OnceLock::new();
    CHILDREN.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Write events as JSON lines on stderr from now on.
pub fn enable_json() {
    JSON.store(true, Ordering::SeqCst);
}

pub fn json_enabled() -> bool {
    JSON.load(Ordering::SeqCst)
}

fn emit(mut event: Map<String, Value>) {
    if !json_enabled() {
        return;
    }
    let mut line = Map::new();
    line.insert("v".to_string(), json!(SCHEMA_VERSION));
    line.append(&mut event);
    let text = Value::Object(line).to_string();
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{text}");
}

fn object(pairs: Vec<(&str, Value)>) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// A stage starts: `decode`, `whisper`, `render`, ...
pub fn stage(name: &str) {
    emit(object(vec![
        ("event", json!("stage")),
        ("name", json!(name)),
    ]));
}

/// The number of units a stage will take, when it is known.
pub fn set_total(stage: &str, total: u64) {
    if let Ok(mut map) = stages().lock() {
        map.entry(stage.to_string())
            .or_insert(Stage {
                total: None,
                last: None,
            })
            .total = Some(total);
    }
}

/// `done` units of `stage` are finished. Written at most [`MAX_PER_SECOND`] times
/// a second, and always when `done` reaches the known total.
pub fn tick(stage: &str, done: u64) {
    if !json_enabled() {
        return;
    }
    let total = {
        let Ok(mut map) = stages().lock() else {
            return;
        };
        let entry = map.entry(stage.to_string()).or_insert(Stage {
            total: None,
            last: None,
        });
        let now = Instant::now();
        let finished = entry.total.is_some_and(|total| done >= total);
        let gap = Duration::from_millis(1_000 / u64::from(MAX_PER_SECOND));
        if !finished
            && entry
                .last
                .is_some_and(|last| now.duration_since(last) < gap)
        {
            return;
        }
        entry.last = Some(now);
        entry.total
    };
    let mut event = object(vec![
        ("event", json!("progress")),
        ("stage", json!(stage)),
        ("done", json!(done)),
    ]);
    if let Some(total) = total {
        event.insert("total".to_string(), json!(total));
    }
    emit(event);
}

/// A note for the user: a `warning` event with `--progress json`, else the text
/// on stderr as it always was.
pub fn note(text: &str) {
    if json_enabled() {
        warning(text);
    } else {
        eprintln!("{text}");
    }
}

/// A stage is over at `done`: always written, with `total` set to `done` (an
/// estimate made at the start is replaced by the count).
pub fn finish(stage: &str, done: u64) {
    set_total(stage, done);
    tick(stage, done);
}

pub fn warning(message: &str) {
    emit(object(vec![
        ("event", json!("warning")),
        ("message", json!(message)),
    ]));
}

pub fn error(code: i32, message: &str) {
    emit(object(vec![
        ("event", json!("error")),
        ("code", json!(code)),
        ("message", json!(message)),
    ]));
}

/// The `key=value` fields of a result line (`init: out=a.otio frames=300 ...`), and
/// the command named before the colon. A value runs to the next space; a quoted
/// value (`hook="..."`) runs to the closing quote.
pub fn parse_result_line(line: &str) -> (Option<String>, BTreeMap<String, String>) {
    let mut fields = BTreeMap::new();
    let (command, rest) = match line.split_once(": ") {
        Some((head, rest)) if !head.contains('=') && !head.contains(' ') => {
            (Some(head.to_string()), rest)
        }
        _ => (None, line),
    };
    let mut chars = rest.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ' ') {
            chars.next();
        }
        let mut key = String::new();
        while let Some(&c) = chars.peek() {
            if c == '=' || c == ' ' {
                break;
            }
            key.push(c);
            chars.next();
        }
        if key.is_empty() {
            break;
        }
        if chars.peek() != Some(&'=') {
            continue;
        }
        chars.next();
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                value.push(c);
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        fields.insert(key, value);
    }
    (command, fields)
}

/// A line printed on stdout as a command's result: with JSON on, also a `result`
/// event with the same fields. A JSON report on stdout (`diff`, `eval`) is
/// announced with `"stdout": "json"` and no fields.
pub fn result_line(default_command: &str, line: &str) {
    if !json_enabled() {
        return;
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        emit(object(vec![
            ("event", json!("result")),
            ("command", json!(default_command)),
            ("stdout", json!("json")),
            ("fields", json!({})),
        ]));
        return;
    }
    let (command, fields) = parse_result_line(line);
    emit(object(vec![
        ("event", json!("result")),
        (
            "command",
            json!(command.unwrap_or_else(|| default_command.to_string())),
        ),
        ("fields", json!(fields)),
    ]));
}

/// A stop was asked for (SIGTERM or SIGINT).
pub fn request_cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
}

pub fn cancel_requested() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

/// The error message of a loop that stopped because of [`cancel_requested`].
pub const CANCELLED_MESSAGE: &str = "cancelled";

/// A registered child process: SIGTERM reaches it through [`terminate_tracked`].
/// Dropping the guard forgets it.
pub struct Tracked(u32);

impl Drop for Tracked {
    fn drop(&mut self) {
        if let Ok(mut map) = children().lock() {
            map.remove(&self.0);
        }
    }
}

/// Register a child by its process id.
pub fn track(pid: u32) -> Tracked {
    if let Ok(mut map) = children().lock() {
        map.insert(pid, ());
    }
    Tracked(pid)
}

/// The children registered now.
pub fn tracked() -> Vec<u32> {
    children()
        .lock()
        .map(|map| map.keys().copied().collect())
        .unwrap_or_default()
}

/// Send SIGTERM to every registered child, with the system's `kill` (no unsafe
/// code here). Returns how many were signalled.
pub fn terminate_tracked() -> usize {
    let pids = tracked();
    for pid in &pids {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    pids.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_line_splits_into_its_command_and_fields() {
        let (command, fields) =
            parse_result_line("init: out=/a b.otio frames=300 fps=30.000 size=640x360 audio=true");
        assert_eq!(command.as_deref(), Some("init"));
        assert_eq!(fields["frames"], "300");
        assert_eq!(fields["out"], "/a");
        assert_eq!(fields["size"], "640x360");
        let (command, fields) = parse_result_line(
            "make-reels: clip=1 tight_length_ms=31000 flags=0 hook=\"Sentence 1 is about topic.\"",
        );
        assert_eq!(command.as_deref(), Some("make-reels"));
        assert_eq!(fields["hook"], "Sentence 1 is about topic.");
        // decompile's line has no command before a colon.
        let (command, fields) = parse_result_line("signal_ms=310 asr_ms=4 cuts=1");
        assert_eq!(command, None);
        assert_eq!(fields["cuts"], "1");
    }

    #[test]
    fn tracked_children_are_forgotten_when_their_guard_drops() {
        let before = tracked().len();
        {
            let _guard = track(4_000_000_001);
            assert!(tracked().contains(&4_000_000_001));
        }
        assert_eq!(tracked().len(), before);
    }

    #[test]
    fn cancel_is_a_flag_anyone_can_read() {
        assert!(!cancel_requested() || cancel_requested());
        request_cancel();
        assert!(cancel_requested());
        CANCELLED.store(false, Ordering::SeqCst);
    }
}
