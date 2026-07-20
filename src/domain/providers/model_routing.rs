use serde_json::{json, Map, Value};
use thiserror::Error;

use super::model::{classify_provider, AppKind, Provider, ProviderType};

pub const DEFAULT_GROK_MODEL: &str = "grok-4.5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelRoutingMode {
    Passthrough,
    Single,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRoutingPolicy {
    pub mode: ModelRoutingMode,
    pub upstream_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelRoutingNormalization {
    pub changed: bool,
    pub required: bool,
    pub resolved: bool,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error(
    "non-native {app} provider '{provider}' requires modelMapping.mode=single and a non-empty upstreamModel"
)]
pub struct ModelRoutingValidationError {
    app: &'static str,
    provider: String,
}

pub fn policy_from_settings(settings: &Value) -> Option<ModelRoutingPolicy> {
    let mapping = settings
        .get("modelMapping")
        .or_else(|| settings.get("model_mapping"))?;
    let mode = mapping.get("mode")?.as_str()?.trim();
    match mode {
        "passthrough" => Some(ModelRoutingPolicy {
            mode: ModelRoutingMode::Passthrough,
            upstream_model: None,
        }),
        "single" => Some(ModelRoutingPolicy {
            mode: ModelRoutingMode::Single,
            upstream_model: mapping_upstream_model(mapping),
        }),
        _ => None,
    }
}

pub fn single_upstream_model(settings: &Value) -> Option<String> {
    let policy = policy_from_settings(settings)?;
    (policy.mode == ModelRoutingMode::Single)
        .then_some(policy.upstream_model)
        .flatten()
}

pub fn is_native_model_provider(app: AppKind, provider: &Provider) -> bool {
    let provider_type = classify_provider(app, provider);
    match (app, provider_type) {
        (AppKind::Claude, ProviderType::ClaudeOAuth) => {
            uses_native_endpoint(provider, app, &["api.anthropic.com"])
        }
        (AppKind::Claude, ProviderType::Claude) => {
            uses_native_endpoint(provider, app, &["api.anthropic.com"])
        }
        (AppKind::Codex, ProviderType::CodexOAuth) => {
            uses_native_endpoint(provider, app, &["chatgpt.com", "api.openai.com"])
        }
        (AppKind::Codex, ProviderType::Codex) => {
            uses_native_endpoint(provider, app, &["api.openai.com"])
        }
        _ => false,
    }
}

fn uses_native_endpoint(provider: &Provider, app: AppKind, native_hosts: &[&str]) -> bool {
    let Some(endpoint) = configured_base_url(provider, app) else {
        return true;
    };
    endpoint_host(endpoint).is_some_and(|host| native_hosts.contains(&host.as_str()))
}

pub fn normalize_provider_model_routing(
    app: AppKind,
    provider: &mut Provider,
) -> ModelRoutingNormalization {
    if !matches!(app, AppKind::Claude | AppKind::Codex) {
        return ModelRoutingNormalization {
            changed: false,
            required: false,
            resolved: true,
        };
    }

    let before = provider.settings_config.clone();
    if is_native_model_provider(app, provider) {
        set_model_mapping(provider, json!({"mode": "passthrough"}));
        return ModelRoutingNormalization {
            changed: provider.settings_config != before,
            required: true,
            resolved: true,
        };
    }

    let provider_type = classify_provider(app, provider);
    let explicit_model = provider
        .settings_config
        .get("modelMapping")
        .or_else(|| provider.settings_config.get("model_mapping"))
        .and_then(mapping_upstream_model);
    let upstream_model = explicit_model.or_else(|| {
        if provider_type == ProviderType::GrokOAuth {
            Some(DEFAULT_GROK_MODEL.to_string())
        } else {
            infer_single_upstream_model(app, &provider.settings_config)
        }
    });

    let resolved = upstream_model.is_some();
    if let Some(upstream_model) = upstream_model {
        set_model_mapping(
            provider,
            json!({
                "mode": "single",
                "upstreamModel": upstream_model,
            }),
        );
    }

    ModelRoutingNormalization {
        changed: provider.settings_config != before,
        required: true,
        resolved,
    }
}

pub fn normalize_and_validate_provider_model_routing(
    app: AppKind,
    provider: &mut Provider,
) -> Result<ModelRoutingNormalization, ModelRoutingValidationError> {
    let normalization = normalize_provider_model_routing(app, provider);
    if normalization.required && !normalization.resolved {
        return Err(ModelRoutingValidationError {
            app: app.as_str(),
            provider: provider.name.trim().to_string(),
        });
    }
    Ok(normalization)
}

fn set_model_mapping(provider: &mut Provider, mapping: Value) {
    if !provider.settings_config.is_object() {
        provider.settings_config = Value::Object(Map::new());
    }
    let settings = provider
        .settings_config
        .as_object_mut()
        .expect("settings_config was normalized to an object");
    settings.remove("model_mapping");
    settings.insert("modelMapping".to_string(), mapping);
}

fn mapping_upstream_model(mapping: &Value) -> Option<String> {
    string_field(mapping, &["upstreamModel", "upstream_model", "model"])
}

fn infer_single_upstream_model(app: AppKind, settings: &Value) -> Option<String> {
    let claude_model = || {
        [
            "/env/ANTHROPIC_MODEL",
            "/env/ANTHROPIC_DEFAULT_SONNET_MODEL",
            "/env/ANTHROPIC_DEFAULT_OPUS_MODEL",
            "/env/ANTHROPIC_DEFAULT_FABLE_MODEL",
            "/env/ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ]
        .into_iter()
        .find_map(|pointer| pointer_string(settings, pointer))
    };
    let codex_model = || {
        pointer_string(settings, "/model")
            .or_else(|| pointer_string(settings, "/config/model"))
            .or_else(|| pointer_string(settings, "/env/OPENAI_MODEL"))
            .or_else(|| pointer_string(settings, "/env/CODEX_MODEL"))
            .or_else(|| {
                settings
                    .get("config")
                    .and_then(Value::as_str)
                    .and_then(extract_codex_toml_model)
            })
    };

    let primary = match app {
        AppKind::Claude => claude_model().or_else(codex_model),
        AppKind::Codex => codex_model().or_else(claude_model),
        AppKind::Gemini => None,
    };
    primary.or_else(|| first_catalog_model(settings))
}

fn first_catalog_model(settings: &Value) -> Option<String> {
    if let Some(models) = settings
        .pointer("/modelCatalog/models")
        .and_then(Value::as_array)
    {
        if let Some(model) = models.iter().find_map(model_value) {
            return Some(model);
        }
    }
    if let Some(catalog) = settings.get("modelCatalog") {
        if let Some(models) = catalog.as_array() {
            if let Some(model) = models.iter().find_map(model_value) {
                return Some(model);
            }
        }
        if let Some(models) = catalog.as_object() {
            for (key, value) in models {
                if key == "models" {
                    continue;
                }
                if let Some(model) = model_value(value).or_else(|| non_empty(key)) {
                    return Some(model);
                }
            }
        }
    }
    settings
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.iter().find_map(model_value))
}

fn model_value(value: &Value) -> Option<String> {
    value.as_str().and_then(non_empty).or_else(|| {
        string_field(
            value,
            &["upstreamModel", "upstream_model", "model", "id", "name"],
        )
    })
}

fn extract_codex_toml_model(config: &str) -> Option<String> {
    for line in config.lines() {
        let line = line.split('#').next().unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "model" {
            continue;
        }
        if let Some(model) = non_empty(value.trim().trim_matches('"').trim_matches('\'')) {
            return Some(model);
        }
    }
    None
}

fn configured_base_url(provider: &Provider, app: AppKind) -> Option<&str> {
    let keys: &[&str] = match app {
        AppKind::Claude => &["ANTHROPIC_BASE_URL", "BASE_URL"],
        AppKind::Codex => &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
        AppKind::Gemini => &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
    };
    for key in keys {
        if let Some(value) = provider
            .settings_config
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
        if let Some(value) = provider
            .settings_config
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
    }
    if app == AppKind::Codex {
        if let Some(value) = provider
            .settings_config
            .pointer("/config/base_url")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(value);
        }
        return provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .and_then(extract_codex_toml_base_url);
    }
    None
}

