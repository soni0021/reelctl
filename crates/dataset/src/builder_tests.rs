//! Tests of `builder`: hand-made `EditDna` and `Understanding` values, synthetic text.

use diff::{reel_metrics, NamedReel};
use edit_dna::{
    AsrBackend, Cut, EditDna, SilenceGap, VisualEvent, VisualEventKind, ZoomCurve, ZoomCurves,
};

use crate::builder::{
    build, edit_events_target, pacing_target, reel_id_from_dna_name, reel_id_from_source_path,
    BuildOptions, FileInput, Output,
};
use crate::clips::fixtures::{lecture_dna, pick, understanding};
use crate::error::DatasetError;
use crate::manifest::{SkipReason, SplitCounts};
use crate::record::{Record, Task};
use crate::sha256;
use crate::split::DEFAULT_SALT;

pub(crate) fn options(val_percent: u32) -> BuildOptions {
    BuildOptions {
        salt: DEFAULT_SALT.to_string(),
        val_percent,
    }
}

pub(crate) fn file(name: &str, value: &impl serde::Serialize) -> FileInput {
    FileInput {
        file_name: name.to_string(),
        bytes: serde_json::to_vec_pretty(value).expect("serialize"),
    }
}

pub(crate) fn dna_file(reel: &str, dna: &EditDna) -> FileInput {
    file(&format!("{reel}.edit_dna.json"), dna)
}

fn lines(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .collect()
}

fn all_lines(output: &Output) -> Vec<serde_json::Value> {
    let mut all = lines(&output.train_jsonl);
    all.extend(lines(&output.val_jsonl));
    all
}

fn with_version(dna: &EditDna, version: u32) -> FileInput {
    let mut value = serde_json::to_value(dna).expect("value");
    value["schema_version"] = version.into();
    if version < 4 {
        value.as_object_mut().expect("object").remove("zoom_curves");
    }
    if version < 3 {
        value
            .as_object_mut()
            .expect("object")
            .remove("visual_events");
    }
    file("old.edit_dna.json", &value)
}

fn event(time_ms: u64) -> VisualEvent {
    VisualEvent {
        time_ms,
        frame_index: time_ms / 33,
        kind: VisualEventKind::PunchIn,
        scale: Some(1.2),
        center_x: Some(0.5),
        center_y: Some(0.5),
        area: None,
    }
}

pub(crate) fn reel_dna() -> EditDna {
    let mut dna = EditDna::empty(12_000, 30.0, 540, 960);
    dna.cuts = vec![
        Cut {
            frame_index: 150,
            time_ms: 5_000,
            interval_ms: 5_000,
            scene_score: 0.9,
        },
        Cut {
            frame_index: 60,
            time_ms: 2_000,
            interval_ms: 2_000,
            scene_score: 0.8,
        },
    ];
    dna.visual_events = vec![event(7_000)];
    dna.zoom_curves = ZoomCurves::measured(vec![ZoomCurve {
        event_index: 0,
        start_ms: 6_800,
        end_ms: 7_000,
        from_scale: 1.0,
        to_scale: 1.2,
        bezier: Some([1.0 / 3.0, 0.1, 2.0 / 3.0, 0.9]),
        samples: 6,
        fit_rms: Some(0.01),
    }]);
    dna.integrated_lufs = -18.5;
    dna
}

