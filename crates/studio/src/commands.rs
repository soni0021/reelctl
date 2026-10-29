//! The command catalogue: every `reelctl` command a job can run, with its typed
//! arguments, its input and output roles, its resource class and the environment
//! names it may get. [`build`] is the one place a job's argv is made; a raw argv is
//! never accepted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Result, StudioError};
use crate::jobs::Class;

/// The type of an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    Str,
    Int,
    Float,
    /// A flag with no value, given when `true`.
    Bool,
    /// An absolute path to a tool or a file that is not a content input (a model,
    /// a font, a style file).
    Path,
    /// One of these values.
    Choice(&'static [&'static str]),
    /// A list of positive integers, written comma-separated.
    IntList,
}

#[derive(Debug, Clone, Copy)]
pub struct ArgSpec {
    pub key: &'static str,
    pub flag: &'static str,
    pub kind: ArgKind,
    pub required: bool,
}

/// A content input: the role a job's `inputs` names, and its flag.
#[derive(Debug, Clone, Copy)]
pub struct InputSpec {
    pub role: &'static str,
    pub flag: &'static str,
    pub required: bool,
    /// How many may be given (`role`, `role.2`, `role.3`, ...), the flag repeated.
    pub most: usize,
}

/// Where an output goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// A file, named by its flag.
    File,
    /// A folder, named by its flag.
    Dir,
    /// The command's stdout (a JSON report), saved as the file.
    Stdout,
}

#[derive(Debug, Clone, Copy)]
pub struct OutputSpec {
    pub role: &'static str,
    /// The flag that names it; none for [`OutputKind::Stdout`].
    pub flag: Option<&'static str>,
    pub kind: OutputKind,
    /// Its file or folder name in the job's scratch folder.
    pub name: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    /// `timeline.cut-silence`, ...
    pub name: &'static str,
    pub words: &'static [&'static str],
    pub class: Class,
    pub inputs: &'static [InputSpec],
    pub outputs: &'static [OutputSpec],
    pub args: &'static [ArgSpec],
    /// Environment variables this command may be given (values come from the
    /// runner, never from the catalogue).
    pub env: &'static [&'static str],
}

const fn arg(key: &'static str, flag: &'static str, kind: ArgKind) -> ArgSpec {
    ArgSpec {
        key,
        flag,
        kind,
        required: false,
    }
}

const fn must(key: &'static str, flag: &'static str, kind: ArgKind) -> ArgSpec {
    ArgSpec {
        key,
        flag,
        kind,
        required: true,
    }
}

const fn input(role: &'static str, flag: &'static str) -> InputSpec {
    InputSpec {
        role,
        flag,
        required: true,
        most: 1,
    }
}

const fn maybe(role: &'static str, flag: &'static str) -> InputSpec {
    InputSpec {
        role,
        flag,
        required: false,
        most: 1,
    }
}

const fn file(role: &'static str, flag: &'static str, name: &'static str) -> OutputSpec {
    OutputSpec {
        role,
        flag: Some(flag),
        kind: OutputKind::File,
        name,
    }
}

const DECODER: ArgKind = ArgKind::Choice(&["auto", "software"]);
const PROJECT_OUT: OutputSpec = file("out", "--out", "project.otio");
const PROJECT_IN: InputSpec = input("project", "--project");

/// The caption style flags `overlay captions` takes.
pub const CAPTION_STYLE: &[ArgSpec] = &[
    arg(
        "preset",
        "--preset",
        ArgKind::Choice(&["bold", "clean", "boxed", "karaoke", "minimal"]),
    ),
    arg("style", "--style", ArgKind::Path),
    arg("font", "--font", ArgKind::Path),
    arg("font_size", "--font-size", ArgKind::Float),
    arg("color", "--color", ArgKind::Str),
    arg("highlight_color", "--highlight-color", ArgKind::Str),
    arg("outline_color", "--outline-color", ArgKind::Str),
    arg("outline_width", "--outline-width", ArgKind::Int),
    arg("shadow", "--shadow", ArgKind::Int),
    arg("box_color", "--box-color", ArgKind::Str),
    arg("box_opacity", "--box-opacity", ArgKind::Float),
    arg(
        "position",
        "--position",
        ArgKind::Choice(&["top", "middle", "bottom"]),
    ),
    arg("margin", "--margin", ArgKind::Float),
    arg("words_per_group", "--words-per-group", ArgKind::Int),
    arg("max_chars", "--max-chars", ArgKind::Int),
    arg(
        "case",
        "--case",
        ArgKind::Choice(&["as-is", "upper", "lower"]),
    ),
    arg("min_gap_ms", "--min-gap-ms", ArgKind::Int),
];

