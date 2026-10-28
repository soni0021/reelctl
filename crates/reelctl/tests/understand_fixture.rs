//! `reelctl understand` end to end, with no network: a recorded model reply
//! (`--replay`) and a model listing from a file (`--catalog`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use edit_dna::EditDna;
use serde_json::{json, Value};
use understand::Understanding;

const DUMMY_KEY: &str = "sk-or-v1-dummy-key-that-must-never-appear";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `reelctl` without the model, weight, and API key variables a shell may export, so
/// the default suite can never call a model or spend money.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("OPENROUTER_API_KEY");
    command
}

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("reelctl-{name}-{}", std::process::id()));
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

/// 20 sentences of 3.2 s every 3.5 s from 1 s: "Sentence N is about topic." Each sentence
/// ends a segment, and the 300 ms pauses between them are dead air (over 180 ms) but
/// not a segment boundary (under 400 ms).
fn sentence(n: u64) -> (u64, u64) {
    let start = 1_000 + 3_500 * n;
    (start, start + 3_200)
}

/// The `lecture70` fixture, decompiled for real, then given hand-written words.
struct Lecture {
    root: PathBuf,
    video: PathBuf,
    dna: PathBuf,
    catalog: PathBuf,
    replay: PathBuf,
}

fn lecture(name: &str) -> Lecture {
    let root = scratch(name);
    let video = root.join("lecture.mp4");
    generate("lecture70", &video);
    let dna = root.join("lecture.edit_dna.json");
    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&video)
        .arg("--out")
        .arg(&dna)
        .arg("--filmstrip-dir")
        .arg(root.join("timeline"))
        .output()
        .expect("spawn decompile");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut value: Value = serde_json::from_slice(&fs::read(&dna).expect("dna")).expect("json");
    let mut words = Vec::new();
    let mut gaps = Vec::new();
    for n in 0..20u64 {
        let (start, end) = sentence(n);
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
    fs::write(&dna, serde_json::to_vec_pretty(&value).expect("serialize")).expect("write dna");

    let catalog = root.join("catalog.json");
    fs::write(
        &catalog,
        json!({"models": {"qwen/qwen3.5-9b": [
            {"provider": "DeepInfra", "quantization": "bf16", "context_length": 262144,
             "prompt_usd_per_token": 0.0000001, "completion_usd_per_token": 0.00000015},
            {"provider": "Slowpoke", "quantization": "int4", "context_length": 8192,
             "prompt_usd_per_token": 0.00000001, "completion_usd_per_token": 0.00000002}
        ]}})
        .to_string(),
    )
    .expect("write catalog");

    let replay = root.join("reply.json");
    fs::write(&replay, reply().to_string()).expect("write reply");
    Lecture {
        root,
        video,
        dna,
        catalog,
        replay,
    }
}

/// What the model "said": a good clip, one with an unknown segment, and a very short one.
fn reply() -> Value {
    let pick =
        |rank: u32, start: u32, hook: u32, payoff: u32, cutoff: u32, end: u32, title: &str| {
            json!({"rank": rank, "start_segment": start, "end_segment": end, "hook_segment": hook,
               "payoff_segment": payoff, "cutoff_segment": cutoff, "title": title,
               "open_loop": "what comes next", "visual_suggestion": "punch in", "why": "because"})
        };
    json!({
        "reply": {
            "video_summary": {"topic": "test lecture", "language": "English", "speaker_count": 1,
                              "visual_formats": ["talking_head"]},
            "beats": [{"start_segment": 1, "end_segment": 2, "kind": "hook", "summary": "opens",
                       "visual": "red screen", "insight_density": 3}],
            "edit_events": [{"t_s": 35.3, "kind": "hard_cut", "note": "the screen turns blue"}],
            "reel_candidates": [
                pick(1, 3, 3, 5, 14, 15, "A good clip"),
                pick(2, 3, 3, 5, 999, 999, "An impossible clip"),
                pick(3, 1, 1, 2, 2, 3, "A short clip")
            ]
        },
        "provider": "DeepInfra",
        "quantization": null,
        "input_tokens": 1234,
        "output_tokens": 56,
        "cost_usd": 0.0071,
        "elapsed_ms": 1500
    })
}

