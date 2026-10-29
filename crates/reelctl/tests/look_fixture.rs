//! `reelctl look` end to end: a textured clip and a copy graded by ffmpeg to a known
//! look. `ffmpeg` is used here to apply the LUT (`lut3d`) and to read frames back,
//! never by the program. No network, no model.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use look::Cube;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model, weight, and API key variables a shell may export.
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
    let root = std::env::temp_dir().join(format!("reelctl-look-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("temp dir");
    root
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
        "fixture {kind} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `still12` (neutral) and `graded12` (the same picture, graded), made once.
fn fixtures() -> &'static (PathBuf, PathBuf) {
    static CLIPS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    CLIPS.get_or_init(|| {
        let dir = scratch("fixtures");
        let plain = dir.join("still12.mp4");
        let graded = dir.join("graded12.mp4");
        generate("still12", &plain);
        generate("graded12", &graded);
        (plain, graded)
    })
}

fn look(draft: &Path, outlier: &Path, out: &Path, extra: &[&str]) -> Output {
    reelctl()
        .arg("look")
        .arg("--draft")
        .arg(draft)
        .arg("--outlier")
        .arg(outlier)
        .arg("--out")
        .arg(out)
        .args(extra)
        .output()
        .expect("run reelctl look")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The value of `key=` in the `look` line.
fn field<'a>(line: &'a str, key: &str) -> &'a str {
    line.split(' ')
        .find_map(|part| part.strip_prefix(&format!("{key}=")))
        .unwrap_or_else(|| panic!("no {key}= in {line:?}"))
}

/// Frames of `clip` every half second as 320-wide RGB, after converting to RGB and
/// (when given) applying a cube: `ffmpeg -vf lut3d`.
fn frames(clip: &Path, cube: Option<&Path>) -> Vec<u8> {
    let lut = match cube {
        Some(path) => format!("lut3d=file={},", path.display()),
        None => String::new(),
    };
    let filter = format!("format=rgb24,{lut}fps=2,scale=320:-2:flags=area,format=rgb24");
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(clip)
        .args(["-vf", &filter, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.stdout.is_empty());
    output.stdout
}

/// Mean absolute difference per channel, on a 0 to 255 scale.
fn mae(a: &[u8], b: &[u8]) -> [f64; 3] {
    assert_eq!(a.len(), b.len(), "different frame counts or sizes");
    let mut sum = [0u64; 3];
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        sum[i % 3] += u64::from(x.abs_diff(*y));
    }
    let pixels = (a.len() / 3) as f64;
    sum.map(|s| s as f64 / pixels)
}

fn mean(a: &[u8]) -> [f64; 3] {
    let mut sum = [0u64; 3];
    for (i, x) in a.iter().enumerate() {
        sum[i % 3] += u64::from(*x);
    }
    let pixels = (a.len() / 3) as f64;
    sum.map(|s| s as f64 / pixels)
}

/// The gap between two channels' cumulative distributions: the mean absolute
/// difference over the 256 levels (the earth mover's distance, as a share of the
/// range, 0 to 1), or with `largest` the biggest single gap (Kolmogorov-Smirnov),
/// which one heavily populated level can dominate.
fn cdf_distance(a: &[u8], b: &[u8], largest: bool) -> [f64; 3] {
    let cdfs = |buffer: &[u8]| {
        let mut counts = [[0u64; 256]; 3];
        for (i, x) in buffer.iter().enumerate() {
            counts[i % 3][usize::from(*x)] += 1;
        }
        let pixels = (buffer.len() / 3) as f64;
        counts.map(|channel| {
            let mut running = 0u64;
            channel.map(|n| {
                running += n;
                running as f64 / pixels
            })
        })
    };
    let (ca, cb) = (cdfs(a), cdfs(b));
    let mut out = [0.0; 3];
    for c in 0..3 {
        let gaps = ca[c].iter().zip(&cb[c]).map(|(x, y)| (x - y).abs());
        out[c] = if largest {
            gaps.fold(0.0, f64::max)
        } else {
            gaps.sum::<f64>() / 256.0
        };
    }
    out
}

