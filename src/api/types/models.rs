use crate::domain::providers::model::AppKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelsQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) expected_share_id: Option<String>,
    #[serde(default)]
    pub(in crate::api) expected_provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct OpenAiModelsResponse {
    pub(in crate::api) object: &'static str,
    pub(in crate::api) data: Vec<OpenAiModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) stale: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) fetched_at_ms: Option<i64>,
    /// Qoder scene observations, including disabled/discovery-only rows.
    /// The standard data array remains the executable model catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) catalog_entries: Option<Vec<crate::proxy::qoder_runtime::QoderCatalogEntry>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct OpenAiModel {
    pub(in crate::api) id: String,
    pub(in crate::api) object: &'static str,
    pub(in crate::api) owned_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) reasoning_efforts: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) input_modalities: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) supports_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::api) max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct GeminiModelsResponse {
    pub(in crate::api) models: Vec<GeminiModel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct GeminiModel {
    pub(in crate::api) name: String,
    pub(in crate::api) version: String,
    pub(in crate::api) display_name: String,
    pub(in crate::api) description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) input_token_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) output_token_limit: Option<u64>,
    pub(in crate::api) supported_generation_methods: Vec<String>,
}
