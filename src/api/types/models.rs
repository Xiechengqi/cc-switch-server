use crate::domain::providers::model::AppKind;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ModelsQuery {
    #[serde(default)]
    pub(in crate::api) app: Option<AppKind>,
    #[serde(default)]
    pub(in crate::api) provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct OpenAiModelsResponse {
    pub(in crate::api) object: &'static str,
    pub(in crate::api) data: Vec<OpenAiModel>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct OpenAiModel {
    pub(in crate::api) id: String,
    pub(in crate::api) object: &'static str,
    pub(in crate::api) owned_by: String,
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
