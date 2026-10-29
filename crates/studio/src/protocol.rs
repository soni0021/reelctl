//! JSON-RPC 2.0 framing: one JSON object per line. Requests, responses, errors
//! and the notifications a subscribed connection is sent.

use serde_json::{json, Value};

use crate::error::StudioError;

/// The JSON-RPC error codes the service uses.
pub mod code {
    pub const PARSE: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// The request was understood but cannot be done (a missing job, a refused
    /// plan, a changed asset).
    pub const REFUSED: i64 = -32000;
    /// Something on the service's side failed (a file, the catalogue).
    pub const INTERNAL: i64 = -32603;
}

/// One parsed request. `id` is `None` for a notification, which gets no reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: Option<Value>,
    pub method: String,
    pub params: Value,
}

/// A JSON-RPC error.
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    /// `{"exit_code": n}` when a child's exit code is the cause.
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn params(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, message)
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self::new(code::REFUSED, message)
    }

    pub fn with_exit_code(mut self, exit_code: i32) -> Self {
        self.data = Some(json!({ "exit_code": exit_code }));
        self
    }
}

impl From<StudioError> for RpcError {
    fn from(error: StudioError) -> Self {
        match error {
            StudioError::Invalid(message) => Self::refused(message),
            other => Self::new(code::INTERNAL, other.to_string()),
        }
    }
}

/// Parse one line. A line that is not a request is an error to send back with a
/// null id.
pub fn parse_request(line: &str) -> Result<Request, RpcError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|error| RpcError::new(code::PARSE, format!("not JSON: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| RpcError::new(code::INVALID_REQUEST, "a request is a JSON object"))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(RpcError::new(
            code::INVALID_REQUEST,
            "\"jsonrpc\" must be \"2.0\"",
        ));
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(code::INVALID_REQUEST, "\"method\" must be a string"))?;
    let id = match object.get("id") {
        None => None,
        Some(id @ (Value::Number(_) | Value::String(_) | Value::Null)) => Some(id.clone()),
        Some(_) => {
            return Err(RpcError::new(
                code::INVALID_REQUEST,
                "\"id\" must be a number, a string or null",
            ))
        }
    };
    let params = match object.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(params @ Value::Object(_)) => params.clone(),
        Some(_) => return Err(RpcError::params("\"params\" must be an object")),
    };
    Ok(Request {
        id,
        method: method.to_string(),
        params,
    })
}

/// The reply line to a request with `id`.
pub fn response(id: &Value, result: Result<Value, RpcError>) -> String {
    let body = match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => {
            let mut detail = json!({ "code": error.code, "message": error.message });
            if let Some(data) = error.data {
                detail["data"] = data;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": detail })
        }
    };
    body.to_string()
}

/// A notification line (no id): `job.status`, `job.event`, `checkpoint.created`.
pub fn notification(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_round_trips_and_bad_ones_get_their_codes() {
        let request =
            parse_request(r#"{"jsonrpc":"2.0","id":7,"method":"jobs.get","params":{"job":3}}"#)
                .unwrap();
        assert_eq!(request.id, Some(json!(7)));
        assert_eq!(request.method, "jobs.get");
        assert_eq!(request.params, json!({"job": 3}));

        let without = parse_request(r#"{"jsonrpc":"2.0","method":"shutdown"}"#).unwrap();
        assert_eq!(without.id, None);
        assert_eq!(without.params, json!({}));

        assert_eq!(parse_request("{").unwrap_err().code, code::PARSE);
        assert_eq!(
            parse_request(r#"{"method":"x"}"#).unwrap_err().code,
            code::INVALID_REQUEST
        );
        assert_eq!(
            parse_request(r#"{"jsonrpc":"2.0","id":[1],"method":"x"}"#)
                .unwrap_err()
                .code,
            code::INVALID_REQUEST
        );
        assert_eq!(
            parse_request(r#"{"jsonrpc":"2.0","id":1,"method":"x","params":[1]}"#)
                .unwrap_err()
                .code,
            code::INVALID_PARAMS
        );
    }

    #[test]
    fn responses_carry_the_result_or_the_error_with_its_exit_code() {
        let ok: Value = serde_json::from_str(&response(&json!(1), Ok(json!({"a": 1})))).unwrap();
        assert_eq!(ok, json!({"jsonrpc": "2.0", "id": 1, "result": {"a": 1}}));
        let error: Value = serde_json::from_str(&response(
            &json!("x"),
            Err(RpcError::refused("over the bound").with_exit_code(4)),
        ))
        .unwrap();
        assert_eq!(error["error"]["code"], code::REFUSED);
        assert_eq!(error["error"]["data"]["exit_code"], 4);
        let invalid: RpcError = StudioError::invalid("no such job").into();
        assert_eq!(invalid.code, code::REFUSED);
        let note: Value =
            serde_json::from_str(&notification("job.status", json!({"job": 1}))).unwrap();
        assert!(note.get("id").is_none());
    }
}
