//! Running whisper passes in a child process.
//!
//! whisper.cpp 1.8.3 aborts the whole process when DTW meets a window of
//! fewer audio tokens than its median filter is wide (`WHISPER_ASSERT
//! (filter_width < a->ne[2])`, fixed upstream in ggml-org/whisper.cpp#4096
//! after the last release). `reelctl` therefore runs whisper in a child
//! `reelctl whisper-worker`, which loads the model once and answers one JSON
//! line per pass. If the child dies, the parent runs that pass again in a
//! fresh child without DTW, then goes back to DTW for the passes after it.
//!
//! The parent writes the 16 kHz audio to a temporary file once; each request
//! names a span of it. The file is removed when the run ends, on success or
//! failure.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::coverage::WordStart;
use crate::error::DecompileError;
use crate::whisper::{preset_for, read_shape, Local, Padding, Pass, PassOutput, Passes};

/// Where whisper passes run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WhisperRunner {
    /// In this process. A whisper.cpp abort ends the process.
    #[default]
    InProcess,
    /// In a child process: `program`, then `args`, then `--model`, `--audio`,
    /// and `--no-dtw` when DTW is off. `reelctl` uses itself with
    /// `whisper-worker`.
    Worker { program: PathBuf, args: Vec<String> },
}

/// The worker side: loads the model once, then answers each request line
/// read from `input` with one line on `output`, until `input` ends.
pub fn run_whisper_worker(
    model: &Path,
    audio: &Path,
    dtw: bool,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<(), DecompileError> {
    let samples = read_audio(audio)?;
    let preset = if dtw {
        let shape = read_shape(model)?;
        let (preset, _) = preset_for(&shape).ok_or_else(|| {
            DecompileError::Whisper(format!(
                "model {} has a shape no DTW preset matches: {shape:?}",
                model.display()
            ))
        })?;
        Some(preset)
    } else {
        None
    };
    let mut local = Local::load(model, preset, samples)?;
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let pass = decode_pass(&line)?;
        let answer = local.run(&pass)?;
        writeln!(output, "{}", encode_output(&answer))?;
        output.flush()?;
    }
    Ok(())
}

/// The parent side of the worker protocol.
pub(crate) struct Remote {
    program: PathBuf,
    args: Vec<String>,
    model: PathBuf,
    audio: TempAudio,
    worker: Option<Worker>,
    fallbacks: Vec<Pass>,
}

struct Worker {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    /// A cancel (SIGTERM to `reelctl`) reaches the worker too.
    _tracked: progress::Tracked,
}

impl Remote {
    /// Writes `audio` to a temporary file. Workers start when the first pass
    /// is asked for.
    pub(crate) fn start(
        program: &Path,
        args: &[String],
        model: &Path,
        audio: &[f32],
    ) -> Result<Self, DecompileError> {
        Ok(Self {
            program: program.to_path_buf(),
            args: args.to_vec(),
            model: model.to_path_buf(),
            audio: TempAudio::write(audio)?,
            worker: None,
            fallbacks: Vec::new(),
        })
    }

    fn spawn(&self, dtw: bool) -> Result<Worker, DecompileError> {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .arg("--model")
            .arg(&self.model)
            .arg("--audio")
            .arg(&self.audio.path);
        if !dtw {
            command.arg("--no-dtw");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|err| {
                DecompileError::Whisper(format!(
                    "could not start the whisper worker {}: {err}",
                    self.program.display()
                ))
            })?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().map(BufReader::new).ok_or_else(|| {
            DecompileError::Whisper("the whisper worker has no stdout".to_string())
        })?;
        progress::stage("whisper");
        let tracked = progress::track(child.id());
        Ok(Worker {
            child,
            stdin,
            stdout,
            _tracked: tracked,
        })
    }
}

