use super::*;

pub(in crate::api) async fn list_accounts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListAccountsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(ListAccountsResponse {
        ok: true,
        accounts: state.accounts.read().await.accounts.clone(),
    }))
}

pub(in crate::api) async fn upsert_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<UpsertAccountInput>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let account = {
        let mut store = state.accounts.write().await;
        let manager = manager_for(input.provider_type);
        manager
            .finish_login(&mut store, input)
            .map_err(ApiError::bad_request)?
    };
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(UpsertAccountResponse { ok: true, account }))
}

pub(in crate::api) async fn account_capabilities() -> Json<AccountCapabilitiesResponse> {
    Json(AccountCapabilitiesResponse {
        ok: true,
        capabilities: crate::domain::accounts::managers::all_capabilities(),
    })
}

pub(in crate::api) async fn account_import_templates(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AccountImportTemplatesResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(AccountImportTemplatesResponse {
        ok: true,
        templates: crate::domain::accounts::managers::account_import_templates(),
    }))
}

pub(in crate::api) async fn start_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartAccountLoginRequest>,
) -> Result<Json<StartAccountLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let redirect_uri = input
        .redirect_uri
        .or_else(|| Some(default_account_login_redirect_uri(&state)));
    let login = {
        let mut store = state.oauth_logins.write().await;
        store
            .start(input.provider_type, redirect_uri, now_ms() as i64)
            .map_err(oauth_login_api_error)?
    };
    Ok(Json(StartAccountLoginResponse { ok: true, login }))
}

pub(in crate::api) async fn account_login_callback(
    State(state): State<ServerState>,
    Query(query): Query<AccountLoginCallbackQuery>,
) -> Result<Json<FinishAccountLoginResponse>, ApiError> {
    let AccountLoginCallbackQuery {
        session_id,
        state: oauth_state,
        code,
        error,
        error_description,
    } = query;
    if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
        let message = error_description
            .filter(|value| !value.trim().is_empty())
            .map(|description| format!("{error}: {description}"))
            .unwrap_or(error);
        return Err(ApiError::bad_request(message));
    }
    let finish = {
        let mut store = state.oauth_logins.write().await;
        store
            .finish(
                session_id.as_deref(),
                oauth_state.as_deref(),
                code.as_deref(),
                false,
                now_ms() as i64,
            )
            .map_err(oauth_login_api_error)?
    };
    Ok(Json(FinishAccountLoginResponse {
        ok: true,
        login: redact_oauth_login_finish(finish),
        account: None,
    }))
}

pub(in crate::api) async fn finish_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<FinishAccountLoginRequest>,
) -> Result<Json<FinishAccountLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let mut finish = {
        let mut store = state.oauth_logins.write().await;
        store
            .finish(
                input.session_id.as_deref(),
                input.state.as_deref(),
                input.code.as_deref(),
                input.execute_token_exchange.unwrap_or(false),
                now_ms() as i64,
            )
            .map_err(oauth_login_api_error)?
    };
    let account = if input.execute_token_exchange.unwrap_or(false) {
        Some(execute_account_login_token_exchange(&state, &mut finish).await?)
    } else {
        None
    };
    Ok(Json(FinishAccountLoginResponse {
        ok: true,
        login: redact_oauth_login_finish(finish),
        account,
    }))
}

