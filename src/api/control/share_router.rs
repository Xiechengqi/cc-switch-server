use super::*;

const SHARE_ROUTER_REQUEST_LOGS_LIMIT: usize = 10;
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
    let mut header_share_ids = headers.get_all(SHARE_ROUTER_SHARE_ID_HEADER).iter();
    let header_share_id = header_share_ids
        .next()
        .filter(|_| header_share_ids.next().is_none())
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::not_found("not found"))?;
    if query
        .share_id
        .as_deref()
        .is_some_and(|query_share_id| query_share_id != header_share_id)
    {
        return Err(ApiError::not_found("not found"));
    }
    let share_id = header_share_id.to_string();
    let limit = query.limit.unwrap_or(SHARE_ROUTER_REQUEST_LOGS_LIMIT);
    let usage = state.usage.read().await.clone();
    let mut matching = usage
        .logs
        .iter()
        .filter(|log| log.share_id.as_deref() == Some(share_id.as_str()))
        .collect::<Vec<_>>();
    matching.sort_by(|left, right| right.created_at_ms.cmp(&left.created_at_ms));
    let mut logs = Vec::new();
    for log in matching.into_iter().take(limit) {
        if let Some(entry) = crate::state::share_request_log_entry(&state, log).await {
            logs.push(entry);
        }
    }
    Ok(Json(ShareRouterRequestLogsResponse {
        share_id: Some(share_id),
        logs,
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
    let descriptor = descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
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
    let providers = state.providers.read().await.clone();
    let usage = state.usage.read().await.clone();
    let provider = providers
        .providers
        .iter()
        .find(|provider| provider.app == app)
        .cloned()
        .ok_or_else(|| ApiError::new(StatusCode::SERVICE_UNAVAILABLE, "no provider selected"))?;
    let health = crate::domain::health::provider_health(&provider, &usage);
    let latest = usage
        .logs
        .iter()
        .rev()
        .find(|log| log.app == app && log.provider_id == provider.provider.id);
    Ok(Json(ShareRouterModelHealthResponse {
        ok: true,
        success: health.healthy,
        status: if health.healthy { "healthy" } else { "failed" }.to_string(),
        message: health
            .reason
            .clone()
            .unwrap_or_else(|| "derived from server usage health".to_string()),
        status_code: latest.map(|log| log.status_code),
        model_used: latest
            .and_then(|log| {
                log.actual_model
                    .clone()
                    .or_else(|| log.requested_model.clone())
            })
            .or_else(|| {
                provider
                    .provider
                    .settings_config
                    .get("model")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_default(),
        response_time_ms: latest.map(|log| clamp_u128_to_u64(log.duration_ms)),
        tested_at: (latest.map(|log| log.created_at_ms).unwrap_or_else(now_ms) / 1000) as i64,
        retry_count: 0,
        error_category: None,
        provider_id: provider.provider.id,
        provider_name: provider.provider.name,
    }))
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShareRouterRequestLogsResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    share_id: Option<String>,
    logs: Vec<ShareRequestLogEntry>,
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