#[test]
fn a_graded_clip_is_pulled_back_toward_the_plain_one_at_every_size() {
    let (plain, graded) = fixtures();
    let dir = scratch("quality");
    let reference = frames(plain, None);
    let before = frames(graded, None);
    let mae_before = mae(&before, &reference);
    let cdf_before = cdf_distance(&before, &reference, false);
    let ks_before = cdf_distance(&before, &reference, true);
    // The grade must be a real one, or the test proves nothing.
    assert!(mae_before.iter().all(|&e| e > 8.0), "{mae_before:?}");
    assert!(cdf_before.iter().all(|&d| d > 0.02), "{cdf_before:?}");

    for size in [17, 33, 65] {
        let cube = dir.join(format!("graded-to-plain-{size}.cube"));
        let output = look(graded, plain, &cube, &["--size", &size.to_string()]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let after = frames(graded, Some(&cube));
        let mae_after = mae(&after, &reference);
        let cdf_after = cdf_distance(&after, &reference, false);
        let ks_after = cdf_distance(&after, &reference, true);
        eprintln!(
            "size {size}: mae {mae_before:?} -> {mae_after:?}; cdf {cdf_before:?} -> {cdf_after:?}; largest gap {ks_before:?} -> {ks_after:?}"
        );
        for c in 0..3 {
            assert!(
                mae_after[c] <= 0.2 * mae_before[c],
                "size {size} channel {c}: error {} -> {}",
                mae_before[c],
                mae_after[c]
            );
            assert!(
                cdf_after[c] <= 0.1 * cdf_before[c],
                "size {size} channel {c}: cdf distance {} -> {}",
                cdf_before[c],
                cdf_after[c]
            );
        }
    }
}

#[test]
fn the_reported_shift_is_the_change_in_the_drafts_channel_means() {
    let (plain, graded) = fixtures();
    let dir = scratch("shift");
    let cube = dir.join("g2p.cube");
    let output = look(graded, plain, &cube, &[]);
    assert!(output.status.success());
    let text = stdout(&output);
    let line = text.trim_end();
    assert_eq!(text.lines().count(), 1, "{text:?}");
    assert!(
        line.starts_with("size=33 samples_draft=60 samples_outlier=60 mean_shift_r="),
        "{line}"
    );
    assert_eq!(field(line, "out"), cube.display().to_string());

    let before = mean(&frames(graded, None));
    let after = mean(&frames(graded, Some(&cube)));
    for (c, key) in ["mean_shift_r", "mean_shift_g", "mean_shift_b"]
        .iter()
        .enumerate()
    {
        let reported: f64 = field(line, key).parse().expect("a number");
        let measured = after[c] - before[c];
        assert!(
            (reported - measured).abs() < 1.5,
            "{key}: reported {reported}, ffmpeg measured {measured}"
        );
        assert!(
            reported.abs() > 3.0,
            "{key} {reported}: the grade moves every channel"
        );
    }
}

#[test]
fn identical_inputs_give_an_identity_cube() {
    let (plain, _) = fixtures();
    let dir = scratch("identity");
    let cube_path = dir.join("same.cube");
    let output = look(plain, plain, &cube_path, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let line = stdout(&output);
    for key in ["mean_shift_r", "mean_shift_g", "mean_shift_b"] {
        let shift: f64 = field(line.trim_end(), key).parse().expect("a number");
        assert!(shift.abs() < 0.01, "{key} {shift}");
    }
    let cube = Cube::parse(&fs::read_to_string(&cube_path).expect("cube")).expect("parse");
    let n = cube.size();
    assert_eq!(n, 33);
    let step = 1.0 / (n - 1) as f64;
    for (index, entry) in cube.entries().iter().enumerate() {
        let node = [index % n, index / n % n, index / (n * n)];
        for c in 0..3 {
            let expected = node[c] as f64 * step;
            assert!(
                (entry[c] - expected).abs() <= 1.0 / 255.0,
                "node {node:?} channel {c}: {} vs {expected}",
                entry[c]
            );
        }
    }
}

#[test]
fn ffmpeg_loads_the_file_and_it_names_its_makers() {
    let (plain, graded) = fixtures();
    let dir = scratch("load");
    let cube = dir.join("load.cube");
    assert!(look(graded, plain, &cube, &[]).status.success());
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=64x64:d=1",
            "-vf",
        ])
        .arg(format!("lut3d=file={}", cube.display()))
        .args(["-f", "null", "-"])
        .status()
        .expect("spawn ffmpeg");
    assert!(status.success(), "ffmpeg did not accept the cube");
    let text = fs::read_to_string(&cube).expect("cube");
    let title = text.lines().next().expect("a first line");
    assert!(title.starts_with("TITLE \""), "{title}");
    assert!(title.contains("reelctl look"), "{title}");
    assert!(
        title.contains("graded12.mp4") && title.contains("still12.mp4"),
        "{title}"
    );
    assert!(!title.contains('/'), "names only: {title}");
}

