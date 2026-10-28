//! The hosted backend and catalog (OpenRouter).
//!
//! The API key is read from the process environment only ([`key_from_env`]), is
//! sent only as an `Authorization` header, and is never printed, logged, or written:
//! [`OpenRouterBackend`] has no `Debug` output that shows it, and every error that
//! leaves [`OpenRouterBackend::analyze`] passes through [`scrub_error`], the one place
//! that removes it. Every request carries `data_collection: deny` and
//! `require_parameters: true`, and there is no retry with looser settings: if no
//! provider qualifies, the call fails and says so.
//!
//! The pure parts (request body, event stream, listing, error mapping) are plain
//! functions with offline tests. The HTTP calls are covered by a loopback server in
//! the tests; the live call is made by hand.

use std::fmt;
use std::io::{BufRead, BufReader, Read};
use std::time::{Duration, Instant};

use base64::Engine;
use serde_json::{json, Value};

use crate::backend::{Part, VlmBackend, VlmReply, VlmRequest};
use crate::catalog::{EndpointInfo, ModelCatalog};
use crate::error::{BackendError, CatalogError};
use crate::types::RawReply;

/// The default API root.
pub const OPENROUTER_API: &str = "https://openrouter.ai/api/v1";

/// The environment variable that holds the key.
pub const KEY_VARIABLE: &str = "OPENROUTER_API_KEY";

/// The name the strict schema is sent under.
const SCHEMA_NAME: &str = "edit_semantics_frames";

/// How long to wait for the whole streamed reply (the slowest model took 108 s on a 28 min lecture).
const BODY_TIMEOUT: Duration = Duration::from_secs(1_200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The key from the environment, if it is set and not empty.
pub fn key_from_env() -> Option<String> {
    std::env::var(KEY_VARIABLE)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .build()
        .into()
}

/// Remove `secret` from text that may be shown to the user.
fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "[key]")
    }
}

/// The longest error text that leaves this module.
const ERROR_TEXT_LIMIT: usize = 2_000;

/// `error` with `key` removed from every text in it, then cut to [`ERROR_TEXT_LIMIT`].
///
/// This is the single place errors are made safe to show. The match has no wildcard, so
/// a new [`BackendError`] variant does not compile until it is handled here. The key is
/// removed before the text is cut, so the cut cannot leave a piece of it.
fn scrub_error(error: BackendError, key: &str) -> BackendError {
    let clean = |text: String| shorten(&scrub(&text, key), ERROR_TEXT_LIMIT);
    match error {
        BackendError::NoProvider { model, reason } => BackendError::NoProvider {
            model: clean(model),
            reason: clean(reason),
        },
        BackendError::Http(text) => BackendError::Http(clean(text)),
        BackendError::Status { status, body } => BackendError::Status {
            status,
            body: clean(body),
        },
        BackendError::BadReply(text) => BackendError::BadReply(clean(text)),
    }
}

/// At most `limit` characters, for error bodies.
fn shorten(text: &str, limit: usize) -> String {
    let mut out: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

fn data_url(jpeg: &[u8]) -> String {
    format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(jpeg)
    )
}

/// The JSON body of a chat completion for `request`. The key is not in it.
pub fn build_body(request: &VlmRequest) -> Value {
    let content: Vec<Value> = request
        .parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => json!({"type": "text", "text": text}),
            Part::ImageJpeg(bytes) => {
                json!({"type": "image_url", "image_url": {"url": data_url(bytes)}})
            }
        })
        .collect();
    let mut body = json!({
        "model": request.model,
        "messages": [
            {"role": "system", "content": request.system},
            {"role": "user", "content": content},
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {"name": SCHEMA_NAME, "strict": true, "schema": request.schema},
        },
        "provider": {
            "require_parameters": true,
            "data_collection": "deny",
            "quantizations": request.quantizations,
        },
        "max_tokens": request.max_tokens,
        "stream": true,
        "usage": {"include": true},
    });
    if let (Some(effort), Some(object)) = (&request.effort, body.as_object_mut()) {
        object.insert("reasoning".to_string(), json!({"effort": effort}));
    }
    body
}

/// What a streamed reply held.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Streamed {
    pub content: String,
    pub usage: Option<Value>,
    pub provider: Option<String>,
    pub finish_reason: Option<String>,
}

