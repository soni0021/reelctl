//! The prompt, the strict reply schema, token estimates, and the cost bound.

use serde_json::Value;

use crate::backend::Part;
use crate::catalog::EndpointInfo;
use crate::types::Segment;
use crate::IMAGE_TOKEN_ESTIMATE;

const SYSTEM_PROMPT: &str = include_str!("prompt.md");
const REPLY_SCHEMA: &str = include_str!("reply_schema.json");

const LENGTH_RULE: &str = include_str!("length_rule.md");
const LENGTH_RULE_SHORT: &str = include_str!("length_rule_short.md");

/// A video shorter than this cannot hold a 45 to 60 s clip with room to spare, so it
/// gets the one-candidate rule instead of the length aim.
const SHORT_VIDEO_S: f64 = 70.0;

/// `1 segment`, `2 segments`.
fn segments_phrase(count: u32) -> String {
    if count == 1 {
        "1 segment".to_string()
    } else {
        format!("{count} segments")
    }
}

/// The system prompt (the Component 3 rubric). The length guidance is counted in
/// segments, because a model can count IDs but cannot add up times over thousands of
/// tokens: it uses this video's mean segment length (first start to last end over the
/// number of segments, pauses included), printed as measured and never clamped. With
/// no usable segments it assumes 3 s. A span counts both its ends, so M segments last
/// M times the mean. A video under 70 s gets a different rule: one candidate, no
/// 45 to 60 s aim. Segments longer than 12 s on average get no payoff offset, and over
/// 20 s the limit is stated in plain terms (a span of one or two segments).
pub fn system_prompt(segments: &[Segment]) -> String {
    let guide = length_guide(segments);
    let (candidates, rule) = if guide.video_s > 0.0 && guide.video_s < SHORT_VIDEO_S {
        (
            "one self-contained span",
            LENGTH_RULE_SHORT
                .trim_end()
                .replace("{VIDEO_SECONDS}", &format!("{:.1}", guide.video_s)),
        )
    } else {
        let span_limit = match guide.max_segments {
            1 => {
                let mut text = "Segments here are long: a span is a single segment, so \
                    cutoff_segment is start_segment."
                    .to_string();
                if guide.mean_s > 60.0 {
                    text.push_str(" Even one segment is over 60 s, so choose the shortest one.");
                }
                text
            }
            2 => "Segments here are long: a span is one or two segments, so cutoff_segment \
                is start_segment or the next ID."
                .to_string(),
            max => format!(
                "A 50 s span is about {} counting both ends, so cutoff_segment is about {} IDs \
                 after start_segment. Never use a span of more than {} counting both ends \
                 (cutoff_segment - start_segment at most {}).",
                segments_phrase(guide.aim_segments),
                guide.aim_segments.saturating_sub(1),
                segments_phrase(max),
                max - 1
            ),
        };
        let payoff = guide.payoff_offset.map_or_else(String::new, |offset| {
            format!(
                " Put payoff_segment about {} after start_segment (6 to 12 s in).",
                segments_phrase(offset)
            )
        });
        (
            "3 to 5 self-contained spans",
            LENGTH_RULE
                .trim_end()
                .replace("{SEGMENT_SECONDS}", &format!("{:.1}", guide.mean_s))
                .replace("{SPAN_LIMIT}", &span_limit)
                .replace("{PAYOFF_PLACEMENT}", &payoff),
        )
    };
    SYSTEM_PROMPT
        .replace("{CANDIDATE_COUNT}", candidates)
        .replace("{LENGTH_RULE}", &rule)
}

/// What the prompt says about span length, counted in this video's segments.
#[derive(Debug, Clone, PartialEq)]
struct LengthGuide {
    /// Mean segment length, never clamped. 3 s when it cannot be measured.
    mean_s: f64,
    /// First start to last end, in seconds; 0 with no usable segments.
    video_s: f64,
    /// The most segments, counting both ends, whose span at the mean is at most 60 s
    /// (at least 1, even when one segment is longer than that).
    max_segments: u32,
    /// Segments, counting both ends, of a 50 s span; never above `max_segments`.
    aim_segments: u32,
    /// Segments after `start_segment` that put the payoff about 9 s in; `None` when
    /// one segment is already more than 12 s, so no offset can land in 6 to 12 s.
    payoff_offset: Option<u32>,
}

