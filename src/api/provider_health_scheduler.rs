use std::time::Duration;

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::store::StoredProvider;
use crate::domain::sharing::model_health::{quota_blocked_for_provider, share_bindings};
use crate::domain::sharing::shares::Share;
use crate::domain::stream_check::{HealthStatus, StreamCheckConfig, StreamCheckResult};
use crate::domain::usage::store::{TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata};
use crate::state::ServerState;

use super::{
    map_provider_test_to_stream_check_result, provider_test_model, test_provider_inner,
    web_stream_check_config, TestProviderQuery, TestProviderResponse,
};

const FIRST_HEALTH_CHECK_DELAY: Duration = Duration::from_secs(120);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const QUOTA_BLOCK_REPEAT_INTERVAL_MS: u128 = 6 * 60 * 60 * 1000;
const BINDING_CHECK_DELAY: Duration = Duration::from_millis(250);
const SCHEDULED_SOURCE: &str = "cc-switch-scheduled";
const QUOTA_SOURCE: &str = "cc-switch-quota";

#[derive(Debug, Clone)]
pub(crate) struct ShareBindingHealthCheck {
    pub(crate) result: StreamCheckResult,
    pub(crate) provider_id: String,
    pub(crate) provider_name: String,
    pub(crate) quota_blocked: bool,
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
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts_snapshot().await;
    let config = web_stream_check_config(state).await;
    let mut checked_any = false;

    for share in shares
        .into_iter()
        .filter(|share| share.enabled && share.status == "active")
    {
        let mut checked_share = false;
        for (app, provider_id) in share_bindings(&share) {
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
                    "share model health binding provider was not found"
                );
                continue;
            };
            checked_any = true;
            checked_share = true;
            if let Err(error) = check_share_binding(
                state,
                &share,
                &provider,
                &accounts,
                &config,
                SCHEDULED_SOURCE,
            )
            .await
            {
                tracing::warn!(
                    share_id = %share.id,
                    app = app.as_str(),
                    provider_id = %provider.provider.id,
                    error = %error,
                    "share model health check failed"
                );
            }
            tokio::time::sleep(BINDING_CHECK_DELAY).await;
        }
        if checked_share {
            notify_runtime_refresh(state, &share).await;
        }
    }

    if !checked_any {
        tracing::debug!("no active share bindings require a model health check");
    }
}

pub(crate) async fn check_share_binding(
    state: &ServerState,
    share: &Share,
    provider: &StoredProvider,
    accounts: &AccountStore,
    config: &StreamCheckConfig,
    source: &str,
) -> anyhow::Result<ShareBindingHealthCheck> {
    if quota_blocked_for_provider(share, provider, Some(accounts)) {
        return record_quota_block(state, share, provider, config).await;
    }

    let model = provider_test_model(provider.app, provider, None, Some(config));
    let query = TestProviderQuery {
        app: Some(provider.app),
        network: Some(true),
        timeout_ms: Some(config.timeout_secs.saturating_mul(1000)),
        model: Some(model.clone()),
        stream: Some(true),
    };
    let result = run_probe_with_retries(state, provider, &query, config, &model).await;
    let log = health_usage_log(share, provider, &result, source, true);
    state.push_usage_log(log).await?;
    Ok(ShareBindingHealthCheck {
        result,
        provider_id: provider.provider.id.clone(),
        provider_name: provider.provider.name.clone(),
        quota_blocked: false,
    })
}

async fn run_probe_with_retries(
    state: &ServerState,
    provider: &StoredProvider,
    query: &TestProviderQuery,
    config: &StreamCheckConfig,
    model: &str,
) -> StreamCheckResult {
    for attempt in 0..=config.max_retries {
        match test_provider_inner(state, provider.clone(), query).await {
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
                    return result;
                }
            }
            Err(error) => {
                let retry = (error.retryable.unwrap_or(false)
                    || error.status.as_u16() == 429
                    || error.status.is_server_error())
                    && attempt < config.max_retries;
                if !retry {
                    return StreamCheckResult {
                        status: HealthStatus::Failed,
                        success: false,
                        message: error.message,
                        response_time_ms: None,
                        http_status: Some(error.status.as_u16()),
                        model_used: model.to_string(),
                        tested_at: chrono::Utc::now().timestamp(),
                        retry_count: attempt,
                        error_category: error.code.map(str::to_string),
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cache_creation_tokens: 0,
                    };
                }
            }
        }
        tokio::time::sleep(BINDING_CHECK_DELAY).await;
    }

    unreachable!("probe loop always returns on its final attempt")
}

async fn record_quota_block(
    state: &ServerState,
    share: &Share,
    provider: &StoredProvider,
    config: &StreamCheckConfig,
) -> anyhow::Result<ShareBindingHealthCheck> {
    let model = provider_test_model(provider.app, provider, None, Some(config));
    let result = StreamCheckResult {
        status: HealthStatus::Failed,
        success: false,
        message: "quota blocked".to_string(),
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
    result.message = persisted
        .error_message
        .unwrap_or_else(|| "quota blocked".to_string());
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
    let token = |value: u32| (value > 0).then_some(u64::from(value));
    let input_tokens = token(result.input_tokens);
    let output_tokens = token(result.output_tokens);
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
        TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_tokens: token(result.cache_read_tokens),
            cache_creation_tokens: token(result.cache_creation_tokens),
            total_tokens: match (input_tokens, output_tokens) {
                (None, None) => None,
                (input, output) => Some(input.unwrap_or(0).saturating_add(output.unwrap_or(0))),
            },
            ..TokenUsage::default()
        },
    );
    log.error_message = (!result.success).then(|| result.message.clone());
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
        || status == Some(429)
        || status.is_some_and(|status| status >= 500)
        || (status.is_none() && has_network_error)
}

fn probe_error_category(status: Option<u16>, stream_completed: Option<bool>) -> Option<String> {
    if stream_completed == Some(false) {
        Some("streamIncomplete".to_string())
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
    if config.router_api_base().is_none() || config.router.identity.is_none() {
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
            "notify router model health refresh failed"
        );
    }
}

fn seconds_from_ms(value: u128) -> i64 {
    (value / 1000).min(i64::MAX as u128) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_only_transient_or_incomplete_probe_failures() {
        assert!(retryable_probe(None, None, true));
        assert!(retryable_probe(Some(429), None, true));
        assert!(retryable_probe(Some(503), None, true));
        assert!(retryable_probe(Some(200), Some(false), true));
        assert!(!retryable_probe(Some(401), None, true));
        assert!(!retryable_probe(Some(404), None, true));
    }

    #[test]
    fn seconds_conversion_saturates() {
        assert_eq!(seconds_from_ms(1_999), 1);
        assert_eq!(seconds_from_ms(u128::MAX), i64::MAX);
    }

    #[test]
    fn health_check_intervals_match_desktop_contract() {
        assert_eq!(FIRST_HEALTH_CHECK_DELAY, Duration::from_secs(120));
        assert_eq!(HEALTH_CHECK_INTERVAL, Duration::from_secs(30 * 60));
        assert_eq!(QUOTA_BLOCK_REPEAT_INTERVAL_MS, 6 * 60 * 60 * 1000);
    }
}
