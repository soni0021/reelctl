//! Score a model's answers against measured truth. Offline: the answers are a file.
//! The metrics and their degenerate cases are defined in `docs/specs/p3-dataset-eval.md`.

use std::collections::{BTreeMap, BTreeSet};

use edit_dna::{EditDna, SilenceGap};
use serde::Serialize;
use understand::{Segment, Understanding, VlmReply};

use crate::clips::{rebuild, reproduce};
use crate::error::{DatasetError, MetricError};
use crate::metrics::{abs_error, match_intervals, match_times, median, ratio, Counts, Mean, Prf};
use crate::record::{
    AnswerLine, AnswerPick, ClipPicksAnswer, Context, EditEventsTarget, Input, PacingAnswer,
    PacingTarget, PickIds, Record, Target, Task,
};

/// `eval_schema_version` of a report.
pub const EVAL_SCHEMA_VERSION: u32 = 1;

/// Clips match when their intervals overlap by at least this IoU.
pub const INTERVAL_IOU: f64 = 0.5;

/// Default `--cut-tolerance-ms`: three frames at 30 fps.
pub const DEFAULT_CUT_TOLERANCE_MS: u64 = 100;

/// Default `--event-tolerance-ms`: a zoom move takes up to 12 frames.
pub const DEFAULT_EVENT_TOLERANCE_MS: u64 = 250;

/// At most this many problems are listed in a report; the rest are counted.
pub const MAX_PROBLEMS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Tolerances {
    pub cut_ms: u64,
    pub event_ms: u64,
    pub interval_iou: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            cut_ms: DEFAULT_CUT_TOLERANCE_MS,
            event_ms: DEFAULT_EVENT_TOLERANCE_MS,
            interval_iou: INTERVAL_IOU,
        }
    }
}

/// A share with the counts behind it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rate {
    pub numerator: u64,
    pub denominator: u64,
    pub rate: Option<f64>,
}

impl Rate {
    pub fn new(numerator: u64, denominator: u64) -> Result<Self, MetricError> {
        Ok(Self {
            numerator,
            denominator,
            rate: ratio(numerator, denominator)?,
        })
    }
}

// ---------------------------------------------------------------------------------
// clip_picks

/// The truth for one `clip_picks` item: the run's transcript, the measured pauses and
/// the teacher's rule-compliant picks.
#[derive(Debug, Clone)]
pub struct ClipTruth {
    pub segments: Vec<Segment>,
    pub pauses: Vec<SilenceGap>,
    pub duration_ms: u64,
    pub picks: Vec<PickIds>,
    pub teacher: String,
}