#[test]
fn a_reel_gets_an_edit_events_record_and_a_pacing_record() {
    let output = build(&[dna_file("reel-a", &reel_dna())], &[], &options(0)).expect("build");
    let records = all_lines(&output);
    assert_eq!(records.len(), 2);
    assert!(output.val_jsonl.is_empty());
    let events = &records[0];
    assert_eq!(events["task"], "edit_events");
    assert_eq!(events["reel_id"], "reel-a");
    assert_eq!(events["split"], "train");
    assert_eq!(events["trainable"], false);
    assert_eq!(events["dataset_schema_version"], 1);
    assert_eq!(events["input"]["duration_ms"], 12_000);
    assert_eq!(events["context"], serde_json::json!({}));
    // Cuts are in time order whatever order the file had.
    assert_eq!(
        events["target"]["cuts"],
        serde_json::json!([{"time_ms": 2000}, {"time_ms": 5000}])
    );
    assert_eq!(events["target"]["visual_events"][0]["kind"], "punch_in");
    assert_eq!(events["target"]["visual_events"][0]["scale"], 1.2);
    assert_eq!(events["target"]["zoom_curves"][0]["event_index"], 0);
    let pacing = &records[1];
    assert_eq!(pacing["task"], "pacing");
    assert_eq!(pacing["target"]["cut_count"], 2);
    assert_eq!(pacing["target"]["integrated_lufs"], -18.5);
    assert_eq!(pacing["target"]["payoff_has_cue"], false);
    assert_eq!(pacing["target"]["payoff_cue_ms"], serde_json::Value::Null);
    assert_eq!(pacing["provenance"]["tool"], "reelctl-dataset");
    assert_eq!(pacing["provenance"]["edit_dna_schema_version"], 4);
    assert_eq!(pacing["provenance"]["cut_detector"], "ffmpeg_scene_score");
    assert_eq!(
        pacing["provenance"]["understanding"],
        serde_json::Value::Null
    );
}

#[test]
fn the_pacing_numbers_are_the_ones_reelctl_diff_reports() {
    let dna = reel_dna();
    let metrics = reel_metrics(NamedReel {
        name: "x",
        dna: &dna,
    });
    let target = pacing_target("x", &dna).expect("finite");
    assert_eq!(target.mean_shot_ms, metrics.mean_shot_ms);
    assert_eq!(target.longest_shot_ms, metrics.longest_shot_ms);
    assert_eq!(target.cut_count, 2);
    // Shots are 0-2000, 2000-5000, 5000-12000.
    assert_eq!(target.longest_shot_ms, 7_000);
    assert_eq!(target.mean_shot_ms, 4_000);
}

#[test]
fn a_burst_rate_is_not_reported_as_a_word_rate() {
    let mut dna = reel_dna();
    dna.words_per_minute = 142.0;
    dna.asr_backend = AsrBackend::SilenceBursts;
    assert_eq!(pacing_target("x", &dna).expect("ok").words_per_minute, None);
    dna.asr_backend = AsrBackend::WhisperCpp;
    assert_eq!(
        pacing_target("x", &dna).expect("ok").words_per_minute,
        Some(142.0)
    );
}

#[test]
fn the_payoff_cue_is_carried_when_there_is_one() {
    let mut dna = reel_dna();
    dna.payoff_window.has_cue = true;
    dna.payoff_window.cue_ms = Some(9_100);
    let target = pacing_target("x", &dna).expect("ok");
    assert!(target.payoff_has_cue);
    assert_eq!(target.payoff_cue_ms, Some(9_100));
}

#[test]
fn unknown_schema_versions_are_refused_before_anything_is_written() {
    let dna = reel_dna();
    for version in [0u32, 5, 99] {
        let error = build(&[with_version(&dna, version)], &[], &options(10)).unwrap_err();
        assert!(
            matches!(error, DatasetError::UnknownSchemaVersion { .. }),
            "{version}: {error}"
        );
        assert!(error.to_string().contains("old.edit_dna.json"), "{error}");
    }
    let mut value = serde_json::to_value(&dna).expect("value");
    value
        .as_object_mut()
        .expect("object")
        .remove("schema_version");
    let error = build(&[file("x.edit_dna.json", &value)], &[], &options(10)).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
}

#[test]
fn a_newer_file_is_refused_even_though_its_fields_would_parse() {
    // Every field this build knows is present; only the version is from the future.
    let mut value = serde_json::to_value(reel_dna()).expect("value");
    value["schema_version"] = 5.into();
    value["something_new"] = serde_json::json!({"x": 1});
    assert!(build(&[file("x.edit_dna.json", &value)], &[], &options(10)).is_err());
}

#[test]
fn an_unparseable_file_is_an_error() {
    let bad = FileInput {
        file_name: "x.edit_dna.json".to_string(),
        bytes: b"{not json".to_vec(),
    };
    assert!(matches!(
        build(&[bad], &[], &options(10)),
        Err(DatasetError::Parse { .. })
    ));
    let truncated = FileInput {
        file_name: "y.edit_dna.json".to_string(),
        bytes: br#"{"schema_version": 4}"#.to_vec(),
    };
    assert!(matches!(
        build(&[truncated], &[], &options(10)),
        Err(DatasetError::Parse { .. })
    ));
}