/// Every command a job can run.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "decompile",
        words: &["decompile"],
        class: Class::Heavy,
        inputs: &[input("input", "--input")],
        outputs: &[
            file("out", "--out", "edit_dna.json"),
            OutputSpec {
                role: "views",
                flag: Some("--filmstrip-dir"),
                kind: OutputKind::Dir,
                name: "views",
            },
        ],
        args: &[
            arg(
                "cut_detector",
                "--cut-detector",
                ArgKind::Choice(&["auto", "scene", "transnet"]),
            ),
            arg("transnet_weights", "--transnet-weights", ArgKind::Path),
            arg("decoder", "--decoder", DECODER),
            arg("whisper_model", "--whisper-model", ArgKind::Path),
        ],
        env: &["ORT_DYLIB_PATH"],
    },
    CommandSpec {
        name: "diff",
        words: &["diff"],
        class: Class::Heavy,
        inputs: &[
            input("draft", "--draft"),
            InputSpec {
                role: "outlier",
                flag: "--outlier",
                required: true,
                most: 3,
            },
        ],
        outputs: &[OutputSpec {
            role: "report",
            flag: None,
            kind: OutputKind::Stdout,
            name: "diff.json",
        }],
        args: &[
            arg("decoder", "--decoder", DECODER),
            arg("whisper_model", "--whisper-model", ArgKind::Path),
        ],
        env: &[],
    },
    CommandSpec {
        name: "understand",
        words: &["understand"],
        class: Class::Network,
        inputs: &[input("input", "--input"), input("dna", "--dna")],
        outputs: &[file("out", "--out", "understanding.json")],
        args: &[
            arg("model", "--model", ArgKind::Str),
            arg("quant", "--quant", ArgKind::Str),
            arg("every_s", "--every-s", ArgKind::Int),
            arg(
                "sheet",
                "--sheet",
                ArgKind::Choice(&["auto", "1", "2", "3"]),
            ),
            arg(
                "effort",
                "--effort",
                ArgKind::Choice(&["low", "medium", "high"]),
            ),
            must("max_cost_usd", "--max-cost-usd", ArgKind::Float),
            arg("max_output_tokens", "--max-output-tokens", ArgKind::Int),
            arg("dry_run", "--dry-run", ArgKind::Bool),
            arg("decoder", "--decoder", DECODER),
        ],
        env: &["OPENROUTER_API_KEY"],
    },
    CommandSpec {
        name: "look",
        words: &["look"],
        class: Class::Heavy,
        inputs: &[input("draft", "--draft"), input("outlier", "--outlier")],
        outputs: &[file("out", "--out", "look.cube")],
        args: &[
            arg("size", "--size", ArgKind::Choice(&["17", "33", "65"])),
            arg("samples", "--samples", ArgKind::Int),
            arg("decoder", "--decoder", DECODER),
        ],
        env: &[],
    },
    CommandSpec {
        name: "timeline.init",
        words: &["timeline", "init"],
        class: Class::Light,
        inputs: &[input("input", "--input"), maybe("dna", "--dna")],
        outputs: &[PROJECT_OUT],
        args: &[],
        env: &[],
    },
    CommandSpec {
        name: "timeline.from-clip",
        words: &["timeline", "from-clip"],
        class: Class::Light,
        inputs: &[
            input("input", "--input"),
            input("dna", "--dna"),
            input("understanding", "--understanding"),
        ],
        outputs: &[PROJECT_OUT],
        args: &[
            must("clip", "--clip", ArgKind::Int),
            arg("allow_flagged", "--allow-flagged", ArgKind::Bool),
        ],
        env: &[],
    },
    CommandSpec {
        name: "timeline.cut-silence",
        words: &["timeline", "cut-silence"],
        class: Class::Light,
        inputs: &[PROJECT_IN, input("dna", "--dna")],
        outputs: &[PROJECT_OUT],
        args: &[
            arg("min_gap_ms", "--min-gap-ms", ArgKind::Int),
            arg("fade_ms", "--fade-ms", ArgKind::Int),
            arg("fillers", "--fillers", ArgKind::Str),
        ],
        env: &[],
    },
    CommandSpec {
        name: "transform.punch-in",
        words: &["transform", "punch-in"],
        class: Class::Light,
        inputs: &[PROJECT_IN],
        outputs: &[PROJECT_OUT],
        args: &[
            must("start_ms", "--start-ms", ArgKind::Int),
            must("end_ms", "--end-ms", ArgKind::Int),
            must("scale", "--scale", ArgKind::Float),
        ],
        env: &[],
    },
    CommandSpec {
        name: "transform.auto-punch-in",
        words: &["transform", "auto-punch-in"],
        class: Class::Light,
        inputs: &[
            PROJECT_IN,
            input("dna", "--dna"),
            InputSpec {
                role: "outlier",
                flag: "--outlier",
                required: false,
                most: 3,
            },
        ],
        outputs: &[PROJECT_OUT],
        args: &[],
        env: &[],
    },
    CommandSpec {
        name: "transform.reframe",
        words: &["transform", "reframe"],
        // `--follow presenter` runs a model: heavy (see `class_of`).
        class: Class::Light,
        inputs: &[PROJECT_IN, maybe("dna", "--dna")],
        outputs: &[PROJECT_OUT],
        args: &[
            arg("aspect", "--aspect", ArgKind::Str),
            must(
                "follow",
                "--follow",
                ArgKind::Choice(&["presenter", "centre"]),
            ),
            arg("weights", "--weights", ArgKind::Path),
            arg("downsample_ratio", "--downsample-ratio", ArgKind::Float),
        ],
        env: &["ORT_DYLIB_PATH"],
    },
    CommandSpec {
        name: "funnel.attach-bridge",
        words: &["funnel", "attach-bridge"],
        class: Class::Light,
        inputs: &[PROJECT_IN, maybe("dna", "--dna")],
        outputs: &[PROJECT_OUT],
        args: &[
            must("target_url", "--target-url", ArgKind::Str),
            arg("cutoff_pct", "--cutoff-pct", ArgKind::Float),
            arg("append", "--append", ArgKind::Bool),
            arg("card_ms", "--card-ms", ArgKind::Int),
        ],
        env: &[],
    },
    CommandSpec {
        name: "overlay.captions",
        words: &["overlay", "captions"],
        class: Class::Light,
        inputs: &[PROJECT_IN, input("dna", "--dna")],
        outputs: &[PROJECT_OUT],
        args: CAPTION_STYLE,
        env: &[],
    },
    CommandSpec {
        name: "overlay.add-chart",
        words: &["overlay", "add-chart"],
        class: Class::Heavy,
        inputs: &[PROJECT_IN, input("spec", "--spec")],
        outputs: &[
            PROJECT_OUT,
            OutputSpec {
                role: "assets",
                flag: Some("--assets-dir"),
                kind: OutputKind::Dir,
                name: "assets",
            },
        ],
        args: &[
            arg("manimgx", "--manimgx", ArgKind::Path),
            arg("skip_inspect", "--skip-inspect", ArgKind::Bool),
            arg("timeout_s", "--timeout-s", ArgKind::Int),
        ],
        env: &["MANIMGX_BIN"],
    },
    CommandSpec {
        name: "overlay.remove",
        words: &["overlay", "remove"],
        class: Class::Light,
        inputs: &[PROJECT_IN],
        outputs: &[PROJECT_OUT],
        args: &[
            arg("id", "--id", ArgKind::Str),
            arg("captions", "--captions", ArgKind::Bool),
        ],
        env: &[],
    },
    CommandSpec {
        name: "render",
        words: &["render"],
        class: Class::Heavy,
        inputs: &[PROJECT_IN],
        outputs: &[file("out", "--out", "render.mp4")],
        args: &[
            arg("size", "--size", ArgKind::Str),
            arg(
                "backend",
                "--backend",
                ArgKind::Choice(&["ffmpeg", "smelter"]),
            ),
            arg("smelter", "--smelter", ArgKind::Path),
        ],
        env: &["SMELTER_BIN"],
    },
    CommandSpec {
        name: "qa.inspect-boundaries",
        words: &["qa", "inspect-boundaries"],
        class: Class::Heavy,
        inputs: &[PROJECT_IN],
        outputs: &[OutputSpec {
            role: "views",
            flag: Some("--out-dir"),
            kind: OutputKind::Dir,
            name: "views",
        }],
        args: &[],
        env: &[],
    },
    CommandSpec {
        name: "compositor.isolate-presenter",
        words: &["compositor", "isolate-presenter"],
        class: Class::Heavy,
        inputs: &[input("input", "--input")],
        outputs: &[file("out", "--out", "presenter.mp4")],
        args: &[
            arg("background", "--background", ArgKind::Str),
            arg("weights", "--weights", ArgKind::Path),
            arg(
                "mode",
                "--mode",
                ArgKind::Choice(&["composite", "matte", "rgba"]),
            ),
            arg("downsample_ratio", "--downsample-ratio", ArgKind::Float),
        ],
        env: &["ORT_DYLIB_PATH"],
    },
    CommandSpec {
        name: "eval",
        words: &["eval"],
        class: Class::Light,
        inputs: &[
            input("truth", "--truth"),
            input("answers", "--answers"),
            maybe("dna", "--dna"),
        ],
        outputs: &[OutputSpec {
            role: "report",
            flag: None,
            kind: OutputKind::Stdout,
            name: "eval.json",
        }],
        args: &[
            arg(
                "split",
                "--split",
                ArgKind::Choice(&["val", "train", "all"]),
            ),
            arg("cut_tolerance_ms", "--cut-tolerance-ms", ArgKind::Int),
            arg("event_tolerance_ms", "--event-tolerance-ms", ArgKind::Int),
        ],
        env: &[],
    },
];

