//! The `make-reels` steps one by one, end to end: `timeline from-clip`,
//! `transform auto-punch-in` and `funnel attach-bridge --append` on the
//! `lecture70` fixture, with an `understanding.json` built by the clip rules over
//! synthetic words (no model, no network).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use edit_dna::EditDna;
use serde_json::{json, Value};
use understand::{
    build_clips, segment_words, ClipPick, ModelRun, RunSettings, Source, Summary, Understanding,
};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model, weight and key variables a shell may export.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("REELCTL_RVM_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
        .env_remove("MANIMGX_BIN")
        .env_remove("SMELTER_BIN")
        .env_remove("OPENROUTER_API_KEY");
    command
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reelctl-steps-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn run(args: &[&str]) -> Output {
    reelctl().args(args).output().expect("spawn reelctl")
}

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

fn exits_2(args: &[&str], needle: &str) {
    let output = run(args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
    assert!(stderr.contains(needle), "{args:?}: {stderr}");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json")
}

/// The decompiled lecture with 20 synthetic sentences in it, and an understanding
/// with one clip from sentence 1 to 13 (hook 1, payoff 3). Made once.
struct Lecture {
    video: PathBuf,
    dna: PathBuf,
    understanding: PathBuf,
}

fn lecture() -> &'static Lecture {
    static LECTURE: OnceLock<Lecture> = OnceLock::new();
    LECTURE.get_or_init(|| {
        let root = scratch("lecture");
        let video = root.join("lecture.mp4");
        let generated = Command::new("bash")
            .arg(repo_root().join("fixtures/generate.sh"))
            .args(["lecture70", &s(&video)])
            .output()
            .expect("spawn fixture generator");
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let dna = root.join("lecture.edit_dna.json");
        ok(&[
            "decompile",
            "--input",
            &s(&video),
            "--out",
            &s(&dna),
            "--filmstrip-dir",
            &s(&root.join("views")),
        ]);
        give_words(&dna);
        let understanding = root.join("understanding.json");
        write_understanding(&dna, &s(&video), &understanding);
        Lecture {
            video,
            dna,
            understanding,
        }
    })
}

/// 20 sentences of 3.2 s every 3.5 s from 1 s, with 300 ms pauses between them,
/// written into the DNA as if whisper had heard them; and a 1 s pause at 20 s.
fn give_words(dna: &Path) {
    let mut value = read_json(dna);
    let mut words = Vec::new();
    let mut gaps = Vec::new();
    for n in 0..20u64 {
        let start = 1_000 + 3_500 * n;
        let end = start + 3_200;
        let step = (end - start) / 5;
        for (index, text) in ["Sentence", &n.to_string(), "is", "about", "topic."]
            .iter()
            .enumerate()
        {
            let from = start + step * index as u64;
            words.push(json!({"text": text, "start_ms": from, "end_ms": from + step}));
        }
        if n < 19 {
            gaps.push(json!({"start_ms": end, "end_ms": end + 300, "duration_ms": 300}));
        }
    }
    value["words"] = json!(words);
    value["silence_gaps"] = json!(gaps);
    value["asr_backend"] = json!("whisper_cpp");
    fs::write(dna, serde_json::to_vec_pretty(&value).expect("json")).expect("write");
}

fn write_understanding(dna_path: &Path, source_path: &str, out: &Path) {
    let dna: EditDna = serde_json::from_slice(&fs::read(dna_path).expect("dna")).expect("dna");
    let segments = segment_words(&dna.words, &dna.silence_gaps);
    let pick = ClipPick {
        start_segment: 1,
        end_segment: 13,
        hook_segment: 1,
        payoff_segment: 3,
        cutoff_segment: 13,
        title: "Clip".to_string(),
        open_loop: "".to_string(),
        visual_suggestion: "".to_string(),
        why: "".to_string(),
    };
    let outcome = build_clips(&[pick], &segments, &dna.silence_gaps, dna.duration_ms);
    let run = Understanding {
        schema_version: understand::SCHEMA_VERSION,
        source: Source {
            path: source_path.to_string(),
            duration_ms: dna.duration_ms,
        },
        model: ModelRun {
            id: "replay".to_string(),
            provider: "replay".to_string(),
            quantization: None,
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: None,
            cost_upper_bound_usd: 0.0,
            elapsed_ms: 1,
        },
        settings: RunSettings {
            every_s: 5,
            sheet: "single".to_string(),
            segment_pause_ms: 400,
            quant: Vec::new(),
        },
        segments,
        summary: Summary {
            topic: "topic".to_string(),
            language: "en".to_string(),
            speaker_count: 1,
            visual_formats: Vec::new(),
        },
        beats: Vec::new(),
        visual_events: Vec::new(),
        cuts: Vec::new(),
        clips: outcome.clips,
        rejected: outcome.rejected,
    };
    fs::write(out, serde_json::to_vec_pretty(&run).expect("json")).expect("write");
}

fn from_clip(lecture: &Lecture, out: &Path, extra: &[&str]) -> Output {
    let mut args = vec![
        "timeline".to_string(),
        "from-clip".to_string(),
        "--input".to_string(),
        s(&lecture.video),
        "--dna".to_string(),
        s(&lecture.dna),
        "--understanding".to_string(),
        s(&lecture.understanding),
        "--out".to_string(),
        s(out),
    ];
    args.extend(extra.iter().map(|arg| arg.to_string()));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run(&refs)
}

/// The video track's `(start, duration)` frames.
fn video_ranges(project: &Path) -> Vec<(f64, f64)> {
    let json = read_json(project);
    let tracks = json["tracks"]["children"].as_array().expect("tracks");
    let video = tracks
        .iter()
        .find(|track| track["kind"] == "Video")
        .expect("video");
    video["children"]
        .as_array()
        .expect("items")
        .iter()
        .map(|item| {
            let range = &item["source_range"];
            (
                range["start_time"]["value"].as_f64().expect("start"),
                range["duration"]["value"].as_f64().expect("duration"),
            )
        })
        .collect()
}

#[test]
fn from_clip_trims_the_project_to_the_pick() {
    let lecture = lecture();
    let root = scratch("from-clip");
    let project = root.join("clip.otio");
    let output = from_clip(lecture, &project, &["--clip", "1"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("from-clip: out="), "{stdout}");

    let understanding = read_json(&lecture.understanding);
    let clip = &understanding["clips"][0];
    let (start_ms, end_ms) = (
        clip["start_ms"].as_u64().unwrap(),
        clip["end_ms"].as_u64().unwrap(),
    );
    // 24 fps: the start snaps down and the end up.
    let start = (start_ms as f64 * 24.0 / 1000.0).floor();
    let end = (end_ms as f64 * 24.0 / 1000.0).ceil();
    assert_eq!(video_ranges(&project), vec![(start, end - start)]);
    let record = &read_json(&project)["metadata"]["reelctl"]["clip"];
    assert_eq!(record["index"], 1);
    assert_eq!(record["hook_id"], clip["hook"]["id"]);

    // The same arguments give the same bytes.
    let again = root.join("again.otio");
    assert!(from_clip(lecture, &again, &["--clip", "1"])
        .status
        .success());
    assert_eq!(fs::read(&project).unwrap(), fs::read(&again).unwrap());
}

#[test]
fn from_clip_refuses_a_missing_or_flagged_clip_and_another_video() {
    let lecture = lecture();
    let root = scratch("from-clip-refusals");
    let out = root.join("clip.otio");
    let args = |extra: &[&str], understanding: &Path| {
        let mut args = vec![
            "timeline".to_string(),
            "from-clip".to_string(),
            "--input".to_string(),
            s(&lecture.video),
            "--dna".to_string(),
            s(&lecture.dna),
            "--understanding".to_string(),
            s(understanding),
            "--out".to_string(),
            s(&out),
        ];
        args.extend(extra.iter().map(|arg| arg.to_string()));
        args
    };
    let call = |args: Vec<String>, needle: &str| {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        exits_2(&refs, needle);
    };
    call(args(&["--clip", "2"], &lecture.understanding), "has 1 clip");
    call(
        args(&["--clip", "0"], &lecture.understanding),
        "numbered from 1",
    );

    let mut flagged = read_json(&lecture.understanding);
    flagged["clips"][0]["flags"] = json!(["tight length 25.0 s is under 30 s"]);
    let flagged_path = root.join("flagged.json");
    fs::write(&flagged_path, serde_json::to_vec(&flagged).unwrap()).unwrap();
    call(args(&["--clip", "1"], &flagged_path), "--allow-flagged");
    let allowed = args(&["--clip", "1", "--allow-flagged"], &flagged_path);
    let refs: Vec<&str> = allowed.iter().map(String::as_str).collect();
    ok(&refs);

    let mut other = read_json(&lecture.understanding);
    other["source"]["path"] = json!("/elsewhere/another.mp4");
    let other_path = root.join("other.json");
    fs::write(&other_path, serde_json::to_vec(&other).unwrap()).unwrap();
    fs::remove_file(&out).unwrap();
    call(args(&["--clip", "1"], &other_path), "another.mp4");
    // A refused run writes nothing.
    assert!(!out.exists());
}

#[test]
fn the_clip_steps_chain_and_render() {
    let lecture = lecture();
    let root = scratch("chain");
    let clip = root.join("01-from-clip.otio");
    assert!(from_clip(lecture, &clip, &["--clip", "1"]).status.success());

    let cut = root.join("02-cut-silence.otio");
    ok(&[
        "timeline",
        "cut-silence",
        "--project",
        &s(&clip),
        "--dna",
        &s(&lecture.dna),
        "--min-gap-ms",
        "250",
        "--out",
        &s(&cut),
    ]);
    // The 300 ms pauses inside the clip are gone: more than one piece.
    assert!(video_ranges(&cut).len() > 1);

    let punched = root.join("03-auto-punch-in.otio");
    let stdout = ok(&[
        "transform",
        "auto-punch-in",
        "--project",
        &s(&cut),
        "--dna",
        &s(&lecture.dna),
        "--outlier",
        &s(&lecture.dna),
        "--out",
        &s(&punched),
    ]);
    // Two flat shots with one cut: static windows everywhere, and no punch-in in
    // the outlier, so the default scale.
    assert!(stdout.contains("scale=1.15 scale_from=default"), "{stdout}");
    let punches: usize = stdout
        .split_whitespace()
        .find_map(|field| field.strip_prefix("punches="))
        .and_then(|n| n.parse().ok())
        .expect("punches");
    assert!(punches >= 10, "{stdout}");
    // A second run adds nothing.
    let again = root.join("03b.otio");
    let stdout = ok(&[
        "transform",
        "auto-punch-in",
        "--project",
        &s(&punched),
        "--dna",
        &s(&lecture.dna),
        "--out",
        &s(&again),
    ]);
    assert!(stdout.contains("windows=0 punches=0"), "{stdout}");
    assert_eq!(fs::read(&punched).unwrap(), fs::read(&again).unwrap());

    exits_2(
        &[
            "funnel",
            "attach-bridge",
            "--project",
            &s(&punched),
            "--target-url",
            "https://example.com/full",
            "--append",
            "--cutoff-pct",
            "0.5",
            "--out",
            &s(&root.join("x.otio")),
        ],
        "cannot be used with",
    );
    let bridged = root.join("04-bridge.otio");
    let stdout = ok(&[
        "funnel",
        "attach-bridge",
        "--project",
        &s(&punched),
        "--target-url",
        "https://example.com/full",
        "--append",
        "--card-ms",
        "2000",
        "--out",
        &s(&bridged),
    ]);
    assert!(stdout.contains("appended=true"), "{stdout}");
    let before: f64 = video_ranges(&punched).iter().map(|(_, d)| d).sum();
    let ranges = video_ranges(&bridged);
    assert_eq!(ranges.last().unwrap().1, 48.0);
    let after: f64 = ranges.iter().map(|(_, d)| d).sum();
    assert_eq!(after, before + 48.0);

    // Captions go last: any other edit refuses a captioned project.
    let captioned = root.join("05-captions.otio");
    let stdout = ok(&[
        "overlay",
        "captions",
        "--project",
        &s(&bridged),
        "--dna",
        &s(&lecture.dna),
        "--preset",
        "bold",
        "--out",
        &s(&captioned),
    ]);
    assert!(stdout.starts_with("captions: out="), "{stdout}");
    assert!(stdout.contains("preset=bold"), "{stdout}");
    exits_2(
        &[
            "transform",
            "punch-in",
            "--project",
            &s(&captioned),
            "--start-ms",
            "0",
            "--end-ms",
            "1000",
            "--scale",
            "1.2",
            "--out",
            &s(&root.join("x.otio")),
        ],
        "add captions last",
    );
    let groups = read_json(&captioned)["metadata"]["reelctl"]["captions"]["groups"].clone();
    let first = &groups[0];
    let caption_frame = (first["start"].as_i64().unwrap() + first["end"].as_i64().unwrap()) / 2;

    let video = root.join("reel.mp4");
    ok(&[
        "render",
        "--project",
        &s(&captioned),
        "--out",
        &s(&video),
        "--size",
        "360x640",
    ]);
    let probe = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-count_frames",
            "-show_entries",
            "stream=width,height,nb_read_frames",
            "-of",
            "json",
        ])
        .arg(&video)
        .output()
        .expect("ffprobe");
    let json: Value = serde_json::from_slice(&probe.stdout).expect("json");
    let stream = &json["streams"][0];
    assert_eq!(
        (stream["width"].as_u64(), stream["height"].as_u64()),
        (Some(360), Some(640))
    );
    let frames: f64 = stream["nb_read_frames"].as_str().unwrap().parse().unwrap();
    assert_eq!(frames, after);

    // On a caption's frame there is near-white text in the lower half of the
    // picture and none in the upper half; the footage is flat red or blue.
    let frame = rgb_frame(&video, caption_frame as u32, 360, 640);
    let white = |rows: std::ops::Range<usize>| {
        rows.flat_map(|y| (0..360).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let at = (y * 360 + x) * 3;
                frame[at] > 220 && frame[at + 1] > 220 && frame[at + 2] > 220
            })
            .count()
    };
    assert!(
        white(320..640) > 100,
        "{} white pixels below the middle",
        white(320..640)
    );
    assert_eq!(white(0..320), 0);
}

