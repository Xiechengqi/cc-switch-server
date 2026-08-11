use super::*;
use crate::domain::accounts::store::{
    active_account_usage_block, AccountStore, AccountUsageBlockKind,
};
use crate::domain::providers::runtime::authoritative_managed_account;
use crate::domain::usage::query::UsageQuery;

const DEFAULT_USAGE_RANGE_MS: u128 = 24 * 60 * 60 * 1_000;
const MAX_USAGE_RANGE_MS: u128 = 32 * 24 * 60 * 60 * 1_000;
const DEFAULT_USAGE_REQUEST_LIMIT: usize = 50;
const MAX_USAGE_REQUEST_LIMIT: usize = 200;
const MAX_USAGE_TREND_POINTS: u128 = 2_000;

pub(in crate::api) async fn usage_overview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageOverviewResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let (query, meta) = usage_query(params)?;
    let data = state.usage.read().await.query_overview(&query);
    Ok(Json(UsageDataResponse { data, meta }))
}

pub(in crate::api) async fn usage_trends(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageTrendsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let requested_window_ms = params.window_ms.map(u128::from);
    let (query, meta) = usage_query(params)?;
    let range_ms = meta.to_ms.saturating_sub(meta.from_ms);
    let window_ms = requested_window_ms.unwrap_or_else(|| {
        if range_ms <= DEFAULT_USAGE_RANGE_MS {
            60 * 60 * 1_000
        } else {
            24 * 60 * 60 * 1_000
        }
    });
    if !(60 * 1_000..=24 * 60 * 60 * 1_000).contains(&window_ms) {
        return Err(ApiError::bad_request(
            "windowMs must be between one minute and one day",
        ));
    }
    let aligned_from_ms = meta.from_ms - (meta.from_ms % window_ms);
    let point_count = meta
        .to_ms
        .saturating_sub(aligned_from_ms)
        .saturating_add(window_ms.saturating_sub(1))
        / window_ms;
    if point_count > MAX_USAGE_TREND_POINTS {
        return Err(ApiError::bad_request(
            "windowMs produces more than 2000 trend points for this range",
        ));
    }
    let data = state.usage.read().await.query_trends(&query, window_ms);
    Ok(Json(UsageDataResponse { data, meta }))
}

pub(in crate::api) async fn usage_facets(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageFacetsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let (query, meta) = usage_query(params)?;
    let data = state.usage.read().await.query_facets(&query);
    Ok(Json(UsageDataResponse { data, meta }))
}

pub(in crate::api) async fn usage_provider_bundles(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageProviderBundlesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let (query, meta) = usage_query(params)?;
    let data = state.usage.read().await.query_provider_bundles(&query);
    Ok(Json(UsageDataResponse { data, meta }))
}

pub(in crate::api) async fn usage_models(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageModelsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let (query, meta) = usage_query(params)?;
    let data = state.usage.read().await.query_models(&query);
    Ok(Json(UsageDataResponse { data, meta }))
}

pub(in crate::api) async fn usage_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let (query, meta) = usage_query(params)?;
    let data = state.usage.read().await.query_shares(&query);
    Ok(Json(UsageDataResponse { data, meta }))
}

pub(in crate::api) async fn usage_requests(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(params): Query<UsageQueryParams>,
) -> Result<Json<UsageRequestPageResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let cursor = normalized_optional(params.cursor.clone());
    let limit = params
        .limit
        .unwrap_or(DEFAULT_USAGE_REQUEST_LIMIT)
        .clamp(1, MAX_USAGE_REQUEST_LIMIT);
    let (query, meta) = usage_query(params)?;
    let (data, next_cursor, total) = state
        .usage
        .read()
        .await
        .query_requests(&query, cursor.as_deref(), limit)
        .map_err(|_| {
            ApiError::bad_request(
                "cursor does not identify a request in the current filtered range",
            )
        })?;
    Ok(Json(UsageRequestPageResponse {
        data,
        meta: UsageRequestPageMeta {
            from_ms: meta.from_ms,
            to_ms: meta.to_ms,
            generated_at_ms: meta.generated_at_ms,
            total,
            next_cursor,
        },
    }))
}

