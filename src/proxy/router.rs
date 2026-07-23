use std::collections::BTreeSet;

use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::{active_account_usage_block, AccountStore, AccountUsageBlock};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::infra::time::now_ms;
use crate::state::AccountInFlightSnapshot;

use super::provider_ops::ProviderExecution;
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
    pub execution: ProviderExecution,
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
    affinity_key: Option<&'a str>,
}

const DEFAULT_ACCOUNT_MAX_CONCURRENT: u32 = 8;

pub(super) fn select_provider(
    store: &ProviderStore,
    accounts: &AccountStore,
    app: AppKind,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(
        store,
        accounts,
        app,
        headers,
        current_provider_id,
        None,
        None,
    )
}

pub(super) fn select_provider_with_account_inflight(
    store: &ProviderStore,
    accounts: &AccountStore,
    app: AppKind,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
    account_in_flight: &AccountInFlightSnapshot,
    affinity_key: Option<&str>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        app,
        headers,
        current_provider_id,
        ProviderSelectionOptions {
            account_in_flight: Some(account_in_flight),
            affinity_key,
            ..ProviderSelectionOptions::default()
        },
    )
}

pub(super) fn select_provider_for_claude_count_tokens(
    store: &ProviderStore,
    accounts: &AccountStore,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
    account_in_flight: &AccountInFlightSnapshot,
    affinity_key: Option<&str>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        AppKind::Claude,
        headers,
        current_provider_id,
        ProviderSelectionOptions {
            provider_filter: Some(provider_supports_claude_count_tokens),
            account_in_flight: Some(account_in_flight),
            affinity_key,
            ..ProviderSelectionOptions::default()
        },
    )
}

pub(super) fn select_failover_provider(
    store: &ProviderStore,
    accounts: &AccountStore,
    route: ProxyRoute,
    account_in_flight: &AccountInFlightSnapshot,
    excluded_provider_ids: &BTreeSet<String>,
) -> Option<ProviderRouteSelection> {
    select_failover_provider_matching(
        store,
        accounts,
        route,
        account_in_flight,
        excluded_provider_ids,
        |_| true,
    )
}

pub(super) fn select_failover_provider_matching(
    store: &ProviderStore,
    accounts: &AccountStore,
    route: ProxyRoute,
    account_in_flight: &AccountInFlightSnapshot,
    excluded_provider_ids: &BTreeSet<String>,
    provider_filter: fn(&StoredProvider) -> bool,
) -> Option<ProviderRouteSelection> {
    let app = route.app();
    let now = now_ms();
    let excluded_account_keys = store
        .list(Some(app))
        .into_iter()
        .filter(|provider| excluded_provider_ids.contains(&provider.provider.id))
        .filter_map(|provider| provider_account_failover_key(&provider))
        .collect::<BTreeSet<_>>();
    for provider in store.list(Some(app)) {
        if excluded_provider_ids.contains(&provider.provider.id)
            || provider_account_failover_key(&provider)
                .is_some_and(|key| excluded_account_keys.contains(&key))
            || !provider_filter(&provider)
            || (route == ProxyRoute::ClaudeCountTokens
                && !provider_supports_claude_count_tokens(&provider))
            || ensure_provider_account_does_not_need_relogin(&provider, accounts).is_err()
            || ensure_provider_account_usage_available(&provider, accounts, now).is_err()
            || account_concurrency_for_provider(&provider, accounts, account_in_flight)
                .is_some_and(|selection| selection.current >= selection.max_concurrent)
        {
            continue;
        }
        let execution = match ProviderExecution::from_store(store, provider) {
            Ok(execution) => execution,
            Err(error) => {
                tracing::debug!(
                    status = error.status.as_u16(),
                    "skipping ineligible failover Provider"
                );
                continue;
            }
        };
        if execution
            .ensure_operation_supported(super::provider_ops::ProviderOperation::Forward)
            .is_err()
        {
            continue;
        }
        return Some(ProviderRouteSelection { execution });
    }
    None
}

