use super::*;

use axum::extract::Query;

pub(in crate::api) async fn router_config(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterConfigResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(RouterConfigResponse {
        ok: true,
        router: RouterConfigView::from_config(&config.router),
    }))
}

pub(in crate::api) async fn update_router_config(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateRouterConfigInput>,
) -> Result<Json<RouterConfigResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let _claim = state.lock_client_tunnel_claim().await;
    let mut config = state.config.read().await.clone();
    config.update_router(input).map_err(ApiError::bad_request)?;
    let router = RouterConfigView::from_config(&config.router);
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(RouterConfigResponse { ok: true, router }))
}

pub(in crate::api) async fn client_tunnel_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    let mut remote_tunnel = None;
    let mut remote_error = None;
    if config.has_registered_router_identity() {
        let http_client = state.http_client().await;
        match crate::clients::router::client::get_client_tunnel(&http_client, &config).await {
            Ok(tunnel) => remote_tunnel = tunnel,
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(error = %message, "router client tunnel status failed");
                state
                    .mutate_shares_immediate(|shares| {
                        shares.last_router_error = Some(message.clone());
                    })
                    .await
                    .map_err(ApiError::internal)?;
                remote_error = Some(message);
            }
        }
    }
    Ok(Json(
        client_tunnel_response(&state, &config, remote_tunnel, remote_error).await,
    ))
}

pub(in crate::api) async fn update_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateClientTunnelInput>,
) -> Result<Json<ClientTunnelResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let _claim = state.lock_client_tunnel_claim().await;
    let mut config = state.config.read().await.clone();
    let previous_subdomain = config.client.tunnel_subdomain.clone();
    let previous_runtime = state
        .tunnels
        .status(&crate::clients::router::tunnel::client_tunnel_key())
        .await;
    config
        .update_client_tunnel(input)
        .map_err(ApiError::bad_request)?;
    state
        .replace_config(config.clone())
        .await
        .map_err(ApiError::internal)?;
    drop(_claim);
    if config.client.tunnel_status.as_deref() == Some("stopped") {
        crate::state::stop_client_tunnel(&state).await;
    } else if previous_subdomain != config.client.tunnel_subdomain
        && previous_runtime
            .as_ref()
            .is_some_and(|status| status.status != "stopped")
    {
        crate::state::force_reconnect_client_tunnel(
            state.clone(),
            "client_tunnel_subdomain_changed",
        )
        .await;
    }
    let response = ClientTunnelResponse {
        ok: true,
        tunnel_subdomain: config.client.tunnel_subdomain.clone(),
        tunnel_status: config.client.tunnel_status.clone(),
        last_heartbeat_ms: config.client.last_heartbeat_ms,
        runtime_status: state
            .tunnels
            .status(&crate::clients::router::tunnel::client_tunnel_key())
            .await,
        remote_tunnel: None,
        remote_error: None,
    };
    Ok(Json(response))
}

pub(in crate::api) async fn claim_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelClaimResponse>, ApiError> {
    require_session(&state, &headers).await?;
    crate::client_tunnel_provision::claim_client_tunnel_config(&state).await?;
    emit_tunnel_event(&state, "tunnel.changed", "client", "claimed_remote");
    Ok(Json(ClientTunnelClaimResponse {
        ok: true,
        status: "claimed_remote".to_string(),
        error: None,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ClientTunnelSubdomainCheckQuery {
    pub(in crate::api) subdomain: String,
}

pub(in crate::api) async fn web_client_tunnel_subdomain_check(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Query(query): Query<ClientTunnelSubdomainCheckQuery>,
) -> Result<Json<SetupSubdomainCheckResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    let subdomain =
        ServerConfig::preview_client_subdomain(&query.subdomain).map_err(ApiError::bad_request)?;
    let router_url = config
        .router_api_base()
        .ok_or_else(|| ApiError::bad_request("router url is not configured"))?;
    let installation_id = config
        .router
        .identity
        .as_ref()
        .map(|identity| identity.installation_id.as_str());
    let availability = crate::client_tunnel_provision::check_subdomain_for_router_outcome(
        &state,
        router_url,
        &subdomain,
        installation_id,
    )
    .await?;
    Ok(Json(SetupSubdomainCheckResponse {
        ok: true,
        available: availability.available,
        checked: availability.checked,
        reason: availability.reason,
    }))
}

pub(in crate::api) async fn issue_client_tunnel_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelLeaseResponse>, ApiError> {
    require_session(&state, &headers).await?;
    crate::state::ensure_client_tunnel_running(state.clone(), "client_tunnel_api_start").await;
    emit_tunnel_event(&state, "tunnel.changed", "client", "started");
    Ok(Json(ClientTunnelLeaseResponse {
        ok: true,
        status: state
            .tunnels
            .status(&crate::clients::router::tunnel::client_tunnel_key())
            .await,
        message: "client tunnel supervisor started".to_string(),
    }))
}

pub(in crate::api) async fn stop_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let _claim = state.lock_client_tunnel_claim().await;
    crate::state::stop_client_tunnel(&state).await;
    let mut config = state.config.read().await.clone();
    config.client.tunnel_status = Some("stopped".to_string());
    let release_config = config.clone();
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    if release_config.has_registered_router_identity()
        && release_config.owner.email.is_some()
        && release_config.client.tunnel_subdomain.is_some()
    {
        let http_client = state.http_client().await;
        if let Err(error) =
            crate::clients::router::client::release_client_tunnel(&http_client, &release_config)
                .await
        {
            let message = error.to_string();
            tracing::warn!(error = %message, "router client tunnel release failed");
            state
                .mutate_shares_immediate(|shares| {
                    shares.last_router_error = Some(message);
                })
                .await
                .map_err(ApiError::internal)?;
        }
    }
    emit_tunnel_event(&state, "tunnel.changed", "client", "stopped");
    Ok(Json(
        client_tunnel_response(&state, &release_config, None, None).await,
    ))
}

pub(in crate::api) async fn router_tunnels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterTunnelsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(RouterTunnelsResponse {
        ok: true,
        tunnels: state.tunnels.statuses().await,
    }))
}