/// The command named `name`.
pub fn command(name: &str) -> Result<&'static CommandSpec> {
    COMMANDS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| StudioError::invalid(format!("\"{name}\" is not a command a job can run")))
}

/// The class a job of `spec` with `args` runs in.
pub fn class_of(spec: &CommandSpec, args: &Value) -> Class {
    if spec.name == "transform.reframe"
        && args.get("follow").and_then(Value::as_str) == Some("presenter")
    {
        return Class::Heavy;
    }
    if spec.name == "understand" && args.get("dry_run").and_then(Value::as_bool) == Some(true) {
        return Class::Light;
    }
    spec.class
}

/// Where each output of `spec` goes in `scratch`.
pub fn default_outputs(spec: &CommandSpec, scratch: &Path) -> BTreeMap<String, PathBuf> {
    spec.outputs
        .iter()
        .map(|output| (output.role.to_string(), scratch.join(output.name)))
        .collect()
}

/// Where each output of a job of `spec` with `args` goes in `scratch`: as
/// [`default_outputs`], except that an RGBA presenter cut-out is a `.mov` (the
/// command refuses another container for it).
pub fn outputs_for(spec: &CommandSpec, args: &Value, scratch: &Path) -> BTreeMap<String, PathBuf> {
    let mut outputs = default_outputs(spec, scratch);
    if spec.name == "compositor.isolate-presenter"
        && args.get("mode").and_then(Value::as_str) == Some("rgba")
    {
        outputs.insert("out".to_string(), scratch.join("presenter.mov"));
    }
    outputs
}