fn extract_codex_toml_base_url(config: &str) -> Option<&str> {
    for line in config.lines() {
        let line = line.split('#').next().unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "base_url" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn endpoint_host(endpoint: &str) -> Option<String> {
    reqwest::Url::parse(endpoint)
        .ok()?
        .host_str()
        .map(|host| host.to_ascii_lowercase())
}

fn pointer_string(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(non_empty)
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).and_then(non_empty))
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::ProviderMeta;

    fn provider(provider_type: Option<&str>, settings_config: Value) -> Provider {
        Provider {
            id: "provider-1".to_string(),
            name: "Provider".to_string(),
            settings_config,
            category: Some("official".to_string()),
            meta: provider_type.map(|provider_type| ProviderMeta {
                provider_type: Some(provider_type.to_string()),
                ..ProviderMeta::default()
            }),
            extra: Default::default(),
        }
    }

    #[test]
    fn native_ownership_uses_identity_and_endpoint_not_category() {
        let anthropic = provider(
            Some("claude"),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.anthropic.com/v1"}}),
        );
        let relay = provider(
            Some("claude"),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://relay.example.com"}}),
        );
        let grok = provider(Some("grok_oauth"), json!({}));

        assert!(is_native_model_provider(AppKind::Claude, &anthropic));
        assert!(!is_native_model_provider(AppKind::Claude, &relay));
        assert!(!is_native_model_provider(AppKind::Codex, &grok));
    }

    #[test]
    fn configured_invalid_endpoint_is_not_treated_as_native() {
        let invalid = provider(
            Some("claude"),
            json!({"env": {"ANTHROPIC_BASE_URL": "relay.example.com"}}),
        );
        let implicit_official = provider(Some("claude"), json!({}));

        assert!(!is_native_model_provider(AppKind::Claude, &invalid));
        assert!(is_native_model_provider(
            AppKind::Claude,
            &implicit_official
        ));
    }

    #[test]
    fn native_providers_normalize_to_passthrough() {
        let mut provider = provider(
            Some("codex_oauth"),
            json!({"modelMapping": {"mode": "single", "upstreamModel": "old"}}),
        );
        let result = normalize_provider_model_routing(AppKind::Codex, &mut provider);

        assert!(result.changed);
        assert!(result.resolved);
        assert_eq!(
            provider.settings_config["modelMapping"],
            json!({"mode": "passthrough"})
        );
    }

    #[test]
    fn grok_without_explicit_mapping_migrates_to_default_model() {
        let mut provider = provider(
            Some("grok_oauth"),
            json!({"config": "model = \"grok-4.3\""}),
        );
        normalize_and_validate_provider_model_routing(AppKind::Codex, &mut provider).unwrap();

        assert_eq!(
            provider.settings_config["modelMapping"],
            json!({"mode": "single", "upstreamModel": "grok-4.5"})
        );
    }

    #[test]
    fn explicit_single_model_is_preserved() {
        let mut provider = provider(
            Some("grok_oauth"),
            json!({"modelMapping": {"upstreamModel": "grok-custom"}}),
        );
        normalize_and_validate_provider_model_routing(AppKind::Codex, &mut provider).unwrap();

        assert_eq!(
            provider.settings_config["modelMapping"],
            json!({"mode": "single", "upstreamModel": "grok-custom"})
        );
    }

    #[test]
    fn infers_codex_model_without_using_catalog_as_an_implicit_override() {
        let mut provider = provider(
            Some("openrouter"),
            json!({
                "config": "model = \"openrouter/actual\"\n",
                "modelCatalog": {"models": [{"model": "client-alias"}]}
            }),
        );
        normalize_and_validate_provider_model_routing(AppKind::Codex, &mut provider).unwrap();

        assert_eq!(
            single_upstream_model(&provider.settings_config).as_deref(),
            Some("openrouter/actual")
        );
    }

    #[test]
    fn unresolved_non_native_provider_is_rejected_for_new_writes() {
        let mut provider = provider(
            Some("openrouter"),
            json!({"env": {"ANTHROPIC_BASE_URL": "https://openrouter.ai/api"}}),
        );
        let error = normalize_and_validate_provider_model_routing(AppKind::Claude, &mut provider)
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("requires modelMapping.mode=single"));
    }
}
