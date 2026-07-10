use std::fs;
use std::path::Path;

use anyhow::Context;
use serde_json::{json, Value};

use crate::domain::settings::config::ServerConfig;
use crate::domain::sharing::share_router_domain::{
    router_domain_from_url, DEFAULT_SHARE_ROUTER_DOMAIN,
};

const UI_SETTINGS_FILE_NAME: &str = "ui-settings.json";
pub const DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES: u64 = 30;
pub const DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS: u64 = 10;

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
        crate::infra::storage::write_json_pretty(&path, &self.value)
            .with_context(|| format!("write ui settings {}", path.display()))
    }

    pub fn for_frontend(&self) -> Value {
        merge_json_values(default_ui_settings(), self.value.clone())
    }

    pub fn settings_for_frontend(&self, server_config: &ServerConfig) -> Value {
        let mut settings = self.for_frontend();
        let stored_domain = self
            .value
            .get("shareRouterDomain")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let resolved_domain = stored_domain
            .or_else(|| server_config.router.domain.clone())
            .or_else(|| router_domain_from_url(server_config.router.url.as_deref()))
            .unwrap_or_else(|| DEFAULT_SHARE_ROUTER_DOMAIN.to_string());
        if let Value::Object(ref mut map) = settings {
            map.insert("shareRouterDomain".into(), json!(resolved_domain));
        }
        settings
    }

    pub fn apply_patch(&mut self, patch: Value) {
        self.value = merge_json_values(self.value.clone(), patch);
    }
}

pub fn ui_settings_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(UI_SETTINGS_FILE_NAME)
}

pub fn oauth_quota_refresh_interval_minutes_from_value(value: &Value) -> u64 {
    value
        .get("oauthQuotaRefreshIntervalMinutes")
        .and_then(Value::as_u64)
        .filter(|minutes| *minutes >= 1)
        .unwrap_or(DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES)
}

pub fn oauth_quota_refresh_interval_minutes(store: &UiSettingsStore) -> u64 {
    oauth_quota_refresh_interval_minutes_from_value(&store.for_frontend())
}

pub fn oauth_quota_refresh_interval_ms(store: &UiSettingsStore) -> i64 {
    oauth_quota_refresh_interval_minutes(store) as i64 * 60 * 1000
}

pub fn default_oauth_quota_refresh_interval_ms() -> i64 {
    DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES as i64 * 60 * 1000
}

pub fn oauth_quota_refresh_timeout_seconds_from_value(value: &Value) -> u64 {
    value
        .get("oauthQuotaRefreshTimeoutSeconds")
        .and_then(Value::as_u64)
        .filter(|seconds| (1..=120).contains(seconds))
        .unwrap_or(DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS)
}

pub fn oauth_quota_refresh_timeout_seconds(store: &UiSettingsStore) -> u64 {
    oauth_quota_refresh_timeout_seconds_from_value(&store.for_frontend())
}

pub fn oauth_quota_refresh_timeout_ms(store: &UiSettingsStore) -> i64 {
    oauth_quota_refresh_timeout_seconds(store) as i64 * 1000
}

pub fn default_oauth_quota_refresh_timeout_ms() -> i64 {
    DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS as i64 * 1000
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
        "oauthQuotaRefreshTimeoutSeconds": 10,
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

pub const LOG_API_MAX_TAIL_LINES: usize = 1_000;
pub const LOG_API_DEFAULT_TAIL_LINES: usize = 100;

pub fn default_log_config() -> Value {
    json!({
        "enabled": true,
        "level": "info",
        "apiEnabled": false,
        "apiTailLines": LOG_API_DEFAULT_TAIL_LINES,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedLogConfig {
    pub enabled: bool,
    pub level: String,
    pub api_enabled: bool,
    pub api_tail_lines: usize,
}

pub fn parse_log_config(value: &Value) -> ParsedLogConfig {
    let enabled = value
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let level = value
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("info")
        .to_ascii_lowercase();
    let api_enabled = value
        .get("apiEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let api_tail_lines = value
        .get("apiTailLines")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(100)
        .clamp(1, LOG_API_MAX_TAIL_LINES);
    ParsedLogConfig {
        enabled,
        level,
        api_enabled,
        api_tail_lines,
    }
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
    fn oauth_quota_refresh_interval_reads_settings_or_default() {
        let store = UiSettingsStore {
            value: json!({ "oauthQuotaRefreshIntervalMinutes": 15 }),
        };
        assert_eq!(oauth_quota_refresh_interval_minutes(&store), 15);
        assert_eq!(oauth_quota_refresh_interval_ms(&store), 15 * 60 * 1000);

        let store = UiSettingsStore::default();
        assert_eq!(
            oauth_quota_refresh_interval_minutes(&store),
            DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES
        );
        assert_eq!(
            oauth_quota_refresh_interval_minutes_from_value(&json!(0)),
            DEFAULT_OAUTH_QUOTA_REFRESH_INTERVAL_MINUTES
        );
    }

    #[test]
    fn oauth_quota_refresh_timeout_reads_settings_or_default() {
        let store = UiSettingsStore {
            value: json!({ "oauthQuotaRefreshTimeoutSeconds": 20 }),
        };
        assert_eq!(oauth_quota_refresh_timeout_seconds(&store), 20);
        assert_eq!(oauth_quota_refresh_timeout_ms(&store), 20_000);

        let store = UiSettingsStore::default();
        assert_eq!(
            oauth_quota_refresh_timeout_seconds(&store),
            DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS
        );
        assert_eq!(
            oauth_quota_refresh_timeout_seconds_from_value(&json!(0)),
            DEFAULT_OAUTH_QUOTA_REFRESH_TIMEOUT_SECONDS
        );
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
        assert_eq!(log["apiEnabled"], json!(false));
        assert_eq!(log["apiTailLines"], json!(LOG_API_DEFAULT_TAIL_LINES));
    }

    #[test]
    fn parse_log_config_clamps_api_tail_lines() {
        let parsed = parse_log_config(&json!({
            "enabled": false,
            "level": "WARN",
            "apiEnabled": true,
            "apiTailLines": 5000
        }));
        assert!(!parsed.enabled);
        assert_eq!(parsed.level, "warn");
        assert!(parsed.api_enabled);
        assert_eq!(parsed.api_tail_lines, LOG_API_MAX_TAIL_LINES);

        let parsed = parse_log_config(&json!({}));
        assert!(parsed.enabled);
        assert_eq!(parsed.level, "info");
        assert!(!parsed.api_enabled);
        assert_eq!(parsed.api_tail_lines, LOG_API_DEFAULT_TAIL_LINES);
    }

    #[test]
    fn stream_check_config_merge_defaults() {
        let store = UiSettingsStore {
            value: json!({
                "streamCheckConfig": { "timeoutSecs": 30, "claudeModel": "custom-model" }
            }),
        };
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
