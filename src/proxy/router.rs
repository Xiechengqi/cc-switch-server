use axum::http::HeaderMap;
use std::collections::BTreeSet;

use crate::domain::accounts::store::AccountStore;
use crate::domain::failover::{current_time_ms, FailoverStore};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::state::AccountInFlightSnapshot;

use super::ProxyError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyRoute {
    ClaudeMessages,
    ClaudeCountTokens,
    CodexChatCompletions,
    CodexResponses,
    CodexResponsesCompact,
    Gemini,
}

impl ProxyRoute {
    pub fn app(self) -> AppKind {
        match self {
            Self::ClaudeMessages | Self::ClaudeCountTokens => AppKind::Claude,
            Self::CodexChatCompletions | Self::CodexResponses | Self::CodexResponsesCompact => {
                AppKind::Codex
            }
            Self::Gemini => AppKind::Gemini,
        }
    }

    pub fn path(self, gemini_path: Option<String>) -> String {
        match self {
            Self::ClaudeMessages => "/v1/messages".to_string(),
            Self::ClaudeCountTokens => "/v1/messages/count_tokens".to_string(),
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

#[derive(Debug, Clone)]
pub(super) struct AccountConcurrencySelection {
    pub provider_type: ProviderType,
    pub account_id: String,
    pub max_concurrent: u32,
    pub current: u32,
}

#[derive(Default)]
struct ProviderSelectionOptions<'a> {
    provider_type_filter: Option<ProviderType>,
    provider_filter: Option<fn(&StoredProvider) -> bool>,
    account_in_flight: Option<&'a AccountInFlightSnapshot>,
    excluded_provider_ids: Option<&'a BTreeSet<String>>,
}

const DEFAULT_CLAUDE_ACCOUNT_MAX_CONCURRENT: u32 = 8;

pub(super) fn select_provider(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(store, accounts, failover, app, headers, None, None)
}

pub(super) fn select_provider_with_account_inflight(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    account_in_flight: &AccountInFlightSnapshot,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(
        store,
        accounts,
        failover,
        app,
        headers,
        None,
        Some(account_in_flight),
    )
}

pub(super) fn select_provider_with_account_inflight_excluding(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    account_in_flight: &AccountInFlightSnapshot,
    excluded_provider_ids: &BTreeSet<String>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        failover,
        app,
        headers,
        ProviderSelectionOptions {
            account_in_flight: Some(account_in_flight),
            excluded_provider_ids: Some(excluded_provider_ids),
            ..ProviderSelectionOptions::default()
        },
    )
}

pub(super) fn select_provider_for_claude_count_tokens(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    headers: &HeaderMap,
    account_in_flight: &AccountInFlightSnapshot,
    excluded_provider_ids: &BTreeSet<String>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        failover,
        AppKind::Claude,
        headers,
        ProviderSelectionOptions {
            provider_filter: Some(provider_supports_claude_count_tokens),
            account_in_flight: Some(account_in_flight),
            excluded_provider_ids: Some(excluded_provider_ids),
            ..ProviderSelectionOptions::default()
        },
    )
}

pub(super) fn provider_supports_claude_count_tokens(provider: &StoredProvider) -> bool {
    provider.app == AppKind::Claude
        && matches!(
            provider.provider_type,
            ProviderType::Claude | ProviderType::ClaudeAuth | ProviderType::ClaudeOAuth
        )
}

pub(super) fn select_provider_for_type(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    provider_type: ProviderType,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(
        store,
        accounts,
        failover,
        app,
        headers,
        Some(provider_type),
        None,
    )
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
        None,
    )
}

fn select_provider_inner(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    provider_type_filter: Option<ProviderType>,
    account_in_flight: Option<&AccountInFlightSnapshot>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        failover,
        app,
        headers,
        ProviderSelectionOptions {
            provider_type_filter,
            account_in_flight,
            ..ProviderSelectionOptions::default()
        },
    )
}

fn select_provider_with_filter(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    filter: fn(&StoredProvider) -> bool,
    account_in_flight: Option<&AccountInFlightSnapshot>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        failover,
        app,
        headers,
        ProviderSelectionOptions {
            provider_filter: Some(filter),
            account_in_flight,
            ..ProviderSelectionOptions::default()
        },
    )
}