/// A built command line.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
    /// The arguments after the program (`reelctl`).
    pub argv: Vec<String>,
    /// The output roles and where they will be (a stdout output is written there
    /// by the runner).
    pub outputs: BTreeMap<String, PathBuf>,
    /// The output role whose file is the command's stdout, if any.
    pub stdout: Option<String>,
    pub env: Vec<&'static str>,
}

fn value_text(spec: &CommandSpec, arg: &ArgSpec, value: &Value) -> Result<Option<String>> {
    let bad = |want: &str| {
        StudioError::invalid(format!("{} {}: {value} is not {want}", spec.name, arg.key))
    };
    Ok(match arg.kind {
        ArgKind::Bool => match value.as_bool() {
            Some(true) => Some(String::new()),
            Some(false) => None,
            None => return Err(bad("true or false")),
        },
        ArgKind::Int => Some(
            value
                .as_i64()
                .ok_or_else(|| bad("a whole number"))?
                .to_string(),
        ),
        ArgKind::Float => {
            let number = value
                .as_f64()
                .filter(|n| n.is_finite())
                .ok_or_else(|| bad("a number"))?;
            Some(number.to_string())
        }
        ArgKind::Str => {
            let text = value.as_str().ok_or_else(|| bad("text"))?;
            if text.starts_with('-') {
                return Err(bad("text that does not start with -"));
            }
            Some(text.to_string())
        }
        ArgKind::Path => {
            let text = value.as_str().ok_or_else(|| bad("a path"))?;
            if !Path::new(text).is_absolute() {
                return Err(bad("an absolute path"));
            }
            Some(text.to_string())
        }
        ArgKind::Choice(choices) => {
            let text = value.as_str().ok_or_else(|| bad("text"))?;
            if !choices.contains(&text) {
                return Err(bad(&format!("one of {}", choices.join(", "))));
            }
            Some(text.to_string())
        }
        ArgKind::IntList => {
            let list = value.as_array().ok_or_else(|| bad("a list"))?;
            let numbers: Vec<String> = list
                .iter()
                .map(|n| n.as_u64().filter(|n| *n > 0).map(|n| n.to_string()))
                .collect::<Option<_>>()
                .ok_or_else(|| bad("a list of positive whole numbers"))?;
            Some(numbers.join(","))
        }
    })
}

