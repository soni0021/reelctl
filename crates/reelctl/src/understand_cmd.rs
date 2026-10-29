//! `reelctl understand`: transcript segments, sampled frames, and a vision-language
//! model, combined into clip picks and labelled cuts (`understanding.json`).
//!
//! Exit codes beyond the usual: 3 when `OPENROUTER_API_KEY` is not set, 4 when the cost
//! upper bound is over `--max-cost-usd`. Neither sends anything to the model, and
//! neither does `--dry-run`. The key is only read from the environment and is never
//! printed or written.

use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::error::ErrorKind;
use clap::{Args, CommandFactory, ValueEnum};
use decompile::decode_frames_at;
use edit_dna::{AsrBackend, EditDna};
use understand::request::{build_parts, EncodedImage};
use understand::{
    assemble, build_clips, choose_layout, cost_upper_bound, eligible_endpoints, encode_jpeg,
    estimate_input_tokens, key_from_env, response_schema, sample_times, segment_lines,
    segment_words, system_prompt, tile, AssembleInput, EndpointInfo, FixedCatalog, FrameImage,
    Layout, ModelCatalog, OpenRouterBackend, OpenRouterCatalog, RecordedBackend, RunSettings,
    SheetMode, VlmBackend, VlmReply, VlmRequest, DEFAULT_MAX_TOKENS, FRAME_WIDTH, SEGMENT_PAUSE_MS,
};

use super::{Cli, DecoderArg};

/// An error that ends the process with its own exit code.
#[derive(Debug)]
pub struct ExitWith {
    pub code: i32,
    pub message: String,
}

