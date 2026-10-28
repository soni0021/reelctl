//! End to end: `reelctl timeline|transform|funnel|render|qa` on synthetic fixtures.
//! Needs the `ffmpeg` and `ffprobe` command line tools (as `fixtures/generate.sh` does).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use edit_dna::EditDna;
use serde_json::Value;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model and weight paths a shell profile may export.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("OPENROUTER_API_KEY");
    command
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reelctl-timeline-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
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

fn run(args: &[&str]) -> Output {
    reelctl().args(args).output().expect("spawn reelctl")
}

/// Run and require success; returns stdout.
fn ok(args: &[&str]) -> String {
    let output = run(args);
    assert!(
        output.status.success(),
        "reelctl {args:?} failed ({:?}): {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf8")
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json")
}

fn decompile(video: &Path, dna: &Path) -> EditDna {
    ok(&[
        "decompile",
        "--input",
        &s(video),
        "--out",
        &s(dna),
        "--filmstrip-dir",
        &s(&dna.with_extension("views")),
    ]);
    serde_json::from_str(&fs::read_to_string(dna).expect("read")).expect("dna")
}

/// `(video seconds, audio seconds, video frames)` of a rendered file.
fn probe(path: &Path) -> (f64, f64, u64) {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-count_frames",
            "-show_entries",
            "stream=codec_type,duration,nb_read_frames",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .expect("spawn ffprobe");
    assert!(output.status.success(), "ffprobe failed");
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    let streams = json["streams"].as_array().expect("streams");
    let mut video = (0.0, 0);
    let mut audio = 0.0;
    for stream in streams {
        let duration: f64 = stream["duration"]
            .as_str()
            .and_then(|text| text.parse().ok())
            .unwrap_or(0.0);
        match stream["codec_type"].as_str() {
            Some("video") => {
                let frames: u64 = stream["nb_read_frames"]
                    .as_str()
                    .and_then(|text| text.parse().ok())
                    .unwrap_or(0);
                video = (duration, frames);
            }
            Some("audio") => audio = duration,
            _ => {}
        }
    }
    (video.0, audio, video.1)
}

/// Mono 48 kHz f32 samples of the first audio stream.
fn pcm(path: &Path) -> Vec<f32> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-vn", "-f", "f32le", "-ac", "1", "-ar", "48000", "-"])
        .output()
        .expect("spawn ffmpeg");
    assert!(output.status.success(), "ffmpeg pcm failed");
    output
        .stdout
        .as_chunks::<4>()
        .0
        .iter()
        .map(|bytes| f32::from_le_bytes(*bytes))
        .collect()
}

/// One frame as raw pixels. `seek` is `["-ss", "7.5"]` or `["-sseof", "-0.1"]`.
fn frame(path: &Path, seek: [&str; 2], filter: Option<&str>, pix_fmt: &str) -> Vec<u8> {
    let mut command = Command::new("ffmpeg");
    command.args(["-v", "error"]).args(seek).arg("-i").arg(path);
    command.args(["-frames:v", "1"]);
    if let Some(filter) = filter {
        command.args(["-vf", filter]);
    }
    let output = command
        .args(["-f", "rawvideo", "-pix_fmt", pix_fmt, "-"])
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "frame extraction failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "frames differ in size");
    let total: u64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    total as f64 / a.len() as f64
}

fn pixel(rgb: &[u8], width: usize, x: usize, y: usize) -> [u8; 3] {
    let at = (y * width + x) * 3;
    [rgb[at], rgb[at + 1], rgb[at + 2]]
}

