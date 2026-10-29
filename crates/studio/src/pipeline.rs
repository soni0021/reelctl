//! The make-reels chain, shared by `reelctl make-reels` and the service's
//! `pipelines.make_reels`: which clips to make, and for each one the steps (a
//! catalogue command, its typed arguments, and where each input comes from). The
//! command line resolves the sources to files; the service, to job inputs.

use serde_json::{json, Value};

use crate::error::{Result, StudioError};

/// Where a step's input comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The long video.
    Input,
    /// Its edit DNA, with whisper words.
    Dna,
    /// Its `understanding.json`.
    Understanding,
    /// Outlier DNA number `n` (from 1).
    Outlier(usize),
    /// The output of the step at this index in the list.
    Step(usize),
}

/// One step of a reel.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The step's short name (`from-clip`, `render`, ...).
    pub name: &'static str,
    /// The catalogue command.
    pub command: &'static str,
    pub args: Value,
    /// Input role to source, in flag order.
    pub inputs: Vec<(String, Source)>,
    /// The output role later steps read (`out`, or `report` for `diff`).
    pub output: &'static str,
}

/// How the reels are made.
#[derive(Debug, Clone, PartialEq)]
pub struct ReelOptions {
    pub allow_flagged: bool,
    /// How many outlier DNAs (1 to 3).
    pub outliers: usize,
    /// `--follow presenter` (else centre).
    pub follow_presenter: bool,
    /// An absolute RobustVideoMatting weights path, for the presenter crop.
    pub weights: Option<String>,
    pub target_url: String,
    pub card_ms: u64,
    /// The `overlay captions` arguments (preset, style file, colours, ...).
    pub style: Value,
    /// The render size, `WxH`.
    pub size: String,
}

/// The input role of outlier `n` (from 1): `outlier`, `outlier.2`, `outlier.3`.
pub fn outlier_role(n: usize) -> String {
    if n == 1 {
        "outlier".to_string()
    } else {
        format!("outlier.{n}")
    }
}

