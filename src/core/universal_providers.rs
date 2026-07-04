use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::provider::{AppKind, Provider, ProviderMeta};

const UNIVERSAL_PROVIDERS_FILE_NAME: &str = "universal-providers.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalProviderStore {
    #[serde(default)]
    pub providers: BTreeMap<String, UniversalProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalProvider {
    pub id: String,
    pub name: String,
    pub provider_type: String,
    pub apps: UniversalProviderApps,
    pub base_url: String,
    pub api_key: String,
    #[serde(default)]
    pub models: UniversalProviderModels,
    #[serde(default)]
    pub website_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub icon_color: Option<String>,
    #[serde(default)]
    pub meta: Option<ProviderMeta>,
    #[serde(default)]
    pub created_at: Option<i64>,
    #[serde(default)]
    pub sort_index: Option<i64>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalProviderApps {
    #[serde(default)]
    pub claude: bool,
    #[serde(default)]
    pub codex: bool,
    #[serde(default)]
    pub gemini: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalProviderModels {
    #[serde(default)]
    pub claude: Option<ClaudeModelConfig>,
    #[serde(default)]
    pub codex: Option<CodexModelConfig>,
    #[serde(default)]
    pub gemini: Option<GeminiModelConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeModelConfig {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub haiku_model: Option<String>,
    #[serde(default)]
    pub sonnet_model: Option<String>,
    #[serde(default)]
    pub opus_model: Option<String>,
    #[serde(default)]
    pub model_catalog: Option<Value>,
    #[serde(default)]
    pub model_mapping: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexModelConfig {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub chat_reasoning: Option<Value>,
    #[serde(default)]
    pub model_catalog: Option<Value>,
    #[serde(default)]
    pub model_mapping: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeminiModelConfig {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub model_catalog: Option<Value>,
    #[serde(default)]
    pub model_mapping: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalProviderPreset {
    pub name: String,
    pub provider_type: String,
    pub default_apps: UniversalProviderApps,
    pub default_models: UniversalProviderModels,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub website_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub is_custom_template: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UniversalProviderSyncResult {
    pub synced: Vec<String>,
    pub skipped: Vec<String>,
    pub removed: Vec<String>,
}

impl UniversalProviderStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = universal_providers_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content =
            fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse {}", path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        let path = universal_providers_path(config_dir);
        crate::core::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write {}", path.display()))
    }

    pub fn upsert(&mut self, provider: UniversalProvider) -> UniversalProvider {
        self.providers.insert(provider.id.clone(), provider.clone());
        provider
    }

    pub fn delete(&mut self, id: &str) -> bool {
        self.providers.remove(id).is_some()
    }
}

pub fn universal_providers_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(UNIVERSAL_PROVIDERS_FILE_NAME)
}

pub fn universal_provider_presets() -> Vec<UniversalProviderPreset> {
    let default_apps = UniversalProviderApps {
        claude: true,
        codex: true,
        gemini: true,
    };
    let default_models = UniversalProviderModels {
        claude: Some(ClaudeModelConfig {
            model: Some("claude-sonnet-5".to_string()),
            haiku_model: Some("claude-haiku-4-5-20251001".to_string()),
            sonnet_model: Some("claude-sonnet-5".to_string()),
            opus_model: Some("claude-opus-4-8".to_string()),
            ..ClaudeModelConfig::default()
        }),
        codex: Some(CodexModelConfig {
            model: Some("gpt-5.5".to_string()),
            reasoning_effort: Some("high".to_string()),
            ..CodexModelConfig::default()
        }),
        gemini: Some(GeminiModelConfig {
            model: Some("gemini-3.5-flash".to_string()),
            ..GeminiModelConfig::default()
        }),
    };

    vec![
        UniversalProviderPreset {
            name: "NewAPI".to_string(),
            provider_type: "newapi".to_string(),
            default_apps: default_apps.clone(),
            default_models: default_models.clone(),
            website_url: Some("https://www.newapi.pro".to_string()),
            icon: Some("newapi".to_string()),
            icon_color: Some("#00A67E".to_string()),
            description: Some(
                "NewAPI 是一个可自部署的 API 网关，支持 Anthropic、OpenAI、Gemini 等多种协议"
                    .to_string(),
            ),
            is_custom_template: false,
        },
        UniversalProviderPreset {
            name: "自定义网关".to_string(),
            provider_type: "custom_gateway".to_string(),
            default_apps,
            default_models,
            website_url: None,
            icon: Some("openai".to_string()),
            icon_color: Some("#6366F1".to_string()),
            description: Some("自定义配置的 API 网关".to_string()),
            is_custom_template: true,
        },
    ]
}

pub fn provider_from_universal(universal: &UniversalProvider, app: AppKind) -> Option<Provider> {
    if !universal_app_enabled(universal, app) {
        return None;
    }

    let mut env = serde_json::Map::new();
    let mut settings = serde_json::Map::new();
    match app {
        AppKind::Claude => {
            let models = universal.models.claude.as_ref();
            let model = model_or(
                models.and_then(|item| item.model.as_deref()),
                "claude-sonnet-4-20250514",
            );
            let haiku = model_or(models.and_then(|item| item.haiku_model.as_deref()), &model);
            let sonnet = model_or(models.and_then(|item| item.sonnet_model.as_deref()), &model);
            let opus = model_or(models.and_then(|item| item.opus_model.as_deref()), &model);
            env.insert(
                "ANTHROPIC_BASE_URL".to_string(),
                Value::String(universal.base_url.clone()),
            );
            env.insert(
                "ANTHROPIC_AUTH_TOKEN".to_string(),
                Value::String(universal.api_key.clone()),
            );
            env.insert("ANTHROPIC_MODEL".to_string(), Value::String(model));
            env.insert(
                "ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(),
                Value::String(haiku),
            );
            env.insert(
                "ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(),
                Value::String(sonnet),
            );
            env.insert(
                "ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(),
                Value::String(opus),
            );
            insert_optional_json(
                &mut settings,
                "modelCatalog",
                models.and_then(|item| item.model_catalog.clone()),
            );
            insert_optional_json(
                &mut settings,
                "modelMapping",
                models.and_then(|item| item.model_mapping.clone()),
            );
        }
        AppKind::Codex => {
            let models = universal.models.codex.as_ref();
            let model = model_or(models.and_then(|item| item.model.as_deref()), "gpt-4o");
            let reasoning_effort = model_or(
                models.and_then(|item| item.reasoning_effort.as_deref()),
                "high",
            );
            let codex_base_url = codex_base_url(&universal.base_url);
            env.insert(
                "OPENAI_BASE_URL".to_string(),
                Value::String(codex_base_url.clone()),
            );
            env.insert(
                "OPENAI_API_KEY".to_string(),
                Value::String(universal.api_key.clone()),
            );

            let mut auth = serde_json::Map::new();
            auth.insert(
                "OPENAI_API_KEY".to_string(),
                Value::String(universal.api_key.clone()),
            );
            settings.insert("auth".to_string(), Value::Object(auth));
            settings.insert(
                "config".to_string(),
                Value::String(codex_config_toml(
                    &universal.name,
                    &codex_base_url,
                    &model,
                    &reasoning_effort,
                )),
            );
            settings.insert(
                "models".to_string(),
                Value::Array(vec![Value::String(model)]),
            );
            settings.insert(
                "codex".to_string(),
                serde_json::json!({"reasoningEffort": reasoning_effort}),
            );
            insert_optional_json(
                &mut settings,
                "modelCatalog",
                models.and_then(|item| item.model_catalog.clone()),
            );
            insert_optional_json(
                &mut settings,
                "modelMapping",
                models.and_then(|item| item.model_mapping.clone()),
            );
        }
        AppKind::Gemini => {
            let models = universal.models.gemini.as_ref();
            let model = model_or(
                models.and_then(|item| item.model.as_deref()),
                "gemini-2.5-pro",
            );
            env.insert(
                "GOOGLE_GEMINI_BASE_URL".to_string(),
                Value::String(universal.base_url.clone()),
            );
            env.insert(
                "GEMINI_API_KEY".to_string(),
                Value::String(universal.api_key.clone()),
            );
            env.insert("GEMINI_MODEL".to_string(), Value::String(model));
            insert_optional_json(
                &mut settings,
                "modelCatalog",
                models.and_then(|item| item.model_catalog.clone()),
            );
            insert_optional_json(
                &mut settings,
                "modelMapping",
                models.and_then(|item| item.model_mapping.clone()),
            );
        }
    }

    settings.insert("env".to_string(), Value::Object(env));

    let mut meta = universal.meta.clone().unwrap_or_default();
    meta.provider_type = Some(universal.provider_type.clone());
    if app == AppKind::Codex && meta.codex_chat_reasoning.is_none() {
        meta.codex_chat_reasoning = universal
            .models
            .codex
            .as_ref()
            .and_then(|item| item.chat_reasoning.clone());
    }
    meta.extra.insert(
        "universalProviderId".to_string(),
        Value::String(universal.id.clone()),
    );

    let mut extra = BTreeMap::new();
    extra.insert(
        "universalProviderId".to_string(),
        Value::String(universal.id.clone()),
    );
    insert_extra_string(&mut extra, "websiteUrl", universal.website_url.as_deref());
    insert_extra_string(&mut extra, "notes", universal.notes.as_deref());
    insert_extra_string(&mut extra, "icon", universal.icon.as_deref());
    insert_extra_string(&mut extra, "iconColor", universal.icon_color.as_deref());
    insert_extra_i64(&mut extra, "createdAt", universal.created_at);
    insert_extra_i64(&mut extra, "sortIndex", universal.sort_index);

    Some(Provider {
        id: universal_provider_id(&universal.id, app),
        name: universal.name.clone(),
        settings_config: Value::Object(settings),
        category: Some("universal".to_string()),
        meta: Some(meta),
        extra,
    })
}

pub fn universal_provider_id(id: &str, app: AppKind) -> String {
    format!("universal:{id}:{}", app.as_str())
}

fn universal_app_enabled(universal: &UniversalProvider, app: AppKind) -> bool {
    match app {
        AppKind::Claude => universal.apps.claude,
        AppKind::Codex => universal.apps.codex,
        AppKind::Gemini => universal.apps.gemini,
    }
}

fn model_or(value: Option<&str>, fallback: &str) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn codex_base_url(base_url: &str) -> String {
    let base_trimmed = base_url.trim_end_matches('/');
    let origin_only = match base_trimmed.split_once("://") {
        Some((_scheme, rest)) => !rest.contains('/'),
        None => !base_trimmed.contains('/'),
    };

    if base_trimmed.ends_with("/v1") {
        base_trimmed.to_string()
    } else if origin_only {
        format!("{base_trimmed}/v1")
    } else {
        base_trimmed.to_string()
    }
}

fn codex_config_toml(name: &str, base_url: &str, model: &str, reasoning_effort: &str) -> String {
    format!(
        "model_provider = \"custom\"\nmodel = \"{}\"\nmodel_reasoning_effort = \"{}\"\ndisable_response_storage = true\n\n[model_providers.custom]\nname = \"{}\"\nbase_url = \"{}\"\nwire_api = \"responses\"\nrequires_openai_auth = true",
        toml_string(model),
        toml_string(reasoning_effort),
        toml_string(name),
        toml_string(base_url)
    )
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn insert_extra_string(map: &mut BTreeMap<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn insert_extra_i64(map: &mut BTreeMap<String, Value>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        map.insert(key.to_string(), Value::Number(value.into()));
    }
}

fn insert_optional_json(map: &mut serde_json::Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value.filter(|value| !value.is_null()) {
        map.insert(key.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universal_presets_match_desktop_defaults() {
        let presets = universal_provider_presets();
        assert_eq!(presets.len(), 2);
        assert!(presets.iter().any(|preset| preset.name == "自定义网关"));

        let newapi = presets
            .iter()
            .find(|preset| preset.provider_type == "newapi")
            .expect("NewAPI preset");
        assert!(newapi.default_apps.claude);
        assert!(newapi.default_apps.codex);
        assert!(newapi.default_apps.gemini);
        assert_eq!(
            newapi
                .default_models
                .codex
                .as_ref()
                .and_then(|config| config.model.as_deref()),
            Some("gpt-5.5")
        );
        assert_eq!(
            newapi
                .default_models
                .gemini
                .as_ref()
                .and_then(|config| config.model.as_deref()),
            Some("gemini-3.5-flash")
        );
    }
}