fn understand(lecture: &Lecture, out: &Path, extra: &[&str]) -> Output {
    reelctl()
        .args(["understand", "--input"])
        .arg(&lecture.video)
        .arg("--dna")
        .arg(&lecture.dna)
        .arg("--out")
        .arg(out)
        .arg("--catalog")
        .arg(&lecture.catalog)
        .args(extra)
        .output()
        .expect("spawn understand")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn a_recorded_reply_becomes_clips_with_exact_text_and_labelled_cuts() {
    let lecture = lecture("und-ok");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &["--replay", &lecture.replay.to_string_lossy()],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).contains("clips=2 rejected=1"),
        "{}",
        text(&output.stdout)
    );

    let understanding: Understanding =
        serde_json::from_slice(&fs::read(&out).expect("output")).expect("understanding.json");
    let dna: EditDna = serde_json::from_slice(&fs::read(&lecture.dna).expect("dna")).expect("dna");

    // Segments: one per sentence, with whisper's exact text.
    assert_eq!(understanding.segments.len(), 20);
    assert_eq!(understanding.segments[2].text, "Sentence 2 is about topic.");
    assert_eq!(understanding.segments[2].start_ms, 8_000);

    // The good clip: starts at its hook (segment 3), ends at segment 14, 6-12 s payoff, in range.
    let clip = &understanding.clips[0];
    assert_eq!((clip.rank, clip.title.as_str()), (1, "A good clip"));
    assert!(clip.flags.is_empty(), "{:?}", clip.flags);
    assert_eq!(clip.hook.id, 3);
    assert_eq!(clip.hook.text, understanding.segments[2].text);
    assert_eq!(clip.last.id, 14);
    assert_eq!(clip.last.text, understanding.segments[13].text);
    assert_eq!(clip.start_ms, 8_000);
    assert_eq!(clip.end_ms, 49_850);
    assert_eq!(clip.dead_air.len(), 11);
    assert_eq!(clip.tight_length_ms, 41_850 - 11 * 300);
    assert_eq!(clip.payoff_at_ms, Some(15_000 - 8_000 - 2 * 300));

    // The short clip is kept and flagged, not changed; the impossible one is rejected.
    assert_eq!(understanding.clips.len(), 2);
    assert!(
        understanding.clips[1]
            .flags
            .iter()
            .any(|flag| flag.contains("30")),
        "{:?}",
        understanding.clips[1].flags
    );
    assert_eq!(understanding.rejected.len(), 1);
    assert_eq!(understanding.rejected[0].rank, 2);

    // The cut at 35 s is labelled with what the model saw in the frame after it.
    assert_eq!(dna.cuts.len(), 1);
    assert_eq!(understanding.cuts[0].time_ms, 35_000);
    let label = understanding.cuts[0].vlm_label.as_ref().expect("a label");
    assert_eq!(label.note, "the screen turns blue");
    // The event's time is a sampled frame time: the first frame (24 fps) at or after
    // 35 s plus 300 ms, which is 35,333 ms.
    assert_eq!(understanding.visual_events[0].seen_at_ms, 35_333);

    // The run's facts.
    assert_eq!(understanding.model.id, "qwen/qwen3.5-9b");
    assert_eq!(understanding.model.provider, "DeepInfra");
    assert_eq!(understanding.model.quantization.as_deref(), Some("bf16"));
    assert_eq!(understanding.model.cost_usd, Some(0.0071));
    assert!(understanding.model.cost_upper_bound_usd > 0.0);
    assert_eq!(understanding.settings.every_s, 5);
    assert_eq!(understanding.settings.sheet, "single");
    assert_eq!(understanding.source.duration_ms, 70_000);
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn every_time_in_the_output_is_a_measured_time() {
    let lecture = lecture("und-times");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &["--replay", &lecture.replay.to_string_lossy()],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    let u: Understanding = serde_json::from_slice(&fs::read(&out).expect("output")).expect("json");
    let segment_times: Vec<u64> = u
        .segments
        .iter()
        .flat_map(|s| [s.start_ms, s.end_ms])
        .collect();
    for beat in &u.beats {
        assert!(segment_times.contains(&beat.start_ms) && segment_times.contains(&beat.end_ms));
    }
    // Frames every 5 s from 0, and the first frames (24 fps) at or after 300 ms either
    // side of the cut at 35 s: 34,708 and 35,333 ms.
    for event in &u.visual_events {
        assert!(
            event.seen_at_ms % 5_000 == 0 || [34_708, 35_333].contains(&event.seen_at_ms),
            "{}",
            event.seen_at_ms
        );
    }
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn without_an_api_key_it_exits_3_before_anything_is_sent() {
    let lecture = lecture("und-nokey");
    let out = lecture.root.join("understanding.json");
    let output = understand(&lecture, &out, &[]);
    assert_eq!(output.status.code(), Some(3), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("OPENROUTER_API_KEY"));
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn a_cost_bound_over_the_cap_exits_4() {
    let lecture = lecture("und-cap");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &[
            "--replay",
            &lecture.replay.to_string_lossy(),
            "--max-cost-usd",
            "0.0000001",
        ],
    );
    assert_eq!(output.status.code(), Some(4), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("upper bound"));
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn a_dry_run_plans_without_a_key_or_a_reply_and_writes_nothing() {
    let lecture = lecture("und-dry");
    let out = lecture.root.join("understanding.json");
    let output = understand(&lecture, &out, &["--dry-run"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let stdout = text(&output.stdout);
    // 15 frames every 5 s from 0 to 70 s, plus 2 around the cut: 17, single frames.
    assert!(
        stdout.contains("frames=17 images=17 sheet=single segments=20"),
        "{stdout}"
    );
    assert!(stdout.contains("cost_upper_bound_usd="), "{stdout}");
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn a_dna_without_whisper_words_is_an_argument_error() {
    let lecture = lecture("und-nowords");
    let mut value: Value =
        serde_json::from_slice(&fs::read(&lecture.dna).expect("dna")).expect("json");
    value["asr_backend"] = json!("silence_bursts");
    fs::write(&lecture.dna, value.to_string()).expect("write");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &["--replay", &lecture.replay.to_string_lossy()],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(text(&output.stderr).contains("whisper"));
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn no_endpoint_with_the_asked_quantization_is_an_error_naming_the_listing() {
    let lecture = lecture("und-quant");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &[
            "--replay",
            &lecture.replay.to_string_lossy(),
            "--quant",
            "fp4",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    let stderr = text(&output.stderr);
    assert!(
        stderr.contains("fp4") && stderr.contains("DeepInfra"),
        "{stderr}"
    );
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn the_api_key_never_appears_in_any_output() {
    let lecture = lecture("und-leak");
    let out = lecture.root.join("understanding.json");
    let output = reelctl()
        .env("OPENROUTER_API_KEY", DUMMY_KEY)
        .args(["understand", "--input"])
        .arg(&lecture.video)
        .arg("--dna")
        .arg(&lecture.dna)
        .arg("--out")
        .arg(&out)
        .arg("--catalog")
        .arg(&lecture.catalog)
        .args(["--replay", &lecture.replay.to_string_lossy()])
        .output()
        .expect("spawn understand");
    assert!(output.status.success(), "{}", text(&output.stderr));
    for (what, content) in [
        ("stdout", text(&output.stdout)),
        ("stderr", text(&output.stderr)),
        ("the output file", fs::read_to_string(&out).expect("output")),
    ] {
        assert!(!content.contains(DUMMY_KEY), "the key is in {what}");
    }
    // Also when the run fails for another reason (the cap), after the key was read.
    let failed = reelctl()
        .env("OPENROUTER_API_KEY", DUMMY_KEY)
        .args(["understand", "--input"])
        .arg(&lecture.video)
        .arg("--dna")
        .arg(&lecture.dna)
        .arg("--out")
        .arg(&out)
        .arg("--catalog")
        .arg(&lecture.catalog)
        // The key is read on this run (no --replay). If the cap check were lost the
        // request would go out, so every proxy variable points at a closed local port:
        // the call fails on this machine and nothing leaves it.
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("all_proxy", "http://127.0.0.1:9")
        .env("https_proxy", "http://127.0.0.1:9")
        .env("http_proxy", "http://127.0.0.1:9")
        .args(["--max-cost-usd", "0.0000001"])
        .output()
        .expect("spawn understand");
    assert_eq!(failed.status.code(), Some(4));
    assert!(!text(&failed.stderr).contains(DUMMY_KEY));
    assert!(!text(&failed.stdout).contains(DUMMY_KEY));
    let _ = fs::remove_dir_all(&lecture.root);
}

/// Rewrites the lecture's edit DNA with `change` applied to its JSON.
fn edit_dna(lecture: &Lecture, change: impl FnOnce(&mut Value)) {
    let mut value: Value =
        serde_json::from_slice(&fs::read(&lecture.dna).expect("dna")).expect("json");
    change(&mut value);
    fs::write(&lecture.dna, value.to_string()).expect("write");
}

/// A catalog path that does not exist: a run that reads it exits 1. A check that must
/// come before the model listing is therefore shown by exit code 2, and no fetch of the
/// public price listing can have happened first.
fn missing_catalog(lecture: &Lecture) -> PathBuf {
    lecture.root.join("no-such-catalog.json")
}

fn understand_with_catalog(lecture: &Lecture, out: &Path, catalog: &Path) -> Output {
    reelctl()
        .args(["understand", "--input"])
        .arg(&lecture.video)
        .arg("--dna")
        .arg(&lecture.dna)
        .arg("--out")
        .arg(out)
        .arg("--catalog")
        .arg(catalog)
        .args(["--replay", &lecture.replay.to_string_lossy()])
        .output()
        .expect("spawn understand")
}

#[test]
fn an_every_s_of_zero_is_an_argument_error() {
    let lecture = lecture("und-every0");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &[
            "--replay",
            &lecture.replay.to_string_lossy(),
            "--every-s",
            "0",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("--every-s"),
        "{}",
        text(&output.stderr)
    );
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn a_transcript_with_no_words_is_an_argument_error_before_any_model_listing() {
    let lecture = lecture("und-nowordlist");
    edit_dna(&lecture, |value| {
        value["words"] = json!([]);
    });
    let out = lecture.root.join("understanding.json");
    let output = understand_with_catalog(&lecture, &out, &missing_catalog(&lecture));
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("no words"),
        "{}",
        text(&output.stderr)
    );
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&lecture.root);
}

#[test]
fn a_dna_with_a_duration_of_zero_is_an_argument_error_before_any_model_listing() {
    let lecture = lecture("und-dur0");
    edit_dna(&lecture, |value| {
        value["duration_ms"] = json!(0);
    });
    let out = lecture.root.join("understanding.json");
    let output = understand_with_catalog(&lecture, &out, &missing_catalog(&lecture));
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stderr));
    assert!(
        text(&output.stderr).contains("duration"),
        "{}",
        text(&output.stderr)
    );
    assert!(!out.exists());
    let _ = fs::remove_dir_all(&lecture.root);
}

/// Checks `value` against the subset of JSON Schema that `understanding.schema.json`
/// uses (`$ref` into `$defs`, `anyOf`, `type` as a name or a list, `required`,
/// `properties`, `items`, `enum`, `minimum`, `maximum`). Nothing here knows about
/// `Understanding`; the schema decides. Returns the first violation.
fn validate(root: &Value, schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let name = reference
            .strip_prefix("#/$defs/")
            .ok_or_else(|| format!("{path}: unsupported $ref {reference}"))?;
        let target = root
            .get("$defs")
            .and_then(|defs| defs.get(name))
            .ok_or_else(|| format!("{path}: no definition {name}"))?;
        return validate(root, target, value, path);
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        let mut errors = Vec::new();
        for option in options {
            match validate(root, option, value, path) {
                Ok(()) => return Ok(()),
                Err(error) => errors.push(error),
            }
        }
        return Err(format!(
            "{path}: matches no anyOf option ({})",
            errors.join("; ")
        ));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(format!("{path}: {value} is not in the enum"));
        }
    }
    let names: Vec<&str> = match schema.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).collect(),
        _ => return Err(format!("{path}: the schema states no type")),
    };
    let matches = |name: &str| match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    };
    if !names.iter().any(|name| matches(name)) {
        return Err(format!("{path}: {value} is not of type {names:?}"));
    }
    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(format!("{path}: {number} is below {minimum}"));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(format!("{path}: {number} is above {maximum}"));
            }
        }
    }
    if let Some(object) = value.as_object() {
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(required) {
                return Err(format!("{path}: missing {required}"));
            }
        }
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            for (key, child) in object {
                if let Some(child_schema) = properties.get(key) {
                    validate(root, child_schema, child, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            validate(root, item_schema, item, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

#[test]
fn the_written_understanding_json_validates_against_the_committed_schema() {
    let schema: Value = serde_json::from_slice(
        &fs::read(repo_root().join("crates/understand/schema/understanding.schema.json"))
            .expect("schema file"),
    )
    .expect("schema json");
    let lecture = lecture("und-schema");
    let out = lecture.root.join("understanding.json");
    let output = understand(
        &lecture,
        &out,
        &["--replay", &lecture.replay.to_string_lossy()],
    );
    assert!(output.status.success(), "{}", text(&output.stderr));
    let written: Value = serde_json::from_slice(&fs::read(&out).expect("output")).expect("json");
    validate(&schema, &schema, &written, "$").expect("the written file follows the schema");

    // Not vacuous: the file has content the schema constrains, and a corrupted copy fails.
    assert!(written["clips"].as_array().is_some_and(|c| !c.is_empty()));
    assert!(written["cuts"].as_array().is_some_and(|c| !c.is_empty()));
    let mut missing_top = written.clone();
    if let Some(object) = missing_top.as_object_mut() {
        object.remove("model");
    }
    assert!(validate(&schema, &schema, &missing_top, "$")
        .is_err_and(|error| error.contains("missing model")));
    let mut missing_nested = written.clone();
    if let Some(object) = missing_nested["clips"][0].as_object_mut() {
        object.remove("title");
    }
    assert!(validate(&schema, &schema, &missing_nested, "$")
        .is_err_and(|error| error.contains("$.clips[0]") && error.contains("missing title")));
    let mut wrong_type = written.clone();
    wrong_type["segments"][0]["start_ms"] = json!("soon");
    assert!(validate(&schema, &schema, &wrong_type, "$").is_err());
    let mut negative = written;
    negative["segments"][0]["start_ms"] = json!(-1);
    assert!(validate(&schema, &schema, &negative, "$").is_err());
    let _ = fs::remove_dir_all(&lecture.root);
}
