use crate::domain::providers::model::AppKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelsQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
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
    pub(in crate::api) input_token_limit: u32,
    pub(in crate::api) output_token_limit: u32,
    pub(in crate::api) supported_generation_methods: Vec<String>,
}
