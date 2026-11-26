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
            owner_email: Option<String>,
            lease_id: Option<String>,
            buyer_id: Option<String>,
            tenant_email: Option<String>,
            restart_after: Option<bool>,
            task_id: Option<String>,
            target_commit: Option<String>,
        }
        let v: Upgrade = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
        let _command = state.market_workspace.commands.lock().await;
        let (_, grant, live) = state.market_workspace.access();
        let leased = v.lease_id.is_some();
        if let Some(lease) = &v.lease_id {
            let grant = grant
                .as_ref()
                .filter(|g| {
                    live && g.lease_id == *lease
                        && Some(&g.tenant_email) == v.tenant_email.as_ref()
                        && v.buyer_id.is_some()
                        && g.buyer_id
                            .as_ref()
                            .is_none_or(|id| Some(id) == v.buyer_id.as_ref())
                })
                .ok_or_else(|| ApiError::forbidden("current tenant required"))?;
            if v.task_id.as_ref().is_some_and(|id| {
                !id.strip_prefix(&format!("lease-{}-", grant.lease_id))
                    .is_some_and(|suffix| {
                        suffix.len() == 32 && suffix.bytes().all(|c| c.is_ascii_hexdigit())
                    })
            }) {
                return Err(ApiError::forbidden("task belongs to another workspace"));
            }
        } else if state.market_workspace.management_locked()
            || !v
                .owner_email
                .as_deref()
                .is_some_and(|email| owner.eq_ignore_ascii_case(email))
        {
            return Err(ApiError::forbidden("installation owner unavailable"));
        }
        if path == "/_ctl/market/upgrade-status" {
            if let Some(task) = v.task_id.as_deref().filter(|_| leased) {
                if let Some(snapshot) = state
                    .upgrade
                    .archived(task)
                    .map_err(|e| ApiError::internal(e.to_string()))?
                {
                    return Ok(Json(json!(snapshot)));
                }
            }
            let snapshot =
                super::self_update::upgrade_status_for_task(&state, v.task_id.as_deref())
                    .await?
                    .0;
            if let Some(lease) = &v.lease_id {
                if !snapshot.task_id.starts_with(&format!("lease-{lease}-")) {
                    return Err(ApiError::not_found("no upgrade for this lease"));
                }
            } else if snapshot.task_id.starts_with("lease-") {
                return Err(ApiError::forbidden("rental task"));
            }
            return Ok(Json(
                serde_json::to_value(snapshot).map_err(ApiError::internal)?,
            ));
        }
        if leased && v.task_id.is_none() {
            return Err(ApiError::bad_request("rental task ID required"));
        }
        if !leased && !config.upgrade_policy.delegate_upgrade_to_router_owner {
            return Err(ApiError::forbidden("owner disabled delegated upgrades"));
        }
        if let Some(task) = v.task_id.as_deref().filter(|_| leased) {
            if state
                .upgrade
                .archived(task)
                .map_err(|e| ApiError::internal(e.to_string()))?
                .is_some()
                || state
                    .upgrade
                    .current()
                    .await
                    .is_some_and(|handle| handle.task_id == task)
            {
                return Ok(Json(json!({"ok":true,"taskId":task})));
            }
        }
        let id = super::self_update::start_authorized_upgrade(
            &state,
            leased || v.restart_after.unwrap_or(true),
            if leased { "tenant" } else { "router-owner" },
            true,
            v.task_id,
            v.target_commit,
        )
        .await?;
        if let Some(lease) = v.lease_id {
            state
                .market_workspace
                .record_event(&lease, "tenant_upgrade")
                .map_err(ApiError::internal)?;
        }
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
            let beginning = matches!(command.action, Operation::Begin { .. });
            if matches!(
                command.action,
                Operation::List { .. } | Operation::Begin { .. } | Operation::Clean { .. }
            ) && state.client_update_in_progress().await
            {
                return Err(ApiError::conflict("Client update in progress"));
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
            if beginning && !replay {
                state.terminal.end_session().await;
                // The next boot revokes persistent credentials before serving requests.
                if let Err(err) = state.web_auth.revoke_all_sessions() {
                    tracing::warn!(%err,"defer session revocation to lease startup");
                }
                state.clear_sessions().await;
            }
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
                released_lease_id: Option<String>,
            }
            let ack: Ack = serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
            if let Some(id) = ack.released_lease_id {
                let _command = state.market_workspace.commands.lock().await;
                state
                    .market_workspace
                    .acknowledge_released(&id)
                    .map_err(|e| ApiError::conflict(e.to_string()))?;
            }
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
        json!({"capability":"marketLeaseV1","exclusiveManagement":true,"upgradeBusy":state.client_update_in_progress().await,"awaitingReturn":journal.awaiting_return,"ownerRental":true,"ownerEmail":owner,"listedOwner":journal.owner,
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
            | "/web-api/market/upgrade-policy"
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
    request: Request,
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
        if state.market_workspace.management_locked() {
            return denied();
        }
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
    if in_family(path, "/web-api/terminal") {
        return denied();
    }
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
    denied()
}

fn workspace_error(status: StatusCode, code: &'static str) -> ApiError {
    let mut error = ApiError::new(status, code);
    error.code = Some(code);
    error
}

pub(super) async fn view(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let grant = tenant(&state, &headers).await?;
    let config = state.config_snapshot().await;
    let upgrade = state
        .upgrade
        .status_snapshot()
        .await
        .filter(|u| u.task_id.starts_with(&format!("lease-{}-", grant.lease_id)))
        .map(|u| json!({"taskId":u.task_id,"status":u.status}));
    Ok(Json(
        json!({"upgrade":upgrade,"role":"tenant","leaseId":grant.lease_id,"expiresAt":grant.expires_at,
        "autoUpgradeEnabled":config.upgrade_policy.auto_upgrade_enabled, "autoUpgradeCheckIntervalMinutes":config.upgrade_policy.auto_upgrade_check_interval_minutes}),
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TenantUpgradePolicy {
    auto_upgrade_enabled: bool,
    auto_upgrade_check_interval_minutes: u64,
}
pub(super) async fn upgrade_policy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<TenantUpgradePolicy>,
) -> Result<Json<Value>, ApiError> {
    let _command = state.market_workspace.commands.lock().await;
    tenant(&state, &headers).await?;
    let policy = crate::domain::settings::config::UpgradePolicyConfig {
        delegate_upgrade_to_router_owner: false,
        auto_upgrade_enabled: input.auto_upgrade_enabled,
        auto_upgrade_check_interval_minutes: input.auto_upgrade_check_interval_minutes,
    }
    .normalize();
    state
        .set_tenant_upgrade_policy(policy.clone())
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(json!(policy)))
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
pub(super) async fn tenant(
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
