#![allow(dead_code)]

pub mod adapters;
mod cache_injector;
mod copilot_model_map;
mod copilot_optimizer;
pub(crate) mod cursor;
mod deepseek;
mod forwarder;
mod kiro;
mod request_governance;
mod router;
mod streaming;
mod thinking;
mod transforms;
mod usage;

use serde_json::Value;

pub use forwarder::forward;
pub use router::ProxyRoute;

pub fn capabilities() -> Vec<adapters::AdapterCapability> {
    adapters::all_capabilities()
}

#[derive(Debug)]
pub struct ProxyError {
    pub status: axum::http::StatusCode,
    pub message: String,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ProxyError {}

impl ProxyError {
    pub(super) fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub(super) fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub(super) fn conflict(message: impl Into<String>) -> Self {
        Self {
            status: axum::http::StatusCode::CONFLICT,
            message: message.into(),
        }
    }

    pub(super) fn bad_gateway(error: impl std::fmt::Display) -> Self {
        Self {
            status: axum::http::StatusCode::BAD_GATEWAY,
            message: format!("proxy upstream request failed: {error}"),
        }
    }

    pub fn error_code(&self) -> &'static str {
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
        matches!(
            self.status,
            axum::http::StatusCode::TOO_MANY_REQUESTS
                | axum::http::StatusCode::BAD_GATEWAY
                | axum::http::StatusCode::SERVICE_UNAVAILABLE
                | axum::http::StatusCode::GATEWAY_TIMEOUT
        )
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
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
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
            scopes: Vec::new(),
            profile: Some(json!({"chatgpt_account_id":"acct_123"})),
            raw: None,
            subscription_level: None,
            quota: None,
            quota_percent: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
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
                ("chatgpt-account-id", "acct_123".to_string())
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
}
