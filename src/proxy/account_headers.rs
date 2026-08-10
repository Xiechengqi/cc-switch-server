use axum::http::{HeaderName, HeaderValue};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::ProviderType;
use crate::domain::providers::runtime::authoritative_managed_account;
use crate::domain::providers::store::StoredProvider;

use super::ProxyError;

pub(super) fn apply_account_header_overrides(
    headers: &mut Vec<(String, String)>,
    stored: &StoredProvider,
    accounts: &AccountStore,
) -> Result<(), ProxyError> {
    let Some(account) = authoritative_managed_account(stored, accounts) else {
        return Ok(());
    };
    for (name, value) in &account.extra_headers {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            ProxyError::bad_request(format!(
                "account {} extra header name is invalid: {name}",
                account.id
            ))
        })?;
        let normalized_name = header_name.as_str();
        if account_header_override_blocked(normalized_name, stored.provider_type) {
            return Err(ProxyError::bad_request(format!(
                "account {} extra header cannot override proxy-controlled header: {normalized_name}",
                account.id
            )));
        }
        HeaderValue::from_str(value).map_err(|_| {
            ProxyError::bad_request(format!(
                "account {} extra header value is invalid for {normalized_name}",
                account.id
            ))
        })?;
        headers.retain(|(existing, _)| !existing.eq_ignore_ascii_case(normalized_name));
        headers.push((normalized_name.to_string(), value.clone()));
    }
    Ok(())
}

pub(super) fn account_header_override_blocked(name: &str, provider_type: ProviderType) -> bool {
    let normalized = name.to_ascii_lowercase();
    if provider_type == ProviderType::ClaudeOAuth
        && (matches!(
            normalized.as_str(),
            "anthropic-beta"
                | "anthropic-version"
                | "x-app"
                | "sec-fetch-mode"
                | "anthropic-dangerous-direct-browser-access"
                | "x-claude-code-session-id"
        ) || normalized.starts_with("x-stainless-"))
    {
        return true;
    }
    if provider_type == ProviderType::GrokOAuth
        && matches!(
            normalized.as_str(),
            "x-xai-token-auth"
                | "x-grok-client-identifier"
                | "x-grok-client-version"
                | "x-grok-client-surface"
                | "x-authenticateresponse"
                | "x-grok-conv-id"
                | "x-grok-cache-identity"
                | "x-grok-turn-idx"
        )
    {
        return true;
    }
    if provider_type == ProviderType::KimiCode
        && matches!(
            normalized.as_str(),
            "x-msh-platform"
                | "x-msh-version"
                | "x-msh-device-name"
                | "x-msh-device-model"
                | "x-msh-os-version"
                | "x-msh-device-id"
        )
    {
        return true;
    }
    if provider_type == ProviderType::GitHubCopilot
        && matches!(
            normalized.as_str(),
            "editor-version"
                | "editor-plugin-version"
                | "copilot-integration-id"
                | "x-github-api-version"
                | "openai-intent"
                | "x-initiator"
                | "x-vscode-user-agent-library-version"
                | "x-interaction-type"
                | "x-request-id"
                | "x-agent-task-id"
                | "x-interaction-id"
        )
    {
        return true;
    }
    matches!(
        normalized.as_str(),
        "authorization"
            | "proxy-authorization"
            | "host"
            | "content-length"
            | "content-type"
            | "accept"
            | "connection"
            | "keep-alive"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
            | "cookie"
            | "set-cookie"
            | "user-agent"
            | "originator"
            | "version"
            | "chatgpt-account-id"
            | "session_id"
            | "x-client-request-id"
            | "x-codex-window-id"
            | "openai-beta"
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn copilot_account_headers_cannot_override_protocol_identity() {
        for name in [
            "editor-version",
            "editor-plugin-version",
            "copilot-integration-id",
            "x-github-api-version",
            "openai-intent",
            "x-initiator",
            "x-vscode-user-agent-library-version",
            "x-interaction-type",
            "x-request-id",
            "x-agent-task-id",
            "x-interaction-id",
        ] {
            assert!(account_header_override_blocked(
                name,
                ProviderType::GitHubCopilot
            ));
        }
        assert!(!account_header_override_blocked(
            "x-enterprise-sso",
            ProviderType::GitHubCopilot
        ));
    }

    #[test]
    fn kimi_account_headers_cannot_override_account_device_identity() {
        for name in [
            "x-msh-platform",
            "x-msh-version",
            "x-msh-device-name",
            "x-msh-device-model",
            "x-msh-os-version",
            "x-msh-device-id",
            "user-agent",
        ] {
            assert!(account_header_override_blocked(
                name,
                ProviderType::KimiCode
            ));
        }
    }

    #[test]
    fn provider_owned_credentials_ignore_stale_account_header_binding() {
        let stored = StoredProvider {
            app: crate::domain::providers::model::AppKind::Codex,
            provider: crate::domain::providers::model::Provider {
                id: "cursor-static".to_string(),
                name: "Cursor static".to_string(),
                settings_config: json!({"apiKey": "provider-key"}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("legacy".to_string()),
                        auth_provider: Some("cursor_apikey".to_string()),
                        account_id: Some("legacy-cursor-key".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorApiKey,
            provider_type_id: ProviderType::CursorApiKey.as_str().to_string(),
            resource: Default::default(),
        };
        let account = serde_json::from_value(json!({
            "id": "legacy-cursor-key",
            "providerType": "cursor_apikey",
            "apiKey": "account-key",
            "extraHeaders": {"x-legacy-account": "must-not-leak"}
        }))
        .unwrap();
        let accounts = AccountStore {
            accounts: vec![account],
            ..Default::default()
        };
        let mut headers = Vec::new();

        apply_account_header_overrides(&mut headers, &stored, &accounts).unwrap();

        assert!(headers.is_empty());
    }
}