#[test]
fn a_version_2_file_has_no_events_to_claim_so_they_are_null_not_empty() {
    let output = build(&[with_version(&reel_dna(), 2)], &[], &options(0)).expect("build");
    let target = &all_lines(&output)[0]["target"];
    assert_eq!(target["visual_events"], serde_json::Value::Null);
    assert_eq!(target["zoom_curves"], serde_json::Value::Null);
    assert_eq!(target["cuts"].as_array().map(Vec::len), Some(2));
}

#[test]
fn a_version_3_file_has_events_but_no_curves() {
    let output = build(&[with_version(&reel_dna(), 3)], &[], &options(0)).expect("build");
    let target = &all_lines(&output)[0]["target"];
    assert_eq!(target["visual_events"].as_array().map(Vec::len), Some(1));
    assert_eq!(target["zoom_curves"], serde_json::Value::Null);
}

#[test]
fn a_version_4_file_with_no_events_says_none_not_unknown() {
    let mut dna = reel_dna();
    dna.visual_events.clear();
    dna.zoom_curves = ZoomCurves::measured(Vec::new());
    let output = build(&[dna_file("a", &dna)], &[], &options(0)).expect("build");
    let target = &all_lines(&output)[0]["target"];
    assert_eq!(target["visual_events"], serde_json::json!([]));
    assert_eq!(target["zoom_curves"], serde_json::json!([]));
}

#[test]
fn a_version_4_file_whose_curves_are_unimplemented_has_null_curves() {
    let mut dna = reel_dna();
    dna.zoom_curves = ZoomCurves::unimplemented();
    let output = build(&[dna_file("a", &dna)], &[], &options(0)).expect("build");
    let target = &all_lines(&output)[0]["target"];
    assert_eq!(target["zoom_curves"], serde_json::Value::Null);
}

#[test]
fn a_non_finite_number_cannot_become_a_target() {
    let mut dna = reel_dna();
    dna.visual_events[0].scale = Some(f64::NAN);
    assert!(edit_events_target(&dna).is_err());
    let mut dna = reel_dna();
    dna.visual_events[0].area = Some(f64::INFINITY);
    assert!(edit_events_target(&dna).is_err());
    let mut dna = reel_dna();
    dna.zoom_curves.curves[0].bezier = Some([0.3, f64::NAN, 0.6, 0.9]);
    assert!(edit_events_target(&dna).is_err());
    let mut dna = reel_dna();
    dna.integrated_lufs = f64::NEG_INFINITY;
    assert!(pacing_target("x", &dna).is_err());
    let mut dna = reel_dna();
    dna.asr_backend = AsrBackend::WhisperCpp;
    dna.words_per_minute = f64::NAN;
    assert!(pacing_target("x", &dna).is_err());
    // The same NaN under burst words is never used, so it does not matter.
    dna.asr_backend = AsrBackend::SilenceBursts;
    assert!(pacing_target("x", &dna).is_ok());
}

#[test]
fn a_reel_with_no_duration_is_skipped_with_the_reason() {
    let zero = EditDna::empty(0, 30.0, 64, 36);
    let output = build(&[dna_file("empty", &zero)], &[], &options(10)).expect("build");
    assert!(all_lines(&output).is_empty());
    assert_eq!(output.manifest.skipped.len(), 1);
    assert_eq!(output.manifest.skipped[0].reason, SkipReason::ZeroDuration);
    assert_eq!(output.manifest.skipped[0].input, "empty");
}

#[test]
fn a_bad_reel_id_is_skipped_with_the_file_name_and_the_rest_is_built() {
    let output = build(
        &[
            dna_file("my reel", &reel_dna()),
            dna_file("good", &reel_dna()),
            dna_file(".hidden", &reel_dna()),
        ],
        &[],
        &options(0),
    )
    .expect("build");
    assert_eq!(all_lines(&output).len(), 2);
    let skipped: Vec<_> = output
        .manifest
        .skipped
        .iter()
        .map(|skip| (skip.input.as_str(), skip.reason))
        .collect();
    assert_eq!(
        skipped,
        vec![
            (".hidden.edit_dna.json", SkipReason::BadReelId),
            ("my reel.edit_dna.json", SkipReason::BadReelId),
        ]
    );
}