#[test]
fn cut_silence_then_render_removes_the_pause_without_a_click() {
    let root = scratch("cut");
    let media = root.join("draft.mp4");
    generate("draft12", &media);
    let dna_path = root.join("draft.edit_dna.json");
    let dna = decompile(&media, &dna_path);
    let pause = dna
        .silence_gaps
        .iter()
        .find(|gap| gap.duration_ms >= 180)
        .copied()
        .expect("the planted pause");
    assert!(
        (1_900..=2_100).contains(&pause.start_ms),
        "the pause starts near 2.0 s: {pause:?}"
    );

    let project = root.join("edit.otio");
    ok(&[
        "timeline",
        "init",
        "--input",
        &s(&media),
        "--dna",
        &s(&dna_path),
        "--out",
        &s(&project),
    ]);
    ok(&[
        "timeline",
        "cut-silence",
        "--project",
        &s(&project),
        "--dna",
        &s(&dna_path),
        "--out",
        &s(&project),
    ]);
    let cuts = read_json(&project)["metadata"]["reelctl"]["cuts"].clone();
    let cuts = cuts.as_array().expect("cuts");
    assert_eq!(cuts.len(), 1, "{cuts:?}");
    let first = cuts[0]["start_frame"].as_i64().expect("start");
    let last = cuts[0]["end_frame"].as_i64().expect("end");
    let removed_frames = last - first;
    // The removed span is the DNA's pause, shrunk by at most a frame at each end.
    let removed_ms = removed_frames as f64 * 1000.0 / 30.0;
    assert!(
        removed_ms <= pause.duration_ms as f64 + 1.0
            && removed_ms >= pause.duration_ms as f64 - 2.0 * 1000.0 / 30.0 - 1.0,
        "removed {removed_ms} ms of a {} ms pause",
        pause.duration_ms
    );

    let out = root.join("cut.mp4");
    let started = std::time::Instant::now();
    ok(&["render", "--project", &s(&project), "--out", &s(&out)]);
    eprintln!("render of draft12 with a cut: {:?}", started.elapsed());

    let (video_s, audio_s, frames) = probe(&out);
    eprintln!("render: video {video_s} s, audio {audio_s} s, {frames} frames");
    let source_frames = 360;
    assert_eq!(frames, source_frames - removed_frames as u64, "frame count");
    let expected_s = frames as f64 / 30.0;
    assert!(
        (video_s - expected_s).abs() <= 1.0 / 30.0,
        "video {video_s} s, expected {expected_s} s"
    );
    assert!(
        (audio_s - video_s).abs() <= 0.05,
        "audio {audio_s} s and video {video_s} s differ"
    );
    // Shorter than the source by the removed pause, within a frame.
    assert!(
        (12.0 - video_s - removed_ms / 1000.0).abs() <= 1.0 / 30.0,
        "shorter by {} s, pause removed {} s",
        12.0 - video_s,
        removed_ms / 1000.0
    );

    // No click: the largest step between neighbouring samples within 10 ms of the join.
    let samples = pcm(&out);
    let join = (first as f64 / 30.0 * 48_000.0) as usize;
    let window = &samples[join - 480..join + 480];
    let step = window
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0_f32, f32::max);
    assert!(step < 0.02, "a step of {step} at the join is a click");
    // The tone is still there on both sides of the join.
    let peak = |range: std::ops::Range<usize>| {
        samples[range]
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max)
    };
    assert!(peak(join - 4_800..join - 960) > 0.05);
    assert!(peak(join + 4_800..join + 9_600) > 0.05);

    // Decompiling the render finds no pause of 180 ms or more where the pause was.
    let rendered = decompile(&out, &root.join("cut.edit_dna.json"));
    let join_ms = first as f64 * 1000.0 / 30.0;
    for gap in &rendered.silence_gaps {
        let overlaps =
            (gap.end_ms as f64) > join_ms - 500.0 && (gap.start_ms as f64) < join_ms + 500.0;
        assert!(
            !(overlaps && gap.duration_ms >= 180),
            "a {} ms gap remains at {}..{} ms (join at {join_ms} ms)",
            gap.duration_ms,
            gap.start_ms,
            gap.end_ms
        );
    }

    // Running it again changes nothing.
    let again = root.join("again.otio");
    ok(&[
        "timeline",
        "cut-silence",
        "--project",
        &s(&project),
        "--dna",
        &s(&dna_path),
        "--out",
        &s(&again),
    ]);
    assert_eq!(
        fs::read(&project).expect("read"),
        fs::read(&again).expect("read")
    );

    // One join, one filmstrip.
    let views = root.join("views");
    ok(&[
        "qa",
        "inspect-boundaries",
        "--project",
        &s(&project),
        "--out-dir",
        &s(&views),
    ]);
    let pngs: Vec<PathBuf> = fs::read_dir(&views)
        .expect("views dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "png"))
        .collect();
    assert_eq!(pngs.len(), 1, "{pngs:?}");
    let image = image::open(&pngs[0]).expect("png");
    assert!(image.width() > 100 && image.height() > 50);
}

