//! `reelctl dataset build` and `reelctl eval`: instruction-tuning records from decompiled
//! reels, and a scorer for a model's answers. Offline: nothing here calls a model or the
//! network (`docs/specs/p3-dataset-eval.md`). The logic is in `crates/dataset`; this file
//! reads and writes the files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use dataset::builder::{
    build, parse_edit_dna, parse_understanding, BuildOptions, FileInput, Output,
};
use dataset::eval::{
    eval_dataset, eval_understanding, read_records, render_text, EvalReport, Tolerances,
    DEFAULT_CUT_TOLERANCE_MS, DEFAULT_EVENT_TOLERANCE_MS, INTERVAL_IOU,
};
use dataset::{DatasetError, Split, DEFAULT_SALT, DEFAULT_VAL_PERCENT};

use crate::understand_cmd::ExitWith;

const DNA_SUFFIX: &str = ".edit_dna.json";

#[derive(Args)]
pub struct DatasetCommand {
    #[command(subcommand)]
    command: DatasetSub,
}

#[derive(Subcommand)]
enum DatasetSub {
    /// Write train.jsonl, val.jsonl and manifest.json from edit_dna.json files and
    /// understanding.json runs. Targets are measured facts only.
    Build(BuildArgs),
}

#[derive(Args)]
struct BuildArgs {
    /// A directory (every `*.edit_dna.json` directly inside it) or edit_dna.json
    /// files. Give it as often as needed.
    #[arg(long, required = true)]
    dna: Vec<PathBuf>,
    /// understanding.json files from `reelctl understand`. A run joins the reel whose id
    /// is the file stem of its `source.path`.
    #[arg(long)]
    understanding: Vec<PathBuf>,
    /// Directory for train.jsonl, val.jsonl and manifest.json.
    #[arg(long)]
    out: PathBuf,
    /// Share of reels in `val`, in percent (0 to 50). The split is by reel.
    #[arg(long, default_value_t = DEFAULT_VAL_PERCENT)]
    val_percent: u32,
    /// Salt of the split. Changing it reshuffles every reel.
    #[arg(long, default_value = DEFAULT_SALT)]
    salt: String,
}

#[derive(Args)]
pub struct EvalArgs {
    /// An understanding.json (also needs --dna), or a dataset: a directory written by
    /// `dataset build`, or one of its .jsonl files.
    #[arg(long)]
    truth: PathBuf,
    /// For an understanding.json: a model reply (what `understand --replay` reads).
    /// For a dataset: JSON lines `{"id": ..., "answer": ...}`.
    #[arg(long)]
    answers: PathBuf,
    /// The edit_dna.json of the video an understanding.json was made from: the clip
    /// rules need its measured pauses.
    #[arg(long)]
    dna: Option<PathBuf>,
    /// Which records of a dataset to score. Ignored for an understanding.json.
    #[arg(long, value_enum, default_value_t = SplitArg::Val)]
    split: SplitArg,
    /// Cut times this close (inclusive) are the same cut.
    #[arg(long, default_value_t = DEFAULT_CUT_TOLERANCE_MS)]
    cut_tolerance_ms: u64,
    /// Visual events of one kind this close (inclusive) are the same event.
    #[arg(long, default_value_t = DEFAULT_EVENT_TOLERANCE_MS)]
    event_tolerance_ms: u64,
    /// How to print the report.
    #[arg(long, value_enum, default_value_t = FormatArg::Json)]
    format: FormatArg,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SplitArg {
    Val,
    Train,
    All,
}

impl SplitArg {
    fn name(self) -> &'static str {
        match self {
            Self::Val => "val",
            Self::Train => "train",
            Self::All => "all",
        }
    }