/// Read server-sent events: concatenate the content deltas, keep the usage, the
/// provider that served the call, and the finish reason.
pub(crate) fn read_stream(reader: impl BufRead) -> Result<Streamed, BackendError> {
    let mut streamed = Streamed::default();
    for line in reader.lines() {
        let line = line.map_err(|err| BackendError::Http(format!("reading the reply: {err}")))?;
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }
        let event: Value = serde_json::from_str(data)
            .map_err(|err| BackendError::BadReply(format!("an event was not JSON: {err}")))?;
        if let Some(error) = event.get("error") {
            return Err(BackendError::BadReply(format!(
                "the stream reported an error: {error}"
            )));
        }
        if let Some(provider) = event.get("provider").and_then(Value::as_str) {
            streamed.provider = Some(provider.to_string());
        }
        if let Some(usage) = event.get("usage").filter(|usage| !usage.is_null()) {
            streamed.usage = Some(usage.clone());
        }
        for choice in event
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str) {
                streamed.content.push_str(text);
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                streamed.finish_reason = Some(reason.to_string());
            }
        }
    }
    Ok(streamed)
}

/// The reply, parsed, with what the call used.
pub(crate) fn reply_from(streamed: Streamed, elapsed_ms: u64) -> Result<VlmReply, BackendError> {
    if streamed.finish_reason.as_deref() == Some("length") {
        return Err(BackendError::BadReply(
            "the reply was cut off at max_tokens; raise it or lower the image count".to_string(),
        ));
    }
    let reply: RawReply = serde_json::from_str(streamed.content.trim()).map_err(|err| {
        BackendError::BadReply(format!(
            "the reply is not the JSON the schema asks for: {err}"
        ))
    })?;
    let usage = streamed.usage.unwrap_or(Value::Null);
    let count = |name: &str| usage.get(name).and_then(Value::as_u64).unwrap_or(0);
    Ok(VlmReply {
        reply,
        provider: streamed.provider.unwrap_or_default(),
        quantization: None,
        input_tokens: count("prompt_tokens"),
        output_tokens: count("completion_tokens"),
        cost_usd: usage.get("cost").and_then(Value::as_f64),
        elapsed_ms,
    })
}

/// An HTTP error status as a [`BackendError`]. The text is not yet safe to show:
/// [`scrub_error`] makes it so.
pub(crate) fn status_error(status: u16, body: &str, model: &str) -> BackendError {
    if status == 404 && body.to_lowercase().contains("no endpoints") {
        return BackendError::NoProvider {
            model: model.to_string(),
            reason: body.to_string(),
        };
    }
    BackendError::Status {
        status,
        body: body.to_string(),
    }
}

/// The hosted model backend.
pub struct OpenRouterBackend {
    key: String,
    base_url: String,
    agent: ureq::Agent,
}

impl fmt::Debug for OpenRouterBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenRouterBackend")
            .field("base_url", &self.base_url)
            .field("key", &"[hidden]")
            .finish()
    }
}

impl OpenRouterBackend {
    pub fn new(key: String) -> Self {
        Self::with_base_url(key, OPENROUTER_API.to_string())
    }

    pub fn with_base_url(key: String, base_url: String) -> Self {
        Self {
            key,
            base_url,
            agent: agent(),
        }
    }
}