#[test]
fn two_files_with_one_reel_id_are_an_error() {
    let a = dna_file("same", &reel_dna());
    let b = FileInput {
        file_name: "same.json".to_string(),
        bytes: a.bytes.clone(),
    };
    let error = build(&[a, b], &[], &options(10)).unwrap_err();
    assert!(matches!(error, DatasetError::DuplicateReelId(ref id) if id == "same"));
}

#[test]
fn the_val_percentage_is_checked() {
    assert!(matches!(
        build(&[], &[], &options(51)),
        Err(DatasetError::ValPercent(51))
    ));
}

#[test]
fn reel_ids_come_from_file_names_and_source_paths() {
    assert_eq!(reel_id_from_dna_name("a.edit_dna.json"), "a");
    assert_eq!(reel_id_from_dna_name("a.v1.edit_dna.json"), "a.v1");
    assert_eq!(reel_id_from_dna_name("a.json"), "a");
    assert_eq!(reel_id_from_dna_name("a"), "a");
    assert_eq!(
        reel_id_from_source_path("Reels/longform/lecture.mp4"),
        "lecture"
    );
    assert_eq!(
        reel_id_from_source_path("C:\\videos\\lecture.mp4"),
        "lecture"
    );
    assert_eq!(reel_id_from_source_path("/abs/dir/a.b.mov"), "a.b");
    assert_eq!(reel_id_from_source_path("lecture"), "lecture");
    assert_eq!(reel_id_from_source_path("dir/.hidden"), ".hidden");
}

fn lecture_inputs() -> (FileInput, FileInput) {
    let picks = [pick(1, 3, 13), pick(20, 22, 32)];
    (
        dna_file("lecture", &lecture_dna(40)),
        file(
            "lecture.understanding.json",
            &understanding("Reels/longform/lecture.mp4", 40, &picks),
        ),
    )
}

fn clip_record(output: &Output) -> Option<serde_json::Value> {
    all_lines(output)
        .into_iter()
        .find(|record| record["task"] == "clip_picks")
}

#[test]
fn clip_picks_carry_ids_and_the_measured_pauses_and_the_teacher() {
    let (dna, run) = lecture_inputs();
    let output = build(&[dna], &[run], &options(0)).expect("build");
    let clip = clip_record(&output).expect("a clip_picks record");
    assert_eq!(clip["trainable"], true);
    assert_eq!(
        clip["target"]["picks"],
        serde_json::json!([
            {"hook_segment": 1, "payoff_segment": 3, "cutoff_segment": 13},
            {"hook_segment": 20, "payoff_segment": 22, "cutoff_segment": 32},
        ])
    );
    assert_eq!(clip["input"]["segments"].as_array().map(Vec::len), Some(40));
    assert_eq!(
        clip["input"]["segments"][0]["text"],
        "Sentence 0 is about topic."
    );
    assert_eq!(clip["input"]["duration_ms"], 1_000 + 3_500 * 40);
    assert_eq!(
        clip["context"]["silence_gaps"].as_array().map(Vec::len),
        Some(39)
    );
    let provenance = &clip["provenance"]["understanding"];
    assert_eq!(provenance["teacher_model"], "teacher/model-x");
    assert_eq!(provenance["teacher_provider"], "ProviderY");
    assert_eq!(provenance["schema_version"], 1);
    assert_eq!(provenance["sha256"].as_str().map(str::len), Some(64));
    assert_eq!(output.manifest.flagged_clips_left_out, 0);
    assert!(output.manifest.skipped.is_empty());
}

#[test]
fn the_teachers_prose_and_labels_never_reach_the_dataset() {
    let (dna, run) = lecture_inputs();
    let output = build(&[dna], &[run], &options(0)).expect("build");
    let everything = format!(
        "{}{}{}",
        output.train_jsonl,
        output.val_jsonl,
        output.manifest_json().expect("manifest")
    );
    for secret in [
        "SECRET-TITLE",
        "SECRET-OPEN-LOOP",
        "SECRET-VISUAL",
        "SECRET-WHY",
        "SECRET-TOPIC",
    ] {
        assert!(!everything.contains(secret), "{secret} leaked");
    }
}

