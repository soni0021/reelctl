//! Tests of `eval`: hand-made truth, synthetic text, no model.

use edit_dna::EditDna;
use serde_json::json;
use understand::{ClipPick, RawReply, RawSummary, Understanding, VlmReply};

use crate::builder::build;
use crate::builder_tests::{dna_file, file, options, reel_dna};
use crate::clips::fixtures::{lecture_dna, pick, understanding};
use crate::clips::rebuild;
use crate::error::DatasetError;
use crate::eval::{
    eval_dataset, eval_understanding, picks_from_reply, read_records, render_text, EvalReport,
    Tolerances, MAX_PROBLEMS,
};
use crate::record::{AnswerPick, Record, Task};

// ---------------------------------------------------------------- understanding truth

fn reply_of(picks: Vec<ClipPick>) -> RawReply {
    RawReply {
        video_summary: RawSummary {
            topic: "t".to_string(),
            language: "en".to_string(),
            speaker_count: 1,
            visual_formats: Vec::new(),
        },
        beats: Vec::new(),
        edit_events: Vec::new(),
        reel_candidates: picks,
    }
}

fn vlm_reply(picks: Vec<ClipPick>) -> Vec<u8> {
    let reply = VlmReply {
        reply: reply_of(picks),
        provider: "ProviderZ".to_string(),
        quantization: None,
        input_tokens: 1,
        output_tokens: 1,
        cost_usd: None,
        elapsed_ms: 1,
    };
    serde_json::to_vec(&reply).expect("reply")
}

fn truth() -> (Understanding, EditDna) {
    let run = understanding(
        "lecture.mp4",
        40,
        &[pick(1, 3, 13), pick(5, 6, 3), pick(20, 22, 32)],
    );
    (run, lecture_dna(40))
}

fn score(picks: Vec<ClipPick>) -> EvalReport {
    let (run, dna) = truth();
    eval_understanding(&run, &dna, "reply.json", &vlm_reply(picks)).expect("eval")
}

fn rate(report: &EvalReport) -> (Option<f64>, Option<f64>) {
    let clip = report.clip_picks.as_ref().expect("clip_picks");
    (clip.valid_rate.rate, clip.rule_pass_rate.rate)
}

#[test]
fn the_truths_own_picks_fed_back_reproduce_its_clips_and_score_one() {
    let (run, dna) = truth();
    // The run has two clips (the middle pick was rejected), ranks 1 and 3.
    assert_eq!(run.clips.len(), 2);
    assert_eq!(run.rejected.len(), 1);
    let mut clips = run.clips.clone();
    clips.sort_by_key(|clip| clip.rank);
    let answer: Vec<AnswerPick> = clips
        .iter()
        .map(|clip| AnswerPick {
            hook_segment: clip.hook.id,
            payoff_segment: clip.payoff.id,
            cutoff_segment: clip.last.id,
            start_segment: None,
            end_segment: None,
        })
        .collect();
    // The stored clips' cut points and flags come back from the same rules.
    let outcome = rebuild(&answer, &run.segments, &dna.silence_gaps, dna.duration_ms);
    assert_eq!(outcome.clips.len(), clips.len());
    for (stored, rebuilt) in clips.iter().zip(&outcome.clips) {
        assert_eq!(stored.start_ms, rebuilt.start_ms);
        assert_eq!(stored.end_ms, rebuilt.end_ms);
        assert_eq!(stored.flags, rebuilt.flags);
        assert_eq!(stored.tight_length_ms, rebuilt.tight_length_ms);
        assert_eq!(stored.payoff_at_ms, rebuilt.payoff_at_ms);
    }

    let reply_picks: Vec<ClipPick> = clips
        .iter()
        .map(|clip| pick(clip.hook.id, clip.payoff.id, clip.last.id))
        .collect();
    let report = score(reply_picks);
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.truth_picks, 2);
    assert_eq!(clip.truth_unreproducible, 0);
    assert_eq!(clip.picks, 2);
    assert_eq!(clip.valid_rate.rate, Some(1.0));
    assert_eq!(clip.rule_pass_rate.rate, Some(1.0));
    assert_eq!(clip.hook.precision, Some(1.0));
    assert_eq!(clip.hook.recall, Some(1.0));
    assert_eq!(clip.hook.f1, Some(1.0));
    assert_eq!(clip.payoff_given_hook.rate, Some(1.0));
    assert_eq!(clip.cutoff_given_hook.rate, Some(1.0));
    assert_eq!(clip.interval.precision, Some(1.0));
    assert_eq!(clip.interval.recall, Some(1.0));
    assert_eq!(clip.interval.f1, Some(1.0));
    assert_eq!(clip.teachers, vec!["teacher/model-x".to_string()]);
    assert_eq!(report.records, 1);
}

