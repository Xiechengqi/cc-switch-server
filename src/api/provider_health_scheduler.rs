use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use futures_util::{stream, StreamExt};

use crate::domain::accounts::store::{AccountStore, AccountUsageBlock};
use crate::domain::health::{
    provider_probe_support, ProviderHealthObservation, ProviderHealthSnapshot,
    ProviderHealthStatus, ProviderProbeSupport, PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS,
};
use crate::domain::providers::bundle::{
    bundle_test_app, is_explicit_bundle_surface, surface_enabled,
};
use crate::domain::providers::model::AppKind;
use crate::domain::providers::runtime::{
    build_provider_model_probe, ProviderHealthCheckConfig, ProviderModelProbe, RuntimeModelPolicy,
    PROVIDER_MODEL_PROBE_PROMPT,
};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::model_health::{
    quota_block_for_provider, quota_block_message, share_bindings,
};
use crate::domain::sharing::shares::{share_app_api_enabled, Share};
use crate::domain::stream_check::{HealthStatus, StreamCheckResult};
use crate::domain::usage::store::{
    UsageLog, UsageLogContext, UsageModelMetadata, UsageProviderHealthResult, UsageStore,
};
use crate::infra::time::now_ms;
use crate::state::ServerState;

use super::{
    map_provider_test_to_stream_check_result, provider_test_model, redact_provider_test_error,
    resolve_provider_execution_by_key, test_provider_inner, web_provider_health_check_config,
    ProviderOperationOutcome, TestProviderQuery, TestProviderResponse,
};

const FIRST_HEALTH_CHECK_DELAY: Duration = Duration::from_secs(120);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const TRANSIENT_CONFIRMATION_DELAY: Duration =
    Duration::from_millis(PROVIDER_HEALTH_TRANSIENT_CONFIRM_AFTER_MS as u64);
const QUOTA_BLOCK_REPEAT_INTERVAL_MS: u128 = 6 * 60 * 60 * 1000;
const ROUTER_PROBE_FALLBACK_AFTER_MS: u128 = 45 * 60 * 1000;
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
pub(crate) struct BatchShareBindingTarget {
    pub(crate) share: Share,
    pub(crate) provider: StoredProvider,
}

#[derive(Debug, Clone)]
pub(crate) struct BatchShareBindingHealthCheck {
    pub(crate) share_id: String,
    pub(crate) app: AppKind,
    pub(crate) check: ShareBindingHealthCheck,
    pub(crate) model_probe: ProviderModelProbe,
    pub(crate) model_policy: RuntimeModelPolicy,
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
    state
        .provider_health_cycle_pending
        .store(true, std::sync::atomic::Ordering::Release);
    let _cycle = state.provider_health_cycle.lock().await;
    while state
        .provider_health_cycle_pending
        .swap(false, std::sync::atomic::Ordering::AcqRel)
    {
        run_share_model_health_cycle_once(state).await;
    }
}