/// Frame `index` of `video` as `width` x `height` RGB bytes.
fn rgb_frame(video: &Path, index: u32, width: usize, height: usize) -> Vec<u8> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .args([
            "-vf",
            &format!("select=eq(n\\,{index})"),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ])
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len(), width * height * 3);
    output.stdout
}

/// A 4 s, 640x360 clip whose luma rises from left to right (0 at x=0, 255 at
/// x=640), so a crop's left edge can be read from its first column.
fn gradient(path: &Path) {
    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=640x360:r=30:d=4,format=yuv420p,geq=lum='X*255/W':cb=128:cr=128",
            "-c:v",
            "libx264",
            "-qp",
            "0",
            "-pix_fmt",
            "yuv420p",
        ])
        .arg(path)
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The luma of column 2, row 180, of frame `index` of `video`.
fn left_luma(video: &Path, index: u32) -> f64 {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(video)
        .args([
            "-vf",
            &format!("select=eq(n\\,{index}),format=gray,crop=1:1:2:180"),
            "-frames:v",
            "1",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .expect("spawn ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    f64::from(output.stdout[0])
}

#[test]
fn reframe_centre_renders_a_vertical_window_and_presenter_needs_the_model() {
    let lecture = lecture();
    let root = scratch("reframe");
    let clip = root.join("clip.otio");
    assert!(from_clip(lecture, &clip, &["--clip", "1"]).status.success());

    // Without weights the presenter cannot be followed: exit 2, nothing written.
    let refused = root.join("refused.otio");
    exits_2(
        &[
            "transform",
            "reframe",
            "--project",
            &s(&clip),
            "--follow",
            "presenter",
            "--out",
            &s(&refused),
        ],
        "weights",
    );
    assert!(!refused.exists());

    let centred = root.join("centred.otio");
    let stdout = ok(&[
        "transform",
        "reframe",
        "--project",
        &s(&clip),
        "--follow",
        "centre",
        "--out",
        &s(&centred),
    ]);
    assert!(
        stdout.contains("aspect=9:16 follow=centre keyframes=1"),
        "{stdout}"
    );
    let record = &read_json(&centred)["metadata"]["reelctl"]["reframe"];
    // 360 * 9 / 16 = 202.5, made even.
    assert_eq!(
        (
            record["crop_width"].as_u64(),
            record["crop_height"].as_u64()
        ),
        (Some(202), Some(360))
    );

    // (The Smelter backend's refusal of a reframed project is a unit test in
    // `timeline::smelter`: `render` looks for the Smelter program first.)
}

#[test]
fn ffmpeg_moves_the_window_along_the_keyframes() {
    let root = scratch("moving-crop");
    let media = root.join("gradient.mp4");
    gradient(&media);
    let project = root.join("gradient.otio");
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
        "reframe",
        "--project",
        &s(&project),
        "--follow",
        "centre",
        "--out",
        &s(&project),
    ]);
    // Move the window's left edge from 0 to 0.6 of the width over frames 0..90,
    // then hold it.
    let mut json = read_json(&project);
    json["metadata"]["reelctl"]["reframe"]["keyframes"] =
        json!([{"source_frame": 0, "x": 0.0}, {"source_frame": 90, "x": 0.6}]);
    fs::write(&project, serde_json::to_vec_pretty(&json).unwrap()).unwrap();
    let video = root.join("moving.mp4");
    ok(&["render", "--project", &s(&project), "--out", &s(&video)]);
    for (frame, x) in [(0u32, 0.0), (45, 0.3), (90, 0.6), (110, 0.6)] {
        // The clip's luma is written as limited range; `format=gray` expands it.
        let luma: f64 = (x * 640.0 + 2.0) * 255.0 / 640.0;
        let want = ((luma - 16.0) * 255.0 / 219.0).clamp(0.0, 255.0);
        let got = left_luma(&video, frame);
        assert!(
            (got - want).abs() <= 6.0,
            "frame {frame}: luma {got}, expected about {want}"
        );
    }
}