#[test]
fn clips_with_flags_and_rejected_picks_are_not_targets_and_the_flagged_are_counted() {
    let picks = [
        pick(1, 2, 2),    // 3.2 s: flagged (under the minimum)
        pick(1, 3, 13),   // overlaps the first, which was kept: flagged
        pick(5, 6, 3),    // cutoff before the hook: rejected
        pick(20, 22, 32), // fine
    ];
    let run = understanding("lecture.mp4", 40, &picks);
    assert_eq!(run.clips.len(), 3);
    assert_eq!(run.rejected.len(), 1);
    let output = build(
        &[dna_file("lecture", &lecture_dna(40))],
        &[file("r.json", &run)],
        &options(0),
    )
    .expect("build");
    let clip = clip_record(&output).expect("record");
    assert_eq!(
        clip["target"]["picks"],
        serde_json::json!([{"hook_segment": 20, "payoff_segment": 22, "cutoff_segment": 32}])
    );
    assert_eq!(output.manifest.flagged_clips_left_out, 2);
}

#[test]
fn a_run_with_only_flagged_clips_is_skipped_with_the_reason() {
    let run = file(
        "r.json",
        &understanding("lecture.mp4", 40, &[pick(1, 2, 2)]),
    );
    let output = build(
        &[dna_file("lecture", &lecture_dna(40))],
        &[run],
        &options(0),
    )
    .expect("build");
    assert!(clip_record(&output).is_none());
    let skip = &output.manifest.skipped[0];
    assert_eq!(skip.reason, SkipReason::NoRuleCompliantClips);
    assert_eq!(skip.input, "r.json");
    assert_eq!(skip.task, Some(Task::ClipPicks));
    assert_eq!(output.manifest.flagged_clips_left_out, 1);
    // The reel's other records are still built.
    assert_eq!(all_lines(&output).len(), 2);
}

#[test]
fn a_run_without_its_edit_dna_is_skipped_not_dropped_silently() {
    let run = file(
        "r.json",
        &understanding("elsewhere.mp4", 40, &[pick(1, 3, 13)]),
    );
    let output = build(
        &[dna_file("lecture", &lecture_dna(40))],
        &[run],
        &options(0),
    )
    .expect("build");
    assert_eq!(output.manifest.skipped.len(), 1);
    assert_eq!(
        output.manifest.skipped[0].reason,
        SkipReason::NoMatchingEditDna
    );
    assert!(output.manifest.skipped[0].detail.contains("elsewhere"));
}

#[test]
fn a_run_of_a_different_length_than_the_edit_dna_is_skipped() {
    let run = file(
        "r.json",
        &understanding("lecture.mp4", 40, &[pick(1, 3, 13)]),
    );
    let output = build(
        &[dna_file("lecture", &lecture_dna(41))],
        &[run],
        &options(0),
    )
    .expect("build");
    assert_eq!(
        output.manifest.skipped[0].reason,
        SkipReason::DurationMismatch
    );
}

#[test]
fn clips_that_break_a_rule_against_these_pauses_are_skipped() {
    let run = file(
        "r.json",
        &understanding("lecture.mp4", 40, &[pick(1, 3, 13)]),
    );
    let mut dna = lecture_dna(40);
    // A 20 s pause inside the clip drops its tight length under 30 s.
    dna.silence_gaps.push(SilenceGap {
        start_ms: 10_000,
        end_ms: 30_000,
        duration_ms: 20_000,
    });
    let output = build(&[dna_file("lecture", &dna)], &[run], &options(0)).expect("build");
    assert_eq!(
        output.manifest.skipped[0].reason,
        SkipReason::ClipsNotReproducible
    );
}

#[test]
fn a_clip_naming_a_missing_segment_is_inconsistent() {
    let mut run = understanding("lecture.mp4", 40, &[pick(1, 3, 13)]);
    run.clips[0].payoff.id = 777;
    let output = build(
        &[dna_file("lecture", &lecture_dna(40))],
        &[file("r.json", &run)],
        &options(0),
    )
    .expect("build");
    assert_eq!(
        output.manifest.skipped[0].reason,
        SkipReason::InconsistentUnderstanding
    );
}