impl OpenRouterBackend {
    /// The call, with error text exactly as the transport and the server gave it.
    fn call(&self, request: &VlmRequest) -> Result<VlmReply, BackendError> {
        let body = build_body(request);
        let started = Instant::now();
        let mut response = self
            .agent
            .post(&format!("{}/chat/completions", self.base_url))
            .header("Authorization", &format!("Bearer {}", self.key))
            .header("Content-Type", "application/json")
            .header("X-Title", "reelctl understand")
            .send_json(&body)
            .map_err(|err| BackendError::Http(err.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            let text = response.body_mut().read_to_string().unwrap_or_default();
            return Err(status_error(status, &text, &request.model));
        }
        let reader = BufReader::new(response.body_mut().as_reader());
        let streamed = read_stream(reader)?;
        reply_from(streamed, started.elapsed().as_millis() as u64)
    }
}

impl VlmBackend for OpenRouterBackend {
    fn analyze(&self, request: &VlmRequest) -> Result<VlmReply, BackendError> {
        self.call(request)
            .map_err(|error| scrub_error(error, &self.key))
    }
}

/// Parse OpenRouter's listing of a model's endpoints (`/models/{id}/endpoints`).
pub(crate) fn parse_endpoints(listing: &str) -> Result<Vec<EndpointInfo>, CatalogError> {
    let value: Value = serde_json::from_str(listing)
        .map_err(|err| CatalogError::Listing(format!("not JSON: {err}")))?;
    let endpoints = value
        .pointer("/data/endpoints")
        .and_then(Value::as_array)
        .ok_or_else(|| CatalogError::Listing("no data.endpoints in the listing".to_string()))?;
    let price = |endpoint: &Value, name: &str| -> Result<f64, CatalogError> {
        endpoint
            .pointer(&format!("/pricing/{name}"))
            .and_then(|value| match value {
                Value::String(text) => text.parse::<f64>().ok(),
                Value::Number(number) => number.as_f64(),
                _ => None,
            })
            .ok_or_else(|| CatalogError::Listing(format!("an endpoint has no {name} price")))
    };
    endpoints
        .iter()
        .map(|endpoint| {
            Ok(EndpointInfo {
                provider: endpoint
                    .get("provider_name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                quantization: endpoint
                    .get("quantization")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                context_length: endpoint
                    .get("context_length")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| {
                        CatalogError::Listing("an endpoint has no context_length".to_string())
                    })?,
                prompt_usd_per_token: price(endpoint, "prompt")?,
                completion_usd_per_token: price(endpoint, "completion")?,
            })
        })
        .collect()
}

/// OpenRouter's public listing, which needs no key.
#[derive(Debug)]
pub struct OpenRouterCatalog {
    base_url: String,
    agent: ureq::Agent,
}

impl Default for OpenRouterCatalog {
    fn default() -> Self {
        Self::with_base_url(OPENROUTER_API.to_string())
    }
}

impl OpenRouterCatalog {
    pub fn with_base_url(base_url: String) -> Self {
        Self {
            base_url,
            agent: agent(),
        }
    }
}

/// Model IDs look like `vendor/name:variant`; anything else is refused before it
/// reaches a URL. Every `/`-separated segment must be non-empty and not `.` or `..`, so
/// an ID cannot climb out of the `/models/` path.
fn valid_model_id(model: &str) -> bool {
    !model.is_empty()
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
        && model
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

impl ModelCatalog for OpenRouterCatalog {
    fn endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>, CatalogError> {
        if !valid_model_id(model) {
            return Err(CatalogError::UnknownModel(model.to_string()));
        }
        let mut response = self
            .agent
            .get(&format!("{}/models/{model}/endpoints", self.base_url))
            .call()
            .map_err(|err| CatalogError::Listing(err.to_string()))?;
        let status = response.status().as_u16();
        let mut text = String::new();
        response
            .body_mut()
            .as_reader()
            .read_to_string(&mut text)
            .map_err(|err| CatalogError::Listing(err.to_string()))?;
        match status {
            200 => parse_endpoints(&text),
            404 => Err(CatalogError::UnknownModel(model.to_string())),
            other => Err(CatalogError::Listing(format!(
                "the listing answered {other}: {}",
                shorten(&text, 300)
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    const KEY: &str = "sk-or-v1-dummy-key-for-tests";

    fn request() -> VlmRequest {
        VlmRequest {
            model: "qwen/qwen3.5-9b".to_string(),
            system: "system text".to_string(),
            parts: vec![
                Part::Text("Frame at 0:00.0".to_string()),
                Part::ImageJpeg(vec![0xFF, 0xD8, 0xFF, 0xD9]),
                Part::Text("G1 [0:00.0-0:01.0] hello.".to_string()),
            ],
            schema: json!({"type": "object", "properties": {}}),
            max_tokens: 16_000,
            effort: None,
            quantizations: vec!["fp8".to_string(), "bf16".to_string()],
        }
    }

    fn reply_json() -> String {
        json!({
            "video_summary": {"topic": "t", "language": "en", "speaker_count": 1, "visual_formats": ["slides"]},
            "beats": [],
            "edit_events": [],
            "reel_candidates": []
        })
        .to_string()
    }

    /// A server-sent stream as OpenRouter sends it, with a keep-alive comment first.
    fn stream_text(content: &str, finish: &str) -> String {
        let half = content.len() / 2;
        let (a, b) = content.split_at(half);
        let event = |value: Value| format!("data: {value}\n\n");
        [
            ": OPENROUTER PROCESSING\n\n".to_string(),
            event(json!({"provider": "DeepInfra", "choices": [{"delta": {"content": a}}]})),
            event(json!({"provider": "DeepInfra", "choices": [{"delta": {"content": b}, "finish_reason": finish}]})),
            event(json!({"choices": [], "usage": {"prompt_tokens": 1234, "completion_tokens": 56, "cost": 0.0071}})),
            "data: [DONE]\n\n".to_string(),
        ]
        .concat()
    }

    #[test]
    fn the_body_always_denies_data_collection_and_requires_parameters() {
        let body = build_body(&request());
        assert_eq!(body["provider"]["data_collection"], "deny");
        assert_eq!(body["provider"]["require_parameters"], true);
        assert_eq!(body["provider"]["quantizations"], json!(["fp8", "bf16"]));
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
        assert_eq!(body["max_tokens"], 16_000);
        assert_eq!(body["stream"], true);
        assert_eq!(body["usage"]["include"], true);
    }

    #[test]
    fn the_body_carries_the_images_in_order_and_no_key() {
        let body = build_body(&request());
        let content = body["messages"][1]["content"].as_array().expect("content");
        assert_eq!(content[0]["text"], "Frame at 0:00.0");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(
            content[1]["image_url"]["url"],
            "data:image/jpeg;base64,/9j/2Q=="
        );
        assert_eq!(content[2]["text"], "G1 [0:00.0-0:01.0] hello.");
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(!body.to_string().contains(KEY));
    }

    #[test]
    fn reasoning_is_sent_only_when_asked_for() {
        assert!(build_body(&request()).get("reasoning").is_none());
        let mut asked = request();
        asked.effort = Some("low".to_string());
        assert_eq!(build_body(&asked)["reasoning"]["effort"], "low");
    }

    #[test]
    fn a_stream_gives_its_content_usage_and_provider() {
        let text = stream_text(&reply_json(), "stop");
        let streamed = read_stream(text.as_bytes()).expect("stream");
        assert_eq!(streamed.content, reply_json());
        assert_eq!(streamed.provider.as_deref(), Some("DeepInfra"));
        assert_eq!(streamed.finish_reason.as_deref(), Some("stop"));
        let reply = reply_from(streamed, 1_500).expect("reply");
        assert_eq!(
            (reply.input_tokens, reply.output_tokens, reply.cost_usd),
            (1234, 56, Some(0.0071))
        );
        assert_eq!(
            (reply.provider.as_str(), reply.elapsed_ms),
            ("DeepInfra", 1_500)
        );
        assert_eq!(reply.reply.video_summary.topic, "t");
    }

    #[test]
    fn an_error_event_fails_the_call() {
        let text =
            "data: {\"error\": {\"message\": \"provider returned error\", \"code\": 502}}\n\n";
        let err = read_stream(text.as_bytes()).expect_err("error");
        assert!(
            matches!(err, BackendError::BadReply(ref m) if m.contains("provider returned error")),
            "{err}"
        );
    }

    #[test]
    fn a_reply_cut_off_at_max_tokens_is_an_error_and_so_is_text_that_is_not_the_schema() {
        let cut = read_stream(stream_text(&reply_json(), "length").as_bytes()).expect("stream");
        let err = reply_from(cut, 1).expect_err("cut off");
        assert!(
            matches!(err, BackendError::BadReply(ref m) if m.contains("max_tokens")),
            "{err}"
        );
        let prose = read_stream(stream_text("Sure! Here is the answer.", "stop").as_bytes())
            .expect("stream");
        assert!(matches!(
            reply_from(prose, 1),
            Err(BackendError::BadReply(_))
        ));
    }

    #[test]
    fn a_status_error_has_no_key_and_a_short_body() {
        let body = format!("bad request, key {KEY} rejected {}", "x".repeat(5_000));
        let err = scrub_error(status_error(401, &body, "m"), KEY);
        let text = err.to_string();
        assert!(!text.contains(KEY), "{text}");
        assert!(text.contains("[key]"));
        assert!(text.chars().count() < 2_200, "{}", text.chars().count());
        assert!(matches!(err, BackendError::Status { status: 401, .. }));
    }

    #[test]
    fn no_matching_provider_is_its_own_error() {
        let err = status_error(404, "No endpoints found matching your data policy", "m");
        assert!(matches!(err, BackendError::NoProvider { .. }), "{err}");
    }

    #[test]
    fn debug_output_hides_the_key() {
        let backend = OpenRouterBackend::new(KEY.to_string());
        let shown = format!("{backend:?}");
        assert!(!shown.contains(KEY), "{shown}");
        assert!(shown.contains("[hidden]"));
    }

    fn listing() -> String {
        json!({"data": {"id": "qwen/qwen3.5-9b", "endpoints": [
            {"provider_name": "DeepInfra", "quantization": "bf16", "context_length": 262144,
             "pricing": {"prompt": "0.0000001", "completion": "0.00000015"}},
            {"provider_name": "Meta", "quantization": null, "context_length": 1000000,
             "pricing": {"prompt": "0.000002", "completion": "0.00001"}}
        ]}})
        .to_string()
    }

    #[test]
    fn a_listing_becomes_endpoints_with_prices() {
        let endpoints = parse_endpoints(&listing()).expect("listing");
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0].provider, "DeepInfra");
        assert_eq!(endpoints[0].quantization.as_deref(), Some("bf16"));
        assert_eq!(endpoints[0].context_length, 262_144);
        assert_eq!(endpoints[0].prompt_usd_per_token, 0.0000001);
        assert_eq!(endpoints[0].completion_usd_per_token, 0.00000015);
        assert_eq!(endpoints[1].quantization, None);
    }

    #[test]
    fn a_listing_without_prices_or_endpoints_is_an_error() {
        assert!(parse_endpoints("not json").is_err());
        assert!(parse_endpoints("{\"data\": {}}").is_err());
        let no_price = json!({"data": {"endpoints": [{"provider_name": "x", "context_length": 1, "pricing": {}}]}});
        assert!(parse_endpoints(&no_price.to_string()).is_err());
    }

    #[test]
    fn model_ids_with_odd_characters_never_reach_a_url() {
        assert!(valid_model_id("qwen/qwen3.5-9b"));
        assert!(valid_model_id("meta/muse-spark-1.2:free"));
        for bad in ["", "a b", "a?b", "a#b", "../x", "a\nb", "a%2Fb"] {
            assert!(!valid_model_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn model_ids_cannot_climb_out_of_the_models_path() {
        for bad in [
            "..",
            ".",
            "../x",
            "x/..",
            "a/../b",
            "a/./b",
            "./x",
            "/x",
            "x/",
            "a//b",
            "/",
            "../../etc",
        ] {
            assert!(!valid_model_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn real_model_ids_are_accepted() {
        for good in [
            "qwen/qwen3.5-9b",
            "anthropic/claude-sonnet-4.5",
            "meta-llama/llama-3.1-8b-instruct:free",
            "meta/muse-spark-1.2:free",
            "vendor/name.with.dots",
            "vendor/sub/name",
            "x",
        ] {
            assert!(valid_model_id(good), "{good:?}");
        }
    }

    /// Serve one HTTP response on loopback, and hand back the request it received.
    fn serve_once(
        status: &str,
        content_type: &str,
        body: String,
    ) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let (sender, receiver) = mpsc::channel();
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut received = Vec::new();
            let mut buffer = [0u8; 8192];
            // Read until the headers and the whole body have arrived.
            while let Ok(count) = stream.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                received.extend_from_slice(&buffer[..count]);
                let text = String::from_utf8_lossy(&received);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if received.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&received).into_owned());
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        });
        (format!("http://{address}"), receiver)
    }

    #[test]
    fn the_backend_posts_with_the_key_in_a_header_only_and_reads_the_stream() {
        let (url, received) = serve_once(
            "200 OK",
            "text/event-stream",
            stream_text(&reply_json(), "stop"),
        );
        let backend = OpenRouterBackend::with_base_url(KEY.to_string(), url);
        let reply = backend.analyze(&request()).expect("reply");
        assert_eq!(reply.provider, "DeepInfra");
        assert_eq!(reply.input_tokens, 1234);
        let sent = received
            .recv_timeout(Duration::from_secs(5))
            .expect("request");
        let (head, body) = sent.split_once("\r\n\r\n").expect("split");
        assert!(head.starts_with("POST /chat/completions"), "{head}");
        assert!(
            head.to_lowercase()
                .contains(&format!("authorization: bearer {}", KEY.to_lowercase())),
            "{head}"
        );
        assert!(!body.contains(KEY), "the key is only a header");
        let sent_body: Value = serde_json::from_str(body).expect("the body is JSON");
        assert_eq!(sent_body["provider"]["data_collection"], "deny");
        assert_eq!(sent_body["provider"]["require_parameters"], true);
    }

    #[test]
    fn the_backend_reports_an_http_error_without_the_key() {
        let (url, _) = serve_once(
            "401 Unauthorized",
            "application/json",
            format!("{{\"error\": \"bad key {KEY}\"}}"),
        );
        let backend = OpenRouterBackend::with_base_url(KEY.to_string(), url);
        let err = backend.analyze(&request()).expect_err("error");
        let text = err.to_string();
        assert!(
            matches!(err, BackendError::Status { status: 401, .. }),
            "{text}"
        );
        assert!(!text.contains(KEY), "{text}");
    }

    #[test]
    fn scrub_error_cleans_every_text_of_every_variant_and_cuts_after_cleaning() {
        let dirty = format!("a {KEY} b");
        for error in [
            BackendError::NoProvider {
                model: dirty.clone(),
                reason: dirty.clone(),
            },
            BackendError::Http(dirty.clone()),
            BackendError::Status {
                status: 500,
                body: dirty.clone(),
            },
            BackendError::BadReply(dirty.clone()),
        ] {
            let clean = scrub_error(error, KEY);
            assert_no_key(&clean);
            assert!(clean.to_string().contains("a [key] b"), "{clean}");
        }
        // The key straddles the cut: cleaning first leaves no piece of it.
        let long = format!("{}{KEY}", "x".repeat(ERROR_TEXT_LIMIT - 5));
        let cut = scrub_error(BackendError::Http(long), KEY);
        let shown = cut.to_string();
        assert!(!shown.contains("sk-or"), "{shown}");
        assert!(shown.chars().count() < ERROR_TEXT_LIMIT + 100, "{shown}");
    }

    #[test]
    fn a_no_provider_answer_stays_a_no_provider_error_and_is_scrubbed() {
        let err = scrub_error(
            status_error(404, &format!("No endpoints found for {KEY}"), "m"),
            KEY,
        );
        assert!(matches!(err, BackendError::NoProvider { .. }), "{err}");
        assert_no_key(&err);
    }

    /// Assert that neither the Display nor the Debug of `err` shows the key.
    fn assert_no_key(err: &BackendError) {
        let shown = err.to_string();
        let debug = format!("{err:?}");
        assert!(!shown.contains(KEY), "Display: {shown}");
        assert!(!debug.contains(KEY), "Debug: {debug}");
    }

    #[test]
    fn a_key_echoed_in_a_stream_error_event_is_not_shown() {
        let (url, _) = serve_once(
            "200 OK",
            "text/event-stream",
            format!("data: {{\"error\": {{\"message\": \"rejected {KEY}\", \"code\": 401}}}}\n\n"),
        );
        let backend = OpenRouterBackend::with_base_url(KEY.to_string(), url);
        let err = backend.analyze(&request()).expect_err("error");
        assert!(matches!(err, BackendError::BadReply(_)), "{err}");
        assert!(err.to_string().contains("rejected"), "{err}");
        assert_no_key(&err);
    }

    #[test]
    fn a_key_echoed_in_a_malformed_reply_is_not_shown() {
        // serde's error for a wrong type quotes the offending value.
        let content = json!({"video_summary": KEY}).to_string();
        let (url, _) = serve_once("200 OK", "text/event-stream", stream_text(&content, "stop"));
        let backend = OpenRouterBackend::with_base_url(KEY.to_string(), url);
        let err = backend.analyze(&request()).expect_err("error");
        assert!(matches!(err, BackendError::BadReply(_)), "{err}");
        assert_no_key(&err);
    }

    #[test]
    fn the_catalog_reads_a_listing_over_http_and_maps_404() {
        let (url, received) = serve_once("200 OK", "application/json", listing());
        let catalog = OpenRouterCatalog::with_base_url(url);
        let endpoints = catalog.endpoints("qwen/qwen3.5-9b").expect("endpoints");
        assert_eq!(endpoints.len(), 2);
        let sent = received
            .recv_timeout(Duration::from_secs(5))
            .expect("request");
        assert!(
            sent.starts_with("GET /models/qwen/qwen3.5-9b/endpoints"),
            "{sent}"
        );
        assert!(
            !sent.to_lowercase().contains("authorization"),
            "the listing needs no key"
        );

        let (url, _) = serve_once("404 Not Found", "application/json", "{}".to_string());
        let err = OpenRouterCatalog::with_base_url(url)
            .endpoints("nope/model")
            .expect_err("unknown");
        assert!(matches!(err, CatalogError::UnknownModel(_)), "{err}");
        assert!(OpenRouterCatalog::default().endpoints("bad model").is_err());
    }
}
