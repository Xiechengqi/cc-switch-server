use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    Claude,
    Codex,
    Gemini,
}

impl AppKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderType {
    #[serde(rename = "claude")]
    Claude,
    #[serde(rename = "claude_auth")]
    ClaudeAuth,
    #[serde(rename = "claude_oauth")]
    ClaudeOAuth,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "codex_oauth")]
    CodexOAuth,
    #[serde(rename = "gemini")]
    Gemini,
    #[serde(rename = "gemini_cli")]
    GeminiCli,
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "github_copilot")]
    GitHubCopilot,
    #[serde(rename = "deepseek_account")]
    DeepSeekAccount,
    #[serde(rename = "kiro_oauth")]
    KiroOAuth,
    #[serde(rename = "cursor_oauth")]
    CursorOAuth,
    #[serde(rename = "cursor_apikey")]
    CursorApiKey,
    #[serde(rename = "antigravity_oauth")]
    AntigravityOAuth,
    #[serde(rename = "agy_oauth")]
    AgyOAuth,
    #[serde(rename = "ollama_cloud")]
    OllamaCloud,
    #[serde(rename = "aws_bedrock")]
    AwsBedrock,
    #[serde(rename = "nvidia")]
    Nvidia,
    #[serde(rename = "deepseek_api")]
    DeepSeekApi,
    #[serde(rename = "grok_oauth")]
    GrokOAuth,
}

