//! The model backend: a request, a reply, and the trait the hosted and recorded
//! backends implement.

use serde::{Deserialize, Serialize};

use crate::error::BackendError;
use crate::types::RawReply;

/// One part of the user message, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    Text(String),
    ImageJpeg(Vec<u8>),
}

/// What is sent to the model. Provider settings that never vary (`require_parameters`,
/// `data_collection: deny`) are the backend's, not part of the request.
#[derive(Debug, Clone, PartialEq)]
pub struct VlmRequest {
    pub model: String,
    pub system: String,
    pub parts: Vec<Part>,
    /// The strict JSON schema the reply must follow.
    pub schema: serde_json::Value,
    pub max_tokens: u32,
    pub effort: Option<String>,
    pub quantizations: Vec<String>,
}

/// The model's parsed answer and what the call used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VlmReply {
    pub reply: RawReply,
    pub provider: String,
    pub quantization: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
    pub elapsed_ms: u64,
}

pub trait VlmBackend {
    fn analyze(&self, request: &VlmRequest) -> Result<VlmReply, BackendError>;
}

/// Returns a recorded reply, for tests and `--replay`. It never touches the network.
#[derive(Debug, Clone)]
pub struct RecordedBackend {
    reply: VlmReply,
}

impl RecordedBackend {
    pub fn new(reply: VlmReply) -> Self {
        Self { reply }
    }
}

impl VlmBackend for RecordedBackend {
    fn analyze(&self, _request: &VlmRequest) -> Result<VlmReply, BackendError> {
        Ok(self.reply.clone())
    }
}