fn provider_account_failover_key(provider: &StoredProvider) -> Option<String> {
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())?
        .trim();
    (!account_id.is_empty()).then(|| format!("{}:{account_id}", provider.provider_type.as_str()))
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
    app: AppKind,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
    provider_type: ProviderType,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_inner(
        store,
        accounts,
        app,
        headers,
        current_provider_id,
        Some(provider_type),
        None,
    )
}

pub(super) fn select_provider_for_codex_image_generation(
    store: &ProviderStore,
    accounts: &AccountStore,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
    account_in_flight: &AccountInFlightSnapshot,
    affinity_key: Option<&str>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        AppKind::Codex,
        headers,
        current_provider_id,
        ProviderSelectionOptions {
            provider_filter: Some(codex_image_generation_provider),
            account_in_flight: Some(account_in_flight),
            affinity_key,
            ..ProviderSelectionOptions::default()
        },
    )
}

fn select_provider_inner(
    store: &ProviderStore,
    accounts: &AccountStore,
    app: AppKind,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
    provider_type_filter: Option<ProviderType>,
    account_in_flight: Option<&AccountInFlightSnapshot>,
) -> Result<ProviderRouteSelection, ProxyError> {
    select_provider_with_optional_filter(
        store,
        accounts,
        app,
        headers,
        current_provider_id,
        ProviderSelectionOptions {
            provider_type_filter,
            account_in_flight,
            ..ProviderSelectionOptions::default()
        },
    )
}

fn select_provider_with_optional_filter(
    store: &ProviderStore,
    accounts: &AccountStore,
    app: AppKind,
    headers: &HeaderMap,
    current_provider_id: Option<&str>,
    options: ProviderSelectionOptions<'_>,
) -> Result<ProviderRouteSelection, ProxyError> {
    let ProviderSelectionOptions {
        provider_type_filter,
        provider_filter,
        account_in_flight,
        affinity_key,
    } = options;
    let now = now_ms();
    let explicit_provider_id = headers
        .get("x-cc-provider-id")
        .and_then(|value| value.to_str().ok());
    let provider_id = explicit_provider_id
        .or(current_provider_id)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProxyError::not_found(format!(
                "no current provider configured for {}",
                app.as_str()
            ))
        })?;
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
    let automatic_managed_selection =
        explicit_provider_id.is_none() && bound_account_for_provider(&provider, accounts).is_some();
    if let Some(account_in_flight) = account_in_flight.filter(|_| automatic_managed_selection) {
        if let Some(selection) = select_managed_provider_candidate(
            store,
            accounts,
            &provider,
            provider_filter,
            account_in_flight,
            affinity_key,
        ) {
            return Ok(selection);
        }
    }
    finalize_provider_selection(store, accounts, provider, account_in_flight, now)
}

fn select_managed_provider_candidate(
    store: &ProviderStore,
    accounts: &AccountStore,
    current_provider: &StoredProvider,
    provider_filter: Option<fn(&StoredProvider) -> bool>,
    account_in_flight: &AccountInFlightSnapshot,
    affinity_key: Option<&str>,
) -> Option<ProviderRouteSelection> {
    let now = now_ms();
    let mut best: Option<(ProviderRouteSelection, u32, u32, u64, bool)> = None;
    for provider in store.list(Some(current_provider.app)) {
        if provider.provider_type != current_provider.provider_type
            || !provider_filter
                .map(|filter| filter(&provider))
                .unwrap_or(true)
            || ensure_provider_account_does_not_need_relogin(&provider, accounts).is_err()
            || ensure_provider_account_usage_available(&provider, accounts, now).is_err()
            || bound_account_for_provider(&provider, accounts).is_none()
        {
            continue;
        }
        let concurrency = account_concurrency_for_provider(&provider, accounts, account_in_flight);
        if concurrency
            .as_ref()
            .is_some_and(|selection| selection.current >= selection.max_concurrent)
        {
            continue;
        }
        let execution = match ProviderExecution::from_store(store, provider) {
            Ok(execution) => execution,
            Err(_) => continue,
        };
        if execution
            .ensure_operation_supported(super::provider_ops::ProviderOperation::Forward)
            .is_err()
        {
            continue;
        }
        let (load, capacity) = concurrency
            .map(|selection| (selection.current, selection.max_concurrent))
            .unwrap_or((0, u32::MAX));
        let affinity = affinity_key
            .map(|key| provider_affinity_score(key, &execution.stored.provider.id))
            .unwrap_or_default();
        let is_current = execution.stored.provider.id == current_provider.provider.id;
        let replace = best.as_ref().is_none_or(
            |(_, best_load, best_capacity, best_affinity, best_is_current)| {
                let left = u64::from(load) * u64::from(*best_capacity);
                let right = u64::from(*best_load) * u64::from(capacity);
                left < right
                    || (left == right
                        && if affinity_key.is_some() {
                            affinity > *best_affinity
                        } else {
                            is_current && !*best_is_current
                        })
            },
        );
        if replace {
            best = Some((
                ProviderRouteSelection { execution },
                load,
                capacity,
                affinity,
                is_current,
            ));
        }
    }
    best.map(|(selection, ..)| selection)
}

