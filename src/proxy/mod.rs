#![allow(dead_code)]

mod account_headers;
pub mod adapters;
mod anthropic_cache_control;
mod anthropic_dateline;
mod anthropic_semantics;
mod antigravity_retry;
mod cache_injector;
mod claude_client_detection;
pub(crate) mod claude_oauth;
mod codex_compaction;
mod codex_http;
mod codex_metadata;
pub(crate) mod codex_models;
mod codex_request_policy;
mod copilot_model_map;
mod copilot_optimizer;
pub(crate) mod cursor;
mod deepseek;
mod downstream_keepalive;
mod forwarder;
mod grok;
pub(crate) mod kimi;
pub(crate) mod kimi_runtime;
pub(crate) mod kiro;
mod openai_capacity_shed;
mod outbound_identity;
pub(crate) mod outbound_request;
mod overflow_compact;
pub mod protocol_compat;
pub(crate) mod provider_ops;
pub(crate) mod qoder;
pub(crate) mod qoder_runtime;
pub(crate) mod reasoning_bridge;
mod remote_image;
mod request_governance;
mod response_semantics;
mod responses_wire;
mod retry_policy;
mod router;
mod stream_transforms;
mod streaming;
mod terminal_detector;
mod thinking;
mod tool_media;
mod tool_schema;
mod transforms;
mod usage;

use serde_json::Value;

pub use forwarder::forward;
pub use forwarder::forward_codex_alpha_search;
pub use forwarder::forward_codex_models_manifest;
pub use forwarder::forward_codex_responses_ws;
pub use forwarder::forward_grok_media;
pub(crate) use forwarder::forward_provider_test;

pub(crate) fn grok_fixed_api_base_url() -> &'static str {
    grok::default_base_url()
}
pub use forwarder::forward_images_edits;
pub use forwarder::forward_images_generations;
pub(crate) use forwarder::validate_and_acquire_share_invocation;
pub(crate) use request_governance::decode_request_body_for_proxy_with_limit;
pub use router::ProxyRoute;

/// Router 未声明上限时，普通 API 档的兜底值。等于本特性上线前的硬编码值。
pub const LEGACY_REQUEST_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;
/// Router 未声明上限时，视频档的兜底值。
pub const MEDIA_REQUEST_BODY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Router 未声明上限时，图片档的兜底值。
pub const CODEX_IMAGES_REQUEST_BODY_LIMIT_BYTES: usize = 48 * 1024 * 1024;
pub const MEDIA_RESPONSE_BODY_LIMIT_BYTES: usize = 64 * 1024 * 1024;

pub(super) const MAX_UPSTREAM_RATE_LIMIT_COOLDOWN_MS: i64 = 8 * 24 * 60 * 60 * 1000;

pub(super) fn bounded_upstream_rate_limit_until(now_ms: i64, until_ms: i64) -> i64 {
    until_ms.clamp(
        now_ms.saturating_add(1_000),
        now_ms.saturating_add(MAX_UPSTREAM_RATE_LIMIT_COOLDOWN_MS),
    )
}

pub fn capabilities() -> Vec<adapters::AdapterCapability> {
    adapters::all_capabilities()
}

