//! What a model's endpoints offer: context length, prices, quantization.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::CatalogError;

/// One provider's endpoint for a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EndpointInfo {
    pub provider: String,
    /// `fp8`, `bf16`, `fp16`, `unknown`, ... or `None` when not stated.
    pub quantization: Option<String>,
    pub context_length: u64,
    /// US dollars per input token.
    pub prompt_usd_per_token: f64,
    /// US dollars per output token.
    pub completion_usd_per_token: f64,
}

pub trait ModelCatalog {
    fn endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>, CatalogError>;
}

/// A catalog written out in full, for tests and `--catalog`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FixedCatalog {
    pub models: BTreeMap<String, Vec<EndpointInfo>>,
}

impl ModelCatalog for FixedCatalog {
    fn endpoints(&self, model: &str) -> Result<Vec<EndpointInfo>, CatalogError> {
        self.models
            .get(model)
            .cloned()
            .ok_or_else(|| CatalogError::UnknownModel(model.to_string()))
    }
}
