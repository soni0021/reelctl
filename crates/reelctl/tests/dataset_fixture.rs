//! `reelctl dataset build` and `reelctl eval` end to end, offline: fixtures from
//! `fixtures/generate.sh` decompiled by the real binary, a hand-made `understanding.json`
//! over synthetic text, and answers made by hand. No model, no network.

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

/// `reelctl` without the model, weight, and API key variables a shell may export, so
/// the default suite can never call a model or spend money.
fn reelctl() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_reelctl"));
    command
        .env_remove("REELCTL_WHISPER_MODEL")
        .env_remove("REELCTL_TRANSNET_WEIGHTS")
        .env_remove("ORT_DYLIB_PATH")
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

fn decompile(kind: &str, root: &Path, dna_dir: &Path) -> PathBuf {
    let video = root.join(format!("{kind}.mp4"));
    generate(kind, &video);
    let dna = dna_dir.join(format!("{kind}.edit_dna.json"));
    let output = reelctl()
        .args(["decompile", "--input"])
        .arg(&video)
        .arg("--out")
        .arg(&dna)
        .arg("--filmstrip-dir")
        .arg(root.join(format!("{kind}_timeline")))
        .output()
        .expect("spawn decompile");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    dna
}

/// 20 sentences of 3.2 s every 3.5 s from 1 s ("Sentence N is about topic."), the pauses
/// between them 300 ms, written into the decompiled `lecture70`'s DNA as if whisper had
/// heard them.
fn give_the_lecture_words(dna_path: &Path) {
    let mut value: Value = serde_json::from_slice(&fs::read(dna_path).expect("dna")).expect("json");
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
    fs::write(
        dna_path,
        serde_json::to_vec_pretty(&value).expect("serialize"),
    )
    .expect("write");
}

fn pick(hook: u32, payoff: u32, cutoff: u32) -> ClipPick {
    ClipPick {
        start_segment: hook,
        end_segment: cutoff,
        hook_segment: hook,
        payoff_segment: payoff,
        cutoff_segment: cutoff,
        title: "SECRET-TITLE".to_string(),
        open_loop: "SECRET-OPEN-LOOP".to_string(),
        visual_suggestion: "SECRET-VISUAL".to_string(),
        why: "SECRET-WHY".to_string(),
    }
}

