//! The prompt, the strict reply schema, token estimates, and the cost bound.

use serde_json::Value;

use crate::backend::Part;
use crate::catalog::EndpointInfo;
use crate::types::Segment;
use crate::IMAGE_TOKEN_ESTIMATE;

const SYSTEM_PROMPT: &str = include_str!("prompt.md");
const REPLY_SCHEMA: &str = include_str!("reply_schema.json");

/// The system prompt (the Component 3 rubric).
pub fn system_prompt() -> &'static str {
    SYSTEM_PROMPT
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
}

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
            [time_ms] => frame_label(*time_ms),
            times_ms => sheet_label(times_ms),
        };
        parts.push(Part::Text(label));
        parts.push(Part::ImageJpeg(image.jpeg.clone()));
    }
    parts.push(Part::Text(segment_lines(segments)));
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

    #[test]
    fn the_system_prompt_is_not_empty() {
        assert!(system_prompt().contains("segment"));
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
            },
            EncodedImage {
                times_ms: vec![0, 5_000],
                jpeg: vec![4],
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
                Part::Text(segment_lines(&segments)),
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
            vec![Part::Text("G1 [0:00.0-0:01.5] Hi.".to_string())]
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