#[test]
fn a_punch_in_zooms_the_footage_by_the_scale() {
    let root = scratch("punch");
    let media = root.join("textured.mp4");
    // 24 fps, 12 s; shot 3 (6 to 9 s) is a test pattern with sharp detail.
    generate("textured_cuts12", &media);
    let project = root.join("edit.otio");
    ok(&[
        "timeline",
        "init",
        "--input",
        &s(&media),
        "--out",
        &s(&project),
    ]);
    ok(&[
        "transform",
        "punch-in",
        "--project",
        &s(&project),
        "--start-ms",
        "7000",
        "--end-ms",
        "8000",
        "--scale",
        "1.5",
        "--out",
        &s(&project),
    ]);
    let out = root.join("punch.mp4");
    let started = std::time::Instant::now();
    ok(&["render", "--project", &s(&project), "--out", &s(&out)]);
    eprintln!(
        "render of textured_cuts12 with a punch-in: {:?}",
        started.elapsed()
    );

    let (_, _, frames) = probe(&out);
    assert_eq!(frames, 288, "a punch-in does not change the frame count");

    let seek = ["-ss", "7.5"];
    let zoomed = frame(&out, seek, Some("format=gray"), "gray");
    let source = frame(&media, seek, Some("format=gray"), "gray");
    let reference = frame(
        &media,
        seek,
        Some("scale=960:540:flags=bicubic,crop=640:360,format=gray"),
        "gray",
    );
    let to_reference = mean_abs_diff(&zoomed, &reference);
    let to_source = mean_abs_diff(&zoomed, &source);
    eprintln!("zoomed vs source*1.5: {to_reference:.2}, zoomed vs source: {to_source:.2}");
    assert!(
        to_reference < 8.0,
        "does not match the source scaled by 1.5: {to_reference}"
    );
    assert!(
        to_source > 3.0 * to_reference,
        "looks like the unzoomed source: {to_source} vs {to_reference}"
    );

    // Outside the span the render is the source (within the encode).
    for time in ["6.5", "8.5"] {
        let seek = ["-ss", time];
        let rendered = frame(&out, seek, Some("format=gray"), "gray");
        let plain = frame(&media, seek, Some("format=gray"), "gray");
        let diff = mean_abs_diff(&rendered, &plain);
        assert!(
            diff < 4.0,
            "frame at {time} s differs from the source by {diff}"
        );
    }
}

#[test]
fn a_bridge_ends_the_render_on_the_card() {
    let root = scratch("bridge");
    let media = root.join("draft.mp4");
    generate("draft12", &media);
    let project = root.join("edit.otio");
    ok(&[
        "timeline",
        "init",
        "--input",
        &s(&media),
        "--out",
        &s(&project),
    ]);
    ok(&[
        "funnel",
        "attach-bridge",
        "--project",
        &s(&project),
        "--target-url",
        "example.com/go",
        "--out",
        &s(&project),
    ]);
    let out = root.join("bridge.mp4");
    ok(&["render", "--project", &s(&project), "--out", &s(&out)]);

    // 72 % of 360 frames is 259; the card is 2.5 s = 75 frames.
    let (video_s, _, frames) = probe(&out);
    assert_eq!(frames, 259 + 75);
    assert!((video_s - 334.0 / 30.0).abs() <= 1.0 / 30.0, "{video_s}");

    let (w, h) = (640, 360);
    let last = frame(&out, ["-sseof", "-0.1"], None, "rgb24");
    assert_eq!(last.len(), w * h * 3);
    let source_last = frame(&media, ["-sseof", "-0.1"], None, "rgb24");
    assert!(
        mean_abs_diff(&last, &source_last) > 40.0,
        "the last frame is the source's"
    );
    // The card's colour in the corner (58, 28, 113), within the encode.
    let corner = pixel(&last, w, 4, 4);
    for (got, want) in corner.iter().zip([58u8, 28, 113]) {
        assert!(got.abs_diff(want) <= 12, "corner {corner:?}");
    }
    // The URL is drawn in the foreground colour near the centre.
    let mut light = 0;
    for y in h / 3..2 * h / 3 {
        for x in w / 8..7 * w / 8 {
            let p = pixel(&last, w, x, y);
            if p.iter().all(|&c| c > 200) {
                light += 1;
            }
        }
    }
    assert!(
        light > 200,
        "only {light} light pixels where the URL should be"
    );
    // Just before the cut the footage still plays (draft12's second half is white).
    let before = frame(&out, ["-ss", "8.5"], None, "rgb24");
    assert!(pixel(&before, w, 4, 4).iter().all(|&c| c > 200));
}

#[test]
fn every_command_is_idempotent() {
    let root = scratch("idempotent");
    let media = root.join("draft.mp4");
    generate("draft12", &media);
    let dna_path = root.join("draft.edit_dna.json");
    decompile(&media, &dna_path);
    let a = root.join("a.otio");
    let b = root.join("b.otio");
    let init = |out: &Path| {
        ok(&[
            "timeline",
            "init",
            "--input",
            &s(&media),
            "--dna",
            &s(&dna_path),
            "--out",
            &s(out),
        ])
    };
    init(&a);
    init(&b);
    assert_eq!(fs::read(&a).expect("read"), fs::read(&b).expect("read"));

    // Each edit applied twice to the same file equals applying it once.
    let steps: [Vec<String>; 3] = [
        vec![
            "timeline".into(),
            "cut-silence".into(),
            "--dna".into(),
            s(&dna_path),
        ],
        vec![
            "transform".into(),
            "punch-in".into(),
            "--start-ms".into(),
            "4000".into(),
            "--end-ms".into(),
            "5000".into(),
            "--scale".into(),
            "1.18".into(),
        ],
        vec![
            "funnel".into(),
            "attach-bridge".into(),
            "--target-url".into(),
            "example.com/go".into(),
        ],
    ];
    for step in &steps {
        let apply = |project: &Path| {
            let mut args: Vec<String> = step.clone();
            args.extend(["--project".into(), s(project), "--out".into(), s(project)]);
            let refs: Vec<&str> = args.iter().map(String::as_str).collect();
            ok(&refs);
        };
        apply(&a);
        let once = fs::read(&a).expect("read");
        apply(&a);
        assert_eq!(once, fs::read(&a).expect("read"), "{step:?} twice");
    }
}