fn provider_affinity_score(key: &str, provider_id: &str) -> u64 {
    let mut digest = Sha256::new();
    digest.update(key.as_bytes());
    digest.update([0]);
    digest.update(provider_id.as_bytes());
    let bytes = digest.finalize();
    u64::from_be_bytes(bytes[..8].try_into().expect("SHA-256 prefix has 8 bytes"))
}

fn finalize_provider_selection(
    store: &ProviderStore,
    accounts: &AccountStore,
    provider: StoredProvider,
    account_in_flight: Option<&AccountInFlightSnapshot>,
    now: u128,
) -> Result<ProviderRouteSelection, ProxyError> {
    ensure_provider_account_does_not_need_relogin(&provider, accounts)?;
    ensure_provider_account_usage_available(&provider, accounts, now)?;
    if account_in_flight
        .and_then(|snapshot| account_concurrency_for_provider(&provider, accounts, snapshot))
        .is_some_and(|selection| selection.current >= selection.max_concurrent)
    {
        return Err(account_concurrency_limit_error(&provider));
    }
    let execution = ProviderExecution::from_store(store, provider)?;
    execution.ensure_operation_supported(super::provider_ops::ProviderOperation::Forward)?;
    Ok(ProviderRouteSelection { execution })
}

pub(super) fn account_concurrency_for_provider(
    provider: &StoredProvider,
    accounts: &AccountStore,
    snapshot: &AccountInFlightSnapshot,
) -> Option<AccountConcurrencySelection> {
    let account = bound_account_for_provider(provider, accounts)?;
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
        .unwrap_or(DEFAULT_ACCOUNT_MAX_CONCURRENT);
    (limit > 0).then_some(limit)
}

fn bound_account_for_provider<'a>(
    provider: &StoredProvider,
    accounts: &'a AccountStore,
) -> Option<&'a crate::domain::accounts::store::Account> {
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())?;
    accounts.find_for_provider(provider.provider_type, Some(account_id))
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

pub(super) fn ensure_provider_account_usage_available(
    provider: &StoredProvider,
    accounts: &AccountStore,
    now_ms: u128,
) -> Result<(), ProxyError> {
    if let Some(block) = provider_account_usage_block(provider, accounts, now_ms) {
        return Err(ProxyError {
            status: axum::http::StatusCode::TOO_MANY_REQUESTS,
            message: format!(
                "provider {} account is {}: {} until {}",
                provider.provider.id,
                block.kind.availability(),
                block.reason,
                block.until_ms,
            ),
        });
    }
    Ok(())
}

pub(super) fn ensure_provider_account_does_not_need_relogin(
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
    let Some(account_id) = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
    else {
        return false;
    };
    accounts
        .find_for_provider(provider.provider_type, Some(account_id))
        .is_some_and(|account| account.needs_relogin)
}