pub(in crate::api) async fn start_copilot_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartCopilotDeviceLoginRequest>,
) -> Result<Json<StartCopilotDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let device = crate::clients::oauth::copilot_device::start_device_flow(
        &http_client,
        input.github_domain.as_deref(),
    )
    .await
    .map_err(map_copilot_device_error)?;
    Ok(Json(StartCopilotDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_copilot_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollCopilotDeviceLoginRequest>,
) -> Result<Json<PollCopilotDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let result = crate::clients::oauth::copilot_device::poll_device_flow(
        &http_client,
        &input.device_code,
        input.github_domain.as_deref(),
        now_ms() as i64,
    )
    .await
    .map_err(map_copilot_device_error)?;
    if result.pending {
        return Ok(Json(PollCopilotDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("copilot device flow completed without account"))?;
    let provider_type = account_input.provider_type;
    let account_result = {
        let mut store = state.accounts.write().await;
        manager_for(provider_type).finish_login(&mut store, account_input)
    };
    let account = account_result.map_err(ApiError::bad_request)?;
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(PollCopilotDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn start_kiro_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartKiroDeviceLoginRequest>,
) -> Result<Json<StartKiroDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let now = now_ms() as i64;
    let (device, flow) = crate::clients::oauth::kiro_device::start_device_flow(
        &http_client,
        input.region.as_deref(),
        input.start_url.as_deref(),
        now,
    )
    .await
    .map_err(map_kiro_device_error)?;
    {
        let mut store = state.kiro_device_flows.write().await;
        store.insert(device.device_code.clone(), flow, now);
    }
    Ok(Json(StartKiroDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_kiro_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollKiroDeviceLoginRequest>,
) -> Result<Json<PollKiroDeviceLoginResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let flow = {
        let mut store = state.kiro_device_flows.write().await;
        store
            .get(&input.device_code, now)
            .ok_or_else(|| ApiError::unauthorized("kiro device flow is expired or unknown"))?
    };
    let http_client = state.http_client().await;
    let result = match crate::clients::oauth::kiro_device::poll_device_flow(
        &http_client,
        &input.device_code,
        flow,
        now,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            if matches!(
                error.status,
                StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
            ) {
                state
                    .kiro_device_flows
                    .write()
                    .await
                    .remove(&input.device_code);
            }
            return Err(map_kiro_device_error(error));
        }
    };
    if result.pending {
        return Ok(Json(PollKiroDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    state
        .kiro_device_flows
        .write()
        .await
        .remove(&input.device_code);
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("kiro device flow completed without account"))?;
    let provider_type = account_input.provider_type;
    let account_result = {
        let mut store = state.accounts.write().await;
        manager_for(provider_type).finish_login(&mut store, account_input)
    };
    let account = account_result.map_err(ApiError::bad_request)?;
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(PollKiroDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn execute_account_login_token_exchange(
    state: &ServerState,
    finish: &mut OAuthLoginFinish,
) -> Result<AccountLoginAccountSummary, ApiError> {
    let request = finish
        .token_request
        .as_ref()
        .ok_or_else(|| ApiError::bad_request("token exchange request is unavailable"))?;
    let http_client = state.http_client().await;
    let (token_response, raw) = match execute_oauth_token_request(
        &http_client,
        finish.provider_type,
        request,
        format!("{} OAuth token exchange", finish.provider_type.as_str()),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(account_refresh_api_error(error));
        }
    };
    let profile_raw = match execute_account_login_profile_request(
        state,
        finish.provider_type,
        finish.flow,
        &token_response.access_token,
    )
    .await
    {
        Ok(profile) => profile,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(account_refresh_api_error(error));
        }
    };
    let input = match upsert_input_from_login_response(
        finish.provider_type,
        &token_response,
        raw,
        profile_raw,
        now_ms() as i64,
    ) {
        Ok(input) => input,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error.message));
        }
    };

    let account_result = {
        let mut store = state.accounts.write().await;
        manager_for(input.provider_type).finish_login(&mut store, input)
    };
    let account = match account_result {
        Ok(account) => account,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error));
        }
    };
    if let Err(error) = state.save_accounts().await {
        mark_account_login_exchange_failed(state, &finish.session_id).await;
        return Err(ApiError::internal(error));
    }
    state
        .oauth_logins
        .write()
        .await
        .mark_exchanged(&finish.session_id)
        .map_err(oauth_login_api_error)?;

    finish.status = OAuthLoginStatus::TokenExchanged;
    finish.method = "token_exchange_completed";
    finish.token_request = None;
    finish.account_import_hint = None;
    finish.message = format!(
        "{} OAuth token exchange completed and account was imported",
        finish.provider_type.as_str()
    );

    Ok(AccountLoginAccountSummary::from_account(&account))
}

pub(in crate::api) async fn execute_account_login_profile_request(
    state: &ServerState,
    provider_type: ProviderType,
    flow: OAuthAuthorizeFlow,
    access_token: &str,
) -> Result<Option<serde_json::Value>, AccountRefreshFailure> {
    if flow == OAuthAuthorizeFlow::CursorDeepControl {
        return Ok(None);
    }
    if !matches!(
        provider_type,
        ProviderType::GeminiCli | ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) {
        return Ok(None);
    }
    let Some(request) = build_profile_request(provider_type, access_token) else {
        return Ok(None);
    };
    let http_client = state.http_client().await;
    execute_oauth_json_request(
        &http_client,
        provider_type,
        &request,
        format!("{} OAuth profile fetch", provider_type.as_str()),
    )
    .await
    .map(Some)
}

pub(in crate::api) async fn mark_account_login_exchange_failed(
    state: &ServerState,
    session_id: &str,
) {
    state
        .oauth_logins
        .write()
        .await
        .mark_exchange_failed(session_id);
}

pub(in crate::api) async fn delete_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let deleted = {
        let mut store = state.accounts.write().await;
        let provider_type = store
            .accounts
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.provider_type);
        match provider_type {
            Some(provider_type) => manager_for(provider_type)
                .revoke_or_delete(&mut store, &id)
                .map_err(ApiError::bad_request)?,
            None => false,
        }
    };
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