#[test]
fn two_runs_write_the_same_bytes() {
    let (plain, graded) = fixtures();
    let dir = scratch("determinism");
    let (a, b) = (dir.join("a.cube"), dir.join("b.cube"));
    assert!(look(graded, plain, &a, &["--samples", "20"])
        .status
        .success());
    assert!(look(graded, plain, &b, &["--samples", "20"])
        .status
        .success());
    assert_eq!(fs::read(&a).expect("a"), fs::read(&b).expect("b"));
    // The software decoder gives the same frames for H.264 as auto does.
    let c = dir.join("c.cube");
    assert!(look(
        graded,
        plain,
        &c,
        &["--samples", "20", "--decoder", "software"]
    )
    .status
    .success());
    assert_eq!(fs::read(&a).expect("a"), fs::read(&c).expect("c"));
}

#[test]
fn the_number_of_samples_is_reported() {
    let (plain, graded) = fixtures();
    let dir = scratch("samples");
    for samples in [1, 7, 600] {
        let cube = dir.join(format!("s{samples}.cube"));
        let output = look(graded, plain, &cube, &["--samples", &samples.to_string()]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = stdout(&output);
        assert_eq!(field(text.trim_end(), "samples_draft"), samples.to_string());
        assert_eq!(
            field(text.trim_end(), "samples_outlier"),
            samples.to_string()
        );
        assert!(cube.exists());
    }
}

#[test]
fn bad_arguments_and_unreadable_files_exit_2_before_decoding() {
    let (plain, _) = fixtures();
    let dir = scratch("usage");
    let out = dir.join("never.cube");
    let missing = dir.join("missing.mp4");
    let cases: Vec<(&str, Output)> = vec![
        ("missing draft", look(&missing, plain, &out, &[])),
        ("missing outlier", look(plain, &missing, &out, &[])),
        ("a directory", look(&dir, plain, &out, &[])),
        ("size 20", look(plain, plain, &out, &["--size", "20"])),
        ("size 0", look(plain, plain, &out, &["--size", "0"])),
        ("samples 0", look(plain, plain, &out, &["--samples", "0"])),
        (
            "samples 601",
            look(plain, plain, &out, &["--samples", "601"]),
        ),
        (
            "samples text",
            look(plain, plain, &out, &["--samples", "many"]),
        ),
        (
            "no --out",
            reelctl()
                .args(["look", "--draft"])
                .arg(plain)
                .arg("--outlier")
                .arg(plain)
                .output()
                .expect("run"),
        ),
    ];
    for (name, output) in cases {
        assert_eq!(output.status.code(), Some(2), "{name}");
        assert!(
            stdout(&output).is_empty(),
            "{name} printed {:?}",
            stdout(&output)
        );
        assert!(
            !String::from_utf8_lossy(&output.stderr).is_empty(),
            "{name}"
        );
    }
    assert!(!out.exists(), "a failed run wrote a cube");
}

#[test]
fn a_file_that_is_not_a_video_exits_1_and_writes_nothing() {
    let (plain, _) = fixtures();
    let dir = scratch("notvideo");
    let text = dir.join("notes.mp4");
    fs::write(&text, "this is not a video\n").expect("write");
    let out = dir.join("never.cube");
    for (draft, outlier) in [(&text, plain), (plain, &text)] {
        let output = look(draft, outlier, &out, &[]);
        assert_eq!(output.status.code(), Some(1));
        assert!(stdout(&output).is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).is_empty());
    }
    assert!(!out.exists());
}
