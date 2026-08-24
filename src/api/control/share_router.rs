use super::*;
use sha2::{Digest, Sha256};

const SHARE_ROUTER_REQUEST_LOGS_LIMIT: usize = 100;
const SHARE_ROUTER_SHARE_ID_HEADER: &str = "x-cc-switch-share-id";

pub(crate) async fn share_router_health(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
) -> Result<Json<ShareRouterHealthResponse>, ApiError> {
    require_share_router_request(&state, "GET", &uri, &headers, &[]).await?;
    Ok(Json(ShareRouterHealthResponse {
        ok: true,
        status: "healthy".to_string(),
        timestamp_ms: now_ms(),
    }))
}

pub(crate) async fn share_router_request_logs(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<ShareRouterRequestLogsQuery>,
) -> Result<Json<ShareRouterRequestLogsResponse>, ApiError> {
    require_share_router_request(&state, "GET", &uri, &headers, &[]).await?;
    let header_share_id = share_id_from_router_header(&headers)?;
    if query
        .share_id
        .as_deref()
        .is_some_and(|query_share_id| query_share_id != header_share_id)
    {
        return Err(ApiError::not_found("not found"));
    }
    let share_id = header_share_id.to_string();
    let limit = query
        .limit
        .unwrap_or(SHARE_ROUTER_REQUEST_LOGS_LIMIT)
        .clamp(1, SHARE_ROUTER_REQUEST_LOGS_LIMIT);
    let after_sequence = query.after_sequence.unwrap_or_default();
    let mut matching = {
        let usage = state.usage.read().await;
        usage
            .logs
            .iter()
            .filter(|log| {
                log.share_id.as_deref() == Some(share_id.as_str())
                    && log.is_user_inference()
                    && log.data_source.as_deref() == Some("router_share")
                    && !log.is_health_check
                    && log.router_export_sequence > after_sequence
            })
            .cloned()
            .collect::<Vec<_>>()
    };
    matching.sort_by(|left, right| {
        left.router_export_sequence
            .cmp(&right.router_export_sequence)
            .then_with(|| left.request_id.cmp(&right.request_id))
    });
    let has_more = matching.len() > limit;
    let mut logs = Vec::new();
    for log in matching.into_iter().take(limit) {
        if let Some(entry) = crate::state::share_request_log_entry(&state, &log).await {
            logs.push(entry);
        }
    }
    let next_sequence = logs
        .last()
        .map(|log| log.export_sequence)
        .unwrap_or(after_sequence);
    Ok(Json(ShareRouterRequestLogsResponse {
        share_id: Some(share_id),
        logs,
        next_sequence,
        has_more,
    }))
}

pub(crate) async fn share_router_runtime(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<ShareRouterRuntimeQuery>,
) -> Result<Json<ShareRouterRuntimeResponse>, ApiError> {
    require_share_router_request(&state, "GET", &uri, &headers, &[]).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = resolve_share_for_internal_request(&state, query.share_id.as_deref()).await?;
    let descriptor = state.share_router_runtime_descriptor(&share, &providers, &accounts, &usage);
    Ok(Json(runtime_response_from_descriptor(descriptor)))
}

