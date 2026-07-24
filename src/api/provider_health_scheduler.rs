use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::{stream, StreamExt};

use crate::domain::accounts::store::{AccountStore, AccountUsageBlock};
use crate::domain::health::{
    provider_probe_support, ProviderHealthObservation, ProviderHealthSnapshot,
    ProviderHealthStatus, ProviderProbeSupport, PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS,
};
use crate::domain::providers::current_provider::resolve_current_provider_id;
use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::model_health::{
    quota_block_for_provider, quota_block_message, share_bindings,
};
use crate::domain::sharing::shares::Share;
use crate::domain::stream_check::{HealthStatus, StreamCheckConfig, StreamCheckResult};
use crate::domain::usage::store::{UsageLog, UsageLogContext, UsageModelMetadata};
use crate::infra::time::now_ms;
use crate::state::ServerState;

use super::{
    map_provider_test_to_stream_check_result, provider_test_model, redact_provider_test_error,
    resolve_provider_execution_by_key, test_provider_inner, web_stream_check_config,
    TestProviderQuery, TestProviderResponse,
};

const FIRST_HEALTH_CHECK_DELAY: Duration = Duration::from_secs(120);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const TRANSIENT_CONFIRMATION_DELAY: Duration =
    Duration::from_millis(PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS as u64);
const QUOTA_BLOCK_REPEAT_INTERVAL_MS: u128 = 6 * 60 * 60 * 1000;
const PROBE_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_CONCURRENT_PROBES: usize = 3;
const SCHEDULED_SOURCE: &str = "cc-switch-scheduled";
const CONFIRMATION_SOURCE: &str = "cc-switch-scheduled-confirmation";
const QUOTA_SOURCE: &str = "cc-switch-quota";

#[derive(Debug, Clone)]
pub(crate) struct ShareBindingHealthCheck {
    pub(crate) result: StreamCheckResult,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) quota_blocked: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RecordedProviderProbe {
    pub(crate) result: StreamCheckResult,
    pub(crate) probe_support: ProviderProbeSupport,
    pub(crate) snapshot: Option<ProviderHealthSnapshot>,
}

#[derive(Debug, Clone)]
struct HealthTarget {
    provider: StoredProvider,
    shares: Vec<Share>,
}

pub(in crate::api) fn spawn_share_model_health_scheduler(state: ServerState) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_HEALTH_CHECK_DELAY).await;
        loop {
            run_share_model_health_cycle(&state).await;
            tokio::time::sleep(HEALTH_CHECK_INTERVAL).await;
        }
    });
}

pub(crate) async fn run_share_model_health_cycle(state: &ServerState) {
    let shares = state.shares.read().await.shares.clone();
    let providers = state.providers_snapshot().await;
    let ui_settings = state.ui_settings.read().await.for_frontend();
    let accounts = state.accounts_snapshot().await;
    let config = web_stream_check_config(state).await;

    if let Err(error) = state.prune_provider_health_snapshots().await {
        tracing::warn!(error = %error, "failed to prune Provider health snapshots");
    }

    let targets = health_targets(&shares, &providers, &ui_settings);
    if targets.is_empty() {
        tracing::debug!("no active Provider targets require a model health check");
        return;
    }
    let target_count = targets.len();
    let results = stream::iter(targets.into_values().map(|target| {
        let state = state.clone();
        let accounts = accounts.clone();
        let config = config.clone();
        async move { process_initial_health_target(&state, target, &accounts, &config).await }
    }))
    .buffer_unordered(MAX_CONCURRENT_PROBES)
    .collect::<Vec<_>>()
    .await;
    let failures = results.iter().filter(|result| result.is_err()).count();
    let mut confirmations = Vec::new();
    for result in results {
        match result {
            Ok(Some(target)) => confirmations.push(target),
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(error = %error, "scheduled Provider health target failed");
            }
        }
    }
    let confirmation_count = confirmations.len();
    if confirmation_count > 0 {
        tokio::time::sleep(TRANSIENT_CONFIRMATION_DELAY).await;
        let confirmation_results = stream::iter(confirmations.into_iter().map(|target| {
            let state = state.clone();
            let config = config.clone();
            async move { confirm_health_target(&state, target, &config).await }
        }))
        .buffer_unordered(MAX_CONCURRENT_PROBES)
        .collect::<Vec<_>>()
        .await;
        for error in confirmation_results.into_iter().filter_map(Result::err) {
            tracing::warn!(error = %error, "scheduled Provider health confirmation failed");
        }
    }
    tracing::info!(
        targets = target_count,
        failures,
        confirmations = confirmation_count,
        "scheduled Provider health cycle completed"
    );
}