#[test]
fn bad_arguments_and_projects_exit_2_and_name_the_culprit() {
    let root = scratch("errors");
    let media = root.join("draft.mp4");
    generate("draft12", &media);
    let project = root.join("edit.otio");
    ok(&[
        "timeline",
        "init",
        "--input",
        &s(&media),
        "--out",
        &s(&project),
    ]);
    let before = fs::read(&project).expect("read");

    let punch = |extra: &[&str]| {
        let mut args = vec!["transform", "punch-in", "--project"];
        let path = s(&project);
        args.push(&path);
        args.extend(extra);
        args.extend(["--out", &path]);
        let owned: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
        run(&refs)
    };
    let expect_2 = |output: Output, needle: &str| {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert_eq!(output.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains(needle), "{needle:?} not in {stderr}");
    };
    expect_2(
        punch(&["--start-ms", "1000", "--end-ms", "3000", "--scale", "1.0"]),
        "--scale",
    );
    expect_2(
        punch(&["--start-ms", "1000", "--end-ms", "3000", "--scale", "2.5"]),
        "--scale",
    );
    expect_2(
        punch(&["--start-ms", "11000", "--end-ms", "14000", "--scale", "1.2"]),
        "14000",
    );
    expect_2(
        punch(&["--start-ms", "1000", "--end-ms", "1100", "--scale", "1.2"]),
        "frames",
    );
    expect_2(
        punch(&["--start-ms", "x", "--end-ms", "1100", "--scale", "1.2"]),
        "--start-ms",
    );
    // A bad project exits 2 and names the file; the output file is not touched.
    let missing = root.join("nope.otio");
    expect_2(
        run(&[
            "render",
            "--project",
            &s(&missing),
            "--out",
            &s(&root.join("x.mp4")),
        ]),
        "nope.otio",
    );
    let broken = root.join("broken.otio");
    fs::write(&broken, "{\"OTIO_SCHEMA\": \"Clip.2\"}").expect("write");
    expect_2(
        run(&[
            "transform",
            "punch-in",
            "--project",
            &s(&broken),
            "--start-ms",
            "0",
            "--end-ms",
            "1000",
            "--scale",
            "1.2",
            "--out",
            &s(&project),
        ]),
        "broken.otio",
    );
    assert_eq!(
        before,
        fs::read(&project).expect("read"),
        "a failed run wrote"
    );
    expect_2(
        run(&[
            "funnel",
            "attach-bridge",
            "--project",
            &s(&project),
            "--target-url",
            "a.co",
            "--cutoff-pct",
            "1.5",
            "--out",
            &s(&project),
        ]),
        "--cutoff-pct",
    );
    // A DNA of another file is refused.
    let other = root.join("other.edit_dna.json");
    let mut dna = EditDna::empty(30_000, 30.0, 640, 360);
    dna.silence_gaps = Vec::new();
    fs::write(&other, serde_json::to_string(&dna).expect("json")).expect("write");
    expect_2(
        run(&[
            "timeline",
            "cut-silence",
            "--project",
            &s(&project),
            "--dna",
            &s(&other),
            "--out",
            &s(&project),
        ]),
        "30000",
    );
    // An unknown flag is clap's exit 2 too.
    assert_eq!(run(&["render", "--nonsense"]).status.code(), Some(2));
    // No leftover temporary files next to the project.
    let leftovers: Vec<String> = fs::read_dir(&root)
        .expect("dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.contains(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn render_fails_clearly_without_ffmpeg() {
    let root = scratch("noffmpeg");
    let media = root.join("draft.mp4");
    generate("draft12", &media);
    let project = root.join("edit.otio");
    ok(&[
        "timeline",
        "init",
        "--input",
        &s(&media),
        "--out",
        &s(&project),
    ]);
    let output = reelctl()
        .env("PATH", "")
        .args(["render", "--project"])
        .arg(&project)
        .arg("--out")
        .arg(root.join("out.mp4"))
        .output()
        .expect("spawn reelctl");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("ffmpeg"), "{stderr}");
    assert!(stderr.contains("PATH"), "{stderr}");
    assert!(!root.join("out.mp4").exists());
}
