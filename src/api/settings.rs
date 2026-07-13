use super::*;

use serde::{Deserialize, Serialize};

use crate::client_tunnel_provision::{
    check_router_reachable, check_subdomain_for_router, suggest_client_tunnel_subdomain,
    RouterReachabilityOutcome, SuggestSubdomainOutcome,
};
use crate::domain::settings::config::{ServerConfig, SetupInput, SetupOptions};
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SetupSubdomainCheckRequest {
    pub(in crate::api) router_url: String,
    pub(in crate::api) subdomain: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SetupSubdomainCheckResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::api) reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SetupRouterCheckRequest {
    pub(in crate::api) router_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct SetupSuggestSubdomainRequest {
    pub(in crate::api) router_url: String,
}

pub(in crate::api) async fn setup_check_router(
    State(state): State<ServerState>,
    Json(input): Json<SetupRouterCheckRequest>,
) -> Result<Json<RouterReachabilityOutcome>, ApiError> {
    if state.config.read().await.is_setup_complete() {
        return Err(ApiError::conflict_code(
            "setup_already_complete",
            "server setup is already complete",
        ));
    }
    let router_url =
        ServerConfig::preview_router_url(&input.router_url).map_err(ApiError::bad_request)?;
    let outcome = check_router_reachable(&state, &router_url).await?;
    Ok(Json(outcome))
}

pub(in crate::api) async fn setup_suggest_subdomain(
    State(state): State<ServerState>,
    Json(input): Json<SetupSuggestSubdomainRequest>,
) -> Result<Json<SuggestSubdomainOutcome>, ApiError> {
    if state.config.read().await.is_setup_complete() {
        return Err(ApiError::conflict_code(
            "setup_already_complete",
            "server setup is already complete",
        ));
    }
    let router_url =
        ServerConfig::preview_router_url(&input.router_url).map_err(ApiError::bad_request)?;
    let outcome = suggest_client_tunnel_subdomain(&state, &router_url, None).await?;
    Ok(Json(outcome))
}

pub(in crate::api) async fn setup_check_subdomain(
    State(state): State<ServerState>,
    Json(input): Json<SetupSubdomainCheckRequest>,
) -> Result<Json<SetupSubdomainCheckResponse>, ApiError> {
    if state.config.read().await.is_setup_complete() {
        return Err(ApiError::conflict_code(
            "setup_already_complete",
            "server setup is already complete",
        ));
    }
    let subdomain =
        ServerConfig::preview_client_subdomain(&input.subdomain).map_err(ApiError::bad_request)?;
    let router_url =
        ServerConfig::preview_router_url(&input.router_url).map_err(ApiError::bad_request)?;
    let availability = check_subdomain_for_router(&state, &router_url, &subdomain, None).await?;
    Ok(Json(SetupSubdomainCheckResponse {
        ok: true,
        available: availability.available,
        reason: availability.reason,
    }))
}

pub(in crate::api) async fn setup_status(
    State(state): State<ServerState>,
) -> Json<SetupStatusResponse> {
    let config = state.config.read().await;
    Json(SetupStatusResponse::from_config(&config))
}

async fn run_setup(
    state: &ServerState,
    input: SetupInput,
    exec: crate::setup::SetupExecution,
) -> Result<Json<SetupResponse>, ApiError> {
    let outcome = crate::setup::complete_setup(state, input, exec).await?;
    Ok(Json(SetupResponse::from_outcome(outcome)))
}

pub(in crate::api) async fn setup_validate(
    State(state): State<ServerState>,
    Json(mut input): Json<SetupInput>,
) -> Result<Json<SetupResponse>, ApiError> {
    if state.config.read().await.is_setup_complete() {
        return Err(ApiError::conflict_code(
            "setup_already_complete",
            "server setup is already complete",
        ));
    }
    input.options = Some(SetupOptions {
        dry_run: true,
        ..input.options.unwrap_or_default()
    });
    run_setup(
        &state,
        input,
        crate::setup::SetupExecution {
            start_client_tunnel: false,
            skip_router_claim: true,
            ..Default::default()
        },
    )
    .await
}

pub(in crate::api) async fn setup_bootstrap(
    State(state): State<ServerState>,
    Json(input): Json<SetupInput>,
) -> Result<Json<SetupResponse>, ApiError> {
    run_setup(
        &state,
        input,
        crate::setup::SetupExecution {
            issue_session_token: true,
            ..Default::default()
        },
    )
    .await
}

pub(in crate::api) async fn setup(
    State(state): State<ServerState>,
    Json(input): Json<SetupInput>,
) -> Result<Json<SetupResponse>, ApiError> {
    run_setup(&state, input, crate::setup::SetupExecution::default()).await
}

pub(in crate::api) async fn login(
    State(state): State<ServerState>,
    Json(input): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    let config = state.config.read().await;
    if !config.is_setup_complete() {
        return Err(ApiError::forbidden("setup is required before login"));
    }
    match input.method.as_str() {
        "password" => {
            if !config.verify_password(input.password.trim()) {
                return Err(ApiError::unauthorized("invalid password"));
            }
        }
        "api_token" => {
            let api_token = input
                .api_token
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("api token is required"))?;
            if !config.verify_api_token(api_token) {
                return Err(ApiError::unauthorized("invalid api token"));
            }
        }
        "email" => {
            let email = input
                .email
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("email is required"))?;
            let code = input
                .code
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("email verification code is required"))?;
            drop(config);
            return complete_email_login(&state, email, code).await;
        }
        _ => return Err(ApiError::bad_request("unsupported auth method")),
    }
    drop(config);

    Ok(Json(issue_login_response(&state).await))
}