pub(in crate::api) async fn usage_request_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UsageRequestDetailResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let log = state
        .usage
        .read()
        .await
        .request_detail(&id)
        .ok_or_else(|| ApiError::not_found("usage request not found"))?;
    let generated_at_ms = now_ms();
    Ok(Json(UsageDataResponse {
        meta: UsageResponseMeta {
            from_ms: log.started_at_ms,
            to_ms: log.completed_at_ms.max(log.started_at_ms).saturating_add(1),
            generated_at_ms,
        },
        data: log,
    }))
}

fn usage_query(params: UsageQueryParams) -> Result<(UsageQuery, UsageResponseMeta), ApiError> {
    let generated_at_ms = now_ms();
    let to_ms = params
        .to_ms
        .map(u128::from)
        .unwrap_or_else(|| generated_at_ms.saturating_add(1));
    let from_ms = params
        .from_ms
        .map(u128::from)
        .unwrap_or_else(|| to_ms.saturating_sub(DEFAULT_USAGE_RANGE_MS));
    if from_ms >= to_ms {
        return Err(ApiError::bad_request("fromMs must be less than toMs"));
    }
    if to_ms.saturating_sub(from_ms) > MAX_USAGE_RANGE_MS {
        return Err(ApiError::bad_request(
            "Usage queries cannot exceed the 32-day detail retention window",
        ));
    }
    Ok((
        UsageQuery {
            from_ms: Some(from_ms),
            to_ms: Some(to_ms),
            app: params.app,
            bundle_id: normalized_optional(params.bundle_id),
            share_id: normalized_optional(params.share_id),
            user_email: normalized_optional(params.user_email)
                .map(|email| email.to_ascii_lowercase()),
            actual_model: normalized_optional(params.actual_model),
            outcome: params.outcome,
            usage_state: params.usage_state,
        },
        UsageResponseMeta {
            from_ms,
            to_ms,
            generated_at_ms,
        },
    ))
}

fn normalized_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(in crate::api) async fn provider_limits(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ProviderLimitsQuery>,
) -> Result<Json<ProviderLimitsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let shares = state.shares.read().await.clone();
    let limits = providers
        .providers
        .iter()
        .filter(|provider| query.app.is_none_or(|app| provider.app == app))
        .filter(|provider| {
            query
                .provider_id
                .as_deref()
                .is_none_or(|id| provider.provider.id == id)
        })
        .map(|provider| provider_limit_status(provider, &accounts, &shares))
        .collect::<Vec<_>>();
    Ok(Json(ProviderLimitsResponse { ok: true, limits }))
}

pub(in crate::api) async fn provider_limits_for_provider(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(provider_id): Path<String>,
    Query(query): Query<ProviderLimitsQuery>,
) -> Result<Json<ProviderLimitResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let provider = providers
        .providers
        .iter()
        .find(|provider| {
            provider.provider.id == provider_id && query.app.is_none_or(|app| provider.app == app)
        })
        .cloned()
        .ok_or_else(|| ApiError::not_found("provider not found"))?;
    let accounts = state.accounts.read().await.clone();
    let shares = state.shares.read().await.clone();
    Ok(Json(ProviderLimitResponse {
        ok: true,
        limit: provider_limit_status(&provider, &accounts, &shares),
    }))
}

