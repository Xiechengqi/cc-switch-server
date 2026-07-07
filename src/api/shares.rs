use super::*;

use crate::domain::sharing::router_contract::descriptor_for_share_with_accounts_and_usage;
use std::collections::BTreeMap;

pub(in crate::api) async fn list_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ListSharesResponse {
        ok: true,
        shares: state.shares.read().await.shares.clone(),
    }))
}

pub(in crate::api) async fn export_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    list_shares(State(state), headers).await
}

pub(in crate::api) async fn import_shares(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportSharesRequest>,
) -> Result<Json<ImportSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let imported = state
        .mutate_shares_immediate(|store| store.import_shares(input.shares))
        .await
        .map_err(ApiError::internal)?;
    state.emit_event(
        ServerEvent::new("share.imported", "share").message(format!("imported {imported} shares")),
    );
    Ok(Json(ImportSharesResponse { ok: true, imported }))
}

pub(in crate::api) async fn upsert_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<UpsertShareInput>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    if input.owner_email.is_none() {
        input.owner_email = state.config.read().await.owner.email.clone();
    }
    let share = state
        .mutate_shares_immediate(|store| store.upsert(input))
        .await
        .map_err(ApiError::internal)?;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "upserted");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn share_connect_info(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ShareConnectInfoResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    Ok(Json(connect_info_for_share(&config, &share)?))
}

pub(in crate::api) async fn update_share_subdomain(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareSubdomainRequest>,
) -> Result<Json<UpdateShareSubdomainResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let subdomain = crate::domain::sharing::shares::normalize_share_subdomain(&input.subdomain)
        .map_err(ApiError::bad_request)?;
    let config = state.config.read().await.clone();
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let current = state
        .shares
        .read()
        .await
        .get(&id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let mut candidate = current.clone();
    candidate.tunnel_subdomain = Some(subdomain.clone());
    let descriptor = descriptor_for_share_with_accounts_and_usage(
        &candidate,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    let mut remote_claimed = false;
    if config.router.identity.is_some() {
        let http_client = state.http_client().await;
        crate::clients::router::client::claim_share_subdomain(&http_client, &config, descriptor)
            .await
            .map_err(|error| ApiError::bad_gateway(error.to_string()))?;
        remote_claimed = true;
    }
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .update_subdomain(&id, subdomain)
                .map_err(map_share_patch_error)
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "subdomain_updated");
    Ok(Json(UpdateShareSubdomainResponse {
        ok: true,
        remote_claimed,
        share,
    }))
}

pub(in crate::api) async fn delete_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = state
        .mutate_shares_immediate(|store| store.delete(&id))
        .await
        .map_err(ApiError::internal)?;
    if deleted {
        spawn_share_delete_sync(state.clone(), id.clone());
        state.emit_event(
            ServerEvent::new("share.deleted", "share")
                .id(id.clone())
                .message("deleted"),
        );
    }
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

pub(in crate::api) async fn pause_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .pause(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "paused");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn resume_share(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .resume(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "resumed");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn start_share_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .set_share_tunnel_status(&id, "active", None)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    crate::state::start_share_tunnel(state.clone(), id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "tunnel_started");
    emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_started");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn stop_share_tunnel(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .set_share_tunnel_status(&id, "stopped", None)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    crate::state::stop_share_tunnel(&state, &id).await;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "tunnel_stopped");
    emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_stopped");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn restore_share_tunnels(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let shares = state
        .mutate_shares_immediate(|store| store.restore_auto_start())
        .await
        .map_err(ApiError::internal)?;
    for share in shares
        .iter()
        .filter(|share| share.auto_start && share.enabled)
    {
        crate::state::start_share_tunnel(state.clone(), share.id.clone()).await;
        spawn_share_upsert_sync(state.clone(), share.clone());
        emit_share_event(&state, "share.changed", share, "tunnel_restored");
        emit_tunnel_event(&state, "tunnel.changed", &share.id, "share_restored");
    }
    Ok(Json(ListSharesResponse { ok: true, shares }))
}

