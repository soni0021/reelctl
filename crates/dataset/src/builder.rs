//! Turn `edit_dna.json` and `understanding.json` bytes into records, a manifest, and the
//! text of the JSONL files. Pure: the caller reads and writes the files.
//!
//! Output depends on the contents of the inputs and their file names only, never on
//! their order: records are sorted and the split comes from the reel id.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;

use diff::{reel_metrics, NamedReel};
use edit_dna::{CurveStatus, EditDna};
use understand::{Understanding, SCHEMA_VERSION as UNDERSTANDING_SCHEMA_VERSION};

use crate::clips::reproduce;
use crate::error::DatasetError;
use crate::manifest::{
    FileInfo, Manifest, ManifestFiles, ManifestReel, Skip, SkipReason, SplitCounts,
};
use crate::record::{
    ClipPicksTarget, Context, CurveTarget, CutTarget, EditEventsTarget, EventTarget, Input,
    MediaFacts, PacingTarget, PickIds, Provenance, Record, Target, Task, TranscriptInput,
    UnderstandingProvenance, DATASET_SCHEMA_VERSION, TOOL, TOOL_VERSION,
};
use crate::sha256;
use crate::split::{check_val_percent, is_valid_reel_id, split_for, Split};

/// One input file: its name (no directory) and its bytes.
#[derive(Debug, Clone)]
pub struct FileInput {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub salt: String,
    pub val_percent: u32,
}

/// Everything `dataset build` writes.
#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub train_jsonl: String,
    pub val_jsonl: String,
    pub manifest: Manifest,
}

impl Output {
    /// `manifest.json`, pretty printed with a final newline.
    pub fn manifest_json(&self) -> Result<String, DatasetError> {
        let mut text = serde_json::to_string_pretty(&self.manifest)
            .map_err(|err| DatasetError::Serialize(err.to_string()))?;
        text.push('\n');
        Ok(text)
    }
}

const DNA_SUFFIX: &str = ".edit_dna.json";

/// The reel id of an `edit_dna.json` file: its name without `.edit_dna.json`, else
/// without `.json`.
pub fn reel_id_from_dna_name(file_name: &str) -> String {
    file_name
        .strip_suffix(DNA_SUFFIX)
        .or_else(|| file_name.strip_suffix(".json"))
        .unwrap_or(file_name)
        .to_string()
}

/// The reel id an `understanding.json` belongs to: the file stem of `source.path`, with
/// `/` and `\` both separating directories.
pub fn reel_id_from_source_path(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_string(),
        _ => name.to_string(),
    }
}

/// Parse `bytes` as JSON, check `schema_version` against `supported` before reading
/// anything else (a newer file would otherwise parse by ignoring what it added), and
/// return the value.
fn read_versioned(
    file: &str,
    kind: &'static str,
    bytes: &[u8],
    supported: RangeInclusive<u64>,
) -> Result<serde_json::Value, DatasetError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|err| DatasetError::Parse {
            file: file.to_string(),
            kind,
            message: err.to_string(),
        })?;
    let supported_text = if supported.start() == supported.end() {
        supported.start().to_string()
    } else {
        format!("{} to {}", supported.start(), supported.end())
    };
    let version = value.get("schema_version");
    match version.and_then(serde_json::Value::as_u64) {
        Some(found) if supported.contains(&found) => Ok(value),
        _ => Err(DatasetError::UnknownSchemaVersion {
            file: file.to_string(),
            found: version.map_or_else(|| "missing".to_string(), ToString::to_string),
            supported: supported_text,
        }),
    }
}

/// An `edit_dna.json`, refused if its `schema_version` is not one this build knows.
pub fn parse_edit_dna(file: &str, bytes: &[u8]) -> Result<EditDna, DatasetError> {
    let value = read_versioned(
        file,
        "edit_dna.json",
        bytes,
        1..=u64::from(edit_dna::SCHEMA_VERSION),
    )?;
    serde_json::from_value(value).map_err(|err| DatasetError::Parse {
        file: file.to_string(),
        kind: "edit_dna.json",
        message: err.to_string(),
    })
}

