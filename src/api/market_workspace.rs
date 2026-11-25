//! This layer runs after Router ingress verification. Unknown routes, including
//! future management APIs, have no access to a leased workspace by default.
use super::*;
pub(super) mod chat;
use crate::clients::router::ingress::IngressContext;
use crate::market_lease::{Command, Operation, ACCESS_EVENT_BATCH};
use axum::extract::OriginalUri;

pub(super) async fn control(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, ApiError> {
    let path = uri.path();
    control::verify_control_request(&state, path, &headers, &body).await?;
    let config = state.config_snapshot().await;
    let owner = config
        .owner
        .email
        .as_deref()
        .ok_or_else(|| ApiError::conflict("owner is missing"))?;
    if matches!(path, "/_ctl/market/upgrade" | "/_ctl/market/upgrade-status") {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Upgrade {
            owner_email: String,
            restart_after: Option<bool>,
            task_id: Option<String>,
        }
        let v: Upgrade = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
        let (listed, grant, _) = state.market_workspace.access();
        // The Router may still treat a Client as listed while a List or Unlist command
        // is in flight, so only the owner is required here. A journal owner, when
        // present, must agree with the installation owner.
        if !owner.eq_ignore_ascii_case(&v.owner_email)
            || listed.is_some_and(|e| !e.eq_ignore_ascii_case(owner))
        {
            return Err(ApiError::forbidden("installation owner required"));
        }
        if path == "/_ctl/market/upgrade-status" {
            let id = v
                .task_id
                .as_deref()
                .filter(|id| !id.is_empty() && id.len() <= 128)
                .ok_or_else(|| ApiError::bad_request("task ID required"))?;
            let snapshot = super::self_update::upgrade_status_for_task(&state, Some(id)).await?;
            return Ok(Json(
                serde_json::to_value(snapshot.0).map_err(ApiError::internal)?,
            ));
        }
        if !config.upgrade_policy.delegate_upgrade_to_router_owner {
            return Err(ApiError::forbidden("owner disabled delegated upgrades"));
        }
        if let Some(g) = grant {
            state
                .market_workspace
                .record_event(&g.lease_id, "upgrade")
                .map_err(ApiError::internal)?;
        }
        let id = super::self_update::start_upgrade_for_actor(
            &state,
            v.restart_after.unwrap_or(true),
            "router-owner",
            true,
        )
        .await?;
        return Ok(Json(json!({"ok":true,"taskId":id})));
    }
    let _command = if path == "/_ctl/market/command" {
        Some(state.market_workspace.commands.lock().await)
    } else {
        None
    };
    let clear = if path == "/_ctl/market/status" && !state.market_workspace.view().fenced {
        true
    } else {
        state.market_workspace_drained().await
    };
    let mut hardware = None;
    match path {
        "/_ctl/market/command" => {
            let command: Command = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
            // A retried command that already took effect is acknowledged as is. The
            // tenant may have rented out a Share since the original reply was lost.
            let replay = state.market_workspace.view().last.as_ref() == Some(&command);
            if !replay
                && matches!(
                    command.action,
                    Operation::List { .. } | Operation::Begin { .. }
                )
                && !clear
            {
                return Err(ApiError::conflict(
                    "Share rentals must finish before listing",
                ));
            }
            if matches!(command.action, Operation::List { .. })
                && state.market_workspace.view().last.is_none()
            {
                state
                    .create_consistent_backup(Some("Before first Client Market listing".into()))
                    .await
                    .map_err(ApiError::internal)?;
            }
            let writer = state.clone();
            let owner = owner.to_owned();
            tokio::task::spawn_blocking(move || {
                writer.market_workspace.apply(
                    command,
                    &owner,
                    chrono::Utc::now().timestamp(),
                    clear,
                )
            })
            .await
            .map_err(ApiError::internal)?
            .map_err(|e| ApiError::conflict(e.to_string()))?;
            if state.market_workspace.switch_required() {
                state.begin_shutdown();
            }
        }
        "/_ctl/market/check" => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Check {
                challenge: String,
            }
            let check: Check = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
            if check.challenge.len() != 32
                || !check.challenge.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(ApiError::bad_request("invalid egress challenge"));
            }
            let base = config
                .router_api_base()
                .ok_or_else(|| ApiError::conflict("Router unavailable"))?;
            crate::infra::http::outbound_client_builder()
                .map_err(ApiError::internal)?
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(4))
                .build()
                .map_err(ApiError::internal)?
                .post(format!(
                    "{base}/v1/client-market/online/egress/{}",
                    check.challenge
                ))
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(ApiError::internal)?;
            hardware = Some(
                crate::workspace_hardware::workspace_hardware(state.config_dir.clone())
                    .await
                    .map_err(ApiError::internal)?,
            );
        }
        "/_ctl/market/status" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Ack {
                lease_id: Option<String>,
                #[serde(default)]
                received: Vec<String>,
                #[serde(default)]
                hardware: bool,
            }
            let ack: Ack = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
            if ack.hardware {
                hardware = Some(
                    crate::workspace_hardware::workspace_hardware(state.config_dir.clone())
                        .await
                        .map_err(ApiError::internal)?,
                );
            }
            if let Some(id) = ack.lease_id {
                state
                    .market_workspace
                    .acknowledge(&id, &ack.received)
                    .map_err(ApiError::internal)?;
            }
        }
        _ => return Err(ApiError::bad_request("unknown operation")),
    }
    let journal = state.market_workspace.view();
    Ok(Json(
        json!({"capability":"marketLeaseV1","ownerEmail":owner,"listedOwner":journal.owner,
        "sequence":journal.last.as_ref().map_or(0,|c|c.sequence), "grant":journal.grant,
        "runningLease":state.market_workspace.boot_lease,"switching":state.market_workspace.switch_required(),
        "events":journal.events.iter().take(ACCESS_EVENT_BATCH).collect::<Vec<_>>(),"fenced":journal.fenced,"cleanupSafe":clear,"hardware":hardware,"version":env!("CARGO_PKG_VERSION")}),
    ))
}