pub(in crate::api) async fn request_email_login_code(
    State(state): State<ServerState>,
    Json(input): Json<EmailLoginCodeRequest>,
) -> Result<Json<crate::clients::router::email_auth::EmailCodeRequestResponse>, ApiError> {
    let config = ensure_email_router_config(&state).await?;
    let email = require_configured_owner_email(&config, &input.email)?;
    let http_client = state.http_client().await;
    let response = crate::clients::router::email_auth::request_code(&http_client, &config, &email)
        .await
        .map_err(map_email_auth_error)?;
    Ok(Json(response))
}

pub(in crate::api) async fn verify_email_login_code(
    State(state): State<ServerState>,
    Json(input): Json<EmailLoginVerifyCodeRequest>,
) -> Result<Json<LoginResponse>, ApiError> {
    complete_email_login(&state, &input.email, &input.code).await
}

pub(in crate::api) async fn web_verify_email_login_code(
    State(state): State<ServerState>,
    Json(input): Json<EmailLoginVerifyCodeRequest>,
) -> Result<Json<crate::clients::router::email_auth::RouterVerifyEmailCodeResponse>, ApiError> {
    complete_client_web_email_login(&state, &input.email, &input.code).await
}

pub(in crate::api) async fn complete_client_web_email_login(
    state: &ServerState,
    email: &str,
    code: &str,
) -> Result<Json<crate::clients::router::email_auth::RouterVerifyEmailCodeResponse>, ApiError> {
    let config = ensure_email_router_config(state).await?;
    let email = require_configured_owner_email(&config, email)?;
    let http_client = state.http_client().await;
    let router_session = crate::clients::router::email_auth::verify_client_web_code(
        &http_client,
        &config,
        &email,
        code,
    )
    .await
    .map_err(map_email_auth_error)?;
    let verified_email =
        crate::clients::router::email_auth::normalize_email(&router_session.user.email)
            .map_err(map_email_auth_error)?;
    if verified_email != email {
        return Err(ApiError::unauthorized(
            "verified email does not match configured owner email",
        ));
    }
    let owner_binding = crate::clients::router::email_auth::bind_owner_email(
        &http_client,
        &config,
        &email,
        &router_session.access_token,
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            crate::clients::router::email_auth::humanize_remote_owner_binding_error(&error.message),
        )
    })?;
    let bound_email =
        crate::clients::router::email_auth::normalize_email(&owner_binding.owner_email)
            .map_err(map_email_auth_error)?;
    if !owner_binding.ok || bound_email != email {
        return Err(ApiError::bad_gateway(
            "router accepted email code but did not bind the configured owner email",
        ));
    }
    let email_state =
        crate::clients::router::email_auth::state_from_router_session(&config, &router_session)
            .map_err(map_email_auth_error)?;
    crate::clients::router::email_auth::save_state(&state.config_dir, &email_state)
        .map_err(ApiError::internal)?;

    Ok(Json(router_session))
}