impl Passes for Remote {
    fn run(&mut self, pass: &Pass) -> Result<PassOutput, DecompileError> {
        if self.worker.is_none() {
            self.worker = Some(self.spawn(true)?);
        }
        let worker = self.worker.as_mut().expect("worker started above");
        if let Some(answer) = ask(worker, pass)? {
            return Ok(answer);
        }
        let died = describe(&end(self.worker.take().expect("worker started above")));
        let mut plain = self.spawn(false)?;
        let answer = ask(&mut plain, pass)?;
        let plain_status = end(plain);
        match answer {
            Some(answer) => {
                self.fallbacks.push(pass.clone());
                Ok(answer)
            }
            None => Err(DecompileError::Whisper(format!(
                "the whisper worker {died} on {:.1}-{:.1} s, and {} again without DTW",
                pass.start_ms as f64 / 1_000.0,
                pass.end_ms as f64 / 1_000.0,
                describe(&plain_status)
            ))),
        }
    }

    fn fallbacks(&self) -> Vec<Pass> {
        self.fallbacks.clone()
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            end(worker);
        }
    }
}

/// Sends one request; `None` means the worker ended without answering.
fn ask(worker: &mut Worker, pass: &Pass) -> Result<Option<PassOutput>, DecompileError> {
    let Some(stdin) = worker.stdin.as_mut() else {
        return Ok(None);
    };
    let sent = writeln!(stdin, "{}", encode_pass(pass)).and_then(|()| stdin.flush());
    if sent.is_err() {
        return Ok(None);
    }
    let mut line = String::new();
    match worker.stdout.read_line(&mut line) {
        Ok(0) | Err(_) => Ok(None),
        Ok(_) => decode_output(&line).map(Some),
    }
}

/// Closes the worker's input and waits for it to exit.
fn end(mut worker: Worker) -> Option<ExitStatus> {
    drop(worker.stdin.take());
    worker.child.wait().ok()
}

fn describe(status: &Option<ExitStatus>) -> String {
    let Some(status) = status else {
        return "ended".to_string();
    };
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            let name = if signal == 6 { " (SIGABRT)" } else { "" };
            return format!("was killed by signal {signal}{name}");
        }
    }
    match status.code() {
        Some(code) => format!("exited with code {code}"),
        None => "ended".to_string(),
    }
}

fn encode_pass(pass: &Pass) -> Value {
    json!({
        "start_ms": pass.start_ms,
        "end_ms": pass.end_ms,
        "padding": match pass.padding {
            Padding::None => "none",
            Padding::Around => "around",
        },
        "language": pass.language,
    })
}

fn decode_pass(line: &str) -> Result<Pass, DecompileError> {
    let value: Value = serde_json::from_str(line)?;
    let padding = match value["padding"].as_str() {
        Some("none") => Padding::None,
        Some("around") => Padding::Around,
        other => return Err(protocol(format!("unknown padding {other:?}"))),
    };
    Ok(Pass {
        start_ms: field_u64(&value, "start_ms")?,
        end_ms: field_u64(&value, "end_ms")?,
        padding,
        language: value["language"].as_str().map(str::to_string),
    })
}

fn encode_output(output: &PassOutput) -> Value {
    let words: Vec<Value> = output
        .words
        .iter()
        .map(|word| {
            json!({
                "text": word.text,
                "start_ms": word.start_ms,
                "segment_end_ms": word.segment_end_ms,
            })
        })
        .collect();
    json!({ "words": words, "language": output.language })
}

fn decode_output(line: &str) -> Result<PassOutput, DecompileError> {
    let value: Value = serde_json::from_str(line)?;
    let words = value["words"]
        .as_array()
        .ok_or_else(|| protocol("answer without words".to_string()))?
        .iter()
        .map(|word| {
            Ok(WordStart {
                text: word["text"]
                    .as_str()
                    .ok_or_else(|| protocol("word without text".to_string()))?
                    .to_string(),
                start_ms: field_u64(word, "start_ms")?,
                segment_end_ms: field_u64(word, "segment_end_ms")?,
            })
        })
        .collect::<Result<Vec<_>, DecompileError>>()?;
    Ok(PassOutput {
        words,
        language: value["language"].as_str().map(str::to_string),
    })
}

