use std::fs;
use std::path::Path;

use anyhow::Context;
use serde_json::{json, Value};

const UI_SETTINGS_FILE_NAME: &str = "ui-settings.json";

#[derive(Debug, Clone, Default)]
pub struct UiSettingsStore {
    pub value: Value,
}

impl UiSettingsStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = ui_settings_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("read ui settings {}", path.display()))?;
        let value = serde_json::from_str(&content)
            .with_context(|| format!("parse ui settings {}", path.display()))?;
        Ok(Self { value })
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = ui_settings_path(config_dir);
        crate::core::storage::write_json_pretty(&path, &self.value)
            .with_context(|| format!("write ui settings {}", path.display()))
    }

    pub fn for_frontend(&self) -> Value {
        merge_json_values(default_ui_settings(), self.value.clone())
    }

    pub fn apply_patch(&mut self, patch: Value) {
        self.value = merge_json_values(self.value.clone(), patch);
    }
}

pub fn ui_settings_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(UI_SETTINGS_FILE_NAME)
}

pub fn default_ui_settings() -> Value {
    json!({
        "showInTray": false,
        "minimizeToTrayOnClose": false,
        "useAppWindowControls": false,
        "enableClaudePluginIntegration": false,
        "skipClaudeOnboarding": false,
        "launchOnStartup": false,
        "silentStartup": false,
        "enableLocalProxy": true,
        "proxyConfirmed": true,
        "usageConfirmed": true,
        "streamCheckConfirmed": true,
        "enableFailoverToggle": true,
        "preserveCodexOfficialAuthOnSwitch": false,
        "unifyCodexSessionHistory": false,
        "unifyCodexMigrateExisting": false,
        "failoverConfirmed": true,
        "firstRunNoticeConfirmed": true,
        "autoSyncConfirmed": true,
        "commonConfigConfirmed": false,
        "oauthQuotaRefreshIntervalMinutes": 30,
        "language": "zh",
        "visibleApps": {
            "claude": true,
            "claude-desktop": false,
            "codex": true,
            "gemini": true,
            "opencode": false,
            "openclaw": false,
            "hermes": false
        },
        "backupIntervalHours": 24,
        "backupRetainCount": 10,
        "rectifierConfig": default_rectifier_config(),
        "optimizerConfig": default_optimizer_config(),
        "logConfig": default_log_config(),
        "streamCheckConfig": default_stream_check_config(),
    })
}

pub fn default_rectifier_config() -> Value {
    json!({
        "enabled": true,
        "requestThinkingSignature": true,
        "requestThinkingBudget": true,
        "requestMediaFallback": true,
        "requestMediaHeuristic": true,
    })
}

pub fn default_optimizer_config() -> Value {
    json!({
        "enabled": false,
        "thinkingOptimizer": true,
        "cacheInjection": true,
        "cacheTtl": "1h",
    })
}

pub fn rectifier_config_for_frontend(store: &UiSettingsStore) -> Value {
    let stored = store
        .value
        .get("rectifierConfig")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    merge_json_values(default_rectifier_config(), stored)
}

pub fn optimizer_config_for_frontend(store: &UiSettingsStore) -> Value {
    let stored = store
        .value
        .get("optimizerConfig")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    merge_json_values(default_optimizer_config(), stored)
}

pub fn default_log_config() -> Value {
    json!({
        "enabled": true,
        "level": "info",
    })
}

pub fn default_stream_check_config() -> Value {
    json!({
        "timeoutSecs": 45,
        "maxRetries": 2,
        "degradedThresholdMs": 6000,
        "claudeModel": "claude-haiku-4-5-20251001",
        "codexModel": "gpt-5.5@low",
        "geminiModel": "gemini-3.5-flash",
        "testPrompt": "Who are you?",
    })
}

pub fn log_config_for_frontend(store: &UiSettingsStore) -> Value {
    let stored = store
        .value
        .get("logConfig")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    merge_json_values(default_log_config(), stored)
}

pub fn stream_check_config_for_frontend(store: &UiSettingsStore) -> Value {
    let stored = store
        .value
        .get("streamCheckConfig")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    merge_json_values(default_stream_check_config(), stored)
}

pub fn normalize_common_config_app_type(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-desktop" => Some("claude"),
        "codex" | "omo" | "omo_slim" => Some("codex"),
        "gemini" => Some("gemini"),
        _ => None,
    }
}