#[test]
fn an_empty_reply_scores_zero_recall_and_no_pass_rate() {
    let report = score(Vec::new());
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.picks, 0);
    assert_eq!(clip.valid_rate.rate, None);
    assert_eq!(clip.rule_pass_rate.rate, None);
    assert_eq!(clip.hook.recall, Some(0.0));
    assert_eq!(clip.hook.precision, None);
    assert_eq!(clip.hook.f1, Some(0.0));
    assert_eq!(clip.interval.recall, Some(0.0));
    assert_eq!(clip.interval.precision, None);
    assert_eq!(clip.payoff_given_hook.rate, None);
    assert_eq!(clip.cutoff_given_hook.rate, None);
}

#[test]
fn an_unknown_segment_id_lowers_validity_and_hook_precision() {
    let report = score(vec![pick(1, 3, 13), pick(900, 901, 902)]);
    assert_eq!(rate(&report), (Some(0.5), Some(0.5)));
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.hook.precision, Some(0.5));
    assert_eq!(clip.hook.recall, Some(0.5));
    assert_eq!(clip.interval.recall, Some(0.5));
}

#[test]
fn a_pick_that_is_valid_but_breaks_a_rule_is_valid_and_not_a_pass() {
    // Segments 1 to 2 are 7 s: under the 30 s minimum.
    let report = score(vec![pick(1, 2, 2)]);
    assert_eq!(rate(&report), (Some(1.0), Some(0.0)));
}

#[test]
fn the_right_hook_with_a_different_payoff_or_cutoff_is_counted_per_field() {
    // Truth (1, 3, 13). The answer keeps the hook and the cutoff but moves the payoff.
    let report = score(vec![pick(1, 4, 13)]);
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.payoff_given_hook.rate, Some(0.0));
    assert_eq!(clip.cutoff_given_hook.rate, Some(1.0));
    assert_eq!(clip.payoff_given_hook.denominator, 1);
    // A different cutoff moves the end of the clip; the interval differs but overlaps.
    let report = score(vec![pick(1, 3, 12)]);
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.payoff_given_hook.rate, Some(1.0));
    assert_eq!(clip.cutoff_given_hook.rate, Some(0.0));
}

#[test]
fn an_interval_must_overlap_by_half_to_match() {
    // Truth clip: segments 1..=13. An answer of segments 1..=5 is 5 of 13 sentences of
    // speech: below half. It is also under 30 s, so it is flagged, but it still has an
    // interval to compare.
    let report = score(vec![pick(1, 3, 5)]);
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.hook.recall, Some(0.5));
    assert_eq!(clip.interval.recall, Some(0.0));
    assert_eq!(clip.interval.precision, Some(0.0));
    // Segments 1..=9 are about 9/13 of the truth's length: above half.
    let report = score(vec![pick(1, 3, 9)]);
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.interval.recall, Some(0.5));
    assert_eq!(clip.interval.precision, Some(1.0));
}

#[test]
fn a_hook_given_twice_is_one_hook_and_the_first_pick_decides_payoff_and_cutoff() {
    let report = score(vec![pick(20, 22, 32), pick(20, 25, 33)]);
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.hook.tp, 1);
    assert_eq!(clip.hook.fp, 0);
    assert_eq!(clip.hook.fn_, 1);
    assert_eq!(clip.payoff_given_hook.rate, Some(1.0));
    assert_eq!(clip.cutoff_given_hook.rate, Some(1.0));
    assert_eq!(clip.picks, 2);
}