#[derive(Debug)]
pub struct ProxyError {
    pub status: axum::http::StatusCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyConcurrencyScope {
    User,
    Share,
    ProviderAccount,
}

impl ProxyConcurrencyScope {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Share => "share",
            Self::ProviderAccount => "provider_account",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "share" => Some(Self::Share),
            "provider_account" => Some(Self::ProviderAccount),
            _ => None,
        }
    }

    fn error_code(self) -> &'static str {
        match self {
            Self::User => "cc_switch_user_concurrency_limit_exceeded",
            Self::Share => "cc_switch_share_concurrency_limit_exceeded",
            Self::ProviderAccount => "cc_switch_provider_account_concurrency_limit_exceeded",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyConcurrencyMetadata {
    pub scope: ProxyConcurrencyScope,
    pub current: u32,
    pub limit: u32,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProxyError {}

impl ProxyError {
    const TOOL_JSON_INVALID_PREFIX: &'static str = "[TOOL_JSON_INVALID] ";
    const TOOL_JSON_INCOMPLETE_PREFIX: &'static str = "[TOOL_JSON_INCOMPLETE] ";
    const TOOL_JSON_LIMIT_PREFIX: &'static str = "[TOOL_JSON_LIMIT] ";
    const KIRO_EVENT_STREAM_INVALID_PREFIX: &'static str = "[KIRO_EVENT_STREAM_INVALID] ";
    const KIRO_EVENT_STREAM_LIMIT_PREFIX: &'static str = "[KIRO_EVENT_STREAM_LIMIT] ";
    const KIRO_EVENT_STREAM_TIMEOUT_PREFIX: &'static str = "[KIRO_EVENT_STREAM_TIMEOUT] ";
    const KIRO_UPSTREAM_STREAM_ERROR_PREFIX: &'static str = "[KIRO_UPSTREAM_STREAM_ERROR] ";
    const CURSOR_SESSION_LOST_PREFIX: &'static str = "[CURSOR_SESSION_LOST] ";
    const CURSOR_RESPONSE_STATE_LOST_PREFIX: &'static str = "[CURSOR_RESPONSE_STATE_LOST] ";
    const CURSOR_CONVERSATION_BUSY_PREFIX: &'static str = "[CURSOR_CONVERSATION_BUSY] ";
    const RETRY_AFTER_PREFIX: &'static str = "[CC_RETRY_AFTER_SECONDS=";
    const CAPACITY_SHED_PREFIX: &'static str = "[CC_UPSTREAM_CAPACITY_SHED] ";
    const CONCURRENCY_PREFIX: &'static str = "[CC_CONCURRENCY:";
    const USER_IDENTITY_REQUIRED_PREFIX: &'static str = "[CC_USER_IDENTITY_REQUIRED] ";
    const PROTOCOL_INCOMPATIBLE_PREFIX: &'static str = "[CC_PROTOCOL_INCOMPATIBLE] ";
    const RESPONSE_CONTEXT_UNAVAILABLE_PREFIX: &'static str = "[CC_RESPONSE_CONTEXT_UNAVAILABLE] ";

    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(crate) fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::UNAUTHORIZED,
            message: message.into(),
        }
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub(super) fn concurrency_limited(
        scope: ProxyConcurrencyScope,
        current: u32,
        limit: u32,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: format!(
                "{}{scope}:{current}:{limit}] {}",
                Self::CONCURRENCY_PREFIX,
                message.into(),
                scope = scope.as_str(),
            ),
        }
    }

    pub(super) fn user_identity_required(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::UNAUTHORIZED,
            message: format!("{}{}", Self::USER_IDENTITY_REQUIRED_PREFIX, message.into()),
        }
    }

    pub(super) fn protocol_incompatible(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            message: format!("{}{}", Self::PROTOCOL_INCOMPATIBLE_PREFIX, message.into()),
        }
    }

    pub(super) fn response_context_unavailable() -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: format!(
                "{}Required previous response tool context is unavailable",
                Self::RESPONSE_CONTEXT_UNAVAILABLE_PREFIX
            ),
        }
    }

    pub(super) fn is_protocol_incompatible(&self) -> bool {
        self.message_without_retry_metadata()
            .starts_with(Self::PROTOCOL_INCOMPATIBLE_PREFIX)
    }

    pub(super) fn cursor_session_lost(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: format!("{}{}", Self::CURSOR_SESSION_LOST_PREFIX, message.into()),
        }
    }

    pub(super) fn cursor_response_state_lost(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: format!(
                "{}{}",
                Self::CURSOR_RESPONSE_STATE_LOST_PREFIX,
                message.into()
            ),
        }
    }

    pub(super) fn cursor_conversation_busy(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: format!(
                "{}{}",
                Self::CURSOR_CONVERSATION_BUSY_PREFIX,
                message.into()
            ),
        }
    }

    pub(super) fn bad_gateway(error: impl std::fmt::Display) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_GATEWAY,
            message: format!("proxy upstream request failed: {error}"),
        }
    }

    pub(super) fn upstream_capacity_shed(retry_after_seconds: u64) -> Self {
        Self {
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            message: format!(
                "{}{retry_after_seconds}]{}OpenAI Codex upstream is temporarily overloaded; retry shortly",
                Self::RETRY_AFTER_PREFIX,
                Self::CAPACITY_SHED_PREFIX,
            ),
        }
    }

    pub(super) fn rate_limited(message: impl Into<String>, retry_after_seconds: u64) -> Self {
        Self {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            message: format!(
                "{}{retry_after_seconds}]{}",
                Self::RETRY_AFTER_PREFIX,
                message.into()
            ),
        }
    }

    pub(super) fn kiro_tool_json(error: kiro::KiroToolJsonError) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_GATEWAY,
            message: format!("[{}] {error}", error.code()),
        }
    }

    pub fn client_message(&self) -> &str {
        let message = self
            .concurrency_metadata_and_message()
            .map(|(_, message)| message)
            .unwrap_or_else(|| self.message_without_retry_metadata());
        message
            .strip_prefix(Self::TOOL_JSON_INVALID_PREFIX)
            .or_else(|| message.strip_prefix(Self::TOOL_JSON_INCOMPLETE_PREFIX))
            .or_else(|| message.strip_prefix(Self::TOOL_JSON_LIMIT_PREFIX))
            .or_else(|| message.strip_prefix(Self::KIRO_EVENT_STREAM_INVALID_PREFIX))
            .or_else(|| message.strip_prefix(Self::KIRO_EVENT_STREAM_LIMIT_PREFIX))
            .or_else(|| message.strip_prefix(Self::KIRO_EVENT_STREAM_TIMEOUT_PREFIX))
            .or_else(|| message.strip_prefix(Self::KIRO_UPSTREAM_STREAM_ERROR_PREFIX))
            .or_else(|| message.strip_prefix(Self::CURSOR_SESSION_LOST_PREFIX))
            .or_else(|| message.strip_prefix(Self::CURSOR_RESPONSE_STATE_LOST_PREFIX))
            .or_else(|| message.strip_prefix(Self::CURSOR_CONVERSATION_BUSY_PREFIX))
            .or_else(|| message.strip_prefix(Self::USER_IDENTITY_REQUIRED_PREFIX))
            .or_else(|| message.strip_prefix(Self::PROTOCOL_INCOMPATIBLE_PREFIX))
            .or_else(|| message.strip_prefix(Self::RESPONSE_CONTEXT_UNAVAILABLE_PREFIX))
            .or_else(|| message.strip_prefix(Self::CAPACITY_SHED_PREFIX))
            .unwrap_or(message)
    }

    pub fn retry_after_seconds(&self) -> Option<u64> {
        let encoded = self.message.strip_prefix(Self::RETRY_AFTER_PREFIX)?;
        let (seconds, _) = encoded.split_once(']')?;
        seconds.parse::<u64>().ok()
    }

    pub fn concurrency_metadata(&self) -> Option<ProxyConcurrencyMetadata> {
        self.concurrency_metadata_and_message()
            .map(|(metadata, _)| metadata)
    }

    pub fn error_scope(&self) -> Option<&'static str> {
        self.concurrency_metadata()
            .map(|metadata| metadata.scope.as_str())
    }

    fn concurrency_metadata_and_message(&self) -> Option<(ProxyConcurrencyMetadata, &str)> {
        let encoded = self
            .message_without_retry_metadata()
            .strip_prefix(Self::CONCURRENCY_PREFIX)?;
        let (metadata, message) = encoded.split_once("] ")?;
        let mut parts = metadata.split(':');
        let scope = ProxyConcurrencyScope::from_str(parts.next()?)?;
        let current = parts.next()?.parse().ok()?;
        let limit = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some((
            ProxyConcurrencyMetadata {
                scope,
                current,
                limit,
            },
            message,
        ))
    }

    fn message_without_retry_metadata(&self) -> &str {
        self.message
            .strip_prefix(Self::RETRY_AFTER_PREFIX)
            .and_then(|encoded| encoded.split_once(']').map(|(_, message)| message))
            .unwrap_or(&self.message)
    }

    pub fn error_code(&self) -> &'static str {
        let message = self.message_without_retry_metadata();
        if let Some(metadata) = self.concurrency_metadata() {
            return metadata.scope.error_code();
        }
        if message.starts_with(Self::USER_IDENTITY_REQUIRED_PREFIX) {
            return "cc_switch_user_identity_required";
        }
        if message.starts_with(Self::CAPACITY_SHED_PREFIX) {
            return "cc_switch_upstream_capacity_shed";
        }
        if message.starts_with(Self::PROTOCOL_INCOMPATIBLE_PREFIX) {
            return "cc_switch_protocol_incompatible";
        }
        if message.starts_with(Self::RESPONSE_CONTEXT_UNAVAILABLE_PREFIX) {
            return "response_context_unavailable";
        }
        if message.starts_with(Self::TOOL_JSON_INVALID_PREFIX) {
            return "TOOL_JSON_INVALID";
        }
        if message.starts_with(Self::TOOL_JSON_INCOMPLETE_PREFIX) {
            return "TOOL_JSON_INCOMPLETE";
        }
        if message.starts_with(Self::TOOL_JSON_LIMIT_PREFIX) {
            return "TOOL_JSON_LIMIT";
        }
        if message.starts_with(Self::KIRO_EVENT_STREAM_INVALID_PREFIX) {
            return "KIRO_EVENT_STREAM_INVALID";
        }
        if message.starts_with(Self::KIRO_EVENT_STREAM_LIMIT_PREFIX) {
            return "KIRO_EVENT_STREAM_LIMIT";
        }
        if message.starts_with(Self::KIRO_EVENT_STREAM_TIMEOUT_PREFIX) {
            return "KIRO_EVENT_STREAM_TIMEOUT";
        }
        if message.starts_with(Self::KIRO_UPSTREAM_STREAM_ERROR_PREFIX) {
            return "KIRO_UPSTREAM_STREAM_ERROR";
        }
        if message.starts_with(Self::CURSOR_SESSION_LOST_PREFIX) {
            return "cursor_session_lost";
        }
        if message.starts_with(Self::CURSOR_RESPONSE_STATE_LOST_PREFIX) {
            return "cursor_response_state_lost";
        }
        if message.starts_with(Self::CURSOR_CONVERSATION_BUSY_PREFIX) {
            return "cursor_conversation_busy";
        }
        match self.status {
            axum::http::StatusCode::BAD_REQUEST => "cc_switch_invalid_request",
            axum::http::StatusCode::UNAUTHORIZED => "cc_switch_auth_error",
            axum::http::StatusCode::FORBIDDEN => "cc_switch_forbidden",
            axum::http::StatusCode::NOT_FOUND => "cc_switch_not_found",
            axum::http::StatusCode::CONFLICT => "cc_switch_conflict",
            axum::http::StatusCode::TOO_MANY_REQUESTS => "cc_switch_rate_limited",
            axum::http::StatusCode::UNPROCESSABLE_ENTITY => "cc_switch_transform_error",
            axum::http::StatusCode::GATEWAY_TIMEOUT => "cc_switch_timeout",
            axum::http::StatusCode::BAD_GATEWAY => "cc_switch_forward_failed",
            axum::http::StatusCode::SERVICE_UNAVAILABLE => "cc_switch_no_available_provider",
            axum::http::StatusCode::INTERNAL_SERVER_ERROR => "cc_switch_internal_error",
            status if status.is_server_error() => "cc_switch_proxy_error",
            status if status.is_client_error() => "cc_switch_invalid_request",
            _ => "cc_switch_proxy_error",
        }
    }

    pub fn error_type(&self) -> &'static str {
        let message = self.message_without_retry_metadata();
        if self.concurrency_metadata().is_some() {
            return "concurrency_limit_error";
        }
        if message.starts_with(Self::PROTOCOL_INCOMPATIBLE_PREFIX) {
            return "invalid_request_error";
        }
        if message.starts_with(Self::RESPONSE_CONTEXT_UNAVAILABLE_PREFIX) {
            return "invalid_request_error";
        }
        if message.starts_with(Self::TOOL_JSON_INVALID_PREFIX)
            || message.starts_with(Self::TOOL_JSON_INCOMPLETE_PREFIX)
            || message.starts_with(Self::TOOL_JSON_LIMIT_PREFIX)
        {
            return "upstream_tool_json_error";
        }
        if message.starts_with(Self::KIRO_EVENT_STREAM_INVALID_PREFIX)
            || message.starts_with(Self::KIRO_EVENT_STREAM_LIMIT_PREFIX)
            || message.starts_with(Self::KIRO_EVENT_STREAM_TIMEOUT_PREFIX)
            || message.starts_with(Self::KIRO_UPSTREAM_STREAM_ERROR_PREFIX)
        {
            return "upstream_protocol_error";
        }
        match self.status {
            axum::http::StatusCode::BAD_REQUEST => "invalid_request_error",
            axum::http::StatusCode::UNAUTHORIZED => "authentication_error",
            axum::http::StatusCode::FORBIDDEN => "permission_error",
            axum::http::StatusCode::NOT_FOUND => "not_found_error",
            axum::http::StatusCode::CONFLICT => "conflict_error",
            axum::http::StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
            axum::http::StatusCode::UNPROCESSABLE_ENTITY => "invalid_request_error",
            axum::http::StatusCode::GATEWAY_TIMEOUT => "timeout_error",
            axum::http::StatusCode::BAD_GATEWAY => "upstream_error",
            axum::http::StatusCode::SERVICE_UNAVAILABLE => "unavailable_error",
            status if status.is_server_error() => "proxy_error",
            _ => "proxy_error",
        }
    }

    pub fn retryable(&self) -> bool {
        let message = self.message_without_retry_metadata();
        if self.concurrency_metadata().is_some()
            || message.starts_with(Self::USER_IDENTITY_REQUIRED_PREFIX)
            || message.starts_with(Self::PROTOCOL_INCOMPATIBLE_PREFIX)
            || message.starts_with(Self::RESPONSE_CONTEXT_UNAVAILABLE_PREFIX)
        {
            return false;
        }
        if message.starts_with(Self::TOOL_JSON_INVALID_PREFIX)
            || message.starts_with(Self::TOOL_JSON_INCOMPLETE_PREFIX)
            || message.starts_with(Self::TOOL_JSON_LIMIT_PREFIX)
        {
            return false;
        }
        matches!(
            self.status,
            axum::http::StatusCode::TOO_MANY_REQUESTS
                | axum::http::StatusCode::BAD_GATEWAY
                | axum::http::StatusCode::SERVICE_UNAVAILABLE
                | axum::http::StatusCode::GATEWAY_TIMEOUT
        )
    }

    pub fn error_param(&self) -> Option<&'static str> {
        let message = self.message_without_retry_metadata();
        if message.starts_with(Self::RESPONSE_CONTEXT_UNAVAILABLE_PREFIX)
            || message.starts_with(Self::CURSOR_RESPONSE_STATE_LOST_PREFIX)
        {
            return Some("previous_response_id");
        }
        for (prefix, parameter) in [
            ("unsupported parameter `n`", "n"),
            ("unsupported parameter `logprobs`", "logprobs"),
            ("unsupported parameter `modalities`", "modalities"),
            ("unsupported parameter `audio`", "audio"),
            ("unsupported parameter `functions`", "functions"),
            ("unsupported parameter `background`", "background"),
            ("unsupported parameter `candidateCount`", "candidateCount"),
            ("unsupported parameter `stream`", "stream"),
            ("unsupported parameter `tools`", "tools"),
        ] {
            if message.starts_with(prefix) {
                return Some(parameter);
            }
        }
        None
    }
}