pub(in crate::api) async fn reset_share_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .reset_usage(&id)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "usage_reset");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn update_share_binding(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareBindingRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .update_binding(&id, input.binding)
                .map_err(|error| match error {
                    ShareUpdateError::NotFound => ApiError::not_found("share not found"),
                    ShareUpdateError::MustBePaused => ApiError::conflict(error.to_string()),
                })
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "binding_updated");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn replace_share_acl(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<ReplaceShareAclRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .replace_acl(&id, input.acl)
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "acl_replaced");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn update_share_market_grant(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<UpdateShareMarketGrantRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let market_grant = input
        .market_grant
        .map(normalize_share_market_grant)
        .transpose()?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = state
        .mutate_shares_immediate(|store| {
            store
                .update_market_grant(&id, market_grant)
                .ok_or_else(|| ApiError::not_found("share not found"))?;
            store.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
            store
                .shares
                .iter()
                .find(|share| share.id == id)
                .cloned()
                .ok_or_else(|| ApiError::not_found("share not found"))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "market_grant_updated");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn list_share_markets(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListShareMarketsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let markets = fetch_public_markets_from_router(&state).await?;
    Ok(Json(ListShareMarketsResponse { ok: true, markets }))
}

pub(in crate::api) async fn authorize_share_market(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(input): Json<AuthorizeShareMarketRequest>,
) -> Result<Json<UpsertShareResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let market_email = crate::clients::router::email_auth::normalize_email(&input.market_email)
        .map_err(map_email_auth_error)?;
    let markets = fetch_public_markets_from_router(&state).await?;
    let public_market_emails = markets
        .iter()
        .map(|market| market.email.trim().to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>();
    let selected_market = markets.iter().find(|market| {
        market.email.eq_ignore_ascii_case(&market_email) && market.market_kind == "share"
    });
    if selected_market.is_none() {
        return Err(ApiError::bad_request(
            "marketEmail must belong to a registered share market",
        ));
    }
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = state
        .mutate_shares_immediate(|store| {
            let share = store
                .authorize_share_market(&id, market_email, &public_market_emails)
                .map_err(map_share_patch_error)?;
            store.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
            Ok::<_, ApiError>(store.get(&id).cloned().unwrap_or(share))
        })
        .await
        .map_err(ApiError::internal)??;
    spawn_share_upsert_sync(state.clone(), share.clone());
    emit_share_event(&state, "share.changed", &share, "share_market_authorized");
    Ok(Json(UpsertShareResponse { ok: true, share }))
}

pub(in crate::api) async fn refresh_share_snapshots(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListSharesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let shares = state
        .mutate_shares_debounced(|store| {
            store.refresh_runtime_snapshots(&providers, Some(&accounts), &usage)
        })
        .await;
    state.emit_event(ServerEvent::new("share.changed", "share").message("runtime_snapshot"));
    Ok(Json(ListSharesResponse { ok: true, shares }))
}

pub(in crate::api) fn emit_share_event(
    state: &ServerState,
    event_type: &str,
    share: &Share,
    message: &str,
) {
    state.emit_event(
        ServerEvent::new(event_type, "share")
            .id(share.id.clone())
            .app(share.app)
            .message(message),
    );
}

pub(in crate::api) fn emit_tunnel_event(
    state: &ServerState,
    event_type: &str,
    tunnel_id: &str,
    message: &str,
) {
    state.emit_event(
        ServerEvent::new(event_type, "tunnel")
            .id(tunnel_id.to_string())
            .message(message),
    );
}

pub(in crate::api) fn connect_info_for_share(
    config: &ServerConfig,
    share: &Share,
) -> Result<ShareConnectInfoResponse, ApiError> {
    let subdomain = share
        .tunnel_subdomain
        .clone()
        .ok_or_else(|| ApiError::conflict("share subdomain is not configured"))?;
    let router_domain = config
        .router
        .domain
        .clone()
        .or_else(|| router_domain_from_url(config.router.url.as_deref()))
        .ok_or_else(|| ApiError::conflict("router domain is not configured"))?;
    let direct_url = share
        .router_url
        .clone()
        .unwrap_or_else(|| format!("https://{subdomain}.{router_domain}"));
    let snippets = [
        (
            AppKind::Claude,
            "Claude / Anthropic",
            vec![
                ("ANTHROPIC_BASE_URL", direct_url.clone()),
                ("ANTHROPIC_AUTH_TOKEN", "<user_api_token>".to_string()),
            ],
        ),
        (
            AppKind::Codex,
            "Codex / OpenAI-compatible",
            vec![
                (
                    "OPENAI_BASE_URL",
                    format!("{}/v1", direct_url.trim_end_matches('/')),
                ),
                ("OPENAI_API_KEY", "<user_api_token>".to_string()),
            ],
        ),
        (
            AppKind::Gemini,
            "Gemini",
            vec![
                ("GEMINI_BASE_URL", direct_url.clone()),
                ("GEMINI_API_KEY", "<user_api_token>".to_string()),
            ],
        ),
    ]
    .into_iter()
    .map(|(app, title, values)| {
        let env = values
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>();
        ShareConnectSnippet {
            app,
            title: title.to_string(),
            env,
        }
    })
    .collect::<Vec<_>>();
    Ok(ShareConnectInfoResponse {
        ok: true,
        share_id: share.id.clone(),
        direct_url,
        subdomain,
        router_domain,
        snippets,
        note: "The caller must use their own cc-switch user_api_token as the bearer/API key."
            .to_string(),
    })
}

pub(in crate::api) fn router_domain_from_url(url: Option<&str>) -> Option<String> {
    let value = url?.trim();
    let without_scheme = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .unwrap_or(value);
    without_scheme
        .split('/')
        .next()
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
}

pub(in crate::api) async fn fetch_public_markets_from_router(
    state: &ServerState,
) -> Result<Vec<PublicShareMarket>, ApiError> {
    let config = state.config.read().await.clone();
    let api_base = config
        .router_api_base()
        .ok_or_else(|| ApiError::conflict("router API base is not configured"))?
        .trim_end_matches('/')
        .to_string();
    let http_client = state.http_client().await;
    let response = http_client
        .get(format!("{api_base}/v1/markets"))
        .send()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("fetch share markets failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(ApiError::bad_gateway(format!(
            "fetch share markets failed: {status}: {body}"
        )));
    }
    let response = response
        .json::<ListShareMarketsResponse>()
        .await
        .map_err(|error| ApiError::bad_gateway(format!("parse share markets failed: {error}")))?;
    Ok(response.markets)
}