pub(crate) async fn share_router_model_health(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ShareRouterModelHealthResponse>, ApiError> {
    require_share_router_request(&state, "POST", &uri, &headers, &body).await?;
    let input: ShareRouterModelHealthRequest =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let app = parse_app_kind(&input.app_type)?;
    let share_id = share_id_from_router_header(&headers)?;
    let share = resolve_share_for_internal_request(&state, Some(share_id)).await?;
    if !crate::domain::sharing::shares::share_app_api_enabled(&share, app) {
        return Err(ApiError::not_found("share app API is not enabled"));
    }
    let provider_id = crate::domain::sharing::model_health::share_bindings(&share)
        .into_iter()
        .find_map(|(bound_app, provider_id)| (bound_app == app).then_some(provider_id))
        .ok_or_else(|| ApiError::not_found("share app binding not found"))?;
    let providers = state.providers.read().await.clone();
    let provider = providers
        .providers
        .iter()
        .find(|provider| provider.app == app && provider.provider.id == provider_id)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "provider not found"))?;
    let accounts = state.accounts_snapshot().await;
    let config = web_provider_health_check_config(&state).await;
    let check = crate::api::provider_health_scheduler::check_share_binding(
        &state,
        &share,
        &provider,
        &accounts,
        &config,
        "cc-switch-router-probe",
    )
    .await
    .map_err(ApiError::internal)?;
    let status = if check.quota_blocked {
        "quota_blocked"
    } else if check.result.success {
        "healthy"
    } else {
        "failed"
    };
    Ok(Json(ShareRouterModelHealthResponse {
        ok: true,
        success: check.result.success,
        status: status.to_string(),
        message: check.result.message,
        status_code: check.result.http_status,
        model_used: check.result.model_used,
        response_time_ms: check.result.response_time_ms,
        tested_at: check.result.tested_at,
        retry_count: check.result.retry_count,
        error_category: check.result.error_category,
        provider_id: check.provider_id,
        provider_name: check.provider_name,
    }))
}

