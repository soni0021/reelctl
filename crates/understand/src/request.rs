//! The prompt, the strict reply schema, token estimates, and the cost bound.

use serde_json::Value;

use crate::catalog::EndpointInfo;
use crate::types::Segment;

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

/// The segments, one per line: `G12 [0:41.2-0:43.0] text`, with a trailing `…` on a
/// segment that ends at a pause in the middle of a sentence (`ends_sentence` false).
pub fn segment_lines(segments: &[Segment]) -> String {
    // Stub: the transcript format is built in its own task.
    let _ = segments;
    String::new()
}

/// Input tokens for a request: `images * IMAGE_TOKEN_ESTIMATE`, plus about one token
/// per 3 characters of `prompt_text` and `segment_text`. It overestimates on purpose.
pub fn estimate_input_tokens(images: usize, prompt_text: &str, segment_text: &str) -> u64 {
    // Stub: the estimate is built in its own task.
    let _ = (images, prompt_text, segment_text);
    0
}

/// Endpoints whose quantization is in `quants`.
pub fn eligible_endpoints<'a>(
    endpoints: &'a [EndpointInfo],
    quants: &[String],
) -> Vec<&'a EndpointInfo> {
    // Stub: the filter is built in its own task.
    let _ = (endpoints, quants);
    Vec::new()
}

/// An upper bound on the cost in US dollars: input tokens at the highest prompt price
/// among `endpoints`, plus `max_tokens` at the highest completion price.
pub fn cost_upper_bound(input_tokens: u64, max_tokens: u32, endpoints: &[&EndpointInfo]) -> f64 {
    // Stub: the bound is built in its own task.
    let _ = (input_tokens, max_tokens, endpoints);
    0.0
}
