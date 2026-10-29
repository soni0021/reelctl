//! `reelctl compositor isolate-presenter`: every way the setup can be wrong, with no
//! model and no ONNX Runtime. Each failure must exit with the documented code, name
//! the file or variable, and leave no output. The run with a fake model is in
//! `crates/compositor/tests/ffmpeg_pipeline.rs`; a run with the real model is a
//! manual command (docs/status.md). Needs the `ffmpeg` and `ffprobe` command line tools.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

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

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("reelctl-compositor-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn make_video(path: &Path, extra: &[&str]) {
    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-nostdin"])
        .args(["-f", "lavfi", "-i", "testsrc=s=64x48:r=25:d=1"])
        .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
        .args(extra)
        .arg(path)
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A valid input video, a stand-in for weights (not a model), and an output path.
struct Setup {
    dir: PathBuf,
    input: PathBuf,
    weights: PathBuf,
    out: PathBuf,
}

fn setup(name: &str) -> Setup {
    let dir = scratch(name);
    let input = dir.join("in.mp4");
    make_video(&input, &[]);
    let weights = dir.join("rvm.onnx");
    fs::write(&weights, b"not a model").unwrap();
    let out = dir.join("out.mp4");
    Setup {
        dir,
        input,
        weights,
        out,
    }
}

impl Setup {
    fn args(&self) -> Vec<String> {
        [
            "compositor",
            "isolate-presenter",
            "--input",
            self.input.to_str().unwrap(),
            "--background",
            "#00ff00",
            "--weights",
            self.weights.to_str().unwrap(),
            "--out",
            self.out.to_str().unwrap(),
        ]
        .map(String::from)
        .to_vec()
    }

    /// The default arguments with `key`'s value replaced.
    fn with(&self, key: &str, value: &str) -> Vec<String> {
        let mut args = self.args();
        let at = args.iter().position(|arg| arg == key).expect("known flag");
        args[at + 1] = value.to_string();
        args
    }

    /// Run `args` with `library` as `ORT_DYLIB_PATH`, if any.
    fn run(&self, args: &[String], library: Option<&str>) -> Output {
        let mut command = reelctl();
        command.args(args);
        if let Some(library) = library {
            command.env("ORT_DYLIB_PATH", library);
        }
        command.output().expect("spawn reelctl")
    }

    /// Expect exit `code`, a message containing `needle`, and no output file.
    fn fails(&self, args: &[String], library: Option<&str>, code: i32, needle: &str) {
        let output = self.run(args, library);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(code), "{args:?}: {stderr}");
        assert!(stderr.contains("error:"), "{stderr}");
        assert!(stderr.contains(needle), "wanted {needle:?} in {stderr}");
        assert!(!self.out.exists(), "an output was written");
        let hidden: Vec<_> = fs::read_dir(&self.dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(hidden.is_empty(), "a temporary file was left: {hidden:?}");
    }
}

#[test]
fn the_command_is_listed_in_the_help() {
    let top = reelctl().arg("--help").output().unwrap();
    assert!(String::from_utf8_lossy(&top.stdout).contains("compositor"));
    let help = reelctl()
        .args(["compositor", "isolate-presenter", "--help"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&help.stdout);
    for flag in [
        "--input",
        "--background",
        "--weights",
        "--out",
        "--mode",
        "--downsample-ratio",
    ] {
        assert!(text.contains(flag), "{flag} missing from {text}");
    }
    assert!(text.contains("REELCTL_RVM_WEIGHTS"));
}

#[test]
fn no_weights_exit_2_naming_the_flag_and_the_variable() {
    let s = setup("noweights");
    let mut args = s.args();
    let at = args.iter().position(|arg| arg == "--weights").unwrap();
    args.drain(at..at + 2);
    s.fails(&args, None, 2, "--weights");
    s.fails(&args, None, 2, "REELCTL_RVM_WEIGHTS");
}

#[test]
fn weights_from_the_environment_are_read() {
    let s = setup("envweights");
    let mut args = s.args();
    let at = args.iter().position(|arg| arg == "--weights").unwrap();
    args.drain(at..at + 2);
    let mut command = reelctl();
    command.args(&args).env("REELCTL_RVM_WEIGHTS", &s.weights);
    let output = command.output().unwrap();
    // The weights were found; the library is what is missing.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("ORT_DYLIB_PATH is not set"), "{stderr}");
}

#[test]
fn weights_that_are_a_directory_or_missing_exit_2() {
    let s = setup("badweights");
    s.fails(
        &s.with("--weights", s.dir.to_str().unwrap()),
        None,
        2,
        "is a directory",
    );
    let missing = s.dir.join("nope.onnx");
    s.fails(
        &s.with("--weights", missing.to_str().unwrap()),
        None,
        2,
        "were not found",
    );
}

#[test]
fn a_missing_or_unusable_onnx_runtime_exits_2_naming_the_variable() {
    let s = setup("noruntime");
    let args = s.args();
    s.fails(&args, None, 2, "ORT_DYLIB_PATH is not set");
    s.fails(&args, Some(""), 2, "ORT_DYLIB_PATH is empty");
    s.fails(
        &args,
        Some("lib/libonnxruntime.dylib"),
        2,
        "not an absolute path",
    );
    s.fails(
        &args,
        Some("/no/such/libonnxruntime.dylib"),
        2,
        "which is not a file",
    );
    // A file that exists but is not a library: ONNX Runtime fails to load, no panic.
    let fake = s.dir.join("libonnxruntime.dylib");
    fs::write(&fake, b"not a library").unwrap();
    s.fails(&args, Some(fake.to_str().unwrap()), 2, "ONNX Runtime");
}

#[test]
fn a_bad_background_exits_2() {
    let s = setup("badbackground");
    s.fails(&s.with("--background", "#12345"), None, 2, "#rrggbb");
    s.fails(
        &s.with("--background", "red"),
        None,
        2,
        "neither a #rrggbb colour",
    );
    let missing = s.dir.join("nope.png");
    s.fails(
        &s.with("--background", missing.to_str().unwrap()),
        None,
        2,
        "neither a #rrggbb colour",
    );
    // A file that is not a video or image.
    let text = s.dir.join("bg.txt");
    fs::write(&text, b"hello").unwrap();
    s.fails(
        &s.with("--background", text.to_str().unwrap()),
        None,
        2,
        "ffprobe cannot read",
    );
}

#[test]
fn a_missing_or_unreadable_input_exits_2() {
    let s = setup("badinput");
    let missing = s.dir.join("nope.mp4");
    s.fails(
        &s.with("--input", missing.to_str().unwrap()),
        None,
        2,
        "--input",
    );
    let text = s.dir.join("in.txt");
    fs::write(&text, b"hello").unwrap();
    s.fails(
        &s.with("--input", text.to_str().unwrap()),
        None,
        2,
        "ffprobe cannot read",
    );
}

#[test]
fn hdr_input_is_refused_with_a_hint() {
    let s = setup("hdr");
    make_video(
        &s.input,
        &[
            // The filter sets the frame properties; the options write them to the stream.
            "-vf",
            "setparams=color_trc=arib-std-b67:colorspace=bt2020nc:color_primaries=bt2020",
            "-color_trc",
            "arib-std-b67",
            "-colorspace",
            "bt2020nc",
            "-color_primaries",
            "bt2020",
        ],
    );
    s.fails(&s.args(), None, 2, "HDR");
}

#[test]
fn a_bad_output_or_mode_exits_2() {
    let s = setup("badout");
    s.fails(
        &s.with("--out", s.dir.to_str().unwrap()),
        None,
        2,
        "is a directory",
    );
    let mut rgba = s.args();
    rgba.extend(["--mode", "rgba"].map(String::from));
    // rgba takes no background and needs a .mov.
    let at = rgba.iter().position(|arg| arg == "--background").unwrap();
    rgba.drain(at..at + 2);
    s.fails(&rgba, None, 2, ".mov");
    // Composite without a background.
    let mut none = s.args();
    let at = none.iter().position(|arg| arg == "--background").unwrap();
    none.drain(at..at + 2);
    s.fails(&none, None, 2, "--background is required");
    // A matte takes none.
    let mut matte = s.args();
    matte.extend(["--mode", "matte"].map(String::from));
    s.fails(&matte, None, 2, "not used with");
}

#[test]
fn a_bad_ratio_exits_2() {
    let s = setup("badratio");
    for ratio in ["0", "1.5", "-1"] {
        let mut args = s.args();
        args.push(format!("--downsample-ratio={ratio}"));
        s.fails(&args, None, 2, "--downsample-ratio");
    }
}

#[test]
fn an_unknown_mode_is_a_usage_error() {
    let s = setup("badmode");
    let mut args = s.args();
    args.extend(["--mode", "sepia"].map(String::from));
    let output = s.run(&args, None);
    assert_eq!(output.status.code(), Some(2));
    assert!(!s.out.exists());
}
