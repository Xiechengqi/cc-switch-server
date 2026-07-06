use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::core::provider::{AppKind, Provider};
use crate::core::providers::ProviderStore;

const OFFICIAL_SEED_IDS: &[&str] = &[
    "claude-official",
    "codex-official",
    "gemini-official",
    "claude-desktop-official",
];

#[derive(Debug, thiserror::Error)]
pub enum LiveConfigImportError {
    #[error("{0}")]
    Message(String),
}

pub fn current_provider_settings_key(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "currentProviderClaude",
        AppKind::Codex => "currentProviderCodex",
        AppKind::Gemini => "currentProviderGemini",
    }
}

pub fn read_current_provider_id(ui_settings: &Value, app: AppKind) -> Option<String> {
    ui_settings
        .get(current_provider_settings_key(app))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn has_non_official_seed_provider(store: &ProviderStore, app: AppKind) -> bool {
    store
        .list(Some(app))
        .into_iter()
        .any(|provider| !is_official_seed_id(&provider.provider.id))
}

pub fn import_default_config(
    store: &mut ProviderStore,
    app: AppKind,
    ui_settings: &Value,
) -> Result<bool, LiveConfigImportError> {
    if has_non_official_seed_provider(store, app) {
        return Ok(false);
    }

    let settings_config = read_live_settings_config(app, ui_settings)?;
    let category = infer_import_category(app, &settings_config);

    let provider = Provider {
        id: "default".to_string(),
        name: "default".to_string(),
        settings_config,
        category: Some(category),
        meta: None,
        extra: Default::default(),
    };

    store.upsert(app, provider);
    Ok(true)
}

fn is_official_seed_id(id: &str) -> bool {
    OFFICIAL_SEED_IDS.contains(&id)
}

fn infer_import_category(app: AppKind, settings_config: &Value) -> String {
    if app == AppKind::Codex {
        let config_text = settings_config
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let has_provider_key =
            extract_codex_api_key(settings_config.get("auth"), config_text).is_some();
        let has_login_material = settings_config
            .get("auth")
            .is_some_and(codex_auth_has_login_material);
        if has_login_material && !has_provider_key {
            return "official".to_string();
        }
    }
    "custom".to_string()
}

fn codex_auth_has_login_material(auth: &Value) -> bool {
    auth.is_object() && !auth.as_object().is_some_and(|map| map.is_empty())
}

fn extract_codex_api_key(auth: Option<&Value>, config_text: &str) -> Option<String> {
    if let Some(token) = auth
        .and_then(|value| value.get("OPENAI_API_KEY"))
        .or_else(|| auth.and_then(|value| value.get("openai_api_key")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(token.to_string());
    }

    for line in config_text.lines() {
        let line = line.trim();
        if line.starts_with("api_key") {
            if let Some((_, value)) = line.split_once('=') {
                let value = value.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn read_live_settings_config(
    app: AppKind,
    ui_settings: &Value,
) -> Result<Value, LiveConfigImportError> {
    match app {
        AppKind::Claude => read_claude_live_settings(ui_settings),
        AppKind::Codex => read_codex_live_settings(ui_settings),
        AppKind::Gemini => read_gemini_live_settings(ui_settings),
    }
}

fn read_claude_live_settings(ui_settings: &Value) -> Result<Value, LiveConfigImportError> {
    let settings_path = claude_settings_path(ui_settings);
    if !settings_path.exists() {
        return Err(LiveConfigImportError::Message(
            "Claude Code 配置文件不存在".to_string(),
        ));
    }
    let content = fs::read_to_string(&settings_path).map_err(|error| {
        LiveConfigImportError::Message(format!(
            "read Claude settings {}: {error}",
            settings_path.display()
        ))
    })?;
    serde_json::from_str(&content).map_err(|error| {
        LiveConfigImportError::Message(format!(
            "parse Claude settings {}: {error}",
            settings_path.display()
        ))
    })
}

fn read_codex_live_settings(ui_settings: &Value) -> Result<Value, LiveConfigImportError> {
    let config_dir = codex_config_dir(ui_settings);
    let auth_path = config_dir.join("auth.json");
    let config_path = config_dir.join("config.toml");

    let auth_present = auth_path.exists();
    let auth: Value = if auth_present {
        read_json_file(&auth_path)?
    } else {
        json!({})
    };

    let config_text = if config_path.exists() {
        fs::read_to_string(&config_path).map_err(|error| {
            LiveConfigImportError::Message(format!(
                "read Codex config {}: {error}",
                config_path.display()
            ))
        })?
    } else {
        String::new()
    };

    if !auth_present && config_text.trim().is_empty() {
        return Err(LiveConfigImportError::Message(
            "Codex 配置文件不存在".to_string(),
        ));
    }

    Ok(json!({ "auth": auth, "config": config_text }))
}

fn read_gemini_live_settings(ui_settings: &Value) -> Result<Value, LiveConfigImportError> {
    let config_dir = gemini_config_dir(ui_settings);
    let env_path = config_dir.join(".env");
    if !env_path.exists() {
        return Err(LiveConfigImportError::Message(
            "Gemini 配置文件不存在".to_string(),
        ));
    }

    let env_map = parse_env_file(&fs::read_to_string(&env_path).map_err(|error| {
        LiveConfigImportError::Message(format!("read Gemini env {}: {error}", env_path.display()))
    })?);
    let env_obj = env_map
        .into_iter()
        .map(|(key, value)| (key, Value::String(value)))
        .collect::<serde_json::Map<_, _>>();

    let settings_path = config_dir.join("settings.json");
    let config_obj = if settings_path.exists() {
        read_json_file(&settings_path)?
    } else {
        json!({})
    };

    Ok(json!({
        "env": env_obj,
        "config": config_obj
    }))
}

fn read_json_file(path: &Path) -> Result<Value, LiveConfigImportError> {
    let content = fs::read_to_string(path).map_err(|error| {
        LiveConfigImportError::Message(format!("read {}: {error}", path.display()))
    })?;
    serde_json::from_str(&content).map_err(|error| {
        LiveConfigImportError::Message(format!("parse {}: {error}", path.display()))
    })
}

fn parse_env_file(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            if !key.is_empty() && key.chars().all(|ch| ch.is_alphanumeric() || ch == '_') {
                map.insert(key.to_string(), value.to_string());
            }
        }
    }
    map
}

fn claude_settings_path(ui_settings: &Value) -> PathBuf {
    let dir = claude_config_dir(ui_settings);
    let settings = dir.join("settings.json");
    if settings.exists() {
        return settings;
    }
    let legacy = dir.join("claude.json");
    if legacy.exists() {
        return legacy;
    }
    settings
}

fn claude_config_dir(ui_settings: &Value) -> PathBuf {
    config_dir_override(ui_settings, "claudeConfigDir")
        .unwrap_or_else(|| home_dir().join(".claude"))
}

fn codex_config_dir(ui_settings: &Value) -> PathBuf {
    config_dir_override(ui_settings, "codexConfigDir").unwrap_or_else(|| home_dir().join(".codex"))
}

fn gemini_config_dir(ui_settings: &Value) -> PathBuf {
    config_dir_override(ui_settings, "geminiConfigDir")
        .unwrap_or_else(|| home_dir().join(".gemini"))
}

fn config_dir_override(ui_settings: &Value, key: &str) -> Option<PathBuf> {
    ui_settings
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(home) = std::env::var("USERPROFILE") {
        let trimmed = home.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    PathBuf::from(".")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::providers::ProviderStore;

    #[test]
    fn skips_import_when_non_seed_provider_exists() {
        let mut store = ProviderStore::default();
        store.upsert(
            AppKind::Claude,
            Provider {
                id: "custom-1".to_string(),
                name: "custom".to_string(),
                settings_config: json!({}),
                category: Some("custom".to_string()),
                meta: None,
                extra: Default::default(),
            },
        );

        let imported =
            import_default_config(&mut store, AppKind::Claude, &json!({})).expect("import");
        assert!(!imported);
    }

    #[test]
    fn imports_claude_live_settings_from_override_dir() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-live-import-claude-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("settings.json"),
            r#"{"env":{"ANTHROPIC_API_KEY":"test-key"}}"#,
        )
        .unwrap();

        let mut store = ProviderStore::default();
        let ui_settings = json!({ "claudeConfigDir": dir.to_string_lossy() });
        let imported = import_default_config(&mut store, AppKind::Claude, &ui_settings).unwrap();
        assert!(imported);

        let providers = store.list(Some(AppKind::Claude));
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].provider.id, "default");
        assert_eq!(
            providers[0].provider.settings_config["env"]["ANTHROPIC_API_KEY"],
            "test-key"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_codex_live_settings_from_config_toml() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-live-import-codex-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), "model = \"gpt-5.5\"\n").unwrap();

        let mut store = ProviderStore::default();
        let ui_settings = json!({ "codexConfigDir": dir.to_string_lossy() });
        let imported = import_default_config(&mut store, AppKind::Codex, &ui_settings).unwrap();
        assert!(imported);

        let providers = store.list(Some(AppKind::Codex));
        let config = providers[0]
            .provider
            .settings_config
            .get("config")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(config.contains("model = \"gpt-5.5\""));

        let _ = fs::remove_dir_all(&dir);
    }
}