#[derive(Debug, Clone, Default)]
pub struct ClipAccumulator {
    records: u64,
    answered: u64,
    truth_picks: u64,
    truth_unreproducible: u64,
    picks: u64,
    valid: u64,
    rule_pass: u64,
    hook: Counts,
    hook_matched: u64,
    payoff_equal: u64,
    cutoff_equal: u64,
    interval: Counts,
    teachers: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClipPicksReport {
    pub records: u64,
    pub answered: u64,
    /// Truth picks that rebuild into a clip with no flags and so are scored.
    pub truth_picks: u64,
    /// Truth picks that do not (a different `--dna` than the run used); not scored.
    pub truth_unreproducible: u64,
    /// The answer's own quality, whatever the teacher chose.
    pub picks: u64,
    pub valid_rate: Rate,
    pub rule_pass_rate: Rate,
    /// Agreement with the teacher run(s) named here, not accuracy.
    pub teachers: Vec<String>,
    pub hook: Prf,
    pub payoff_given_hook: Rate,
    pub cutoff_given_hook: Rate,
    pub interval: Prf,
}

/// The first occurrence of each value, in order.
fn distinct(values: impl IntoIterator<Item = u32>) -> Vec<u32> {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .filter(|value| seen.insert(*value))
        .collect()
}

impl ClipAccumulator {
    /// Score one item. `answer` is `None` when the model gave none.
    pub fn add(
        &mut self,
        truth: &ClipTruth,
        answer: Option<&ClipPicksAnswer>,
    ) -> Result<(), MetricError> {
        self.records += 1;
        self.answered += u64::from(answer.is_some());
        self.teachers.insert(truth.teacher.clone());

        let reproduced = reproduce(
            &truth.picks,
            &truth.segments,
            &truth.pauses,
            truth.duration_ms,
        );
        self.truth_unreproducible += reproduced.unreproducible as u64;
        self.truth_picks += reproduced.kept.len() as u64;
        let truth_ids: Vec<PickIds> = reproduced
            .kept
            .iter()
            .map(|(index, _)| truth.picks[*index])
            .collect();
        let truth_intervals: Vec<(u64, u64)> = reproduced
            .kept
            .iter()
            .map(|(_, clip)| (clip.start_ms, clip.end_ms))
            .collect();

        let answer_picks: &[AnswerPick] = answer.map_or(&[], |answer| answer.picks.as_slice());
        let outcome = rebuild(
            answer_picks,
            &truth.segments,
            &truth.pauses,
            truth.duration_ms,
        );
        self.picks += answer_picks.len() as u64;
        self.valid += outcome.clips.len() as u64;
        self.rule_pass += outcome
            .clips
            .iter()
            .filter(|clip| clip.flags.is_empty())
            .count() as u64;

        let truth_hooks = distinct(truth_ids.iter().map(|ids| ids.hook_segment));
        let answer_hooks = distinct(answer_picks.iter().map(|pick| pick.hook_segment));
        let shared: Vec<u32> = truth_hooks
            .iter()
            .copied()
            .filter(|hook| answer_hooks.contains(hook))
            .collect();
        self.hook.add(Counts::from_matches(
            shared.len(),
            truth_hooks.len(),
            answer_hooks.len(),
        ));
        for hook in shared {
            let truth_pick = truth_ids.iter().find(|ids| ids.hook_segment == hook);
            let answer_pick = answer_picks.iter().find(|pick| pick.hook_segment == hook);
            if let (Some(truth_pick), Some(answer_pick)) = (truth_pick, answer_pick) {
                self.hook_matched += 1;
                self.payoff_equal +=
                    u64::from(truth_pick.payoff_segment == answer_pick.payoff_segment);
                self.cutoff_equal +=
                    u64::from(truth_pick.cutoff_segment == answer_pick.cutoff_segment);
            }
        }

        let answer_intervals: Vec<(u64, u64)> = outcome
            .clips
            .iter()
            .map(|clip| (clip.start_ms, clip.end_ms))
            .collect();
        let pairs = match_intervals(&truth_intervals, &answer_intervals, INTERVAL_IOU)?.len();
        self.interval.add(Counts::from_matches(
            pairs,
            truth_intervals.len(),
            answer_intervals.len(),
        ));
        Ok(())
    }

