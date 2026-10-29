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
//! Exit codes: 0 when at least one reel rendered (or `--pick`); 1 when none did
//! (or none was chosen); 2 for a bad argument or when every reel's first failing
//! step refused its input; 3 without an API key (checked before asking); 4 when
//! the cost is over `--max-cost-usd` (checked before asking) or the call was not
//! confirmed.

use std::collections::BTreeMap;
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
    progress::stage(name);
    let started = Instant::now();
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("run the {name} step"))?;
    let _tracked = progress::track(child.id());
    let output = child
        .wait_with_output()
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

/// The style flags as `overlay captions` job arguments. File paths are made
/// absolute (the catalogue takes absolute paths only); they were checked to exist
/// when the style was resolved.
fn style_json(style: &CaptionStyleArgs) -> Value {
    let absolute = |path: &str| {
        fs::canonicalize(path)
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string())
    };
    let mut map = serde_json::Map::new();
    let mut put = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            map.insert(key.to_string(), value);
        }
    };
    put("preset", style.preset.clone().map(Value::from));
    put(
        "style",
        style
            .style
            .as_ref()
            .map(|path| Value::from(absolute(&path.to_string_lossy()))),
    );
    put(
        "font",
        style
            .font
            .as_deref()
            .map(|font| Value::from(absolute(font))),
    );
    put("font_size", style.font_size.map(Value::from));
    put("color", style.color.clone().map(Value::from));
    put(
        "highlight_color",
        style.highlight_color.clone().map(Value::from),
    );
    put(
        "outline_color",
        style.outline_color.clone().map(Value::from),
    );
    put("outline_width", style.outline_width.map(Value::from));
    put("shadow", style.shadow.map(Value::from));
    put("box_color", style.box_color.clone().map(Value::from));
    put("box_opacity", style.box_opacity.map(Value::from));
    put("position", style.position.clone().map(Value::from));
    put("margin", style.margin.map(Value::from));
    put("words_per_group", style.words_per_group.map(Value::from));
    put("max_chars", style.max_chars.map(Value::from));
    put("case", style.case.clone().map(Value::from));
    put("min_gap_ms", style.min_gap_ms.map(Value::from));
    Value::Object(map)
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
    // Every step's argv comes from the command catalogue, as the service's jobs
    // do: the same typed arguments, inputs and outputs.
    let built = |command: &str,
                 args: Value,
                 inputs: &[(&str, &Path)],
                 out: &Path|
     -> Result<Vec<OsString>> {
        let inputs: BTreeMap<String, PathBuf> = inputs
            .iter()
            .map(|(role, path)| (role.to_string(), path.to_path_buf()))
            .collect();
        // `diff` writes its report on stdout: its output role is `report`.
        let role = if command == "diff" { "report" } else { "out" };
        let outputs = BTreeMap::from([(role.to_string(), out.to_path_buf())]);
        let built = studio::commands::build(command, &args, &inputs, &outputs)
            .map_err(|error| usage(error.to_string()))?;
        Ok(built.argv.into_iter().map(OsString::from).collect())
    };
    let outlier_roles: Vec<String> = (1..=args.outlier.len())
        .map(|n| {
            if n == 1 {
                "outlier".to_string()
            } else {
                format!("outlier.{n}")
            }
        })
        .collect();
    let mut plan: Vec<(&str, Vec<OsString>, Vec<(&str, OsString)>)> = Vec::new();
    plan.push((
        "from-clip",
        built(
            "timeline.from-clip",
            json!({"clip": index, "allow_flagged": args.allow_flagged}),
            &[
                ("input", &args.input),
                ("dna", dna),
                ("understanding", understanding),
            ],
            &project(1, "from-clip"),
        )?,
        Vec::new(),
    ));
    plan.push((
        "cut-silence",
        built(
            "timeline.cut-silence",
            json!({}),
            &[("project", &project(1, "from-clip")), ("dna", dna)],
            &project(2, "cut-silence"),
        )?,
        Vec::new(),
    ));
    let cut = project(2, "cut-silence");
    let outliers: Vec<(&str, &Path)> = outlier_roles
        .iter()
        .zip(&args.outlier)
        .map(|(role, path)| (role.as_str(), path.as_path()))
        .collect();
    let punch_inputs: Vec<(&str, &Path)> = [
        vec![("project", cut.as_path()), ("dna", dna)],
        outliers.clone(),
    ]
    .concat();
    plan.push((
        "auto-punch-in",
        built(
            "transform.auto-punch-in",
            json!({}),
            &punch_inputs,
            &project(3, "auto-punch-in"),
        )?,
        Vec::new(),
    ));
    let mut reframe_env: Vec<(&'static str, OsString)> = Vec::new();
    if let Some(library) = std::env::var_os(compositor::LIBRARY_ENV) {
        reframe_env.push((compositor::LIBRARY_ENV, library));
    }
    let punched = project(3, "auto-punch-in");
    let reframe_inputs = [("project", punched.as_path()), ("dna", dna)];
    // The fallback is built like the first, never by rewriting it.
    let centre_argv = built(
        "transform.reframe",
        json!({"follow": "centre"}),
        &reframe_inputs,
        &project(4, "reframe"),
    )?;
    let reframe_args = match (args.follow, &args.weights) {
        (FollowArg::Presenter, Some(weights)) => {
            // The catalogue takes absolute paths; a missing file is reframe's to
            // report (and to fall back from).
            let weights = fs::canonicalize(weights)
                .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default().join(weights));
            json!({"follow": "presenter", "weights": weights.to_string_lossy()})
        }
        (FollowArg::Presenter, None) => json!({"follow": "presenter"}),
        (FollowArg::Centre, _) => json!({"follow": "centre"}),
    };
    plan.push((
        "reframe",
        built(
            "transform.reframe",
            reframe_args,
            &reframe_inputs,
            &project(4, "reframe"),
        )?,
        reframe_env,
    ));
    plan.push((
        "bridge",
        built(
            "funnel.attach-bridge",
            json!({"target_url": args.target_url, "append": true, "card_ms": args.card_ms}),
            &[("project", &project(4, "reframe"))],
            &project(5, "bridge"),
        )?,
        Vec::new(),
    ));
    plan.push((
        "captions",
        built(
            "overlay.captions",
            style_json(&args.style),
            &[("project", &project(5, "bridge")), ("dna", dna)],
            &project(6, "captions"),
        )?,
        Vec::new(),
    ));
    let previous = project(6, "captions");

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
            step = run("reframe", centre_argv.clone(), &[])?;
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
        built(
            "render",
            json!({"size": args.size}),
            &[("project", &previous)],
            &video,
        )?,
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

    let report_path = reel_dir.join(format!("reel-{index}.diff.json"));
    let diff_inputs: Vec<(&str, &Path)> = [vec![("draft", video.as_path())], outliers].concat();
    let diff = run(
        "diff",
        built("diff", json!({}), &diff_inputs, &report_path)?,
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
    let mut seen = std::collections::BTreeSet::new();
    for &index in &args.clips {
        if index == 0 {
            return Err(usage("--clips are numbered from 1"));
        }
        if !seen.insert(index) {
            return Err(usage(format!("--clips names clip {index} twice")));
        }
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
        out!(
            "make-reels: plan model={} frames={frames} cost_upper_bound_usd={bound}",
            args.model
        );
        steps.push(plan);
        // The cap is checked here, before anyone is asked: a dry run that would be
        // refused still exits 0.
        let over = bound
            .parse::<f64>()
            .map(|bound| bound > args.max_cost_usd)
            .unwrap_or(false);
        if over {
            return Err(exit(
                4,
                format!(
                    "the model call could cost up to ${bound}, over --max-cost-usd {}; raise it to make the call",
                    args.max_cost_usd
                ),
            ));
        }
        let key = std::env::var_os(understand::KEY_VARIABLE);
        if args.understand_replay.is_none() && key.is_none() {
            return Err(exit(
                3,
                format!(
                    "{} is not set; set it to an OpenRouter API key to let understand pick the clips (nothing was sent)",
                    understand::KEY_VARIABLE
                ),
            ));
        }
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
                if let Some(key) = key {
                    env.push((understand::KEY_VARIABLE, key));
                }
            }
        }
        steps.push(required(run("understand", live, &env)?)?);
    }
    let understanding = read_json(&understanding_path)?;
    let source_name = Path::new(understanding["source"]["path"].as_str().unwrap_or(""))
        .file_name()
        .map(|name| name.to_os_string());
    if source_name.as_deref() != args.input.file_name() {
        return Err(usage(format!(
            "{} is of {}, not of {}; use another --out-dir or delete it",
            understanding_path.display(),
            understanding["source"]["path"],
            args.input.display()
        )));
    }
    let used_model = understanding["model"]["id"].as_str().unwrap_or("");
    if used_model != args.model {
        progress::note(&format!(
            "make-reels: {} was made with {used_model}, not {}; delete it to ask {} again",
            understanding_path.display(),
            args.model,
            args.model
        ));
    }
    let chosen = choose(&understanding, &args)?;
    for &index in &chosen {
        let clip = &understanding["clips"][index - 1];
        out!(
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
        out!(
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
        )
        .unwrap_or_else(|error| Reel {
            index,
            status: "failed",
            path: None,
            length_ms: None,
            failure: Some(
                json!({"step": "make-reels", "code": null, "stderr": format!("{error:#}")}),
            ),
            notes: Vec::new(),
            diff_flags: Vec::new(),
            steps: Vec::new(),
        });
        let clip = &understanding["clips"][index - 1];
        let mut notes = reel.notes.clone();
        if clip["flags"]
            .as_array()
            .is_some_and(|flags| !flags.is_empty())
        {
            notes.push(format!("made with --allow-flagged: {}", clip["flags"]));
        }
        out!(
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
    let failure_codes: Vec<Option<i32>> = reels
        .iter()
        .filter(|reel| reel["status"] != "ok")
        .map(|reel| reel["failure"]["code"].as_i64().map(|code| code as i32))
        .collect();
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
    out!(
        "make-reels: out_dir={} reels={} ok={ok} failed={failed} cost_usd={} follow={follow_used}",
        args.out_dir.display(),
        chosen.len(),
        understanding["model"]["cost_usd"]
    );
    if chosen.is_empty() {
        return Err(anyhow::anyhow!(
            "no clip to make (none passes the clip rules, or --reels 0); see --allow-flagged and --clips"
        ));
    }
    if ok == 0 {
        // Every reel failed: the exit code of the step that refused, when the
        // first failure was a refusal (2 to 4), else 1.
        let code = failure_codes.first().copied().flatten();
        let message = format!("no reel rendered; see {}", summary_path.display());
        return Err(match code {
            Some(code @ 2..=4) => exit(code, message),
            _ => anyhow::anyhow!(message),
        });
    }
    Ok(())
}