fn health_targets(
    shares: &[Share],
    providers: &ProviderStore,
    ui_settings: &serde_json::Value,
) -> BTreeMap<(AppKind, String), HealthTarget> {
    let mut targets = BTreeMap::<(AppKind, String), HealthTarget>::new();
    for share in shares
        .iter()
        .filter(|share| share.enabled && share.status == "active")
    {
        for (app, provider_id) in share_bindings(share) {
            let Some(provider) = providers
                .providers
                .iter()
                .find(|provider| provider.app == app && provider.provider.id == provider_id)
                .cloned()
            else {
                tracing::warn!(
                    share_id = %share.id,
                    app = app.as_str(),
                    provider_id,
                    "share model health binding Provider was not found"
                );
                continue;
            };
            let target = targets
                .entry((app, provider_id))
                .or_insert_with(|| HealthTarget {
                    provider,
                    shares: Vec::new(),
                });
            if !target.shares.iter().any(|existing| existing.id == share.id) {
                target.shares.push(share.clone());
            }
        }
    }

    for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
        let Some(provider_id) = resolve_current_provider_id(providers, ui_settings, app) else {
            continue;
        };
        let key = (app, provider_id.clone());
        if targets.contains_key(&key) {
            continue;
        }
        if let Some(provider) = providers
            .providers
            .iter()
            .find(|provider| provider.app == app && provider.provider.id == provider_id)
            .cloned()
        {
            targets.insert(
                key,
                HealthTarget {
                    provider,
                    shares: Vec::new(),
                },
            );
        }
    }
    targets
}

async fn process_initial_health_target(
    state: &ServerState,
    target: HealthTarget,
    accounts: &AccountStore,
    config: &StreamCheckConfig,
) -> anyhow::Result<Option<HealthTarget>> {
    if let Some(block) = quota_block_for_provider(&target.provider, Some(accounts)) {
        let active_shares = current_active_shares_for_provider(state, &target.provider).await;
        for share in &active_shares {
            record_quota_block(state, share, &target.provider, config, &block).await?;
            notify_runtime_refresh(state, share).await;
        }
        return Ok(None);
    }

    let probe =
        probe_provider_and_record(state, &target.provider, config, SCHEDULED_SOURCE).await?;
    if probe.probe_support == ProviderProbeSupport::Unsupported {
        tracing::debug!(
            app = target.provider.app.as_str(),
            provider_id = %target.provider.provider.id,
            "skipped scheduled Provider health check because its Driver does not support testing"
        );
        return Ok(None);
    }
    let Some(snapshot) = probe.snapshot.as_ref() else {
        tracing::debug!(
            app = target.provider.app.as_str(),
            provider_id = %target.provider.provider.id,
            "discarded scheduled Provider health projection because the runtime changed during the probe"
        );
        return Ok(None);
    };
    let active_shares = current_active_shares_for_provider(state, &target.provider).await;
    project_probe_to_shares(
        state,
        &active_shares,
        &target.provider,
        &probe.result,
        SCHEDULED_SOURCE,
    )
    .await?;

    if !snapshot.confirmation_pending {
        return Ok(None);
    }

    let mut target = target;
    target.provider.resource.revision = snapshot.provider_revision;
    Ok(Some(target))
}

