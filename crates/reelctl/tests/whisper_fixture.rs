//! End-to-end word timing with whisper.cpp.
//!
//! The model test needs a model file and macOS `say`, so it is ignored by
//! default. Run it with:
//! `REELCTL_WHISPER_MODEL=~/models/whisper/<model>.bin cargo test -- --include-ignored`

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use edit_dna::{AsrBackend, EditDna};

const KEY_WORDS: [&str; 6] = ["cut", "dead", "air", "proof", "viewer", "scrolls"];
const PAUSE_MARGIN_MS: u64 = 200;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model and weight paths a shell profile may export.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("REELCTL_RVM_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
        .env_remove("OPENROUTER_API_KEY")
        .env_remove("MANIMGX_BIN");
    command
}

fn generate(kind: &str, path: &Path) {
    let output = Command::new("bash")
        .arg(repo_root().join("fixtures/generate.sh"))
        .arg(kind)
        .arg(path)
        .output()
        .expect("spawn fixture generator");
    assert!(
        output.status.success(),
        "generate.sh {kind} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reelctl-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn a_missing_whisper_model_exits_with_code_1_naming_the_path() {
    let dir = temp_dir("missing-model");
    let input = dir.join("draft_12s.mp4");
    generate("draft12", &input);
    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(dir.join("draft.edit_dna.json"))
        .args(["--whisper-model", "/nonexistent/ggml-missing.bin"])
        .output()
        .expect("spawn reelctl");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(
        stderr.contains("/nonexistent/ggml-missing.bin"),
        "stderr: {stderr}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
#[ignore = "needs a whisper model in REELCTL_WHISPER_MODEL and macOS `say`"]
fn whisper_finds_the_spoken_words_on_either_side_of_the_planted_pause() {
    let model = std::env::var_os("REELCTL_WHISPER_MODEL")
        .expect("set REELCTL_WHISPER_MODEL to a ggml whisper model");
    let dir = temp_dir("speech12");
    let input = dir.join("speech_12s.mp4");
    let out = dir.join("speech.edit_dna.json");
    generate("speech12", &input);
    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&out)
        .arg("--whisper-model")
        .arg(&model)
        .output()
        .expect("spawn reelctl");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "stderr: {stderr}");
    assert!(stderr.is_empty(), "whisper.cpp printed: {stderr}");
    let dna: EditDna = serde_json::from_slice(&fs::read(&out).expect("read json")).expect("parse");

    assert_eq!(dna.asr_backend, AsrBackend::WhisperCpp, "{}", dna.asr_note);
    let texts: Vec<String> = dna
        .words
        .iter()
        .map(|word| {
            word.text
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .collect();
    let mut remaining = texts.iter();
    for key in KEY_WORDS {
        assert!(
            remaining.any(|text| text == key),
            "{key:?} missing or out of order in {texts:?}"
        );
    }

    for (word, next) in dna.words.iter().zip(dna.words.iter().skip(1)) {
        assert!(word.start_ms <= next.start_ms, "{word:?} then {next:?}");
    }
    for word in &dna.words {
        assert!(
            word.start_ms <= word.end_ms && word.end_ms <= dna.duration_ms,
            "{word:?}"
        );
    }

    // The planted pause is the silence gap of at least 900 ms between the first and last word.
    let first = dna.words.first().expect("words").start_ms;
    let last = dna.words.last().expect("words").start_ms;
    let pause = dna
        .silence_gaps
        .iter()
        .find(|gap| gap.duration_ms >= 900 && gap.start_ms > first && gap.end_ms < last)
        .unwrap_or_else(|| panic!("no planted pause in {:?}", dna.silence_gaps));
    let deep_inside =
        |t: u64| t > pause.start_ms + PAUSE_MARGIN_MS && t + PAUSE_MARGIN_MS < pause.end_ms;
    for word in &dna.words {
        assert!(
            !deep_inside(word.start_ms) && !deep_inside(word.end_ms),
            "{word:?} is inside the pause {pause:?}"
        );
        let overlap = word
            .end_ms
            .min(pause.end_ms)
            .saturating_sub(word.start_ms.max(pause.start_ms));
        assert!(
            overlap <= PAUSE_MARGIN_MS,
            "{word:?} spans the pause {pause:?}"
        );
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
#[ignore = "needs a whisper model in REELCTL_WHISPER_MODEL and macOS `say`"]
fn a_stretch_of_tone_is_transcribed_again_and_stays_empty() {
    let model = std::env::var_os("REELCTL_WHISPER_MODEL")
        .expect("set REELCTL_WHISPER_MODEL to a ggml whisper model");
    let dir = temp_dir("tonespeech");
    let input = dir.join("tonespeech.mp4");
    let out = dir.join("tonespeech.edit_dna.json");
    generate("tonespeech", &input);
    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&input)
        .arg("--out")
        .arg(&out)
        .arg("--whisper-model")
        .arg(&model)
        .output()
        .expect("spawn reelctl");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dna: EditDna = serde_json::from_slice(&fs::read(&out).expect("read json")).expect("parse");

    // The 25 s tone has sound and no words: one hole, re-run, nothing found.
    assert!(
        dna.asr_note.contains("left 1 stretches") && dna.asr_note.contains("found 0 words"),
        "{}",
        dna.asr_note
    );
    assert!(
        dna.words.iter().all(|word| word.start_ms >= 24_000),
        "a word inside the tone: {:?}",
        dna.words
    );
    let texts: Vec<String> = dna
        .words
        .iter()
        .map(|word| {
            word.text
                .chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .collect();
    let mut remaining = texts.iter();
    for key in ["dead", "air", "proof", "viewer", "scrolls"] {
        assert!(
            remaining.any(|text| text == key),
            "{key:?} missing or out of order in {texts:?}"
        );
    }
    fs::remove_dir_all(&dir).ok();
}
