use axum::http::HeaderMap;

use crate::domain::accounts::store::AccountStore;
use crate::domain::failover::{current_time_ms, FailoverStore};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::{ProviderStore, StoredProvider};

use super::ProxyError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyRoute {
    ClaudeMessages,
    CodexChatCompletions,
    CodexResponses,
    CodexResponsesCompact,
    Gemini,
}

impl ProxyRoute {
    pub fn app(self) -> AppKind {
        match self {
            Self::ClaudeMessages => AppKind::Claude,
            Self::CodexChatCompletions | Self::CodexResponses | Self::CodexResponsesCompact => {
                AppKind::Codex
            }
            Self::Gemini => AppKind::Gemini,
        }
    }

    pub fn path(self, gemini_path: Option<String>) -> String {
        match self {
            Self::ClaudeMessages => "/v1/messages".to_string(),
            Self::CodexChatCompletions => "/v1/chat/completions".to_string(),
            Self::CodexResponses => "/v1/responses".to_string(),
            Self::CodexResponsesCompact => "/v1/responses/compact".to_string(),
            Self::Gemini => format!("/v1beta/{}", gemini_path.unwrap_or_default()),
        }
    }
}

#[derive(Debug)]
pub(super) struct ProviderRouteSelection {
    pub provider: StoredProvider,
    pub failover_state_changed: bool,
}

pub(super) fn select_provider(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(store, accounts, failover, app, headers, None)
}

pub(super) fn select_provider_for_type(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    provider_type: ProviderType,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(store, accounts, failover, app, headers, Some(provider_type))
}

pub(super) fn select_provider_for_codex_image_generation(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    headers: &HeaderMap,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_filter(
        store,
        accounts,
        failover,
        AppKind::Codex,
        headers,
        codex_image_generation_provider,
    )
}

fn select_provider_inner(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    provider_type_filter: Option<ProviderType>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        failover,
        app,
        headers,
        provider_type_filter,
        None,
    )
}

fn select_provider_with_filter(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    filter: fn(&StoredProvider) -> bool,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        failover,
        app,
        headers,
        None,
        Some(filter),
    )
}

fn select_provider_with_optional_filter(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    provider_type_filter: Option<ProviderType>,
    provider_filter: Option<fn(&StoredProvider) -> bool>,
) -> Result<ProviderRouteSelection, ProxyError> {
    let now = current_time_ms();
    let provider_id = headers
        .get("x-cc-provider-id")
        .and_then(|value| value.to_str().ok());

    if let Some(provider_id) = provider_id {
        let provider = store
            .providers
            .iter()
            .find(|item| {
                item.app == app
                    && item.provider.id == provider_id
                    && provider_type_filter
                        .map(|provider_type| item.provider_type == provider_type)
                        .unwrap_or(true)
                    && provider_filter.map(|filter| filter(item)).unwrap_or(true)
            })
            .cloned()
            .ok_or_else(|| ProxyError::not_found(format!("provider not found: {provider_id}")))?;
        ensure_provider_not_rate_limited(&provider, accounts, now)?;
        return Ok(ProviderRouteSelection {
            provider,
            failover_state_changed: false,
        });
    }

    let candidates = store
        .providers
        .iter()
        .filter(|item| item.app == app)
        .filter(|item| {
            provider_type_filter
                .map(|provider_type| item.provider_type == provider_type)
                .unwrap_or(true)
                && provider_filter.map(|filter| filter(item)).unwrap_or(true)
        })
        .filter(|item| provider_rate_limited_until(item, accounts, now).is_none())
        .collect::<Vec<_>>();
    let has_matching_provider = store.providers.iter().any(|item| {
        item.app == app
            && provider_type_filter
                .map(|provider_type| item.provider_type == provider_type)
                .unwrap_or(true)
            && provider_filter.map(|filter| filter(item)).unwrap_or(true)
    });
    if candidates.is_empty() && has_matching_provider {
        let until = store
            .providers
            .iter()
            .filter(|item| item.app == app)
            .filter(|item| {
                provider_type_filter
                    .map(|provider_type| item.provider_type == provider_type)
                    .unwrap_or(true)
                    && provider_filter.map(|filter| filter(item)).unwrap_or(true)
            })
            .filter_map(|item| provider_rate_limited_until(item, accounts, now))
            .min()
            .unwrap_or(0);
        if until == 0 {
            return Err(ProxyError::not_found(format!(
                "no provider configured for {:?}",
                app
            )));
        }
        let label = provider_type_filter
            .map(|provider_type| provider_type.as_str().replace('_', " "))
            .unwrap_or_else(|| {
                if store
                    .providers
                    .iter()
                    .filter(|item| item.app == app)
                    .all(|item| item.provider_type == ProviderType::GrokOAuth)
                {
                    "grok oauth".to_string()
                } else {
                    "codex oauth".to_string()
                }
            });
        return Err(ProxyError {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            message: format!("all {label} accounts are rate limited until {until}"),
        });
    }
    if candidates.is_empty() {
        return Err(ProxyError::not_found(format!(
            "no provider configured for {:?}",
            app
        )));
    }

    let selection = failover
        .select_provider(app, &candidates, now)
        .ok_or_else(|| ProxyError::not_found(format!("no provider configured for {:?}", app)))?;
    Ok(ProviderRouteSelection {
        provider: selection.provider.clone(),
        failover_state_changed: selection.state_changed,
    })
}