/// The `understanding.json` that `reelctl understand` would write for the lecture, from
/// the rules applied to `picks`. `source_path` is stored as given.
fn write_understanding(dna_path: &Path, source_path: &str, out: &Path) -> Understanding {
    let dna: EditDna = serde_json::from_slice(&fs::read(dna_path).expect("dna")).expect("dna");
    let segments = segment_words(&dna.words, &dna.silence_gaps);
    let outcome = build_clips(
        &[pick(1, 3, 13)],
        &segments,
        &dna.silence_gaps,
        dna.duration_ms,
    );
    let run = Understanding {
        schema_version: understand::SCHEMA_VERSION,
        source: Source {
            path: source_path.to_string(),
            duration_ms: dna.duration_ms,
        },
        model: ModelRun {
            id: "teacher/model-x".to_string(),
            provider: "ProviderY".to_string(),
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
            topic: "SECRET-TOPIC".to_string(),
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
    fs::write(out, serde_json::to_vec_pretty(&run).expect("serialize")).expect("write");
    run
}

struct Corpus {
    /// A directory of this test's own, for what it writes.
    root: PathBuf,
    /// The directory every test shares, which holds the decompiled fixtures.
    base: PathBuf,
    dna_dir: PathBuf,
    understanding: PathBuf,
}

/// Four decompiled fixtures (`draft12`, `motion12`, `ease12`, `lecture70` as `lecture`)
/// and a run over the lecture whose `source.path` is absolute, in the scratch root.
/// Decompiling takes seconds, so it is done once for all the tests of this file.
fn shared() -> &'static (PathBuf, PathBuf, PathBuf) {
    static SHARED: OnceLock<(PathBuf, PathBuf, PathBuf)> = OnceLock::new();
    SHARED.get_or_init(|| {
        let base = scratch("dataset-corpus");
        let dna_dir = base.join("dna");
        fs::create_dir_all(&dna_dir).expect("dir");
        for kind in ["draft12", "motion12", "ease12"] {
            decompile(kind, &base, &dna_dir);
        }
        let lecture = decompile("lecture70", &base, &dna_dir);
        let renamed = dna_dir.join("lecture.edit_dna.json");
        fs::rename(&lecture, &renamed).expect("rename");
        give_the_lecture_words(&renamed);
        let understanding = base.join("lecture.understanding.json");
        let source = format!("{}/private-videos/lecture.mp4", base.display());
        write_understanding(&renamed, &source, &understanding);
        (base, dna_dir, understanding)
    })
}

fn corpus(name: &str) -> Corpus {
    let (base, dna_dir, understanding) = shared();
    let root = base.join(name);
    fs::create_dir_all(&root).expect("dir");
    Corpus {
        root,
        base: base.clone(),
        dna_dir: dna_dir.clone(),
        understanding: understanding.clone(),
    }
}

fn run(command: &mut Command) -> Output {
    command.output().expect("spawn reelctl")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn build(corpus: &Corpus, out: &Path, extra: &[&str]) -> Output {
    run(reelctl()
        .args(["dataset", "build", "--dna"])
        .arg(&corpus.dna_dir)
        .arg("--understanding")
        .arg(&corpus.understanding)
        .arg("--out")
        .arg(out)
        .args(extra))
}

fn eval_json(args: &[&OsStrArg]) -> (Output, Value) {
    let mut command = reelctl();
    command.arg("eval");
    for arg in args {
        command.arg(arg.0);
    }
    let output = run(&mut command);
    let value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    (output, value)
}

/// A borrowed argument, so the helper above takes paths and strings alike.
struct OsStrArg<'a>(&'a std::ffi::OsStr);

fn arg<'a, S: AsRef<std::ffi::OsStr> + ?Sized>(value: &'a S) -> OsStrArg<'a> {
    OsStrArg(value.as_ref())
}

fn records(out: &Path) -> Vec<Value> {
    let mut all = Vec::new();
    for name in ["train.jsonl", "val.jsonl"] {
        for line in fs::read_to_string(out.join(name)).expect("read").lines() {
            all.push(serde_json::from_str(line).expect("a record"));
        }
    }
    all
}

/// Answers equal to the truth: each record's target.
fn perfect_answers(out: &Path, path: &Path, change: impl Fn(&Value) -> Value) {
    let lines: Vec<String> = records(out)
        .iter()
        .map(|record| json!({"id": record["id"], "answer": change(record)}).to_string())
        .collect();
    fs::write(path, lines.join("\n")).expect("write answers");
}