impl fmt::Display for ExitWith {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ExitWith {}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SheetArg {
    Auto,
    #[value(name = "1")]
    One,
    #[value(name = "2")]
    Two,
    #[value(name = "3")]
    Three,
}

impl SheetArg {
    fn mode(self) -> SheetMode {
        match self {
            Self::Auto => SheetMode::Auto,
            Self::One => SheetMode::Single,
            Self::Two => SheetMode::Grid(2),
            Self::Three => SheetMode::Grid(3),
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum EffortArg {
    Low,
    Medium,
    High,
}

impl EffortArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Args)]
pub struct UnderstandArgs {
    /// The long video (the one `--dna` came from).
    #[arg(long)]
    input: PathBuf,
    /// Its edit_dna.json from `decompile` with a whisper model: the words, silence
    /// gaps, cuts, and duration are used, and the audio is not decoded again.
    #[arg(long)]
    dna: PathBuf,
    /// Where to write understanding.json.
    #[arg(long)]
    out: PathBuf,
    /// OpenRouter model ID.
    #[arg(long, default_value = "qwen/qwen3.5-9b")]
    model: String,
    /// Provider quantizations allowed, comma-separated. Meta's Muse endpoint reports
    /// `unknown` and needs `--quant unknown`.
    #[arg(long, value_delimiter = ',', default_values_t = ["fp8".to_string(), "bf16".to_string(), "fp16".to_string()])]
    quant: Vec<String>,
    /// Seconds between sampled frames.
    #[arg(long, default_value_t = understand::DEFAULT_EVERY_S)]
    every_s: u32,
    /// Send single frames, or tile them 2x2 or 3x3. `auto` tiles only when the
    /// request would not fit the model's context.
    #[arg(long, value_enum, default_value_t = SheetArg::Auto)]
    sheet: SheetArg,
    /// Reasoning effort, for models that take one.
    #[arg(long, value_enum)]
    effort: Option<EffortArg>,
    /// Refuse (exit 4) when the cost upper bound is over this many US dollars.
    #[arg(long, default_value_t = 0.50)]
    max_cost_usd: f64,
    /// Plan only: print the frame, image and segment counts and the cost upper bound,
    /// and do not call the model. Fetches the model's public price listing.
    #[arg(long)]
    dry_run: bool,
    /// Video decoder, as for `decompile`.
    #[arg(long, value_enum, default_value_t = DecoderArg::Auto)]
    decoder: DecoderArg,
    /// Internal: use a recorded model reply (JSON) instead of calling the model.
    #[arg(long, hide = true)]
    replay: Option<PathBuf>,
    /// Internal: use a model listing from a file instead of fetching it.
    #[arg(long, hide = true)]
    catalog: Option<PathBuf>,
}

/// An argument error: clap's usage message for this subcommand, exit code 2.
fn usage_error(message: String) -> ! {
    let mut command = Cli::command();
    match command.find_subcommand_mut("understand") {
        Some(sub) => sub.error(ErrorKind::ValueValidation, message).exit(),
        None => command.error(ErrorKind::ValueValidation, message).exit(),
    }
}

fn exit_with(code: i32, message: impl Into<String>) -> anyhow::Error {
    ExitWith {
        code,
        message: message.into(),
    }
    .into()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T> {
    let file = File::open(path).with_context(|| format!("open {what} {}", path.display()))?;
    serde_json::from_reader(file).with_context(|| format!("parse {what} {}", path.display()))
}

/// What will be sent, worked out before anything is decoded or sent.
struct Plan {
    segments: Vec<understand::Segment>,
    times_ms: Vec<u64>,
    layout: Layout,
    images: usize,
    endpoints: Vec<EndpointInfo>,
    input_tokens: u64,
    bound_usd: f64,
    context_tokens: u64,
}

fn sheet_name(layout: Layout) -> &'static str {
    match layout {
        Layout::Single => "single",
        Layout::Grid(2) => "2x2",
        Layout::Grid(_) => "3x3",
    }
}

fn plan(args: &UnderstandArgs, dna: &EditDna, catalog: &dyn ModelCatalog) -> Result<Plan> {
    let segments = segment_words(&dna.words, &dna.silence_gaps);
    let cut_times: Vec<u64> = dna.cuts.iter().map(|cut| cut.time_ms).collect();
    let times_ms = sample_times(dna.duration_ms, &cut_times, args.every_s);

    let listed = catalog
        .endpoints(&args.model)
        .with_context(|| format!("model {}", args.model))?;
    let endpoints: Vec<EndpointInfo> = eligible_endpoints(&listed, &args.quant)
        .into_iter()
        .cloned()
        .collect();
    if endpoints.is_empty() {
        return Err(exit_with(
            1,
            format!(
                "no endpoint of {} serves quantization {}; the listing has: {}",
                args.model,
                args.quant.join(","),
                listed
                    .iter()
                    .map(|e| format!(
                        "{} ({})",
                        e.provider,
                        e.quantization.as_deref().unwrap_or("not stated")
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    let context_tokens = endpoints
        .iter()
        .map(|endpoint| endpoint.context_length)
        .min()
        .unwrap_or(0);

    let segment_text = segment_lines(&segments);
    let prompt = system_prompt(&segments);
    let text_tokens = estimate_input_tokens(0, &prompt, &segment_text);
    let layout = choose_layout(
        args.sheet.mode(),
        times_ms.len(),
        text_tokens,
        DEFAULT_MAX_TOKENS,
        context_tokens,
    );
    let images = match layout {
        Layout::Single => times_ms.len(),
        Layout::Grid(n) => times_ms.len().div_ceil((n * n) as usize),
    };
    let input_tokens = estimate_input_tokens(images, &prompt, &segment_text);
    let refs: Vec<&EndpointInfo> = endpoints.iter().collect();
    let bound_usd = cost_upper_bound(input_tokens, DEFAULT_MAX_TOKENS, &refs);
    Ok(Plan {
        segments,
        times_ms,
        layout,
        images,
        endpoints,
        input_tokens,
        bound_usd,
        context_tokens,
    })
}

fn images_for(args: &UnderstandArgs, plan: &Plan) -> Result<(Vec<EncodedImage>, Vec<u64>)> {
    let sampled = decode_frames_at(
        &args.input,
        &plan.times_ms,
        FRAME_WIDTH,
        args.decoder.setting(),
    )
    .with_context(|| format!("decode frames of {}", args.input.display()))?;
    let mut frames: Vec<FrameImage> = Vec::with_capacity(sampled.len());
    for frame in sampled {
        // Two requests can land on one frame; it is sent once.
        if frames
            .last()
            .is_some_and(|last| last.time_ms == frame.time_ms)
        {
            continue;
        }
        let image = image::RgbImage::from_raw(frame.width, frame.height, frame.rgb)
            .context("a decoded frame has the wrong size")?;
        frames.push(FrameImage {
            time_ms: frame.time_ms,
            image,
        });
    }
    let frame_times: Vec<u64> = frames.iter().map(|frame| frame.time_ms).collect();
    let images = match plan.layout {
        Layout::Single => frames
            .iter()
            .map(|frame| {
                Ok(EncodedImage {
                    times_ms: vec![frame.time_ms],
                    jpeg: encode_jpeg(&frame.image)?,
                    sheet: false,
                })
            })
            .collect::<Result<Vec<_>, understand::FrameError>>()?,
        Layout::Grid(n) => tile(&frames, n, n)?
            .iter()
            .map(|sheet| {
                Ok(EncodedImage {
                    times_ms: sheet.times_ms.clone(),
                    jpeg: encode_jpeg(&sheet.image)?,
                    sheet: true,
                })
            })
            .collect::<Result<Vec<_>, understand::FrameError>>()?,
    };
    Ok((images, frame_times))
}

/// The request sent to the model: this video's system prompt (its length limit is
/// counted in this video's segments), the frames and transcript, and the settings.
fn build_request(
    args: &UnderstandArgs,
    segments: &[understand::Segment],
    images: &[EncodedImage],
) -> VlmRequest {
    VlmRequest {
        model: args.model.clone(),
        system: system_prompt(segments),
        parts: build_parts(images, segments),
        schema: response_schema(),
        max_tokens: DEFAULT_MAX_TOKENS,
        effort: args.effort.map(|effort| effort.as_str().to_string()),
        quantizations: args.quant.clone(),
    }
}

pub fn understand(args: UnderstandArgs) -> Result<()> {
    let dna: EditDna = read_json(&args.dna, "edit DNA")?;
    if dna.asr_backend != AsrBackend::WhisperCpp {
        usage_error(format!(
            "{} has no whisper words (asr_backend is {}); run `reelctl decompile` with --whisper-model first",
            args.dna.display(),
            dna.asr_backend.as_str()
        ));
    }
    if args.every_s == 0 {
        usage_error("--every-s must be at least 1".to_string());
    }
    // Both are checked before the key, the catalog, or anything else that can touch the
    // network: a DNA with nothing to segment or no length cannot make a useful request.
    if dna.words.is_empty() {
        usage_error(format!(
            "{} has no words to segment; a transcript is needed",
            args.dna.display()
        ));
    }
    if dna.duration_ms == 0 {
        usage_error(format!(
            "{} has a duration of 0 ms; there is no video to understand",
            args.dna.display()
        ));
    }

    // The key is needed only to call the model: not for --dry-run, and not for a replay.
    let calls_the_model = !args.dry_run && args.replay.is_none();
    let key = if calls_the_model {
        match key_from_env() {
            Some(key) => Some(key),
            None => {
                return Err(exit_with(
                    3,
                    format!(
                        "{} is not set; export it (for example `set -a; source .env; set +a`) or use --dry-run",
                        understand::KEY_VARIABLE
                    ),
                ))
            }
        }
    } else {
        None
    };

    let catalog: Box<dyn ModelCatalog> = match &args.catalog {
        Some(path) => Box::new(read_json::<FixedCatalog>(path, "model catalog")?),
        None => Box::new(OpenRouterCatalog::default()),
    };
    // `segment_words` gives at least one segment for any word, so the plan has segments.
    let plan = plan(&args, &dna, catalog.as_ref())?;

    out!(
        "frames={} images={} sheet={} segments={} context_tokens={} input_tokens_estimate={} cost_upper_bound_usd={:.4} max_cost_usd={:.2}",
        plan.times_ms.len(),
        plan.images,
        sheet_name(plan.layout),
        plan.segments.len(),
        plan.context_tokens,
        plan.input_tokens,
        plan.bound_usd,
        args.max_cost_usd
    );
    if args.dry_run {
        if plan.bound_usd > args.max_cost_usd {
            out!("a real run would be refused: the bound is over --max-cost-usd");
        }
        return Ok(());
    }
    if plan.bound_usd > args.max_cost_usd {
        return Err(exit_with(
            4,
            format!(
                "the cost upper bound ${:.4} is over --max-cost-usd ${:.2}; raise it, use a larger --every-s, or a cheaper model",
                plan.bound_usd, args.max_cost_usd
            ),
        ));
    }

    let backend: Box<dyn VlmBackend> = match (&args.replay, key) {
        (Some(path), _) => Box::new(RecordedBackend::new(read_json::<VlmReply>(
            path,
            "recorded reply",
        )?)),
        (None, Some(key)) => Box::new(OpenRouterBackend::new(key)),
        (None, None) => return Err(exit_with(3, "no API key")),
    };

    crate::timeline_cmd::cancelled()?;
    let (images, frame_times) = images_for(&args, &plan)?;
    let request = build_request(&args, &plan.segments, &images);
    crate::timeline_cmd::cancelled()?;
    let mut reply = backend.analyze(&request).context("the model call failed")?;
    reply.quantization = plan
        .endpoints
        .iter()
        .find(|endpoint| endpoint.provider == reply.provider)
        .and_then(|endpoint| endpoint.quantization.clone());

    let outcome = build_clips(
        &reply.reply.reel_candidates,
        &plan.segments,
        &dna.silence_gaps,
        dna.duration_ms,
    );
    let source = args.input.display().to_string();
    let understanding = assemble(AssembleInput {
        source_path: &source,
        duration_ms: dna.duration_ms,
        model_id: &args.model,
        segments: &plan.segments,
        cuts: &dna.cuts,
        frame_times_ms: &frame_times,
        reply: &reply,
        clips: outcome,
        settings: RunSettings {
            every_s: args.every_s,
            sheet: sheet_name(plan.layout).to_string(),
            segment_pause_ms: SEGMENT_PAUSE_MS,
            quant: args.quant.clone(),
        },
        cost_upper_bound_usd: plan.bound_usd,
    });
    if let Some(parent) = args.out.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
    }
    let json = serde_json::to_string_pretty(&understanding).context("format understanding.json")?;
    crate::timeline_cmd::cancelled()?;
    std::fs::write(&args.out, format!("{json}\n"))
        .with_context(|| format!("write {}", args.out.display()))?;
    out!(
        "clips={} rejected={} model={} provider={} cost_usd={} out={}",
        understanding.clips.len(),
        understanding.rejected.len(),
        understanding.model.id,
        understanding.model.provider,
        understanding
            .model
            .cost_usd
            .map_or("unknown".to_string(), |cost| format!("{cost:.4}")),
        args.out.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use understand::backend::Part;
    use understand::Segment;

    fn args() -> UnderstandArgs {
        UnderstandArgs {
            input: PathBuf::from("in.mp4"),
            dna: PathBuf::from("edit_dna.json"),
            out: PathBuf::from("understanding.json"),
            model: "vendor/model".to_string(),
            quant: vec!["fp8".to_string()],
            every_s: 5,
            sheet: SheetArg::Auto,
            effort: Some(EffortArg::High),
            max_cost_usd: 0.50,
            dry_run: false,
            decoder: DecoderArg::Auto,
            replay: None,
            catalog: None,
        }
    }

    /// 100 segments of 4 s each, so this video's mean segment length is 4.0 s.
    fn four_second_segments() -> Vec<Segment> {
        (0..100u32)
            .map(|i| Segment {
                id: i + 1,
                start_ms: u64::from(i) * 4_000,
                end_ms: u64::from(i + 1) * 4_000,
                text: "hello".to_string(),
                ends_sentence: true,
            })
            .collect()
    }

    #[test]
    fn the_request_carries_this_videos_prompt_not_a_generic_one() {
        let segments = four_second_segments();
        let request = build_request(&args(), &segments, &[]);
        assert!(
            request.system.contains("about 4.0 s each"),
            "{}",
            request.system
        );
        assert!(!request.system.contains('{'), "{}", request.system);
        assert_eq!(request.system, system_prompt(&segments));
        assert_ne!(request.system, system_prompt(&[]));
    }

    #[test]
    fn the_request_carries_the_model_settings_and_the_transcript() {
        let request = build_request(&args(), &four_second_segments(), &[]);
        assert_eq!(request.model, "vendor/model");
        assert_eq!(request.quantizations, vec!["fp8".to_string()]);
        assert_eq!(request.effort.as_deref(), Some("high"));
        assert_eq!(request.max_tokens, DEFAULT_MAX_TOKENS);
        assert!(request
            .parts
            .iter()
            .any(|part| matches!(part, Part::Text(text) if text.contains("G100"))));
    }
}