fn codex_image_generation_provider(provider: &StoredProvider) -> bool {
    match provider.provider_type {
        ProviderType::GrokOAuth => true,
        ProviderType::CodexOAuth => provider
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.codex_image_generation_enabled)
            .unwrap_or(false),
        _ => false,
    }
}

fn ensure_provider_not_rate_limited(
    provider: &StoredProvider,
    accounts: &AccountStore,
    now_ms: u128,
) -> Result<(), ProxyError> {
    if let Some(until) = provider_rate_limited_until(provider, accounts, now_ms) {
        return Err(ProxyError {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            message: format!(
                "provider {} account is rate limited until {until}",
                provider.provider.id
            ),
        });
    }
    Ok(())
}

fn provider_rate_limited_until(
    provider: &StoredProvider,
    accounts: &AccountStore,
    now_ms: u128,
) -> Option<i64> {
    if !matches!(
        provider.provider_type,
        ProviderType::CodexOAuth
            | ProviderType::GrokOAuth
            | ProviderType::CursorOAuth
            | ProviderType::CursorApiKey
    ) {
        return None;
    }
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    accounts
        .find_for_provider(provider.provider_type, account_id)
        .and_then(|account| account.rate_limited_until)
        .filter(|until| (*until as u128) > now_ms)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::UpsertAccountInput;
    use crate::domain::failover::{FailoverStore, ProviderOutcome, UpdateFailoverAppInput};
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta, ProviderType};

    fn provider(app: AppKind, id: &str) -> StoredProvider {
        StoredProvider {
            app,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
        }
    }

    fn codex_oauth_provider(id: &str, account_id: &str) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some("codex_oauth".to_string()),
                        account_id: Some(account_id.to_string()),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
        }
    }

    fn codex_oauth_account(id: &str, rate_limited_until: Option<i64>) -> UpsertAccountInput {
        UpsertAccountInput {
            id: Some(id.to_string()),
            provider_type: ProviderType::CodexOAuth,
            email: None,
            access_token: Some(format!("token-{id}")),
            refresh_token: Some(format!("refresh-{id}")),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            scopes: Vec::new(),
            profile: Some(json!({"chatgpt_account_id": id})),
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until,
            last_refresh_error: None,
        }
    }

    fn cursor_oauth_provider(id: &str, account_id: &str) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some("cursor_oauth".to_string()),
                        account_id: Some(account_id.to_string()),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: "cursor_oauth".to_string(),
        }
    }

    fn cursor_oauth_account(id: &str, rate_limited_until: Option<i64>) -> UpsertAccountInput {
        UpsertAccountInput {
            id: Some(id.to_string()),
            provider_type: ProviderType::CursorOAuth,
            email: None,
            access_token: Some(format!("token-{id}")),
            refresh_token: Some(format!("refresh-{id}")),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            scopes: Vec::new(),
            profile: Some(json!({"accountId": id})),
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until,
            last_refresh_error: None,
        }
    }

    fn provider_store() -> ProviderStore {
        ProviderStore {
            providers: vec![
                provider(AppKind::Codex, "p1"),
                provider(AppKind::Codex, "p2"),
            ],
        }
    }

    fn enabled_failover(store: &ProviderStore) -> FailoverStore {
        let mut failover = FailoverStore::default();
        failover.update_app_config(
            AppKind::Codex,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p1".to_string(), "p2".to_string()]),
                failure_threshold: Some(1),
                open_duration_ms: Some(1_000),
                half_open_max_probes: Some(1),
            },
            store,
        );
        failover
    }

    #[test]
    fn explicit_provider_header_bypasses_failover_selection() {
        let store = provider_store();
        let mut failover = enabled_failover(&store);
        failover.record_outcome(AppKind::Codex, "p1", ProviderOutcome::NetworkFailure, 100);
        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p1"));

        let accounts = AccountStore::default();
        let selected =
            select_provider(&store, &accounts, &mut failover, AppKind::Codex, &headers).unwrap();

        assert_eq!(selected.provider.provider.id, "p1");
        assert!(!selected.failover_state_changed);
    }

    #[test]
    fn automatic_selection_skips_open_breaker() {
        let store = provider_store();
        let mut failover = enabled_failover(&store);
        let now = current_time_ms();
        failover.record_outcome(AppKind::Codex, "p1", ProviderOutcome::NetworkFailure, now);

        let selected = select_provider(
            &store,
            &AccountStore::default(),
            &mut failover,
            AppKind::Codex,
            &HeaderMap::new(),
        )
        .unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
        assert!(!selected.failover_state_changed);
    }

    #[test]
    fn automatic_selection_skips_rate_limited_codex_oauth_account() {
        let now = current_time_ms() as i64;
        let store = ProviderStore {
            providers: vec![
                codex_oauth_provider("p1", "acct-1"),
                codex_oauth_provider("p2", "acct-2"),
            ],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", Some(now + 60_000)));
        accounts.upsert(codex_oauth_account("acct-2", None));
        let mut failover = enabled_failover(&store);

        let selected = select_provider(
            &store,
            &accounts,
            &mut failover,
            AppKind::Codex,
            &HeaderMap::new(),
        )
        .unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
    }

    #[test]
    fn automatic_selection_skips_rate_limited_cursor_oauth_account() {
        let now = current_time_ms() as i64;
        let store = ProviderStore {
            providers: vec![
                cursor_oauth_provider("p1", "cursor-acct-1"),
                cursor_oauth_provider("p2", "cursor-acct-2"),
            ],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(cursor_oauth_account("cursor-acct-1", Some(now + 60_000)));
        accounts.upsert(cursor_oauth_account("cursor-acct-2", None));
        let mut failover = enabled_failover(&store);

        let selected = select_provider(
            &store,
            &accounts,
            &mut failover,
            AppKind::Codex,
            &HeaderMap::new(),
        )
        .unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
    }

    #[test]
    fn explicit_rate_limited_codex_oauth_provider_returns_429() {
        let now = current_time_ms() as i64;
        let store = ProviderStore {
            providers: vec![codex_oauth_provider("p1", "acct-1")],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", Some(now + 60_000)));
        let mut failover = enabled_failover(&store);
        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p1"));

        let error = select_provider(&store, &accounts, &mut failover, AppKind::Codex, &headers)
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }
}