#[test]
fn build_writes_records_a_manifest_and_nothing_that_names_a_path_or_the_teachers_words() {
    let corpus = corpus("dataset-build");
    let out = corpus.root.join("out");
    let output = build(&corpus, &out, &["--val-percent", "50"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let records = records(&out);
    // Four reels x (edit_events, pacing) and one clip_picks run.
    assert_eq!(records.len(), 9);
    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("manifest.json")).expect("manifest"))
            .expect("json");
    assert_eq!(manifest["dataset_schema_version"], 1);
    assert_eq!(manifest["reels"].as_array().map(Vec::len), Some(4));
    assert_eq!(manifest["skipped"], json!([]));
    assert_eq!(
        manifest["records"]["clip_picks"]["train"]
            .as_u64()
            .unwrap_or(0)
            + manifest["records"]["clip_picks"]["val"]
                .as_u64()
                .unwrap_or(0),
        1
    );

    // motion12 has a measured punch-in and zoom-out; they and their curves are targets.
    let motion = records
        .iter()
        .find(|r| r["task"] == "edit_events" && r["reel_id"] == "motion12")
        .expect("motion12 record");
    let kinds: Vec<_> = motion["target"]["visual_events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["kind"].as_str().unwrap_or(""))
        .collect();
    assert!(kinds.contains(&"punch_in"), "{kinds:?}");
    assert!(kinds.contains(&"zoom_out"), "{kinds:?}");
    assert!(motion["target"]["zoom_curves"]
        .as_array()
        .is_some_and(|c| !c.is_empty()));
    // These clips used burst words, not whisper.
    let pacing = records
        .iter()
        .find(|r| r["task"] == "pacing" && r["reel_id"] == "draft12")
        .expect("pacing");
    assert_eq!(pacing["target"]["words_per_minute"], Value::Null);
    assert_eq!(pacing["provenance"]["asr_backend"], "silence_bursts");

    // Hygiene: no path, no media, none of the teacher's prose, in any output file.
    let root = corpus.base.display().to_string();
    for name in ["train.jsonl", "val.jsonl", "manifest.json"] {
        let text = fs::read_to_string(out.join(name)).expect("read");
        assert!(!text.contains(&root), "{name} holds the scratch root");
        assert!(!text.contains("private-videos"), "{name}");
        assert!(!text.contains("/Users"), "{name}");
        for secret in [
            "SECRET-TITLE",
            "SECRET-OPEN-LOOP",
            "SECRET-VISUAL",
            "SECRET-WHY",
            "SECRET-TOPIC",
        ] {
            assert!(!text.contains(secret), "{name} holds {secret}");
        }
    }
    let names: Vec<_> = fs::read_dir(&out)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
}

#[test]
fn building_twice_with_the_inputs_in_another_order_gives_the_same_bytes() {
    let corpus = corpus("dataset-determinism");
    let first = corpus.root.join("first");
    assert!(build(&corpus, &first, &["--val-percent", "50"])
        .status
        .success());

    // The files named one by one, backwards, and a different output directory.
    let second = corpus.root.join("second");
    let mut command = reelctl();
    command.args(["dataset", "build"]);
    let mut names: Vec<_> = fs::read_dir(&corpus.dna_dir)
        .expect("dir")
        .map(|e| e.expect("entry").path())
        .collect();
    names.sort();
    names.reverse();
    for name in &names {
        command.arg("--dna").arg(name);
    }
    command
        .arg("--understanding")
        .arg(&corpus.understanding)
        .arg("--out")
        .arg(&second)
        .args(["--val-percent", "50"]);
    assert!(run(&mut command).status.success());

    for name in ["train.jsonl", "val.jsonl", "manifest.json"] {
        assert_eq!(
            fs::read(first.join(name)).expect("first"),
            fs::read(second.join(name)).expect("second"),
            "{name}"
        );
    }
}

#[test]
fn a_dataset_scored_against_its_own_targets_scores_one_and_a_wrong_answer_scores_lower() {
    let corpus = corpus("dataset-eval");
    let out = corpus.root.join("out");
    assert!(build(&corpus, &out, &["--val-percent", "50"])
        .status
        .success());
    let answers = corpus.root.join("answers.jsonl");
    perfect_answers(&out, &answers, |record| record["target"].clone());

    let (output, report) = eval_json(&[
        &arg("--truth"),
        &arg(&out),
        &arg("--answers"),
        &arg(&answers),
        &arg("--split"),
        &arg("all"),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(report["truth_kind"], "dataset");
    assert_eq!(report["records"], 9);
    assert_eq!(report["answered"], 9);
    assert_eq!(report["clip_picks"]["rule_pass_rate"]["rate"], 1.0);
    assert_eq!(report["clip_picks"]["hook"]["f1"], 1.0);
    assert_eq!(report["clip_picks"]["interval"]["f1"], 1.0);
    assert_eq!(report["edit_events"]["cuts"]["f1"], 1.0);
    assert_eq!(report["edit_events"]["cuts"]["mean_abs_error_ms"], 0.0);
    assert_eq!(report["edit_events"]["visual_events"]["f1"], 1.0);
    assert_eq!(report["edit_events"]["zoom_curves"]["mean_abs_error"], 0.0);
    assert_eq!(report["pacing"]["cut_count"]["exact_rate"], 1.0);

    // The same answers with every cut 150 ms late (past the 100 ms tolerance), a
    // wrong cut count, and a clip pick at the wrong place.
    let wrong = corpus.root.join("wrong.jsonl");
    perfect_answers(&out, &wrong, |record| {
        let mut answer = record["target"].clone();
        match record["task"].as_str() {
            Some("edit_events") => {
                for cut in answer["cuts"].as_array_mut().into_iter().flatten() {
                    cut["time_ms"] = (cut["time_ms"].as_u64().unwrap_or(0) + 150).into();
                }
            }
            Some("pacing") => {
                answer["cut_count"] = (answer["cut_count"].as_u64().unwrap_or(0) + 2).into();
            }
            Some("clip_picks") => {
                answer = json!({"picks": [{"hook_segment": 14, "payoff_segment": 15, "cutoff_segment": 20}]});
            }
            _ => {}
        }
        answer
    });
    let (output, report) = eval_json(&[
        &arg("--truth"),
        &arg(&out),
        &arg("--answers"),
        &arg(&wrong),
        &arg("--split"),
        &arg("all"),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(report["edit_events"]["cuts"]["tp"], 0);
    assert_eq!(report["edit_events"]["cuts"]["f1"], 0.0);
    assert_eq!(report["pacing"]["cut_count"]["mean_abs_error"], 2.0);
    assert_eq!(report["clip_picks"]["hook"]["recall"], 0.0);
    assert_eq!(report["clip_picks"]["interval"]["recall"], 0.0);

    // A wider tolerance on the command line takes the late cuts back.
    let (_, wide) = eval_json(&[
        &arg("--truth"),
        &arg(&out),
        &arg("--answers"),
        &arg(&wrong),
        &arg("--split"),
        &arg("all"),
        &arg("--cut-tolerance-ms"),
        &arg("150"),
    ]);
    assert_eq!(wide["edit_events"]["cuts"]["f1"], 1.0);
    assert_eq!(wide["edit_events"]["cuts"]["mean_abs_error_ms"], 150.0);
}

#[test]
fn an_understanding_is_scored_against_a_replay_style_reply() {
    let corpus = corpus("dataset-understanding");
    let run: Understanding =
        serde_json::from_slice(&fs::read(&corpus.understanding).expect("run")).expect("run");
    assert_eq!(run.clips.len(), 1);
    let dna = corpus.dna_dir.join("lecture.edit_dna.json");

    // The reply `understand --replay` reads: the truth's own pick, then a pick with an
    // ID the transcript does not have.
    let reply = json!({
        "reply": {
            "video_summary": {"topic": "t", "language": "en", "speaker_count": 1, "visual_formats": []},
            "beats": [], "edit_events": [],
            "reel_candidates": [pick(1, 3, 13), pick(500, 501, 502)]
        },
        "provider": "ProviderZ", "quantization": null, "input_tokens": 1, "output_tokens": 1,
        "cost_usd": null, "elapsed_ms": 1
    });
    let reply_path = corpus.root.join("reply.json");
    fs::write(&reply_path, reply.to_string()).expect("write");
    let (output, report) = eval_json(&[
        &arg("--truth"),
        &arg(&corpus.understanding),
        &arg("--dna"),
        &arg(&dna),
        &arg("--answers"),
        &arg(&reply_path),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(report["truth_kind"], "understanding");
    assert_eq!(report["split"], Value::Null);
    let clip = &report["clip_picks"];
    assert_eq!(clip["picks"], 2);
    assert_eq!(clip["valid_rate"]["rate"], 0.5);
    assert_eq!(clip["rule_pass_rate"]["rate"], 0.5);
    assert_eq!(clip["hook"]["recall"], 1.0);
    assert_eq!(clip["hook"]["precision"], 0.5);
    assert_eq!(clip["teachers"], json!(["teacher/model-x"]));

    // The text format is a table of the same numbers.
    let text = run_text(&corpus.understanding, &dna, &reply_path);
    assert!(text.contains("rule_pass_rate: 0.500 (1 of 2)"), "{text}");
    assert!(text.contains("teacher: teacher/model-x"), "{text}");
}

fn run_text(truth: &Path, dna: &Path, answers: &Path) -> String {
    let output = run(reelctl()
        .args(["eval", "--format", "text", "--truth"])
        .arg(truth)
        .arg("--dna")
        .arg(dna)
        .arg("--answers")
        .arg(answers));
    assert!(output.status.success(), "{}", stderr(&output));
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn bad_inputs_exit_with_the_documented_codes_and_write_nothing() {
    let corpus = corpus("dataset-errors");
    let out = corpus.root.join("never");

    // An unknown schema version is an error that writes nothing.
    let mut value: Value = serde_json::from_slice(
        &fs::read(corpus.dna_dir.join("draft12.edit_dna.json")).expect("dna"),
    )
    .expect("json");
    value["schema_version"] = 99.into();
    let bad = corpus.root.join("bad");
    fs::create_dir_all(&bad).expect("dir");
    fs::write(bad.join("future.edit_dna.json"), value.to_string()).expect("write");
    let output = run(reelctl()
        .args(["dataset", "build", "--dna"])
        .arg(&bad)
        .arg("--out")
        .arg(&out));
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("unknown schema_version 99"),
        "{}",
        stderr(&output)
    );
    assert!(!out.exists(), "nothing is written for a refused input");

    // --val-percent over 50 is an argument error.
    let output = build(&corpus, &out, &["--val-percent", "51"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(!out.exists());

    // A directory with no edit_dna.json is an argument error.
    let empty = corpus.root.join("empty");
    fs::create_dir_all(&empty).expect("dir");
    let output = run(reelctl()
        .args(["dataset", "build", "--dna"])
        .arg(&empty)
        .arg("--out")
        .arg(&out));
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));

    // An understanding.json without --dna is an argument error.
    let reply = corpus.root.join("reply.json");
    fs::write(&reply, "{}").expect("write");
    let output = run(reelctl()
        .args(["eval", "--truth"])
        .arg(&corpus.understanding)
        .arg("--answers")
        .arg(&reply));
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("--dna"), "{}", stderr(&output));
}

#[test]
fn an_empty_split_is_reported_when_building_and_refused_when_scoring() {
    let corpus = corpus("dataset-empty-split");
    let out = corpus.root.join("out");
    let output = build(&corpus, &out, &["--val-percent", "0"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("warning: the val split has no records"),
        "{}",
        stderr(&output)
    );
    assert_eq!(fs::metadata(out.join("val.jsonl")).expect("val").len(), 0);

    let answers = corpus.root.join("answers.jsonl");
    fs::write(&answers, "").expect("write");
    let output = run(reelctl()
        .args(["eval", "--truth"])
        .arg(&out)
        .arg("--answers")
        .arg(&answers));
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("split val has no records"),
        "{}",
        stderr(&output)
    );
    // The train split scores, and no answers means nothing was answered.
    let (output, report) = eval_json(&[
        &arg("--truth"),
        &arg(&out),
        &arg("--answers"),
        &arg(&answers),
        &arg("--split"),
        &arg("train"),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(report["answered"], 0);
    assert_eq!(report["missing"], 9);
}

#[test]
fn a_run_with_no_reel_is_skipped_loudly() {
    let corpus = corpus("dataset-skip");
    let out = corpus.root.join("out");
    // Only draft12: the lecture's run has no edit_dna.json to join.
    let output = run(reelctl()
        .args(["dataset", "build", "--dna"])
        .arg(corpus.dna_dir.join("draft12.edit_dna.json"))
        .arg("--understanding")
        .arg(&corpus.understanding)
        .arg("--out")
        .arg(&out)
        .args(["--val-percent", "50"]));
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stderr(&output)
            .contains("skipped lecture.understanding.json [clip_picks]: no_matching_edit_dna"),
        "{}",
        stderr(&output)
    );
    let manifest: Value =
        serde_json::from_slice(&fs::read(out.join("manifest.json")).expect("manifest"))
            .expect("json");
    assert_eq!(manifest["skipped"][0]["reason"], "no_matching_edit_dna");
    assert_eq!(records(&out).len(), 2);
}