fn length_guide(segments: &[Segment]) -> LengthGuide {
    let video_s = match (segments.first(), segments.last()) {
        (Some(first), Some(last)) => last.end_ms.saturating_sub(first.start_ms) as f64 / 1000.0,
        _ => 0.0,
    };
    let measured = video_s / segments.len().max(1) as f64;
    let mean_s = if measured.is_finite() && measured > 0.0 {
        measured
    } else {
        3.0
    };
    let whole = |seconds: f64| seconds.max(1.0) as u32;
    let max_segments = whole((60.0 / mean_s + 1e-9).floor());
    LengthGuide {
        mean_s,
        video_s,
        max_segments,
        aim_segments: whole((50.0 / mean_s).round()).min(max_segments),
        payoff_offset: (mean_s <= 12.0).then(|| whole((9.0 / mean_s).round())),
    }
}

/// The strict JSON schema of the model's reply.
pub fn response_schema() -> Value {
    serde_json::from_str(REPLY_SCHEMA).unwrap_or(Value::Null)
}

/// An image ready to send: one frame (one time) or a sheet (its times in reading order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedImage {
    pub times_ms: Vec<u64>,
    pub jpeg: Vec<u8>,
    /// Tiles of several frames. A sheet can hold a single frame (the last of a set), and it
    /// is still labelled as a sheet, because the image has empty tiles.
    pub sheet: bool,
}

/// Introduces the numbered segments in the user message.
const TRANSCRIPT_HEADER: &str = "Transcript segments:\n";

/// A time in tenths of a second, rounded half up.
fn tenths(ms: u64) -> u64 {
    ms.saturating_add(50) / 100
}

/// `m:ss.s`, with minutes unbounded.
fn clock(ms: u64) -> String {
    let t = tenths(ms);
    format!("{}:{:02}.{}", t / 600, t % 600 / 10, t % 10)
}

/// `303.0 s`.
fn seconds(ms: u64) -> String {
    let t = tenths(ms);
    format!("{}.{} s", t / 10, t % 10)
}

/// `5:03.0 (303.0 s)`.
fn stamp(ms: u64) -> String {
    format!("{} ({})", clock(ms), seconds(ms))
}

/// The label that precedes a single frame: `Frame at 5:03.0 (303.0 s)`.
pub fn frame_label(time_ms: u64) -> String {
    format!("Frame at {}", stamp(time_ms))
}

/// The label that precedes a sheet: its frame times in reading order.
pub fn sheet_label(times_ms: &[u64]) -> String {
    let noun = if times_ms.len() == 1 {
        "frame"
    } else {
        "frames"
    };
    let times: Vec<String> = times_ms.iter().map(|&t| stamp(t)).collect();
    format!(
        "Sheet of {} {noun}, left to right then top to bottom, at {}",
        times_ms.len(),
        times.join(", ")
    )
}

/// The user message: each image preceded by its label, then the numbered segments as one
/// text part.
pub fn build_parts(images: &[EncodedImage], segments: &[Segment]) -> Vec<Part> {
    let mut parts = Vec::with_capacity(images.len() * 2 + 1);
    for image in images {
        let label = match image.times_ms.as_slice() {
            [time_ms] if !image.sheet => frame_label(*time_ms),
            times_ms => sheet_label(times_ms),
        };
        parts.push(Part::Text(label));
        parts.push(Part::ImageJpeg(image.jpeg.clone()));
    }
    parts.push(Part::Text(format!(
        "{TRANSCRIPT_HEADER}{}",
        segment_lines(segments)
    )));
    parts
}