/// The clips to make: `clips` when given (each must exist, and pass the clip rules
/// unless `allow_flagged`), else every clip that passes (or every clip with
/// `allow_flagged`), then at most `reels`.
pub fn choose(
    understanding: &Value,
    clips: &[usize],
    reels: Option<usize>,
    allow_flagged: bool,
) -> Result<Vec<usize>> {
    let all = understanding["clips"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let flagged = |index: usize| {
        all[index - 1]["flags"]
            .as_array()
            .is_some_and(|flags| !flags.is_empty())
    };
    let mut chosen: Vec<usize> = if clips.is_empty() {
        (1..=all.len())
            .filter(|&index| allow_flagged || !flagged(index))
            .collect()
    } else {
        for &index in clips {
            if index == 0 || index > all.len() {
                return Err(StudioError::invalid(format!(
                    "--clips {index}: understanding.json has {} clips (numbered from 1)",
                    all.len()
                )));
            }
            if flagged(index) && !allow_flagged {
                return Err(StudioError::invalid(format!(
                    "clip {index} breaks the clip rules; pass --allow-flagged to make it anyway"
                )));
            }
        }
        clips.to_vec()
    };
    if let Some(limit) = reels {
        chosen.truncate(limit);
    }
    Ok(chosen)
}

/// The index of each step in [`reel_steps`]' list.
pub mod at {
    pub const FROM_CLIP: usize = 0;
    pub const CUT_SILENCE: usize = 1;
    pub const AUTO_PUNCH_IN: usize = 2;
    pub const REFRAME: usize = 3;
    pub const BRIDGE: usize = 4;
    pub const CAPTIONS: usize = 5;
    pub const RENDER: usize = 6;
    pub const DIFF: usize = 7;
}

fn roles(pairs: &[(&str, Source)]) -> Vec<(String, Source)> {
    pairs
        .iter()
        .map(|(role, source)| (role.to_string(), source.clone()))
        .collect()
}

fn outliers(count: usize) -> Vec<(String, Source)> {
    (1..=count)
        .map(|n| (outlier_role(n), Source::Outlier(n)))
        .collect()
}

/// The reframe step with the centre crop: the fallback when the presenter crop
/// cannot run, built like the first and never by rewriting it.
pub fn centre_reframe() -> Step {
    Step {
        name: "reframe",
        command: "transform.reframe",
        args: json!({"follow": "centre"}),
        inputs: roles(&[
            ("project", Source::Step(at::AUTO_PUNCH_IN)),
            ("dna", Source::Dna),
        ]),
        output: "out",
    }
}

/// The steps that make clip `clip` (from 1) into a reel, in order: from-clip,
/// cut-silence, auto-punch-in, reframe, the appended card, captions, render, and
/// `diff` of the render against the outliers.
pub fn reel_steps(clip: usize, options: &ReelOptions) -> Vec<Step> {
    let reframe = if options.follow_presenter {
        let mut args = json!({"follow": "presenter"});
        if let Some(weights) = &options.weights {
            args["weights"] = json!(weights);
        }
        Step {
            args,
            ..centre_reframe()
        }
    } else {
        centre_reframe()
    };
    vec![
        Step {
            name: "from-clip",
            command: "timeline.from-clip",
            args: json!({"clip": clip, "allow_flagged": options.allow_flagged}),
            inputs: roles(&[
                ("input", Source::Input),
                ("dna", Source::Dna),
                ("understanding", Source::Understanding),
            ]),
            output: "out",
        },
        Step {
            name: "cut-silence",
            command: "timeline.cut-silence",
            args: json!({}),
            inputs: roles(&[
                ("project", Source::Step(at::FROM_CLIP)),
                ("dna", Source::Dna),
            ]),
            output: "out",
        },
        Step {
            name: "auto-punch-in",
            command: "transform.auto-punch-in",
            args: json!({}),
            inputs: [
                roles(&[
                    ("project", Source::Step(at::CUT_SILENCE)),
                    ("dna", Source::Dna),
                ]),
                outliers(options.outliers),
            ]
            .concat(),
            output: "out",
        },
        reframe,
        Step {
            name: "bridge",
            command: "funnel.attach-bridge",
            args: json!({"target_url": options.target_url, "append": true, "card_ms": options.card_ms}),
            inputs: roles(&[("project", Source::Step(at::REFRAME))]),
            output: "out",
        },
        Step {
            name: "captions",
            command: "overlay.captions",
            args: options.style.clone(),
            inputs: roles(&[("project", Source::Step(at::BRIDGE)), ("dna", Source::Dna)]),
            output: "out",
        },
        Step {
            name: "render",
            command: "render",
            args: json!({"size": options.size}),
            inputs: roles(&[("project", Source::Step(at::CAPTIONS))]),
            output: "out",
        },
        Step {
            name: "diff",
            command: "diff",
            args: json!({}),
            inputs: [
                roles(&[("draft", Source::Step(at::RENDER))]),
                outliers(options.outliers),
            ]
            .concat(),
            output: "report",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn options() -> ReelOptions {
        ReelOptions {
            allow_flagged: false,
            outliers: 2,
            follow_presenter: true,
            weights: Some("/models/rvm.onnx".to_string()),
            target_url: "https://example.com/course".to_string(),
            card_ms: 2500,
            style: json!({"preset": "bold"}),
            size: "1080x1920".to_string(),
        }
    }

    #[test]
    fn every_step_builds_from_the_catalogue_and_reads_only_earlier_steps() {
        let steps = reel_steps(3, &options());
        assert_eq!(
            steps.iter().map(|step| step.name).collect::<Vec<_>>(),
            [
                "from-clip",
                "cut-silence",
                "auto-punch-in",
                "reframe",
                "bridge",
                "captions",
                "render",
                "diff"
            ]
        );
        for (index, step) in steps.iter().enumerate() {
            let inputs: BTreeMap<String, PathBuf> = step
                .inputs
                .iter()
                .map(|(role, source)| {
                    if let Source::Step(from) = source {
                        assert!(*from < index, "{} reads a later step", step.name);
                    }
                    (role.clone(), PathBuf::from(format!("/in/{role}")))
                })
                .collect();
            let spec = commands::command(step.command).unwrap();
            let outputs = commands::default_outputs(spec, std::path::Path::new("/out"));
            assert!(outputs.contains_key(step.output), "{}", step.name);
            commands::build(step.command, &step.args, &inputs, &outputs)
                .unwrap_or_else(|error| panic!("{}: {error}", step.name));
        }
        assert_eq!(steps[at::FROM_CLIP].args["clip"], 3);
        assert_eq!(steps[at::REFRAME].args["weights"], "/models/rvm.onnx");
        assert_eq!(centre_reframe().args, json!({"follow": "centre"}));
        assert!(steps[at::DIFF]
            .inputs
            .iter()
            .any(|(role, source)| role == "outlier.2" && *source == Source::Outlier(2)));
    }

    #[test]
    fn clips_are_chosen_by_the_rules_the_list_and_the_limit() {
        let understanding =
            json!({"clips": [{"flags": []}, {"flags": ["too_long"]}, {"flags": []}]});
        assert_eq!(choose(&understanding, &[], None, false).unwrap(), [1, 3]);
        assert_eq!(choose(&understanding, &[], Some(1), true).unwrap(), [1]);
        assert_eq!(
            choose(&understanding, &[3, 1], None, false).unwrap(),
            [3, 1]
        );
        assert!(choose(&understanding, &[2], None, false).is_err());
        assert!(choose(&understanding, &[4], None, false).is_err());
        assert!(choose(&understanding, &[0], None, true).is_err());
    }
}