pub(in crate::api) fn provider_limit_status(
    provider: &StoredProvider,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> ProviderLimitStatusView {
    let account = authoritative_managed_account(provider, accounts).cloned();
    let account_quota_percent = account
        .as_ref()
        .and_then(|account| account.quota_percent)
        .or_else(|| account.as_ref().and_then(account_tier_quota_percent));
    let share_limits = shares
        .shares
        .iter()
        .filter(|share| share_uses_provider(share, provider))
        .map(share_limit_status)
        .collect::<Vec<_>>();
    let share_blocked = share_limits.iter().any(|share| share.blocked);
    let account_usage_block = account.as_ref().and_then(|account| {
        active_account_usage_block(account, now_ms().min(i64::MAX as u128) as i64)
    });
    let account_blocked = account_usage_block.is_some();

    let mut warnings = Vec::new();
    if let Some(block) = account_usage_block.as_ref() {
        warnings.push(
            match block.kind {
                AccountUsageBlockKind::RateLimited => "account_rate_limited",
                AccountUsageBlockKind::QuotaExhausted => "account_quota_exhausted",
            }
            .to_string(),
        );
    } else if account
        .as_ref()
        .and_then(|account| account.last_refresh_error.clone())
        .is_some()
    {
        warnings.push("account_quota_refresh_error".to_string());
    }
    if share_blocked {
        warnings.push("share_limit_blocks_usage".to_string());
    }
    if !account_blocked
        && account
            .as_ref()
            .and_then(|account| account.quota.as_ref())
            .is_some_and(|quota| {
                quota
                    .tiers
                    .iter()
                    .filter_map(|tier| tier.utilization)
                    .any(|value| normalize_quota_utilization_percent(value) >= 95.0)
            })
    {
        warnings.push("account_quota_near_limit".to_string());
    }

    ProviderLimitStatusView {
        app: provider.app,
        provider_id: provider.provider.id.clone(),
        provider_name: provider.provider.name.clone(),
        provider_type: provider.provider_type,
        account_id: account.as_ref().map(|account| account.id.clone()),
        account_email: account.as_ref().and_then(|account| account.email.clone()),
        account_quota_percent,
        account_quota_refreshed_at: account
            .as_ref()
            .and_then(|account| account.quota_refreshed_at),
        account_last_refresh_error: account.as_ref().and_then(|account| {
            account
                .last_refresh_error
                .as_deref()
                .map(|error| redact_account_public_diagnostic(account, error))
        }),
        shares: share_limits,
        warnings,
        blocked: account_blocked || share_blocked,
    }
}

pub(in crate::api) fn account_tier_quota_percent(account: &Account) -> Option<f64> {
    account.quota.as_ref().and_then(|quota| {
        quota
            .tiers
            .iter()
            .filter_map(|tier| tier.utilization)
            .map(normalize_quota_utilization_percent)
            .max_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal))
    })
}

pub(in crate::api) fn normalize_quota_utilization_percent(value: f64) -> f64 {
    if value <= 1.0 {
        value * 100.0
    } else {
        value
    }
}

pub(in crate::api) fn share_uses_provider(share: &Share, provider: &StoredProvider) -> bool {
    (share.app == provider.app && share.provider_id == provider.provider.id)
        || share.bindings.iter().any(|binding| {
            binding.app == provider.app && binding.provider_id == provider.provider.id
        })
}

pub(in crate::api) fn share_limit_status(share: &Share) -> ShareLimitStatusView {
    let now = now_ms() as i64;
    let token_exceeded = share
        .token_limit
        .map(|limit| share.tokens_used >= limit)
        .unwrap_or(false);
    let expired = share
        .expires_at
        .map(|expires_at| expires_at <= now)
        .unwrap_or(false);
    let inactive = !share.enabled || share.status != "active";
    let blocked = inactive || token_exceeded || expired;
    let mut warnings = Vec::new();
    if inactive {
        warnings.push("share_inactive".to_string());
    }
    if token_exceeded {
        warnings.push("share_token_limit_exceeded".to_string());
    } else if let Some(limit) = share.token_limit {
        if limit > 0 && (share.tokens_used as f64 / limit as f64) >= 0.9 {
            warnings.push("share_token_limit_near".to_string());
        }
    }
    if expired {
        warnings.push("share_expired".to_string());
    }
    ShareLimitStatusView {
        share_id: share.id.clone(),
        share_name: share
            .display_name
            .clone()
            .unwrap_or_else(|| share.id.clone()),
        status: share.status.clone(),
        enabled: share.enabled,
        token_limit: share.token_limit,
        tokens_used: share.tokens_used,
        parallel_limit: share.parallel_limit,
        expires_at: share.expires_at,
        token_exceeded,
        expired,
        blocked,
        warnings,
    }
}