#[test]
fn the_same_run_twice_is_one_record_and_one_skip() {
    let (dna, run) = lecture_inputs();
    let copy = FileInput {
        file_name: "copy.understanding.json".to_string(),
        bytes: run.bytes.clone(),
    };
    let output = build(&[dna], &[run, copy], &options(0)).expect("build");
    let clip_records = all_lines(&output)
        .iter()
        .filter(|record| record["task"] == "clip_picks")
        .count();
    assert_eq!(clip_records, 1);
    let skip = &output.manifest.skipped[0];
    assert_eq!(skip.reason, SkipReason::DuplicateUnderstanding);
    // "copy..." sorts before "lecture...", so it is used and the other is skipped.
    assert_eq!(skip.input, "lecture.understanding.json");
}

#[test]
fn two_teachers_runs_make_two_records_with_different_ids() {
    let (dna, run) = lecture_inputs();
    let mut other = understanding("lecture.mp4", 40, &[pick(1, 3, 13)]);
    other.model.id = "teacher/model-z".to_string();
    let output = build(&[dna], &[run, file("z.json", &other)], &options(0)).expect("build");
    let ids: Vec<_> = all_lines(&output)
        .iter()
        .filter(|record| record["task"] == "clip_picks")
        .map(|record| record["id"].as_str().expect("id").to_string())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn an_understanding_of_an_unknown_version_is_an_error() {
    let (dna, _) = lecture_inputs();
    let mut run =
        serde_json::to_value(understanding("lecture.mp4", 40, &[pick(1, 3, 13)])).expect("value");
    run["schema_version"] = 2.into();
    let error = build(&[dna], &[file("r.json", &run)], &options(0)).unwrap_err();
    assert!(
        matches!(error, DatasetError::UnknownSchemaVersion { .. }),
        "{error}"
    );
}

#[test]
fn no_absolute_path_or_directory_reaches_any_output() {
    let picks = [pick(1, 3, 13)];
    let run = file(
        "lecture.understanding.json",
        &understanding("/Users/someone/private-dir/lecture.mp4", 40, &picks),
    );
    let output = build(
        &[dna_file("lecture", &lecture_dna(40))],
        &[run],
        &options(0),
    )
    .expect("build");
    let everything = format!(
        "{}{}{}",
        output.train_jsonl,
        output.val_jsonl,
        output.manifest_json().expect("manifest")
    );
    assert!(clip_record(&output).is_some());
    assert!(!everything.contains("/Users"), "{everything}");
    assert!(!everything.contains("private-dir"));
    assert!(!everything.contains(".mp4"));
}

#[test]
fn the_same_inputs_in_any_order_give_the_same_bytes() {
    let a = dna_file("alpha", &reel_dna());
    let b = dna_file("beta", &lecture_dna(40));
    let c = dna_file("lecture", &lecture_dna(40));
    let run1 = file(
        "1.json",
        &understanding("lecture.mp4", 40, &[pick(1, 3, 13)]),
    );
    let run2 = file(
        "2.json",
        &understanding("beta.mp4", 40, &[pick(20, 22, 32)]),
    );
    let forward = build(
        &[a.clone(), b.clone(), c.clone()],
        &[run1.clone(), run2.clone()],
        &options(50),
    )
    .expect("build");
    let shuffled = build(&[c, a, b], &[run2, run1], &options(50)).expect("build");
    assert_eq!(forward.train_jsonl, shuffled.train_jsonl);
    assert_eq!(forward.val_jsonl, shuffled.val_jsonl);
    assert_eq!(
        forward.manifest_json().expect("m"),
        shuffled.manifest_json().expect("m")
    );
    assert_eq!(all_lines(&forward).len(), 8);
}

#[test]
fn records_are_sorted_by_task_then_reel_then_id() {
    let output = build(
        &[
            dna_file("zeta", &reel_dna()),
            dna_file("alpha", &reel_dna()),
        ],
        &[],
        &options(0),
    )
    .expect("build");
    let order: Vec<_> = all_lines(&output)
        .iter()
        .map(|record| {
            format!(
                "{} {}",
                record["task"].as_str().expect("task"),
                record["reel_id"].as_str().expect("reel")
            )
        })
        .collect();
    assert_eq!(
        order,
        vec![
            "edit_events alpha",
            "edit_events zeta",
            "pacing alpha",
            "pacing zeta"
        ]
    );
}

#[test]
fn every_record_of_a_reel_is_in_the_reels_split_and_other_reels_do_not_move_it() {
    let dna = dna_file("draft12", &lecture_dna(40));
    let run = file(
        "draft12.understanding.json",
        &understanding("draft12.mp4", 40, &[pick(1, 3, 13)]),
    );
    // "draft12" has bucket 37: it is val at --val-percent 38 or more, train below.
    let output = build(
        std::slice::from_ref(&dna),
        std::slice::from_ref(&run),
        &options(38),
    )
    .expect("build");
    let records = all_lines(&output);
    assert_eq!(records.len(), 3);
    assert!(records.iter().all(|record| record["split"] == "val"));
    assert!(output.train_jsonl.is_empty());
    let mut many: Vec<FileInput> = (0..30)
        .map(|n| dna_file(&format!("extra{n}"), &reel_dna()))
        .collect();
    many.push(dna.clone());
    let larger = build(&many, &[run.clone()], &options(38)).expect("build");
    let draft_splits: Vec<_> = all_lines(&larger)
        .iter()
        .filter(|record| record["reel_id"] == "draft12")
        .map(|record| record["split"].as_str().expect("split").to_string())
        .collect();
    assert_eq!(draft_splits, vec!["val", "val", "val"]);
    let below = build(&[dna], &[run], &options(37)).expect("build");
    assert!(all_lines(&below)
        .iter()
        .all(|record| record["split"] == "train"));
}

#[test]
fn the_manifest_counts_files_and_warns_about_an_empty_split() {
    let output = build(&[dna_file("reel-a", &reel_dna())], &[], &options(0)).expect("build");
    let manifest = &output.manifest;
    assert_eq!(manifest.files.train.records, 2);
    assert_eq!(manifest.files.val.records, 0);
    assert_eq!(
        manifest.files.train.sha256,
        sha256::hex(output.train_jsonl.as_bytes())
    );
    assert_eq!(manifest.files.val.sha256, sha256::hex(b""));
    assert_eq!(manifest.records["edit_events"].train, 1);
    assert_eq!(manifest.records["pacing"].train, 1);
    assert_eq!(manifest.records["clip_picks"], SplitCounts::default());
    assert_eq!(manifest.reels.len(), 1);
    assert_eq!(manifest.reels[0].reel_id, "reel-a");
    assert_eq!(manifest.reels[0].edit_dna_sha256.len(), 64);
    assert_eq!(manifest.warnings.len(), 1);
    assert!(manifest.warnings[0].starts_with("the val split has no records"));
    assert_eq!(manifest.salt, DEFAULT_SALT);
    assert_eq!(manifest.val_percent, 0);
}

#[test]
fn no_inputs_give_empty_files_and_two_warnings() {
    let output = build(&[], &[], &options(10)).expect("build");
    assert!(output.train_jsonl.is_empty() && output.val_jsonl.is_empty());
    assert_eq!(output.manifest.warnings.len(), 2);
}

#[test]
fn every_line_parses_back_to_the_same_line() {
    let (dna, run) = lecture_inputs();
    let output = build(
        &[dna, dna_file("reel-a", &reel_dna())],
        &[run],
        &options(50),
    )
    .expect("build");
    let mut count = 0;
    for text in [&output.train_jsonl, &output.val_jsonl] {
        for line in text.lines() {
            let record = Record::parse(line).expect("parses");
            assert_eq!(serde_json::to_string(&record).expect("json"), line);
            count += 1;
        }
    }
    assert_eq!(count, 5);
}

#[test]
fn a_record_of_another_version_is_refused_when_read() {
    let output = build(&[dna_file("reel-a", &reel_dna())], &[], &options(0)).expect("build");
    let line = output.train_jsonl.lines().next().expect("line");
    let changed = line.replacen(
        "\"dataset_schema_version\":1",
        "\"dataset_schema_version\":2",
        1,
    );
    assert_ne!(changed, line);
    assert!(Record::parse(&changed).is_err());
}

#[test]
fn ids_name_the_task_and_the_reel_and_change_with_the_input() {
    let output = build(&[dna_file("reel-a", &reel_dna())], &[], &options(0)).expect("build");
    let first = all_lines(&output)[0]["id"]
        .as_str()
        .expect("id")
        .to_string();
    assert!(first.starts_with("edit_events-reel-a-"), "{first}");
    assert_eq!(first.len(), "edit_events-reel-a-".len() + 12);
    let mut changed = reel_dna();
    changed.duration_ms += 1;
    let other = build(&[dna_file("reel-a", &changed)], &[], &options(0)).expect("build");
    assert_ne!(all_lines(&other)[0]["id"], first.as_str());
    let again = build(&[dna_file("reel-a", &reel_dna())], &[], &options(0)).expect("build");
    assert_eq!(all_lines(&again)[0]["id"], first.as_str());
}

#[test]
fn a_jsonl_file_has_one_record_per_line() {
    let (dna, run) = lecture_inputs();
    let output = build(&[dna], &[run], &options(0)).expect("build");
    assert!(output.train_jsonl.ends_with('\n'));
    assert_eq!(output.train_jsonl.lines().count(), 3);
}

#[test]
fn a_file_of_schema_3_that_claims_measured_curves_still_gets_null_curves() {
    // Curves arrived in schema 4; a version 3 file cannot have measured them.
    let mut dna = reel_dna();
    dna.schema_version = 3;
    let output = build(&[dna_file("a", &dna)], &[], &options(0)).expect("build");
    let target = &all_lines(&output)[0]["target"];
    assert_eq!(target["zoom_curves"], serde_json::Value::Null);
    assert_eq!(target["visual_events"].as_array().map(Vec::len), Some(1));
}

#[test]
fn targets_follow_the_teachers_rank_whatever_order_the_file_lists_the_clips_in() {
    let mut run = understanding("lecture.mp4", 40, &[pick(1, 3, 13), pick(20, 22, 32)]);
    run.clips.reverse();
    assert_eq!(run.clips[0].rank, 2);
    let output = build(
        &[dna_file("lecture", &lecture_dna(40))],
        &[file("r.json", &run)],
        &options(0),
    )
    .expect("build");
    let clip = clip_record(&output).expect("record");
    assert_eq!(clip["target"]["picks"][0]["hook_segment"], 1);
    assert_eq!(clip["target"]["picks"][1]["hook_segment"], 20);
}

#[test]
fn a_trainable_task_on_one_side_of_the_split_only_is_warned_about() {
    // "lecture" has bucket 78, so it is train at every allowed --val-percent.
    let (dna, run) = lecture_inputs();
    let output = build(&[dna], &[run], &options(50)).expect("build");
    let warnings = &output.manifest.warnings;
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("clip_picks has 1 records in train and none in val")),
        "{warnings:?}"
    );
    // The eval-only tasks are not held to it.
    assert!(
        !warnings.iter().any(|w| w.starts_with("pacing")),
        "{warnings:?}"
    );
}

#[test]
fn a_trainable_task_in_both_splits_gives_no_such_warning() {
    // draft12 (bucket 37) is val at 38 and lecture (bucket 78) is train.
    let (lecture, run) = lecture_inputs();
    let draft = dna_file("draft12", &lecture_dna(40));
    let run2 = file(
        "d.json",
        &understanding("draft12.mp4", 40, &[pick(1, 3, 13)]),
    );
    let output = build(&[lecture, draft], &[run, run2], &options(38)).expect("build");
    assert!(
        output.manifest.warnings.is_empty(),
        "{:?}",
        output.manifest.warnings
    );
    // No clip_picks at all is not a one-sided task.
    let none = build(&[dna_file("reel-a", &reel_dna())], &[], &options(50)).expect("build");
    assert!(!none
        .manifest
        .warnings
        .iter()
        .any(|w| w.starts_with("clip_picks")));
}
