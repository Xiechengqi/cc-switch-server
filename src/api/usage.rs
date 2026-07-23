use super::*;
use crate::domain::accounts::store::{active_account_usage_block, AccountUsageBlockKind};
pub(in crate::api) async fn usage_logs(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageLogsQuery>,
) -> Result<Json<UsageLogsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(UsageLogsResponse {
        ok: true,
        logs: state.usage.read().await.latest_filtered(query.into()),
    }))
}

pub(in crate::api) async fn usage_summary(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageSummaryResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageSummaryResponse {
        ok: true,
        summary: state.usage.read().await.rollup_filtered(&filter),
    }))
}

pub(in crate::api) async fn usage_trends(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageTrendsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageTrendsResponse {
        ok: true,
        trends: state.usage.read().await.trends(&filter),
    }))
}

pub(in crate::api) async fn usage_provider_stats(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageProviderStatsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageProviderStatsResponse {
        ok: true,
        providers: state.usage.read().await.provider_stats(&filter),
    }))
}

pub(in crate::api) async fn usage_model_stats(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<UsageStatsQuery>,
) -> Result<Json<UsageModelStatsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let filter = UsageStatsFilter::from(query);
    Ok(Json(UsageModelStatsResponse {
        ok: true,
        models: state.usage.read().await.model_stats(&filter),
    }))
}

pub(in crate::api) async fn usage_log_detail(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UsageLogDetailResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let log = state
        .usage
        .read()
        .await
        .request_detail(&id)
        .ok_or_else(|| ApiError::not_found("usage request not found"))?;
    Ok(Json(UsageLogDetailResponse { ok: true, log }))
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
    let account = accounts
        .find_for_provider(
            provider.provider_type,
            provider
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.account_id.as_deref()),
        )
        .cloned();
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
    let summary = crate::state::sync_pending_direct_share_logs(state, 200, true).await;
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
                        auth_identity_generation: None,
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
}