pub(in crate::api) async fn retry_usage_router_sync(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UsageRouterSyncRetryResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let summary = crate::state::sync_pending_router_share_logs(state, 200, true).await;
    Ok(Json(UsageRouterSyncRetryResponse {
        ok: true,
        attempted: summary.attempted,
        synced: summary.synced,
        failed: summary.failed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::accounts::store::UpsertAccountInput;
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta};
    use crate::domain::providers::store::ProviderResourceMetadata;

    fn account_provider() -> StoredProvider {
        StoredProvider {
            app: AppKind::Claude,
            provider: Provider {
                id: "kiro-provider".to_string(),
                name: "Kiro OAuth".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account_store".to_string()),
                        auth_provider: Some("kiro_oauth".to_string()),
                        account_id: Some("kiro-account".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
            provider_type: ProviderType::KiroOAuth,
            provider_type_id: ProviderType::KiroOAuth.as_str().to_string(),
            resource: ProviderResourceMetadata::default(),
        }
    }

    #[test]
    fn provider_limit_status_blocks_only_on_explicit_account_state() {
        let provider = account_provider();
        let mut accounts = AccountStore::default();
        let input: UpsertAccountInput = serde_json::from_value(json!({
            "id": "kiro-account",
            "providerType": "kiro_oauth",
            "quotaPercent": 100.0
        }))
        .unwrap();
        accounts.upsert(input);

        let display_only = provider_limit_status(&provider, &accounts, &ShareStore::default());
        assert!(!display_only.blocked);
        assert!(!display_only
            .warnings
            .contains(&"account_rate_limited".to_string()));

        accounts.mark_rate_limited_until("kiro-account", now_ms() as i64 + 60_000);
        let blocked = provider_limit_status(&provider, &accounts, &ShareStore::default());
        assert!(blocked.blocked);
        assert!(blocked
            .warnings
            .contains(&"account_rate_limited".to_string()));
        assert!(!blocked
            .warnings
            .contains(&"account_quota_refresh_error".to_string()));
    }

    #[test]
    fn provider_limit_status_does_not_select_default_for_unbound_managed_provider() {
        let mut provider = account_provider();
        provider.provider.meta.as_mut().unwrap().auth_binding = None;
        let mut accounts = AccountStore::default();
        accounts.upsert(
            serde_json::from_value(json!({
                "id": "kiro-account",
                "providerType": "kiro_oauth",
                "quotaPercent": 100.0
            }))
            .unwrap(),
        );
        accounts.mark_rate_limited_until("kiro-account", now_ms() as i64 + 60_000);

        let status = provider_limit_status(&provider, &accounts, &ShareStore::default());

        assert_eq!(status.account_id, None);
        assert!(!status.blocked);
    }

    #[test]
    fn provider_limit_status_ignores_stale_binding_for_metadata_account() {
        let mut provider = account_provider();
        provider.provider_type = ProviderType::CursorApiKey;
        provider.provider_type_id = ProviderType::CursorApiKey.as_str().to_string();
        let binding = provider
            .provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap();
        binding.auth_provider = Some(ProviderType::CursorApiKey.as_str().to_string());
        binding.account_id = Some("cursor-metadata".to_string());
        let mut accounts = AccountStore::default();
        accounts.upsert(
            serde_json::from_value(json!({
                "id": "cursor-metadata",
                "providerType": "cursor_apikey",
                "quotaPercent": 100.0
            }))
            .unwrap(),
        );
        accounts.mark_rate_limited_until("cursor-metadata", now_ms() as i64 + 60_000);

        let status = provider_limit_status(&provider, &accounts, &ShareStore::default());

        assert_eq!(status.account_id, None);
        assert!(!status.blocked);
    }
}