pub(in crate::api) async fn web_session_refresh(
    State(state): State<ServerState>,
    Json(input): Json<WebSessionRefreshRequest>,
) -> Result<Json<crate::clients::router::email_auth::RouterVerifyEmailCodeResponse>, ApiError> {
    let config = ensure_email_router_config(&state).await?;
    let http_client = state.http_client().await;
    let response = crate::clients::router::email_auth::refresh_session(
        &http_client,
        &config,
        &input.refresh_token,
    )
    .await
    .map_err(map_email_auth_error)?;
    Ok(Json(response))
}

pub(in crate::api) async fn web_auth_methods(
    State(state): State<ServerState>,
) -> Result<Json<crate::domain::web_auth::AuthMethods>, ApiError> {
    let config = state.config.read().await;
    Ok(Json(crate::domain::web_auth::auth_methods(&config)))
}

pub(in crate::api) async fn web_password_login(
    State(state): State<ServerState>,
    Json(input): Json<WebPasswordRequest>,
) -> Result<Json<crate::domain::web_auth::PasswordLoginResponse>, ApiError> {
    let config = state.config.read().await.clone();
    state
        .web_auth
        .login(&config, &input.password)
        .map(Json)
        .map_err(map_web_auth_error)
}

pub(in crate::api) async fn web_password_setup(
    State(state): State<ServerState>,
    Json(input): Json<WebPasswordRequest>,
) -> Result<Json<crate::domain::web_auth::PasswordLoginResponse>, ApiError> {
    let mut config = state.config.read().await.clone();
    let response = state
        .web_auth
        .setup_password(&mut config, &input.password)
        .map_err(map_web_auth_error)?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(response))
}

pub(in crate::api) async fn web_password_refresh(
    State(state): State<ServerState>,
    Json(input): Json<WebSessionRefreshRequest>,
) -> Result<Json<crate::domain::web_auth::PasswordLoginResponse>, ApiError> {
    state
        .web_auth
        .refresh(&input.refresh_token)
        .map(Json)
        .map_err(map_web_auth_error)
}

pub(in crate::api) async fn web_password_logout(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let token = bearer_token(&headers)
        .ok_or_else(|| ApiError::unauthorized("authorization bearer token is required"))?;
    state.web_auth.logout(token).map_err(map_web_auth_error)?;
    Ok(Json(json!({ "ok": true })))
}

pub(in crate::api) async fn web_password_change(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<WebPasswordChangeRequest>,
) -> Result<Json<Value>, ApiError> {
    require_web_admin_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    state
        .web_auth
        .change_password(&mut config, &input.current_password, &input.new_password)
        .map_err(map_web_auth_error)?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    state.clear_sessions().await;
    Ok(Json(json!({ "ok": true })))
}

pub(in crate::api) async fn complete_email_login(
    state: &ServerState,
    email: &str,
    code: &str,
) -> Result<Json<LoginResponse>, ApiError> {
    let config = ensure_email_router_config(state).await?;
    let email = require_configured_owner_email(&config, email)?;
    let http_client = state.http_client().await;
    let router_session = crate::clients::router::email_auth::verify_client_web_code(
        &http_client,
        &config,
        &email,
        code,
    )
    .await
    .map_err(map_email_auth_error)?;
    let verified_email =
        crate::clients::router::email_auth::normalize_email(&router_session.user.email)
            .map_err(map_email_auth_error)?;
    if verified_email != email {
        return Err(ApiError::unauthorized(
            "verified email does not match configured owner email",
        ));
    }
    let owner_binding = crate::clients::router::email_auth::bind_owner_email(
        &http_client,
        &config,
        &email,
        &router_session.access_token,
    )
    .await
    .map_err(|error| {
        ApiError::new(
            StatusCode::from_u16(error.status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY),
            crate::clients::router::email_auth::humanize_remote_owner_binding_error(&error.message),
        )
    })?;
    let bound_email =
        crate::clients::router::email_auth::normalize_email(&owner_binding.owner_email)
            .map_err(map_email_auth_error)?;
    if !owner_binding.ok || bound_email != email {
        return Err(ApiError::bad_gateway(
            "router accepted email code but did not bind the configured owner email",
        ));
    }
    let email_state =
        crate::clients::router::email_auth::state_from_router_session(&config, &router_session)
            .map_err(map_email_auth_error)?;
    crate::clients::router::email_auth::save_state(&state.config_dir, &email_state)
        .map_err(ApiError::internal)?;

    Ok(Json(issue_login_response(state).await))
}