pub(crate) async fn share_router_model_health_batch(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ShareRouterModelHealthBatchResponse>, ApiError> {
    share_router_model_health_batch_inner(state, uri, headers, body, false).await
}

pub(crate) async fn share_router_model_health_batch_v2(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ShareRouterModelHealthBatchResponse>, ApiError> {
    share_router_model_health_batch_inner(state, uri, headers, body, true).await
}

async fn share_router_model_health_batch_inner(
    state: ServerState,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
    evidence_v2: bool,
) -> Result<Json<ShareRouterModelHealthBatchResponse>, ApiError> {
    require_share_router_request(&state, "POST", &uri, &headers, &body).await?;
    let installation_id = super::ctl::required_header(&headers, "x-ctl-installation-id")?;
    let input: ShareRouterModelHealthBatchRequest =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    if evidence_v2 != (input.contract_version == Some(2)) {
        return Err(ApiError::bad_request(
            "model health batch contractVersion does not match the endpoint",
        ));
    }
    validate_router_cycle_id(&input.cycle_id)?;
    if input.targets.is_empty() || input.targets.len() > 256 {
        return Err(ApiError::bad_request(
            "targets must contain between 1 and 256 entries",
        ));
    }

    let shares = state.shares.read().await.shares.clone();
    let providers = state.providers_snapshot().await;
    let mut dedupe = std::collections::BTreeSet::new();
    let mut targets = Vec::with_capacity(input.targets.len());
    for target in input.targets {
        let share_id = target.share_id.trim();
        let (app, _) = model_health_api_app(&target.app_type)?;
        if !dedupe.insert(share_id.to_string()) {
            continue;
        }
        let Some(share) = shares
            .iter()
            .find(|share| share.id == share_id && share.enabled && share.status == "active")
            .cloned()
        else {
            tracing::warn!(share_id, "omitted stale Share model health batch target");
            continue;
        };
        if !crate::domain::sharing::shares::share_app_api_enabled(&share, app) {
            tracing::warn!(
                share_id,
                app = app.as_str(),
                "omitted disabled Share App model health target"
            );
            continue;
        }
        let Some(provider_id) = crate::domain::sharing::model_health::share_bindings(&share)
            .into_iter()
            .find_map(|(bound_app, provider_id)| (bound_app == app).then_some(provider_id))
        else {
            tracing::warn!(
                share_id,
                app = app.as_str(),
                "omitted unbound Share model health target"
            );
            continue;
        };
        let Some(provider) = providers
            .providers
            .iter()
            .find(|provider| provider.app == app && provider.provider.id == provider_id)
            .cloned()
        else {
            tracing::warn!(
                share_id,
                app = app.as_str(),
                provider_id,
                "omitted missing Provider model health target"
            );
            continue;
        };
        targets.push(
            crate::api::provider_health_scheduler::BatchShareBindingTarget { share, provider },
        );
    }

    let config = web_provider_health_check_config(&state).await;
    let source = format!("cc-switch-router-cycle:{}", input.cycle_id);
    let checks = crate::api::provider_health_scheduler::check_share_bindings_batch(
        &state, targets, &config, &source,
    )
    .await
    .map_err(ApiError::internal)?;
    let results = checks
        .into_iter()
        .map(|item| {
            let (_, api_type) = model_health_api_app(item.app.as_str())?;
            let requested_model = item.model_probe.requested_model.clone();
            let health_fingerprint = Some(item.model_probe.health_fingerprint.clone());
            let actual_model =
                model_health_actual_model(Some(&item.model_policy), &item.model_probe);
            let policy_mode = Some(match &item.model_policy {
                crate::domain::providers::runtime::RuntimeModelPolicy::Passthrough => "passthrough",
                crate::domain::providers::runtime::RuntimeModelPolicy::Single { .. } => "single",
            });
            let status = if item.check.quota_blocked {
                "quota_blocked"
            } else if item.check.result.success {
                match item.check.result.status {
                    crate::domain::stream_check::HealthStatus::Degraded => "degraded",
                    _ => "success",
                }
            } else {
                "failed"
            };
            let (outcome, failure_domain, reason_code) = if evidence_v2 {
                let (outcome, failure_domain, reason_code) =
                    model_health_evidence(&item.check, status);
                (
                    Some(outcome.to_string()),
                    failure_domain.map(str::to_string),
                    Some(reason_code),
                )
            } else {
                (None, None, None)
            };
            let observation_id = if evidence_v2 {
                let fingerprint = health_fingerprint.as_deref().ok_or_else(|| {
                    ApiError::internal(anyhow::anyhow!(
                        "Provider runtime fingerprint disappeared before v2 response"
                    ))
                })?;
                Some(model_health_observation_id(
                    installation_id,
                    &input.cycle_id,
                    item.app,
                    &item.check.provider_id,
                    fingerprint,
                )?)
            } else {
                None
            };
            Ok(ShareRouterModelHealthBatchResult {
                share_id: item.share_id,
                app_type: api_type.to_string(),
                requested_model,
                actual_model,
                status: status.to_string(),
                status_code: item.check.result.http_status,
                latency_ms: item.check.result.response_time_ms.unwrap_or_default(),
                checked_at: item.check.result.tested_at,
                retry_count: item.check.result.retry_count,
                error_category: item.check.result.error_category,
                error_message: (!item.check.result.success).then(|| {
                    crate::api::providers::redact_provider_test_error(&item.check.result.message)
                        .chars()
                        .take(500)
                        .collect()
                }),
                provider_id: item.check.provider_id,
                provider_name: item.check.provider_name,
                policy_mode: policy_mode.map(str::to_string),
                health_fingerprint,
                observation_id,
                outcome,
                failure_domain,
                reason_code,
                evidence_scope: evidence_v2.then(|| "provider_runtime".to_string()),
                evidence_version: evidence_v2.then_some(2),
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    Ok(Json(ShareRouterModelHealthBatchResponse {
        ok: true,
        cycle_id: input.cycle_id,
        results,
    }))
}

fn model_health_actual_model(
    policy: Option<&crate::domain::providers::runtime::RuntimeModelPolicy>,
    probe: &crate::domain::providers::runtime::ProviderModelProbe,
) -> String {
    match policy {
        Some(crate::domain::providers::runtime::RuntimeModelPolicy::Single { upstream_model })
            if !upstream_model.trim().is_empty() =>
        {
            upstream_model.clone()
        }
        _ => probe.wire_model.clone(),
    }
}

fn model_health_evidence(
    check: &crate::api::provider_health_scheduler::ShareBindingHealthCheck,
    status: &str,
) -> (&'static str, Option<&'static str>, String) {
    if check.result.success {
        return (
            "success",
            None,
            if status == "degraded" {
                "probe_degraded".to_string()
            } else {
                "probe_succeeded".to_string()
            },
        );
    }
    if check.quota_blocked {
        return ("failure", Some("quota"), "quota_blocked".to_string());
    }
    let category = check.result.error_category.as_deref().unwrap_or("unknown");
    let (domain, reason) = match category {
        "auth" => ("provider_config", "provider_auth"),
        "invalidConfig" => ("provider_config", "provider_invalid_config"),
        "missingCredential" => ("provider_config", "provider_missing_credential"),
        "modelNotFound" => ("provider_config", "provider_model_not_found"),
        "protocol" => ("provider_config", "provider_protocol_rejected"),
        "rateLimit" => ("upstream", "upstream_rate_limited"),
        "timeout" => ("upstream", "upstream_timeout"),
        "network" => ("upstream", "upstream_network"),
        "streamIncomplete" => ("upstream", "upstream_stream_incomplete"),
        "upstream" => ("upstream", "upstream_failure"),
        _ => ("unknown", "probe_failed"),
    };
    ("failure", Some(domain), reason.to_string())
}

fn model_health_observation_id(
    installation_id: &str,
    cycle_id: &str,
    app: AppKind,
    provider_id: &str,
    health_fingerprint: &str,
) -> Result<String, ApiError> {
    let canonical = serde_json::to_vec(&(
        "cc-switch-model-health-observation-v2",
        installation_id,
        cycle_id,
        app.as_str(),
        provider_id,
        health_fingerprint,
    ))
    .map_err(ApiError::internal)?;
    Ok(format!("{:x}", Sha256::digest(canonical)))
}

fn model_health_api_app(value: &str) -> Result<(AppKind, &'static str), ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "openai" | "codex" | "omo" | "omo_slim" => Ok((AppKind::Codex, "openai")),
        "anthropic" | "claude" | "claude-desktop" => Ok((AppKind::Claude, "anthropic")),
        "gemini" => Ok((AppKind::Gemini, "gemini")),
        _ => Err(ApiError::bad_request("invalid appType")),
    }
}

fn validate_router_cycle_id(value: &str) -> Result<(), ApiError> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
    {
        return Err(ApiError::bad_request("invalid cycleId"));
    }
    Ok(())
}

fn share_id_from_router_header(headers: &HeaderMap) -> Result<&str, ApiError> {
    let mut values = headers.get_all(SHARE_ROUTER_SHARE_ID_HEADER).iter();
    values
        .next()
        .filter(|_| values.next().is_none())
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::not_found("not found"))
}

async fn require_share_router_request(
    state: &ServerState,
    method: &str,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ApiError> {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or_else(|| uri.path());
    super::ctl::verify_control_request_for_method(state, method, path_and_query, headers, body)
        .await
        .map_err(|_| ApiError::not_found("not found"))
}

pub(crate) async fn resolve_share_for_internal_request(
    state: &ServerState,
    share_id: Option<&str>,
) -> Result<Share, ApiError> {
    let shares = state.shares.read().await;
    if let Some(share_id) = share_id.map(str::trim).filter(|value| !value.is_empty()) {
        return shares
            .shares
            .iter()
            .find(|share| share.id == share_id)
            .cloned()
            .ok_or_else(|| ApiError::not_found(format!("share not found: {share_id}")));
    }
    match shares.shares.as_slice() {
        [share] => Ok(share.clone()),
        [] => Err(ApiError::not_found("share not found")),
        _ => Err(ApiError::bad_request(
            "multiple shares present; router must specify ?shareId=",
        )),
    }
}

pub(crate) fn runtime_response_from_descriptor(
    descriptor: ShareDescriptor,
) -> ShareRouterRuntimeResponse {
    ShareRouterRuntimeResponse {
        share_id: descriptor.share_id,
        queried_at: (now_ms() / 1000) as i64,
        token_limit: Some(descriptor.token_limit),
        tokens_used: Some(descriptor.tokens_used),
        requests_count: Some(descriptor.requests_count),
        share_status: Some(descriptor.share_status),
        support: descriptor.support,
        app_runtimes: descriptor.app_runtimes,
        app_providers: descriptor.app_providers,
        app_availability: descriptor.app_availability,
        model_health: descriptor.model_health,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterHealthResponse {
    ok: bool,
    status: String,
    timestamp_ms: u128,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterRequestLogsQuery {
    #[serde(default)]
    share_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    after_sequence: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterRequestLogsResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_id: Option<String>,
    logs: Vec<ShareRequestLogEntry>,
    next_sequence: u64,
    has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterRuntimeQuery {
    #[serde(default)]
    share_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterRuntimeResponse {
    share_id: String,
    queried_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tokens_used: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requests_count: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_status: Option<String>,
    support: ShareSupport,
    app_runtimes: ShareAppRuntimes,
    app_providers: ShareAppProviders,
    app_availability: ShareAppAvailability,
    model_health: crate::domain::sharing::model_health::ShareModelHealthSummary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterModelHealthRequest {
    app_type: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterModelHealthResponse {
    ok: bool,
    success: bool,
    status: String,
    message: String,
    status_code: Option<u16>,
    model_used: String,
    response_time_ms: Option<u64>,
    tested_at: i64,
    retry_count: u32,
    error_category: Option<String>,
    provider_id: String,
    provider_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShareRouterModelHealthBatchRequest {
    #[serde(default)]
    contract_version: Option<u16>,
    cycle_id: String,
    targets: Vec<ShareRouterModelHealthBatchTarget>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ShareRouterModelHealthBatchTarget {
    share_id: String,
    app_type: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterModelHealthBatchResponse {
    ok: bool,
    cycle_id: String,
    results: Vec<ShareRouterModelHealthBatchResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ShareRouterModelHealthBatchResult {
    share_id: String,
    app_type: String,
    requested_model: String,
    actual_model: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status_code: Option<u16>,
    latency_ms: u64,
    checked_at: i64,
    retry_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
    provider_id: String,
    provider_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    policy_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    health_fingerprint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_domain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    evidence_version: Option<u16>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::ProviderType;
    use crate::domain::providers::runtime::{build_provider_model_probe, RuntimeModelPolicy};

    #[test]
    fn batch_actual_model_matches_the_public_probe_and_fixed_policy() {
        let claude = build_provider_model_probe(
            AppKind::Claude,
            ProviderType::Claude,
            "claude-sonnet@2026-08-24",
            "ping",
            true,
            "fingerprint",
        );
        assert_eq!(
            model_health_actual_model(Some(&RuntimeModelPolicy::Passthrough), &claude),
            "claude-sonnet@2026-08-24"
        );

        let codex = build_provider_model_probe(
            AppKind::Codex,
            ProviderType::CodexOAuth,
            "gpt-test@low",
            "ping",
            true,
            "fingerprint",
        );
        assert_eq!(
            model_health_actual_model(Some(&RuntimeModelPolicy::Passthrough), &codex),
            "gpt-test"
        );
        assert_eq!(
            model_health_actual_model(
                Some(&RuntimeModelPolicy::Single {
                    upstream_model: "fixed-upstream".to_string(),
                }),
                &codex,
            ),
            "fixed-upstream"
        );
    }

    #[test]
    fn batch_evidence_classifies_missing_credentials_as_provider_configuration() {
        let check = crate::api::provider_health_scheduler::ShareBindingHealthCheck {
            result: crate::domain::stream_check::StreamCheckResult {
                status: crate::domain::stream_check::HealthStatus::Failed,
                success: false,
                provider_revision: Some(1),
                message: "missing credential".to_string(),
                response_time_ms: None,
                http_status: None,
                model_used: "gpt-test".to_string(),
                tested_at: 1,
                retry_count: 0,
                error_category: Some("missingCredential".to_string()),
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            },
            provider_id: "provider-test".to_string(),
            provider_name: "Provider Test".to_string(),
            quota_blocked: false,
        };

        assert_eq!(
            model_health_evidence(&check, "failed"),
            (
                "failure",
                Some("provider_config"),
                "provider_missing_credential".to_string()
            )
        );
    }
}