/// The argv for a job of command `name` with typed `args` (a JSON object), its
/// content `inputs` (role to path; `outlier`, `outlier.2`, ... for a repeated one)
/// and where its outputs go. Unknown arguments and inputs, missing required ones
/// and values of the wrong type are refused.
pub fn build(
    name: &str,
    args: &Value,
    inputs: &BTreeMap<String, PathBuf>,
    outputs: &BTreeMap<String, PathBuf>,
) -> Result<Built> {
    let spec = command(name)?;
    let object = match args {
        Value::Object(map) => map.clone(),
        Value::Null => serde_json::Map::new(),
        _ => {
            return Err(StudioError::invalid(format!(
                "{name}: the arguments are not an object"
            )))
        }
    };
    for key in object.keys() {
        if !spec.args.iter().any(|arg| arg.key == key) {
            return Err(StudioError::invalid(format!(
                "{name} has no argument \"{key}\""
            )));
        }
    }
    let mut argv: Vec<String> = spec.words.iter().map(|word| word.to_string()).collect();

    for role in inputs.keys() {
        let base = role.split('.').next().unwrap_or(role);
        if !spec.inputs.iter().any(|input| input.role == base) {
            return Err(StudioError::invalid(format!(
                "{name} has no input \"{role}\""
            )));
        }
    }
    for input in spec.inputs {
        let mut given: Vec<(&String, &PathBuf)> = inputs
            .iter()
            .filter(|(role, _)| {
                *role == input.role
                    || role
                        .strip_prefix(input.role)
                        .and_then(|rest| rest.strip_prefix('.'))
                        .is_some_and(|n| n.parse::<usize>().is_ok())
            })
            .collect();
        given.sort_by_key(|(role, _)| {
            role.split('.')
                .nth(1)
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(1)
        });
        if given.is_empty() && input.required {
            return Err(StudioError::invalid(format!(
                "{name} needs the input \"{}\"",
                input.role
            )));
        }
        if given.len() > input.most {
            return Err(StudioError::invalid(format!(
                "{name} takes at most {} \"{}\" inputs",
                input.most, input.role
            )));
        }
        for (_, path) in given {
            argv.push(input.flag.to_string());
            argv.push(path.to_string_lossy().into_owned());
        }
    }

    for arg in spec.args {
        match object.get(arg.key) {
            None | Some(Value::Null) => {
                if arg.required {
                    return Err(StudioError::invalid(format!(
                        "{name} needs the argument \"{}\"",
                        arg.key
                    )));
                }
            }
            Some(value) => {
                if let Some(text) = value_text(spec, arg, value)? {
                    argv.push(arg.flag.to_string());
                    if arg.kind != ArgKind::Bool {
                        argv.push(text);
                    }
                }
            }
        }
    }

    let mut placed = BTreeMap::new();
    let mut stdout = None;
    for output in spec.outputs {
        let path = outputs.get(output.role).ok_or_else(|| {
            StudioError::invalid(format!(
                "{name}: no place for the output \"{}\"",
                output.role
            ))
        })?;
        match output.flag {
            Some(flag) => {
                argv.push(flag.to_string());
                argv.push(path.to_string_lossy().into_owned());
            }
            None => stdout = Some(output.role.to_string()),
        }
        placed.insert(output.role.to_string(), path.clone());
    }
    Ok(Built {
        argv,
        outputs: placed,
        stdout,
        env: spec.env.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn paths(pairs: &[(&str, &str)]) -> BTreeMap<String, PathBuf> {
        pairs
            .iter()
            .map(|(role, path)| (role.to_string(), PathBuf::from(path)))
            .collect()
    }

    #[test]
    fn every_command_builds_with_its_required_parts() {
        for spec in COMMANDS {
            let inputs: BTreeMap<String, PathBuf> = spec
                .inputs
                .iter()
                .filter(|input| input.required)
                .map(|input| {
                    (
                        input.role.to_string(),
                        PathBuf::from(format!("/in/{}", input.role)),
                    )
                })
                .collect();
            let mut args = serde_json::Map::new();
            for arg in spec.args.iter().filter(|arg| arg.required) {
                let value = match arg.kind {
                    ArgKind::Int => json!(1),
                    ArgKind::Float => json!(1.5),
                    ArgKind::Choice(choices) => json!(choices[0]),
                    _ => json!("x"),
                };
                args.insert(arg.key.to_string(), value);
            }
            let outputs = default_outputs(spec, Path::new("/scratch"));
            let built = build(spec.name, &Value::Object(args), &inputs, &outputs)
                .unwrap_or_else(|error| panic!("{}: {error}", spec.name));
            assert_eq!(&built.argv[..spec.words.len()], spec.words);
            assert_eq!(built.outputs.len(), spec.outputs.len(), "{}", spec.name);
        }
    }

    #[test]
    fn a_cut_silence_job_is_the_command_line_a_person_would_type() {
        let built = build(
            "timeline.cut-silence",
            &json!({"min_gap_ms": 250, "fillers": "um,uh"}),
            &paths(&[("project", "/s/p.otio"), ("dna", "/s/d.json")]),
            &paths(&[("out", "/scratch/project.otio")]),
        )
        .unwrap();
        assert_eq!(
            built.argv,
            [
                "timeline",
                "cut-silence",
                "--project",
                "/s/p.otio",
                "--dna",
                "/s/d.json",
                "--min-gap-ms",
                "250",
                "--fillers",
                "um,uh",
                "--out",
                "/scratch/project.otio"
            ]
        );
    }

    #[test]
    fn repeated_inputs_keep_their_order_and_have_a_limit() {
        let built = build(
            "diff",
            &json!({}),
            &paths(&[
                ("draft", "/d.mp4"),
                ("outlier.2", "/b.json"),
                ("outlier", "/a.json"),
            ]),
            &paths(&[("report", "/scratch/diff.json")]),
        )
        .unwrap();
        assert_eq!(
            built.argv,
            [
                "diff",
                "--draft",
                "/d.mp4",
                "--outlier",
                "/a.json",
                "--outlier",
                "/b.json"
            ]
        );
        assert_eq!(built.stdout.as_deref(), Some("report"));
        let four = paths(&[
            ("draft", "/d"),
            ("outlier", "/1"),
            ("outlier.2", "/2"),
            ("outlier.3", "/3"),
            ("outlier.4", "/4"),
        ]);
        assert!(build("diff", &json!({}), &four, &paths(&[("report", "/r")])).is_err());
    }

    #[test]
    fn wrong_names_types_and_missing_parts_are_refused() {
        let inputs = paths(&[("project", "/p.otio")]);
        let out = paths(&[("out", "/o.otio")]);
        let punch = |args: Value| build("transform.punch-in", &args, &inputs, &out);
        assert!(punch(json!({"start_ms": 0, "end_ms": 1000, "scale": 1.2})).is_ok());
        assert!(punch(json!({"start_ms": 0, "end_ms": 1000}))
            .unwrap_err()
            .to_string()
            .contains("scale"));
        assert!(punch(json!({"start_ms": "0", "end_ms": 1000, "scale": 1.2})).is_err());
        assert!(
            punch(json!({"start_ms": 0, "end_ms": 1000, "scale": 1.2, "shell": "rm"}))
                .unwrap_err()
                .to_string()
                .contains("shell")
        );
        assert!(build("transform.punch-in", &json!({}), &BTreeMap::new(), &out).is_err());
        assert!(build("rm", &json!({}), &inputs, &out).is_err());
        // A path argument must be absolute, and text may not look like a flag.
        let reframe = |args: Value| build("transform.reframe", &args, &inputs, &out);
        assert!(reframe(json!({"follow": "presenter", "weights": "rvm.onnx"})).is_err());
        assert!(reframe(json!({"follow": "presenter", "weights": "/m/rvm.onnx"})).is_ok());
        assert!(reframe(json!({"follow": "sideways"})).is_err());
        let bridge = build(
            "funnel.attach-bridge",
            &json!({"target_url": "--help"}),
            &inputs,
            &out,
        );
        assert!(bridge.is_err());
    }

    #[test]
    fn a_flag_argument_is_given_only_when_true() {
        let inputs = paths(&[("project", "/p.otio")]);
        let out = paths(&[("out", "/o.otio")]);
        let on = build(
            "funnel.attach-bridge",
            &json!({"target_url": "https://x", "append": true}),
            &inputs,
            &out,
        )
        .unwrap();
        assert!(on.argv.contains(&"--append".to_string()));
        let off = build(
            "funnel.attach-bridge",
            &json!({"target_url": "https://x", "append": false}),
            &inputs,
            &out,
        )
        .unwrap();
        assert!(!off.argv.contains(&"--append".to_string()));
    }

    #[test]
    fn the_class_follows_the_arguments_where_they_change_the_cost() {
        let reframe = command("transform.reframe").unwrap();
        assert_eq!(
            class_of(reframe, &json!({"follow": "presenter"})),
            Class::Heavy
        );
        assert_eq!(
            class_of(reframe, &json!({"follow": "centre"})),
            Class::Light
        );
        let understand = command("understand").unwrap();
        assert_eq!(
            class_of(understand, &json!({"dry_run": true})),
            Class::Light
        );
        assert_eq!(class_of(understand, &json!({})), Class::Network);
        assert_eq!(command("understand").unwrap().env, ["OPENROUTER_API_KEY"]);
    }
}