#[test]
fn truth_that_does_not_fit_the_given_pauses_is_counted_and_not_scored() {
    let (run, mut dna) = truth();
    dna.silence_gaps.push(edit_dna::SilenceGap {
        start_ms: 10_000,
        end_ms: 30_000,
        duration_ms: 20_000,
    });
    let report =
        eval_understanding(&run, &dna, "r", &vlm_reply(vec![pick(1, 3, 13)])).expect("eval");
    let clip = report.clip_picks.expect("clip_picks");
    assert_eq!(clip.truth_unreproducible, 1);
    assert_eq!(clip.truth_picks, 1);
}

#[test]
fn a_different_video_is_refused() {
    let (run, _) = truth();
    let error =
        eval_understanding(&run, &lecture_dna(41), "r", &vlm_reply(Vec::new())).unwrap_err();
    assert!(error.to_string().contains("not the same video"), "{error}");
}

#[test]
fn a_reply_is_a_vlm_reply_or_a_bare_raw_reply_and_nothing_else() {
    let picks = vec![pick(1, 3, 13)];
    let from_vlm = picks_from_reply("r", &vlm_reply(picks.clone())).expect("vlm");
    let bare = serde_json::to_vec(&reply_of(picks)).expect("raw");
    let from_raw = picks_from_reply("r", &bare).expect("raw");
    assert_eq!(from_vlm, from_raw);
    assert_eq!(from_vlm[0].hook_segment, 1);
    assert_eq!(from_vlm[0].start_segment, Some(1));
    assert!(picks_from_reply("r", b"not json").is_err());
    assert!(picks_from_reply("r", br#"{"reel_candidates": []}"#).is_err());
    assert!(picks_from_reply("r", br#"{"reply": {"beats": []}}"#).is_err());
}

// ---------------------------------------------------------------- dataset truth

/// The reel with events, a lecture with a run and a second plain reel, as records.
fn records() -> Vec<Record> {
    let run = file(
        "lecture.understanding.json",
        &understanding("lecture.mp4", 40, &[pick(1, 3, 13), pick(20, 22, 32)]),
    );
    let output = build(
        &[
            dna_file("lecture", &lecture_dna(40)),
            dna_file("reel-a", &reel_dna()),
        ],
        &[run],
        &options(0),
    )
    .expect("build");
    read_records("train.jsonl", &output.train_jsonl).expect("records")
}

fn target_of(record: &Record) -> serde_json::Value {
    serde_json::to_value(&record.target).expect("target")
}

fn answers_for(
    records: &[Record],
    edit: impl Fn(&Record, serde_json::Value) -> serde_json::Value,
) -> String {
    records
        .iter()
        .map(|record| {
            json!({"id": record.id, "answer": edit(record, target_of(record))}).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn dataset_score(records: &[Record], answers: &str) -> EvalReport {
    eval_dataset(records, answers, "train", &Tolerances::default()).expect("eval")
}

#[test]
fn a_datasets_own_targets_score_perfectly() {
    let records = records();
    assert_eq!(records.len(), 5);
    let report = dataset_score(&records, &answers_for(&records, |_, target| target));
    assert_eq!((report.records, report.answered, report.missing), (5, 5, 0));
    assert_eq!((report.unparseable, report.extra_answers), (0, 0));
    assert!(report.problems.is_empty());
    let clip = report.clip_picks.expect("clip");
    assert_eq!(clip.hook.f1, Some(1.0));
    assert_eq!(clip.interval.f1, Some(1.0));
    assert_eq!(clip.rule_pass_rate.rate, Some(1.0));
    assert_eq!(clip.teachers, vec!["teacher/model-x".to_string()]);
    let events = report.edit_events.expect("events");
    assert_eq!(events.cuts.scores.f1, Some(1.0));
    assert_eq!(events.cuts.mean_abs_error_ms, Some(0.0));
    assert_eq!(events.cuts.median_abs_error_ms, Some(0.0));
    assert_eq!(events.visual_events.f1, Some(1.0));
    assert_eq!(events.scale.pairs, 1);
    assert_eq!(events.scale.mean_abs_error, Some(0.0));
    assert_eq!(events.zoom_curves.pairs, 1);
    assert_eq!(events.zoom_curves.mean_abs_error, Some(0.0));
    let pacing = report.pacing.expect("pacing");
    assert_eq!(pacing.cut_count.exact_rate, Some(1.0));
    assert_eq!(pacing.integrated_lufs.mean_abs_error, Some(0.0));
    assert_eq!(pacing.payoff_has_cue.accuracy, Some(1.0));
    // reel-a has burst words, so its truth is `null` and is not scored; the lecture's
    // words come from whisper.
    assert_eq!(pacing.words_per_minute.pairs, 1);
}

#[test]
fn no_answers_at_all_score_zero_recall_and_count_every_record_missing() {
    let records = records();
    let report = dataset_score(&records, "");
    assert_eq!((report.answered, report.missing), (0, 5));
    let events = report.edit_events.expect("events");
    assert_eq!(events.answered, 0);
    assert_eq!(events.cuts.scores.recall, Some(0.0));
    assert_eq!(events.cuts.scores.precision, None);
    assert_eq!(events.cuts.mean_abs_error_ms, None);
    assert_eq!(events.visual_events.recall, Some(0.0));
    let pacing = report.pacing.expect("pacing");
    assert_eq!(pacing.cut_count.missing, 2);
    assert_eq!(pacing.cut_count.mean_abs_error, None);
    assert_eq!(pacing.payoff_has_cue.missing, 2);
    assert_eq!(pacing.payoff_has_cue.accuracy, None);
    assert_eq!(report.clip_picks.expect("clip").picks, 0);
}

fn shift_cuts(by: i64) -> impl Fn(&Record, serde_json::Value) -> serde_json::Value {
    move |record, mut target| {
        if record.task == Task::EditEvents {
            for cut in target["cuts"].as_array_mut().expect("cuts") {
                let time = cut["time_ms"].as_i64().expect("time") + by;
                cut["time_ms"] = time.into();
            }
        }
        target
    }
}

#[test]
fn cut_timing_error_is_measured_inside_the_tolerance() {
    let records = records();
    let report = dataset_score(&records, &answers_for(&records, shift_cuts(60)));
    let cuts = report.edit_events.expect("events").cuts;
    assert_eq!(cuts.scores.f1, Some(1.0));
    assert_eq!(cuts.mean_abs_error_ms, Some(60.0));
    assert_eq!(cuts.median_abs_error_ms, Some(60.0));
}

#[test]
fn the_cut_tolerance_edge_is_inclusive_and_beyond_it_is_a_miss() {
    let records = records();
    let at_edge = dataset_score(&records, &answers_for(&records, shift_cuts(100)));
    assert_eq!(
        at_edge.edit_events.expect("events").cuts.scores.f1,
        Some(1.0)
    );
    let beyond = dataset_score(&records, &answers_for(&records, shift_cuts(101)));
    let cuts = beyond.edit_events.expect("events").cuts;
    assert_eq!(cuts.scores.tp, 0);
    assert_eq!(cuts.scores.fp, 2);
    assert_eq!(cuts.scores.fn_, 2);
    assert_eq!(cuts.scores.f1, Some(0.0));
    assert_eq!(cuts.mean_abs_error_ms, None);
    // A wider tolerance takes the same answer back.
    let wide = Tolerances {
        cut_ms: 101,
        ..Tolerances::default()
    };
    let report = eval_dataset(
        &records,
        &answers_for(&records, shift_cuts(101)),
        "train",
        &wide,
    )
    .expect("eval");
    assert_eq!(
        report.edit_events.expect("events").cuts.scores.f1,
        Some(1.0)
    );
    assert_eq!(report.tolerances.cut_ms, 101);
}

#[test]
fn an_event_of_the_wrong_kind_is_not_a_match_and_the_scale_and_curve_errors_are_measured() {
    let records = records();
    let wrong_kind = answers_for(&records, |record, mut target| {
        if record.task == Task::EditEvents && record.reel_id == "reel-a" {
            target["visual_events"][0]["kind"] = "zoom_out".into();
        }
        target
    });
    let report = dataset_score(&records, &wrong_kind);
    let events = report.edit_events.expect("events").visual_events;
    assert_eq!((events.tp, events.fp, events.fn_), (0, 1, 1));

    let off = answers_for(&records, |record, mut target| {
        if record.task == Task::EditEvents && record.reel_id == "reel-a" {
            target["visual_events"][0]["scale"] = 1.3.into();
            target["zoom_curves"][0]["bezier"] = json!([1.0 / 3.0, 0.2, 2.0 / 3.0, 0.9]);
        }
        target
    });
    let report = dataset_score(&records, &off).edit_events.expect("events");
    let scale = report.scale.mean_abs_error.expect("scale error");
    assert!((scale - 0.1).abs() < 1e-9, "{scale}");
    // y1 is off by 0.1 and y2 by 0: the mean over the two is 0.05.
    let curve = report.zoom_curves.mean_abs_error.expect("curve error");
    assert!((curve - 0.05).abs() < 1e-9, "{curve}");
    assert_eq!(report.zoom_curves.pairs, 1);
}

#[test]
fn a_truth_of_not_measured_is_not_scored_and_a_truth_of_none_is_scored() {
    let mut dna = reel_dna();
    dna.schema_version = 2;
    let mut value = serde_json::to_value(&dna).expect("value");
    value
        .as_object_mut()
        .expect("object")
        .remove("visual_events");
    value.as_object_mut().expect("object").remove("zoom_curves");
    let old = file("old.edit_dna.json", &value);
    let mut none = reel_dna();
    none.visual_events.clear();
    none.zoom_curves = edit_dna::ZoomCurves::measured(Vec::new());
    let output = build(&[old, dna_file("none", &none)], &[], &options(0)).expect("build");
    let records = read_records("t", &output.train_jsonl).expect("records");
    // Both answers claim one punch-in.
    let claim = answers_for(&records, |record, mut target| {
        if record.task == Task::EditEvents {
            target["visual_events"] =
                json!([{"time_ms": 7000, "kind": "punch_in", "scale": 1.2, "area": null}]);
        }
        target
    });
    let events = dataset_score(&records, &claim).edit_events.expect("events");
    assert_eq!(events.visual_events_unscored, 1);
    assert_eq!(events.zoom_curves_unscored, 0);
    // Only the reel that measured none is scored, and the claim is a false positive.
    assert_eq!(events.visual_events.fp, 1);
    assert_eq!(events.visual_events.tp, 0);
    assert_eq!(events.visual_events.recall, None);
}

#[test]
fn pacing_errors_missing_fields_and_wrong_booleans_are_counted() {
    let records = records();
    let answers = answers_for(&records, |record, mut target| {
        if record.task == Task::Pacing && record.reel_id == "reel-a" {
            target["cut_count"] = 3.into();
            target["integrated_lufs"] = (-16.5).into();
            target["payoff_has_cue"] = true.into();
            target
                .as_object_mut()
                .expect("object")
                .remove("mean_shot_ms");
        }
        target
    });
    let pacing = dataset_score(&records, &answers).pacing.expect("pacing");
    // Only reel-a is changed: 2 cuts answered as 3, loudness 2 dB off, the cue claimed.
    assert_eq!(pacing.cut_count.pairs, 2);
    assert_eq!(pacing.cut_count.mean_abs_error, Some(0.5));
    assert_eq!(pacing.cut_count.exact_rate, Some(0.5));
    assert_eq!(pacing.integrated_lufs.mean_abs_error, Some(1.0));
    assert_eq!(pacing.mean_shot_ms.missing, 1);
    assert_eq!(pacing.mean_shot_ms.pairs, 1);
    assert_eq!(pacing.mean_shot_ms.exact_rate, Some(1.0));
    assert_eq!(pacing.payoff_has_cue.pairs, 2);
    assert_eq!(pacing.payoff_has_cue.accuracy, Some(0.5));
}

#[test]
fn an_answer_that_is_not_the_tasks_shape_is_unparseable_and_scored_as_no_answer() {
    let records = records();
    let answers = answers_for(&records, |record, target| {
        if record.task == Task::Pacing && record.reel_id == "reel-a" {
            json!("this is not an object")
        } else {
            target
        }
    });
    let report = dataset_score(&records, &answers);
    assert_eq!(report.unparseable, 1);
    assert_eq!(report.answered, 4);
    assert_eq!(report.missing, 0);
    assert_eq!(report.problems.len(), 1);
    assert!(
        report.problems[0].starts_with("pacing-reel-a-"),
        "{:?}",
        report.problems
    );
    assert_eq!(report.pacing.expect("pacing").answered, 1);
}

#[test]
fn a_line_that_is_not_json_is_a_problem_and_blank_lines_are_ignored() {
    let records = records();
    let good = answers_for(&records, |_, target| target);
    let report = dataset_score(&records, &format!("{good}\n\nnot json at all\n"));
    assert_eq!(report.unparseable, 1);
    assert_eq!(report.answered, 5);
    assert!(
        report.problems[0].starts_with("answers line"),
        "{:?}",
        report.problems
    );
}

#[test]
fn a_duplicate_answer_id_is_an_error() {
    let records = records();
    let line = json!({"id": records[0].id, "answer": {}}).to_string();
    let error = eval_dataset(
        &records,
        &format!("{line}\n{line}"),
        "train",
        &Tolerances::default(),
    )
    .unwrap_err();
    assert!(matches!(error, DatasetError::DuplicateAnswer(ref id) if *id == records[0].id));
}

#[test]
fn an_answer_for_no_record_is_counted_and_ignored() {
    let records = records();
    let good = answers_for(&records, |_, target| target);
    let report = dataset_score(
        &records,
        &format!(
            "{good}\n{}",
            json!({"id": "pacing-ghost-000000000000", "answer": {}})
        ),
    );
    assert_eq!(report.extra_answers, 1);
    assert_eq!(report.answered, 5);
}

#[test]
fn only_the_first_twenty_problems_are_listed() {
    let records = records();
    let junk: Vec<String> = (0..25).map(|n| format!("junk {n}")).collect();
    let report = dataset_score(&records, &junk.join("\n"));
    assert_eq!(report.unparseable, 25);
    assert_eq!(report.problems.len(), MAX_PROBLEMS + 1);
    assert_eq!(report.problems[MAX_PROBLEMS], "... and 5 more not listed");
}

#[test]
fn an_empty_split_is_an_error_not_a_score() {
    let error = eval_dataset(&[], "", "val", &Tolerances::default()).unwrap_err();
    assert!(
        error.to_string().contains("split val has no records"),
        "{error}"
    );
}

#[test]
fn a_record_line_with_a_bad_shape_names_the_file_and_line() {
    let error = read_records("val.jsonl", "\n{\"id\": 1}\n").unwrap_err();
    assert!(
        error.to_string().starts_with("val.jsonl: line 2:"),
        "{error}"
    );
}

#[test]
fn a_report_has_no_nan_and_its_text_names_the_numbers() {
    let records = records();
    let report = dataset_score(&records, "");
    let text = serde_json::to_string(&report).expect("json");
    assert!(!text.contains("NaN") && !text.contains("inf"), "{text}");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    assert_eq!(value["eval_schema_version"], 1);
    assert_eq!(value["truth_kind"], "dataset");
    assert_eq!(value["split"], "train");
    assert_eq!(value["edit_events"]["cuts"]["recall"], 0.0);
    assert_eq!(
        value["edit_events"]["cuts"]["precision"],
        serde_json::Value::Null
    );
    assert_eq!(value["edit_events"]["cuts"]["fn"], 2);
    let rendered = render_text(&report);
    assert!(
        rendered.contains("truth: dataset, split train"),
        "{rendered}"
    );
    assert!(
        rendered.contains("items 5 | answered 0 | missing 5"),
        "{rendered}"
    );
    assert!(rendered.contains("recall 0.000"), "{rendered}");
    assert!(rendered.contains("precision n/a"), "{rendered}");
    assert!(rendered.contains("clip_picks"), "{rendered}");
}

#[test]
fn a_report_of_an_understanding_names_no_split() {
    let report = score(vec![pick(1, 3, 13)]);
    assert_eq!(report.split, None);
    let value = serde_json::to_value(&report).expect("json");
    assert_eq!(value["truth_kind"], "understanding");
    assert_eq!(value["split"], serde_json::Value::Null);
    assert_eq!(value["edit_events"], serde_json::Value::Null);
    assert!(render_text(&report).contains("truth: understanding.json"));
}
