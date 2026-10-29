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
        assert!(reel["length_ms"].as_u64().unwrap() > 20_000, "{reel}");
        // No secret in any recorded argument list.
        assert!(!reel["steps"].to_string().contains("OPENROUTER"));
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
    let first = fs::read(out.join("reel-2/06-captions.otio")).unwrap();
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
    assert_eq!(
        first,
        fs::read(out.join("reel-2/06-captions.otio")).unwrap()
    );
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