pub(in crate::api) async fn refresh_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let existing = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("account not found"))?;

    if provider_native_refresh_available(existing.provider_type) {
        let now = now_ms() as i64;
        let _refresh_guard = state
            .account_refresh_locks
            .try_lock(existing.provider_type, &existing.id)
            .ok_or_else(|| ApiError::conflict("account refresh is already in progress"))?;
        let http_client = state.http_client().await;
        let update = match execute_native_account_refresh(&http_client, &existing, now).await {
            Ok(update) => update,
            Err(error) => {
                {
                    let mut store = state.accounts.write().await;
                    store.mark_refresh_failure(&id, error.message.clone());
                }
                state.save_accounts().await.map_err(ApiError::internal)?;
                return Err(account_refresh_api_error(error));
            }
        };
        let account = {
            let mut store = state.accounts.write().await;
            store
                .mark_refresh_success(&id, update)
                .ok_or_else(|| ApiError::not_found("account not found"))?
        };
        state.save_accounts().await.map_err(ApiError::internal)?;
        return Ok(Json(UpsertAccountResponse { ok: true, account }));
    }

    let account = {
        let mut store = state.accounts.write().await;
        manager_for(existing.provider_type)
            .refresh_token(&mut store, &id, now_ms() as i64)
            .map_err(ApiError::bad_request)?
    };
    state.save_accounts().await.map_err(ApiError::internal)?;
    Ok(Json(UpsertAccountResponse { ok: true, account }))
}

pub(in crate::api) fn account_refresh_api_error(error: AccountRefreshFailure) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
        error.message,
    )
}

pub(in crate::api) async fn account_refresh_plan(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<AccountRefreshPlanResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let account = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    let spec = oauth_provider_spec(account.provider_type);
    let refresh_request = build_refresh_request(account.provider_type, &account)
        .ok()
        .map(redact_oauth_request);
    let profile_request = account
        .access_token
        .as_deref()
        .and_then(|token| build_profile_request(account.provider_type, token))
        .map(redact_oauth_request);
    let refresh_required = token_expires_soon(&account, now_ms() as i64);
    let message = if spec.is_some_and(|item| item.server_native_refresh_enabled())
        && refresh_request.is_some()
    {
        "native refresh/profile execution is available after importing refresh credentials"
            .to_string()
    } else if refresh_request.is_some() {
        "refresh request shape is available; native refresh execution remains disabled".to_string()
    } else if spec.is_some_and(|item| item.token_urls.is_empty()) {
        "provider has no OAuth refresh endpoint; manual import/API key mode only".to_string()
    } else {
        "refresh request shape is unavailable; account likely lacks a refresh token or provider credentials".to_string()
    };

    Ok(Json(AccountRefreshPlanResponse {
        ok: true,
        account_id: account.id,
        provider_type: account.provider_type,
        refresh_required,
        server_native_stage: spec.map(|item| item.stage),
        quota_strategy: spec.map(|item| item.quota_strategy),
        refresh_request,
        profile_request,
        message,
    }))
}