fn in_family(path: &str, base: &str) -> bool {
    path == base
        || path
            .strip_prefix(base)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub(super) fn workspace_command(command: &str) -> bool {
    super::web_runtime::command(command).is_some_and(|c| {
        c.workspace_access == super::web_runtime::WorkspaceAccess::Tenant && c.implemented
    })
}
// Match Axum's registered route template. Newly added management endpoints
// require an explicit workspace permission, including siblings of allowed APIs.
fn inference_path(route: &str) -> bool {
    matches!(
        route,
        "/v1/models"
            | "/models"
            | "/backend-api/codex/models"
            | "/v1/messages"
            | "/claude/v1/messages"
            | "/v1/messages/count_tokens"
            | "/claude/v1/messages/count_tokens"
            | "/v1/chat/completions"
            | "/v1/v1/chat/completions"
            | "/chat/completions"
            | "/codex/v1/chat/completions"
            | "/v1/responses"
            | "/v1/responses/compact"
            | "/v1/v1/responses"
            | "/v1/v1/responses/compact"
            | "/responses"
            | "/responses/compact"
            | "/codex/v1/responses"
            | "/codex/v1/responses/compact"
            | "/backend-api/codex/responses"
            | "/backend-api/codex/responses/compact"
            | "/v1/responses/input_tokens"
            | "/responses/input_tokens"
            | "/alpha/search"
            | "/v1/alpha/search"
            | "/backend-api/codex/alpha/search"
            | "/v1/images/files/:token"
            | "/v1/images/generations"
            | "/images/generations"
            | "/v1/images/edits"
            | "/images/edits"
            | "/v1/videos/generations"
            | "/videos/generations"
            | "/v1/videos/:request_id"
            | "/videos/:request_id"
            | "/v1beta/*path"
            | "/gemini/v1/*path"
            | "/gemini/v1beta/*path"
    )
}
fn tenant_path(route: &str, path: &str) -> bool {
    if route == "/web-api/invoke/*command" {
        return path
            .strip_prefix("/web-api/invoke/")
            .is_some_and(workspace_command);
    }
    matches!(
        route,
        "/web-api/market/chat"
            | "/web-api/market/workspace"
            | "/web-api/market/token"
            | "/web-api/market/backups"
            | "/web-api/market/backups/:id/restore"
            | "/api/provider-bundles"
            | "/api/provider-bundles/order"
            | "/api/provider-bundles/validate-draft"
            | "/api/provider-bundles/:id"
            | "/api/provider-bundles/:id/delete-preview"
            | "/api/provider-bundles/:id/rebind-custom"
            | "/api/provider-bundles/:id/share-preflight"
            | "/api/providers"
            | "/api/providers/:id"
            | "/api/providers/export"
            | "/api/providers/import"
            | "/api/providers/account-bindings/migration"
            | "/api/providers/health"
            | "/api/providers/test"
            | "/api/providers/:id/test"
            | "/api/providers/:id/inference-test"
            | "/api/providers/:id/coding-plan-quota"
            | "/api/providers/:id/account-usage"
            | "/api/providers/:id/cursor-account"
            | "/api/providers/:id/fetch-models"
            | "/api/providers/:id/delete-preview"
            | "/api/providers/:id/adopt-profile"
            | "/api/providers/:id/rebind-custom"
            | "/api/providers/:id/clone-as-custom"
            | "/api/providers/from-preset"
            | "/api/accounts"
            | "/api/accounts/codex/active"
            | "/api/accounts/capabilities"
            | "/api/accounts/import-templates"
            | "/api/accounts/claude/credentials/import"
            | "/api/accounts/grok/auth-json/import"
            | "/api/accounts/kiro/credentials/import"
            | "/api/accounts/kiro/api-key/import"
            | "/api/accounts/login/start"
            | "/api/accounts/login/callback"
            | "/api/accounts/login/finish"
            | "/api/accounts/login/cancel"
            | "/api/accounts/copilot/device/start"
            | "/api/accounts/copilot/device/poll"
            | "/api/accounts/kiro/device/start"
            | "/api/accounts/kiro/device/poll"
            | "/api/accounts/amazon-q/device/start"
            | "/api/accounts/amazon-q/device/poll"
            | "/api/accounts/codex/device/start"
            | "/api/accounts/codex/device/poll"
            | "/api/accounts/codex/device/cancel"
            | "/api/accounts/grok/device/start"
            | "/api/accounts/grok/device/poll"
            | "/api/accounts/grok/device/cancel"
            | "/api/accounts/kimi/device/start"
            | "/api/accounts/kimi/device/poll"
            | "/api/accounts/kimi/device/cancel"
            | "/api/accounts/qoder/device/start"
            | "/api/accounts/qoder/device/poll"
            | "/api/accounts/qoder/device/cancel"
            | "/api/accounts/buddy/import"
            | "/api/accounts/codebuddy/login/start"
            | "/api/accounts/workbuddy/login/start"
            | "/api/accounts/codebuddy/login/poll"
            | "/api/accounts/workbuddy/login/poll"
            | "/api/accounts/codebuddy/login/cancel"
            | "/api/accounts/workbuddy/login/cancel"
            | "/api/accounts/trae/login/start"
            | "/api/accounts/trae/login/status"
            | "/api/accounts/trae/login/complete"
            | "/api/accounts/trae/login/cancel"
            | "/api/accounts/qoder/pat/import"
            | "/api/accounts/:id"
            | "/api/accounts/:id/delete-preview"
            | "/api/accounts/:id/refresh"
            | "/api/accounts/:id/refresh-plan"
            | "/api/accounts/:id/quota"
            | "/api/accounts/:id/antigravity/access/reset"
            | "/api/accounts/:id/checkin"
            | "/api/providers/:id/limits"
            | "/api/shares"
            | "/api/shares/reuse-candidates"
            | "/api/shares/export"
            | "/api/shares/import"
            | "/api/shares/:id"
            | "/api/shares/:id/connect-info"
            | "/api/shares/:id/subdomain"
            | "/api/shares/:id/binding"
            | "/api/shares/:id/bindings"
            | "/api/shares/:id/bindings/remove"
            | "/api/shares/:id/pause"
            | "/api/shares/:id/resume"
            | "/api/shares/:id/tunnel/start"
            | "/api/shares/:id/tunnel/stop"
            | "/api/shares/:id/reset-usage"
            | "/api/shares/runtime-snapshot"
            | "/web-api/usage/overview"
            | "/web-api/usage/trends"
            | "/web-api/usage/facets"
            | "/web-api/usage/provider-bundles"
            | "/web-api/usage/models"
            | "/web-api/usage/shares"
            | "/web-api/usage/requests/:id"
            | "/web-api/usage/requests"
            | "/api/events"
            | "/web-api/events"
            | "/web-api/context"
            | "/api/auth/me"
            | "/api/proxy/capabilities"
            | "/api/provider-limits"
            | "/api/provider-registry"
            | "/api/provider-presets"
            | "/api/provider-matrix"
            | "/api/provider-coverage"
            | "/api/provider-type"
    )
}
pub(super) async fn authorize(
    State(state): State<ServerState>,
    mut request: Request,
    next: Next,
) -> Response {
    let (owner, grant, live) = state.market_workspace.access();
    let Some(owner) = owner.as_deref() else {
        return next.run(request).await;
    };
    let path = request.uri().path();
    let route = request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or("", |v| v.as_str());
    if matches!(
        path,
        "/_ctl/market/check"
            | "/_ctl/market/command"
            | "/_ctl/market/status"
            | "/_ctl/market/upgrade"
            | "/_ctl/market/upgrade-status"
            | "/health"
            | "/_share-router/health"
            | "/ready"
            | "/version"
    ) {
        return next.run(request).await;
    }
    let denied = || workspace_error(StatusCode::FORBIDDEN, "workspace_leased").into_response();
    // Reject every alternate entry capable of changing installation identity.
    if grant.is_none() {
        if path.starts_with("/_ctl/client-subdomain-adoption")
            || in_family(path, "/api/backup") && path.ends_with("/restore")
            || in_family(path, "/api/backups") && path.ends_with("/restore")
            || in_family(path, "/api/setup") && path != "/api/setup/status"
            || in_family(path, "/api/router") && request.method() != axum::http::Method::GET
            || path.strip_prefix("/web-api/invoke/").is_some_and(|cmd| {
                matches!(
                    cmd,
                    "complete_server_setup"
                        | "email_auth_change_owner_email"
                        | "email_auth_request_owner_change_code"
                        | "restore_backup"
                        | "restore_db_backup"
                        | "configure_tunnel"
                        | "update_client_tunnel"
                        | "claim_client_tunnel"
                        | "stop_client_tunnel"
                        | "set_owner_email"
                        | "change_owner_email"
                        | "save_router_settings"
                        | "set_router_config"
                        | "set_client_tunnel_subdomain"
                        | "save_client_tunnel"
                        | "set_share_router_config"
                )
            })
        {
            return workspace_error(StatusCode::FORBIDDEN, "unlist_first").into_response();
        }
        if let Some(email) = session::router_web_user_email(request.headers()) {
            if email != owner {
                return denied();
            }
        }
        return next.run(request).await;
    }
    let grant = grant.unwrap();
    let ctx = request.extensions().get::<IngressContext>();
    let header_email = session::router_web_user_email(request.headers());
    let label = ctx.and_then(|c| c.public_host.split('.').next());
    if live && ctx.is_none() && inference_path(route) {
        // The inference router authenticates the local peer and resolves an
        // explicit Share in the tenant workspace before dispatch.
        return next.run(request).await;
    }
    if path.starts_with("/_ctl/") {
        // These individual handlers authenticate the literal path and body. Revoke
        // receipts must still be deliverable after public traffic has been fenced.
        return if matches!(
            path,
            "/_ctl/apply_share_settings"
                | "/_ctl/refresh_share_usage"
                | "/_ctl/preview_account_reset"
                | "/_ctl/preview_value_quota"
                | "/_ctl/quota_conversion_receipt"
                | "/_ctl/edit_value_usage"
                | "/_ctl/prepare_quota_history"
                | "/_ctl/preview_share_capabilities"
                | "/_ctl/verify_share_account_recovery"
        ) {
            next.run(request).await
        } else {
            denied()
        };
    }
    if ctx.is_some_and(|c| c.share_id.is_some()) {
        let bound = label
            .and_then(|l| l.split_once("--"))
            .is_some_and(|(_, alias)| alias == grant.alias);
        let share_endpoint = matches!(
            route,
            "/_share-router/share-runtime"
                | "/_share-router/model-health"
                | "/_share-router/model-health/batch"
                | "/_share-router/model-health/batch-v2"
                | "/_share-router/request-logs"
        );
        return if live && bound && (share_endpoint || inference_path(route)) {
            next.run(request).await
        } else {
            denied()
        };
    }
    if label == Some(grant.alias.as_str()) {
        if !live {
            return workspace_error(StatusCode::GONE, "lease_ended").into_response();
        }
        // These handlers consume a provider-bound OAuth state from this running workspace.
        if matches!(
            path,
            "/web-api/oauth/claude-cli/callback" | "/web-api/oauth/openai-cli/callback"
        ) && request.method() == axum::http::Method::GET
        {
            return next.run(request).await;
        }
        if header_email.is_none() {
            return workspace_error(StatusCode::UNAUTHORIZED, "tenant_session_required")
                .into_response();
        }
        return if header_email.as_deref() == Some(&grant.tenant_email) && tenant_path(route, path) {
            next.run(request).await
        } else {
            denied()
        };
    }
    let on_owner =
        ctx.is_none() || label == state.config.read().await.client.tunnel_subdomain.as_deref();
    if !on_owner {
        return denied();
    }
    if matches!(
        path,
        "/api/auth/login"
            | "/api/auth/email/request-code"
            | "/api/auth/email/verify-code"
            | "/web-api/auth/email/request-code"
            | "/web-api/auth/email/verify-code"
            | "/web-api/auth/methods"
            | "/web-api/auth/password/login"
            | "/web-api/auth/password/refresh"
            | "/web-api/auth/password/logout"
            | "/web-api/auth/session/refresh"
            | "/web-api/context"
    ) && header_email.as_deref().is_none_or(|e| e == owner)
    {
        return next.run(request).await;
    }
    let principal = session::resolve_web_admin_principal(&state, request.headers())
        .await
        .ok()
        .flatten();
    let Some(principal) = principal else {
        return if path.starts_with("/web-api/market/") || path.starts_with("/web-api/terminal/") {
            workspace_error(StatusCode::UNAUTHORIZED, "owner_session_required").into_response()
        } else {
            denied()
        };
    };
    let local = principal.source == session::WebPrincipalSource::LocalOwner;
    if !local
        && (principal.source != session::WebPrincipalSource::Router
            || principal.user_email() != owner)
    {
        return denied();
    }
    let independent = !local
        && request
            .headers()
            .get("x-cc-switch-owner-proof")
            .and_then(|v| v.to_str().ok())
            .and_then(|t| state.web_auth.authenticate_access_token(t).ok().flatten())
            .is_some();
    let maintenance = path.starts_with("/web-api/terminal/")
        || matches!(
            path,
            "/web-api/market/maintenance"
                | "/web-api/market/maintenance/status"
                | "/web-api/market/maintenance/upgrade-status"
                | "/api/admin/upgrade"
                | "/api/admin/rollback"
                | "/api/admin/restart"
                | "/web-api/invoke/start_admin_upgrade"
                | "/web-api/invoke/restart_server_service"
                | "/web-api/invoke/rollback_server_service"
        );
    if maintenance && !(local || independent) {
        return workspace_error(StatusCode::FORBIDDEN, "owner_proof_required").into_response();
    }
    if path == "/web-api/market/workspace" || maintenance {
        if let Some(kind) = match path {
            "/web-api/market/workspace" => Some("assistance"),
            "/web-api/terminal/stream" => Some("terminal"),
            "/web-api/market/maintenance" => Some("host_settings"),
            "/api/admin/upgrade" | "/web-api/invoke/start_admin_upgrade" => Some("upgrade"),
            "/api/admin/restart" | "/web-api/invoke/restart_server_service" => Some("restart"),
            "/api/admin/rollback" | "/web-api/invoke/rollback_server_service" => Some("rollback"),
            _ => None,
        } {
            if state
                .market_workspace
                .record_visit(
                    &grant.lease_id,
                    kind,
                    request
                        .headers()
                        .get("x-cc-switch-workspace-visit")
                        .and_then(|v| v.to_str().ok()),
                )
                .is_err()
            {
                return ApiError::internal("record owner access").into_response();
            }
        }
        if independent && path != "/web-api/market/workspace" {
            // These routes use the existing local-owner maintenance policy.
            let proof = request.headers_mut().remove("x-cc-switch-owner-proof");
            if let Some(value) = proof.and_then(|v| {
                v.to_str()
                    .ok()
                    .and_then(|v| format!("Bearer {v}").parse().ok())
            }) {
                let headers = request.headers_mut();
                headers.remove("x-cc-switch-web-user-email");
                headers.remove("x-cc-switch-web-role");
                headers.insert(axum::http::header::AUTHORIZATION, value);
            }
        }
        return next.run(request).await;
    }
    denied()
}

fn workspace_error(status: StatusCode, code: &'static str) -> ApiError {
    let mut error = ApiError::new(status, code);
    error.code = Some(code);
    error
}

pub(super) async fn maintenance_status(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    session::require_local_server_owner_session(&state, &headers).await?;
    if state.market_workspace.access().1.is_none() {
        return Err(ApiError::conflict("leased Client required"));
    }
    Ok(Json(
        json!({"authorized":true,"instanceId":state.process_instance_id}),
    ))
}

pub(super) async fn view(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let principal = session::require_web_admin_session(&state, &headers).await?;
    let journal = state.market_workspace.view();
    let Some(grant) = journal.grant else {
        return Ok(Json(json!({"role":"owner"})));
    };
    let role = if principal.source == session::WebPrincipalSource::LeaseTenant {
        "tenant"
    } else {
        "assistance"
    };
    let waiting_on_shares = journal.fenced && !state.market_workspace_drained().await;
    // Counts and stable codes only. Raw request bodies, logs and account objects can contain secrets.
    let (shares, registered, last_heartbeat_ms) = {
        let store = state.shares.read().await;
        (
            store.shares.len(),
            store.router_registered,
            store.last_router_heartbeat_ms,
        )
    };
    let config = state.config_snapshot().await;
    let maintenance = (role == "assistance").then(|| json!({
        "terminalEnabled":config.enable_web_terminal, "terminalAvailable":config.is_web_terminal_enabled(),
        "upgradePolicy":config.upgrade_policy
    }));
    let hardware = if role == "assistance" {
        crate::workspace_hardware::workspace_hardware(state.config_dir.clone())
            .await
            .ok()
    } else {
        None
    };
    let providers = {
        let store = state.providers.read().await;
        let usage = state.usage.read().await;
        store.list(None).iter().map(|p|{
            let plan=store.runtime_plan(p.app,&p.provider.id);
            let h=crate::domain::health::provider_health_for_plan(p,&usage,plan.as_deref());
            json!({"app":h.app,"providerId":h.provider_id,"status":h.status,"statusCode":h.last_status_code,"requests":h.requests,"failures":h.failures})
        }).collect::<Vec<_>>()
    };
    let log = state.audit_log();
    let audit = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<Value>> {
        let Some(mut cursor) = log.latest_cursor()? else {
            return Ok(vec![]);
        };
        cursor.sequence = cursor.sequence.saturating_sub(20);
        Ok(log
            .read_batch(Some(&cursor), 20)?
            .events
            .into_iter()
            .map(|e| json!({"time":e.timestamp_ms,"status":e.status_code,"sequence":e.sequence}))
            .collect())
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::internal)?;
    Ok(Json(
        json!({"role":role,"leaseId":grant.lease_id,"expiresAt":grant.expires_at,"shareCount":shares,"providers":providers,"audit":audit,
        "fenced":journal.fenced,"cleaning":journal.purging,"localOwner":principal.source == session::WebPrincipalSource::LocalOwner,
        "pendingEvents":journal.events.len(),"waitingOnShares":waiting_on_shares,"version":env!("CARGO_PKG_VERSION"),"events":journal.events.iter().rev().take(50).collect::<Vec<_>>(),"hardware":hardware,"maintenance":maintenance,
        "connectivity":{"registered":registered,"lastHeartbeatMs":last_heartbeat_ms}}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Maintenance {
    terminal_enabled: bool,
    upgrade_policy: crate::domain::settings::config::UpgradePolicyConfig,
}
pub(super) async fn maintenance(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(v): Json<Maintenance>,
) -> Result<Json<Value>, ApiError> {
    session::require_local_server_owner_session(&state, &headers).await?;
    let (_, grant, _) = state.market_workspace.access();
    if grant.is_none() {
        return Err(ApiError::conflict("leased Client required"));
    }
    state
        .set_workspace_host_maintenance(v.terminal_enabled, v.upgrade_policy)
        .await
        .map_err(ApiError::internal)?;
    let cfg = state.config_snapshot().await;
    Ok(Json(
        json!({"terminalEnabled":cfg.enable_web_terminal,"terminalAvailable":cfg.is_web_terminal_enabled(),"upgradePolicy":cfg.upgrade_policy}),
    ))
}

pub(super) async fn token(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let grant = tenant(&state, &headers).await?;
    Ok(Json(
        json!({"token":state.market_workspace.rotate_token(&grant.lease_id).map_err(ApiError::internal)?}),
    ))
}
async fn tenant(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<crate::market_lease::Grant, ApiError> {
    let (_, grant, live) = state.market_workspace.access();
    grant
        .filter(|g| {
            live && session::router_web_user_email(headers).as_deref() == Some(&g.tenant_email)
                && headers
                    .get("x-cc-switch-client-tunnel-subdomain")
                    .and_then(|v| v.to_str().ok())
                    == Some(&g.alias)
        })
        .ok_or_else(|| ApiError::forbidden("current tenant workspace required"))
}
pub(super) async fn backups(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    tenant(&state, &headers).await?;
    let backups = state
        .list_backups_command()
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(
        json!({"backups":backups.into_iter().filter(|b|b.files.iter().all(|f|crate::market_lease::portable_name(&f.file_name))).map(|b|b.id).collect::<Vec<_>>(),
        "restorable":!state.market_workspace.view().managed_activity}),
    ))
}
pub(super) async fn export(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    use std::io::{Read, Write};
    let grant = tenant(&state, &headers).await?;
    let manifest = state
        .create_consistent_backup(Some("Tenant workspace export".into()))
        .await
        .map_err(ApiError::internal)?;
    let dir = state.config_dir.join("backups").join(&manifest.id);
    let bytes = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o600);
        archive.start_file(format!("{}/manifest.json", manifest.id), options)?;
        archive.write_all(&serde_json::to_vec(&manifest)?)?;
        let mut remaining = 128_u64 * 1024 * 1024;
        for item in &manifest.files {
            anyhow::ensure!(
                crate::market_lease::portable_name(&item.file_name),
                "invalid workspace backup entry"
            );
            remaining = remaining
                .checked_sub(item.size_bytes)
                .ok_or_else(|| anyhow::anyhow!("workspace export exceeds 128 MiB"))?;
            let file = std::fs::File::open(dir.join(&item.file_name))?;
            archive.start_file(format!("{}/{}", manifest.id, item.file_name), options)?;
            anyhow::ensure!(
                std::io::copy(&mut file.take(item.size_bytes), &mut archive)? == item.size_bytes,
                "workspace backup changed"
            );
        }
        Ok(archive.finish()?.into_inner())
    })
    .await
    .map_err(ApiError::internal)?
    .map_err(ApiError::bad_request)?;
    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/zip".to_owned(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=workspace-{}.zip", grant.lease_id),
            ),
            (axum::http::header::CACHE_CONTROL, "no-store".into()),
        ],
        bytes,
    )
        .into_response())
}
pub(super) async fn restore(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    tenant(&state, &headers).await?;
    let _chat = state.market_workspace.chat.lock().await;
    state
        .restore_backup_command(id)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(Json(json!({"ok":true})))
}