pub(in crate::api) async fn router_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterStatusResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let shares = state.shares.read().await;
    Ok(Json(RouterStatusResponse {
        ok: true,
        registered: shares.router_registered,
        last_error: shares.last_router_error.clone(),
        last_heartbeat_ms: shares.last_router_heartbeat_ms,
        pending_request_log_sync: crate::state::pending_router_log_count(&state).await,
    }))
}

pub(in crate::api) async fn router_diagnostics(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterDiagnosticsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    let shares = state.shares.read().await;
    let share_sync = shares
        .shares
        .iter()
        .map(|share| ShareSyncDiagnostic {
            share_id: share.id.clone(),
            share_name: share
                .display_name
                .clone()
                .unwrap_or_else(|| share.id.clone()),
            status: share.status.clone(),
            enabled: share.enabled,
            router_last_synced_at_ms: share.router_last_synced_at_ms,
            router_last_sync_error: share.router_last_sync_error.clone(),
            router_url: share.router_url.clone(),
        })
        .collect();
    Ok(Json(RouterDiagnosticsResponse {
        ok: true,
        router: RouterConfigView::from_config(&config.router),
        registered: shares.router_registered,
        last_error: shares.last_router_error.clone(),
        last_heartbeat_ms: shares.last_router_heartbeat_ms,
        pending_request_log_sync: crate::state::pending_router_log_count(&state).await,
        tunnels: state.tunnels.statuses().await,
        share_sync,
    }))
}

pub(in crate::api) async fn router_heartbeat(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterStatusResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let now = now_ms();
    let config = state.config.read().await.clone();
    let http_client = state.http_client().await;
    if let Err(error) =
        crate::clients::router::client::pending_share_edits(&http_client, &config, Vec::new()).await
    {
        let message = format!("router heartbeat probe failed: {error}");
        state
            .mutate_shares_immediate(|shares| {
                shares.router_registered = false;
                shares.last_router_error = Some(message.clone());
            })
            .await
            .map_err(ApiError::internal)?;
        return Err(ApiError::bad_gateway(message));
    }

    state
        .record_client_tunnel_heartbeat(now)
        .await
        .map_err(ApiError::internal)?;
    state
        .mutate_shares_debounced(|shares| {
            shares.last_router_heartbeat_ms = Some(now);
            shares.router_registered = true;
            shares.last_router_error = None;
        })
        .await;
    router_status(State(state), headers).await
}

pub(in crate::api) async fn router_register(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterRegisterResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        return Err(ApiError::bad_request("setup is incomplete"));
    }

    match state.register_router_installation().await {
        Ok(registration) => {
            state
                .complete_router_registration_control_plane("manual_router_register")
                .await
                .map_err(ApiError::internal)?;
            Ok(Json(RouterRegisterResponse {
                ok: true,
                registration,
            }))
        }
        Err(error) => {
            state
                .mutate_shares_immediate(|shares| {
                    shares.router_registered = false;
                    shares.last_router_error = Some(error.to_string());
                })
                .await
                .map_err(ApiError::internal)?;
            Err(ApiError::bad_gateway(format!(
                "router installation register failed: {error}"
            )))
        }
    }
}

pub(in crate::api) async fn router_pull_share_edits(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<RouterShareEditPullResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let summary = crate::state::pull_and_apply_pending_share_edits(state).await;
    Ok(Json(RouterShareEditPullResponse {
        ok: summary.error.is_none(),
        summary,
    }))
}

pub(in crate::api) fn spawn_share_upsert_sync(state: ServerState, share: Share) {
    let share_id = share.id.clone();
    tokio::spawn(async move {
        if let Err(error) = sync_share_upsert(state, share).await {
            tracing::error!(
                %error,
                %share_id,
                "background Router Share synchronization failed"
            );
        }
    });
}

pub(in crate::api) async fn sync_share_upsert(
    state: ServerState,
    share: Share,
) -> Result<(), String> {
    Box::pin(crate::state::sync_share_to_router_with_runtime_refresh(
        &state, &share.id,
    ))
    .await
    .map_err(|error| error.to_string())
}

pub(in crate::api) fn spawn_share_delete_sync(state: ServerState, tombstone: ShareDeleteTombstone) {
    crate::state::spawn_router_share_delete_retry(state, tombstone);
}

async fn client_tunnel_response(
    state: &ServerState,
    config: &crate::domain::settings::config::ServerConfig,
    remote_tunnel: Option<crate::clients::router::client::ClientTunnelView>,
    remote_error: Option<String>,
) -> ClientTunnelResponse {
    ClientTunnelResponse {
        ok: true,
        tunnel_subdomain: config.client.tunnel_subdomain.clone(),
        tunnel_status: config.client.tunnel_status.clone(),
        last_heartbeat_ms: config.client.last_heartbeat_ms,
        runtime_status: state
            .tunnels
            .status(&crate::clients::router::tunnel::client_tunnel_key())
            .await,
        remote_tunnel,
        remote_error,
    }
}