pub(in crate::api) async fn account_quota(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<AccountQuotaQuery>,
) -> Result<Json<AccountQuotaResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let existing = state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .find(|item| item.id == id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    if !query.refresh.unwrap_or(false) {
        let store = state.accounts.read().await;
        let quota = manager_for(existing.provider_type)
            .query_quota(&store, &id)
            .map_err(ApiError::bad_request)?;
        let next_refresh_at = existing.quota_next_refresh_at;
        return Ok(Json(AccountQuotaResponse {
            ok: true,
            quota,
            account: Some(existing),
            refreshed: false,
            message: Some(
                "quota snapshot returned; use refresh=true to query upstream".to_string(),
            ),
            next_refresh_at,
        }));
    }

    let now = now_ms() as i64;
    let force = query.force.unwrap_or(false);
    if !force {
        if let Some(next_refresh_at) = existing.quota_next_refresh_at {
            if next_refresh_at > now {
                return Ok(Json(AccountQuotaResponse {
                    ok: true,
                    quota: existing.quota.clone(),
                    account: Some(existing),
                    refreshed: false,
                    message: Some(format!("quota refresh skipped until {next_refresh_at}")),
                    next_refresh_at: Some(next_refresh_at),
                }));
            }
        }
    }

    let mut active_account = existing;
    let mut account_mutated = false;
    if account_needs_native_refresh(&active_account, now) {
        let _refresh_guard = state
            .account_refresh_locks
            .try_lock(active_account.provider_type, &active_account.id)
            .ok_or_else(|| ApiError::conflict("account refresh is already in progress"))?;
        let http_client = state.http_client().await;
        let update = match execute_native_account_refresh(&http_client, &active_account, now).await
        {
            Ok(update) => update,
            Err(error) => {
                {
                    let mut store = state.accounts.write().await;
                    store.mark_refresh_failure(&id, error.message.clone());
                }
                state.save_accounts().await.map_err(ApiError::internal)?;
                return Err(account_refresh_api_error(error));
            }
        };
        active_account = {
            let mut store = state.accounts.write().await;
            store
                .mark_refresh_success(&id, update)
                .ok_or_else(|| ApiError::not_found("account not found"))?
        };
        account_mutated = true;
    }

    let http_client = state.http_client().await;
    match refresh_account_quota(&http_client, &active_account, now, force).await {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let account = {
                let mut store = state.accounts.write().await;
                store
                    .mark_refresh_success(&id, update)
                    .ok_or_else(|| ApiError::not_found("account not found"))?
            };
            state.save_accounts().await.map_err(ApiError::internal)?;
            Ok(Json(AccountQuotaResponse {
                ok: true,
                quota: account.quota.clone(),
                account: Some(account.clone()),
                refreshed: true,
                message: Some(message),
                next_refresh_at: account.quota_next_refresh_at,
            }))
        }
        Ok(QuotaRefreshResult::SkippedCooldown {
            next_refresh_at,
            message,
        }) => {
            if account_mutated {
                state.save_accounts().await.map_err(ApiError::internal)?;
            }
            Ok(Json(AccountQuotaResponse {
                ok: true,
                quota: active_account.quota.clone(),
                account: Some(active_account),
                refreshed: false,
                message: Some(message),
                next_refresh_at: Some(next_refresh_at),
            }))
        }
        Err(error) => {
            {
                let mut store = state.accounts.write().await;
                store.mark_refresh_success(
                    &id,
                    AccountRefreshUpdate {
                        quota_next_refresh_at: error.next_refresh_at,
                        last_refresh_error: Some(error.message.clone()),
                        ..Default::default()
                    },
                );
            }
            state.save_accounts().await.map_err(ApiError::internal)?;
            Err(ApiError::new(
                StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
                error.message,
            ))
        }
    }
}

pub(in crate::api) fn redact_oauth_request(mut request: OAuthHttpRequest) -> OAuthHttpRequest {
    for (name, value) in &mut request.headers {
        if name.eq_ignore_ascii_case("authorization") {
            *value = "[REDACTED]".to_string();
        }
    }
    request.url = redact_oauth_url(&request.url);
    redact_oauth_json(&mut request.body);
    request
}

pub(in crate::api) fn redact_oauth_url(url: &str) -> String {
    let Some((base, query)) = url.split_once('?') else {
        return url.to_string();
    };
    let redacted_query = query
        .split('&')
        .map(|part| {
            let Some((key, _value)) = part.split_once('=') else {
                return part.to_string();
            };
            if is_oauth_secret_key(key) {
                format!("{key}=[REDACTED]")
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{base}?{redacted_query}")
}

pub(in crate::api) fn redact_oauth_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                if is_oauth_secret_key(key) {
                    *item = serde_json::Value::String("[REDACTED]".to_string());
                } else {
                    redact_oauth_json(item);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_oauth_json(item);
            }
        }
        _ => {}
    }
}

pub(in crate::api) fn is_oauth_secret_key(key: &str) -> bool {
    let key_lower = key.to_ascii_lowercase();
    key_lower.contains("token")
        || key_lower.contains("secret")
        || key_lower.contains("api_key")
        || key_lower == "password"
        || key_lower == "code"
        || key_lower == "code_verifier"
        || key_lower == "verifier"
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::accounts::oauth::{OAuthHttpRequest, OAuthRequestBodyFormat};

    #[test]
    fn oauth_request_redaction_removes_authorization_codes_and_verifiers() {
        let request = OAuthHttpRequest {
            method: "POST",
            url: "https://api2.cursor.sh/auth/poll?uuid=session&verifier=secret-verifier"
                .to_string(),
            headers: vec![(
                "Authorization".to_string(),
                "Bearer access-token".to_string(),
            )],
            body: json!({
                "code": "auth-code",
                "code_verifier": "secret-code-verifier",
                "client_secret": "secret-client",
                "nested": {"refresh_token": "refresh-token"}
            }),
            body_format: OAuthRequestBodyFormat::Json,
        };

        let redacted = redact_oauth_request(request);
        let serialized = serde_json::to_string(&redacted).unwrap();

        assert!(!serialized.contains("auth-code"));
        assert!(!serialized.contains("secret-code-verifier"));
        assert!(!serialized.contains("secret-client"));
        assert!(!serialized.contains("refresh-token"));
        assert!(!serialized.contains("secret-verifier"));
        assert!(serialized.contains("[REDACTED]"));
    }
}