pub(super) fn setting(
    provider: &crate::domain::providers::model::Provider,
    keys: &[&str],
) -> Option<String> {
    for key in keys {
        if let Some(value) = provider
            .settings_config
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str)
            .or_else(|| provider.settings_config.get(*key).and_then(Value::as_str))
        {
            if !value.trim().is_empty() {
                return Some(value.trim().to_string());
            }
        }
    }
    None
}

/// Resolve Codex provider API key from env, auth.json, and config.toml shapes.
pub(super) fn codex_provider_api_key(
    provider: &crate::domain::providers::model::Provider,
) -> Option<String> {
    if let Some(key) = setting(
        provider,
        &[
            "OPENAI_API_KEY",
            "XAI_API_KEY",
            "GROK_API_KEY",
            "CODEX_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_API_KEY",
            "GEMINI_API_KEY",
            "GOOGLE_API_KEY",
            "API_KEY",
        ],
    ) {
        return Some(key);
    }

    if let Some(auth) = provider.settings_config.get("auth") {
        if let Some(key) = auth
            .get("OPENAI_API_KEY")
            .or_else(|| auth.get("openai_api_key"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(key.to_string());
        }
    }

    let config_text = provider
        .settings_config
        .get("config")
        .and_then(Value::as_str)
        .unwrap_or_default();
    for line in config_text.lines() {
        let line = line.split('#').next().unwrap_or(line).trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key != "api_key" && key != "experimental_bearer_token" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }

    None
}

pub(super) fn join_url(base_url: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use serde_json::{json, Value};

    use crate::domain::accounts::store::AccountStore;
    use crate::domain::providers::model::{AppKind, Provider, ProviderMeta, ProviderType};
    use crate::domain::providers::store::StoredProvider;
    use crate::proxy::adapters::ProviderAdapter;

    use super::*;

    #[test]
    fn extracts_codex_base_url_from_toml_config() {
        let provider = Provider {
            id: "p1".to_string(),
            name: "codex".to_string(),
            settings_config: json!({
                "config": "[model_providers.custom]\nbase_url = \"https://example.com/v1\"\n"
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        };

        assert_eq!(
            adapters::codex_config_base_url(&provider).as_deref(),
            Some("https://example.com/v1")
        );
    }

    #[test]
    fn extracts_codex_api_key_from_auth_json() {
        let provider = Provider {
            id: "p1".to_string(),
            name: "default".to_string(),
            settings_config: json!({
                "auth": { "OPENAI_API_KEY": "sk-custom-key" },
                "config": "base_url = \"https://relay.example/v1\"\n"
            }),
            category: Some("custom".to_string()),
            meta: None,
            extra: Default::default(),
        };

        assert_eq!(
            codex_provider_api_key(&provider).as_deref(),
            Some("sk-custom-key")
        );
    }

    #[test]
    fn builds_auth_header_from_bound_account() {
        let stored = StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "codex oauth".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some("codex_oauth".to_string()),
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("managed_account".to_string()),
                        auth_provider: Some("codex_oauth".to_string()),
                        account_id: Some("a1".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
            resource: Default::default(),
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(crate::domain::accounts::store::UpsertAccountInput {
            id: Some("a1".to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: None,
            access_token: Some("token".to_string()),
            refresh_token: None,
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: Some(json!({
                "verifiedOpenAiClaims": {"chatgpt_account_id":"acct_123"}
            })),
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota: None,
            quota_percent: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
        });

        let adapter = adapters::adapter_for(AppKind::Codex, ProviderType::CodexOAuth);
        let headers = adapter
            .build_headers(AppKind::Codex, &stored, &accounts)
            .unwrap();
        assert_eq!(
            headers,
            vec![
                ("authorization", "Bearer token".to_string()),
                ("chatgpt-account-id", "acct_123".to_string()),
                ("originator", "codex_cli_rs".to_string()),
                ("version", "0.144.1".to_string()),
            ]
        );
    }

    #[test]
    fn applies_single_model_mapping() {
        let provider = Provider {
            id: "p1".to_string(),
            name: "mapped".to_string(),
            settings_config: json!({
                "modelMapping": {
                    "mode": "single",
                    "upstreamModel": "glm-5.2"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        };

        let stored = StoredProvider {
            app: AppKind::Claude,
            provider,
            provider_type: ProviderType::Claude,
            provider_type_id: "claude".to_string(),
            resource: Default::default(),
        };
        let adapter = adapters::adapter_for(AppKind::Claude, ProviderType::Claude);
        let request = adapter
            .transform_request(
                Bytes::from_static(br#"{"model":"gpt-5.5","messages":[]}"#),
                &stored,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&request.body).unwrap();

        assert_eq!(request.model.as_deref(), Some("glm-5.2"));
        assert_eq!(value.get("model").and_then(Value::as_str), Some("glm-5.2"));
    }

    #[test]
    fn cursor_state_conflicts_have_stable_public_codes() {
        let lost = ProxyError::cursor_response_state_lost("expired");
        assert_eq!(lost.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(lost.error_code(), "cursor_response_state_lost");
        assert_eq!(lost.client_message(), "expired");

        let busy = ProxyError::cursor_conversation_busy("running");
        assert_eq!(busy.error_code(), "cursor_conversation_busy");
        assert_eq!(busy.client_message(), "running");

        let unsupported = ProxyError::bad_request(
            "unsupported parameter `background`: Cursor responses run synchronously",
        );
        assert_eq!(unsupported.error_param(), Some("background"));
    }
}
