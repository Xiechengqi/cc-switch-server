use super::types::{GeminiModel, GeminiModelsResponse, OpenAiModel};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::api::error::ApiError;
use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::StoredProvider;
use crate::state::ServerState;

pub(in crate::api) async fn gemini_models_response(
    state: &ServerState,
    headers: &HeaderMap,
    path: &str,
) -> Result<Option<Response>, ApiError> {
    let path = path.trim_matches('/');
    if path != "models" && !path.starts_with("models/") {
        return Ok(None);
    }
    let provider_id = headers
        .get("x-cc-provider-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let providers = state.providers.read().await;
    let models = openai_model_list(&providers.providers, Some(AppKind::Gemini), provider_id)
        .into_iter()
        .map(gemini_model_from_openai)
        .collect::<Vec<_>>();
    if path == "models" {
        return Ok(Some(Json(GeminiModelsResponse { models }).into_response()));
    }
    let requested = path.trim_start_matches("models/").trim();
    let requested_name = gemini_model_name(requested);
    let model = models
        .into_iter()
        .find(|model| model.name == requested_name || model.name == requested)
        .ok_or_else(|| ApiError::not_found("Gemini model not found"))?;
    Ok(Some(Json(model).into_response()))
}

pub(in crate::api) fn gemini_model_from_openai(model: OpenAiModel) -> GeminiModel {
    let id = model.id.trim_start_matches("models/").to_string();
    GeminiModel {
        name: gemini_model_name(&id),
        version: "001".to_string(),
        display_name: id.clone(),
        description: format!("cc-switch provider model {id}"),
        input_token_limit: 1_048_576,
        output_token_limit: 65_536,
        supported_generation_methods: vec![
            "generateContent".to_string(),
            "streamGenerateContent".to_string(),
        ],
    }
}

pub(in crate::api) fn gemini_model_name(model_id: &str) -> String {
    if model_id.starts_with("models/") {
        model_id.to_string()
    } else {
        format!("models/{model_id}")
    }
}

pub(in crate::api) fn openai_model_list(
    providers: &[StoredProvider],
    app: Option<AppKind>,
    provider_id: Option<&str>,
) -> Vec<OpenAiModel> {
    let mut models = BTreeMap::<String, OpenAiModel>::new();
    for provider in providers.iter().filter(|provider| {
        app.is_none_or(|app| provider.app == app)
            && provider_id.is_none_or(|id| provider.provider.id == id)
    }) {
        let owned_by = model_owner(provider);
        for model_id in provider_model_ids(provider) {
            let key = format!("{model_id}\u{0}{owned_by}");
            models.entry(key).or_insert(OpenAiModel {
                id: model_id,
                object: "model",
                owned_by: owned_by.clone(),
            });
        }
    }
    models.into_values().collect()
}

pub(in crate::api) fn model_owner(provider: &StoredProvider) -> String {
    let name = provider.provider.name.trim();
    if name.is_empty() {
        provider.provider.id.clone()
    } else {
        name.to_string()
    }
}

pub(in crate::api) fn provider_model_ids(provider: &StoredProvider) -> Vec<String> {
    let settings = &provider.provider.settings_config;
    let mut models = Vec::new();
    push_model_catalog(
        settings
            .get("modelCatalog")
            .or_else(|| settings.get("model_catalog")),
        &mut models,
    );
    push_models_value(settings.get("models"), &mut models);
    push_model_mapping(
        settings
            .get("modelMapping")
            .or_else(|| settings.get("model_mapping")),
        &mut models,
    );
    for key in [
        "MODEL",
        "OPENAI_MODEL",
        "ANTHROPIC_MODEL",
        "CLAUDE_MODEL",
        "CODEX_MODEL",
        "GEMINI_MODEL",
    ] {
        if let Some(model) = settings_model_string(settings, key) {
            models.push(model);
        }
    }
    dedupe_non_empty(models)
}

pub(in crate::api) fn push_model_catalog(catalog: Option<&Value>, models: &mut Vec<String>) {
    let Some(catalog) = catalog else {
        return;
    };
    push_models_value(catalog.get("models"), models);
}

pub(in crate::api) fn push_models_value(value: Option<&Value>, models: &mut Vec<String>) {
    match value {
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(model) = model_id_from_value(item) {
                    models.push(model);
                }
            }
        }
        Some(value) => {
            if let Some(model) = model_id_from_value(value) {
                models.push(model);
            }
        }
        None => {}
    }
}

pub(in crate::api) fn push_model_mapping(mapping: Option<&Value>, models: &mut Vec<String>) {
    let Some(Value::Object(map)) = mapping else {
        return;
    };
    if let Some(model) = map
        .get("upstreamModel")
        .or_else(|| map.get("upstream_model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        models.push(model.to_string());
    }
    for (key, value) in map {
        if matches!(
            key.as_str(),
            "upstreamModel" | "upstream_model" | "rules" | "modelRules" | "model_rules"
        ) {
            continue;
        }
        if !key.trim().is_empty() {
            models.push(key.trim().to_string());
        }
        if let Some(model) = model_id_from_value(value) {
            models.push(model);
        }
    }
    for rules_key in ["rules", "modelRules", "model_rules"] {
        if let Some(Value::Array(rules)) = map.get(rules_key) {
            for rule in rules {
                if let Some(model) = string_field(
                    rule,
                    &["model", "requestModel", "request_model", "id", "name"],
                ) {
                    models.push(model);
                }
            }
        }
    }
}

pub(in crate::api) fn model_id_from_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| string_field(value, &["id", "model", "name"]))
}

pub(in crate::api) fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(in crate::api) fn settings_model_string(settings: &Value, key: &str) -> Option<String> {
    settings
        .pointer(&format!("/env/{key}"))
        .and_then(Value::as_str)
        .or_else(|| settings.get(key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(in crate::api) fn dedupe_non_empty(values: Vec<String>) -> Vec<String> {
    let mut deduped = BTreeMap::<String, ()>::new();
    for value in values {
        let value = value.trim();
        if !value.is_empty() {
            deduped.entry(value.to_string()).or_insert(());
        }
    }
    deduped.into_keys().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gemini_model_response_wraps_openai_model_id() {
        let model = gemini_model_from_openai(OpenAiModel {
            id: "gemini-2.5-pro".to_string(),
            object: "model",
            owned_by: "gemini".to_string(),
        });

        assert_eq!(model.name, "models/gemini-2.5-pro");
        assert!(model
            .supported_generation_methods
            .contains(&"generateContent".to_string()));
        assert!(model
            .supported_generation_methods
            .contains(&"streamGenerateContent".to_string()));
        assert_eq!(
            gemini_model_name("models/gemini-2.5-pro"),
            "models/gemini-2.5-pro"
        );
    }
}