async fn run_share_model_health_cycle_once(state: &ServerState) {
    let shares = state.shares.read().await.shares.clone();
    let providers = state.providers_snapshot().await;
    let config = web_provider_health_check_config(state).await;

    if let Err(error) = state.prune_provider_health_snapshots().await {
        tracing::warn!(error = %error, "failed to prune Provider health snapshots");
    }

    let targets = health_targets(&shares, &providers);
    if targets.is_empty() {
        tracing::debug!("no active Provider targets require a model health check");
        return;
    }
    let target_count = targets.len();
    let results = stream::iter(targets.into_values().map(|target| {
        let state = state.clone();
        let config = config.clone();
        async move { process_initial_health_target(&state, target, &config).await }
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
) -> BTreeMap<(AppKind, String), HealthTarget> {
    let mut targets = BTreeMap::<(AppKind, String), HealthTarget>::new();
    for provider in providers
        .providers
        .iter()
        .filter(|provider| surface_enabled(&provider.provider))
    {
        if !is_health_target_surface(provider) {
            continue;
        }
        let provider = provider.clone();
        targets.insert(
            (provider.app, provider.provider.id.clone()),
            HealthTarget {
                provider,
                shares: Vec::new(),
            },
        );
    }
    for share in shares
        .iter()
        .filter(|share| share.enabled && share.status == "active")
    {
        for (app, provider_id) in share_bindings(share) {
            if !share_app_api_enabled(share, app) {
                continue;
            }
            let provider_exists = providers
                .providers
                .iter()
                .find(|provider| provider.app == app && provider.provider.id == provider_id)
                .is_some();
            if !provider_exists {
                tracing::warn!(
                    share_id = %share.id,
                    app = app.as_str(),
                    provider_id,
                    "share model health binding Provider was not found"
                );
                continue;
            }
            let Some(target) = targets.get_mut(&(app, provider_id)) else {
                continue;
            };
            if !target.shares.iter().any(|existing| existing.id == share.id) {
                target.shares.push(share.clone());
            }
        }
    }

    targets
}

fn is_health_target_surface(provider: &StoredProvider) -> bool {
    if !is_explicit_bundle_surface(&provider.provider) {
        return true;
    }
    match bundle_test_app(&provider.provider) {
        Ok(Some(test_app)) => provider.app == test_app,
        Ok(None) => false,
        Err(error) => {
            tracing::warn!(
                app = provider.app.as_str(),
                provider_id = %provider.provider.id,
                error = %error,
                "skipped malformed Provider Bundle health target"
            );
            false
        }
    }
}

async fn process_initial_health_target(
    state: &ServerState,
    target: HealthTarget,
    config: &ProviderHealthCheckConfig,
) -> anyhow::Result<Option<HealthTarget>> {
    if recent_router_cycle_covers_provider(state, &target.provider).await {
        tracing::debug!(
            app = target.provider.app.as_str(),
            provider_id = %target.provider.provider.id,
            "skipped local Provider health probe because a recent Router cycle is authoritative"
        );
        return Ok(None);
    }
    let probe_guard = state
        .lock_provider_health_probe(target.provider.app, &target.provider.provider.id)
        .await;
    if recent_router_cycle_covers_provider(state, &target.provider).await {
        tracing::debug!(
            app = target.provider.app.as_str(),
            provider_id = %target.provider.provider.id,
            "skipped queued local Provider health probe because a Router cycle became authoritative"
        );
        return Ok(None);
    }
    let Some(plan) = state
        .provider_runtime_plan(target.provider.app, &target.provider.provider.id)
        .await
        .filter(|plan| plan.provider_revision == target.provider.resource.revision)
    else {
        return Ok(None);
    };
    let accounts = state.accounts_snapshot().await;
    if let Some(block) = quota_block_for_provider(&target.provider, Some(&accounts)) {
        let active_shares = current_active_shares_for_provider(state, &target.provider).await;
        let model = plan.test_model.clone().unwrap_or_else(|| {
            provider_test_model(target.provider.app, &target.provider, None, Some(config))
        });
        let health_fingerprint = plan.health_fingerprint();
        let mut refreshes = Vec::with_capacity(active_shares.len());
        for share in &active_shares {
            record_quota_block_with_identity(
                state,
                share,
                &target.provider,
                model.clone(),
                Some(&health_fingerprint),
                &block,
                QUOTA_SOURCE,
                QUOTA_BLOCK_REPEAT_INTERVAL_MS,
            )
            .await?;
            refreshes.push(share.clone());
        }
        drop(probe_guard);
        notify_runtime_refreshes(state, refreshes).await;
        return Ok(None);
    }

    let probe =
        probe_provider_and_record_locked(state, &target.provider, config, SCHEDULED_SOURCE).await?;
    drop(probe_guard);
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
        &snapshot.runtime_fingerprint,
    )
    .await?;

    if !snapshot.confirmation_pending {
        return Ok(None);
    }

    let mut target = target;
    target.provider.resource.revision = snapshot.provider_revision;
    Ok(Some(target))
}

async fn recent_router_cycle_covers_provider(
    state: &ServerState,
    provider: &StoredProvider,
) -> bool {
    let providers = state.providers_snapshot().await;
    let plan = providers.runtime_plan(provider.app, &provider.provider.id);
    let usage = state.usage.read().await;
    let health = crate::domain::health::provider_health_for_plan(provider, &usage, plan.as_deref());
    health
        .source
        .as_deref()
        .is_some_and(|source| source.starts_with("cc-switch-router-cycle:"))
        && health.checked_at_ms.is_some_and(|checked_at| {
            now_ms().saturating_sub(checked_at) < ROUTER_PROBE_FALLBACK_AFTER_MS
        })
}

async fn confirm_health_target(
    state: &ServerState,
    target: HealthTarget,
    config: &ProviderHealthCheckConfig,
) -> anyhow::Result<()> {
    let probe_guard = state
        .lock_provider_health_probe(target.provider.app, &target.provider.provider.id)
        .await;
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
    if recent_router_cycle_covers_provider(state, &current).await {
        tracing::debug!(
            app = current.app.as_str(),
            provider_id = %current.provider.id,
            "skipped queued Provider health confirmation because a Router cycle became authoritative"
        );
        return Ok(());
    }
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
        probe_provider_and_record_locked(state, &current, config, CONFIRMATION_SOURCE).await?;
    drop(probe_guard);
    if let Some(snapshot) = confirmation
        .snapshot
        .as_ref()
        .filter(|_| confirmation.probe_support == ProviderProbeSupport::Supported)
    {
        let active_shares = current_active_shares_for_provider(state, &current).await;
        project_probe_to_shares(
            state,
            &active_shares,
            &current,
            &confirmation.result,
            CONFIRMATION_SOURCE,
            &snapshot.runtime_fingerprint,
        )
        .await?;
    }
    Ok(())
}

pub(crate) async fn probe_provider_and_record(
    state: &ServerState,
    provider: &StoredProvider,
    config: &ProviderHealthCheckConfig,
    source: &str,
) -> anyhow::Result<RecordedProviderProbe> {
    let _probe = state
        .lock_provider_health_probe(provider.app, &provider.provider.id)
        .await;
    probe_provider_and_record_locked(state, provider, config, source).await
}

async fn probe_provider_and_record_locked(
    state: &ServerState,
    provider: &StoredProvider,
    config: &ProviderHealthCheckConfig,
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
    let model = plan
        .test_model
        .clone()
        .unwrap_or_else(|| provider_test_model(provider.app, provider, None, Some(config)));
    if support == ProviderProbeSupport::Unsupported {
        return Ok(RecordedProviderProbe {
            result: failed_probe_result(
                provider,
                model,
                format!("driver {} does not support test", plan.driver_id),
                "unsupported",
                None,
                0,
            ),
            probe_support: support,
            snapshot: None,
        });
    }

    let query = TestProviderQuery {
        app: provider.app,
        network: Some(true),
        timeout_ms: Some(config.timeout_seconds.saturating_mul(1_000)),
        model: Some(model.clone()),
        test_prompt: Some(PROVIDER_MODEL_PROBE_PROMPT.to_string()),
        stream: Some(true),
    };
    let health_fingerprint = plan.health_fingerprint();
    let (result, runtime_fingerprint) =
        run_probe_with_retries(state, provider, &query, config, &model, &health_fingerprint).await;
    if !probe_matches_target_generation(
        provider.resource.revision,
        &health_fingerprint,
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
            checked_at_ms: u128::try_from(result.tested_at)
                .unwrap_or_default()
                .saturating_mul(1_000),
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
    config: &ProviderHealthCheckConfig,
    expected_health_fingerprint: &str,
    source: &str,
) -> anyhow::Result<Option<ProviderHealthSnapshot>> {
    if !response.network_checked || response.outcome == super::ProviderOperationOutcome::Unsupported
    {
        return Ok(None);
    }
    let result = map_provider_test_to_stream_check_result(response, config);
    let Some(plan) = state
        .provider_runtime_plan(provider.app, &provider.provider.id)
        .await
    else {
        return Ok(None);
    };
    if plan.health_fingerprint() != expected_health_fingerprint
        || response.runtime_fingerprint != plan.runtime_fingerprint
        || plan.test_model.as_deref() != Some(result.model_used.as_str())
    {
        return Ok(None);
    }
    let snapshot = record_probe_observation(
        state,
        provider,
        expected_health_fingerprint,
        &result,
        source,
    )
    .await?;
    if let Some(snapshot) = snapshot.as_ref() {
        project_accepted_probe_to_active_shares(
            state,
            provider,
            &result,
            source,
            &snapshot.runtime_fingerprint,
        )
        .await?;
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
    project_accepted_probe_to_active_shares(
        state,
        provider,
        &probe.result,
        source,
        &probe
            .snapshot
            .as_ref()
            .expect("snapshot checked above")
            .runtime_fingerprint,
    )
    .await
}

pub(crate) async fn check_share_binding(
    state: &ServerState,
    share: &Share,
    provider: &StoredProvider,
    accounts: &AccountStore,
    config: &ProviderHealthCheckConfig,
    source: &str,
) -> anyhow::Result<ShareBindingHealthCheck> {
    if let Some(block) = quota_block_for_provider(provider, Some(accounts)) {
        return record_quota_block(
            state,
            share,
            provider,
            config,
            &block,
            QUOTA_SOURCE,
            QUOTA_BLOCK_REPEAT_INTERVAL_MS,
        )
        .await;
    }

    let probe = probe_provider_and_record(state, provider, config, source).await?;
    if let Some(snapshot) = probe
        .snapshot
        .as_ref()
        .filter(|_| probe.probe_support == ProviderProbeSupport::Supported)
    {
        state
            .push_usage_log(health_usage_log(
                share,
                provider,
                &probe.result,
                source,
                true,
                Some(&snapshot.runtime_fingerprint),
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

/// Executes at most one network probe for each Provider runtime and projects
/// the accepted result to every requested Share. `source` is a deterministic
/// Router cycle identifier, so an HTTP retry reuses an already persisted
/// result instead of consuming another model request.
pub(crate) async fn check_share_bindings_batch(
    state: &ServerState,
    targets: Vec<BatchShareBindingTarget>,
    config: &ProviderHealthCheckConfig,
    source: &str,
) -> anyhow::Result<Vec<BatchShareBindingHealthCheck>> {
    let mut grouped = BTreeMap::<(AppKind, String), Vec<BatchShareBindingTarget>>::new();
    for target in targets {
        grouped
            .entry((target.provider.app, target.provider.provider.id.clone()))
            .or_default()
            .push(target);
    }

    let mut checked_groups = stream::iter(grouped.into_iter().map(|(key, group)| async move {
        (
            key,
            check_share_binding_group(state, group, config, source).await,
        )
    }))
    .buffer_unordered(MAX_CONCURRENT_PROBES)
    .collect::<Vec<_>>()
    .await;
    checked_groups.sort_by(|left, right| left.0.cmp(&right.0));

    let mut output = Vec::new();
    for ((app, provider_id), result) in checked_groups {
        match result {
            Ok(results) => output.extend(results),
            Err(error) => {
                tracing::warn!(
                    app = app.as_str(),
                    provider_id,
                    %error,
                    "omitted failed Provider group from Router model health batch"
                );
            }
        }
    }
    Ok(output)
}

async fn check_share_binding_group(
    state: &ServerState,
    group: Vec<BatchShareBindingTarget>,
    config: &ProviderHealthCheckConfig,
    source: &str,
) -> anyhow::Result<Vec<BatchShareBindingHealthCheck>> {
    let Some(first) = group.first() else {
        return Ok(Vec::new());
    };
    let provider = first.provider.clone();
    let probe_guard = state
        .lock_provider_health_probe(provider.app, &provider.provider.id)
        .await;
    let current_shares = state.shares.read().await.shares.clone();
    let group = group
        .into_iter()
        .filter_map(|target| {
            current_shares
                .iter()
                .find(|share| {
                    share.id == target.share.id
                        && share.enabled
                        && share.status == "active"
                        && share_app_api_enabled(share, provider.app)
                        && share_bindings(share).iter().any(|(app, provider_id)| {
                            *app == provider.app && provider_id == &provider.provider.id
                        })
                })
                .cloned()
                .map(|share| BatchShareBindingTarget {
                    share,
                    provider: target.provider,
                })
        })
        .collect::<Vec<_>>();
    if group.is_empty() {
        return Ok(Vec::new());
    }
    let plan = state
        .provider_runtime_plan(provider.app, &provider.provider.id)
        .await
        .ok_or_else(|| {
            anyhow::anyhow!("Provider runtime plan is unavailable for Router model health batch")
        })?;
    anyhow::ensure!(
        plan.provider_revision == provider.resource.revision,
        "Provider runtime changed before Router model health batch"
    );
    anyhow::ensure!(
        provider_probe_support(&plan) == ProviderProbeSupport::Supported,
        "Provider runtime no longer supports model testing"
    );
    let requested_model = plan
        .test_model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .ok_or_else(|| anyhow::anyhow!("Provider test model is unavailable for Router batch"))?;
    let expected_health_fingerprint = plan.health_fingerprint();
    let model_probe = build_provider_model_probe(
        provider.app,
        provider.provider_type,
        requested_model,
        PROVIDER_MODEL_PROBE_PROMPT,
        true,
        expected_health_fingerprint.clone(),
    );
    let model_policy = plan.model_policy.clone();
    let mut output = Vec::with_capacity(group.len());
    let mut refreshes = Vec::new();
    if let Some(result) =
        existing_cycle_result(state, &provider, source, &expected_health_fingerprint).await
    {
        let quota_blocked = recovered_cycle_result_is_quota_blocked(&result);
        let mut already_projected =
            existing_cycle_share_ids(state, &provider, source, &expected_health_fingerprint).await;
        for target in group {
            if !already_projected.contains(&target.share.id) {
                state
                    .push_usage_log(health_usage_log(
                        &target.share,
                        &provider,
                        &result,
                        source,
                        !quota_blocked,
                        Some(&expected_health_fingerprint),
                    ))
                    .await?;
                refreshes.push(target.share.clone());
                already_projected.insert(target.share.id.clone());
            }
            output.push(BatchShareBindingHealthCheck {
                share_id: target.share.id,
                app: provider.app,
                check: ShareBindingHealthCheck {
                    result: result.clone(),
                    provider_id: provider.provider.id.clone(),
                    provider_name: provider.provider.name.clone(),
                    quota_blocked,
                },
                model_probe: model_probe.clone(),
                model_policy: model_policy.clone(),
            });
        }
        drop(probe_guard);
        notify_runtime_refreshes(state, refreshes).await;
        return Ok(output);
    }
    let accounts = state.accounts_snapshot().await;
    if let Some(block) = quota_block_for_provider(&provider, Some(&accounts)) {
        let (first, remaining) = group
            .split_first()
            .expect("non-empty Provider group was checked above");
        let check = record_quota_block_with_identity(
            state,
            &first.share,
            &provider,
            requested_model.to_string(),
            Some(&expected_health_fingerprint),
            &block,
            source,
            0,
        )
        .await?;
        let current_fingerprint = state
            .provider_runtime_plan(provider.app, &provider.provider.id)
            .await
            .filter(|current| current.provider_revision == provider.resource.revision)
            .map(|current| current.health_fingerprint());
        anyhow::ensure!(
            current_fingerprint.as_deref() == Some(expected_health_fingerprint.as_str()),
            "Provider runtime changed during Router quota observation"
        );
        refreshes.push(first.share.clone());
        output.push(BatchShareBindingHealthCheck {
            share_id: first.share.id.clone(),
            app: provider.app,
            check: check.clone(),
            model_probe: model_probe.clone(),
            model_policy: model_policy.clone(),
        });
        for target in remaining {
            state
                .push_usage_log(health_usage_log(
                    &target.share,
                    &provider,
                    &check.result,
                    source,
                    false,
                    Some(&expected_health_fingerprint),
                ))
                .await?;
            refreshes.push(target.share.clone());
            output.push(BatchShareBindingHealthCheck {
                share_id: target.share.id.clone(),
                app: provider.app,
                check: check.clone(),
                model_probe: model_probe.clone(),
                model_policy: model_policy.clone(),
            });
        }
        drop(probe_guard);
        notify_runtime_refreshes(state, refreshes).await;
        return Ok(output);
    }

    let probe = probe_provider_and_record_locked(state, &provider, config, source).await?;
    let Some(snapshot) = probe
        .snapshot
        .as_ref()
        .filter(|_| probe.probe_support == ProviderProbeSupport::Supported)
    else {
        anyhow::bail!(
            "Provider runtime changed or became unsupported during Router model health batch"
        );
    };
    anyhow::ensure!(
        snapshot.runtime_fingerprint == expected_health_fingerprint,
        "Provider runtime changed during Router model health batch"
    );
    let result = probe.result;

    let already_projected =
        existing_cycle_share_ids(state, &provider, source, &expected_health_fingerprint).await;
    for target in group {
        if !already_projected.contains(&target.share.id) {
            state
                .push_usage_log(health_usage_log(
                    &target.share,
                    &provider,
                    &result,
                    source,
                    true,
                    Some(&expected_health_fingerprint),
                ))
                .await?;
            refreshes.push(target.share.clone());
        }
        output.push(BatchShareBindingHealthCheck {
            share_id: target.share.id,
            app: provider.app,
            check: ShareBindingHealthCheck {
                result: result.clone(),
                provider_id: provider.provider.id.clone(),
                provider_name: provider.provider.name.clone(),
                quota_blocked: false,
            },
            model_probe: model_probe.clone(),
            model_policy: model_policy.clone(),
        });
    }
    drop(probe_guard);
    notify_runtime_refreshes(state, refreshes).await;
    Ok(output)
}

async fn existing_cycle_result(
    state: &ServerState,
    provider: &StoredProvider,
    source: &str,
    expected_health_fingerprint: &str,
) -> Option<StreamCheckResult> {
    let usage = state.usage.read().await;
    existing_cycle_result_from_usage(&usage, provider, source, expected_health_fingerprint)
}

fn existing_cycle_result_from_usage(
    usage: &UsageStore,
    provider: &StoredProvider,
    source: &str,
    expected_health_fingerprint: &str,
) -> Option<StreamCheckResult> {
    let log = usage
        .logs
        .iter()
        .filter(|log| {
            log.is_health_check
                && log.app == provider.app
                && log.provider_id == provider.provider.id
                && log.data_source.as_deref() == Some(source)
                && log.provider_health_fingerprint.as_deref() == Some(expected_health_fingerprint)
        })
        .max_by_key(|log| log.created_at_ms);
    if let Some(result) = log.and_then(|log| exact_cycle_result_from_log(log, provider)) {
        return Some(result);
    }
    if let Some(snapshot) = usage
        .provider_health
        .get(provider.app, &provider.provider.id)
        .filter(|snapshot| {
            snapshot.source == source && snapshot.runtime_fingerprint == expected_health_fingerprint
        })
    {
        let success = snapshot.status.is_success();
        return Some(StreamCheckResult {
            status: match snapshot.status {
                ProviderHealthStatus::Healthy => HealthStatus::Operational,
                ProviderHealthStatus::Degraded => HealthStatus::Degraded,
                ProviderHealthStatus::Unknown | ProviderHealthStatus::Unhealthy => {
                    HealthStatus::Failed
                }
            },
            success,
            provider_revision: Some(snapshot.provider_revision),
            message: if success {
                "Check succeeded".to_string()
            } else {
                snapshot
                    .error_message
                    .clone()
                    .unwrap_or_else(|| "Check failed".to_string())
            },
            response_time_ms: snapshot.latency_ms,
            http_status: snapshot.status_code,
            model_used: snapshot
                .model
                .clone()
                .unwrap_or_else(|| provider.app.as_str().to_string()),
            tested_at: seconds_from_ms(snapshot.checked_at_ms),
            retry_count: log
                .map(|log| log.attempt_count.saturating_sub(1))
                .unwrap_or_default(),
            error_category: snapshot.error_category.clone(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        });
    }
    let log = log?;
    let success = (200..400).contains(&log.status_code)
        && (!log.is_streaming || log.stream_status.as_deref() == Some("completed"));
    Some(StreamCheckResult {
        status: if success {
            HealthStatus::Operational
        } else {
            HealthStatus::Failed
        },
        success,
        provider_revision: Some(provider.resource.revision),
        message: if success {
            "Check succeeded".to_string()
        } else {
            log.error_message
                .clone()
                .unwrap_or_else(|| "Check failed".to_string())
        },
        response_time_ms: Some(log.duration_ms.min(u128::from(u64::MAX)) as u64),
        http_status: Some(log.status_code),
        model_used: log
            .requested_model
            .clone()
            .or_else(|| log.model.clone())
            .unwrap_or_else(|| provider.app.as_str().to_string()),
        tested_at: seconds_from_ms(log.created_at_ms),
        retry_count: log.attempt_count.saturating_sub(1),
        error_category: log.failure_kind.clone(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    })
}

fn exact_cycle_result_from_log(
    log: &UsageLog,
    provider: &StoredProvider,
) -> Option<StreamCheckResult> {
    let evidence = log.provider_health_result.as_ref()?;
    let status = match evidence.status.as_str() {
        "operational" => HealthStatus::Operational,
        "degraded" => HealthStatus::Degraded,
        "failed" => HealthStatus::Failed,
        _ => return None,
    };
    let success = matches!(status, HealthStatus::Operational | HealthStatus::Degraded);
    Some(StreamCheckResult {
        status,
        success,
        provider_revision: Some(provider.resource.revision),
        message: if success {
            "Check succeeded".to_string()
        } else {
            log.error_message
                .clone()
                .unwrap_or_else(|| "Check failed".to_string())
        },
        response_time_ms: Some(log.duration_ms.min(u128::from(u64::MAX)) as u64),
        http_status: evidence.http_status,
        model_used: log
            .requested_model
            .clone()
            .or_else(|| log.model.clone())
            .unwrap_or_else(|| provider.app.as_str().to_string()),
        tested_at: seconds_from_ms(log.created_at_ms),
        retry_count: log.attempt_count.saturating_sub(1),
        error_category: log.failure_kind.clone(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    })
}

fn recovered_cycle_result_is_quota_blocked(result: &StreamCheckResult) -> bool {
    result.error_category.as_deref() == Some("quotaBlocked")
}

async fn existing_cycle_share_ids(
    state: &ServerState,
    provider: &StoredProvider,
    source: &str,
    expected_health_fingerprint: &str,
) -> HashSet<String> {
    let usage = state.usage.read().await;
    existing_cycle_share_ids_from_usage(&usage, provider, source, expected_health_fingerprint)
}

fn existing_cycle_share_ids_from_usage(
    usage: &UsageStore,
    provider: &StoredProvider,
    source: &str,
    expected_health_fingerprint: &str,
) -> HashSet<String> {
    usage
        .logs
        .iter()
        .filter(|log| {
            log.is_health_check
                && log.app == provider.app
                && log.provider_id == provider.provider.id
                && log.data_source.as_deref() == Some(source)
                && log.provider_health_fingerprint.as_deref() == Some(expected_health_fingerprint)
        })
        .filter_map(|log| log.share_id.clone())
        .collect()
}

async fn project_probe_to_shares(
    state: &ServerState,
    shares: &[Share],
    provider: &StoredProvider,
    result: &StreamCheckResult,
    source: &str,
    health_fingerprint: &str,
) -> anyhow::Result<()> {
    for share in shares {
        state
            .push_usage_log(health_usage_log(
                share,
                provider,
                result,
                source,
                true,
                Some(health_fingerprint),
            ))
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
    health_fingerprint: &str,
) -> anyhow::Result<usize> {
    let shares = current_active_shares_for_provider(state, provider).await;
    let projected = shares.len();
    project_probe_to_shares(state, &shares, provider, result, source, health_fingerprint).await?;
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
        .filter(|share| share_app_api_enabled(share, app))
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
    config: &ProviderHealthCheckConfig,
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
    let runtime_fingerprint = execution.plan.health_fingerprint();
    for attempt in 0..=config.max_retries {
        match test_provider_inner(state, execution.clone(), query).await {
            Ok(response) => {
                let retry = !probe_succeeded(&response)
                    && retryable_probe(response.outcome)
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
    config: &ProviderHealthCheckConfig,
    block: &AccountUsageBlock,
    source: &str,
    repeat_interval_ms: u128,
) -> anyhow::Result<ShareBindingHealthCheck> {
    let runtime_plan = state
        .provider_runtime_plan(provider.app, &provider.provider.id)
        .await;
    let model = runtime_plan
        .as_ref()
        .and_then(|plan| plan.test_model.clone())
        .unwrap_or_else(|| provider_test_model(provider.app, provider, None, Some(config)));
    let health_fingerprint = runtime_plan.map(|plan| plan.health_fingerprint());
    record_quota_block_with_identity(
        state,
        share,
        provider,
        model,
        health_fingerprint.as_deref(),
        block,
        source,
        repeat_interval_ms,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn record_quota_block_with_identity(
    state: &ServerState,
    share: &Share,
    provider: &StoredProvider,
    model: String,
    health_fingerprint: Option<&str>,
    block: &AccountUsageBlock,
    source: &str,
    repeat_interval_ms: u128,
) -> anyhow::Result<ShareBindingHealthCheck> {
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
    let log = health_usage_log(share, provider, &result, source, false, health_fingerprint);
    let persisted = state
        .push_health_usage_log_if_due(log, repeat_interval_ms)
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
    health_fingerprint: Option<&str>,
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
    log.provider_health_fingerprint = health_fingerprint.map(str::to_string);
    log.provider_health_result = Some(UsageProviderHealthResult {
        status: match result.status {
            HealthStatus::Operational => "operational",
            HealthStatus::Degraded => "degraded",
            HealthStatus::Failed => "failed",
        }
        .to_string(),
        http_status: result.http_status,
    });
    let checked_at_ms = u128::try_from(result.tested_at)
        .unwrap_or_default()
        .saturating_mul(1_000);
    log.created_at_ms = checked_at_ms;
    log.completed_at_ms = checked_at_ms;
    log.started_at_ms =
        checked_at_ms.saturating_sub(u128::from(result.response_time_ms.unwrap_or(0)));
    log.apply_context(UsageLogContext {
        share_id: Some(share.id.clone()),
        share_name: share.display_name.clone(),
        data_source: Some(source.to_string()),
        is_health_check: true,
        is_streaming: streaming,
        error_message: (!result.success).then(|| redact_provider_test_error(&result.message)),
        failure_kind: result.error_category.clone(),
        attempt_count: Some(result.retry_count.saturating_add(1)),
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

fn retryable_probe(outcome: ProviderOperationOutcome) -> bool {
    matches!(
        outcome,
        ProviderOperationOutcome::RateLimit
            | ProviderOperationOutcome::Timeout
            | ProviderOperationOutcome::Network
            | ProviderOperationOutcome::Upstream
    )
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

async fn notify_runtime_refreshes(state: &ServerState, shares: Vec<Share>) {
    for share in shares {
        notify_runtime_refresh(state, &share).await;
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

    fn bundle_provider_with(
        app: AppKind,
        id: &str,
        family_id: &str,
        test_app: AppKind,
        enabled: bool,
    ) -> StoredProvider {
        let mut provider = provider_with(app, id);
        provider.provider.extra.extend([
            ("bundleId".to_string(), json!(id)),
            ("familyId".to_string(), json!(family_id)),
            ("surfaceEnabled".to_string(), json!(enabled)),
            ("modelPolicyScope".to_string(), json!("global")),
            ("testApp".to_string(), json!(test_app.as_str())),
        ]);
        provider
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
        for outcome in [
            ProviderOperationOutcome::RateLimit,
            ProviderOperationOutcome::Timeout,
            ProviderOperationOutcome::Network,
            ProviderOperationOutcome::Upstream,
        ] {
            assert!(retryable_probe(outcome));
        }
        for outcome in [
            ProviderOperationOutcome::Success,
            ProviderOperationOutcome::Unsupported,
            ProviderOperationOutcome::InvalidConfig,
            ProviderOperationOutcome::MissingCredential,
            ProviderOperationOutcome::Auth,
            ProviderOperationOutcome::Quota,
            ProviderOperationOutcome::Protocol,
        ] {
            assert!(!retryable_probe(outcome));
        }
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
    fn router_cycle_recovery_rejects_a_previous_runtime_fingerprint() {
        let provider = provider();
        let share = share("share-1", "p1", true, "active");
        let source = "cc-switch-router-cycle:utc-1800";
        let mut result = failed_probe_result(
            &provider,
            "gpt-test".to_string(),
            "unused".to_string(),
            "network",
            Some(200),
            1,
        );
        result.status = HealthStatus::Degraded;
        result.success = true;
        result.message = "Check succeeded".to_string();
        result.response_time_ms = Some(25);
        result.http_status = None;
        result.error_category = None;

        let mut usage = UsageStore::default();
        usage.logs.push(health_usage_log(
            &share,
            &provider,
            &result,
            source,
            true,
            Some("health-fingerprint-old"),
        ));
        usage.provider_health.record(ProviderHealthObservation {
            app: provider.app,
            provider_id: provider.provider.id.clone(),
            provider_revision: provider.resource.revision,
            runtime_fingerprint: "health-fingerprint-old".to_string(),
            status: ProviderHealthStatus::Healthy,
            checked_at_ms: u128::try_from(result.tested_at)
                .unwrap_or_default()
                .saturating_mul(1_000),
            source: source.to_string(),
            status_code: result.http_status,
            latency_ms: result.response_time_ms,
            model: Some(result.model_used.clone()),
            error_category: None,
            error_message: None,
            transient_failure: false,
        });

        let recovered =
            existing_cycle_result_from_usage(&usage, &provider, source, "health-fingerprint-old")
                .expect("matching runtime should recover the persisted cycle result");
        assert!(recovered.success);
        assert_eq!(recovered.status, HealthStatus::Degraded);
        assert_eq!(recovered.http_status, None);
        assert_eq!(recovered.retry_count, 1);
        assert!(!recovered_cycle_result_is_quota_blocked(&recovered));
        let mut quota_blocked = recovered.clone();
        quota_blocked.success = false;
        quota_blocked.error_category = Some("quotaBlocked".to_string());
        assert!(recovered_cycle_result_is_quota_blocked(&quota_blocked));
        assert!(existing_cycle_result_from_usage(
            &usage,
            &provider,
            source,
            "health-fingerprint-new",
        )
        .is_none());
        assert_eq!(
            existing_cycle_share_ids_from_usage(
                &usage,
                &provider,
                source,
                "health-fingerprint-old",
            ),
            HashSet::from([share.id.clone()])
        );
        assert!(existing_cycle_share_ids_from_usage(
            &usage,
            &provider,
            source,
            "health-fingerprint-new",
        )
        .is_empty());
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

        let targets = health_targets(&shares, &providers);
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
    fn targets_include_all_enabled_ordinary_providers_and_exclude_disabled_surfaces() {
        let providers = ProviderStore {
            providers: vec![provider(), provider_with(AppKind::Codex, "p2"), {
                let mut disabled = provider_with(AppKind::Claude, "p3");
                disabled
                    .provider
                    .extra
                    .insert("surfaceEnabled".to_string(), json!(false));
                disabled
            }],
            ..Default::default()
        };
        let shares = [
            share("active", "p1", true, "active"),
            share("paused", "p2", true, "paused"),
            share("disabled", "p2", false, "active"),
        ];
        let targets = health_targets(&shares, &providers);

        assert_eq!(targets.len(), 2);
        assert!(targets.contains_key(&(AppKind::Codex, "p1".to_string())));
        assert!(targets.contains_key(&(AppKind::Codex, "p2".to_string())));
        assert!(!targets.contains_key(&(AppKind::Claude, "p3".to_string())));
    }

    #[test]
    fn bundle_targets_only_include_the_selected_test_app() {
        let providers = ProviderStore {
            providers: vec![
                bundle_provider_with(
                    AppKind::Claude,
                    "bundle-1",
                    "family.grok_oauth",
                    AppKind::Claude,
                    true,
                ),
                bundle_provider_with(
                    AppKind::Codex,
                    "bundle-1",
                    "family.grok_oauth",
                    AppKind::Claude,
                    true,
                ),
                bundle_provider_with(
                    AppKind::Gemini,
                    "bundle-1",
                    "family.grok_oauth",
                    AppKind::Claude,
                    true,
                ),
            ],
            ..Default::default()
        };

        let targets = health_targets(&[], &providers);

        assert_eq!(targets.len(), 1);
        assert!(targets.contains_key(&(AppKind::Claude, "bundle-1".to_string())));
    }

    #[test]
    fn openai_oauth_bundle_can_select_codex_as_its_only_health_target() {
        let providers = ProviderStore {
            providers: vec![
                bundle_provider_with(
                    AppKind::Claude,
                    "openai-oauth",
                    "family.openai_oauth",
                    AppKind::Codex,
                    true,
                ),
                bundle_provider_with(
                    AppKind::Codex,
                    "openai-oauth",
                    "family.openai_oauth",
                    AppKind::Codex,
                    true,
                ),
            ],
            ..Default::default()
        };

        let targets = health_targets(&[], &providers);

        assert_eq!(targets.len(), 1);
        assert!(targets.contains_key(&(AppKind::Codex, "openai-oauth".to_string())));
    }

    #[test]
    fn share_binding_does_not_restore_a_non_selected_bundle_surface() {
        let providers = ProviderStore {
            providers: vec![
                bundle_provider_with(
                    AppKind::Claude,
                    "bundle-1",
                    "family.grok_oauth",
                    AppKind::Claude,
                    true,
                ),
                bundle_provider_with(
                    AppKind::Codex,
                    "bundle-1",
                    "family.grok_oauth",
                    AppKind::Claude,
                    true,
                ),
            ],
            ..Default::default()
        };
        let shares = [share("codex-share", "bundle-1", true, "active")];

        let targets = health_targets(&shares, &providers);

        assert_eq!(targets.len(), 1);
        assert!(targets.contains_key(&(AppKind::Claude, "bundle-1".to_string())));
        assert!(!targets.contains_key(&(AppKind::Codex, "bundle-1".to_string())));
    }

    #[test]
    fn targets_include_enabled_surface_without_an_active_share() {
        let providers = ProviderStore {
            providers: vec![provider(), provider_with(AppKind::Codex, "p2")],
            ..Default::default()
        };
        let shares = [share("paused", "p1", true, "paused")];
        let targets = health_targets(&shares, &providers);

        assert_eq!(targets.len(), 2);
        let target = targets
            .get(&(AppKind::Codex, "p2".to_string()))
            .expect("enabled Provider Surface should be checked");
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

    #[test]
    fn projection_excludes_a_bound_but_disabled_share_app() {
        let mut disabled = share("disabled-app", "p1", true, "active");
        disabled.enabled_apps = Some(Default::default());

        assert!(active_shares_for_provider(&[disabled], AppKind::Codex, "p1").is_empty());
    }
}
