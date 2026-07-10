use super::*;

use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareSyncOperation,
};

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
    if config.router.identity.is_some() {
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
    config
        .update_client_tunnel(input)
        .map_err(ApiError::bad_request)?;
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
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(response))
}

pub(in crate::api) async fn claim_client_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelClaimResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        let http_client = state.http_client().await;
        crate::clients::router::client::register_installation(&http_client, &mut config)
            .await
            .map_err(|error| {
                ApiError::bad_gateway(format!("router installation register failed: {error}"))
            })?;
        state
            .replace_config(config.clone())
            .await
            .map_err(ApiError::internal)?;
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
        subdomain,
        enabled: true,
    };
    let http_client = state.http_client().await;
    match crate::clients::router::client::claim_client_tunnel(&http_client, &config, tunnel).await {
        Ok(()) => {
            let mut next = config;
            next.client.tunnel_status = Some("claimed_remote".to_string());
            next.router.last_register_error = None;
            state
                .replace_config(next)
                .await
                .map_err(ApiError::internal)?;
            emit_tunnel_event(&state, "tunnel.changed", "client", "claimed_remote");
            Ok(Json(ClientTunnelClaimResponse {
                ok: true,
                status: "claimed_remote".to_string(),
                error: None,
            }))
        }
        Err(error) => {
            let mut next = config;
            next.client.tunnel_status = Some("claim_failed".to_string());
            next.router.last_register_error = Some(error.to_string());
            state
                .replace_config(next)
                .await
                .map_err(ApiError::internal)?;
            Err(ApiError::bad_gateway(format!(
                "router client tunnel claim failed: {error}"
            )))
        }
    }
}

pub(in crate::api) async fn issue_client_tunnel_lease(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ClientTunnelLeaseResponse>, ApiError> {
    require_session(&state, &headers).await?;
    crate::state::start_client_tunnel(state.clone()).await;
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
    if release_config.router.identity.is_some()
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
    let mut config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        return Err(ApiError::bad_request("setup is incomplete"));
    }

    let http_client = state.http_client().await;
    match crate::clients::router::client::register_installation(&http_client, &mut config).await {
        Ok(registration) => {
            state
                .replace_config(config)
                .await
                .map_err(ApiError::internal)?;
            state
                .mutate_shares_immediate(|shares| {
                    shares.router_registered = true;
                    shares.last_router_error = None;
                })
                .await
                .map_err(ApiError::internal)?;
            if let Err(error) = crate::state::reconcile_all_shares_to_router(state.clone()).await {
                tracing::warn!(error = %error, "automatic router share reconcile after registration failed");
            }
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
            let mut failed_config = config;
            failed_config.router.last_register_error = Some(error.to_string());
            state
                .replace_config(failed_config)
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
        let providers = state.providers.read().await.clone();
        let accounts = state.accounts.read().await.clone();
        let usage = state.usage.read().await.clone();
        let descriptor = descriptor_for_share_with_accounts_and_usage(
            &share,
            &providers,
            Some(&accounts),
            Some(&usage),
        );
        let op = ShareSyncOperation {
            kind: "upsert".to_string(),
            share_id: None,
            share: Some(descriptor),
        };
        sync_share_ops(state, vec![op]).await;
    });
}

pub(in crate::api) fn spawn_share_delete_sync(state: ServerState, share_id: String) {
    tokio::spawn(async move {
        let op = ShareSyncOperation {
            kind: "delete".to_string(),
            share_id: Some(share_id),
            share: None,
        };
        sync_share_ops(state, vec![op]).await;
    });
}

pub(in crate::api) async fn sync_share_ops(state: ServerState, ops: Vec<ShareSyncOperation>) {
    let synced_share_ids = ops
        .iter()
        .filter_map(|op| {
            op.share
                .as_ref()
                .map(|share| share.share_id.clone())
                .or_else(|| op.share_id.clone())
        })
        .collect::<Vec<_>>();
    let refresh_targets = ops
        .iter()
        .filter(|op| op.kind == "upsert")
        .filter_map(|op| {
            op.share
                .as_ref()
                .map(|share| (share.share_id.clone(), share.subdomain.clone()))
        })
        .collect::<Vec<_>>();
    let config = state.config.read().await.clone();
    if config.router.identity.is_none() {
        return;
    }
    let router_base = config.router_api_base().map(str::to_string);
    let http_client = state.http_client().await;
    match crate::clients::router::client::push_share_ops(&http_client, &config, ops).await {
        Ok(()) => {
            let now = now_ms();
            let refresh_error =
                notify_runtime_refreshes(&http_client, &config, &refresh_targets).await;
            state
                .mutate_shares_debounced(|store| {
                    store.router_registered = true;
                    store.last_router_error = refresh_error.clone();
                    for share_id in &synced_share_ids {
                        store.mark_router_sync(share_id, router_base.clone(), Ok(now));
                    }
                })
                .await;
        }
        Err(error) => {
            tracing::warn!(error = %error, "router share sync failed");
            let message = error.to_string();
            state
                .mutate_shares_debounced(|store| {
                    store.last_router_error = Some(message.clone());
                    for share_id in &synced_share_ids {
                        store.mark_router_sync(share_id, router_base.clone(), Err(message.clone()));
                    }
                })
                .await;
        }
    }
}

async fn notify_runtime_refreshes(
    http_client: &reqwest::Client,
    config: &crate::domain::settings::config::ServerConfig,
    refresh_targets: &[(String, String)],
) -> Option<String> {
    let mut last_error = None;
    for (share_id, subdomain) in refresh_targets {
        if let Err(error) = crate::clients::router::client::notify_runtime_refresh(
            http_client,
            config,
            share_id.clone(),
            subdomain.clone(),
        )
        .await
        {
            let message = error.to_string();
            tracing::warn!(
                share_id = %share_id,
                subdomain = %subdomain,
                error = %message,
                "router share runtime refresh failed"
            );
            last_error = Some(message);
        }
    }
    last_error
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
