//! `reelctl make-reels`: one long video in, finished vertical reels out. It runs the
//! other commands in order, each as a child `reelctl` process with only the
//! environment that step needs, so each ONNX model gets its own process, a step's
//! failure stays in that step, and the API key reaches `understand` alone.
//!
//! Steps: decompile (with whisper), the cost plan, the user's yes, understand, then
//! for each chosen clip from-clip, cut-silence, auto-punch-in, reframe, the bridge
//! card, captions, render and `diff` against the outliers. Every intermediate
//! project is kept (`reel-<N>/NN-<step>.otio`), so any step can be re-run by hand.
//!
//! Exit codes: 0 when at least one reel rendered (or `--pick`), 1 when none did,
//! 2 for a bad argument or a step that refused its input, 3 without an API key,
//! 4 when the cost is over `--max-cost-usd` or the call was not confirmed.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Args;
use edit_dna::{AsrBackend, EditDna};
use serde_json::{json, Value};

use crate::overlay_cmd::CaptionStyleArgs;
use crate::timeline_cmd::usage;
use crate::understand_cmd::ExitWith;

/// The summary's schema version.
pub const MAKE_REELS_SCHEMA_VERSION: u32 = 1;
/// How much of a failed step's stderr is kept.
const STDERR_LINES: usize = 12;
/// Variables a child may need to find its libraries and temporary space. Nothing
/// else of this process's environment is passed on.
const PASS_THROUGH: [&str; 6] = [
    "PATH",
    "TMPDIR",
    "HOME",
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
];

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum FollowArg {
    Presenter,
    Centre,
}

#[derive(Args)]
pub struct MakeReelsArgs {
    /// The long video.
    #[arg(long)]
    input: PathBuf,
    /// An outlier reel's edit_dna.json, for pacing and the check; one to three.
    #[arg(long, required = true)]
    outlier: Vec<PathBuf>,
    /// The link the card at the end of each reel shows.
    #[arg(long)]
    target_url: String,
    /// Where everything is written.
    #[arg(long)]
    out_dir: PathBuf,
    /// The whisper model for the decompile step (needed unless --dna is given).
    #[arg(long, env = "REELCTL_WHISPER_MODEL")]
    whisper_model: Option<PathBuf>,
    /// The long video's edit_dna.json with whisper words, to skip decompiling.
    #[arg(long)]
    dna: Option<PathBuf>,
    /// RobustVideoMatting weights for --follow presenter (ONNX Runtime from
    /// ORT_DYLIB_PATH). Without them the crop stays centred, and the summary says so.
    #[arg(long, env = "REELCTL_RVM_WEIGHTS")]
    weights: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = FollowArg::Presenter)]
    follow: FollowArg,
    /// The model `understand` calls.
    #[arg(long, default_value = "qwen/qwen3.5-9b")]
    model: String,
    /// Stop (exit 4) when the model call could cost more than this, in US dollars.
    #[arg(long, default_value_t = 0.50)]
    max_cost_usd: f64,
    /// Make the model call without asking.
    #[arg(long)]
    yes: bool,
    /// Make at most this many reels.
    #[arg(long)]
    reels: Option<usize>,
    /// Make these clips (numbered as in understanding.json, from 1), comma-separated.
    #[arg(long, value_delimiter = ',')]
    clips: Vec<usize>,
    /// Use clips the clip rules flagged.
    #[arg(long)]
    allow_flagged: bool,
    /// Stop after understand: list the clips in make_reels.json and exit 0.
    #[arg(long)]
    pick: bool,
    /// Size of each reel.
    #[arg(long, default_value = "1080x1920")]
    size: String,
    /// Length of the card at the end.
    #[arg(long, default_value_t = 2500)]
    card_ms: u64,
    #[command(flatten)]
    style: CaptionStyleArgs,
    /// Internal: a recorded model reply for `understand --replay`.
    #[arg(long, hide = true)]
    understand_replay: Option<PathBuf>,
    /// Internal: a model listing for `understand --catalog`.
    #[arg(long, hide = true)]
    understand_catalog: Option<PathBuf>,
}