pub fn common_config_snippet_for_frontend(store: &UiSettingsStore, app_type: &str) -> Value {
    let Some(key) = normalize_common_config_app_type(app_type) else {
        return Value::Null;
    };
    store
        .value
        .get("commonConfigSnippets")
        .and_then(|snippets| snippets.get(key))
        .and_then(Value::as_str)
        .filter(|snippet| !snippet.trim().is_empty())
        .map(|snippet| json!(snippet))
        .unwrap_or(Value::Null)
}

fn merge_json_values(base: Value, patch: Value) -> Value {
    match (base, patch) {
        (Value::Object(mut base_map), Value::Object(patch_map)) => {
            for (key, patch_value) in patch_map {
                match base_map.get_mut(&key) {
                    Some(existing) => {
                        *existing = merge_json_values(existing.clone(), patch_value);
                    }
                    None => {
                        base_map.insert(key, patch_value);
                    }
                }
            }
            Value::Object(base_map)
        }
        (_, patch_value) => patch_value,
    }
}

pub fn settings_patch_from_args(args: &Value) -> Result<Value, String> {
    args.get("settings")
        .cloned()
        .ok_or_else(|| "settings payload is required".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_nested_objects() {
        let base = json!({
            "commonConfigConfirmed": false,
            "visibleApps": { "claude": true, "codex": true }
        });
        let patch = json!({
            "commonConfigConfirmed": true,
            "visibleApps": { "gemini": true }
        });
        let merged = merge_json_values(base, patch);
        assert_eq!(merged["commonConfigConfirmed"], json!(true));
        assert_eq!(merged["visibleApps"]["claude"], json!(true));
        assert_eq!(merged["visibleApps"]["codex"], json!(true));
        assert_eq!(merged["visibleApps"]["gemini"], json!(true));
    }

    #[test]
    fn for_frontend_applies_defaults() {
        let store = UiSettingsStore {
            value: json!({ "commonConfigConfirmed": true }),
        };
        let frontend = store.for_frontend();
        assert_eq!(frontend["commonConfigConfirmed"], json!(true));
        assert_eq!(frontend["enableFailoverToggle"], json!(true));
        assert!(frontend.get("visibleApps").is_some());
    }

    #[test]
    fn rectifier_and_optimizer_configs_merge_defaults() {
        let store = UiSettingsStore {
            value: json!({
                "rectifierConfig": { "enabled": false },
                "optimizerConfig": { "enabled": true, "cacheTtl": "5m" }
            }),
        };
        let rectifier = rectifier_config_for_frontend(&store);
        assert_eq!(rectifier["enabled"], json!(false));
        assert_eq!(rectifier["requestThinkingSignature"], json!(true));

        let optimizer = optimizer_config_for_frontend(&store);
        assert_eq!(optimizer["enabled"], json!(true));
        assert_eq!(optimizer["cacheTtl"], json!("5m"));
        assert_eq!(optimizer["thinkingOptimizer"], json!(true));
    }

    #[test]
    fn common_config_snippet_round_trip() {
        let mut store = UiSettingsStore::default();
        store.apply_patch(json!({
            "commonConfigSnippets": {
                "claude": "{ \"env\": {} }",
                "codex": "# common"
            }
        }));
        assert_eq!(
            common_config_snippet_for_frontend(&store, "claude"),
            json!("{ \"env\": {} }")
        );
        assert_eq!(
            normalize_common_config_app_type("claude-desktop"),
            Some("claude")
        );
        assert_eq!(normalize_common_config_app_type("omo"), Some("codex"));
        assert!(normalize_common_config_app_type("hermes").is_none());
    }

    #[test]
    fn log_and_stream_check_configs_merge_defaults() {
        let store = UiSettingsStore {
            value: json!({
                "logConfig": { "level": "debug" },
                "streamCheckConfig": { "timeoutSecs": 30, "claudeModel": "custom-model" }
            }),
        };
        let log = log_config_for_frontend(&store);
        assert_eq!(log["enabled"], json!(true));
        assert_eq!(log["level"], json!("debug"));

        let stream = stream_check_config_for_frontend(&store);
        assert_eq!(stream["timeoutSecs"], json!(30));
        assert_eq!(stream["claudeModel"], json!("custom-model"));
        assert_eq!(stream["maxRetries"], json!(2));
        assert_eq!(stream["geminiModel"], json!("gemini-3.5-flash"));
    }

    #[test]
    fn round_trip_save_load() {
        let dir =
            std::env::temp_dir().join(format!("cc-switch-ui-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut store = UiSettingsStore::default();
        store.apply_patch(json!({ "language": "en" }));
        store.save(&dir).unwrap();
        let loaded = UiSettingsStore::load_or_default(&dir).unwrap();
        assert_eq!(loaded.value["language"], json!("en"));
        let _ = fs::remove_dir_all(&dir);
    }
}