/// The segments, one per line: `G12 [0:41.2-0:43.0] text`, with a trailing `…` on a
/// segment that ends at a pause in the middle of a sentence (`ends_sentence` false).
pub fn segment_lines(segments: &[Segment]) -> String {
    let lines: Vec<String> = segments
        .iter()
        .map(|s| {
            let mark = if s.ends_sentence { "" } else { "…" };
            format!(
                "G{} [{}-{}] {}{mark}",
                s.id,
                clock(s.start_ms),
                clock(s.end_ms),
                s.text
            )
        })
        .collect();
    lines.join("\n")
}

/// Input tokens for a request: `images * IMAGE_TOKEN_ESTIMATE`, plus about one token
/// per 3 characters of `prompt_text` and `segment_text`. It overestimates on purpose.
pub fn estimate_input_tokens(images: usize, prompt_text: &str, segment_text: &str) -> u64 {
    let text = |s: &str| {
        let chars = u64::try_from(s.chars().count()).unwrap_or(u64::MAX);
        chars.div_ceil(3)
    };
    u64::try_from(images)
        .unwrap_or(u64::MAX)
        .saturating_mul(IMAGE_TOKEN_ESTIMATE)
        .saturating_add(text(prompt_text))
        .saturating_add(text(segment_text))
}

/// Endpoints whose quantization is in `quants`, ignoring case. An endpoint that states
/// none counts as `unknown`.
pub fn eligible_endpoints<'a>(
    endpoints: &'a [EndpointInfo],
    quants: &[String],
) -> Vec<&'a EndpointInfo> {
    endpoints
        .iter()
        .filter(|endpoint| {
            let quantization = endpoint.quantization.as_deref().unwrap_or("unknown");
            quants.iter().any(|q| q.eq_ignore_ascii_case(quantization))
        })
        .collect()
}