async fn confirm_health_target(
    state: &ServerState,
    target: HealthTarget,
    config: &StreamCheckConfig,
) -> anyhow::Result<()> {
    let providers = state.providers_snapshot().await;
    let Some(current) = providers
        .providers
        .iter()
        .find(|provider| {
            provider.app == target.provider.app
                && provider.provider.id == target.provider.provider.id
                && provider.resource.revision == target.provider.resource.revision
        })
        .cloned()
    else {
        return Ok(());
    };
    let runtime_plan = providers.runtime_plan(current.app, &current.provider.id);
    let still_pending = {
        let usage = state.usage.read().await;
        crate::domain::health::provider_health_for_plan(&current, &usage, runtime_plan.as_deref())
            .confirmation_pending
    };
    if !still_pending {
        return Ok(());
    }
    let accounts = state.accounts_snapshot().await;
    if quota_block_for_provider(&current, Some(&accounts)).is_some() {
        return Ok(());
    }
    let confirmation =
        probe_provider_and_record(state, &current, config, CONFIRMATION_SOURCE).await?;
    if confirmation.probe_support == ProviderProbeSupport::Supported
        && confirmation.snapshot.is_some()
    {
        let active_shares = current_active_shares_for_provider(state, &current).await;
        project_probe_to_shares(
            state,
            &active_shares,
            &current,
            &confirmation.result,
            CONFIRMATION_SOURCE,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn probe_provider_and_record(
    state: &ServerState,
    provider: &StoredProvider,
    config: &StreamCheckConfig,
    source: &str,
) -> anyhow::Result<RecordedProviderProbe> {
    let Some(plan) = state
        .provider_runtime_plan(provider.app, &provider.provider.id)
        .await
    else {
        let result = failed_probe_result(
            provider,
            provider_test_model(provider.app, provider, None, Some(config)),
            "Provider runtime plan is unavailable".to_string(),
            "invalidConfig",
            None,
            0,
        );
        let snapshot = record_probe_observation(state, provider, "", &result, source).await?;
        return Ok(RecordedProviderProbe {
            result,
            probe_support: ProviderProbeSupport::Supported,
            snapshot,
        });
    };
    let support = provider_probe_support(&plan);
    if support == ProviderProbeSupport::Unsupported {
        return Ok(RecordedProviderProbe {
            result: failed_probe_result(
                provider,
                provider_test_model(provider.app, provider, None, Some(config)),
                format!("driver {} does not support test", plan.driver_id),
                "unsupported",
                None,
                0,
            ),
            probe_support: support,
            snapshot: None,
        });
    }

    let model = provider_test_model(provider.app, provider, None, Some(config));
    let query = TestProviderQuery {
        app: provider.app,
        network: Some(true),
        timeout_ms: Some(config.timeout_secs.saturating_mul(1000)),
        model: Some(model.clone()),
        stream: Some(true),
    };
    let (result, runtime_fingerprint) = run_probe_with_retries(
        state,
        provider,
        &query,
        config,
        &model,
        &plan.runtime_fingerprint,
    )
    .await;
    if !probe_matches_target_generation(
        provider.resource.revision,
        &plan.runtime_fingerprint,
        &result,
        &runtime_fingerprint,
    ) {
        return Ok(RecordedProviderProbe {
            result,
            probe_support: support,
            snapshot: None,
        });
    }
    let snapshot =
        record_probe_observation(state, provider, &runtime_fingerprint, &result, source).await?;
    Ok(RecordedProviderProbe {
        result,
        probe_support: support,
        snapshot,
    })
}

pub(crate) async fn record_probe_observation(
    state: &ServerState,
    provider: &StoredProvider,
    runtime_fingerprint: &str,
    result: &StreamCheckResult,
    source: &str,
) -> anyhow::Result<Option<ProviderHealthSnapshot>> {
    let status = match result.status {
        HealthStatus::Operational => ProviderHealthStatus::Healthy,
        HealthStatus::Degraded => ProviderHealthStatus::Degraded,
        HealthStatus::Failed => ProviderHealthStatus::Unhealthy,
    };
    state
        .record_provider_health_observation(ProviderHealthObservation {
            app: provider.app,
            provider_id: provider.provider.id.clone(),
            provider_revision: result
                .provider_revision
                .unwrap_or(provider.resource.revision),
            runtime_fingerprint: runtime_fingerprint.to_string(),
            status,
            checked_at_ms: now_ms(),
            source: source.to_string(),
            status_code: result.http_status,
            latency_ms: result.response_time_ms,
            model: (!result.model_used.trim().is_empty()).then(|| result.model_used.clone()),
            error_category: result.error_category.clone(),
            error_message: (!result.success).then(|| redact_provider_test_error(&result.message)),
            transient_failure: !result.success
                && result
                    .error_category
                    .as_deref()
                    .is_some_and(is_transient_probe_category),
        })
        .await
}

pub(crate) async fn record_provider_test_response(
    state: &ServerState,
    provider: &StoredProvider,
    response: &TestProviderResponse,
    config: &StreamCheckConfig,
    source: &str,
) -> anyhow::Result<Option<ProviderHealthSnapshot>> {
    if !response.network_checked || response.outcome == super::ProviderOperationOutcome::Unsupported
    {
        return Ok(None);
    }
    let result = map_provider_test_to_stream_check_result(response, config);
    let snapshot = record_probe_observation(
        state,
        provider,
        &response.runtime_fingerprint,
        &result,
        source,
    )
    .await?;
    if snapshot.is_some() {
        project_accepted_probe_to_active_shares(state, provider, &result, source).await?;
    }
    Ok(snapshot)
}

pub(crate) async fn project_recorded_probe_to_active_shares(
    state: &ServerState,
    provider: &StoredProvider,
    probe: &RecordedProviderProbe,
    source: &str,
) -> anyhow::Result<usize> {
    if probe.probe_support == ProviderProbeSupport::Unsupported || probe.snapshot.is_none() {
        return Ok(0);
    }
    project_accepted_probe_to_active_shares(state, provider, &probe.result, source).await
}

pub(crate) async fn check_share_binding(
    state: &ServerState,
    share: &Share,
    provider: &StoredProvider,
    accounts: &AccountStore,
    config: &StreamCheckConfig,
    source: &str,
) -> anyhow::Result<ShareBindingHealthCheck> {
    if let Some(block) = quota_block_for_provider(provider, Some(accounts)) {
        return record_quota_block(state, share, provider, config, &block).await;
    }

    let probe = probe_provider_and_record(state, provider, config, source).await?;
    if probe.probe_support == ProviderProbeSupport::Supported && probe.snapshot.is_some() {
        state
            .push_usage_log(health_usage_log(
                share,
                provider,
                &probe.result,
                source,
                true,
            ))
            .await?;
    }
    Ok(ShareBindingHealthCheck {
        result: probe.result,
        provider_id: provider.provider.id.clone(),
        provider_name: provider.provider.name.clone(),
        quota_blocked: false,
    })
}

async fn project_probe_to_shares(
    state: &ServerState,
    shares: &[Share],
    provider: &StoredProvider,
    result: &StreamCheckResult,
    source: &str,
) -> anyhow::Result<()> {
    for share in shares {
        state
            .push_usage_log(health_usage_log(share, provider, result, source, true))
            .await?;
        notify_runtime_refresh(state, share).await;
    }
    Ok(())
}

async fn project_accepted_probe_to_active_shares(
    state: &ServerState,
    provider: &StoredProvider,
    result: &StreamCheckResult,
    source: &str,
) -> anyhow::Result<usize> {
    let shares = current_active_shares_for_provider(state, provider).await;
    let projected = shares.len();
    project_probe_to_shares(state, &shares, provider, result, source).await?;
    Ok(projected)
}

async fn current_active_shares_for_provider(
    state: &ServerState,
    provider: &StoredProvider,
) -> Vec<Share> {
    let shares = state.shares.read().await.shares.clone();
    active_shares_for_provider(&shares, provider.app, &provider.provider.id)
}

fn active_shares_for_provider(shares: &[Share], app: AppKind, provider_id: &str) -> Vec<Share> {
    shares
        .iter()
        .filter(|share| share.enabled && share.status == "active")
        .filter(|share| {
            share_bindings(share)
                .iter()
                .any(|(binding_app, binding_provider_id)| {
                    *binding_app == app && binding_provider_id == provider_id
                })
        })
        .cloned()
        .collect()
}

async fn run_probe_with_retries(
    state: &ServerState,
    provider: &StoredProvider,
    query: &TestProviderQuery,
    config: &StreamCheckConfig,
    model: &str,
    fallback_runtime_fingerprint: &str,
) -> (StreamCheckResult, String) {
    let execution =
        match resolve_provider_execution_by_key(state, provider.app, &provider.provider.id).await {
            Ok(execution) => execution,
            Err(error) => {
                return (
                    failed_probe_result(
                        provider,
                        model.to_string(),
                        redact_provider_test_error(&error.message),
                        error.code.unwrap_or("invalidConfig"),
                        Some(error.status.as_u16()),
                        0,
                    ),
                    fallback_runtime_fingerprint.to_string(),
                );
            }
        };
    let runtime_fingerprint = execution.plan.runtime_fingerprint.clone();
    for attempt in 0..=config.max_retries {
        match test_provider_inner(state, execution.clone(), query).await {
            Ok(response) => {
                let retry = !probe_succeeded(&response)
                    && retryable_probe(
                        response.network_status_code,
                        response.network_stream_completed,
                        response.network_error.is_some(),
                    )
                    && attempt < config.max_retries;
                let mut result = map_provider_test_to_stream_check_result(&response, config);
                result.retry_count = attempt;
                if result.error_category.is_none() && !result.success {
                    result.error_category = probe_error_category(
                        response.network_status_code,
                        response.network_stream_completed,
                    );
                }
                if !retry {
                    return (result, runtime_fingerprint);
                }
            }
            Err(error) => {
                let retry = (error.retryable.unwrap_or(false)
                    || error.status.as_u16() == 429
                    || error.status.is_server_error())
                    && attempt < config.max_retries;
                if !retry {
                    let category = error
                        .code
                        .unwrap_or_else(|| category_for_status(error.status.as_u16()));
                    return (
                        failed_probe_result(
                            provider,
                            model.to_string(),
                            redact_provider_test_error(&error.message),
                            category,
                            Some(error.status.as_u16()),
                            attempt,
                        ),
                        runtime_fingerprint,
                    );
                }
            }
        }
        tokio::time::sleep(PROBE_RETRY_DELAY).await;
    }

    unreachable!("probe loop always returns on its final attempt")
}

fn failed_probe_result(
    provider: &StoredProvider,
    model: String,
    message: String,
    category: &str,
    status: Option<u16>,
    retry_count: u32,
) -> StreamCheckResult {
    StreamCheckResult {
        status: HealthStatus::Failed,
        success: false,
        provider_revision: Some(provider.resource.revision),
        message,
        response_time_ms: None,
        http_status: status,
        model_used: model,
        tested_at: chrono::Utc::now().timestamp(),
        retry_count,
        error_category: Some(category.to_string()),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    }
}

fn probe_matches_target_generation(
    expected_revision: u64,
    expected_runtime_fingerprint: &str,
    result: &StreamCheckResult,
    actual_runtime_fingerprint: &str,
) -> bool {
    result.provider_revision.unwrap_or(expected_revision) == expected_revision
        && actual_runtime_fingerprint == expected_runtime_fingerprint
}

async fn record_quota_block(
    state: &ServerState,
    share: &Share,
    provider: &StoredProvider,
    config: &StreamCheckConfig,
    block: &AccountUsageBlock,
) -> anyhow::Result<ShareBindingHealthCheck> {
    let model = provider_test_model(provider.app, provider, None, Some(config));
    let result = StreamCheckResult {
        status: HealthStatus::Failed,
        success: false,
        provider_revision: Some(provider.resource.revision),
        message: quota_block_message(block),
        response_time_ms: Some(0),
        http_status: Some(429),
        model_used: model,
        tested_at: chrono::Utc::now().timestamp(),
        retry_count: 0,
        error_category: Some("quotaBlocked".to_string()),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    };
    let log = health_usage_log(share, provider, &result, QUOTA_SOURCE, false);
    let persisted = state
        .push_health_usage_log_if_due(log, QUOTA_BLOCK_REPEAT_INTERVAL_MS)
        .await?;
    let mut result = result;
    result.tested_at = seconds_from_ms(persisted.created_at_ms);
    result.message = quota_block_message(block);
    result.model_used = persisted
        .requested_model
        .or(persisted.model)
        .unwrap_or(result.model_used);
    Ok(ShareBindingHealthCheck {
        result,
        provider_id: provider.provider.id.clone(),
        provider_name: provider.provider.name.clone(),
        quota_blocked: true,
    })
}

fn health_usage_log(
    share: &Share,
    provider: &StoredProvider,
    result: &StreamCheckResult,
    source: &str,
    streaming: bool,
) -> UsageLog {
    let mut log = UsageLog::new(
        provider.app,
        provider.provider.id.clone(),
        provider.provider.name.clone(),
        provider.provider_type,
        result.http_status.unwrap_or(599),
        u128::from(result.response_time_ms.unwrap_or(0)),
        UsageModelMetadata {
            model: Some(result.model_used.clone()),
            requested_model: Some(result.model_used.clone()),
            ..UsageModelMetadata::default()
        },
        Default::default(),
    );
    log.error_message = (!result.success).then(|| redact_provider_test_error(&result.message));
    log.apply_context(UsageLogContext {
        share_id: Some(share.id.clone()),
        share_name: share.display_name.clone(),
        data_source: Some(source.to_string()),
        is_health_check: true,
        is_streaming: streaming,
        stream_status: streaming.then(|| {
            if result.success {
                "completed".to_string()
            } else {
                "failed".to_string()
            }
        }),
        ..UsageLogContext::default()
    });
    log
}

fn probe_succeeded(response: &TestProviderResponse) -> bool {
    response.network_checked
        && response.network_error.is_none()
        && response
            .network_status_code
            .is_some_and(|status| (200..400).contains(&status))
        && response.network_stream_completed.unwrap_or(true)
}

fn retryable_probe(
    status: Option<u16>,
    stream_completed: Option<bool>,
    has_network_error: bool,
) -> bool {
    stream_completed == Some(false)
        || status == Some(408)
        || status == Some(429)
        || status.is_some_and(|status| status >= 500)
        || (status.is_none() && has_network_error)
}

fn probe_error_category(status: Option<u16>, stream_completed: Option<bool>) -> Option<String> {
    if stream_completed == Some(false) {
        Some("streamIncomplete".to_string())
    } else if status == Some(408) {
        Some("timeout".to_string())
    } else if status == Some(429) {
        Some("rateLimit".to_string())
    } else if status.is_some_and(|status| status >= 500) {
        Some("upstream".to_string())
    } else if status.is_none() {
        Some("network".to_string())
    } else {
        None
    }
}

fn category_for_status(status: u16) -> &'static str {
    match status {
        401 | 403 => "auth",
        404 => "modelNotFound",
        408 => "timeout",
        429 => "rateLimit",
        500..=599 => "upstream",
        _ => "protocol",
    }
}

fn is_transient_probe_category(category: &str) -> bool {
    matches!(
        category,
        "network" | "timeout" | "rateLimit" | "upstream" | "streamIncomplete"
    )
}

async fn notify_runtime_refresh(state: &ServerState, share: &Share) {
    let Some(subdomain) = share
        .tunnel_subdomain
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    let config = state.config_snapshot().await;
    if config.router_api_base().is_none() || !config.has_registered_router_identity() {
        return;
    }
    let http = state.http_client().await;
    if let Err(error) = crate::clients::router::client::notify_runtime_refresh(
        &http,
        &config,
        share.id.clone(),
        subdomain.to_string(),
    )
    .await
    {
        tracing::warn!(
            share_id = %share.id,
            subdomain,
            error = %error,
            "notify Router model health refresh failed"
        );
    }
}

fn seconds_from_ms(value: u128) -> i64 {
    (value / 1000).min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::providers::model::{Provider, ProviderType};
    use crate::domain::providers::store::ProviderResourceMetadata;

    fn provider() -> StoredProvider {
        provider_with(AppKind::Codex, "p1")
    }

    fn provider_with(app: AppKind, id: &str) -> StoredProvider {
        StoredProvider {
            app,
            provider: Provider {
                id: id.to_string(),
                name: "Provider".to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: ProviderType::Codex.as_str().to_string(),
            resource: ProviderResourceMetadata::default(),
        }
    }

    fn share(id: &str, provider_id: &str, enabled: bool, status: &str) -> Share {
        serde_json::from_value(json!({
            "id": id,
            "app": "codex",
            "providerId": provider_id,
            "providerType": "codex",
            "enabled": enabled,
            "status": status
        }))
        .unwrap()
    }

    #[test]
    fn retries_only_transient_or_incomplete_probe_failures() {
        assert!(retryable_probe(None, None, true));
        assert!(retryable_probe(Some(408), None, true));
        assert!(retryable_probe(Some(429), None, true));
        assert!(retryable_probe(Some(503), None, true));
        assert!(retryable_probe(Some(200), Some(false), true));
        assert!(!retryable_probe(Some(401), None, true));
        assert!(!retryable_probe(Some(404), None, true));
    }

    #[test]
    fn failure_categories_distinguish_transient_and_terminal_results() {
        for category in [
            "network",
            "timeout",
            "rateLimit",
            "upstream",
            "streamIncomplete",
        ] {
            assert!(is_transient_probe_category(category));
        }
        for category in [
            "auth",
            "invalidConfig",
            "missingCredential",
            "modelNotFound",
            "protocol",
        ] {
            assert!(!is_transient_probe_category(category));
        }
    }

    #[test]
    fn probe_projection_requires_the_original_provider_generation() {
        let provider = provider();
        let result = failed_probe_result(
            &provider,
            "gpt-test".to_string(),
            "failed".to_string(),
            "network",
            None,
            0,
        );

        assert!(probe_matches_target_generation(
            provider.resource.revision,
            "runtime-1",
            &result,
            "runtime-1",
        ));
        assert!(!probe_matches_target_generation(
            provider.resource.revision.saturating_add(1),
            "runtime-1",
            &result,
            "runtime-1",
        ));
        assert!(!probe_matches_target_generation(
            provider.resource.revision,
            "runtime-1",
            &result,
            "runtime-2",
        ));
    }

    #[test]
    fn seconds_conversion_saturates() {
        assert_eq!(seconds_from_ms(1_999), 1);
        assert_eq!(seconds_from_ms(u128::MAX), i64::MAX);
    }

    #[test]
    fn health_check_intervals_match_server_contract() {
        assert_eq!(FIRST_HEALTH_CHECK_DELAY, Duration::from_secs(120));
        assert_eq!(HEALTH_CHECK_INTERVAL, Duration::from_secs(30 * 60));
        assert_eq!(TRANSIENT_CONFIRMATION_DELAY, Duration::from_secs(60));
        assert_eq!(QUOTA_BLOCK_REPEAT_INTERVAL_MS, 6 * 60 * 60 * 1000);
        assert_eq!(MAX_CONCURRENT_PROBES, 3);
    }

    #[test]
    fn targets_deduplicate_provider_across_shares() {
        let providers = ProviderStore {
            providers: vec![provider()],
            ..Default::default()
        };
        let shares = [
            share("share-1", "p1", true, "active"),
            share("share-2", "p1", true, "active"),
        ];

        let targets = health_targets(&shares, &providers, &json!({}));
        assert_eq!(targets.len(), 1);
        assert_eq!(
            targets
                .get(&(AppKind::Codex, "p1".to_string()))
                .unwrap()
                .shares
                .len(),
            2
        );
    }

    #[test]
    fn targets_exclude_inactive_shares_and_unused_providers() {
        let providers = ProviderStore {
            providers: vec![provider(), provider_with(AppKind::Codex, "p2")],
            ..Default::default()
        };
        let shares = [
            share("active", "p1", true, "active"),
            share("paused", "p2", true, "paused"),
            share("disabled", "p2", false, "active"),
        ];
        let settings = json!({ "currentProviderCodex": "" });

        let targets = health_targets(&shares, &providers, &settings);

        assert_eq!(targets.len(), 1);
        assert!(targets.contains_key(&(AppKind::Codex, "p1".to_string())));
        assert!(!targets.contains_key(&(AppKind::Codex, "p2".to_string())));
    }

    #[test]
    fn targets_include_current_provider_without_an_active_share() {
        let providers = ProviderStore {
            providers: vec![provider(), provider_with(AppKind::Codex, "p2")],
            ..Default::default()
        };
        let shares = [share("paused", "p1", true, "paused")];
        let settings = json!({ "currentProviderCodex": "p2" });

        let targets = health_targets(&shares, &providers, &settings);

        assert_eq!(targets.len(), 1);
        let target = targets
            .get(&(AppKind::Codex, "p2".to_string()))
            .expect("current Provider should be checked");
        assert!(target.shares.is_empty());
    }

    #[test]
    fn manual_projection_only_targets_active_matching_shares() {
        let shares = [
            share("active", "p1", true, "active"),
            share("paused", "p1", true, "paused"),
            share("disabled", "p1", false, "active"),
            share("other", "p2", true, "active"),
        ];

        let selected = active_shares_for_provider(&shares, AppKind::Codex, "p1");

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "active");
    }
}