impl ProviderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeAuth => "claude_auth",
            Self::ClaudeOAuth => "claude_oauth",
            Self::Codex => "codex",
            Self::CodexOAuth => "codex_oauth",
            Self::Gemini => "gemini",
            Self::GeminiCli => "gemini_cli",
            Self::OpenRouter => "openrouter",
            Self::GitHubCopilot => "github_copilot",
            Self::DeepSeekAccount => "deepseek_account",
            Self::KiroOAuth => "kiro_oauth",
            Self::CursorOAuth => "cursor_oauth",
            Self::CursorApiKey => "cursor_apikey",
            Self::AntigravityOAuth => "antigravity_oauth",
            Self::AgyOAuth => "agy_oauth",
            Self::OllamaCloud => "ollama_cloud",
            Self::AwsBedrock => "aws_bedrock",
            Self::Nvidia => "nvidia",
            Self::DeepSeekApi => "deepseek_api",
            Self::GrokOAuth => "grok_oauth",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub settings_config: Value,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub meta: Option<ProviderMeta>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMeta {
    #[serde(default, rename = "custom_endpoints", alias = "customEndpoints")]
    pub custom_endpoints: Option<BTreeMap<String, Value>>,
    #[serde(default)]
    pub common_config_enabled: Option<bool>,
    #[serde(default)]
    pub claude_desktop_mode: Option<String>,
    #[serde(default)]
    pub claude_desktop_model_routes: Option<BTreeMap<String, Value>>,
    #[serde(default, rename = "usage_script", alias = "usageScript")]
    pub usage_script: Option<Value>,
    #[serde(default)]
    pub endpoint_auto_select: Option<bool>,
    #[serde(default)]
    pub is_partner: Option<bool>,
    #[serde(default)]
    pub partner_promotion_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub test_config: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_multiplier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_model_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale_official_price_percent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_dispatch_limit_percent: Option<u32>,
    #[serde(default)]
    pub api_format: Option<String>,
    #[serde(default)]
    pub provider_type: Option<String>,
    #[serde(default)]
    #[serde(alias = "github_account_id")]
    pub github_account_id: Option<String>,
    #[serde(default)]
    pub auth_binding: Option<AuthBinding>,
    #[serde(default)]
    pub api_key_field: Option<String>,
    #[serde(default)]
    pub custom_user_agent: Option<String>,
    #[serde(default)]
    pub is_full_url: Option<bool>,
    #[serde(default)]
    pub prompt_cache_key: Option<String>,
    #[serde(default)]
    pub codex_fast_mode: Option<bool>,
    #[serde(default)]
    pub codex_image_generation_enabled: Option<bool>,
    #[serde(default)]
    pub codex_chat_reasoning: Option<Value>,
    #[serde(default)]
    pub local_proxy_request_overrides: Option<Value>,
    #[serde(default, flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthBinding {
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub auth_provider: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTypeRequest {
    pub app: AppKind,
    pub provider: Provider,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTypeResponse {
    pub provider_type: ProviderType,
    pub provider_type_id: &'static str,
}

pub fn classify_provider(app: AppKind, provider: &Provider) -> ProviderType {
    match app {
        AppKind::Claude => classify_claude_provider(provider),
        AppKind::Codex => classify_codex_provider(provider),
        AppKind::Gemini => classify_gemini_provider(provider),
    }
}

pub fn classify_provider_response(app: AppKind, provider: &Provider) -> ProviderTypeResponse {
    let provider_type = classify_provider(app, provider);
    ProviderTypeResponse {
        provider_type,
        provider_type_id: provider_type.as_str(),
    }
}

fn classify_claude_provider(provider: &Provider) -> ProviderType {
    match provider_type(provider) {
        Some("antigravity_oauth") => return ProviderType::AntigravityOAuth,
        Some("agy_oauth") => return ProviderType::AgyOAuth,
        Some("claude") => return ProviderType::Claude,
        Some("claude_auth") => return ProviderType::ClaudeAuth,
        Some("gemini") => return ProviderType::Gemini,
        Some("gemini_cli") | Some("google_gemini_oauth") => return ProviderType::GeminiCli,
        Some("openrouter") => return ProviderType::OpenRouter,
        _ => {}
    }

    if claude_api_format(provider) == Some("gemini_native") {
        return if gemini_uses_oauth(provider) {
            ProviderType::GeminiCli
        } else {
            ProviderType::Gemini
        };
    }

    match provider_type(provider) {
        Some("github_copilot") => return ProviderType::GitHubCopilot,
        Some("codex_oauth") => return ProviderType::CodexOAuth,
        Some("claude_oauth") => return ProviderType::ClaudeOAuth,
        Some("deepseek_account") => return ProviderType::DeepSeekAccount,
        Some("aws_bedrock") => return ProviderType::AwsBedrock,
        Some("nvidia") => return ProviderType::Nvidia,
        Some("deepseek_api") => return ProviderType::DeepSeekApi,
        Some("grok_oauth") => return ProviderType::GrokOAuth,
        Some("ollama_cloud") => return ProviderType::OllamaCloud,
        Some("kiro_oauth") => return ProviderType::KiroOAuth,
        Some("cursor_oauth") => return ProviderType::CursorOAuth,
        Some("cursor_apikey") => return ProviderType::CursorApiKey,
        _ => {}
    }

    if let Some(base_url) = claude_base_url(provider) {
        if base_url.contains("githubcopilot.com") {
            return ProviderType::GitHubCopilot;
        }
        if base_url.contains("openrouter.ai") {
            return ProviderType::OpenRouter;
        }
        if base_url.contains("bedrock-runtime.") || base_url.contains("amazonaws.com") {
            return ProviderType::AwsBedrock;
        }
        if base_url.contains("integrate.api.nvidia.com") {
            return ProviderType::Nvidia;
        }
        if base_url.contains("api.deepseek.com") {
            return ProviderType::DeepSeekApi;
        }
    }

    if auth_mode(provider) == Some("bearer_only") {
        return ProviderType::ClaudeAuth;
    }

    ProviderType::Claude
}

fn classify_codex_provider(provider: &Provider) -> ProviderType {
    match provider_type(provider) {
        Some("claude") => ProviderType::Claude,
        Some("claude_auth") => ProviderType::ClaudeAuth,
        Some("claude_oauth") => ProviderType::ClaudeOAuth,
        Some("gemini") => ProviderType::Gemini,
        Some("gemini_cli") | Some("google_gemini_oauth") => ProviderType::GeminiCli,
        Some("openrouter") => ProviderType::OpenRouter,
        Some("cursor_oauth") => ProviderType::CursorOAuth,
        Some("cursor_apikey") => ProviderType::CursorApiKey,
        Some("ollama_cloud") => ProviderType::OllamaCloud,
        Some("codex_oauth") => ProviderType::CodexOAuth,
        Some("github_copilot") => ProviderType::GitHubCopilot,
        Some("nvidia") => ProviderType::Nvidia,
        Some("deepseek_api") => ProviderType::DeepSeekApi,
        Some("grok_oauth") => ProviderType::GrokOAuth,
        _ => {
            if provider_base_url(provider).is_some_and(|url| url.contains("githubcopilot.com")) {
                ProviderType::GitHubCopilot
            } else if provider_base_url(provider).is_some_and(|url| url.contains("openrouter.ai")) {
                ProviderType::OpenRouter
            } else if provider_base_url(provider)
                .is_some_and(|url| url.contains("integrate.api.nvidia.com"))
            {
                ProviderType::Nvidia
            } else if provider_base_url(provider)
                .is_some_and(|url| url.contains("api.deepseek.com"))
            {
                ProviderType::DeepSeekApi
            } else {
                ProviderType::Codex
            }
        }
    }
}

fn classify_gemini_provider(provider: &Provider) -> ProviderType {
    match provider_type(provider) {
        Some("claude") => return ProviderType::Claude,
        Some("claude_auth") => return ProviderType::ClaudeAuth,
        Some("claude_oauth") => return ProviderType::ClaudeOAuth,
        Some("codex") => return ProviderType::Codex,
        Some("codex_oauth") => return ProviderType::CodexOAuth,
        Some("gemini") => return ProviderType::Gemini,
        Some("gemini_cli") => return ProviderType::GeminiCli,
        Some("openrouter") => return ProviderType::OpenRouter,
        Some("github_copilot") => return ProviderType::GitHubCopilot,
        Some("nvidia") => return ProviderType::Nvidia,
        Some("deepseek_api") => return ProviderType::DeepSeekApi,
        Some("grok_oauth") => return ProviderType::GrokOAuth,
        Some("antigravity_oauth") => return ProviderType::AntigravityOAuth,
        Some("agy_oauth") => return ProviderType::AgyOAuth,
        Some("google_gemini_oauth") => return ProviderType::GeminiCli,
        _ => {}
    }

    if provider_base_url(provider).is_some_and(|url| url.contains("githubcopilot.com")) {
        ProviderType::GitHubCopilot
    } else if provider_base_url(provider).is_some_and(|url| url.contains("openrouter.ai")) {
        ProviderType::OpenRouter
    } else if gemini_uses_oauth(provider) {
        ProviderType::GeminiCli
    } else {
        ProviderType::Gemini
    }
}

fn provider_type(provider: &Provider) -> Option<&str> {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.provider_type.as_deref())
}

fn claude_api_format(provider: &Provider) -> Option<&str> {
    provider
        .meta
        .as_ref()
        .and_then(|meta| meta.api_format.as_deref())
}

fn claude_base_url(provider: &Provider) -> Option<&str> {
    app_base_url(provider, AppKind::Claude)
}

fn provider_base_url(provider: &Provider) -> Option<&str> {
    app_base_url(provider, AppKind::Claude)
        .or_else(|| app_base_url(provider, AppKind::Codex))
        .or_else(|| app_base_url(provider, AppKind::Gemini))
        .or_else(|| {
            provider
                .settings_config
                .get("BASE_URL")
                .and_then(Value::as_str)
        })
}

fn app_base_url(provider: &Provider, app: AppKind) -> Option<&str> {
    let keys: &[&str] = match app {
        AppKind::Claude => &["ANTHROPIC_BASE_URL", "BASE_URL"],
        AppKind::Codex => &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
        AppKind::Gemini => &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
    };

    for key in keys {
        let pointer = format!("/env/{key}");
        if let Some(value) = provider
            .settings_config
            .pointer(&pointer)
            .and_then(Value::as_str)
        {
            return Some(value);
        }
        if let Some(value) = provider.settings_config.get(*key).and_then(Value::as_str) {
            return Some(value);
        }
    }
    None
}

fn auth_mode(provider: &Provider) -> Option<&str> {
    provider
        .settings_config
        .get("auth_mode")
        .and_then(Value::as_str)
        .or_else(|| {
            provider
                .settings_config
                .pointer("/env/AUTH_MODE")
                .and_then(Value::as_str)
        })
}

fn gemini_uses_oauth(provider: &Provider) -> bool {
    gemini_api_key(provider).is_some_and(|key| {
        let value = key.trim();
        value.starts_with("ya29.") || value.starts_with('{')
    })
}

fn gemini_api_key(provider: &Provider) -> Option<&str> {
    provider
        .settings_config
        .pointer("/env/GEMINI_API_KEY")
        .and_then(Value::as_str)
        .or_else(|| {
            provider
                .settings_config
                .pointer("/env/GOOGLE_API_KEY")
                .and_then(Value::as_str)
        })
        .or_else(|| {
            provider
                .settings_config
                .get("GEMINI_API_KEY")
                .and_then(Value::as_str)
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn provider(meta_type: Option<&str>) -> Provider {
        Provider {
            id: "p1".to_string(),
            name: "test".to_string(),
            settings_config: json!({}),
            category: None,
            meta: meta_type.map(|provider_type| ProviderMeta {
                provider_type: Some(provider_type.to_string()),
                ..ProviderMeta::default()
            }),
            extra: Default::default(),
        }
    }

    #[test]
    fn classifies_claude_meta_provider_types() {
        let cases = [
            ("github_copilot", ProviderType::GitHubCopilot),
            ("claude", ProviderType::Claude),
            ("claude_auth", ProviderType::ClaudeAuth),
            ("codex_oauth", ProviderType::CodexOAuth),
            ("claude_oauth", ProviderType::ClaudeOAuth),
            ("gemini", ProviderType::Gemini),
            ("gemini_cli", ProviderType::GeminiCli),
            ("openrouter", ProviderType::OpenRouter),
            ("deepseek_account", ProviderType::DeepSeekAccount),
            ("aws_bedrock", ProviderType::AwsBedrock),
            ("nvidia", ProviderType::Nvidia),
            ("deepseek_api", ProviderType::DeepSeekApi),
            ("ollama_cloud", ProviderType::OllamaCloud),
            ("kiro_oauth", ProviderType::KiroOAuth),
            ("cursor_oauth", ProviderType::CursorOAuth),
            ("cursor_apikey", ProviderType::CursorApiKey),
            ("antigravity_oauth", ProviderType::AntigravityOAuth),
            ("agy_oauth", ProviderType::AgyOAuth),
        ];
        for (meta_type, expected) in cases {
            assert_eq!(
                classify_provider(AppKind::Claude, &provider(Some(meta_type))),
                expected
            );
        }
    }

    #[test]
    fn provider_meta_omits_pricing_fields_when_none() {
        let meta = ProviderMeta::default();
        let value = serde_json::to_value(&meta).expect("serialize ProviderMeta");

        assert!(value.get("costMultiplier").is_none());
        assert!(value.get("pricingModelSource").is_none());
        assert!(value.get("quotaDispatchLimitPercent").is_none());
        assert!(value.get("forSaleOfficialPricePercent").is_none());
        assert!(value.get("testConfig").is_none());
    }

    #[test]
    fn provider_meta_serializes_pricing_fields_when_set() {
        let meta = ProviderMeta {
            cost_multiplier: Some("1.5".to_string()),
            pricing_model_source: Some("response".to_string()),
            quota_dispatch_limit_percent: Some(80),
            ..ProviderMeta::default()
        };
        let value = serde_json::to_value(&meta).expect("serialize ProviderMeta");

        assert_eq!(
            value.get("costMultiplier").and_then(|item| item.as_str()),
            Some("1.5")
        );
        assert_eq!(
            value
                .get("pricingModelSource")
                .and_then(|item| item.as_str()),
            Some("response")
        );
        assert_eq!(
            value
                .get("quotaDispatchLimitPercent")
                .and_then(|item| item.as_u64()),
            Some(80)
        );
    }

    #[test]
    fn serializes_provider_type_with_cc_switch_ids() {
        assert_eq!(
            serde_json::to_value(ProviderType::GitHubCopilot).unwrap(),
            json!("github_copilot")
        );
        assert_eq!(
            serde_json::to_value(ProviderType::ClaudeOAuth).unwrap(),
            json!("claude_oauth")
        );
        assert_eq!(
            serde_json::to_value(ProviderType::CursorApiKey).unwrap(),
            json!("cursor_apikey")
        );
        assert_eq!(
            serde_json::to_value(ProviderType::AwsBedrock).unwrap(),
            json!("aws_bedrock")
        );
    }

    #[test]
    fn classifies_claude_base_url_and_auth_mode_fallbacks() {
        let mut copilot = provider(None);
        copilot.settings_config =
            json!({"env": {"ANTHROPIC_BASE_URL": "https://api.githubcopilot.com"}});
        assert_eq!(
            classify_provider(AppKind::Claude, &copilot),
            ProviderType::GitHubCopilot
        );

        let mut openrouter = provider(None);
        openrouter.settings_config =
            json!({"env": {"ANTHROPIC_BASE_URL": "https://openrouter.ai/api"}});
        assert_eq!(
            classify_provider(AppKind::Claude, &openrouter),
            ProviderType::OpenRouter
        );

        let mut nvidia = provider(None);
        nvidia.settings_config =
            json!({"env": {"ANTHROPIC_BASE_URL": "https://integrate.api.nvidia.com"}});
        assert_eq!(
            classify_provider(AppKind::Claude, &nvidia),
            ProviderType::Nvidia
        );

        let mut bedrock = provider(None);
        bedrock.settings_config = json!({
            "env": {
                "ANTHROPIC_BASE_URL": "https://bedrock-runtime.us-west-2.amazonaws.com",
                "CLAUDE_CODE_USE_BEDROCK": "1"
            }
        });
        assert_eq!(
            classify_provider(AppKind::Claude, &bedrock),
            ProviderType::AwsBedrock
        );

        let mut bearer = provider(None);
        bearer.settings_config = json!({"env": {"AUTH_MODE": "bearer_only"}});
        assert_eq!(
            classify_provider(AppKind::Claude, &bearer),
            ProviderType::ClaudeAuth
        );
    }

    #[test]
    fn classifies_codex_meta_provider_types() {
        let cases = [
            ("claude", ProviderType::Claude),
            ("claude_auth", ProviderType::ClaudeAuth),
            ("claude_oauth", ProviderType::ClaudeOAuth),
            ("gemini", ProviderType::Gemini),
            ("gemini_cli", ProviderType::GeminiCli),
            ("openrouter", ProviderType::OpenRouter),
            ("cursor_oauth", ProviderType::CursorOAuth),
            ("cursor_apikey", ProviderType::CursorApiKey),
            ("ollama_cloud", ProviderType::OllamaCloud),
            ("codex_oauth", ProviderType::CodexOAuth),
            ("github_copilot", ProviderType::GitHubCopilot),
            ("nvidia", ProviderType::Nvidia),
            ("deepseek_api", ProviderType::DeepSeekApi),
        ];
        for (meta_type, expected) in cases {
            assert_eq!(
                classify_provider(AppKind::Codex, &provider(Some(meta_type))),
                expected
            );
        }
        assert_eq!(
            classify_provider(AppKind::Codex, &provider(None)),
            ProviderType::Codex
        );

        let mut openrouter = provider(None);
        openrouter.settings_config =
            json!({"env": {"OPENAI_BASE_URL": "https://openrouter.ai/api/v1"}});
        assert_eq!(
            classify_provider(AppKind::Codex, &openrouter),
            ProviderType::OpenRouter
        );

        let mut deepseek = provider(None);
        deepseek.settings_config = json!({"env": {"OPENAI_BASE_URL": "https://api.deepseek.com"}});
        assert_eq!(
            classify_provider(AppKind::Codex, &deepseek),
            ProviderType::DeepSeekApi
        );

        let mut copilot = provider(None);
        copilot.settings_config =
            json!({"env": {"OPENAI_BASE_URL": "https://api.githubcopilot.com"}});
        assert_eq!(
            classify_provider(AppKind::Codex, &copilot),
            ProviderType::GitHubCopilot
        );
    }

    #[test]
    fn classifies_gemini_meta_and_oauth_keys() {
        assert_eq!(
            classify_provider(AppKind::Gemini, &provider(Some("antigravity_oauth"))),
            ProviderType::AntigravityOAuth
        );
        assert_eq!(
            classify_provider(AppKind::Gemini, &provider(Some("agy_oauth"))),
            ProviderType::AgyOAuth
        );
        assert_eq!(
            classify_provider(AppKind::Gemini, &provider(Some("google_gemini_oauth"))),
            ProviderType::GeminiCli
        );
        let cases = [
            ("claude", ProviderType::Claude),
            ("claude_auth", ProviderType::ClaudeAuth),
            ("claude_oauth", ProviderType::ClaudeOAuth),
            ("codex", ProviderType::Codex),
            ("codex_oauth", ProviderType::CodexOAuth),
            ("gemini", ProviderType::Gemini),
            ("gemini_cli", ProviderType::GeminiCli),
            ("openrouter", ProviderType::OpenRouter),
            ("github_copilot", ProviderType::GitHubCopilot),
            ("nvidia", ProviderType::Nvidia),
            ("deepseek_api", ProviderType::DeepSeekApi),
        ];
        for (meta_type, expected) in cases {
            assert_eq!(
                classify_provider(AppKind::Gemini, &provider(Some(meta_type))),
                expected
            );
        }

        let mut oauth = provider(None);
        oauth.settings_config = json!({"env": {"GEMINI_API_KEY": "ya29.token"}});
        assert_eq!(
            classify_provider(AppKind::Gemini, &oauth),
            ProviderType::GeminiCli
        );

        let mut openrouter = provider(None);
        openrouter.settings_config =
            json!({"env": {"GOOGLE_GEMINI_BASE_URL": "https://openrouter.ai/api"}});
        assert_eq!(
            classify_provider(AppKind::Gemini, &openrouter),
            ProviderType::OpenRouter
        );
        let mut copilot = provider(None);
        copilot.settings_config =
            json!({"env": {"GEMINI_BASE_URL": "https://api.githubcopilot.com"}});
        assert_eq!(
            classify_provider(AppKind::Gemini, &copilot),
            ProviderType::GitHubCopilot
        );
        assert_eq!(
            classify_provider(AppKind::Gemini, &provider(None)),
            ProviderType::Gemini
        );
    }

    #[test]
    fn classifies_claude_gemini_native() {
        let mut gemini_native = provider(None);
        gemini_native.meta = Some(ProviderMeta {
            api_format: Some("gemini_native".to_string()),
            ..ProviderMeta::default()
        });
        gemini_native.settings_config = json!({"env": {"GEMINI_API_KEY": "plain-key"}});
        assert_eq!(
            classify_provider(AppKind::Claude, &gemini_native),
            ProviderType::Gemini
        );

        gemini_native.settings_config = json!({"env": {"GEMINI_API_KEY": "ya29.token"}});
        assert_eq!(
            classify_provider(AppKind::Claude, &gemini_native),
            ProviderType::GeminiCli
        );
    }
}