    pub fn report(&self) -> Result<ClipPicksReport, MetricError> {
        Ok(ClipPicksReport {
            records: self.records,
            answered: self.answered,
            truth_picks: self.truth_picks,
            truth_unreproducible: self.truth_unreproducible,
            picks: self.picks,
            valid_rate: Rate::new(self.valid, self.picks)?,
            rule_pass_rate: Rate::new(self.rule_pass, self.picks)?,
            teachers: self.teachers.iter().cloned().collect(),
            hook: self.hook.scores(),
            payoff_given_hook: Rate::new(self.payoff_equal, self.hook_matched)?,
            cutoff_given_hook: Rate::new(self.cutoff_equal, self.hook_matched)?,
            interval: self.interval.scores(),
        })
    }
}

// ---------------------------------------------------------------------------------
// edit_events

#[derive(Debug, Clone, Default)]
pub struct EditEventsAccumulator {
    records: u64,
    answered: u64,
    cuts: Counts,
    cut_errors: Vec<f64>,
    events: Counts,
    scale: Mean,
    curves: Mean,
    curve_pairs: u64,
    visual_events_unscored: u64,
    zoom_curves_unscored: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CutsReport {
    #[serde(flatten)]
    pub scores: Prf,
    pub mean_abs_error_ms: Option<f64>,
    pub median_abs_error_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ErrorReport {
    pub pairs: u64,
    pub mean_abs_error: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EditEventsReport {
    pub records: u64,
    pub answered: u64,
    pub cuts: CutsReport,
    pub visual_events: Prf,
    pub scale: ErrorReport,
    pub zoom_curves: ErrorReport,
    /// Records whose truth has no `visual_events` (the file predates schema 3).
    pub visual_events_unscored: u64,
    /// Records whose truth has no measured `zoom_curves`.
    pub zoom_curves_unscored: u64,
}

impl EditEventsAccumulator {
    pub fn add(
        &mut self,
        truth: &EditEventsTarget,
        answer: Option<&EditEventsTarget>,
        tolerances: &Tolerances,
    ) -> Result<(), MetricError> {
        self.records += 1;
        self.answered += u64::from(answer.is_some());
        let empty = EditEventsTarget::default();
        let answer = answer.unwrap_or(&empty);

        let truth_cuts: Vec<(u64, ())> = truth.cuts.iter().map(|cut| (cut.time_ms, ())).collect();
        let answer_cuts: Vec<(u64, ())> = answer.cuts.iter().map(|cut| (cut.time_ms, ())).collect();
        let pairs = match_times(&truth_cuts, &answer_cuts, tolerances.cut_ms);
        self.cuts.add(Counts::from_matches(
            pairs.len(),
            truth_cuts.len(),
            answer_cuts.len(),
        ));
        self.cut_errors
            .extend(pairs.iter().map(|pair| pair.distance_ms as f64));

        match &truth.visual_events {
            None => self.visual_events_unscored += 1,
            Some(truth_events) => {
                let no_events = Vec::new();
                let answer_events = answer.visual_events.as_ref().unwrap_or(&no_events);
                let truth_timed: Vec<_> = truth_events
                    .iter()
                    .map(|event| (event.time_ms, event.kind))
                    .collect();
                let answer_timed: Vec<_> = answer_events
                    .iter()
                    .map(|event| (event.time_ms, event.kind))
                    .collect();
                let pairs = match_times(&truth_timed, &answer_timed, tolerances.event_ms);
                self.events.add(Counts::from_matches(
                    pairs.len(),
                    truth_timed.len(),
                    answer_timed.len(),
                ));
                for pair in &pairs {
                    let truth_event = &truth_events[pair.truth];
                    let answer_event = &answer_events[pair.predicted];
                    if let (Some(a), Some(b)) = (truth_event.scale, answer_event.scale) {
                        self.scale.push("a scale", abs_error("a scale", a, b)?)?;
                    }
                }
                match &truth.zoom_curves {
                    None => self.zoom_curves_unscored += 1,
                    Some(truth_curves) => {
                        let no_curves = Vec::new();
                        let answer_curves = answer.zoom_curves.as_ref().unwrap_or(&no_curves);
                        for pair in &pairs {
                            let truth_bezier = curve_of(truth_curves, pair.truth);
                            let answer_bezier = curve_of(answer_curves, pair.predicted);
                            if let (Some(a), Some(b)) = (truth_bezier, answer_bezier) {
                                // x1 and x2 are fixed at 1/3 and 2/3; y1 and y2 are the curve.
                                for index in [1, 3] {
                                    self.curves.push(
                                        "a bezier value",
                                        abs_error("a bezier value", a[index], b[index])?,
                                    )?;
                                }
                                self.curve_pairs += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn report(&self) -> Result<EditEventsReport, MetricError> {
        let mut cut_error = Mean::default();
        for error in &self.cut_errors {
            cut_error.push("a cut timing error", *error)?;
        }
        Ok(EditEventsReport {
            records: self.records,
            answered: self.answered,
            cuts: CutsReport {
                scores: self.cuts.scores(),
                mean_abs_error_ms: cut_error.value(),
                median_abs_error_ms: median("a cut timing error", &self.cut_errors)?,
            },
            visual_events: self.events.scores(),
            scale: ErrorReport {
                pairs: self.scale.count(),
                mean_abs_error: self.scale.value(),
            },
            zoom_curves: ErrorReport {
                pairs: self.curve_pairs,
                mean_abs_error: self.curves.value(),
            },
            visual_events_unscored: self.visual_events_unscored,
            zoom_curves_unscored: self.zoom_curves_unscored,
        })
    }
}

/// The bezier of the curve for event `index`, when it has one.
fn curve_of(curves: &[crate::record::CurveTarget], index: usize) -> Option<[f64; 4]> {
    curves
        .iter()
        .find(|curve| curve.event_index as usize == index)
        .and_then(|curve| curve.bezier)
}

// ---------------------------------------------------------------------------------
// pacing

#[derive(Debug, Clone, Default)]
struct FieldAccumulator {
    pairs: u64,
    missing: u64,
    exact: u64,
    error: Mean,
}

impl FieldAccumulator {
    /// A truth of `None` is not scored; an answer of `None` is counted as missing.
    fn add(
        &mut self,
        what: &'static str,
        truth: Option<f64>,
        answer: Option<f64>,
    ) -> Result<(), MetricError> {
        let Some(truth) = truth else {
            return Ok(());
        };
        let Some(answer) = answer else {
            self.missing += 1;
            return Ok(());
        };
        self.error.push(what, abs_error(what, truth, answer)?)?;
        self.pairs += 1;
        self.exact += u64::from(truth == answer);
        Ok(())
    }

    fn report(&self) -> Result<FieldReport, MetricError> {
        Ok(FieldReport {
            pairs: self.pairs,
            missing: self.missing,
            mean_abs_error: self.error.value(),
            exact_rate: ratio(self.exact, self.pairs)?,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PacingAccumulator {
    records: u64,
    answered: u64,
    cut_count: FieldAccumulator,
    mean_shot_ms: FieldAccumulator,
    longest_shot_ms: FieldAccumulator,
    dead_air_count: FieldAccumulator,
    words_per_minute: FieldAccumulator,
    integrated_lufs: FieldAccumulator,
    payoff_cue_ms: FieldAccumulator,
    payoff_pairs: u64,
    payoff_correct: u64,
    payoff_missing: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldReport {
    pub pairs: u64,
    pub missing: u64,
    pub mean_abs_error: Option<f64>,
    pub exact_rate: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PacingReport {
    pub records: u64,
    pub answered: u64,
    pub cut_count: FieldReport,
    pub mean_shot_ms: FieldReport,
    pub longest_shot_ms: FieldReport,
    pub dead_air_count: FieldReport,
    pub words_per_minute: FieldReport,
    pub integrated_lufs: FieldReport,
    pub payoff_cue_ms: FieldReport,
    pub payoff_has_cue: PayoffReport,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PayoffReport {
    pub pairs: u64,
    pub missing: u64,
    pub accuracy: Option<f64>,
}

fn number(value: Option<u64>) -> Option<f64> {
    value.map(|value| value as f64)
}

impl PacingAccumulator {
    pub fn add(
        &mut self,
        truth: &PacingTarget,
        answer: Option<&PacingAnswer>,
    ) -> Result<(), MetricError> {
        self.records += 1;
        self.answered += u64::from(answer.is_some());
        let none = PacingAnswer::default();
        let answer = answer.unwrap_or(&none);
        self.cut_count.add(
            "cut_count",
            Some(truth.cut_count as f64),
            number(answer.cut_count),
        )?;
        self.mean_shot_ms.add(
            "mean_shot_ms",
            Some(truth.mean_shot_ms as f64),
            number(answer.mean_shot_ms),
        )?;
        self.longest_shot_ms.add(
            "longest_shot_ms",
            Some(truth.longest_shot_ms as f64),
            number(answer.longest_shot_ms),
        )?;
        self.dead_air_count.add(
            "dead_air_count",
            Some(truth.dead_air_count as f64),
            number(answer.dead_air_count),
        )?;
        self.words_per_minute.add(
            "words_per_minute",
            truth.words_per_minute,
            answer.words_per_minute,
        )?;
        self.integrated_lufs.add(
            "integrated_lufs",
            Some(truth.integrated_lufs),
            answer.integrated_lufs,
        )?;
        self.payoff_cue_ms.add(
            "payoff_cue_ms",
            number(truth.payoff_cue_ms),
            number(answer.payoff_cue_ms),
        )?;
        match answer.payoff_has_cue {
            None => self.payoff_missing += 1,
            Some(has_cue) => {
                self.payoff_pairs += 1;
                self.payoff_correct += u64::from(has_cue == truth.payoff_has_cue);
            }
        }
        Ok(())
    }

    pub fn report(&self) -> Result<PacingReport, MetricError> {
        Ok(PacingReport {
            records: self.records,
            answered: self.answered,
            cut_count: self.cut_count.report()?,
            mean_shot_ms: self.mean_shot_ms.report()?,
            longest_shot_ms: self.longest_shot_ms.report()?,
            dead_air_count: self.dead_air_count.report()?,
            words_per_minute: self.words_per_minute.report()?,
            integrated_lufs: self.integrated_lufs.report()?,
            payoff_cue_ms: self.payoff_cue_ms.report()?,
            payoff_has_cue: PayoffReport {
                pairs: self.payoff_pairs,
                missing: self.payoff_missing,
                accuracy: ratio(self.payoff_correct, self.payoff_pairs)?,
            },
        })
    }
}

// ---------------------------------------------------------------------------------
// the report and the two entry points

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TruthKind {
    Understanding,
    Dataset,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalReport {
    pub eval_schema_version: u32,
    pub truth_kind: TruthKind,
    /// The split scored (`train`, `val` or `all`), or `file` for one .jsonl; `null` for an `understanding.json`.
    pub split: Option<String>,
    /// Truth items: records, or 1 for an `understanding.json`.
    pub records: u64,
    /// Items with a usable answer.
    pub answered: u64,
    /// Items with no answer line.
    pub missing: u64,
    /// Items whose answer line could not be read; scored as no answer.
    pub unparseable: u64,
    /// Answer lines whose id is in no scored record (a record of the other split is
    /// one); ignored.
    pub extra_answers: u64,
    pub problems: Vec<String>,
    pub tolerances: Tolerances,
    pub clip_picks: Option<ClipPicksReport>,
    pub edit_events: Option<EditEventsReport>,
    pub pacing: Option<PacingReport>,
}

#[derive(Default)]
struct Problems {
    listed: Vec<String>,
    unlisted: u64,
}

impl Problems {
    fn push(&mut self, message: String) {
        if self.listed.len() < MAX_PROBLEMS {
            self.listed.push(message);
        } else {
            self.unlisted += 1;
        }
    }

    fn finish(mut self) -> Vec<String> {
        if self.unlisted > 0 {
            self.listed
                .push(format!("... and {} more not listed", self.unlisted));
        }
        self.listed
    }
}

/// The picks of a model's reply for an `understanding.json`: a `VlmReply` (what
/// `reelctl understand --replay` reads) or a bare `RawReply`.
pub fn picks_from_reply(file: &str, bytes: &[u8]) -> Result<Vec<AnswerPick>, DatasetError> {
    let parse_error = |message: String| DatasetError::Parse {
        file: file.to_string(),
        kind: "model reply",
        message,
    };
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| parse_error(err.to_string()))?;
    let reply = if value.get("reply").is_some() {
        serde_json::from_value::<VlmReply>(value)
            .map_err(|err| parse_error(err.to_string()))?
            .reply
    } else {
        serde_json::from_value(value).map_err(|err| parse_error(err.to_string()))?
    };
    Ok(reply
        .reel_candidates
        .iter()
        .map(|pick| AnswerPick {
            hook_segment: pick.hook_segment,
            payoff_segment: pick.payoff_segment,
            cutoff_segment: pick.cutoff_segment,
            start_segment: Some(pick.start_segment),
            end_segment: Some(pick.end_segment),
        })
        .collect())
}

/// Score a model's reply against one `understanding.json`. `dna` supplies the pauses
/// the clip rules need and must be the reel the run was made from.
pub fn eval_understanding(
    truth: &Understanding,
    dna: &EditDna,
    reply_file: &str,
    reply_bytes: &[u8],
) -> Result<EvalReport, DatasetError> {
    if truth.source.duration_ms != dna.duration_ms {
        return Err(DatasetError::Truth(format!(
            "the run's source duration is {} ms but the edit_dna.json's is {} ms: that is not the same video",
            truth.source.duration_ms, dna.duration_ms
        )));
    }
    let picks = picks_from_reply(reply_file, reply_bytes)?;
    let mut clips: Vec<_> = truth.clips.iter().filter(|c| c.flags.is_empty()).collect();
    clips.sort_by_key(|clip| clip.rank);
    let item = ClipTruth {
        segments: truth.segments.clone(),
        pauses: dna.silence_gaps.clone(),
        duration_ms: dna.duration_ms,
        picks: clips
            .iter()
            .map(|clip| PickIds {
                hook_segment: clip.hook.id,
                payoff_segment: clip.payoff.id,
                cutoff_segment: clip.last.id,
            })
            .collect(),
        teacher: truth.model.id.clone(),
    };
    let mut accumulator = ClipAccumulator::default();
    accumulator.add(&item, Some(&ClipPicksAnswer { picks }))?;
    Ok(EvalReport {
        eval_schema_version: EVAL_SCHEMA_VERSION,
        truth_kind: TruthKind::Understanding,
        split: None,
        records: 1,
        answered: 1,
        missing: 0,
        unparseable: 0,
        extra_answers: 0,
        problems: Vec::new(),
        tolerances: Tolerances::default(),
        clip_picks: Some(accumulator.report()?),
        edit_events: None,
        pacing: None,
    })
}

/// Read the records of a `train.jsonl` or `val.jsonl`.
pub fn read_records(file: &str, text: &str) -> Result<Vec<Record>, DatasetError> {
    let mut records = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record = Record::parse(line).map_err(|message| DatasetError::BadRecord {
            file: file.to_string(),
            line: index + 1,
            message,
        })?;
        records.push(record);
    }
    Ok(records)
}

/// Score the answers of a dataset: one JSON line `{"id": ..., "answer": ...}` for each
/// record answered. `split` only labels the report.
pub fn eval_dataset(
    records: &[Record],
    answers_text: &str,
    split: &str,
    tolerances: &Tolerances,
) -> Result<EvalReport, DatasetError> {
    if records.is_empty() {
        return Err(DatasetError::Truth(format!(
            "split {split} has no records: nothing to score"
        )));
    }
    let mut problems = Problems::default();
    let mut answers: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut unreadable_lines = 0u64;
    for (index, line) in answers_text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<AnswerLine>(line) {
            Ok(parsed) => {
                if answers.insert(parsed.id.clone(), parsed.answer).is_some() {
                    return Err(DatasetError::DuplicateAnswer(parsed.id));
                }
            }
            Err(err) => {
                unreadable_lines += 1;
                problems.push(format!("answers line {}: {err}", index + 1));
            }
        }
    }
    let known: BTreeSet<&str> = records.iter().map(|record| record.id.as_str()).collect();
    let extra_answers = answers
        .keys()
        .filter(|id| !known.contains(id.as_str()))
        .count() as u64;

    let mut clip = ClipAccumulator::default();
    let mut events = EditEventsAccumulator::default();
    let mut pacing = PacingAccumulator::default();
    let (mut seen_clip, mut seen_events, mut seen_pacing) = (false, false, false);
    let (mut answered, mut missing, mut unparseable) = (0u64, 0u64, 0u64);

    for record in records {
        let raw = answers.get(&record.id);
        if raw.is_none() {
            missing += 1;
        }
        // The answer is read as the task's shape; a line that is not is unusable.
        macro_rules! read_answer {
            ($type:ty) => {
                match raw {
                    None => None,
                    Some(value) => match serde_json::from_value::<$type>(value.clone()) {
                        Ok(parsed) => {
                            answered += 1;
                            Some(parsed)
                        }
                        Err(err) => {
                            unparseable += 1;
                            problems.push(format!("{}: {err}", record.id));
                            None
                        }
                    },
                }
            };
        }
        match (&record.task, &record.input, &record.target, &record.context) {
            (
                Task::ClipPicks,
                Input::Transcript(input),
                Target::ClipPicks(target),
                Context::ClipPicks { silence_gaps },
            ) => {
                let answer = read_answer!(ClipPicksAnswer);
                let teacher = record
                    .provenance
                    .understanding
                    .as_ref()
                    .map_or_else(String::new, |u| u.teacher_model.clone());
                clip.add(
                    &ClipTruth {
                        segments: input.segments.clone(),
                        pauses: silence_gaps.clone(),
                        duration_ms: input.duration_ms,
                        picks: target.picks.clone(),
                        teacher,
                    },
                    answer.as_ref(),
                )?;
                seen_clip = true;
            }
            (Task::EditEvents, _, Target::EditEvents(target), _) => {
                let answer = read_answer!(EditEventsTarget);
                events.add(target, answer.as_ref(), tolerances)?;
                seen_events = true;
            }
            (Task::Pacing, _, Target::Pacing(target), _) => {
                let answer = read_answer!(PacingAnswer);
                pacing.add(target, answer.as_ref())?;
                seen_pacing = true;
            }
            _ => {
                return Err(DatasetError::Truth(format!(
                    "record {} does not have the parts its task needs",
                    record.id
                )))
            }
        }
    }

    Ok(EvalReport {
        eval_schema_version: EVAL_SCHEMA_VERSION,
        truth_kind: TruthKind::Dataset,
        split: Some(split.to_string()),
        records: records.len() as u64,
        answered,
        missing,
        unparseable: unparseable + unreadable_lines,
        extra_answers,
        problems: problems.finish(),
        tolerances: *tolerances,
        clip_picks: seen_clip.then(|| clip.report()).transpose()?,
        edit_events: seen_events.then(|| events.report()).transpose()?,
        pacing: seen_pacing.then(|| pacing.report()).transpose()?,
    })
}

fn show(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_string(), |value| format!("{value:.3}"))
}

fn show_prf(name: &str, prf: &Prf) -> String {
    format!(
        "  {name}: precision {} recall {} f1 {} (tp {} fp {} fn {})\n",
        show(prf.precision),
        show(prf.recall),
        show(prf.f1),
        prf.tp,
        prf.fp,
        prf.fn_
    )
}

fn show_rate(name: &str, rate: &Rate) -> String {
    format!(
        "  {name}: {} ({} of {})\n",
        show(rate.rate),
        rate.numerator,
        rate.denominator
    )
}

fn show_field(name: &str, field: &FieldReport) -> String {
    format!(
        "  {name}: mean abs error {} exact {} (pairs {}, missing {})\n",
        show(field.mean_abs_error),
        show(field.exact_rate),
        field.pairs,
        field.missing
    )
}

/// A readable report: `--format text`.
pub fn render_text(report: &EvalReport) -> String {
    let mut out = String::new();
    let truth = match report.truth_kind {
        TruthKind::Understanding => "understanding.json".to_string(),
        TruthKind::Dataset => format!("dataset, split {}", report.split.as_deref().unwrap_or("?")),
    };
    out.push_str(&format!(
        "truth: {truth}\nitems {} | answered {} | missing {} | unparseable {} | extra answers {}\n",
        report.records, report.answered, report.missing, report.unparseable, report.extra_answers
    ));
    if let Some(clip) = &report.clip_picks {
        out.push_str(&format!(
            "clip_picks ({} answered of {}; teacher: {}; {} truth picks scored, {} not reproducible)\n",
            clip.answered,
            clip.records,
            clip.teachers.join(", "),
            clip.truth_picks,
            clip.truth_unreproducible
        ));
        out.push_str(&format!("  picks made: {}\n", clip.picks));
        out.push_str(&show_rate("valid_rate", &clip.valid_rate));
        out.push_str(&show_rate("rule_pass_rate", &clip.rule_pass_rate));
        out.push_str(&show_prf("hook agreement", &clip.hook));
        out.push_str(&show_rate("payoff given hook", &clip.payoff_given_hook));
        out.push_str(&show_rate("cutoff given hook", &clip.cutoff_given_hook));
        out.push_str(&show_prf("interval agreement (IoU 0.5)", &clip.interval));
    }
    if let Some(events) = &report.edit_events {
        out.push_str(&format!(
            "edit_events ({} answered of {})\n",
            events.answered, events.records
        ));
        out.push_str(&show_prf("cuts", &events.cuts.scores));
        out.push_str(&format!(
            "  cut timing error ms: mean {} median {}\n",
            show(events.cuts.mean_abs_error_ms),
            show(events.cuts.median_abs_error_ms)
        ));
        out.push_str(&show_prf("visual_events", &events.visual_events));
        out.push_str(&format!(
            "  scale mean abs error {} (pairs {}); bezier mean abs error {} (pairs {}); unscored: {} records without events, {} without curves\n",
            show(events.scale.mean_abs_error),
            events.scale.pairs,
            show(events.zoom_curves.mean_abs_error),
            events.zoom_curves.pairs,
            events.visual_events_unscored,
            events.zoom_curves_unscored
        ));
    }
    if let Some(pacing) = &report.pacing {
        out.push_str(&format!(
            "pacing ({} answered of {})\n",
            pacing.answered, pacing.records
        ));
        out.push_str(&show_field("cut_count", &pacing.cut_count));
        out.push_str(&show_field("mean_shot_ms", &pacing.mean_shot_ms));
        out.push_str(&show_field("longest_shot_ms", &pacing.longest_shot_ms));
        out.push_str(&show_field("dead_air_count", &pacing.dead_air_count));
        out.push_str(&show_field("words_per_minute", &pacing.words_per_minute));
        out.push_str(&show_field("integrated_lufs", &pacing.integrated_lufs));
        out.push_str(&show_field("payoff_cue_ms", &pacing.payoff_cue_ms));
        out.push_str(&format!(
            "  payoff_has_cue accuracy {} (pairs {}, missing {})\n",
            show(pacing.payoff_has_cue.accuracy),
            pacing.payoff_has_cue.pairs,
            pacing.payoff_has_cue.missing
        ));
    }
    for problem in &report.problems {
        out.push_str(&format!("problem: {problem}\n"));
    }
    out
}