/// An upper bound on the cost in US dollars: input tokens at the highest prompt price
/// among `endpoints`, plus `max_tokens` at the highest completion price.
pub fn cost_upper_bound(input_tokens: u64, max_tokens: u32, endpoints: &[&EndpointInfo]) -> f64 {
    let highest =
        |price: fn(&EndpointInfo) -> f64| endpoints.iter().map(|e| price(e)).fold(0.0, f64::max);
    input_tokens as f64 * highest(|e| e.prompt_usd_per_token)
        + f64::from(max_tokens) * highest(|e| e.completion_usd_per_token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ClipPick, RawBeat, RawReply, RawSummary, RawVisualEvent, VisualEventKind};
    use crate::IMAGE_TOKEN_ESTIMATE;
    use serde_json::json;

    fn seg(id: u32, start_ms: u64, end_ms: u64, text: &str, ends_sentence: bool) -> Segment {
        Segment {
            id,
            start_ms,
            end_ms,
            text: text.to_string(),
            ends_sentence,
        }
    }

    fn endpoint(quant: Option<&str>, prompt: f64, completion: f64) -> EndpointInfo {
        EndpointInfo {
            provider: "p".to_string(),
            quantization: quant.map(str::to_string),
            context_length: 262_144,
            prompt_usd_per_token: prompt,
            completion_usd_per_token: completion,
        }
    }

    fn quants(list: &[&str]) -> Vec<String> {
        list.iter().map(|q| (*q).to_string()).collect()
    }

    // Schema.

    fn sample_reply() -> RawReply {
        RawReply {
            video_summary: RawSummary {
                topic: "a lecture".to_string(),
                language: "en".to_string(),
                speaker_count: 1,
                visual_formats: vec!["talking_head".to_string(), "slides".to_string()],
            },
            beats: vec![RawBeat {
                start_segment: 1,
                end_segment: 4,
                kind: "hook".to_string(),
                summary: "opens".to_string(),
                visual: "speaker".to_string(),
                insight_density: 3,
            }],
            edit_events: vec![RawVisualEvent {
                t_s: 12.0,
                kind: VisualEventKind::PunchIn,
                note: "zoom".to_string(),
            }],
            reel_candidates: vec![ClipPick {
                start_segment: 1,
                end_segment: 9,
                hook_segment: 2,
                payoff_segment: 7,
                cutoff_segment: 9,
                title: "t".to_string(),
                open_loop: "o".to_string(),
                visual_suggestion: "v".to_string(),
                why: "w".to_string(),
            }],
        }
    }

    /// A small validator for the subset of JSON Schema `reply_schema.json` uses:
    /// `type`, `required`, `properties`, `additionalProperties: false`, `items`, `enum`.
    fn validate(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
        if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
            if !allowed.contains(value) {
                return Err(format!("{path}: {value} is not in the enum"));
            }
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("object") => {
                let object = value
                    .as_object()
                    .ok_or_else(|| format!("{path}: not an object"))?;
                let properties = schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .ok_or_else(|| format!("{path}: schema has no properties"))?;
                for required in schema
                    .get("required")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    if !object.contains_key(required) {
                        return Err(format!("{path}: missing {required}"));
                    }
                }
                for (key, child) in object {
                    let child_schema = properties
                        .get(key)
                        .ok_or_else(|| format!("{path}: unexpected {key}"))?;
                    validate(child_schema, child, &format!("{path}.{key}"))?;
                }
                Ok(())
            }
            Some("array") => {
                let items = value
                    .as_array()
                    .ok_or_else(|| format!("{path}: not an array"))?;
                let item_schema = schema
                    .get("items")
                    .ok_or_else(|| format!("{path}: schema has no items"))?;
                for (i, item) in items.iter().enumerate() {
                    validate(item_schema, item, &format!("{path}[{i}]"))?;
                }
                Ok(())
            }
            Some("string") if value.is_string() => Ok(()),
            Some("integer") if value.is_i64() || value.is_u64() => Ok(()),
            Some("number") if value.is_number() => Ok(()),
            other => Err(format!("{path}: {value} is not a {other:?}")),
        }
    }

    /// The model also answers a `rank` per clip pick (the schema requires it); `ClipPick`
    /// has no such field because the order of `reel_candidates` is the ranking.
    fn as_the_model_answers(reply: &RawReply) -> Value {
        let mut value = serde_json::to_value(reply).expect("serializes");
        if let Some(picks) = value["reel_candidates"].as_array_mut() {
            for (i, pick) in picks.iter_mut().enumerate() {
                pick["rank"] = json!(i + 1);
            }
        }
        value
    }

    #[test]
    fn the_schema_parses_to_an_object_schema() {
        let schema = response_schema();
        assert!(schema.is_object(), "reply_schema.json must parse");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn the_schema_requires_the_fields_of_raw_reply() {
        let schema = response_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let value = serde_json::to_value(sample_reply()).expect("serializes");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        let mut required_sorted = required.clone();
        required_sorted.sort_unstable();
        assert_eq!(required_sorted, keys);
        let properties = schema["properties"].as_object().expect("properties");
        for key in &required {
            assert!(properties.contains_key(*key), "no property {key}");
        }
    }

    #[test]
    fn a_serialized_raw_reply_validates_against_the_schema() {
        let value = as_the_model_answers(&sample_reply());
        validate(&response_schema(), &value, "$").expect("valid");
    }

    #[test]
    fn a_reply_missing_a_required_field_does_not_validate() {
        let mut value = as_the_model_answers(&sample_reply());
        value["video_summary"]
            .as_object_mut()
            .expect("object")
            .remove("language");
        assert!(validate(&response_schema(), &value, "$").is_err());
        let mut value = as_the_model_answers(&sample_reply());
        value["beats"][0]["kind"] = json!("not_a_kind");
        assert!(validate(&response_schema(), &value, "$").is_err());
    }

    #[test]
    fn a_reply_the_schema_accepts_deserializes_into_raw_reply() {
        let value = as_the_model_answers(&sample_reply());
        let back: RawReply = serde_json::from_value(value).expect("deserializes");
        assert_eq!(back, sample_reply());
    }

    #[test]
    fn the_clip_pick_schema_matches_clip_pick_apart_from_rank() {
        let schema = response_schema();
        let pick_schema = &schema["properties"]["reel_candidates"]["items"];
        let mut required: Vec<&str> = pick_schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .filter_map(Value::as_str)
            .filter(|k| *k != "rank")
            .collect();
        required.sort_unstable();
        let value = serde_json::to_value(sample_reply().reel_candidates[0].clone()).expect("ok");
        let mut keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(required, keys);
    }

    fn timed(count: u32, each_ms: u64) -> Vec<Segment> {
        (0..count)
            .map(|i| Segment {
                id: i + 1,
                start_ms: u64::from(i) * each_ms,
                end_ms: u64::from(i + 1) * each_ms,
                text: "word".into(),
                ends_sentence: true,
            })
            .collect()
    }

    #[test]
    fn the_prompt_counts_the_length_limit_in_this_videos_segments() {
        let prompt = system_prompt(&timed(100, 3_000));
        assert!(prompt.contains("about 3.0 s each"), "{prompt}");
        assert!(
            prompt.contains("A 50 s span is about 17 segments counting both ends"),
            "{prompt}"
        );
        assert!(
            prompt.contains("cutoff_segment is about 16 IDs after start_segment"),
            "{prompt}"
        );
        assert!(
            prompt.contains("more than 20 segments counting both ends"),
            "{prompt}"
        );
        assert!(
            prompt.contains("cutoff_segment - start_segment at most 19"),
            "{prompt}"
        );
        assert!(
            prompt.contains("about 3 segments after start_segment"),
            "{prompt}"
        );
        let long = system_prompt(&timed(100, 10_000));
        assert!(
            long.contains("A 50 s span is about 5 segments counting both ends"),
            "{long}"
        );
        assert!(
            long.contains("more than 6 segments counting both ends"),
            "{long}"
        );
    }

    /// The span of `count` consecutive segments from index `start`, in seconds.
    fn span_seconds(segments: &[Segment], start: usize, count: u32) -> f64 {
        let last = start + count as usize - 1;
        (segments[last].end_ms - segments[start].start_ms) as f64 / 1000.0
    }

    #[test]
    fn the_largest_span_the_prompt_allows_never_exceeds_sixty_seconds_at_the_mean() {
        for mean_s in [0.5, 1.7, 3.0, 3.2, 6.0, 10.0, 20.0, 30.0] {
            let segments = timed(1_000, (mean_s * 1000.0) as u64);
            let guide = length_guide(&segments);
            assert!(guide.max_segments >= 1);
            let longest = span_seconds(&segments, 0, guide.max_segments);
            assert!(
                longest <= 60.0,
                "mean {mean_s}: {} segments last {longest} s",
                guide.max_segments
            );
            // Tight: one more segment would go over.
            assert!(
                span_seconds(&segments, 0, guide.max_segments + 1) > 60.0,
                "mean {mean_s}: {} is not the largest",
                guide.max_segments
            );
            assert!(guide.aim_segments <= guide.max_segments);
            let prompt = system_prompt(&segments);
            if guide.max_segments > 2 {
                let stated = format!(
                    "more than {} segments counting both ends",
                    guide.max_segments
                );
                assert!(prompt.contains(&stated), "mean {mean_s}: {prompt}");
            }
        }
    }

    #[test]
    fn a_mean_over_sixty_seconds_still_allows_one_segment_and_says_it_is_too_long() {
        let segments = timed(10, 70_000);
        assert_eq!(length_guide(&segments).max_segments, 1);
        let prompt = system_prompt(&segments);
        assert!(prompt.contains("about 70.0 s each"), "{prompt}");
        assert!(prompt.contains("a span is a single segment"), "{prompt}");
        assert!(prompt.contains("over 60 s"), "{prompt}");
    }

    #[test]
    fn a_video_under_seventy_seconds_gets_a_one_candidate_rule_without_the_aim() {
        let short = system_prompt(&timed(10, 4_000));
        assert!(short.contains("lasts only about 40.0 s"), "{short}");
        assert!(short.contains("Return one candidate"), "{short}");
        assert!(short.contains("one self-contained span"), "{short}");
        assert!(!short.contains("aim for 45 to 60 s"), "{short}");
        assert!(!short.contains("3 to 5"), "{short}");
        assert!(!short.contains("segments after start_segment"), "{short}");
        assert!(!short.contains('{'), "{short}");
        let edge = system_prompt(&timed(10, 6_990));
        assert!(edge.contains("Return one candidate"), "{edge}");
    }

    #[test]
    fn a_video_of_seventy_seconds_or_more_keeps_the_forty_five_to_sixty_aim() {
        for segments in [timed(10, 7_000), timed(30, 3_000)] {
            let prompt = system_prompt(&segments);
            assert!(prompt.contains("aim for 45 to 60 s"), "{prompt}");
            assert!(prompt.contains("3 to 5 self-contained spans"), "{prompt}");
            assert!(!prompt.contains("Return one candidate"), "{prompt}");
            assert!(!prompt.contains('{'), "{prompt}");
        }
    }

    #[test]
    fn long_segments_are_described_honestly_without_a_clamp_or_an_impossible_payoff() {
        // A mean over 30 s is printed as it is, not as 30.0.
        let prompt = system_prompt(&timed(10, 45_000));
        assert!(prompt.contains("about 45.0 s each"), "{prompt}");
        assert!(!prompt.contains("about 30.0 s each"), "{prompt}");
        // 25 s segments: a span is one or two, so no payoff offset is given.
        let twenty = system_prompt(&timed(10, 25_000));
        assert!(twenty.contains("a span is one or two segments"), "{twenty}");
        assert!(!twenty.contains("segments after start_segment"), "{twenty}");
        // Over 12 s one segment is already past the payoff window: the clause is dropped.
        let fifteen = system_prompt(&timed(100, 15_000));
        assert!(!fifteen.contains("Put payoff_segment about"), "{fifteen}");
        assert!(!fifteen.contains("(6 to 12 s in)"), "{fifteen}");
        // At 12 s or less it is kept, and one is singular.
        let ten = system_prompt(&timed(100, 10_000));
        assert!(
            ten.contains("Put payoff_segment about 1 segment after start_segment"),
            "{ten}"
        );
    }

    #[test]
    fn a_count_of_one_is_never_written_as_a_plural() {
        for each_ms in [
            500, 1_700, 3_000, 6_000, 8_000, 10_000, 12_000, 15_000, 20_000, 30_000, 45_000, 70_000,
        ] {
            let prompt = system_prompt(&timed(100, each_ms));
            assert!(!prompt.contains(" 1 segments"), "{each_ms}: {prompt}");
        }
    }

    #[test]
    fn no_placeholder_survives_and_odd_input_falls_back_to_three_seconds() {
        for segments in [
            timed(0, 0),
            timed(5, 0),
            timed(100, 0),
            timed(5, 100),
            timed(100, 3_000),
            timed(10, 4_000),
        ] {
            let prompt = system_prompt(&segments);
            assert!(!prompt.contains('{'), "{prompt}");
        }
        assert!(system_prompt(&[]).contains("about 3.0 s each"));
        assert!(system_prompt(&timed(100, 0)).contains("about 3.0 s each"));
    }

    // Labels and parts.

    #[test]
    fn a_frame_label_gives_minutes_seconds_and_total_seconds() {
        assert_eq!(frame_label(303_000), "Frame at 5:03.0 (303.0 s)");
        assert_eq!(frame_label(0), "Frame at 0:00.0 (0.0 s)");
        assert_eq!(frame_label(41_250), "Frame at 0:41.3 (41.3 s)");
        assert_eq!(frame_label(3_723_400), "Frame at 62:03.4 (3723.4 s)");
    }

    #[test]
    fn a_frame_label_rounds_to_a_tenth_without_showing_sixty_seconds() {
        assert_eq!(frame_label(59_949), "Frame at 0:59.9 (59.9 s)");
        assert_eq!(frame_label(59_950), "Frame at 1:00.0 (60.0 s)");
    }

    #[test]
    fn a_sheet_label_lists_the_times_in_reading_order() {
        assert_eq!(
            sheet_label(&[5_000, 10_000, 303_000, 4_000]),
            "Sheet of 4 frames, left to right then top to bottom, at 0:05.0 (5.0 s), \
             0:10.0 (10.0 s), 5:03.0 (303.0 s), 0:04.0 (4.0 s)"
        );
    }

    #[test]
    fn a_sheet_label_of_one_frame_says_frame_not_frames() {
        assert_eq!(
            sheet_label(&[5_000]),
            "Sheet of 1 frame, left to right then top to bottom, at 0:05.0 (5.0 s)"
        );
    }

    #[test]
    fn build_parts_precedes_each_image_with_its_label_then_one_transcript_part() {
        let images = vec![
            EncodedImage {
                times_ms: vec![303_000],
                jpeg: vec![1, 2, 3],
                sheet: false,
            },
            EncodedImage {
                times_ms: vec![0, 5_000],
                jpeg: vec![4],
                sheet: true,
            },
            // The last sheet of five frames in 2x2 tiles holds one frame.
            EncodedImage {
                times_ms: vec![25_000],
                jpeg: vec![5],
                sheet: true,
            },
        ];
        let segments = vec![
            seg(1, 0, 1_500, "Hello there", false),
            seg(2, 1_900, 3_000, "Done.", true),
        ];
        let parts = build_parts(&images, &segments);
        assert_eq!(
            parts,
            vec![
                Part::Text(frame_label(303_000)),
                Part::ImageJpeg(vec![1, 2, 3]),
                Part::Text(sheet_label(&[0, 5_000])),
                Part::ImageJpeg(vec![4]),
                Part::Text(sheet_label(&[25_000])),
                Part::ImageJpeg(vec![5]),
                Part::Text(format!(
                    "Transcript segments:\n{}",
                    segment_lines(&segments)
                )),
            ]
        );
        assert_eq!(
            parts[0],
            Part::Text("Frame at 5:03.0 (303.0 s)".to_string())
        );
    }

    #[test]
    fn build_parts_with_no_images_is_just_the_transcript() {
        let segments = vec![seg(1, 0, 1_500, "Hi.", true)];
        assert_eq!(
            build_parts(&[], &segments),
            vec![Part::Text(
                "Transcript segments:\nG1 [0:00.0-0:01.5] Hi.".to_string()
            )]
        );
    }

    // Segments.

    #[test]
    fn segment_lines_are_numbered_by_id_with_times_and_the_pause_mark() {
        let segments = vec![
            seg(12, 41_200, 43_000, "so the point is", false),
            seg(13, 303_700, 305_100, "Done.", true),
        ];
        assert_eq!(
            segment_lines(&segments),
            "G12 [0:41.2-0:43.0] so the point is…\nG13 [5:03.7-5:05.1] Done."
        );
    }

    #[test]
    fn segment_lines_let_minutes_run_past_an_hour() {
        let segments = vec![seg(400, 3_723_400, 3_725_000, "late", true)];
        assert_eq!(segment_lines(&segments), "G400 [62:03.4-62:05.0] late");
    }

    #[test]
    fn segment_lines_round_to_a_tenth() {
        let segments = vec![seg(1, 59_949, 59_950, "x", true)];
        assert_eq!(segment_lines(&segments), "G1 [0:59.9-1:00.0] x");
    }

    #[test]
    fn segment_lines_of_nothing_is_empty() {
        assert_eq!(segment_lines(&[]), "");
    }

    // Tokens.

    #[test]
    fn the_estimate_is_images_plus_a_third_of_the_characters_rounded_up() {
        assert_eq!(
            estimate_input_tokens(2, "abcd", "abcdef"),
            2 * IMAGE_TOKEN_ESTIMATE + 2 + 2
        );
        assert_eq!(estimate_input_tokens(0, "", ""), 0);
        assert_eq!(estimate_input_tokens(0, "abc", ""), 1);
        assert_eq!(estimate_input_tokens(0, "abcd", ""), 2);
    }

    #[test]
    fn the_estimate_counts_characters_not_bytes() {
        // Four `…` are 12 bytes but 4 characters: 2 tokens, not 4.
        assert_eq!(estimate_input_tokens(0, "…………", ""), 2);
        assert_eq!(estimate_input_tokens(0, "", "…"), 1);
    }

    #[test]
    fn the_estimate_saturates() {
        assert_eq!(estimate_input_tokens(usize::MAX, "abc", "abc"), u64::MAX);
    }

    // Endpoints and cost.

    #[test]
    fn eligible_endpoints_match_the_quantization_ignoring_case() {
        let endpoints = vec![
            endpoint(Some("fp8"), 1.0, 1.0),
            endpoint(Some("BF16"), 1.0, 1.0),
            endpoint(Some("int4"), 1.0, 1.0),
            endpoint(None, 1.0, 1.0),
        ];
        let picked = eligible_endpoints(&endpoints, &quants(&["Fp8", "bf16"]));
        let got: Vec<Option<&str>> = picked.iter().map(|e| e.quantization.as_deref()).collect();
        assert_eq!(got, vec![Some("fp8"), Some("BF16")]);
    }

    #[test]
    fn an_endpoint_without_a_quantization_needs_unknown_in_the_list() {
        let endpoints = vec![endpoint(None, 1.0, 1.0), endpoint(Some("fp8"), 1.0, 1.0)];
        let picked = eligible_endpoints(&endpoints, &quants(&["unknown"]));
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].quantization, None);
        let picked = eligible_endpoints(&endpoints, &quants(&["UNKNOWN", "fp8"]));
        assert_eq!(picked.len(), 2);
    }

    #[test]
    fn an_endpoint_that_states_unknown_matches_unknown() {
        let endpoints = vec![endpoint(Some("unknown"), 1.0, 1.0)];
        assert_eq!(
            eligible_endpoints(&endpoints, &quants(&["unknown"])).len(),
            1
        );
        assert!(eligible_endpoints(&endpoints, &quants(&["fp8"])).is_empty());
    }

    #[test]
    fn no_quantizations_allowed_means_no_endpoints() {
        let endpoints = vec![endpoint(Some("fp8"), 1.0, 1.0), endpoint(None, 1.0, 1.0)];
        assert!(eligible_endpoints(&endpoints, &[]).is_empty());
    }

    #[test]
    fn the_cost_bound_uses_the_highest_price_of_each_kind() {
        // A is cheap to read and dear to write; B the reverse. The bound takes the dearest of each.
        let a = endpoint(Some("fp8"), 1e-7, 4e-7);
        let b = endpoint(Some("fp8"), 3e-7, 2e-7);
        let bound = cost_upper_bound(100_000, 16_000, &[&a, &b]);
        let by_hand = 100_000.0 * 3e-7 + 16_000.0 * 4e-7;
        assert!((bound - by_hand).abs() < 1e-12, "{bound} vs {by_hand}");
        assert!((by_hand - 0.0364).abs() < 1e-12);
    }

    #[test]
    fn the_cost_bound_of_one_endpoint_is_its_own_prices() {
        let a = endpoint(Some("fp8"), 2e-7, 5e-7);
        let bound = cost_upper_bound(1_000_000, 16_000, &[&a]);
        assert!((bound - (0.2 + 0.008)).abs() < 1e-12, "{bound}");
    }

    #[test]
    fn the_cost_bound_with_no_endpoints_is_zero() {
        assert_eq!(cost_upper_bound(1_000_000, 16_000, &[]), 0.0);
    }

    #[test]
    fn the_estimate_feeds_the_bound_as_a_hand_count() {
        let a = endpoint(Some("fp8"), 1e-7, 4e-7);
        let b = endpoint(Some("fp8"), 3e-7, 2e-7);
        let tokens = estimate_input_tokens(86, "abc", "abcdef");
        assert_eq!(tokens, 86 * 1_445 + 1 + 2);
        let bound = cost_upper_bound(tokens, 16_000, &[&a, &b]);
        let by_hand = 124_273.0 * 3e-7 + 16_000.0 * 4e-7;
        assert!((bound - by_hand).abs() < 1e-12, "{bound} vs {by_hand}");
    }
}
