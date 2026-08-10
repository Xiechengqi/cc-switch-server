use std::collections::BTreeSet;

use crate::domain::accounts::store::{
    active_account_usage_block, active_account_usage_block_for_share, AccountStore,
    AccountUsageBlock,
};
#[cfg(test)]
use crate::domain::providers::bundle::surface_enabled;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::{authoritative_managed_account, managed_account_binding};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::infra::time::now_ms;
use crate::state::AccountInFlightSnapshot;

use super::provider_ops::ProviderExecution;
use super::{ProxyConcurrencyScope, ProxyError};

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

const DEFAULT_ACCOUNT_MAX_CONCURRENT: u32 = 8;

#[cfg(test)]
pub(super) fn select_test_provider(
    store: &ProviderStore,
    accounts: &AccountStore,
    app: AppKind,
    provider_id: &str,
    account_in_flight: Option<&AccountInFlightSnapshot>,
) -> Result<ProviderRouteSelection, ProxyError> {
    let provider = store
        .providers
        .iter()
        .find(|item| {
            item.app == app
                && surface_enabled(&item.provider)
                && item.provider.id == provider_id.trim()
        })
        .cloned()
        .ok_or_else(|| {
            ProxyError::not_found(format!(
                "no enabled {} Surface is configured for Provider {provider_id}",
                app.as_str()
            ))
        })?;
    finalize_provider_selection(store, accounts, provider, account_in_flight, now_ms())
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
    let providers = store.list(Some(app));
    let excluded_contains_managed_execution = providers
        .iter()
        .filter(|provider| excluded_provider_ids.contains(&provider.provider.id))
        .cloned()
        .any(|provider| {
            ProviderExecution::from_store(store, provider)
                .ok()
                .is_some_and(|execution| execution.managed_account_target().is_some())
        });
    if excluded_contains_managed_execution {
        return None;
    }
    for provider in providers {
        if excluded_provider_ids.contains(&provider.provider.id)
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
        if execution.managed_account_target().is_some() {
            continue;
        }
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

pub(super) fn provider_supports_claude_count_tokens(provider: &StoredProvider) -> bool {
    provider.app == AppKind::Claude
        && matches!(
            provider.provider_type,
            ProviderType::Claude
                | ProviderType::ClaudeAuth
                | ProviderType::ClaudeOAuth
                | ProviderType::KiroOAuth
        )
}

fn finalize_provider_selection(
    store: &ProviderStore,
    accounts: &AccountStore,
    provider: StoredProvider,
    account_in_flight: Option<&AccountInFlightSnapshot>,
    now: u128,
) -> Result<ProviderRouteSelection, ProxyError> {
    ensure_codex_oauth_binding(&provider, accounts)?;
    ensure_provider_account_does_not_need_relogin(&provider, accounts)?;
    ensure_provider_account_usage_available(&provider, accounts, now)?;
    if let Some(selection) = account_in_flight
        .and_then(|snapshot| account_concurrency_for_provider(&provider, accounts, snapshot))
        .filter(|selection| selection.current >= selection.max_concurrent)
    {
        return Err(account_concurrency_limit_error(
            &provider,
            selection.current,
            selection.max_concurrent,
        ));
    }
    let execution = ProviderExecution::from_store(store, provider)?;
    execution.ensure_operation_supported(super::provider_ops::ProviderOperation::Forward)?;
    Ok(ProviderRouteSelection { execution })
}

pub(crate) fn ensure_codex_oauth_binding(
    provider: &StoredProvider,
    accounts: &AccountStore,
) -> Result<(), ProxyError> {
    if provider.provider_type != ProviderType::CodexOAuth {
        return Ok(());
    }
    let Some((ProviderType::CodexOAuth, account_id)) = managed_account_binding(provider) else {
        return Err(ProxyError {
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            message: format!(
                "Codex OAuth provider {} has no explicit account binding",
                provider.provider.id
            ),
        });
    };
    if authoritative_managed_account(provider, accounts).is_none() {
        return Err(ProxyError {
            status: axum::http::StatusCode::SERVICE_UNAVAILABLE,
            message: format!(
                "Codex OAuth provider {} account binding {account_id} is missing or stale",
                provider.provider.id,
            ),
        });
    }
    Ok(())
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
    authoritative_managed_account(provider, accounts)
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

fn account_concurrency_limit_error(
    provider: &StoredProvider,
    current: u32,
    limit: u32,
) -> ProxyError {
    ProxyError::concurrency_limited(
        ProxyConcurrencyScope::ProviderAccount,
        current,
        limit,
        format!(
            "Provider account concurrency limit has been reached ({current}/{limit}) for {}. Wait for an in-flight request to finish.",
            provider.provider.id,
        ),
    )
}

pub(super) fn codex_image_generation_provider(provider: &StoredProvider) -> bool {
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
    if let Some(block) = provider_account_usage_block(provider, accounts, now_ms, false) {
        return Err(provider_account_usage_error(provider, now_ms, block));
    }
    Ok(())
}

pub(super) fn ensure_provider_account_usage_available_for_share(
    provider: &StoredProvider,
    accounts: &AccountStore,
    share: &crate::domain::sharing::shares::Share,
    now_ms: u128,
) -> Result<(), ProxyError> {
    if let Some(block) =
        provider_account_usage_block(provider, accounts, now_ms, share.allow_personal_credits)
    {
        return Err(provider_account_usage_error(provider, now_ms, block));
    }
    Ok(())
}

fn provider_account_usage_error(
    provider: &StoredProvider,
    now_ms: u128,
    block: AccountUsageBlock,
) -> ProxyError {
    let retry_after_seconds = u64::try_from(block.until_ms.saturating_sub(now_ms as i64))
        .unwrap_or(u64::MAX)
        .saturating_add(999)
        / 1_000;
    ProxyError::rate_limited(
        format!(
            "provider {} account is {}: {} until {}",
            provider.provider.id,
            block.kind.availability(),
            block.reason,
            block.until_ms,
        ),
        retry_after_seconds.max(1),
    )
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
    bound_account_for_provider(provider, accounts).is_some_and(|account| account.needs_relogin)
}

fn provider_account_usage_block(
    provider: &StoredProvider,
    accounts: &AccountStore,
    now_ms: u128,
    allow_personal_credits: bool,
) -> Option<AccountUsageBlock> {
    let now_ms = i64::try_from(now_ms).unwrap_or(i64::MAX);
    bound_account_for_provider(provider, accounts).and_then(|account| {
        if allow_personal_credits {
            active_account_usage_block_for_share(account, now_ms, true)
        } else {
            active_account_usage_block(account, now_ms)
        }
    })
}

#[cfg(test)]
mod tests {
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
                        auth_identity_generation: Some(1),
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

    fn grok_oauth_provider(id: &str, account_id: &str, max_concurrent: u32) -> StoredProvider {
        let mut provider = codex_oauth_provider(id, account_id);
        provider.provider_type = ProviderType::GrokOAuth;
        provider.provider_type_id = ProviderType::GrokOAuth.as_str().to_string();
        provider.provider.settings_config["ACCOUNT_MAX_CONCURRENT"] = json!(max_concurrent);
        provider
            .provider
            .meta
            .as_mut()
            .and_then(|meta| meta.auth_binding.as_mut())
            .unwrap()
            .auth_provider = Some(ProviderType::GrokOAuth.as_str().to_string());
        provider
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
                        auth_identity_generation: Some(1),
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

    fn claude_api_key_provider(id: &str) -> StoredProvider {
        StoredProvider {
            app: AppKind::Claude,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({
                    "env": {
                        "ANTHROPIC_AUTH_TOKEN": format!("key-{id}"),
                        "ANTHROPIC_BASE_URL": "https://api.anthropic.com"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Claude,
            provider_type_id: ProviderType::Claude.as_str().to_string(),
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

    fn grok_oauth_account(id: &str) -> UpsertAccountInput {
        let mut account = codex_oauth_account(id, None);
        account.provider_type = ProviderType::GrokOAuth;
        account.profile = Some(json!({
            "verifiedGrokClaims": {
                "subject": id
            }
        }));
        account
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
                        auth_identity_generation: Some(1),
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

    fn cursor_api_key_provider_with_stale_binding(id: &str, account_id: &str) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({"apiKey": "provider-owned-key"}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("legacy".to_string()),
                        auth_provider: Some("cursor_apikey".to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::CursorApiKey,
            provider_type_id: ProviderType::CursorApiKey.as_str().to_string(),
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

    fn runtime_store(mut providers: Vec<StoredProvider>) -> ProviderStore {
        let mut accounts = AccountStore::default();
        for provider in &mut providers {
            let Some(account_id) = provider
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.account_id.as_deref())
                .map(str::to_string)
            else {
                continue;
            };
            let generation = accounts
                .accounts
                .iter()
                .find(|account| account.id == account_id)
                .map(|account| account.auth_identity_generation)
                .unwrap_or_else(|| {
                    let input = match provider.provider_type {
                        ProviderType::ClaudeOAuth => claude_oauth_account(&account_id),
                        ProviderType::CodexOAuth => codex_oauth_account(&account_id, None),
                        ProviderType::GrokOAuth => grok_oauth_account(&account_id),
                        ProviderType::CursorOAuth => cursor_oauth_account(&account_id, None),
                        _ => return 1,
                    };
                    accounts.upsert(input).auth_identity_generation
                });
            if let Some(binding) = provider
                .provider
                .meta
                .as_mut()
                .and_then(|meta| meta.auth_binding.as_mut())
            {
                binding.auth_identity_generation = Some(generation);
            }
        }
        let mut store = ProviderStore {
            providers,
            ..Default::default()
        };
        store.rebuild_runtime_index(&accounts).unwrap();
        store
    }

    #[test]
    fn bound_surface_selects_the_exact_enabled_surface() {
        let store = provider_store();
        let selected =
            select_test_provider(&store, &AccountStore::default(), AppKind::Codex, "p2", None)
                .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p2");
    }

    #[test]
    fn missing_or_disabled_bound_surface_is_rejected() {
        let mut store = provider_store();
        store.providers[1]
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(false));

        for bound_surface in ["missing", "p2"] {
            let error = select_test_provider(
                &store,
                &AccountStore::default(),
                AppKind::Codex,
                bound_surface,
                None,
            )
            .unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::NOT_FOUND);
            assert!(error.message.contains("no enabled codex Surface"));
        }
    }

    #[test]
    fn codex_bound_surface_uses_its_binding_independent_of_active_account() {
        let store = runtime_store(vec![codex_oauth_provider("p2", "acct-2")]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        accounts
            .select_active_codex_oauth_account("acct-1")
            .unwrap();

        let selected = select_test_provider(&store, &accounts, AppKind::Codex, "p2", None).unwrap();

        assert_eq!(
            selected.execution.managed_account_target(),
            Some((ProviderType::CodexOAuth, "acct-2"))
        );
    }

    #[test]
    fn codex_bound_surface_rejects_a_missing_explicit_binding() {
        let store = runtime_store(vec![codex_oauth_provider("p1", "acct-1")]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-2", None));

        let error =
            select_test_provider(&store, &accounts, AppKind::Codex, "p1", None).unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
        assert!(error.message.contains("missing or stale"));
    }

    #[test]
    fn bound_surface_rate_limited_provider_returns_429_without_fallback() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![
            codex_oauth_provider("p1", "acct-1"),
            codex_oauth_provider("p2", "acct-2"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", Some(now + 60_000)));
        accounts.upsert(codex_oauth_account("acct-2", None));
        let error =
            select_test_provider(&store, &accounts, AppKind::Codex, "p1", None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn bound_surface_quota_exhausted_provider_returns_429_without_fallback() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![codex_oauth_provider("p1", "acct-1")]);
        let mut accounts = AccountStore::default();
        accounts.upsert(exhausted_codex_oauth_account("acct-1", now));

        let error =
            select_test_provider(&store, &accounts, AppKind::Codex, "p1", None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
        assert!(error.message.contains("quota_exhausted"));
    }

    #[test]
    fn bound_surface_cursor_provider_respects_account_cooldown() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![
            cursor_oauth_provider("p1", "cursor-acct-1"),
            cursor_oauth_provider("p2", "cursor-acct-2"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(cursor_oauth_account("cursor-acct-1", Some(now + 60_000)));
        accounts.upsert(cursor_oauth_account("cursor-acct-2", None));
        let error =
            select_test_provider(&store, &accounts, AppKind::Codex, "p1", None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::TOO_MANY_REQUESTS);
    }

    #[test]
    fn provider_owned_cursor_key_ignores_stale_account_state() {
        let now = now_ms() as i64;
        let store = runtime_store(vec![cursor_api_key_provider_with_stale_binding(
            "cursor-static",
            "legacy-cursor-account",
        )]);
        let mut account = cursor_oauth_account("legacy-cursor-account", Some(now + 60_000));
        account.provider_type = ProviderType::CursorApiKey;
        account.api_key = Some("deprecated-account-key".to_string());
        let mut accounts = AccountStore::default();
        accounts.upsert(account);
        accounts.accounts[0].needs_relogin = true;
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::CursorApiKey, "legacy-cursor-account", 1)
            .unwrap();

        let selected = select_test_provider(
            &store,
            &accounts,
            AppKind::Codex,
            "cursor-static",
            Some(&tracker.snapshot()),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "cursor-static");
        assert!(account_concurrency_for_provider(
            &selected.execution.stored,
            &accounts,
            &tracker.snapshot()
        )
        .is_none());
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
    fn bound_surface_provider_that_requires_relogin_returns_401_without_fallback() {
        let store = runtime_store(vec![
            codex_oauth_provider("p1", "acct-1"),
            codex_oauth_provider("p2", "acct-2"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        accounts.accounts[0].needs_relogin = true;
        let error =
            select_test_provider(&store, &accounts, AppKind::Codex, "p1", None).unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::UNAUTHORIZED);
        assert!(error.message.contains("requires login"));
    }

    #[test]
    fn saturated_managed_provider_returns_concurrency_error_without_switching_accounts() {
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
        let error = select_test_provider(&store, &accounts, AppKind::Claude, "p1", Some(&snapshot))
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            error.error_code(),
            "cc_switch_provider_account_concurrency_limit_exceeded"
        );
        assert_eq!(error.error_scope(), Some("provider_account"));
        assert_eq!(
            error.concurrency_metadata(),
            Some(crate::proxy::ProxyConcurrencyMetadata {
                scope: crate::proxy::ProxyConcurrencyScope::ProviderAccount,
                current: 1,
                limit: 1,
            })
        );
        assert!(error.retry_after_seconds().is_none());
        assert!(error.client_message().contains("p1"));
    }

    #[test]
    fn grok_bound_surface_at_concurrency_limit_never_selects_another_account() {
        let store = runtime_store(vec![
            grok_oauth_provider("grok-current", "grok-acct-current", 1),
            grok_oauth_provider("grok-other", "grok-acct-other", 1),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(grok_oauth_account("grok-acct-current"));
        accounts.upsert(grok_oauth_account("grok-acct-other"));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::GrokOAuth, "grok-acct-current", 1)
            .unwrap();

        let error = select_test_provider(
            &store,
            &accounts,
            AppKind::Codex,
            "grok-current",
            Some(&tracker.snapshot()),
        )
        .unwrap_err();

        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            error.error_code(),
            "cc_switch_provider_account_concurrency_limit_exceeded"
        );
        assert!(error.retry_after_seconds().is_none());
        assert!(error.client_message().contains("grok-current"));
    }

    #[test]
    fn grok_bound_surface_remains_selected_when_another_account_is_less_busy() {
        let store = runtime_store(vec![
            grok_oauth_provider("grok-current", "grok-acct-current", 8),
            grok_oauth_provider("grok-other", "grok-acct-other", 8),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(grok_oauth_account("grok-acct-current"));
        accounts.upsert(grok_oauth_account("grok-acct-other"));
        let tracker = std::sync::Arc::new(AccountInFlightTracker::default());
        let _guard = tracker
            .try_acquire(ProviderType::GrokOAuth, "grok-acct-current", 8)
            .unwrap();

        let selected = select_test_provider(
            &store,
            &accounts,
            AppKind::Codex,
            "grok-current",
            Some(&tracker.snapshot()),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "grok-current");
    }

    #[test]
    fn claude_bound_surface_never_switches_accounts() {
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

        let error = select_test_provider(
            &store,
            &accounts,
            AppKind::Claude,
            "p1",
            Some(&tracker.snapshot()),
        )
        .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::CONFLICT);
        assert_eq!(
            error.error_code(),
            "cc_switch_provider_account_concurrency_limit_exceeded"
        );
        assert!(error.retry_after_seconds().is_none());
    }

    #[test]
    fn codex_images_honor_bound_surface_binding_independent_of_active_account() {
        let mut requested = codex_oauth_provider("p2", "acct-2");
        requested
            .provider
            .meta
            .as_mut()
            .unwrap()
            .codex_image_generation_enabled = Some(true);
        let store = runtime_store(vec![requested]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        accounts
            .select_active_codex_oauth_account("acct-1")
            .unwrap();

        let selected = select_test_provider(
            &store,
            &accounts,
            AppKind::Codex,
            "p2",
            Some(&AccountInFlightTracker::default().snapshot()),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p2");
        assert!(codex_image_generation_provider(&selected.execution.stored));
    }

    #[test]
    fn codex_bound_surface_does_not_switch_for_load() {
        let mut first = codex_oauth_provider("p1", "acct-1");
        first.provider.settings_config["ACCOUNT_MAX_CONCURRENT"] = json!(8);
        let mut second = codex_oauth_provider("p2", "acct-2");
        second.provider.settings_config["ACCOUNT_MAX_CONCURRENT"] = json!(2);
        let mut store = runtime_store(vec![first, second]);
        let mut accounts = AccountStore::default();
        accounts.upsert(codex_oauth_account("acct-1", None));
        accounts.upsert(codex_oauth_account("acct-2", None));
        accounts
            .select_active_codex_oauth_account("acct-2")
            .unwrap();
        store.rebuild_runtime_index(&accounts).unwrap();
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

        let selected = select_test_provider(
            &store,
            &accounts,
            AppKind::Codex,
            "p2",
            Some(&tracker.snapshot()),
        )
        .unwrap();

        assert_eq!(selected.execution.stored.provider.id, "p2");
        assert_eq!(
            account_concurrency_for_provider(
                &selected.execution.stored,
                &accounts,
                &tracker.snapshot()
            )
            .unwrap()
            .max_concurrent,
            2
        );
    }

    #[test]
    fn repeated_bound_surface_selection_remains_stable() {
        let store = runtime_store(vec![
            claude_oauth_provider("p1", "acct-1", Some(8)),
            claude_oauth_provider("p2", "acct-2", Some(8)),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("acct-1"));
        accounts.upsert(claude_oauth_account("acct-2"));
        let snapshot = AccountInFlightTracker::default().snapshot();

        let first = select_test_provider(&store, &accounts, AppKind::Claude, "p1", Some(&snapshot))
            .unwrap();
        let second =
            select_test_provider(&store, &accounts, AppKind::Claude, "p1", Some(&snapshot))
                .unwrap();

        assert_eq!(first.execution.stored.provider.id, "p1");
        assert_eq!(second.execution.stored.provider.id, "p1");
    }

    #[test]
    fn failover_selection_uses_authoritative_order_and_exclusions() {
        let mut store = runtime_store(vec![
            claude_api_key_provider("p1"),
            claude_api_key_provider("p2"),
            claude_api_key_provider("p3"),
        ]);
        store.order.insert(
            AppKind::Claude,
            vec!["p3".to_string(), "p1".to_string(), "p2".to_string()],
        );
        let accounts = AccountStore::default();
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
    fn failover_selection_skips_managed_provider_candidates() {
        let store = runtime_store(vec![
            claude_api_key_provider("failed"),
            claude_oauth_provider("managed", "managed-account", None),
            claude_api_key_provider("backup"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("managed-account"));

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
    fn failover_selection_stops_after_managed_provider_origin() {
        let store = runtime_store(vec![
            claude_oauth_provider("failed-managed", "managed-account", None),
            claude_api_key_provider("backup"),
        ]);
        let mut accounts = AccountStore::default();
        accounts.upsert(claude_oauth_account("managed-account"));

        let selected = select_failover_provider(
            &store,
            &accounts,
            ProxyRoute::ClaudeMessages,
            &AccountInFlightTracker::default().snapshot(),
            &BTreeSet::from(["failed-managed".to_string()]),
        );

        assert!(selected.is_none());
    }

    #[test]
    fn failover_selection_never_uses_another_managed_account() {
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
        );

        assert!(selected.is_none());
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

        let selected = select_test_provider(
            &store,
            &accounts,
            AppKind::Claude,
            "claude-native",
            Some(&tracker.snapshot()),
        )
        .unwrap();
        assert_eq!(selected.execution.stored.provider.id, "claude-native");
        assert!(provider_supports_claude_count_tokens(
            &selected.execution.stored
        ));

        let selected = select_test_provider(
            &store,
            &accounts,
            AppKind::Claude,
            "codex-first",
            Some(&tracker.snapshot()),
        )
        .unwrap();
        assert!(!provider_supports_claude_count_tokens(
            &selected.execution.stored
        ));
    }
}
