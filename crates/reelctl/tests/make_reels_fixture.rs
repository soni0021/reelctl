//! `reelctl make-reels` end to end on `lecture70`: synthetic whisper words in the
//! DNA, a recorded model reply (`--understand-replay`) and a model listing from a
//! file, so nothing calls a model or the network. No RVM weights here, so the crop
//! falls back to the centre and the summary says so.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use serde_json::{json, Value};

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
    let dir = std::env::temp_dir().join(format!("reelctl-make-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn s(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json")
}

struct Lecture {
    video: PathBuf,
    dna: PathBuf,
    outlier: PathBuf,
    catalog: PathBuf,
    replay: PathBuf,
}

/// The lecture, decompiled, with 20 sentences written in as whisper words; the
/// `draft12` fixture decompiled as the outlier; a listing and a reply with two picks.
fn lecture() -> &'static Lecture {
    static LECTURE: OnceLock<Lecture> = OnceLock::new();
    LECTURE.get_or_init(|| {
        let root = scratch("lecture");
        let generate = |kind: &str, path: &Path| {
            let output = Command::new("bash")
                .arg(repo_root().join("fixtures/generate.sh"))
                .args([kind, &s(path)])
                .output()
                .expect("spawn fixture generator");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        let decompile = |video: &Path, dna: &Path| {
            let output = reelctl()
                .args(["decompile", "--input", &s(video), "--out", &s(dna), "--filmstrip-dir"])
                .arg(dna.with_extension("views"))
                .output()
                .expect("spawn decompile");
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        };
        let video = root.join("lecture.mp4");
        generate("lecture70", &video);
        let dna = root.join("lecture.edit_dna.json");
        decompile(&video, &dna);
        let mut value = read_json(&dna);
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
        fs::write(&dna, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let outlier_video = root.join("outlier.mp4");
        generate("draft12", &outlier_video);
        let outlier = root.join("outlier.edit_dna.json");
        decompile(&outlier_video, &outlier);

        let catalog = root.join("catalog.json");
        fs::write(
            &catalog,
            json!({"models": {"qwen/qwen3.5-9b": [
                {"provider": "DeepInfra", "quantization": "bf16", "context_length": 262144,
                 "prompt_usd_per_token": 0.0000001, "completion_usd_per_token": 0.00000015}
            ]}})
            .to_string(),
        )
        .unwrap();
        let pick = |rank: u32, hook: u32, payoff: u32, cutoff: u32| {
            json!({"rank": rank, "start_segment": hook, "end_segment": cutoff, "hook_segment": hook,
                   "payoff_segment": payoff, "cutoff_segment": cutoff, "title": format!("Clip {rank}"),
                   "open_loop": "next", "visual_suggestion": "punch in", "why": "because"})
        };
        let replay = root.join("reply.json");
        fs::write(
            &replay,
            json!({
                "reply": {
                    "video_summary": {"topic": "test", "language": "English", "speaker_count": 1, "visual_formats": []},
                    "beats": [],
                    "edit_events": [],
                    "reel_candidates": [pick(1, 1, 3, 10), pick(2, 11, 13, 19)]
                },
                "provider": "DeepInfra", "quantization": null,
                "input_tokens": 1000, "output_tokens": 50, "cost_usd": 0.0042, "elapsed_ms": 900
            })
            .to_string(),
        )
        .unwrap();
        Lecture {
            video,
            dna,
            outlier,
            catalog,
            replay,
        }
    })
}

fn make_reels(lecture: &Lecture, out_dir: &Path, extra: &[&str]) -> Output {
    reelctl()
        .args([
            "make-reels",
            "--input",
            &s(&lecture.video),
            "--dna",
            &s(&lecture.dna),
            "--outlier",
            &s(&lecture.outlier),
            "--target-url",
            "https://example.com/full",
            "--out-dir",
            &s(out_dir),
            "--understand-catalog",
            &s(&lecture.catalog),
            "--understand-replay",
            &s(&lecture.replay),
        ])
        .args(extra)
        .output()
        .expect("spawn make-reels")
}

fn text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn without_a_yes_and_no_terminal_it_stops_before_the_model_call() {
    let lecture = lecture();
    let out = scratch("no-yes");
    let output = make_reels(lecture, &out, &[]);
    let (stdout, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(4), "{stdout}\n{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(stdout.contains("cost_upper_bound_usd="), "{stdout}");
    assert!(!out.join("understanding.json").exists());
    // A bound over the cap stops it the same way, even with --yes.
    let output = make_reels(lecture, &out, &["--yes", "--max-cost-usd", "0.0000001"]);
    assert_eq!(output.status.code(), Some(4), "{}", text(&output).1);
    assert!(!out.join("understanding.json").exists());
}

#[test]
fn one_video_in_two_vertical_reels_out() {
    let lecture = lecture();
    let out = scratch("two");
    // 540x960 keeps the debug-build decode in `diff` quick; 1080x1920 is the default.
    let output = make_reels(
        lecture,
        &out,
        &[
            "--yes",
            "--allow-flagged",
            "--clips",
            "1,2",
            "--size",
            "540x960",
        ],
    );
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    let last = stdout.lines().last().unwrap();
    assert!(last.starts_with("make-reels: out_dir="), "{stdout}");
    assert!(last.contains("reels=2 ok=2 failed=0"), "{last}");
    assert!(last.contains("follow=centre"), "{last}");

    let summary = read_json(&out.join("make_reels.json"));
    assert_eq!(summary["schema_version"], 1);
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["cost_usd"], 0.0042);
    let reels = summary["reels"].as_array().unwrap();
    assert_eq!(reels.len(), 2);
    for reel in reels {
        assert_eq!(reel["status"], "ok", "{reel}");
        let notes = reel["notes"].to_string();
        assert!(notes.contains("reframe followed the centre"), "{notes}");
        let video = PathBuf::from(reel["path"].as_str().unwrap());
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "json",
            ])
            .arg(&video)
            .output()
            .unwrap();
        let json: Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(
            (
                json["streams"][0]["width"].as_u64(),
                json["streams"][0]["height"].as_u64()
            ),
            (Some(540), Some(960))
        );
        // The reel's length is the edited project's: footage plus the card.
        let index = reel["index"].as_u64().unwrap();
        let project = read_json(&out.join(format!("reel-{index}/06-captions.otio")));
        let frames: f64 = project["tracks"]["children"][0]["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["source_range"]["duration"]["value"].as_f64().unwrap())
            .sum();
        let want_ms = frames * 1000.0 / 24.0;
        let length_ms = reel["length_ms"].as_f64().unwrap();
        assert!(
            (length_ms - want_ms).abs() <= 50.0,
            "{length_ms} vs {want_ms}"
        );
        // The pauses were cut: diff finds no dead air in the footage. The silent
        // card at the end is dead air to diff, and is the only one.
        let report = read_json(&out.join(format!("reel-{index}/reel-{index}.diff.json")));
        let card_start = length_ms - 2_500.0;
        let early: Vec<&Value> = report["flags"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|flag| flag["kind"] == "dead_air")
            .filter(|flag| flag["end_ms"].as_f64().unwrap() < card_start + 100.0)
            .collect();
        assert!(early.is_empty(), "dead air before the card: {early:?}");
        // The last item on the video track is the card.
        let last = project["tracks"]["children"][0]["children"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .clone();
        assert_eq!(
            last["media_references"]["DEFAULT_MEDIA"]["generator_kind"], "reelctl.card",
            "{last}"
        );
        // On a caption's frame there is near-white text in the lower half and
        // none in the upper half (the footage is flat red or blue).
        let group = &project["metadata"]["reelctl"]["captions"]["groups"][0];
        let frame = (group["start"].as_i64().unwrap() + group["end"].as_i64().unwrap()) / 2;
        let pixels = rgb_frame(&video, frame as u32, 540, 960);
        let white = |rows: std::ops::Range<usize>| {
            rows.flat_map(|y| (0..540).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let at = (y * 540 + x) * 3;
                    pixels[at] > 220 && pixels[at + 1] > 220 && pixels[at + 2] > 220
                })
                .count()
        };
        assert!(
            white(480..960) > 200,
            "reel {index}: {} white pixels",
            white(480..960)
        );
        assert_eq!(white(0..480), 0, "reel {index}");
    }
    // The last project ends on the card and carries the captions and the crop.
    let project = read_json(&out.join("reel-1/06-captions.otio"));
    let meta = &project["metadata"]["reelctl"];
    assert!(meta["captions"]["groups"]
        .as_array()
        .is_some_and(|groups| !groups.is_empty()));
    assert_eq!(meta["reframe"]["follow"], "centre");
    assert_eq!(meta["bridge"]["appended"], true);
    assert!(out.join("reel-1/reel-1.diff.json").is_file());

    // Run again: the understanding is used again (no model step) and every
    // project comes out byte for byte the same.
    let projects = |out: &Path| -> Vec<Vec<u8>> {
        let mut all = Vec::new();
        for reel in 1..=2 {
            for step in [
                "01-from-clip",
                "02-cut-silence",
                "03-auto-punch-in",
                "04-reframe",
                "05-bridge",
                "06-captions",
            ] {
                all.push(fs::read(out.join(format!("reel-{reel}/{step}.otio"))).unwrap());
            }
        }
        all
    };
    let first = projects(&out);
    let again = make_reels(
        lecture,
        &out,
        &["--allow-flagged", "--clips", "1,2", "--size", "540x960"],
    );
    assert!(again.status.success(), "{}", text(&again).1);
    let summary = read_json(&out.join("make_reels.json"));
    assert!(
        !summary["steps"].to_string().contains("\"understand\""),
        "{}",
        summary["steps"]
    );
    assert!(
        first == projects(&out),
        "a project changed on the second run"
    );

    // A refused step's exit code is the run's when no reel renders: an odd size
    // makes render exit 2 for every reel.
    let odd = make_reels(
        lecture,
        &out,
        &["--allow-flagged", "--clips", "1", "--size", "541x960"],
    );
    assert_eq!(odd.status.code(), Some(2), "{}", text(&odd).1);
    let summary = read_json(&out.join("make_reels.json"));
    assert_eq!(summary["reels"][0]["failure"]["step"], "render");
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

#[test]
fn the_cap_and_the_key_are_checked_before_anyone_is_asked() {
    let lecture = lecture();
    // Over the cap: exit 4 at once, even without --yes, and the message names the
    // cap rather than asking for a yes.
    let out = scratch("cap");
    let output = make_reels(lecture, &out, &["--max-cost-usd", "0.0000001"]);
    let (_, stderr) = text(&output);
    assert_eq!(output.status.code(), Some(4), "{stderr}");
    assert!(stderr.contains("--max-cost-usd"), "{stderr}");
    assert!(!stderr.contains("--yes"), "{stderr}");
    assert!(!out.join("understanding.json").exists());
    // No key and no recorded reply: exit 3 before asking, nothing written.
    let out = scratch("no-key");
    let output = reelctl()
        .args([
            "make-reels",
            "--input",
            &s(&lecture.video),
            "--dna",
            &s(&lecture.dna),
            "--outlier",
            &s(&lecture.outlier),
            "--target-url",
            "https://example.com",
            "--out-dir",
            &s(&out),
            "--understand-catalog",
            &s(&lecture.catalog),
            "--yes",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "{}", text(&output).1);
    assert!(!out.join("understanding.json").exists());
}

#[test]
fn the_key_is_never_written_or_printed() {
    let lecture = lecture();
    let out = scratch("key");
    let secret = "sk-or-test-NEVER-WRITE-THIS";
    let output = {
        let mut command = reelctl();
        command.env("OPENROUTER_API_KEY", secret).args([
            "make-reels",
            "--input",
            &s(&lecture.video),
            "--dna",
            &s(&lecture.dna),
            "--outlier",
            &s(&lecture.outlier),
            "--target-url",
            "https://example.com",
            "--out-dir",
            &s(&out),
            "--understand-catalog",
            &s(&lecture.catalog),
            "--understand-replay",
            &s(&lecture.replay),
            "--yes",
            "--pick",
            "--allow-flagged",
        ]);
        command.output().unwrap()
    };
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stderr}");
    for (what, body) in [
        ("stdout", stdout),
        ("stderr", stderr),
        (
            "make_reels.json",
            fs::read_to_string(out.join("make_reels.json")).unwrap(),
        ),
        (
            "understanding.json",
            fs::read_to_string(out.join("understanding.json")).unwrap(),
        ),
    ] {
        assert!(!body.contains(secret), "the key is in {what}");
    }
}

#[test]
fn nothing_to_make_and_a_clip_named_twice_are_refused() {
    let lecture = lecture();
    let out = scratch("nothing");
    let output = make_reels(lecture, &out, &["--yes", "--reels", "0"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output).1);
    assert!(text(&output).1.contains("no clip to make"));
    let output = make_reels(lecture, &out, &["--clips", "1,1"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output).1);
    assert!(text(&output).1.contains("twice"));
}

#[test]
fn pick_lists_the_clips_and_stops() {
    let lecture = lecture();
    let out = scratch("pick");
    let output = make_reels(lecture, &out, &["--yes", "--pick", "--allow-flagged"]);
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    let summary = read_json(&out.join("make_reels.json"));
    assert_eq!(summary["status"], "picked");
    assert_eq!(summary["reels"].as_array().unwrap().len(), 2);
    assert!(!out.join("reel-1").exists());
    // A clip number out of range is an argument error.
    let output = make_reels(lecture, &out, &["--clips", "9"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output).1);
}

#[test]
fn without_whisper_words_it_refuses_before_anything_runs() {
    let lecture = lecture();
    let out = scratch("no-words");
    let output = reelctl()
        .args([
            "make-reels",
            "--input",
            &s(&lecture.video),
            "--outlier",
            &s(&lecture.outlier),
            "--target-url",
            "https://example.com",
            "--out-dir",
            &s(&out),
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(text(&output).1.contains("--whisper-model"));
}