    fn keeps(self, split: Split) -> bool {
        match self {
            Self::All => true,
            Self::Val => split == Split::Val,
            Self::Train => split == Split::Train,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FormatArg {
    Json,
    Text,
}

/// An error that exits with code 2.
fn usage(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ExitWith {
        code: 2,
        message: message.into(),
    })
}

/// A library error: exit 2 for an argument, else 1.
fn library(error: DatasetError) -> anyhow::Error {
    match error {
        DatasetError::ValPercent(_) => usage(error.to_string()),
        other => anyhow::Error::new(other),
    }
}

pub fn run_dataset(command: DatasetCommand) -> Result<()> {
    match command.command {
        DatasetSub::Build(args) => build_cmd(args),
    }
}

fn read_input(path: &Path) -> Result<FileInput> {
    let metadata = fs::metadata(path).with_context(|| format!("read {}", path.display()))?;
    if !metadata.is_file() {
        return Err(usage(format!("{} is not a regular file", path.display())));
    }
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| usage(format!("{} has no file name", path.display())))?;
    Ok(FileInput { file_name, bytes })
}

/// The edit_dna.json files named by `--dna`: a directory contributes every regular
/// file directly inside it whose name ends `.edit_dna.json`.
fn dna_inputs(paths: &[PathBuf]) -> Result<Vec<FileInput>> {
    let mut inputs = Vec::new();
    for path in paths {
        let metadata = fs::metadata(path).with_context(|| format!("read {}", path.display()))?;
        if !metadata.is_dir() {
            inputs.push(read_input(path)?);
            continue;
        }
        let mut found = Vec::new();
        for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
            let entry = entry.with_context(|| format!("read {}", path.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(DNA_SUFFIX) && entry.path().is_file() {
                found.push(entry.path());
            }
        }
        if found.is_empty() {
            return Err(usage(format!(
                "{} holds no file ending {DNA_SUFFIX}",
                path.display()
            )));
        }
        found.sort();
        for file in found {
            inputs.push(read_input(&file)?);
        }
    }
    Ok(inputs)
}

/// Write `contents` to `path` through a temporary file, so a failed run leaves the old
/// file whole.
fn write_file(path: &Path, contents: &str) -> Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = path.with_file_name(format!(".{name}.tmp"));
    fs::write(&temp, contents).with_context(|| format!("write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

fn write_output(out: &Path, output: &Output) -> Result<()> {
    if out.exists() && !out.is_dir() {
        return Err(usage(format!(
            "{} exists and is not a directory",
            out.display()
        )));
    }
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    let manifest = output.manifest_json().map_err(library)?;
    write_file(&out.join("train.jsonl"), &output.train_jsonl)?;
    write_file(&out.join("val.jsonl"), &output.val_jsonl)?;
    write_file(&out.join("manifest.json"), &manifest)?;
    Ok(())
}

fn build_cmd(args: BuildArgs) -> Result<()> {
    let dnas = dna_inputs(&args.dna)?;
    let understandings = args
        .understanding
        .iter()
        .map(|path| read_input(path))
        .collect::<Result<Vec<_>>>()?;
    let output = build(
        &dnas,
        &understandings,
        &BuildOptions {
            salt: args.salt,
            val_percent: args.val_percent,
        },
    )
    .map_err(library)?;
    write_output(&args.out, &output)?;

    for skip in &output.manifest.skipped {
        eprintln!(
            "skipped {}{}: {}: {}",
            skip.input,
            skip.task
                .map_or_else(String::new, |task| format!(" [{}]", task.as_str())),
            skip.reason.as_str(),
            skip.detail
        );
    }
    for warning in &output.manifest.warnings {
        eprintln!("warning: {warning}");
    }
    let files = &output.manifest.files;
    println!(
        "wrote {} records ({} train, {} val) from {} reels, {} skipped, to {}",
        files.train.records + files.val.records,
        files.train.records,
        files.val.records,
        output.manifest.reels.len(),
        output.manifest.skipped.len(),
        args.out.display()
    );
    Ok(())
}

fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("read {}", path.display()))
}

/// The records of a dataset directory (both files) or of one `.jsonl` file.
fn dataset_records(truth: &Path) -> Result<Vec<dataset::record::Record>> {
    let files: Vec<PathBuf> = if truth.is_dir() {
        vec![truth.join("train.jsonl"), truth.join("val.jsonl")]
    } else {
        vec![truth.to_path_buf()]
    };
    let mut records = Vec::new();
    for file in files {
        let name = file
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        records.extend(read_records(&name, &read_text(&file)?).map_err(library)?);
    }
    Ok(records)
}

pub fn run_eval(args: EvalArgs) -> Result<()> {
    let tolerances = Tolerances {
        cut_ms: args.cut_tolerance_ms,
        event_ms: args.event_tolerance_ms,
        interval_iou: INTERVAL_IOU,
    };
    let report: EvalReport = if args.truth.is_dir()
        || args.truth.extension().is_some_and(|ext| ext == "jsonl")
    {
        let records: Vec<_> = dataset_records(&args.truth)?
            .into_iter()
            .filter(|record| args.split.keeps(record.split))
            .collect();
        eval_dataset(
            &records,
            &read_text(&args.answers)?,
            args.split.name(),
            &tolerances,
        )
        .map_err(library)?
    } else {
        let Some(dna_path) = &args.dna else {
            return Err(usage(
                "an understanding.json needs --dna: the clip rules need the measured pauses of the same video",
            ));
        };
        let dna_input = read_input(dna_path)?;
        let dna = parse_edit_dna(&dna_input.file_name, &dna_input.bytes).map_err(library)?;
        let truth_input = read_input(&args.truth)?;
        let truth =
            parse_understanding(&truth_input.file_name, &truth_input.bytes).map_err(library)?;
        let answers = read_input(&args.answers)?;
        eval_understanding(&truth, &dna, &answers.file_name, &answers.bytes).map_err(library)?
    };
    match args.format {
        FormatArg::Json => println!(
            "{}",
            serde_json::to_string_pretty(&report).context("format report")?
        ),
        FormatArg::Text => print!("{}", render_text(&report)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("reelctl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn exit_code(error: &anyhow::Error) -> Option<i32> {
        error.downcast_ref::<ExitWith>().map(|exit| exit.code)
    }

    #[test]
    fn a_directory_contributes_only_edit_dna_files_in_name_order() {
        let dir = scratch("dataset-dir");
        for name in ["b.edit_dna.json", "a.edit_dna.json", "notes.json", "c.mp4"] {
            fs::write(dir.join(name), "{}").expect("write");
        }
        fs::create_dir(dir.join("sub.edit_dna.json")).expect("dir");
        let inputs = dna_inputs(&[dir]).expect("inputs");
        let names: Vec<_> = inputs.iter().map(|file| file.file_name.as_str()).collect();
        assert_eq!(names, vec!["a.edit_dna.json", "b.edit_dna.json"]);
    }

    #[test]
    fn a_directory_without_edit_dna_files_is_a_usage_error() {
        let dir = scratch("dataset-empty");
        let error = dna_inputs(&[dir]).expect_err("nothing to read");
        assert_eq!(exit_code(&error), Some(2));
    }

    #[test]
    fn a_missing_path_is_a_runtime_error() {
        let error = dna_inputs(&[PathBuf::from("/no/such/path.edit_dna.json")]).expect_err("gone");
        assert_eq!(exit_code(&error), None);
    }

    #[test]
    fn the_split_flag_keeps_the_records_of_its_split() {
        assert!(SplitArg::Val.keeps(Split::Val) && !SplitArg::Val.keeps(Split::Train));
        assert!(SplitArg::Train.keeps(Split::Train) && !SplitArg::Train.keeps(Split::Val));
        assert!(SplitArg::All.keeps(Split::Train) && SplitArg::All.keeps(Split::Val));
    }

    #[test]
    fn writing_replaces_the_files_and_leaves_no_temporary_file() {
        let dir = scratch("dataset-write");
        let path = dir.join("out.jsonl");
        write_file(&path, "old\n").expect("write");
        write_file(&path, "new\n").expect("write");
        assert_eq!(fs::read_to_string(&path).expect("read"), "new\n");
        let names: Vec<_> = fs::read_dir(&dir)
            .expect("dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, vec!["out.jsonl"]);
    }
}