fn select_provider_with_optional_filter(
    store: &ProviderStore,
    accounts: &AccountStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
    options: ProviderSelectionOptions<'_>,
) -> Result<ProviderRouteSelection, ProxyError> {
    let ProviderSelectionOptions {
        provider_type_filter,
        provider_filter,
        account_in_flight,
        excluded_provider_ids,
    } = options;
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
        ensure_provider_account_does_not_need_relogin(&provider, accounts)?;
        ensure_provider_not_rate_limited(&provider, accounts, now)?;
        if account_in_flight
            .and_then(|snapshot| account_concurrency_for_provider(&provider, accounts, snapshot))
            .is_some_and(|selection| selection.current >= selection.max_concurrent)
        {
            return Err(account_concurrency_limit_error(&provider));
        }
        return Ok(ProviderRouteSelection {
            provider,
            failover_state_changed: false,
        });
    }

    let matching_candidates = store
        .providers
        .iter()
        .filter(|item| item.app == app)
        .filter(|item| {
            provider_type_filter
                .map(|provider_type| item.provider_type == provider_type)
                .unwrap_or(true)
                && provider_filter.map(|filter| filter(item)).unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let active_candidates = matching_candidates
        .iter()
        .copied()
        .filter(|item| !provider_account_needs_relogin(item, accounts))
        .collect::<Vec<_>>();
    if active_candidates.is_empty() && !matching_candidates.is_empty() {
        return Err(ProxyError {
            status: axum::http::StatusCode::UNAUTHORIZED,
            message: "all managed accounts require login".to_string(),
        });
    }
    let rate_available_candidates = active_candidates
        .iter()
        .copied()
        .filter(|item| provider_rate_limited_until(item, accounts, now).is_none())
        .collect::<Vec<_>>();
    let available_candidates = rate_available_candidates
        .iter()
        .copied()
        .filter(|provider| {
            account_in_flight
                .and_then(|snapshot| account_concurrency_for_provider(provider, accounts, snapshot))
                .is_none_or(|selection| selection.current < selection.max_concurrent)
        })
        .collect::<Vec<_>>();
    let non_excluded_candidates = available_candidates
        .iter()
        .copied()
        .filter(|provider| {
            excluded_provider_ids.is_none_or(|excluded| !excluded.contains(&provider.provider.id))
        })
        .collect::<Vec<_>>();
    // If every available provider has already failed in this logical request,
    // fall back to the normal candidate set and let the retry budget stop loops.
    let candidates = if non_excluded_candidates.is_empty() {
        available_candidates
    } else {
        non_excluded_candidates
    };
    let has_matching_provider = !matching_candidates.is_empty();
    if candidates.is_empty() && !rate_available_candidates.is_empty() {
        return Err(ProxyError {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            message: "all Claude OAuth accounts are at their concurrency limit".to_string(),
        });
    }
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
        .select_provider_with_load(app, &candidates, now, |provider| {
            account_in_flight
                .and_then(|snapshot| account_concurrency_for_provider(provider, accounts, snapshot))
                .map(account_concurrency_load_score)
                .unwrap_or_default()
        })
        .ok_or_else(|| ProxyError::not_found(format!("no provider configured for {:?}", app)))?;
    Ok(ProviderRouteSelection {
        provider: selection.provider.clone(),
        failover_state_changed: selection.state_changed,
    })
}

pub(super) fn account_concurrency_for_provider(
    provider: &StoredProvider,
    accounts: &AccountStore,
    snapshot: &AccountInFlightSnapshot,
) -> Option<AccountConcurrencySelection> {
    if provider.provider_type != ProviderType::ClaudeOAuth {
        return None;
    }
    let bound_account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    let account = accounts.find_for_provider(provider.provider_type, bound_account_id)?;
    let max_concurrent = provider_account_concurrency_limit(provider, account)?;
    Some(AccountConcurrencySelection {
        provider_type: provider.provider_type,
        account_id: account.id.clone(),
        max_concurrent,
        current: snapshot.current(provider.provider_type, &account.id),
    })
}

fn provider_account_concurrency_limit(
    provider: &StoredProvider,
    account: &crate::domain::accounts::store::Account,
) -> Option<u32> {
    let limit = provider_concurrency_override(provider)
        .or_else(|| account_profile_concurrency_limit(account))
        .or_else(|| {
            std::env::var("CC_SWITCH_ACCOUNT_MAX_CONCURRENT")
                .ok()
                .and_then(|value| value.trim().parse::<u32>().ok())
        })
        .unwrap_or(DEFAULT_CLAUDE_ACCOUNT_MAX_CONCURRENT);
    (limit > 0).then_some(limit)
}

fn provider_concurrency_override(provider: &StoredProvider) -> Option<u32> {
    const POINTERS: &[&str] = &[
        "/env/ACCOUNT_MAX_CONCURRENT",
        "/env/MAX_CONCURRENT_REQUESTS",
        "/ACCOUNT_MAX_CONCURRENT",
        "/MAX_CONCURRENT_REQUESTS",
        "/accountMaxConcurrent",
        "/maxConcurrentRequests",
    ];
    POINTERS.iter().find_map(|pointer| {
        provider
            .provider
            .settings_config
            .pointer(pointer)
            .and_then(json_u32)
    })
}

fn account_profile_concurrency_limit(
    account: &crate::domain::accounts::store::Account,
) -> Option<u32> {
    const POINTERS: &[&str] = &[
        "/max_concurrent_requests",
        "/maxConcurrentRequests",
        "/rate_limit/max_concurrent_requests",
        "/rateLimit/maxConcurrentRequests",
    ];
    let profile = account.profile.as_ref()?;
    POINTERS
        .iter()
        .find_map(|pointer| profile.pointer(pointer).and_then(json_u32))
}

fn json_u32(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| value.as_str()?.trim().parse::<u32>().ok())
}