/// An `understanding.json`, refused if its `schema_version` is not this build's.
pub fn parse_understanding(file: &str, bytes: &[u8]) -> Result<Understanding, DatasetError> {
    let version = u64::from(UNDERSTANDING_SCHEMA_VERSION);
    let value = read_versioned(file, "understanding.json", bytes, version..=version)?;
    serde_json::from_value(value).map_err(|err| DatasetError::Parse {
        file: file.to_string(),
        kind: "understanding.json",
        message: err.to_string(),
    })
}

/// A reel that is going to get records.
struct Reel {
    id: String,
    dna: EditDna,
    sha256: String,
    split: Split,
}

#[derive(Default)]
struct Skips {
    list: Vec<Skip>,
}

impl Skips {
    fn add(&mut self, input: &str, task: Option<Task>, reason: SkipReason, detail: String) {
        self.list.push(Skip {
            input: input.to_string(),
            task,
            reason,
            detail,
        });
    }
}

fn all_finite(values: &[f64]) -> bool {
    values.iter().all(|value| value.is_finite())
}

fn record_id(task: Task, reel_id: &str, parts: &[&str]) -> String {
    let version = DATASET_SCHEMA_VERSION.to_string();
    let mut hasher = sha256::Sha256::new();
    for part in [version.as_str(), task.as_str(), reel_id]
        .into_iter()
        .chain(parts.iter().copied())
    {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    let hash = sha256::to_hex(&hasher.finish());
    format!("{}-{}-{}", task.as_str(), reel_id, &hash[..12])
}

fn provenance(reel: &Reel, understanding: Option<UnderstandingProvenance>) -> Provenance {
    Provenance {
        tool: TOOL.to_string(),
        tool_version: TOOL_VERSION.to_string(),
        reel_id: reel.id.clone(),
        edit_dna_schema_version: reel.dna.schema_version,
        edit_dna_sha256: reel.sha256.clone(),
        cut_detector: reel.dna.cut_detector.as_str().to_string(),
        asr_backend: reel.dna.asr_backend.as_str().to_string(),
        understanding,
    }
}

fn media_facts(dna: &EditDna) -> MediaFacts {
    MediaFacts {
        duration_ms: dna.duration_ms,
        fps: dna.fps,
        frame_size: dna.frame_size,
    }
}

/// The `edit_events` target, or the reason a number in it cannot be used.
pub(crate) fn edit_events_target(dna: &EditDna) -> Result<EditEventsTarget, String> {
    let mut cuts: Vec<CutTarget> = dna
        .cuts
        .iter()
        .map(|cut| CutTarget {
            time_ms: cut.time_ms,
        })
        .collect();
    cuts.sort_by_key(|cut| cut.time_ms);

    // Before schema 3 there was no motion detector: no events is "not measured".
    let visual_events = (dna.schema_version >= 3).then(|| {
        dna.visual_events
            .iter()
            .map(|event| EventTarget {
                time_ms: event.time_ms,
                kind: event.kind,
                scale: event.scale,
                area: event.area,
            })
            .collect::<Vec<_>>()
    });
    if let Some(events) = &visual_events {
        for event in events {
            if !all_finite(&[event.scale.unwrap_or(0.0), event.area.unwrap_or(0.0)]) {
                return Err(format!("the visual event at {} ms", event.time_ms));
            }
        }
    }

    let curves_measured =
        dna.schema_version >= 4 && dna.zoom_curves.status == CurveStatus::Measured;
    let zoom_curves = curves_measured.then(|| {
        dna.zoom_curves
            .curves
            .iter()
            .map(|curve| CurveTarget {
                event_index: curve.event_index,
                bezier: curve.bezier,
            })
            .collect::<Vec<_>>()
    });
    if let Some(curves) = &zoom_curves {
        for curve in curves {
            if !curve.bezier.is_none_or(|bezier| all_finite(&bezier)) {
                return Err(format!("the zoom curve of event {}", curve.event_index));
            }
        }
    }
    Ok(EditEventsTarget {
        cuts,
        visual_events,
        zoom_curves,
    })
}

pub(crate) fn pacing_target(reel_id: &str, dna: &EditDna) -> Result<PacingTarget, String> {
    let metrics = reel_metrics(NamedReel { name: reel_id, dna });
    // Under `silence_bursts` the figure counts voiced bursts: it is not a word rate.
    let words_per_minute =
        (dna.asr_backend == edit_dna::AsrBackend::WhisperCpp).then_some(dna.words_per_minute);
    if !all_finite(&[metrics.integrated_lufs, words_per_minute.unwrap_or(0.0)]) {
        return Err("integrated_lufs or words_per_minute".to_string());
    }
    Ok(PacingTarget {
        cut_count: metrics.cut_count as u64,
        mean_shot_ms: metrics.mean_shot_ms,
        longest_shot_ms: metrics.longest_shot_ms,
        dead_air_count: metrics.dead_air_count as u64,
        words_per_minute,
        integrated_lufs: metrics.integrated_lufs,
        payoff_has_cue: dna.payoff_window.has_cue,
        payoff_cue_ms: dna.payoff_window.cue_ms,
    })
}

fn media_record(task: Task, reel: &Reel, target: Target) -> Record {
    Record {
        dataset_schema_version: DATASET_SCHEMA_VERSION,
        id: record_id(task, &reel.id, &[&reel.sha256]),
        task,
        reel_id: reel.id.clone(),
        split: reel.split,
        trainable: task.trainable(),
        instruction: task.instruction().to_string(),
        input: Input::Media(media_facts(&reel.dna)),
        target,
        context: Context::Empty {},
        provenance: provenance(reel, None),
    }
}

/// Build the records, the manifest and the text of the JSONL files.
pub fn build(
    dnas: &[FileInput],
    understandings: &[FileInput],
    options: &BuildOptions,
) -> Result<Output, DatasetError> {
    check_val_percent(options.val_percent)?;
    let mut skips = Skips::default();

    // Inputs in name order, so a duplicate-understanding skip names the same file
    // whatever order they were given in.
    let mut dna_files: Vec<&FileInput> = dnas.iter().collect();
    dna_files.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let mut reels: BTreeMap<String, Reel> = BTreeMap::new();
    for file in dna_files {
        let dna = parse_edit_dna(&file.file_name, &file.bytes)?;
        let id = reel_id_from_dna_name(&file.file_name);
        if !is_valid_reel_id(&id) {
            skips.add(
                &file.file_name,
                None,
                SkipReason::BadReelId,
                "the reel id must be 1 to 96 characters of A-Z a-z 0-9 . _ - and not start with a dot"
                    .to_string(),
            );
            continue;
        }
        let split = split_for(&id, &options.salt, options.val_percent);
        let reel = Reel {
            sha256: sha256::hex(&file.bytes),
            split,
            dna,
            id: id.clone(),
        };
        if reels.insert(id.clone(), reel).is_some() {
            return Err(DatasetError::DuplicateReelId(id));
        }
    }

    let mut records: Vec<Record> = Vec::new();
    for reel in reels.values() {
        if reel.dna.duration_ms == 0 {
            skips.add(
                &reel.id,
                None,
                SkipReason::ZeroDuration,
                "duration_ms is 0".to_string(),
            );
            continue;
        }
        if !reel.dna.fps.is_finite() {
            skips.add(
                &reel.id,
                None,
                SkipReason::NonFiniteValue,
                "fps".to_string(),
            );
            continue;
        }
        match edit_events_target(&reel.dna) {
            Ok(target) => records.push(media_record(
                Task::EditEvents,
                reel,
                Target::EditEvents(target),
            )),
            Err(detail) => skips.add(
                &reel.id,
                Some(Task::EditEvents),
                SkipReason::NonFiniteValue,
                detail,
            ),
        }
        match pacing_target(&reel.id, &reel.dna) {
            Ok(target) => records.push(media_record(Task::Pacing, reel, Target::Pacing(target))),
            Err(detail) => skips.add(
                &reel.id,
                Some(Task::Pacing),
                SkipReason::NonFiniteValue,
                detail,
            ),
        }
    }

    let mut flagged_clips_left_out = 0u64;
    let mut understanding_files: Vec<&FileInput> = understandings.iter().collect();
    understanding_files.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    let mut seen_hashes: BTreeSet<String> = BTreeSet::new();
    for file in understanding_files {
        let understanding = parse_understanding(&file.file_name, &file.bytes)?;
        let hash = sha256::hex(&file.bytes);
        if !seen_hashes.insert(hash.clone()) {
            skips.add(
                &file.file_name,
                Some(Task::ClipPicks),
                SkipReason::DuplicateUnderstanding,
                "the same bytes were given as another --understanding file".to_string(),
            );
            continue;
        }
        let reel_id = reel_id_from_source_path(&understanding.source.path);
        let Some(reel) = reels.get(&reel_id).filter(|reel| reel.dna.duration_ms > 0) else {
            skips.add(
                &file.file_name,
                Some(Task::ClipPicks),
                SkipReason::NoMatchingEditDna,
                format!("no usable edit_dna.json with reel id {reel_id:?} was given"),
            );
            continue;
        };
        if understanding.source.duration_ms != reel.dna.duration_ms {
            skips.add(
                &file.file_name,
                Some(Task::ClipPicks),
                SkipReason::DurationMismatch,
                format!(
                    "the run's source duration is {} ms, the edit_dna.json's is {} ms",
                    understanding.source.duration_ms, reel.dna.duration_ms
                ),
            );
            continue;
        }
        match clip_picks_record(reel, &understanding, &hash, &mut flagged_clips_left_out) {
            Ok(record) => records.push(record),
            Err((reason, detail)) => {
                skips.add(&file.file_name, Some(Task::ClipPicks), reason, detail)
            }
        }
    }

    records.sort_by(|a, b| (a.task, &a.reel_id, &a.id).cmp(&(b.task, &b.reel_id, &b.id)));
    assemble(options, reels, records, skips, flagged_clips_left_out)
}

fn clip_picks_record(
    reel: &Reel,
    understanding: &Understanding,
    understanding_sha256: &str,
    flagged_clips_left_out: &mut u64,
) -> Result<Record, (SkipReason, String)> {
    let mut clips: Vec<_> = understanding.clips.iter().collect();
    clips.sort_by_key(|clip| clip.rank);
    let (kept, flagged): (Vec<_>, Vec<_>) =
        clips.into_iter().partition(|clip| clip.flags.is_empty());
    *flagged_clips_left_out += flagged.len() as u64;
    if kept.is_empty() {
        return Err((
            SkipReason::NoRuleCompliantClips,
            format!(
                "{} clips, {} with flags, {} rejected",
                understanding.clips.len(),
                flagged.len(),
                understanding.rejected.len()
            ),
        ));
    }
    let segment_ids: BTreeSet<u32> = understanding.segments.iter().map(|seg| seg.id).collect();
    let mut picks = Vec::with_capacity(kept.len());
    for clip in kept {
        let ids = PickIds {
            hook_segment: clip.hook.id,
            payoff_segment: clip.payoff.id,
            cutoff_segment: clip.last.id,
        };
        for id in [ids.hook_segment, ids.payoff_segment, ids.cutoff_segment] {
            if !segment_ids.contains(&id) {
                return Err((
                    SkipReason::InconsistentUnderstanding,
                    format!(
                        "clip {} names segment {id}, which the run does not have",
                        clip.rank
                    ),
                ));
            }
        }
        picks.push(ids);
    }
    let rebuilt = reproduce(
        &picks,
        &understanding.segments,
        &reel.dna.silence_gaps,
        reel.dna.duration_ms,
    );
    if rebuilt.unreproducible > 0 {
        return Err((
            SkipReason::ClipsNotReproducible,
            format!(
                "{} of {} kept clips break a rule when rebuilt from this edit_dna.json's pauses",
                rebuilt.unreproducible,
                picks.len()
            ),
        ));
    }
    Ok(Record {
        dataset_schema_version: DATASET_SCHEMA_VERSION,
        id: record_id(
            Task::ClipPicks,
            &reel.id,
            &[&reel.sha256, understanding_sha256],
        ),
        task: Task::ClipPicks,
        reel_id: reel.id.clone(),
        split: reel.split,
        trainable: Task::ClipPicks.trainable(),
        instruction: Task::ClipPicks.instruction().to_string(),
        input: Input::Transcript(TranscriptInput {
            duration_ms: reel.dna.duration_ms,
            segments: understanding.segments.clone(),
        }),
        target: Target::ClipPicks(ClipPicksTarget { picks }),
        context: Context::ClipPicks {
            silence_gaps: reel.dna.silence_gaps.clone(),
        },
        provenance: provenance(
            reel,
            Some(UnderstandingProvenance {
                schema_version: understanding.schema_version,
                sha256: understanding_sha256.to_string(),
                teacher_model: understanding.model.id.clone(),
                teacher_provider: understanding.model.provider.clone(),
            }),
        ),
    })
}

/// Render the records of each split and the manifest.
fn assemble(
    options: &BuildOptions,
    reels: BTreeMap<String, Reel>,
    records: Vec<Record>,
    skips: Skips,
    flagged_clips_left_out: u64,
) -> Result<Output, DatasetError> {
    let mut train_jsonl = String::new();
    let mut val_jsonl = String::new();
    let mut counts: BTreeMap<String, SplitCounts> = Task::ALL
        .iter()
        .map(|task| (task.as_str().to_string(), SplitCounts::default()))
        .collect();
    let (mut train_records, mut val_records) = (0u64, 0u64);
    for record in &records {
        let line = serde_json::to_string(record)
            .map_err(|err| DatasetError::Serialize(err.to_string()))?;
        let entry = counts.entry(record.task.as_str().to_string()).or_default();
        let (text, total, per_task) = match record.split {
            Split::Train => (&mut train_jsonl, &mut train_records, &mut entry.train),
            Split::Val => (&mut val_jsonl, &mut val_records, &mut entry.val),
        };
        text.push_str(&line);
        text.push('\n');
        *total += 1;
        *per_task += 1;
    }

    let mut warnings = Vec::new();
    for (name, total) in [("train", train_records), ("val", val_records)] {
        if total == 0 {
            warnings.push(format!(
                "the {name} split has no records ({} reels, --val-percent {})",
                reels.len(),
                options.val_percent
            ));
        }
    }
    let mut skipped = skips.list;
    skipped.sort();

    let manifest = Manifest {
        dataset_schema_version: DATASET_SCHEMA_VERSION,
        tool_version: TOOL_VERSION.to_string(),
        salt: options.salt.clone(),
        val_percent: options.val_percent,
        reels: reels
            .values()
            .map(|reel| ManifestReel {
                reel_id: reel.id.clone(),
                split: reel.split,
                edit_dna_sha256: reel.sha256.clone(),
            })
            .collect(),
        files: ManifestFiles {
            train: FileInfo {
                records: train_records,
                sha256: sha256::hex(train_jsonl.as_bytes()),
            },
            val: FileInfo {
                records: val_records,
                sha256: sha256::hex(val_jsonl.as_bytes()),
            },
        },
        records: counts,
        skipped,
        flagged_clips_left_out,
        warnings,
    };
    Ok(Output {
        train_jsonl,
        val_jsonl,
        manifest,
    })
}