fn field_u64(value: &Value, key: &str) -> Result<u64, DecompileError> {
    value[key]
        .as_u64()
        .ok_or_else(|| protocol(format!("missing {key}")))
}

fn protocol(detail: String) -> DecompileError {
    DecompileError::Whisper(format!("whisper worker protocol: {detail}"))
}

/// 16 kHz mono `f32` audio in a temporary file, removed on drop.
struct TempAudio {
    path: PathBuf,
}

impl TempAudio {
    fn write(audio: &[f32]) -> Result<Self, DecompileError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "reelctl-whisper-{}-{nanos}.f32",
            std::process::id()
        ));
        let bytes: Vec<u8> = audio
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        std::fs::write(&path, bytes)?;
        Ok(Self { path })
    }
}

impl Drop for TempAudio {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_audio(path: &Path) -> Result<Vec<f32>, DecompileError> {
    let bytes = std::fs::read(path)?;
    Ok(bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    const ANSWER: &str =
        r#"{"words":[{"text":"hi","start_ms":10,"segment_end_ms":20}],"language":"en"}"#;

    fn pass() -> Pass {
        Pass {
            start_ms: 16_000,
            end_ms: 30_900,
            padding: Padding::Around,
            language: Some("en".to_string()),
        }
    }

    /// A stand-in worker: `/bin/sh -c <script>`, which sees the parent's
    /// `--model ... --audio ... [--no-dtw]` as its positional arguments.
    fn fake(script: &str) -> Remote {
        Remote::start(
            Path::new("/bin/sh"),
            &["-c".to_string(), script.to_string(), "sh".to_string()],
            Path::new("/nonexistent/model.bin"),
            &[0.0; 1_600],
        )
        .expect("start")
    }

    #[test]
    fn an_answer_is_read_back_as_word_starts() {
        let mut remote = fake(&format!("read line; echo '{ANSWER}'; cat > /dev/null"));
        let answer = remote.run(&pass()).expect("answer");
        assert_eq!(answer.words.len(), 1);
        assert_eq!(answer.words[0].text, "hi");
        assert_eq!(answer.language.as_deref(), Some("en"));
        assert!(remote.fallbacks().is_empty());
    }

    #[test]
    fn an_abort_with_dtw_is_retried_without_it() {
        let script = format!(
            "case \" $* \" in *' --no-dtw '*) read line; echo '{ANSWER}'; cat > /dev/null;; *) kill -ABRT $$;; esac"
        );
        let mut remote = fake(&script);
        let answer = remote.run(&pass()).expect("the retry answers");
        assert_eq!(answer.words[0].text, "hi");
        assert_eq!(remote.fallbacks(), vec![pass()]);
    }

    #[test]
    fn an_abort_without_dtw_too_is_an_error_naming_both() {
        let mut remote = fake("kill -ABRT $$");
        let err = remote.run(&pass()).expect_err("both attempts abort");
        let message = err.to_string();
        assert!(message.contains("signal 6 (SIGABRT)"), "{message}");
        assert!(message.contains("again without DTW"), "{message}");
        assert!(message.contains("16.0-30.9 s"), "{message}");
    }

    #[test]
    fn the_temporary_audio_is_removed_after_the_run() {
        let remote = fake("cat > /dev/null");
        let path = remote.audio.path.clone();
        assert!(path.is_file());
        drop(remote);
        assert!(!path.exists());
    }

    #[test]
    fn requests_and_answers_survive_the_round_trip() {
        let decoded = decode_pass(&encode_pass(&pass()).to_string()).expect("decode");
        assert_eq!(decoded, pass());
        let output = decode_output(ANSWER).expect("decode");
        let again = decode_output(&encode_output(&output).to_string()).expect("decode");
        assert_eq!(again.words, output.words);
    }
}
