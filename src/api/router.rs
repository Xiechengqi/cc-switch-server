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
    let mut config = state.config.read().await.clone();
    if !config.has_registered_router_identity() {
        state
            .register_router_installation()
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("router installation register failed: {error}"))
            })?;
        state
            .complete_router_registration_control_plane("client_tunnel_claim")
            .await
            .map_err(ApiError::internal)?;
        config = state.config_snapshot().await;
    }
    let owner_email = config
        .owner
        .email
        .clone()
        .ok_or_else(|| ApiError::bad_request("owner email is not configured"))?;
    let subdomain = config
        .client
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::bad_request("client tunnel subdomain is not configured"))?;
    let tunnel = crate::clients::router::client::ClientTunnelConfig {
        owner_email,
        subdomain: subdomain.clone(),
        enabled: true,
    };
    let http_client = state.http_client().await;
    if let Err(error) = crate::state::ensure_router_installation_owner_bound(&state, &config).await
    {
        let mut next = config;
        next.client.tunnel_status = Some("claim_failed".to_string());
        next.router.last_register_error = Some(error.to_string());
        state
            .replace_config(next)
            .await
            .map_err(ApiError::internal)?;
        return Err(ApiError::conflict(error.to_string()));
    }
    match crate::clients::router::client::claim_client_tunnel(&http_client, &config, tunnel).await {
        Ok(()) => {
            crate::client_tunnel_provision::mark_claim_success(&state, &mut config).await;
            emit_tunnel_event(&state, "tunnel.changed", "client", "claimed_remote");
            Ok(Json(ClientTunnelClaimResponse {
                ok: true,
                status: "claimed_remote".to_string(),
                error: None,
            }))
        }
        Err(error) => {
            let message = error.to_string();
            let mut next = config;
            next.client.tunnel_status = Some("claim_failed".to_string());
            next.router.last_register_error = Some(message.clone());
            state
                .replace_config(next)
                .await
                .map_err(ApiError::internal)?;
            {
                let mut shares = state.shares.write().await;
                shares.router_registered = false;
                shares.last_router_error = Some(message.clone());
            }
            if let Err(error) = state.save_shares().await {
                tracing::warn!(error = %error, "persist router claim failure failed");
            }
            if crate::client_tunnel_provision::is_subdomain_conflict_error(&message) {
                return Err(crate::client_tunnel_provision::subdomain_conflict_error(
                    &subdomain,
                    Some("already_claimed"),
                ));
            }
            Err(ApiError::bad_gateway(format!(
                "router client tunnel claim failed: {error}"
            )))
        }
    }
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

    let mut next_config = config;
    next_config.client.last_heartbeat_ms = Some(now);
    state
        .replace_config(next_config)
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
    tokio::spawn(async move {
        let _ = sync_share_upsert(state, share).await;
    });
}

pub(in crate::api) async fn sync_share_upsert(
    state: ServerState,
    share: Share,
) -> Result<(), String> {
    crate::state::sync_one_share_to_router(&state, &share.id)
        .await
        .map_err(|error| error.to_string())?;
    let Some(current) = state.shares.read().await.get(&share.id).cloned() else {
        return Ok(());
    };
    let Some(subdomain) = current.tunnel_subdomain.as_deref() else {
        return Ok(());
    };
    let config = state.config_snapshot().await;
    let http_client = state.http_client().await;
    if let Err(error) = crate::clients::router::client::notify_runtime_refresh(
        &http_client,
        &config,
        current.id.clone(),
        subdomain.to_string(),
    )
    .await
    {
        let message = error.to_string();
        tracing::warn!(share_id = %current.id, error = %message, "router share runtime refresh failed");
        state
            .mutate_shares_debounced(|store| {
                store.last_router_error = Some(message.clone());
            })
            .await;
    }
    Ok(())
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
