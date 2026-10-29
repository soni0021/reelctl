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
static COMMAND: OnceLock<String> = OnceLock::new();

/// The subcommand being run (`look`, `timeline init`, ...): the `command` of a
/// result line that does not name one.
pub fn set_command(name: &str) {
    let _ = COMMAND.set(name.to_string());
}

fn command_or(default_command: &str) -> String {
    COMMAND
        .get()
        .cloned()
        .unwrap_or_else(|| default_command.to_string())
}
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
/// the command named before the colon. A value runs to the next ` key=` (a space,
/// then a lower-case name of letters, digits and `_`, then `=`), so a path with a
/// space in it stays whole; a quoted value (`hook="..."`) runs to the closing
/// quote.
pub fn parse_result_line(line: &str) -> (Option<String>, BTreeMap<String, String>) {
    let mut fields = BTreeMap::new();
    let (command, rest) = match line.split_once(": ") {
        Some((head, rest)) if !head.contains('=') && !head.contains(' ') => {
            (Some(head.to_string()), rest)
        }
        _ => (None, line),
    };
    let bytes = rest.as_bytes();
    let is_key_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_';
    // The end of the `key` that starts at `at`, if a `key=` starts there.
    let key_at = |at: usize| -> Option<usize> {
        let mut end = at;
        while end < bytes.len() && is_key_char(bytes[end]) {
            end += 1;
        }
        (end > at && end < bytes.len() && bytes[end] == b'=' && bytes[at].is_ascii_lowercase())
            .then_some(end)
    };
    let starts: Vec<usize> = (0..bytes.len())
        .filter(|&at| (at == 0 || bytes[at - 1] == b' ') && key_at(at).is_some())
        .collect();
    let mut index = 0;
    while index < starts.len() {
        let at = starts[index];
        let Some(eq) = key_at(at) else {
            index += 1;
            continue;
        };
        let key = rest[at..eq].to_string();
        let value_start = eq + 1;
        if bytes.get(value_start) == Some(&b'"') {
            let close = rest[value_start + 1..]
                .find('"')
                .map_or(rest.len(), |i| value_start + 1 + i);
            fields.insert(key, rest[value_start + 1..close].to_string());
            index += 1;
            // Key-like words inside the quotes are not keys.
            while index < starts.len() && starts[index] <= close {
                index += 1;
            }
            continue;
        }
        let value_end = starts.get(index + 1).map_or(rest.len(), |next| next - 1);
        fields.insert(
            key,
            rest[value_start..value_end.max(value_start)]
                .trim_end()
                .to_string(),
        );
        index += 1;
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
            ("command", json!(command_or(default_command))),
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
            json!(COMMAND
                .get()
                .cloned()
                .or(command)
                .unwrap_or_else(|| default_command.to_string())),
        ),
        ("fields", json!(fields)),
    ]));
}

fn temp_files() -> &'static Mutex<BTreeMap<std::path::PathBuf, ()>> {
    static TEMP: OnceLock<Mutex<BTreeMap<std::path::PathBuf, ()>>> = OnceLock::new();
    TEMP.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// A temporary output that must not outlive a cancel, even one that ends the
/// process before its own guard runs: [`remove_temporaries`] deletes it.
pub struct Temporary(std::path::PathBuf);

impl Drop for Temporary {
    fn drop(&mut self) {
        if let Ok(mut map) = temp_files().lock() {
            map.remove(&self.0);
        }
    }
}

/// Register a temporary file or folder.
pub fn temporary(path: &std::path::Path) -> Temporary {
    if let Ok(mut map) = temp_files().lock() {
        map.insert(path.to_path_buf(), ());
    }
    Temporary(path.to_path_buf())
}

/// Delete every registered temporary (the last step before a forced exit).
pub fn remove_temporaries() {
    let paths: Vec<std::path::PathBuf> = temp_files()
        .lock()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    for path in paths {
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(&path);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// `Err(CANCELLED_MESSAGE)` once a stop was asked for: called before each long
/// step and before an output is written, so a cancelled run writes nothing more.
pub fn check() -> Result<(), &'static str> {
    if cancel_requested() {
        Err(CANCELLED_MESSAGE)
    } else {
        Ok(())
    }
}

/// A stop was asked for (SIGTERM, SIGINT or SIGHUP).
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

/// Run `command` to completion like [`std::process::Command::output`], with the
/// child registered while it runs, so a forced exit signals it too.
pub fn tracked_output(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Output> {
    let child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let _tracked = track(child.id());
    child.wait_with_output()
}

/// The children registered now.
pub fn tracked() -> Vec<u32> {
    children()
        .lock()
        .map(|map| map.keys().copied().collect())
        .unwrap_or_default()
}

/// Send SIGTERM to every registered child (with `kill(2)` through `nix`, a safe
/// API; no `kill` process). Returns how many were signalled.
pub fn terminate_tracked() -> usize {
    let mut sent = 0;
    for pid in tracked() {
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            let Ok(raw) = i32::try_from(pid) else {
                continue;
            };
            if kill(nix::unistd::Pid::from_raw(raw), Signal::SIGTERM).is_ok() {
                sent += 1;
            }
        }
        #[cfg(not(unix))]
        let _ = pid;
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_result_line_splits_into_its_command_and_fields() {
        let (command, fields) = parse_result_line(
            "init: out=/my dir/a b.otio frames=300 fps=30.000 size=640x360 audio=true",
        );
        assert_eq!(command.as_deref(), Some("init"));
        assert_eq!(fields["frames"], "300");
        assert_eq!(fields["out"], "/my dir/a b.otio");
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
        // A line with no fields at all.
        let (_, fields) =
            parse_result_line("wrote 4 records (3 train, 1 val) from 2 reels, 0 skipped, to /out");
        assert!(fields.is_empty());
        // A quoted value may hold `=` and spaces.
        let (_, fields) = parse_result_line("clip=1 hook=\"a b=c\" flags=0");
        assert_eq!(fields["hook"], "a b=c");
        assert_eq!(fields["flags"], "0");
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
    fn cancel_is_a_flag_and_check_turns_it_into_an_error() {
        CANCELLED.store(false, Ordering::SeqCst);
        assert_eq!(check(), Ok(()));
        request_cancel();
        assert!(cancel_requested());
        assert_eq!(check(), Err(CANCELLED_MESSAGE));
        CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn registered_temporaries_are_removed_and_forgotten_by_their_guards() {
        let dir = std::env::temp_dir().join(format!("progress-temp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.tmp");
        std::fs::write(&file, b"x").unwrap();
        let guard = temporary(&file);
        remove_temporaries();
        assert!(!file.exists());
        drop(guard);
        std::fs::write(&file, b"x").unwrap();
        remove_temporaries();
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