fn account_concurrency_load_score(selection: AccountConcurrencySelection) -> u64 {
    u64::from(selection.current)
        .saturating_mul(1_000_000)
        .checked_div(u64::from(selection.max_concurrent))
        .unwrap_or_default()
}

fn account_concurrency_limit_error(provider: &StoredProvider) -> ProxyError {
    ProxyError {
        status: axum::http::StatusCode::TOO_MANY_REQUESTS,
        message: format!(
            "provider {} account concurrency limit has been reached",
            provider.provider.id
        ),
    }
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

fn ensure_provider_account_does_not_need_relogin(
    provider: &StoredProvider,
    accounts: &AccountStore,
) -> Result<(), ProxyError> {
    if provider_account_needs_relogin(provider, accounts) {
        return Err(ProxyError {
            status: axum::http::StatusCode::UNAUTHORIZED,
            message: format!("provider {} account requires login", provider.provider.id),
        });
    }
    Ok(())
}

fn provider_account_needs_relogin(provider: &StoredProvider, accounts: &AccountStore) -> bool {
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    accounts
        .find_for_provider(provider.provider_type, account_id)
        .is_some_and(|account| account.needs_relogin)
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
    use crate::state::AccountInFlightTracker;

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

    fn claude_oauth_provider(
        id: &str,
        account_id: &str,
        max_concurrent: Option<u32>,
    ) -> StoredProvider {
        let mut settings = json!({});
        if let Some(max_concurrent) = max_concurrent {
            settings["ACCOUNT_MAX_CONCURRENT"] = json!(max_concurrent);
        }
        StoredProvider {
            app: AppKind::Claude,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: settings,
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some("claude_oauth".to_string()),
                        account_id: Some(account_id.to_string()),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::ClaudeOAuth,
            provider_type_id: "claude_oauth".to_string(),
        }
    }

    fn claude_oauth_account(id: &str) -> UpsertAccountInput {
        UpsertAccountInput {
            id: Some(id.to_string()),
            provider_type: ProviderType::ClaudeOAuth,
            email: None,
            access_token: Some(format!("token-{id}")),
            refresh_token: Some(format!("refresh-{id}")),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: None,
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            rate_limited_until: None,
            last_refresh_error: None,
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
            extra_headers: None,
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
            extra_headers: None,
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
    fn logical_retry_excludes_failed_provider_but_preserves_explicit_pin() {
        let store = provider_store();
        let accounts = AccountStore::default();
        let tracker = AccountInFlightTracker::default();
        let snapshot = tracker.snapshot();
        let mut failover = enabled_failover(&store);
        let excluded = BTreeSet::from(["p1".to_string()]);

        let selected = select_provider_with_account_inflight_excluding(
            &store,
            &accounts,
            &mut failover,
            AppKind::Codex,
            &HeaderMap::new(),
            &snapshot,
            &excluded,
        )
        .unwrap();
        assert_eq!(selected.provider.provider.id, "p2");

        let mut pinned = HeaderMap::new();
        pinned.insert("x-cc-provider-id", HeaderValue::from_static("p1"));
        let selected = select_provider_with_account_inflight_excluding(
            &store,
            &accounts,
            &mut failover,
            AppKind::Codex,
            &pinned,
            &snapshot,
            &excluded,
        )
        .unwrap();
        assert_eq!(selected.provider.provider.id, "p1");
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

    #[test]
    fn automatic_selection_skips_account_that_requires_relogin() {
        let store = ProviderStore {
            providers: vec![
                codex_oauth_provider("p1", "acct-1"),
                codex_oauth_provider("p2", "acct-2"),
            ],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        accounts.accounts[0].needs_relogin = true;
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
    fn explicit_provider_returns_401_when_account_requires_relogin() {
        let store = ProviderStore {
            providers: vec![codex_oauth_provider("p1", "acct-1")],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.accounts[0].needs_relogin = true;
        let mut failover = enabled_failover(&store);
        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p1"));

        let error = select_provider(&store, &accounts, &mut failover, AppKind::Codex, &headers)
            .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("requires login"));
    }

    #[test]
    fn claude_oauth_selection_prefers_lower_account_load() {
        let store = ProviderStore {
            providers: vec![
                claude_oauth_provider("p1", "acct-1", Some(8)),
                claude_oauth_provider("p2", "acct-2", Some(8)),
            ],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        accounts.upsert(claude_oauth_account("acct-2"));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 8)
            .unwrap();
        let snapshot = tracker.snapshot();
        let mut failover = FailoverStore::default();
        failover.update_app_config(
            AppKind::Claude,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p1".to_string(), "p2".to_string()]),
                failure_threshold: None,
                open_duration_ms: None,
                half_open_max_probes: None,
            },
            &store,
        );

        let selected = select_provider_with_account_inflight(
            &store,
            &accounts,
            &mut failover,
            AppKind::Claude,
            &HeaderMap::new(),
            &snapshot,
        )
        .unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
    }

    #[test]
    fn claude_oauth_selection_skips_saturated_account() {
        let store = ProviderStore {
            providers: vec![
                claude_oauth_provider("p1", "acct-1", Some(1)),
                claude_oauth_provider("p2", "acct-2", Some(1)),
            ],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        accounts.upsert(claude_oauth_account("acct-2"));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 1)
            .unwrap();
        let snapshot = tracker.snapshot();
        let mut failover = FailoverStore::default();

        let selected = select_provider_with_account_inflight(
            &store,
            &accounts,
            &mut failover,
            AppKind::Claude,
            &HeaderMap::new(),
            &snapshot,
        )
        .unwrap();
        assert_eq!(selected.provider.provider.id, "p2");

        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p1"));
        let error = select_provider_with_account_inflight(
            &store,
            &accounts,
            &mut failover,
            AppKind::Claude,
            &headers,
            &snapshot,
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn count_tokens_selection_rejects_transform_providers() {
        let mut unsupported = provider(AppKind::Claude, "codex-first");
        unsupported.provider_type = ProviderType::Codex;
        unsupported.provider_type_id = "codex".to_string();
        let supported = claude_oauth_provider("claude-native", "acct-1", None);
        let store = ProviderStore {
            providers: vec![unsupported, supported],
        };
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        let tracker = AccountInFlightTracker::default();
        let mut failover = FailoverStore::default();

        let selected = select_provider_for_claude_count_tokens(
            &store,
            &accounts,
            &mut failover,
            &HeaderMap::new(),
            &tracker.snapshot(),
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(selected.provider.provider.id, "claude-native");

        let mut pinned = HeaderMap::new();
        pinned.insert("x-cc-provider-id", HeaderValue::from_static("codex-first"));
        let error = select_provider_for_claude_count_tokens(
            &store,
            &accounts,
            &mut failover,
            &pinned,
            &tracker.snapshot(),
            &BTreeSet::new(),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
    }
}
