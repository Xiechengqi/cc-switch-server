use super::*;
use std::collections::BTreeMap;

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

pub(in crate::api) async fn backfill_usage_costs(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UsageBackfillResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let pricing = state.pricing.read().await.clone();
    let updated = state
        .usage
        .write()
        .await
        .backfill_costs(&providers, &pricing);
    state.save_usage().await.map_err(ApiError::internal)?;
    Ok(Json(UsageBackfillResponse { ok: true, updated }))
}

pub(in crate::api) async fn list_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ModelPricingListResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ModelPricingListResponse {
        ok: true,
        models: state.pricing.read().await.list(),
    }))
}

pub(in crate::api) async fn upsert_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateModelPricingInput>,
) -> Result<Json<ModelPricingUpdateResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let model_id = input
        .model_id
        .clone()
        .ok_or_else(|| ApiError::bad_request("modelId is required"))?;
    update_model_pricing_inner(state, model_id, input).await
}

pub(in crate::api) async fn update_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
    Json(input): Json<UpdateModelPricingInput>,
) -> Result<Json<ModelPricingUpdateResponse>, ApiError> {
    require_session(&state, &headers).await?;
    update_model_pricing_inner(state, model_id, input).await
}

pub(in crate::api) async fn update_model_pricing_inner(
    state: ServerState,
    model_id: String,
    input: UpdateModelPricingInput,
) -> Result<Json<ModelPricingUpdateResponse>, ApiError> {
    let entry = state
        .try_mutate_pricing_immediate(|pricing| pricing.upsert(model_id, input))
        .await
        .map_err(ApiError::internal)?
        .map_err(ApiError::bad_request)?;

    let providers = state.providers.read().await.clone();
    let pricing = state.pricing.read().await.clone();
    let updated =
        state
            .usage
            .write()
            .await
            .backfill_costs_for_model(&providers, &pricing, &entry.model_id);
    if updated > 0 {
        state.save_usage().await.map_err(ApiError::internal)?;
    }

    Ok(Json(ModelPricingUpdateResponse {
        ok: true,
        model: entry,
        backfilled: updated,
    }))
}

pub(in crate::api) async fn delete_model_pricing(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(model_id): Path<String>,
) -> Result<Json<ModelPricingDeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = state
        .mutate_pricing_immediate(|pricing| pricing.delete(&model_id))
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ModelPricingDeleteResponse { ok: true, deleted }))
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
    let usage = state.usage.read().await.clone();
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
        .map(|provider| provider_limit_status(provider, &accounts, &shares, &usage))
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
    let usage = state.usage.read().await.clone();
    Ok(Json(ProviderLimitResponse {
        ok: true,
        limit: provider_limit_status(&provider, &accounts, &shares, &usage),
    }))
}

pub(in crate::api) fn provider_limit_status(
    provider: &StoredProvider,
    accounts: &AccountStore,
    shares: &ShareStore,
    usage: &UsageStore,
) -> ProviderLimitStatusView {
    let daily_limit_usd = provider_number_setting(provider, &["limitDailyUsd", "dailyLimitUsd"]);
    let monthly_limit_usd =
        provider_number_setting(provider, &["limitMonthlyUsd", "monthlyLimitUsd"]);
    let quota_dispatch_limit_percent = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.quota_dispatch_limit_percent)
        .map(|value| value as f64)
        .or_else(|| {
            provider_number_setting(
                provider,
                &["quotaDispatchLimitPercent", "quota_dispatch_limit_percent"],
            )
        });
    let daily_usage_usd = provider_usage_cost_since(
        usage,
        provider,
        current_utc_day_start_ms().unwrap_or(0) as u128,
    );
    let monthly_usage_usd = provider_usage_cost_since(
        usage,
        provider,
        current_utc_month_start_ms().unwrap_or(0) as u128,
    );
    let daily_exceeded = daily_limit_usd
        .map(|limit| daily_usage_usd >= limit)
        .unwrap_or(false);
    let monthly_exceeded = monthly_limit_usd
        .map(|limit| monthly_usage_usd >= limit)
        .unwrap_or(false);
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
    let quota_dispatch_exceeded = quota_dispatch_limit_percent
        .zip(account_quota_percent)
        .map(|(limit, quota)| quota >= limit)
        .unwrap_or(false);
    let share_limits = shares
        .shares
        .iter()
        .filter(|share| share_uses_provider(share, provider))
        .map(share_limit_status)
        .collect::<Vec<_>>();
    let share_blocked = share_limits.iter().any(|share| share.blocked);

    let mut warnings = Vec::new();
    if daily_exceeded {
        warnings.push("daily_cost_limit_exceeded".to_string());
    }
    if monthly_exceeded {
        warnings.push("monthly_cost_limit_exceeded".to_string());
    }
    if quota_dispatch_exceeded {
        warnings.push("quota_dispatch_limit_exceeded".to_string());
    }
    if account
        .as_ref()
        .and_then(|account| account.last_refresh_error.clone())
        .is_some()
    {
        warnings.push("account_quota_refresh_error".to_string());
    }
    if share_blocked {
        warnings.push("share_limit_blocks_usage".to_string());
    }
    if account
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
        daily_usage_usd,
        daily_limit_usd,
        daily_exceeded,
        monthly_usage_usd,
        monthly_limit_usd,
        monthly_exceeded,
        account_id: account.as_ref().map(|account| account.id.clone()),
        account_email: account.as_ref().and_then(|account| account.email.clone()),
        account_quota_percent,
        account_quota_refreshed_at: account
            .as_ref()
            .and_then(|account| account.quota_refreshed_at),
        account_last_refresh_error: account
            .as_ref()
            .and_then(|account| account.last_refresh_error.clone()),
        quota_dispatch_limit_percent,
        quota_dispatch_exceeded,
        shares: share_limits,
        warnings,
        blocked: daily_exceeded || monthly_exceeded || quota_dispatch_exceeded || share_blocked,
    }
}

pub(in crate::api) fn provider_usage_cost_since(
    usage: &UsageStore,
    provider: &StoredProvider,
    start_ms: u128,
) -> f64 {
    usage.provider_cost_since(provider.app, &provider.provider.id, start_ms)
}

pub(in crate::api) fn provider_number_setting(
    provider: &StoredProvider,
    keys: &[&str],
) -> Option<f64> {
    provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| map_number_value(&meta.extra, keys))
        .or_else(|| map_number_value(&provider.provider.extra, keys))
        .or_else(|| value_number_setting(&provider.provider.settings_config, keys))
        .or_else(|| {
            provider
                .provider
                .settings_config
                .get("limits")
                .and_then(|value| value_number_setting(value, keys))
        })
        .or_else(|| {
            provider
                .provider
                .settings_config
                .get("usageLimits")
                .and_then(|value| value_number_setting(value, keys))
        })
}

pub(in crate::api) fn map_number_value(
    map: &BTreeMap<String, Value>,
    keys: &[&str],
) -> Option<f64> {
    keys.iter()
        .find_map(|key| map.get(*key).and_then(value_as_f64))
}

pub(in crate::api) fn value_number_setting(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(value_as_f64))
}

pub(in crate::api) fn value_as_f64(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
    })
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

pub(in crate::api) fn current_utc_day_start_ms() -> Option<i64> {
    chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|value| value.and_utc().timestamp_millis())
}

pub(in crate::api) fn current_utc_month_start_ms() -> Option<i64> {
    let now = chrono::Utc::now();
    chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|value| value.and_utc().timestamp_millis())
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