fn provider_account_usage_block(
    provider: &StoredProvider,
    accounts: &AccountStore,
    now_ms: u128,
) -> Option<AccountUsageBlock> {
    let now_ms = i64::try_from(now_ms).unwrap_or(i64::MAX);
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())?;
    accounts
        .find_for_provider(provider.provider_type, Some(account_id))
        .and_then(|account| active_account_usage_block(account, now_ms))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::{AccountQuota, AccountQuotaTier, UpsertAccountInput};
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
            resource: Default::default(),
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
                        auth_identity_generation: None,
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
            resource: Default::default(),
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
                        auth_identity_generation: None,
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::ClaudeOAuth,
            provider_type_id: "claude_oauth".to_string(),
            resource: Default::default(),
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

    fn exhausted_codex_oauth_account(id: &str, now_ms: i64) -> UpsertAccountInput {
        let mut input = codex_oauth_account(id, None);
        input.quota_percent = Some(100.0);
        input.quota = Some(AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Plus".to_string()),
            tiers: vec![AccountQuotaTier {
                name: "seven_day".to_string(),
                utilization: Some(1.0),
                resets_at: Some(now_ms + 7 * 24 * 60 * 60 * 1000),
                ..Default::default()
            }],
            extra_usage: Some(json!({
                "subscriptionEvidence": {
                    "usageAllowed": false,
                    "usageLimitReached": true
                }
            })),
        });
        input.quota_refreshed_at = Some(now_ms - 5 * 60 * 1000);
        input.quota_next_refresh_at = Some(now_ms + 25 * 60 * 1000);
        input
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
                        auth_identity_generation: None,
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorOAuth,
            provider_type_id: "cursor_oauth".to_string(),
            resource: Default::default(),
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
        runtime_store(vec![
            provider(AppKind::Codex, "p1"),
            provider(AppKind::Codex, "p2"),
        ])
    }

    fn runtime_store(providers: Vec<StoredProvider>) -> ProviderStore {
        let mut store = ProviderStore {
            providers,
            ..Default::default()
        };
        store
            .rebuild_runtime_index(&AccountStore::default())
            .unwrap();
        store
    }

    #[test]
    fn explicit_provider_header_overrides_current_provider() {
        let store = provider_store();
        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p2"));

        let accounts = AccountStore::default();
        let selected =
            select_provider(&store, &accounts, AppKind::Codex, &headers, Some("p1")).unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p2");
    }

    #[test]
    fn current_provider_is_selected_without_automatic_fallback() {
        let store = provider_store();
        let selected = select_provider(
            &store,
            &AccountStore::default(),
            AppKind::Codex,
            &HeaderMap::new(),
            Some("p2"),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p2");
    }

    #[test]
    fn missing_current_provider_is_rejected() {
        let store = provider_store();
        let error = select_provider(
            &store,
            &AccountStore::default(),
            AppKind::Codex,
            &HeaderMap::new(),
            None,
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
        assert!(error.message.contains("no current provider"));
    }

    #[test]
    fn current_rate_limited_provider_returns_429_without_fallback() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![
            codex_oauth_provider("p1", "acct-1"),
            codex_oauth_provider("p2", "acct-2"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", Some(now + 60_000)));
        accounts.upsert(codex_oauth_account("acct-2", None));
        let error = select_provider(
            &store,
            &accounts,
            AppKind::Codex,
            &HeaderMap::new(),
            Some("p1"),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn current_quota_exhausted_provider_returns_429_without_fallback() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![codex_oauth_provider("p1", "acct-1")]);
        let mut accounts = AccountStore::default();
        accounts.upsert(exhausted_codex_oauth_account("acct-1", now));

        let error = select_provider(
            &store,
            &accounts,
            AppKind::Codex,
            &HeaderMap::new(),
            Some("p1"),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert!(error.message.contains("quota_exhausted"));
    }

    #[test]
    fn current_cursor_provider_respects_account_cooldown() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![
            cursor_oauth_provider("p1", "cursor-acct-1"),
            cursor_oauth_provider("p2", "cursor-acct-2"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(cursor_oauth_account("cursor-acct-1", Some(now + 60_000)));
        accounts.upsert(cursor_oauth_account("cursor-acct-2", None));
        let error = select_provider(
            &store,
            &accounts,
            AppKind::Codex,
            &HeaderMap::new(),
            Some("p1"),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn account_bound_kiro_provider_respects_account_cooldown() {
        let now = now_ms() as i64;
        let mut provider = claude_oauth_provider("kiro", "kiro-acct", None);
        provider.provider_type = ProviderType::KiroOAuth;
        provider.provider_type_id = ProviderType::KiroOAuth.as_str().to_string();
        let mut account = claude_oauth_account("kiro-acct");
        account.provider_type = ProviderType::KiroOAuth;
        account.rate_limited_until = Some(now + 60_000);
        let mut accounts = AccountStore::default();
        accounts.upsert(account);

        let error = ensure_provider_account_usage_available(&provider, &accounts, now as u128)
            .expect_err("Kiro account cooldown must be enforced");
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn current_provider_that_requires_relogin_returns_401_without_fallback() {
        let store = runtime_store(vec![
            codex_oauth_provider("p1", "acct-1"),
            codex_oauth_provider("p2", "acct-2"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        accounts.accounts[0].needs_relogin = true;
        let error = select_provider(
            &store,
            &accounts,
            AppKind::Codex,
            &HeaderMap::new(),
            Some("p1"),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("requires login"));
    }

    #[test]
    fn automatic_managed_selection_skips_saturated_current_account() {
        let store = runtime_store(vec![
            claude_oauth_provider("p1", "acct-1", Some(1)),
            claude_oauth_provider("p2", "acct-2", Some(1)),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        accounts.upsert(claude_oauth_account("acct-2"));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 1)
            .unwrap();
        let snapshot = tracker.snapshot();
        let selected = select_provider_with_account_inflight(
            &store,
            &accounts,
            AppKind::Claude,
            &HeaderMap::new(),
            Some("p1"),
            &snapshot,
            None,
        )
        .unwrap();
        assert_eq!(selected.execution.stored.provider.id, "p2");
    }

    #[test]
    fn explicit_managed_provider_at_concurrency_limit_returns_429() {
        let store = runtime_store(vec![
            claude_oauth_provider("p1", "acct-1", Some(1)),
            claude_oauth_provider("p2", "acct-2", Some(1)),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        accounts.upsert(claude_oauth_account("acct-2"));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-1", 1)
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p1"));

        let error = select_provider_with_account_inflight(
            &store,
            &accounts,
            AppKind::Claude,
            &headers,
            Some("p1"),
            &tracker.snapshot(),
            Some("session-pinned"),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn automatic_managed_selection_prefers_lower_occupancy_ratio_for_all_oauth_types() {
        let mut first = codex_oauth_provider("p1", "acct-1");
        first.provider.settings_config["ACCOUNT_MAX_CONCURRENT"] = json!(8);
        let mut second = codex_oauth_provider("p2", "acct-2");
        second.provider.settings_config["ACCOUNT_MAX_CONCURRENT"] = json!(2);
        let store = runtime_store(vec![first, second]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _first_guards = [
            tracker
                .try_acquire(ProviderType::CodexOAuth, "acct-1", 8)
                .unwrap(),
            tracker
                .try_acquire(ProviderType::CodexOAuth, "acct-1", 8)
                .unwrap(),
        ];
        let _second_guard = tracker
            .try_acquire(ProviderType::CodexOAuth, "acct-2", 2)
            .unwrap();

        let selected = select_provider_with_account_inflight(
            &store,
            &accounts,
            AppKind::Codex,
            &HeaderMap::new(),
            Some("p2"),
            &tracker.snapshot(),
            Some("session-load-aware"),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p1");
        assert_eq!(
            account_concurrency_for_provider(
                &selected.execution.stored,
                &accounts,
                &tracker.snapshot()
            )
            .unwrap()
            .max_concurrent,
            8
        );
    }

    #[test]
    fn managed_selection_affinity_is_stable_when_load_is_equal() {
        let store = runtime_store(vec![
            claude_oauth_provider("p1", "acct-1", Some(8)),
            claude_oauth_provider("p2", "acct-2", Some(8)),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        accounts.upsert(claude_oauth_account("acct-2"));
        let snapshot = AccountInFlightTracker::default().snapshot();

        let first = select_provider_with_account_inflight(
            &store,
            &accounts,
            AppKind::Claude,
            &HeaderMap::new(),
            Some("p1"),
            &snapshot,
            Some("stable-session"),
        )
        .unwrap();
        let second = select_provider_with_account_inflight(
            &store,
            &accounts,
            AppKind::Claude,
            &HeaderMap::new(),
            Some("p1"),
            &snapshot,
            Some("stable-session"),
        )
        .unwrap();

        assert_eq!(
            first.execution.stored.provider.id,
            second.execution.stored.provider.id
        );
    }

    #[test]
    fn failover_selection_uses_authoritative_order_and_exclusions() {
        let mut store = runtime_store(vec![
            claude_oauth_provider("p1", "acct-1", None),
            claude_oauth_provider("p2", "acct-2", None),
            claude_oauth_provider("p3", "acct-3", None),
        ]);
        store.order.insert(
            AppKind::Claude,
            vec!["p3".to_string(), "p1".to_string(), "p2".to_string()],
        );
        let mut accounts = AccountStore::default();
        for account_id in ["acct-1", "acct-2", "acct-3"] {
            accounts.upsert(claude_oauth_account(account_id));
        }
        let excluded = BTreeSet::from(["p3".to_string()]);

        let selected = select_failover_provider(
            &store,
            &accounts,
            ProxyRoute::ClaudeMessages,
            &AccountInFlightTracker::default().snapshot(),
            &excluded,
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p1");
    }

    #[test]
    fn failover_selection_skips_other_providers_bound_to_excluded_account() {
        let store = runtime_store(vec![
            claude_oauth_provider("failed", "shared-account", None),
            claude_oauth_provider("duplicate", "shared-account", None),
            claude_oauth_provider("backup", "backup-account", None),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("shared-account"));
        accounts.upsert(claude_oauth_account("backup-account"));

        let selected = select_failover_provider(
            &store,
            &accounts,
            ProxyRoute::ClaudeMessages,
            &AccountInFlightTracker::default().snapshot(),
            &BTreeSet::from(["failed".to_string()]),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "backup");
    }

    #[test]
    fn failover_selection_skips_unhealthy_and_saturated_accounts() {
        let store = runtime_store(vec![
            claude_oauth_provider("excluded", "acct-excluded", None),
            claude_oauth_provider("relogin", "acct-relogin", None),
            claude_oauth_provider("limited", "acct-limited", None),
            claude_oauth_provider("saturated", "acct-saturated", Some(1)),
            claude_oauth_provider("healthy", "acct-healthy", None),
        ]);
        let mut accounts = AccountStore::default();
        for account_id in [
            "acct-excluded",
            "acct-relogin",
            "acct-limited",
            "acct-saturated",
            "acct-healthy",
        ] {
            accounts.upsert(claude_oauth_account(account_id));
        }
        accounts
            .accounts
            .iter_mut()
            .find(|account| account.id == "acct-relogin")
            .unwrap()
            .needs_relogin = true;
        accounts
            .accounts
            .iter_mut()
            .find(|account| account.id == "acct-limited")
            .unwrap()
            .rate_limited_until = Some(now_ms() as i64 + 60_000);
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::ClaudeOAuth, "acct-saturated", 1)
            .unwrap();
        let excluded = BTreeSet::from(["excluded".to_string()]);

        let selected = select_failover_provider(
            &store,
            &accounts,
            ProxyRoute::ClaudeMessages,
            &tracker.snapshot(),
            &excluded,
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "healthy");
    }

    #[test]
    fn count_tokens_selection_rejects_transform_providers() {
        let mut unsupported = provider(AppKind::Claude, "codex-first");
        unsupported.provider_type = ProviderType::Codex;
        unsupported.provider_type_id = "codex".to_string();
        let supported = claude_oauth_provider("claude-native", "acct-1", None);
        let store = runtime_store(vec![unsupported, supported]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        let tracker = AccountInFlightTracker::default();

        let selected = select_provider_for_claude_count_tokens(
            &store,
            &accounts,
            &HeaderMap::new(),
            Some("claude-native"),
            &tracker.snapshot(),
            None,
        )
        .unwrap();
        assert_eq!(selected.execution.stored.provider.id, "claude-native");

        let mut pinned = HeaderMap::new();
        pinned.insert("x-cc-provider-id", HeaderValue::from_static("codex-first"));
        let error = select_provider_for_claude_count_tokens(
            &store,
            &accounts,
            &pinned,
            Some("claude-native"),
            &tracker.snapshot(),
            None,
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
    }
}