pub(in crate::api) async fn ensure_email_router_config(
    state: &ServerState,
) -> Result<ServerConfig, ApiError> {
    let mut config = state.config.read().await.clone();
    if !config.is_setup_complete() {
        return Err(ApiError::forbidden("setup is required before email login"));
    }
    let has_identity = config.router.identity.as_ref().is_some_and(|identity| {
        !identity.installation_id.trim().is_empty() && !identity.private_key.trim().is_empty()
    });
    if has_identity {
        return Ok(config);
    }

    let http_client = state.http_client().await;
    match crate::clients::router::client::register_installation(&http_client, &mut config).await {
        Ok(_) => {
            state
                .replace_config(config.clone())
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
            Ok(config)
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

pub(in crate::api) fn require_configured_owner_email(
    config: &ServerConfig,
    email: &str,
) -> Result<String, ApiError> {
    let email =
        crate::clients::router::email_auth::normalize_email(email).map_err(map_email_auth_error)?;
    let owner_email = config
        .owner
        .email
        .as_deref()
        .ok_or_else(|| ApiError::forbidden("owner email is not configured"))?
        .trim()
        .to_ascii_lowercase();
    if owner_email != email {
        return Err(ApiError::unauthorized(
            "email does not match configured owner email",
        ));
    }
    Ok(email)
}

pub(crate) async fn issue_login_response(state: &ServerState) -> LoginResponse {
    let token = generate_session_token();
    state
        .push_session(Session {
            token: token.clone(),
        })
        .await;
    LoginResponse {
        ok: true,
        token,
        token_type: "bearer",
    }
}

pub(in crate::api) async fn change_password(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<Json<ChangePasswordResponse>, ApiError> {
    require_session(&state, &headers).await?;
    set_admin_password(&state, &input.new_password).await?;
    Ok(Json(ChangePasswordResponse { ok: true }))
}

pub(in crate::api) async fn web_password_set(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ChangePasswordRequest>,
) -> Result<Json<ChangePasswordResponse>, ApiError> {
    require_web_admin_session(&state, &headers).await?;
    set_admin_password(&state, &input.new_password).await?;
    Ok(Json(ChangePasswordResponse { ok: true }))
}

async fn set_admin_password(state: &ServerState, new_password: &str) -> Result<(), ApiError> {
    let mut config = state.config.read().await.clone();
    config
        .set_password(new_password)
        .map_err(|error| ApiError::bad_request(error.to_string()))?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    state.clear_sessions().await;
    state
        .web_auth
        .revoke_all_sessions()
        .map_err(ApiError::internal)?;
    Ok(())
}

pub(in crate::api) async fn rotate_api_token(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ApiTokenResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let api_token = generate_session_token();
    let mut config = state.config.read().await.clone();
    config
        .set_api_token(&api_token)
        .map_err(ApiError::internal)?;
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ApiTokenResponse {
        ok: true,
        api_token,
    }))
}

pub(in crate::api) async fn auth_me(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AuthMeResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(AuthMeResponse {
        ok: true,
        owner_email: config.owner.email.clone(),
    }))
}

pub(in crate::api) async fn config_snapshot(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ConfigSnapshotResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(ConfigSnapshotResponse::from_config(&config)))
}

pub(in crate::api) async fn upstream_proxy(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<UpstreamProxyResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let config = state.config.read().await;
    Ok(Json(UpstreamProxyResponse {
        ok: true,
        upstream_proxy: UpstreamProxyView::from_config(&config),
    }))
}

pub(in crate::api) async fn update_upstream_proxy(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpdateUpstreamProxyInput>,
) -> Result<Json<UpstreamProxyResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut config = state.config.read().await.clone();
    config
        .update_upstream_proxy(input)
        .map_err(ApiError::bad_request)?;
    let upstream_proxy = UpstreamProxyView::from_config(&config);
    state
        .replace_config(config)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(UpstreamProxyResponse {
        ok: true,
        upstream_proxy,
    }))
}