/// One child run.
#[derive(Debug, Clone)]
struct Step {
    name: String,
    argv: Vec<String>,
    code: Option<i32>,
    ms: u128,
    stdout: String,
    stderr: String,
}

impl Step {
    fn ok(&self) -> bool {
        self.code == Some(0)
    }

    fn tail(&self) -> String {
        let lines: Vec<&str> = self.stderr.lines().rev().take(STDERR_LINES).collect();
        lines.into_iter().rev().collect::<Vec<_>>().join("\n")
    }

    /// `key=value` from the stdout line that starts with `prefix`.
    fn field(&self, key: &str) -> Option<String> {
        self.stdout
            .split_whitespace()
            .find_map(|part| part.strip_prefix(&format!("{key}=")))
            .map(str::to_string)
    }

    fn record(&self) -> Value {
        json!({"name": self.name, "argv": self.argv, "code": self.code, "ms": self.ms as u64})
    }
}

fn os(path: &Path) -> OsString {
    path.as_os_str().to_os_string()
}

/// Run `reelctl <args>` with a clean environment plus `env`.
fn run(name: &str, args: Vec<OsString>, env: &[(&str, OsString)]) -> Result<Step> {
    let exe = std::env::current_exe().context("find the reelctl executable")?;
    let mut command = Command::new(exe);
    command.args(&args).env_clear().stdin(Stdio::null());
    for key in PASS_THROUGH {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for (key, value) in env {
        command.env(key, value);
    }
    let started = Instant::now();
    let output = command
        .output()
        .with_context(|| format!("run the {name} step"))?;
    Ok(Step {
        name: name.to_string(),
        argv: args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
        code: output.status.code(),
        ms: started.elapsed().as_millis(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// A step that must succeed for the whole run: its own exit code and words.
fn required(step: Step) -> Result<Step> {
    if step.ok() {
        return Ok(step);
    }
    let message = format!("the {} step failed:\n{}", step.name, step.tail());
    match step.code {
        Some(code @ 2..=4) => Err(anyhow::Error::new(ExitWith { code, message })),
        _ => Err(anyhow::anyhow!(message)),
    }
}

fn exit(code: i32, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(ExitWith {
        code,
        message: message.into(),
    })
}

fn read_json(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path)
        .map_err(|error| usage(format!("cannot read {}: {error}", path.display())))?;
    serde_json::from_str(&text).map_err(|error| usage(format!("{}: {error}", path.display())))
}

fn load_dna(path: &Path) -> Result<EditDna> {
    serde_json::from_value(read_json(path)?)
        .map_err(|error| usage(format!("{}: not an edit_dna.json: {error}", path.display())))
}

/// Ask on the terminal. Anything but `y` is no; no terminal is no.
fn confirm(question: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("{question} [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer).is_ok()
        && answer.trim().eq_ignore_ascii_case("y")
}

/// The clips to make: `--clips`, else every clip that passes the rules (or every
/// clip with `--allow-flagged`), then at most `--reels`.
fn choose(understanding: &Value, args: &MakeReelsArgs) -> Result<Vec<usize>> {
    let clips = understanding["clips"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let flagged = |index: usize| {
        clips[index - 1]["flags"]
            .as_array()
            .is_some_and(|flags| !flags.is_empty())
    };
    let mut chosen: Vec<usize> = if args.clips.is_empty() {
        (1..=clips.len())
            .filter(|&index| args.allow_flagged || !flagged(index))
            .collect()
    } else {
        for &index in &args.clips {
            if index == 0 || index > clips.len() {
                return Err(usage(format!(
                    "--clips {index}: understanding.json has {} clips (numbered from 1)",
                    clips.len()
                )));
            }
            if flagged(index) && !args.allow_flagged {
                return Err(usage(format!(
                    "clip {index} breaks the clip rules; pass --allow-flagged to make it anyway"
                )));
            }
        }
        args.clips.clone()
    };
    if let Some(limit) = args.reels {
        chosen.truncate(limit);
    }
    Ok(chosen)
}

/// The style flags as arguments for `overlay captions`.
fn style_args(style: &CaptionStyleArgs) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |flag: &str, value: Option<String>| {
        if let Some(value) = value {
            args.push(format!("--{flag}").into());
            args.push(value.into());
        }
    };
    push("preset", style.preset.clone());
    push(
        "style",
        style.style.as_ref().map(|path| path.display().to_string()),
    );
    push("font", style.font.clone());
    push("font-size", style.font_size.map(|v| v.to_string()));
    push("color", style.color.clone());
    push("highlight-color", style.highlight_color.clone());
    push("outline-color", style.outline_color.clone());
    push("outline-width", style.outline_width.map(|v| v.to_string()));
    push("shadow", style.shadow.map(|v| v.to_string()));
    push("box-color", style.box_color.clone());
    push("box-opacity", style.box_opacity.map(|v| v.to_string()));
    push("position", style.position.clone());
    push("margin", style.margin.map(|v| v.to_string()));
    push(
        "words-per-group",
        style.words_per_group.map(|v| v.to_string()),
    );
    push("max-chars", style.max_chars.map(|v| v.to_string()));
    push("case", style.case.clone());
    push("min-gap-ms", style.min_gap_ms.map(|v| v.to_string()));
    args
}

/// What one reel came to.
struct Reel {
    index: usize,
    status: &'static str,
    path: Option<PathBuf>,
    length_ms: Option<u64>,
    failure: Option<Value>,
    notes: Vec<String>,
    diff_flags: Vec<String>,
    steps: Vec<Step>,
}

fn make_reel(
    args: &MakeReelsArgs,
    index: usize,
    dna: &Path,
    understanding: &Path,
    reel_dir: &Path,
) -> Result<Reel> {
    fs::create_dir_all(reel_dir).with_context(|| format!("create {}", reel_dir.display()))?;
    let mut reel = Reel {
        index,
        status: "failed",
        path: None,
        length_ms: None,
        failure: None,
        notes: Vec::new(),
        diff_flags: Vec::new(),
        steps: Vec::new(),
    };
    let project = |n: usize, step: &str| reel_dir.join(format!("{n:02}-{step}.otio"));
    let outliers: Vec<OsString> = args
        .outlier
        .iter()
        .flat_map(|path| [OsString::from("--outlier"), os(path)])
        .collect();
    let mut previous = project(1, "from-clip");
    let mut plan: Vec<(&str, Vec<OsString>, Vec<(&str, OsString)>)> = vec![(
        "from-clip",
        [
            vec![
                "timeline".into(),
                "from-clip".into(),
                "--input".into(),
                os(&args.input),
            ],
            vec![
                "--dna".into(),
                os(dna),
                "--understanding".into(),
                os(understanding),
            ],
            vec![
                "--clip".into(),
                index.to_string().into(),
                "--out".into(),
                os(&previous),
            ],
            if args.allow_flagged {
                vec!["--allow-flagged".into()]
            } else {
                Vec::new()
            },
        ]
        .concat(),
        Vec::new(),
    )];
    let mut chain = |n: usize,
                     name: &'static str,
                     head: &[&str],
                     tail: Vec<OsString>,
                     env: Vec<(&'static str, OsString)>| {
        let out = project(n, name);
        let mut argv: Vec<OsString> = head.iter().map(OsString::from).collect();
        argv.extend(["--project".into(), os(&previous)]);
        argv.extend(tail);
        argv.extend(["--out".into(), os(&out)]);
        previous = out;
        plan.push((name, argv, env));
    };
    chain(
        2,
        "cut-silence",
        &["timeline", "cut-silence"],
        vec!["--dna".into(), os(dna)],
        Vec::new(),
    );
    chain(
        3,
        "auto-punch-in",
        &["transform", "auto-punch-in"],
        [vec!["--dna".into(), os(dna)], outliers.clone()].concat(),
        Vec::new(),
    );
    let mut reframe_env: Vec<(&'static str, OsString)> = Vec::new();
    if let Some(library) = std::env::var_os(compositor::LIBRARY_ENV) {
        reframe_env.push((compositor::LIBRARY_ENV, library));
    }
    let follow = match args.follow {
        FollowArg::Presenter => "presenter",
        FollowArg::Centre => "centre",
    };
    let mut reframe_tail: Vec<OsString> =
        vec!["--follow".into(), follow.into(), "--dna".into(), os(dna)];
    if let (FollowArg::Presenter, Some(weights)) = (args.follow, &args.weights) {
        reframe_tail.extend(["--weights".into(), os(weights)]);
    }
    chain(
        4,
        "reframe",
        &["transform", "reframe"],
        reframe_tail,
        reframe_env,
    );
    chain(
        5,
        "bridge",
        &["funnel", "attach-bridge"],
        vec![
            "--target-url".into(),
            args.target_url.clone().into(),
            "--append".into(),
            "--card-ms".into(),
            args.card_ms.to_string().into(),
        ],
        Vec::new(),
    );
    chain(
        6,
        "captions",
        &["overlay", "captions"],
        [vec!["--dna".into(), os(dna)], style_args(&args.style)].concat(),
        Vec::new(),
    );

    for (name, argv, env) in plan {
        let mut step = run(name, argv.clone(), &env)?;
        if name == "reframe"
            && !step.ok()
            && step.code == Some(2)
            && matches!(args.follow, FollowArg::Presenter)
        {
            // No model: keep the crop centred and say why.
            let reason = step
                .stderr
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string();
            reel.notes.push(format!(
                "reframe followed the centre, not the presenter: {reason}"
            ));
            reel.steps.push(step);
            let centred: Vec<OsString> = argv
                .iter()
                .map(|arg| {
                    if arg == "presenter" {
                        OsString::from("centre")
                    } else {
                        arg.clone()
                    }
                })
                .collect();
            let mut without_weights = Vec::new();
            let mut skip = false;
            for arg in centred {
                if skip {
                    skip = false;
                    continue;
                }
                if arg == "--weights" {
                    skip = true;
                    continue;
                }
                without_weights.push(arg);
            }
            step = run("reframe", without_weights, &[])?;
        }
        let ok = step.ok();
        if !ok {
            reel.failure = Some(json!({"step": name, "code": step.code, "stderr": step.tail()}));
            reel.steps.push(step);
            return Ok(reel);
        }
        reel.steps.push(step);
    }

    let video = reel_dir.join(format!("reel-{index}.mp4"));
    let render = run(
        "render",
        vec![
            "render".into(),
            "--project".into(),
            os(&previous),
            "--out".into(),
            os(&video),
            "--size".into(),
            args.size.clone().into(),
        ],
        &[],
    )?;
    if !render.ok() {
        reel.failure =
            Some(json!({"step": "render", "code": render.code, "stderr": render.tail()}));
        reel.steps.push(render);
        return Ok(reel);
    }
    reel.length_ms = render.field("duration_ms").and_then(|ms| ms.parse().ok());
    reel.notes.extend(
        render
            .stdout
            .lines()
            .skip(1)
            .filter(|line| line.starts_with("render:"))
            .map(str::to_string),
    );
    reel.steps.push(render);
    reel.path = Some(video.clone());
    reel.status = "ok";

    let diff = run(
        "diff",
        [vec!["diff".into(), "--draft".into(), os(&video)], outliers].concat(),
        &[],
    )?;
    if diff.ok() {
        let report: Value = serde_json::from_str(&diff.stdout).unwrap_or(Value::Null);
        fs::write(
            reel_dir.join(format!("reel-{index}.diff.json")),
            &diff.stdout,
        )
        .with_context(|| format!("write the diff of reel {index}"))?;
        reel.diff_flags = report["flags"]
            .as_array()
            .map(|flags| {
                flags
                    .iter()
                    .filter_map(|flag| flag["kind"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
    } else {
        reel.notes.push(format!("diff failed: {}", diff.tail()));
    }
    reel.steps.push(diff);
    Ok(reel)
}

pub fn make_reels(args: MakeReelsArgs) -> Result<()> {
    if args.outlier.len() > 3 {
        return Err(usage(format!(
            "--outlier was given {} times; at most 3",
            args.outlier.len()
        )));
    }
    if args.target_url.trim().is_empty() {
        return Err(usage("--target-url is empty"));
    }
    if !(args.max_cost_usd.is_finite() && args.max_cost_usd >= 0.0) {
        return Err(usage(format!(
            "--max-cost-usd {} is not an amount",
            args.max_cost_usd
        )));
    }
    for outlier in &args.outlier {
        load_dna(outlier)?;
    }
    // The caption style and its font are checked before anything runs.
    let style = args.style.resolve()?;
    crate::captions_draw::find_font(&style)?;
    fs::create_dir_all(&args.out_dir)
        .with_context(|| format!("create {}", args.out_dir.display()))?;
    let mut steps: Vec<Step> = Vec::new();

    // 1. The long video's edit DNA, with whisper words.
    let dna = match &args.dna {
        Some(path) => path.clone(),
        None => {
            let Some(model) = &args.whisper_model else {
                return Err(usage(
                    "make-reels needs whisper words: give --whisper-model (or REELCTL_WHISPER_MODEL), or --dna with an edit_dna.json made with one",
                ));
            };
            let out = args.out_dir.join("source.edit_dna.json");
            let step = required(run(
                "decompile",
                vec![
                    "decompile".into(),
                    "--input".into(),
                    os(&args.input),
                    "--out".into(),
                    os(&out),
                    "--whisper-model".into(),
                    os(model),
                ],
                &[],
            )?)?;
            steps.push(step);
            out
        }
    };
    if load_dna(&dna)?.asr_backend != AsrBackend::WhisperCpp {
        return Err(usage(format!(
            "{}: its words are voiced bursts, not whisper words; decompile with --whisper-model",
            dna.display()
        )));
    }

    // 2–4. The clip picks: an understanding already in the folder is used again;
    // otherwise the cost is planned, confirmed, and the model is called once.
    let understanding_path = args.out_dir.join("understanding.json");
    if !understanding_path.is_file() {
        let mut base: Vec<OsString> = vec![
            "understand".into(),
            "--input".into(),
            os(&args.input),
            "--dna".into(),
            os(&dna),
            "--out".into(),
            os(&understanding_path),
            "--model".into(),
            args.model.clone().into(),
            "--max-cost-usd".into(),
            args.max_cost_usd.to_string().into(),
        ];
        if let Some(catalog) = &args.understand_catalog {
            base.extend(["--catalog".into(), os(catalog)]);
        }
        let plan = required(run(
            "understand-plan",
            [base.clone(), vec!["--dry-run".into()]].concat(),
            &[],
        )?)?;
        let bound = plan
            .field("cost_upper_bound_usd")
            .unwrap_or_else(|| "unknown".to_string());
        let frames = plan.field("frames").unwrap_or_else(|| "?".to_string());
        println!(
            "make-reels: plan model={} frames={frames} cost_upper_bound_usd={bound}",
            args.model
        );
        steps.push(plan);
        let question = format!(
            "understand will send {frames} frames and the transcript to {} and cost at most ${bound}. Continue?",
            args.model
        );
        if !args.yes && !confirm(&question) {
            return Err(exit(
                4,
                format!("not confirmed: the model call could cost up to ${bound}; run again with --yes to make it"),
            ));
        }
        let mut env: Vec<(&str, OsString)> = Vec::new();
        let mut live = base;
        match &args.understand_replay {
            Some(replay) => live.extend(["--replay".into(), os(replay)]),
            None => {
                if let Some(key) = std::env::var_os(understand::KEY_VARIABLE) {
                    env.push((understand::KEY_VARIABLE, key));
                }
            }
        }
        steps.push(required(run("understand", live, &env)?)?);
    }
    let understanding = read_json(&understanding_path)?;
    let chosen = choose(&understanding, &args)?;
    for &index in &chosen {
        let clip = &understanding["clips"][index - 1];
        println!(
            "make-reels: clip={index} tight_length_ms={} flags={} hook=\"{}\"",
            clip["tight_length_ms"],
            clip["flags"].as_array().map_or(0, Vec::len),
            clip["hook"]["text"].as_str().unwrap_or("")
        );
    }

    let summary_path = args.out_dir.join("make_reels.json");
    let base_summary = |reels: Value, status: &str| {
        json!({
            "schema_version": MAKE_REELS_SCHEMA_VERSION,
            "status": status,
            "source": args.input.display().to_string(),
            "dna": dna.display().to_string(),
            "understanding": understanding_path.display().to_string(),
            "outliers": args.outlier.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            "model": understanding["model"]["id"],
            "cost_usd": understanding["model"]["cost_usd"],
            "follow": match args.follow { FollowArg::Presenter => "presenter", FollowArg::Centre => "centre" },
            "style": serde_json::to_value(&style).unwrap_or(Value::Null),
            "steps": steps.iter().map(Step::record).collect::<Vec<_>>(),
            "reels": reels,
        })
    };
    let write_summary = |summary: &Value| -> Result<()> {
        fs::write(
            &summary_path,
            format!("{}\n", serde_json::to_string_pretty(summary)?),
        )
        .with_context(|| format!("write {}", summary_path.display()))
    };
    if args.pick {
        let picked: Vec<Value> = chosen
            .iter()
            .map(|&index| {
                let clip = &understanding["clips"][index - 1];
                json!({"index": index, "status": "picked", "hook": clip["hook"]["text"], "last": clip["last"]["text"],
                       "tight_length_ms": clip["tight_length_ms"], "flags": clip["flags"]})
            })
            .collect();
        write_summary(&base_summary(Value::Array(picked), "picked"))?;
        println!(
            "make-reels: out_dir={} reels={} ok=0 failed=0 cost_usd={} picked=true",
            args.out_dir.display(),
            chosen.len(),
            understanding["model"]["cost_usd"]
        );
        return Ok(());
    }

    let mut reels = Vec::new();
    for &index in &chosen {
        let reel = make_reel(
            &args,
            index,
            &dna,
            &understanding_path,
            &args.out_dir.join(format!("reel-{index}")),
        )?;
        let clip = &understanding["clips"][index - 1];
        let mut notes = reel.notes.clone();
        if clip["flags"]
            .as_array()
            .is_some_and(|flags| !flags.is_empty())
        {
            notes.push(format!("made with --allow-flagged: {}", clip["flags"]));
        }
        println!(
            "make-reels: reel={index} status={} path={}",
            reel.status,
            reel.path
                .as_ref()
                .map_or("-".to_string(), |path| path.display().to_string())
        );
        reels.push(json!({
            "index": reel.index,
            "status": reel.status,
            "path": reel.path.as_ref().map(|path| path.display().to_string()),
            "length_ms": reel.length_ms,
            "hook": clip["hook"]["text"],
            "last": clip["last"]["text"],
            "diff_flags": reel.diff_flags,
            "notes": notes,
            "failure": reel.failure,
            "steps": reel.steps.iter().map(Step::record).collect::<Vec<_>>(),
        }));
    }
    let ok = reels.iter().filter(|reel| reel["status"] == "ok").count();
    let follow_used = if reels.iter().any(|reel| {
        reel["notes"].as_array().is_some_and(|notes| {
            notes.iter().any(|note| {
                note.as_str()
                    .is_some_and(|n| n.starts_with("reframe followed the centre"))
            })
        })
    }) {
        "centre"
    } else {
        match args.follow {
            FollowArg::Presenter => "presenter",
            FollowArg::Centre => "centre",
        }
    };
    let failed = reels.len() - ok;
    write_summary(&base_summary(
        Value::Array(reels),
        if ok > 0 { "ok" } else { "failed" },
    ))?;
    println!(
        "make-reels: out_dir={} reels={} ok={ok} failed={failed} cost_usd={} follow={follow_used}",
        args.out_dir.display(),
        chosen.len(),
        understanding["model"]["cost_usd"]
    );
    if ok == 0 && !chosen.is_empty() {
        return Err(anyhow::anyhow!(
            "no reel rendered; see {}",
            summary_path.display()
        ));
    }
    Ok(())
}
