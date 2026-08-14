use super::*;
use axum::response::Html;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(in crate::api) async fn list_accounts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListAccountsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let snapshot = state.accounts_snapshot().await;
    let accounts = snapshot
        .accounts
        .iter()
        .map(AccountPublicView::from)
        .collect();
    Ok(Json(ListAccountsResponse {
        ok: true,
        accounts,
        codex_oauth: snapshot.codex_oauth_selection(),
    }))
}

pub(in crate::api) async fn select_active_codex_oauth_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SelectActiveCodexOAuthAccountRequest>,
) -> Result<Json<SelectActiveCodexOAuthAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let account = state
        .select_active_codex_oauth_account_command(&input.account_id)
        .await
        .map_err(ApiError::internal)?
        .map_err(map_codex_active_account_selection_error)?;
    let codex_oauth = state.accounts_snapshot().await.codex_oauth_selection();
    Ok(Json(SelectActiveCodexOAuthAccountResponse {
        ok: true,
        account: account.into(),
        codex_oauth,
    }))
}

pub(in crate::api) async fn upsert_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<UpsertAccountInput>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    verify_and_mark_managed_account_input(&state, &mut input, false).await?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            let manager = manager_for(input.provider_type);
            manager
                .finish_login(store, input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    drop(managed_auth_operation);
    state.schedule_gemini_v1internal_project_enrichment(account.provider_type, &account.id);
    Ok(Json(UpsertAccountResponse {
        ok: true,
        account: account.into(),
    }))
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

pub(in crate::api) async fn import_claude_credentials(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportClaudeCredentialsRequest>,
) -> Result<Json<ImportClaudeCredentialsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let upsert = upsert_input_from_claude_credentials(input.credentials)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::ClaudeOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    drop(managed_auth_operation);
    Ok(Json(ImportClaudeCredentialsResponse {
        ok: true,
        account: AccountLoginAccountSummary::from_account(&account),
    }))
}

pub(in crate::api) async fn import_grok_auth_json(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportGrokAuthJsonRequest>,
) -> Result<Json<ImportGrokAuthJsonResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let upsert = upsert_input_from_grok_auth_json(&state, input.auth_json).await?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::GrokOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    drop(managed_auth_operation);
    Ok(Json(ImportGrokAuthJsonResponse {
        ok: true,
        account: AccountLoginAccountSummary::from_account(&account),
    }))
}

pub(in crate::api) async fn import_kiro_credentials_json(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportKiroCredentialsRequest>,
) -> Result<Json<ImportKiroCredentialsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let upsert =
        crate::clients::oauth::kiro::import_credentials_json(input.credentials, now_ms() as i64)
            .map_err(account_refresh_api_error)?;
    import_kiro_upsert(state, upsert, Some("json".to_string())).await
}

pub(in crate::api) async fn import_kiro_local_credentials(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportKiroLocalCredentialsRequest>,
) -> Result<Json<ImportKiroCredentialsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let path = resolve_kiro_credentials_path(input.path)
        .ok_or_else(|| ApiError::bad_request("Kiro credentials path is not available"))?;
    let content = std::fs::read_to_string(&path).map_err(|error| {
        ApiError::bad_request(format!("read {} failed: {error}", path.display()))
    })?;
    let credentials: Value = serde_json::from_str(&content).map_err(|error| {
        ApiError::bad_request(format!("parse {} as JSON failed: {error}", path.display()))
    })?;
    let upsert = crate::clients::oauth::kiro::import_credentials_json(credentials, now_ms() as i64)
        .map_err(account_refresh_api_error)?;
    import_kiro_upsert(state, upsert, Some(path.display().to_string())).await
}

pub(in crate::api) async fn import_kiro_api_key(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportKiroApiKeyRequest>,
) -> Result<Json<ImportKiroCredentialsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let upsert = crate::clients::oauth::kiro::import_validated_api_key(
        &http_client,
        &input.api_key,
        input.region.as_deref(),
        now_ms() as i64,
    )
    .await
    .map_err(account_refresh_api_error)?;
    import_kiro_upsert(state, upsert, Some("api_key".to_string())).await
}

async fn import_kiro_upsert(
    state: ServerState,
    upsert: UpsertAccountInput,
    source: Option<String>,
) -> Result<Json<ImportKiroCredentialsResponse>, ApiError> {
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::KiroOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    drop(managed_auth_operation);
    Ok(Json(ImportKiroCredentialsResponse {
        ok: true,
        account: AccountLoginAccountSummary::from_account(&account),
        source,
    }))
}

fn resolve_kiro_credentials_path(input: Option<String>) -> Option<PathBuf> {
    input
        .or_else(|| std::env::var("KIRO_CREDENTIALS_PATH").ok())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|home| PathBuf::from(home).join(".kiro").join("credentials.json"))
        })
}

pub(in crate::api) async fn import_cursor_local_auth(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ImportCursorLocalAuthResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let import =
        import_from_local_cursor().map_err(|error| ApiError::bad_request(error.message))?;
    let source = import.source.as_str().to_string();
    let path = import.path.as_ref().map(|path| path.display().to_string());
    let profile_result = execute_cursor_profile_request(
        &state,
        &import.access_token,
        import.workos_user_id.as_deref(),
    )
    .await;
    let (profile_raw, profile_error) = match profile_result {
        Ok(profile) => (profile, None),
        Err(error) => {
            let diagnostic = crate::logging::redact_sensitive_text_with_values(
                &error.message,
                [import.access_token.as_str()],
            );
            tracing::debug!(error = %diagnostic, "cursor local import profile enrichment failed");
            (None, Some(diagnostic))
        }
    };
    let upsert = upsert_input_from_cursor_local_import(import, profile_raw, now_ms() as i64);
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::CursorOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    drop(managed_auth_operation);
    Ok(Json(ImportCursorLocalAuthResponse {
        ok: true,
        account: AccountLoginAccountSummary::from_account(&account),
        source,
        path,
        profile_error,
    }))
}

pub(in crate::api) async fn start_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartAccountLoginRequest>,
) -> Result<Json<StartAccountLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    if input.provider_type == ProviderType::CodexOAuth {
        crate::api::invoke::handlers::require_secure_manual_cli_origin(&state, &headers).await?;
    }
    let redirect_uri = match input.provider_type {
        ProviderType::CodexOAuth => {
            Some(crate::domain::accounts::oauth::CODEX_CLI_REDIRECT_URI.to_string())
        }
        ProviderType::GrokOAuth => {
            Some(crate::domain::accounts::oauth::XAI_LOOPBACK_REDIRECT_URI.to_string())
        }
        _ => input
            .redirect_uri
            .or_else(|| Some(default_account_login_redirect_uri(&state))),
    };
    let principal_id = principal.oauth_binding_id();
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let login = state
        .mutate_oauth_logins(|store| {
            store.start_for_principal(
                input.provider_type,
                redirect_uri,
                principal_id,
                now_ms() as i64,
            )
        })
        .await
        .map_err(oauth_login_api_error)?;
    drop(managed_auth_operation);
    Ok(Json(StartAccountLoginResponse { ok: true, login }))
}

pub(in crate::api) async fn cancel_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelAccountLoginRequest>,
) -> Result<Json<CancelAccountLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let principal_id = principal.oauth_binding_id();
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let login = state
        .mutate_oauth_logins(|store| match input.expected_provider_type {
            Some(expected_provider_type) => store.cancel_for_principal_with_expected_provider(
                input.session_id.as_deref(),
                input.state.as_deref(),
                &principal_id,
                expected_provider_type,
                now_ms() as i64,
            ),
            None => store.cancel_for_principal(
                input.session_id.as_deref(),
                input.state.as_deref(),
                &principal_id,
                now_ms() as i64,
            ),
        })
        .await
        .map_err(oauth_login_api_error)?;
    drop(managed_auth_operation);
    Ok(Json(CancelAccountLoginResponse { ok: true, login }))
}

pub(in crate::api) async fn account_login_callback(
    State(state): State<ServerState>,
    Query(query): Query<AccountLoginCallbackQuery>,
) -> Result<Json<FinishAccountLoginResponse>, ApiError> {
    let AccountLoginCallbackQuery {
        state: oauth_state,
        code,
        error,
        error_description,
    } = query;
    if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
        let message = oauth_callback_public_error(error, error_description);
        cancel_oauth_callback_session(&state, oauth_state.as_deref(), None).await;
        return Err(ApiError::bad_request(message));
    }
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let finish = state
        .mutate_oauth_logins(|store| {
            let oauth_state = oauth_state
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    OAuthLoginError::RequestShape("oauth callback state is required".to_string())
                })?;
            store.finish_from_oauth_callback(oauth_state, code.as_deref(), false, now_ms() as i64)
        })
        .await
        .map_err(oauth_login_api_error)?;
    drop(managed_auth_operation);
    Ok(Json(FinishAccountLoginResponse {
        ok: true,
        login: redact_oauth_login_finish(finish),
        account: None,
    }))
}

pub(in crate::api) async fn openai_cli_oauth_callback(
    State(state): State<ServerState>,
    Query(query): Query<AccountLoginCallbackQuery>,
) -> Result<Html<String>, ApiError> {
    cli_oauth_callback(state, query, ProviderType::CodexOAuth, "Codex").await
}

pub(in crate::api) async fn claude_cli_oauth_callback(
    State(state): State<ServerState>,
    Query(query): Query<AccountLoginCallbackQuery>,
) -> Result<Html<String>, ApiError> {
    cli_oauth_callback(state, query, ProviderType::ClaudeOAuth, "Claude").await
}

async fn cli_oauth_callback(
    state: ServerState,
    query: AccountLoginCallbackQuery,
    expected_provider_type: ProviderType,
    label: &str,
) -> Result<Html<String>, ApiError> {
    let AccountLoginCallbackQuery {
        state: oauth_state,
        code,
        error,
        error_description,
    } = query;
    if let Some(error) = error.filter(|value| !value.trim().is_empty()) {
        let message = oauth_callback_public_error(error, error_description);
        cancel_oauth_callback_session(&state, oauth_state.as_deref(), Some(expected_provider_type))
            .await;
        return Ok(Html(oauth_callback_html(label, false, &message)));
    }
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let finish_result = state
        .mutate_oauth_logins(|store| {
            let oauth_state = oauth_state
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    OAuthLoginError::RequestShape("oauth callback state is required".to_string())
                })?;
            store.finish_from_oauth_callback_with_expected_provider(
                oauth_state,
                code.as_deref(),
                true,
                expected_provider_type,
                now_ms() as i64,
            )
        })
        .await;
    drop(managed_auth_operation);
    let mut finish = match finish_result {
        Ok(finish) => finish,
        Err(OAuthLoginError::ProviderMismatch) => {
            return Ok(Html(oauth_callback_html(
                label,
                false,
                &format!("{label} OAuth callback does not match this login session"),
            )));
        }
        Err(error) => return Err(oauth_login_api_error(error)),
    };
    if finish.status == OAuthLoginStatus::TokenExchanged {
        let account = finish
            .account_id
            .as_deref()
            .unwrap_or("the existing account");
        return Ok(Html(oauth_callback_html(
            label,
            true,
            &format!("{label} OAuth login was already completed for {account}"),
        )));
    }
    let account = execute_account_login_token_exchange(&state, &mut finish, None).await?;
    Ok(Html(oauth_callback_html(
        label,
        true,
        &format!("{label} OAuth login completed for {}", account.id),
    )))
}

async fn cancel_oauth_callback_session(
    state: &ServerState,
    oauth_state: Option<&str>,
    expected_provider_type: Option<ProviderType>,
) {
    let Some(oauth_state) = oauth_state.map(str::trim).filter(|value| !value.is_empty()) else {
        return;
    };
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let cancellation = state
        .mutate_oauth_logins(|store| match expected_provider_type {
            Some(expected_provider_type) => store
                .cancel_from_oauth_callback_with_expected_provider(
                    oauth_state,
                    expected_provider_type,
                    now_ms() as i64,
                ),
            None => store.cancel_from_oauth_callback(oauth_state, now_ms() as i64),
        })
        .await;
    drop(managed_auth_operation);
    if let Err(error) = cancellation {
        tracing::debug!(error = %error, "oauth error callback did not match a cancellable login session");
    }
}

fn oauth_callback_public_error(error: String, description: Option<String>) -> String {
    let message = description
        .filter(|value| !value.trim().is_empty())
        .map(|description| format!("{error}: {description}"))
        .unwrap_or(error);
    crate::logging::redact_sensitive_text(&message)
        .chars()
        .take(800)
        .collect()
}

fn oauth_callback_html(label: &str, success: bool, message: &str) -> String {
    let title = if success {
        format!("{label} OAuth completed")
    } else {
        format!("{label} OAuth failed")
    };
    let escaped = message
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    format!(
        r#"<!doctype html><meta charset="utf-8"><title>{title}</title><body><h1>{title}</h1><p>{escaped}</p><p>You can close this window.</p></body>"#
    )
}

fn upsert_input_from_claude_credentials(
    credentials: Value,
) -> Result<UpsertAccountInput, ApiError> {
    let access_token = first_json_string(
        &credentials,
        &[
            "/accessToken",
            "/access_token",
            "/apiKey",
            "/api_key",
            "/claudeAiOauth/accessToken",
            "/claudeAiOauth/access_token",
            "/oauth/accessToken",
            "/oauth/access_token",
            "/tokens/accessToken",
            "/tokens/access_token",
        ],
    );
    let refresh_token = first_json_string(
        &credentials,
        &[
            "/refreshToken",
            "/refresh_token",
            "/claudeAiOauth/refreshToken",
            "/claudeAiOauth/refresh_token",
            "/oauth/refreshToken",
            "/oauth/refresh_token",
            "/tokens/refreshToken",
            "/tokens/refresh_token",
        ],
    );
    if access_token.is_none() && refresh_token.is_none() {
        return Err(ApiError::bad_request(
            "Claude credentials import requires accessToken/access_token or refreshToken/refresh_token",
        ));
    }
    let account_id = first_json_string(
        &credentials,
        &[
            "/accountId",
            "/account_id",
            "/accountUuid",
            "/account_uuid",
            "/claudeAiOauth/accountId",
            "/claudeAiOauth/account_id",
            "/claudeAiOauth/accountUuid",
            "/claudeAiOauth/account_uuid",
            "/account/id",
            "/account/uuid",
        ],
    )
    .unwrap_or_else(|| stable_import_account_id(access_token.as_deref(), refresh_token.as_deref()));
    let email = first_json_string(
        &credentials,
        &[
            "/email",
            "/account/email",
            "/profile/email",
            "/claudeAiOauth/email",
        ],
    );
    let expires_at = first_json_i64(
        &credentials,
        &[
            "/expiresAt",
            "/expires_at",
            "/claudeAiOauth/expiresAt",
            "/claudeAiOauth/expires_at",
            "/oauth/expiresAt",
            "/oauth/expires_at",
            "/tokens/expiresAt",
            "/tokens/expires_at",
        ],
    );
    let token_type = first_json_string(
        &credentials,
        &[
            "/tokenType",
            "/token_type",
            "/claudeAiOauth/tokenType",
            "/claudeAiOauth/token_type",
        ],
    )
    .or_else(|| Some("Bearer".to_string()));
    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::ClaudeOAuth,
        email,
        access_token,
        refresh_token,
        id_token: None,
        token_type,
        api_key: None,
        extra_headers: None,
        scopes: Vec::new(),
        profile: Some(json!({
            "providerType": ProviderType::ClaudeOAuth.as_str(),
            "source": "claude_credentials_import"
        })),
        raw: Some(json!({
            "source": "claude_credentials_import",
            "importedAtMs": now_ms(),
            "credentials": credentials
        })),
        subscription_level: None,
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    })
}

async fn upsert_input_from_grok_auth_json(
    state: &ServerState,
    auth_json: Value,
) -> Result<UpsertAccountInput, ApiError> {
    let entry = grok_auth_json_entry(&auth_json).ok_or_else(|| {
        ApiError::bad_request(
            "Grok auth import requires a ~/.grok/auth.json entry with key/access_token or refresh_token",
        )
    })?;
    let access_token = first_json_string(
        entry,
        &[
            "/key",
            "/accessToken",
            "/access_token",
            "/token",
            "/oauth/accessToken",
            "/oauth/access_token",
        ],
    );
    let refresh_token = first_json_string(
        entry,
        &[
            "/refreshToken",
            "/refresh_token",
            "/oauth/refreshToken",
            "/oauth/refresh_token",
        ],
    );
    if access_token.is_none() && refresh_token.is_none() {
        return Err(ApiError::bad_request(
            "Grok auth import requires key/accessToken/access_token or refreshToken/refresh_token",
        ));
    }
    let id_token = first_json_string(entry, &["/idToken", "/id_token"])
        .ok_or_else(|| ApiError::bad_request("Grok auth import requires a signed id_token"))?;
    let verified = crate::clients::oauth::grok_jwks::verify_grok_id_token(
        &state.http_client().await,
        &id_token,
        None,
    )
    .await
    .map_err(ApiError::bad_request)?;
    let expires_at = normalize_oauth_expires_at(first_json_i64(
        entry,
        &["/expiresAt", "/expires_at", "/expires"],
    ));
    let scope = first_json_string(entry, &["/scope", "/scopes"]);
    let now = now_ms() as i64;
    let tokens = crate::domain::accounts::oauth::OAuthTokenResponse {
        access_token: access_token.unwrap_or_default(),
        refresh_token,
        id_token: Some(id_token),
        token_type: first_json_string(entry, &["/tokenType", "/token_type"])
            .or_else(|| Some("Bearer".to_string())),
        scope,
        expires_in: expires_at
            .map(|expires_at| expires_at.saturating_sub(now).saturating_add(999) / 1_000),
        extra: Value::Null,
    };
    let mut raw = entry.clone();
    if let Some(object) = raw.as_object_mut() {
        object.insert("importedBy".to_string(), json!("grok_auth_json_import"));
        object.insert("importedAtMs".to_string(), json!(now));
    }
    let mut input = crate::domain::accounts::oauth::upsert_input_from_verified_grok_token_response(
        &tokens,
        raw,
        &verified.identity,
        now,
    )
    .map_err(|error| ApiError::bad_request(error.message))?;
    input.expires_at = expires_at.or(input.expires_at);
    crate::domain::accounts::store::set_verified_grok_claims(
        &mut input.profile,
        Some(verified.canonical_claims),
    );
    Ok(input)
}

fn grok_auth_json_entry(value: &Value) -> Option<&Value> {
    if grok_auth_entry_has_secret(value) {
        return Some(value);
    }
    let object = value.as_object()?;
    object
        .iter()
        .find(|(key, entry)| key.contains("auth.x.ai") && grok_auth_entry_has_secret(entry))
        .map(|(_, entry)| entry)
        .or_else(|| {
            object
                .values()
                .find(|entry| grok_auth_entry_has_secret(entry))
        })
}

fn grok_auth_entry_has_secret(value: &Value) -> bool {
    first_json_string(
        value,
        &[
            "/key",
            "/accessToken",
            "/access_token",
            "/refreshToken",
            "/refresh_token",
        ],
    )
    .is_some()
}

fn normalize_oauth_expires_at(value: Option<i64>) -> Option<i64> {
    value.map(|value| {
        if value < 10_000_000_000 {
            value.saturating_mul(1000)
        } else {
            value
        }
    })
}

fn first_json_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
    })
}

fn first_json_i64(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        value.as_i64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
    })
}

fn stable_import_account_id(access_token: Option<&str>, refresh_token: Option<&str>) -> String {
    let seed = refresh_token.or(access_token).unwrap_or("claude-oauth");
    let digest = Sha256::digest(seed.as_bytes());
    let suffix = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("claude-oauth-{suffix}")
}

fn device_flow_expires_at(now_ms: i64, expires_in_secs: u64) -> i64 {
    let ttl_ms = i64::try_from(expires_in_secs)
        .unwrap_or(i64::MAX)
        .saturating_mul(1_000);
    now_ms.saturating_add(ttl_ms)
}

async fn require_device_flow_owner(
    state: &ServerState,
    provider_type: ProviderType,
    device_code: &str,
    principal_id: &str,
    now_ms: i64,
    provider_label: &str,
) -> Result<(), ApiError> {
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let owned = state
        .device_flow_is_owned_by(provider_type, device_code, principal_id, now_ms)
        .await;
    drop(managed_auth_operation);
    if owned {
        Ok(())
    } else {
        Err(ApiError::unauthorized(format!(
            "{provider_label} device flow is expired or unknown"
        )))
    }
}

fn verified_codex_subject(input: &UpsertAccountInput) -> Option<String> {
    input
        .profile
        .as_ref()
        .and_then(|profile| profile.pointer("/verifiedOpenAiClaims/subject"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .map(str::to_string)
}

fn reuse_existing_codex_subject_account(
    store: &crate::domain::accounts::store::AccountStore,
    input: &mut UpsertAccountInput,
    subject: &str,
) {
    if let Some(account_id) = store.codex_account_id_for_verified_subject(subject) {
        input.id = Some(account_id.to_string());
    }
}

pub(in crate::api) async fn finish_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<FinishAccountLoginRequest>,
) -> Result<Json<FinishAccountLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let principal_id = principal.oauth_binding_id();
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let provider_type = state
        .mutate_oauth_logins(|store| {
            store.provider_type_for_principal(
                input.session_id.as_deref(),
                input.state.as_deref(),
                &principal_id,
                now_ms() as i64,
            )
        })
        .await
        .map_err(oauth_login_api_error)?;
    drop(managed_auth_operation);
    if provider_type == ProviderType::CodexOAuth {
        crate::api::invoke::handlers::require_secure_manual_cli_origin(&state, &headers).await?;
    }
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let mut finish = state
        .mutate_oauth_logins(|store| match input.expected_provider_type {
            Some(expected_provider_type) => store.finish_for_principal_with_expected_provider(
                OAuthLoginFinishAttempt {
                    session_id: input.session_id.as_deref(),
                    state: input.state.as_deref(),
                    code: input.code.as_deref(),
                    execute_token_exchange: input.execute_token_exchange.unwrap_or(false),
                },
                &principal_id,
                expected_provider_type,
                now_ms() as i64,
            ),
            None => store.finish_for_principal(
                input.session_id.as_deref(),
                input.state.as_deref(),
                input.code.as_deref(),
                input.execute_token_exchange.unwrap_or(false),
                &principal_id,
                now_ms() as i64,
            ),
        })
        .await
        .map_err(oauth_login_api_error)?;
    drop(managed_auth_operation);
    let account = if input.execute_token_exchange.unwrap_or(false) {
        if finish.status == OAuthLoginStatus::TokenExchanged {
            let account_id = finish
                .account_id
                .as_deref()
                .ok_or_else(|| ApiError::conflict("completed oauth login has no account id"))?;
            let account = state
                .find_account_by_id(account_id)
                .await
                .ok_or_else(|| ApiError::not_found("completed oauth account not found"))?;
            Some(AccountLoginAccountSummary::from_account(&account))
        } else {
            Some(
                execute_account_login_token_exchange(&state, &mut finish, Some(&principal_id))
                    .await?,
            )
        }
    } else {
        None
    };
    Ok(Json(FinishAccountLoginResponse {
        ok: true,
        login: redact_oauth_login_finish(finish),
        account,
    }))
}

pub(in crate::api) fn parse_openai_cli_callback_input(
    input: &str,
) -> Result<(String, String), ApiError> {
    let callback = url::Url::parse(input.trim())
        .map_err(|_| ApiError::bad_request("a complete OpenAI callback URL is required"))?;
    if callback.scheme() != "http"
        || callback.host_str() != Some("localhost")
        || callback.port_or_known_default() != Some(1455)
        || callback.path() != "/auth/callback"
        || !callback.username().is_empty()
        || callback.password().is_some()
        || callback.fragment().is_some()
    {
        return Err(ApiError::bad_request(
            "OpenAI callback URL must match http://localhost:1455/auth/callback",
        ));
    }
    let mut code = None;
    let mut state = None;
    let mut oauth_error = None;
    for (key, value) in callback.query_pairs() {
        match key.as_ref() {
            "code" if code.is_none() => code = Some(value.into_owned()),
            "state" if state.is_none() => state = Some(value.into_owned()),
            "error" if oauth_error.is_none() => oauth_error = Some(value.into_owned()),
            "code" | "state" => {
                return Err(ApiError::bad_request(
                    "OpenAI callback URL contains duplicate OAuth parameters",
                ));
            }
            _ => {}
        }
    }
    if oauth_error.is_some() {
        return Err(ApiError::bad_request(
            "OpenAI authorization returned an OAuth error",
        ));
    }
    let code = code
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("OpenAI callback URL is missing code"))?;
    let state = state
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::bad_request("OpenAI callback URL is missing state"))?;
    Ok((code, state))
}

pub(in crate::api) async fn start_copilot_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartCopilotDeviceLoginRequest>,
) -> Result<Json<StartCopilotDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let device = crate::clients::oauth::copilot_device::start_device_flow(
        &http_client,
        input.github_domain.as_deref(),
    )
    .await
    .map_err(map_copilot_device_error)?;
    let now = now_ms() as i64;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .bind_device_flow_principal(
            ProviderType::GitHubCopilot,
            device.device_code.clone(),
            principal.oauth_binding_id(),
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(StartCopilotDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_copilot_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollCopilotDeviceLoginRequest>,
) -> Result<Json<PollCopilotDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let principal_id = principal.oauth_binding_id();
    require_device_flow_owner(
        &state,
        ProviderType::GitHubCopilot,
        &input.device_code,
        &principal_id,
        now,
        "copilot",
    )
    .await?;
    let http_client = state.http_client().await;
    let result = match crate::clients::oauth::copilot_device::poll_device_flow(
        &http_client,
        &input.device_code,
        input.github_domain.as_deref(),
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
                let managed_auth_operation = state.lock_managed_auth_operations().await;
                state
                    .remove_device_flow_for_principal_under_managed_auth_guard(
                        &managed_auth_operation,
                        ProviderType::GitHubCopilot,
                        &input.device_code,
                        &principal_id,
                        now,
                    )
                    .await;
                drop(managed_auth_operation);
            }
            return Err(map_copilot_device_error(error));
        }
    };
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
    let account = persist_completed_device_login(
        &state,
        ProviderType::GitHubCopilot,
        &input.device_code,
        &principal_id,
        account_input,
    )
    .await?;
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
    let principal = require_web_admin_session(&state, &headers).await?;
    let principal_id = principal.oauth_binding_id();
    let http_client = state.http_client().await;
    let now = now_ms() as i64;
    if let Some(login_provider) = input
        .login_provider
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let (device, flow) = crate::clients::oauth::kiro_device::start_social_device_flow(
            &http_client,
            login_provider,
            input.region.as_deref(),
            now,
        )
        .await
        .map_err(map_kiro_device_error)?;
        let managed_auth_operation = state.lock_managed_auth_operations().await;
        state
            .insert_kiro_social_device_flow(device.device_code.clone(), flow, now)
            .await;
        state
            .bind_device_flow_principal(
                ProviderType::KiroOAuth,
                device.device_code.clone(),
                principal_id,
                device_flow_expires_at(now, device.expires_in),
                now,
            )
            .await;
        drop(managed_auth_operation);
        return Ok(Json(StartKiroDeviceLoginResponse { ok: true, device }));
    }
    let (device, flow) = crate::clients::oauth::kiro_device::start_device_flow(
        &http_client,
        input.region.as_deref(),
        input.start_url.as_deref(),
        input.issuer_url.as_deref(),
        now,
    )
    .await
    .map_err(map_kiro_device_error)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .insert_kiro_device_flow(device.device_code.clone(), flow, now)
        .await;
    state
        .bind_device_flow_principal(
            ProviderType::KiroOAuth,
            device.device_code.clone(),
            principal_id,
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(StartKiroDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_kiro_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollKiroDeviceLoginRequest>,
) -> Result<Json<PollKiroDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let principal_id = principal.oauth_binding_id();
    require_device_flow_owner(
        &state,
        ProviderType::KiroOAuth,
        &input.device_code,
        &principal_id,
        now,
        "kiro",
    )
    .await?;
    let http_client = state.http_client().await;
    let result = if let Some(flow) = state.get_kiro_device_flow(&input.device_code, now).await {
        crate::clients::oauth::kiro_device::poll_device_flow(
            &http_client,
            &input.device_code,
            flow,
            now,
        )
        .await
    } else if let Some(flow) = state
        .get_kiro_social_device_flow(&input.device_code, now)
        .await
    {
        crate::clients::oauth::kiro_device::poll_social_device_flow(
            &http_client,
            &input.device_code,
            flow,
            now,
        )
        .await
    } else {
        return Err(ApiError::unauthorized(
            "kiro device flow is expired or unknown",
        ));
    };
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            if matches!(
                error.status,
                StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
            ) {
                let managed_auth_operation = state.lock_managed_auth_operations().await;
                state
                    .remove_device_flow_for_principal_under_managed_auth_guard(
                        &managed_auth_operation,
                        ProviderType::KiroOAuth,
                        &input.device_code,
                        &principal_id,
                        now,
                    )
                    .await;
                drop(managed_auth_operation);
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
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("kiro device flow completed without account"))?;
    let account = persist_completed_device_login(
        &state,
        ProviderType::KiroOAuth,
        &input.device_code,
        &principal_id,
        account_input,
    )
    .await?;
    Ok(Json(PollKiroDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn start_codex_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(_input): Json<StartCodexDeviceLoginRequest>,
) -> Result<Json<StartCodexDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let now = now_ms() as i64;
    let (device, flow) = crate::clients::oauth::codex_device::start_device_flow(&http_client, now)
        .await
        .map_err(map_codex_device_error)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .insert_codex_device_flow(device.device_code.clone(), flow, now)
        .await;
    state
        .bind_device_flow_principal(
            ProviderType::CodexOAuth,
            device.device_code.clone(),
            principal.oauth_binding_id(),
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(StartCodexDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_codex_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollCodexDeviceLoginRequest>,
) -> Result<Json<PollCodexDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let principal_id = principal.oauth_binding_id();
    require_device_flow_owner(
        &state,
        ProviderType::CodexOAuth,
        &input.device_code,
        &principal_id,
        now,
        "codex",
    )
    .await?;
    let lease = state
        .begin_codex_device_poll(&input.device_code, now)
        .await
        .ok_or_else(|| ApiError::unauthorized("codex device flow is expired or unknown"))?;
    let result = match lease {
        crate::clients::oauth::codex_device::CodexDevicePollLease::Ready(flow) => {
            let http_client = state.http_client().await;
            match crate::clients::oauth::codex_device::poll_device_flow(
                &http_client,
                &input.device_code,
                &flow,
                now,
            )
            .await
            {
                Ok(mut result) => {
                    if let Some(account_input) = result.account_input.as_mut() {
                        match verify_and_mark_managed_account_input(&state, account_input, true)
                            .await
                        {
                            Ok(()) => {}
                            Err(error) => {
                                let managed_auth_operation =
                                    state.lock_managed_auth_operations().await;
                                state.fail_codex_device_poll(&input.device_code, true).await;
                                state
                                    .remove_device_flow_for_principal_under_managed_auth_guard(
                                        &managed_auth_operation,
                                        ProviderType::CodexOAuth,
                                        &input.device_code,
                                        &principal_id,
                                        now,
                                    )
                                    .await;
                                drop(managed_auth_operation);
                                return Err(error);
                            }
                        }
                    }
                    if !state
                        .finish_codex_device_poll(&input.device_code, result.clone())
                        .await
                    {
                        return Err(ApiError::unauthorized(
                            "codex device flow was cancelled while polling",
                        ));
                    }
                    result
                }
                Err(error) => {
                    let terminal = matches!(
                        error.status,
                        StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
                    );
                    if terminal {
                        let managed_auth_operation = state.lock_managed_auth_operations().await;
                        state.fail_codex_device_poll(&input.device_code, true).await;
                        state
                            .remove_device_flow_for_principal_under_managed_auth_guard(
                                &managed_auth_operation,
                                ProviderType::CodexOAuth,
                                &input.device_code,
                                &principal_id,
                                now,
                            )
                            .await;
                        drop(managed_auth_operation);
                    } else {
                        state
                            .fail_codex_device_poll(&input.device_code, false)
                            .await;
                    }
                    return Err(map_codex_device_error(error));
                }
            }
        }
        crate::clients::oauth::codex_device::CodexDevicePollLease::InProgress => {
            return Ok(Json(PollCodexDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_in_progress".to_string(),
                retry_after_secs: Some(1),
                account: None,
            }));
        }
        crate::clients::oauth::codex_device::CodexDevicePollLease::Completed(result) => *result,
    };
    if result.pending {
        return Ok(Json(PollCodexDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    let account_input = result
        .account_input
        .clone()
        .ok_or_else(|| ApiError::bad_gateway("codex device flow completed without account"))?;
    let account = persist_completed_device_login(
        &state,
        ProviderType::CodexOAuth,
        &input.device_code,
        &principal_id,
        account_input,
    )
    .await?;
    Ok(Json(PollCodexDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn cancel_codex_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelCodexDeviceLoginRequest>,
) -> Result<Json<CancelCodexDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let cancelled = state
        .remove_device_flow_for_principal_under_managed_auth_guard(
            &managed_auth_operation,
            ProviderType::CodexOAuth,
            &input.device_code,
            &principal.oauth_binding_id(),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(CancelCodexDeviceLoginResponse {
        ok: true,
        cancelled,
    }))
}

pub(in crate::api) async fn start_grok_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(_input): Json<StartGrokDeviceLoginRequest>,
) -> Result<Json<StartGrokDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let now = now_ms() as i64;
    let (device, flow) = crate::clients::oauth::grok_device::start_device_flow(&http_client, now)
        .await
        .map_err(map_grok_device_error)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .insert_grok_device_flow(device.device_code.clone(), flow, now)
        .await;
    state
        .bind_device_flow_principal(
            ProviderType::GrokOAuth,
            device.device_code.clone(),
            principal.oauth_binding_id(),
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(StartGrokDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_grok_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollGrokDeviceLoginRequest>,
) -> Result<Json<PollGrokDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let principal_id = principal.oauth_binding_id();
    require_device_flow_owner(
        &state,
        ProviderType::GrokOAuth,
        &input.device_code,
        &principal_id,
        now,
        "grok",
    )
    .await?;
    let lease = state
        .begin_grok_device_poll(&input.device_code, now)
        .await
        .ok_or_else(|| ApiError::unauthorized("grok device flow is expired or unknown"))?;
    let result = match lease {
        crate::clients::oauth::grok_device::GrokDevicePollLease::Ready(flow) => {
            let http_client = state.http_client().await;
            match crate::clients::oauth::grok_device::poll_device_flow(
                &http_client,
                &input.device_code,
                &flow,
                now,
            )
            .await
            {
                Ok(result) => {
                    let completed_at = now_ms() as i64;
                    if !state
                        .finish_grok_device_poll(&input.device_code, result.clone(), completed_at)
                        .await
                    {
                        return Err(ApiError::unauthorized(
                            "grok device flow was cancelled while polling",
                        ));
                    }
                    result
                }
                Err(error) => {
                    let completed_at = now_ms() as i64;
                    let terminal = matches!(
                        error.status,
                        StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
                    );
                    if terminal {
                        let managed_auth_operation = state.lock_managed_auth_operations().await;
                        state
                            .fail_grok_device_poll(&input.device_code, true, completed_at)
                            .await;
                        state
                            .remove_device_flow_for_principal_under_managed_auth_guard(
                                &managed_auth_operation,
                                ProviderType::GrokOAuth,
                                &input.device_code,
                                &principal_id,
                                completed_at,
                            )
                            .await;
                        drop(managed_auth_operation);
                    } else {
                        state
                            .fail_grok_device_poll(&input.device_code, false, completed_at)
                            .await;
                    }
                    return Err(map_grok_device_error(error));
                }
            }
        }
        crate::clients::oauth::grok_device::GrokDevicePollLease::InProgress => {
            return Ok(Json(PollGrokDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_in_progress".to_string(),
                retry_after_secs: Some(1),
                account: None,
            }));
        }
        crate::clients::oauth::grok_device::GrokDevicePollLease::Wait(retry_after_secs) => {
            return Ok(Json(PollGrokDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_interval_not_elapsed".to_string(),
                retry_after_secs: Some(retry_after_secs.max(1)),
                account: None,
            }));
        }
        crate::clients::oauth::grok_device::GrokDevicePollLease::Completed(result) => *result,
    };
    if result.pending {
        return Ok(Json(PollGrokDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    let account_input = result
        .account_input
        .clone()
        .ok_or_else(|| ApiError::bad_gateway("grok device flow completed without account"))?;
    let account = persist_completed_grok_device_login(
        &state,
        &input.device_code,
        &principal_id,
        account_input,
    )
    .await?;
    Ok(Json(PollGrokDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

async fn persist_completed_grok_device_login(
    state: &ServerState,
    device_code: &str,
    principal_id: &str,
    account_input: UpsertAccountInput,
) -> Result<Account, ApiError> {
    persist_completed_device_login(
        state,
        ProviderType::GrokOAuth,
        device_code,
        principal_id,
        account_input,
    )
    .await
}

async fn persist_completed_device_login(
    state: &ServerState,
    provider_type: ProviderType,
    device_code: &str,
    principal_id: &str,
    mut account_input: UpsertAccountInput,
) -> Result<Account, ApiError> {
    if account_input.provider_type != provider_type {
        return Err(ApiError::bad_gateway(format!(
            "{} device flow returned a {} account",
            provider_type.as_str(),
            account_input.provider_type.as_str()
        )));
    }
    let verified_codex_subject = (provider_type == ProviderType::CodexOAuth)
        .then(|| verified_codex_subject(&account_input))
        .flatten();
    let completed_at = now_ms() as i64;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    if !state
        .device_flow_is_owned_by(provider_type, device_code, principal_id, completed_at)
        .await
    {
        return Err(ApiError::unauthorized(format!(
            "{} device flow was cancelled before account import",
            provider_type.as_str()
        )));
    }
    let account = state
        .try_mutate_accounts_immediate(|store| {
            if let Some(subject) = verified_codex_subject.as_deref() {
                reuse_existing_codex_subject_account(store, &mut account_input, subject);
            }
            manager_for(provider_type)
                .finish_login(store, account_input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    state
        .remove_device_flow_for_principal_under_managed_auth_guard(
            &managed_auth_operation,
            provider_type,
            device_code,
            principal_id,
            completed_at,
        )
        .await;
    drop(managed_auth_operation);
    state.schedule_gemini_v1internal_project_enrichment(account.provider_type, &account.id);
    if account.provider_type == ProviderType::GitHubCopilot {
        state
            .record_copilot_auth_evidence_if_current(
                &account.id,
                account.auth_identity_generation,
                "copilot_device_flow",
            )
            .await
            .map_err(ApiError::internal)?;
    }
    Ok(account)
}

pub(in crate::api) async fn cancel_grok_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelGrokDeviceLoginRequest>,
) -> Result<Json<CancelGrokDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let cancelled = state
        .remove_device_flow_for_principal_under_managed_auth_guard(
            &managed_auth_operation,
            ProviderType::GrokOAuth,
            &input.device_code,
            &principal.oauth_binding_id(),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(CancelGrokDeviceLoginResponse {
        ok: true,
        cancelled,
    }))
}

pub(in crate::api) async fn start_kimi_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(_input): Json<StartKimiDeviceLoginRequest>,
) -> Result<Json<StartKimiDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let now = now_ms() as i64;
    let (device, flow) = crate::clients::oauth::kimi_device::start_device_flow(&http_client, now)
        .await
        .map_err(map_kimi_device_error)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .insert_kimi_device_flow(device.device_code.clone(), flow, now)
        .await;
    state
        .bind_device_flow_principal(
            ProviderType::KimiCode,
            device.device_code.clone(),
            principal.oauth_binding_id(),
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(StartKimiDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_kimi_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollKimiDeviceLoginRequest>,
) -> Result<Json<PollKimiDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let principal_id = principal.oauth_binding_id();
    require_device_flow_owner(
        &state,
        ProviderType::KimiCode,
        &input.device_code,
        &principal_id,
        now,
        "Kimi",
    )
    .await?;
    let lease = state
        .begin_kimi_device_poll(&input.device_code, now)
        .await
        .ok_or_else(|| ApiError::unauthorized("Kimi device flow is expired or unknown"))?;
    let result = match lease {
        crate::clients::oauth::kimi_device::KimiDevicePollLease::Ready(flow) => {
            let http_client = state.http_client().await;
            match crate::clients::oauth::kimi_device::poll_device_flow(
                &http_client,
                &input.device_code,
                &flow,
                now,
            )
            .await
            {
                Ok(mut result) => {
                    if let Some(account_input) = result.account_input.as_mut() {
                        if let Err(error) =
                            verify_and_mark_managed_account_input(&state, account_input, true).await
                        {
                            let managed_auth_operation = state.lock_managed_auth_operations().await;
                            state
                                .fail_kimi_device_poll(&input.device_code, true, now)
                                .await;
                            state
                                .remove_device_flow_for_principal_under_managed_auth_guard(
                                    &managed_auth_operation,
                                    ProviderType::KimiCode,
                                    &input.device_code,
                                    &principal_id,
                                    now,
                                )
                                .await;
                            drop(managed_auth_operation);
                            return Err(error);
                        }
                    }
                    let completed_at = now_ms() as i64;
                    if !state
                        .finish_kimi_device_poll(&input.device_code, result.clone(), completed_at)
                        .await
                    {
                        return Err(ApiError::unauthorized(
                            "Kimi device flow was cancelled while polling",
                        ));
                    }
                    result
                }
                Err(error) => {
                    let completed_at = now_ms() as i64;
                    let terminal = matches!(
                        error.status,
                        StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
                    );
                    if terminal {
                        let managed_auth_operation = state.lock_managed_auth_operations().await;
                        state
                            .fail_kimi_device_poll(&input.device_code, true, completed_at)
                            .await;
                        state
                            .remove_device_flow_for_principal_under_managed_auth_guard(
                                &managed_auth_operation,
                                ProviderType::KimiCode,
                                &input.device_code,
                                &principal_id,
                                completed_at,
                            )
                            .await;
                        drop(managed_auth_operation);
                    } else {
                        state
                            .fail_kimi_device_poll(&input.device_code, false, completed_at)
                            .await;
                    }
                    return Err(map_kimi_device_error(error));
                }
            }
        }
        crate::clients::oauth::kimi_device::KimiDevicePollLease::InProgress => {
            return Ok(Json(PollKimiDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_in_progress".to_string(),
                retry_after_secs: Some(1),
                account: None,
            }));
        }
        crate::clients::oauth::kimi_device::KimiDevicePollLease::Wait(retry_after_secs) => {
            return Ok(Json(PollKimiDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_interval_not_elapsed".to_string(),
                retry_after_secs: Some(retry_after_secs.max(1)),
                account: None,
            }));
        }
        crate::clients::oauth::kimi_device::KimiDevicePollLease::Completed(result) => *result,
    };
    if result.pending {
        return Ok(Json(PollKimiDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    let account_input = result
        .account_input
        .clone()
        .ok_or_else(|| ApiError::bad_gateway("Kimi device flow completed without account"))?;
    let account = persist_completed_device_login(
        &state,
        ProviderType::KimiCode,
        &input.device_code,
        &principal_id,
        account_input,
    )
    .await?;
    Ok(Json(PollKimiDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn cancel_kimi_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelKimiDeviceLoginRequest>,
) -> Result<Json<CancelKimiDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let cancelled = state
        .remove_device_flow_for_principal_under_managed_auth_guard(
            &managed_auth_operation,
            ProviderType::KimiCode,
            &input.device_code,
            &principal.oauth_binding_id(),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(CancelKimiDeviceLoginResponse {
        ok: true,
        cancelled,
    }))
}

pub(in crate::api) async fn start_qoder_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<StartQoderDeviceLoginRequest>,
) -> Result<Json<StartQoderDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let site = crate::domain::qoder::QoderSite::parse(input.site.as_deref().unwrap_or_default())
        .map_err(ApiError::bad_request)?;
    let now = now_ms() as i64;
    let (device, flow) = crate::clients::oauth::qoder::start_device_flow(site, now)
        .map_err(map_qoder_client_error)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .insert_qoder_device_flow(device.device_code.clone(), flow, now)
        .await;
    state
        .bind_device_flow_principal(
            ProviderType::QoderCosy,
            device.device_code.clone(),
            principal.oauth_binding_id(),
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(StartQoderDeviceLoginResponse { ok: true, device }))
}

pub(in crate::api) async fn poll_qoder_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<PollQoderDeviceLoginRequest>,
) -> Result<Json<PollQoderDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let principal_id = principal.oauth_binding_id();
    let now = now_ms() as i64;
    require_device_flow_owner(
        &state,
        ProviderType::QoderCosy,
        &input.device_code,
        &principal_id,
        now,
        "Qoder",
    )
    .await?;
    if !state
        .qoder_device_flow_state_matches(&input.device_code, &input.state, now)
        .await
    {
        return Err(ApiError::unauthorized(
            "Qoder device flow state is expired or invalid",
        ));
    }
    let lease = state
        .begin_qoder_device_poll(&input.device_code, now)
        .await
        .ok_or_else(|| ApiError::unauthorized("Qoder device flow is expired or unknown"))?;
    let result = match lease {
        crate::clients::oauth::qoder::QoderDevicePollLease::Ready(flow) => {
            let http_client = state.http_client().await;
            match crate::clients::oauth::qoder::poll_device_flow(
                &http_client,
                &flow,
                &input.state,
                now,
            )
            .await
            {
                Ok(result) => {
                    let completed_at = now_ms() as i64;
                    if !state
                        .finish_qoder_device_poll(&input.device_code, result.clone(), completed_at)
                        .await
                    {
                        return Err(ApiError::unauthorized(
                            "Qoder device flow was cancelled while polling",
                        ));
                    }
                    result
                }
                Err(error) => {
                    let completed_at = now_ms() as i64;
                    if error.terminal {
                        let managed_auth_operation = state.lock_managed_auth_operations().await;
                        state
                            .fail_qoder_device_poll(&input.device_code, true, completed_at)
                            .await;
                        state
                            .remove_device_flow_for_principal_under_managed_auth_guard(
                                &managed_auth_operation,
                                ProviderType::QoderCosy,
                                &input.device_code,
                                &principal_id,
                                completed_at,
                            )
                            .await;
                        drop(managed_auth_operation);
                    } else {
                        state
                            .fail_qoder_device_poll(&input.device_code, false, completed_at)
                            .await;
                    }
                    return Err(map_qoder_client_error(error));
                }
            }
        }
        crate::clients::oauth::qoder::QoderDevicePollLease::Wait(retry_after_secs) => {
            return Ok(Json(PollQoderDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_interval_not_elapsed".to_string(),
                retry_after_secs: Some(retry_after_secs.max(1)),
                account: None,
            }));
        }
        crate::clients::oauth::qoder::QoderDevicePollLease::InProgress => {
            return Ok(Json(PollQoderDeviceLoginResponse {
                ok: true,
                pending: true,
                message: "poll_in_progress".to_string(),
                retry_after_secs: Some(1),
                account: None,
            }));
        }
        crate::clients::oauth::qoder::QoderDevicePollLease::Completed(result) => *result,
    };
    if result.pending {
        return Ok(Json(PollQoderDeviceLoginResponse {
            ok: true,
            pending: true,
            message: result.message,
            retry_after_secs: result.retry_after_secs,
            account: None,
        }));
    }
    let account_input = result
        .account_input
        .clone()
        .ok_or_else(|| ApiError::bad_gateway("Qoder device flow completed without account"))?;
    let account = persist_completed_device_login(
        &state,
        ProviderType::QoderCosy,
        &input.device_code,
        &principal_id,
        account_input,
    )
    .await?;
    Ok(Json(PollQoderDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn cancel_qoder_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelQoderDeviceLoginRequest>,
) -> Result<Json<CancelQoderDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let cancelled = state
        .remove_device_flow_for_principal_under_managed_auth_guard(
            &managed_auth_operation,
            ProviderType::QoderCosy,
            &input.device_code,
            &principal.oauth_binding_id(),
            now_ms() as i64,
        )
        .await;
    drop(managed_auth_operation);
    Ok(Json(CancelQoderDeviceLoginResponse {
        ok: true,
        cancelled,
    }))
}

pub(in crate::api) async fn import_qoder_pat(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<ImportQoderPatRequest>,
) -> Result<Json<ImportQoderPatResponse>, ApiError> {
    require_web_admin_session(&state, &headers).await?;
    let http_client = state.http_client().await;
    let account_input = crate::clients::oauth::qoder::import_pat(
        &http_client,
        &input.personal_token,
        now_ms() as i64,
    )
    .await
    .map_err(map_qoder_client_error)?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::QoderCosy)
                .finish_login(store, account_input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    drop(managed_auth_operation);
    Ok(Json(ImportQoderPatResponse {
        ok: true,
        account: AccountLoginAccountSummary::from_account(&account),
    }))
}

pub(in crate::api) async fn execute_account_login_token_exchange(
    state: &ServerState,
    finish: &mut OAuthLoginFinish,
    principal_id: Option<&str>,
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
    let verified_openai_identity = if finish.provider_type == ProviderType::CodexOAuth {
        match crate::clients::oauth::openai_jwks::verify_openai_identity_tokens(
            &http_client,
            token_response.id_token.as_deref(),
            &token_response.access_token,
        )
        .await
        {
            Ok(identity) => Some(identity),
            Err(error) => {
                mark_account_login_exchange_failed(state, &finish.session_id).await;
                return Err(ApiError::bad_request(error));
            }
        }
    } else {
        None
    };
    let verified_grok_identity = if finish.provider_type == ProviderType::GrokOAuth {
        let Some(id_token) = token_response.id_token.as_deref() else {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(
                "Grok OAuth token response is missing id_token",
            ));
        };
        match crate::clients::oauth::grok_jwks::verify_grok_id_token(
            &http_client,
            id_token,
            Some(&finish.state),
        )
        .await
        {
            Ok(identity) => Some(identity),
            Err(error) => {
                mark_account_login_exchange_failed(state, &finish.session_id).await;
                return Err(ApiError::bad_request(error));
            }
        }
    } else {
        None
    };
    let verified_openai_subject = verified_openai_identity
        .as_ref()
        .and_then(|verified| verified.identity.subject.clone());
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
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;
    let input_result = if let Some(verified) = verified_openai_identity.as_ref() {
        upsert_input_from_verified_openai_login_response(
            &token_response,
            raw,
            profile_raw,
            &verified.identity,
            now_ms() as i64,
            interval_ms,
        )
    } else if let Some(verified) = verified_grok_identity.as_ref() {
        upsert_input_from_verified_grok_login_response(
            &token_response,
            raw,
            profile_raw,
            &verified.identity,
            now_ms() as i64,
            interval_ms,
        )
    } else {
        upsert_input_from_login_response(
            finish.provider_type,
            &token_response,
            raw,
            profile_raw,
            now_ms() as i64,
            interval_ms,
        )
    };
    let mut input = match input_result {
        Ok(input) => input,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error.message));
        }
    };
    if let Some(verified) = verified_openai_identity {
        apply_verified_codex_identity(&mut input, verified, true);
    }
    if let Some(verified) = verified_grok_identity {
        apply_verified_grok_identity(&mut input, verified);
    }

    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .mutate_oauth_logins(|store| {
            store.ensure_exchange_commit_allowed(
                &finish.session_id,
                principal_id,
                finish.provider_type,
                now_ms() as i64,
            )
        })
        .await
        .map_err(oauth_login_api_error)?;
    let account_result = match state
        .try_mutate_accounts_immediate(|store| {
            if let Some(subject) = verified_openai_subject.as_deref() {
                reuse_existing_codex_subject_account(store, &mut input, subject);
            }
            manager_for(input.provider_type).finish_login(store, input)
        })
        .await
    {
        Ok(result) => result,
        Err(error) => {
            drop(managed_auth_operation);
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::internal(error));
        }
    };
    let account = match account_result {
        Ok(account) => account,
        Err(error) => {
            drop(managed_auth_operation);
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error));
        }
    };
    state
        .mutate_oauth_logins(|store| store.mark_exchanged(&finish.session_id, &account.id))
        .await
        .map_err(oauth_login_api_error)?;
    drop(managed_auth_operation);
    state.schedule_gemini_v1internal_project_enrichment(account.provider_type, &account.id);

    finish.status = OAuthLoginStatus::TokenExchanged;
    finish.account_id = Some(account.id.clone());
    finish.method = "token_exchange_completed";
    finish.token_request = None;
    finish.account_import_hint = None;
    finish.message = format!(
        "{} OAuth token exchange completed and account was imported",
        finish.provider_type.as_str()
    );

    Ok(AccountLoginAccountSummary::from_account(&account))
}

async fn verify_and_mark_managed_account_input(
    state: &ServerState,
    input: &mut UpsertAccountInput,
    replace_account_record_id: bool,
) -> Result<(), ApiError> {
    match input.provider_type {
        ProviderType::CodexOAuth => {
            crate::domain::accounts::store::clear_codex_workspace_provenance(&mut input.profile);
            let access_token = input
                .access_token
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("Codex OAuth access_token is required"))?;
            let verified = crate::clients::oauth::openai_jwks::verify_openai_identity_tokens(
                &state.http_client().await,
                input.id_token.as_deref(),
                access_token,
            )
            .await
            .map_err(ApiError::bad_request)?;
            apply_verified_codex_identity(input, verified, replace_account_record_id);
        }
        ProviderType::GrokOAuth => {
            let id_token = input
                .id_token
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("Grok OAuth id_token is required"))?;
            let verified = crate::clients::oauth::grok_jwks::verify_grok_id_token(
                &state.http_client().await,
                id_token,
                None,
            )
            .await
            .map_err(ApiError::bad_request)?;
            apply_verified_grok_identity(input, verified);
        }
        ProviderType::KimiCode => {
            let access_token = input
                .access_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let refresh_token = input
                .refresh_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if access_token.is_none() && refresh_token.is_none() {
                return Err(ApiError::bad_request(
                    "Kimi OAuth access_token or refresh_token is required",
                ));
            }
            let token_user_id = access_token.and_then(crate::domain::kimi_cli::extract_user_id);
            let profile_user_id =
                crate::domain::kimi_cli::user_id_from_profile(input.profile.as_ref());
            if token_user_id
                .as_deref()
                .zip(profile_user_id.as_deref())
                .is_some_and(|(token, profile)| token != profile)
            {
                return Err(ApiError::bad_request(
                    "Kimi access token userId does not match the imported profile",
                ));
            }
            let user_id = token_user_id.or(profile_user_id);
            let identity_seed = user_id
                .as_deref()
                .or(access_token)
                .or(refresh_token)
                .expect("Kimi credential presence was checked");
            let account_id = crate::domain::kimi_cli::account_record_id(identity_seed);
            let device =
                crate::domain::kimi_cli::device_identity_from_profile(input.profile.as_ref())
                    .unwrap_or_else(|| {
                        crate::domain::kimi_cli::KimiDeviceIdentity::stable_for_account(&account_id)
                    });
            crate::domain::kimi_cli::enrich_profile(
                &mut input.profile,
                user_id.as_deref(),
                &device,
            );
            input.id = Some(account_id);
        }
        ProviderType::CursorApiKey => {
            let api_key = input
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ApiError::bad_request("Cursor API key is required"))?;
            let verified =
                crate::clients::oauth::cursor::verify_api_key(&state.http_client().await, api_key)
                    .await
                    .map_err(|error| {
                        let status = StatusCode::from_u16(error.status_code)
                            .unwrap_or(StatusCode::BAD_GATEWAY);
                        ApiError::new(status, error.message)
                    })?;
            input.id = Some(verified.account_id);
            input.email = verified.email;
            input.profile = Some(verified.profile);
        }
        _ => {}
    }
    Ok(())
}

fn apply_verified_codex_identity(
    input: &mut UpsertAccountInput,
    verified: crate::clients::oauth::openai_jwks::VerifiedOpenAiIdentity,
    replace_account_record_id: bool,
) {
    if replace_account_record_id || input.id.is_none() {
        input.id = verified
            .identity
            .subject
            .as_deref()
            .and_then(crate::domain::accounts::oauth::openai_account_record_id_from_subject);
    }
    if verified.identity.email.is_some() {
        input.email = verified.identity.email;
    }
    if verified.identity.plan_type.is_some() {
        input.subscription_level = verified.identity.plan_type;
    }
    crate::domain::accounts::store::set_verified_openai_claims(
        &mut input.profile,
        Some(verified.canonical_claims),
    );
}

fn apply_verified_grok_identity(
    input: &mut UpsertAccountInput,
    verified: crate::clients::oauth::grok_jwks::VerifiedGrokIdentity,
) {
    input.id = verified
        .identity
        .subject
        .as_deref()
        .and_then(crate::domain::accounts::oauth::grok_account_record_id_from_subject);
    if verified.identity.email.is_some() {
        input.email = verified.identity.email;
    }
    if verified.identity.plan_type.is_some() {
        input.subscription_level = verified.identity.plan_type;
    }
    crate::domain::accounts::store::set_verified_grok_claims(
        &mut input.profile,
        Some(verified.canonical_claims),
    );
}

pub(in crate::api) async fn execute_account_login_profile_request(
    state: &ServerState,
    provider_type: ProviderType,
    flow: OAuthAuthorizeFlow,
    access_token: &str,
) -> Result<Option<serde_json::Value>, AccountRefreshFailure> {
    if provider_type == ProviderType::ClaudeOAuth {
        let http_client = state.http_client().await;
        return Ok(
            crate::clients::oauth::quota::fetch_claude_bootstrap_profile(
                &http_client,
                access_token,
                state.oauth_quota_refresh_timeout_ms().await,
                now_ms() as i64,
            )
            .await,
        );
    }
    if flow == OAuthAuthorizeFlow::CursorDeepControl {
        return match execute_cursor_profile_request(state, access_token, None).await {
            Ok(profile) => Ok(profile),
            Err(error) => {
                let diagnostic = crate::logging::redact_sensitive_text_with_values(
                    &error.message,
                    [access_token],
                );
                tracing::debug!(error = %diagnostic, "cursor oauth profile enrichment failed");
                Ok(None)
            }
        };
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

pub(in crate::api) async fn execute_cursor_profile_request(
    state: &ServerState,
    access_token: &str,
    workos_user_id: Option<&str>,
) -> Result<Option<serde_json::Value>, AccountRefreshFailure> {
    let workos_user_id = workos_user_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| cursor_workos_user_id_from_access_token(access_token));
    let Some(workos_user_id) = workos_user_id else {
        return Ok(None);
    };
    let Some(request) = build_cursor_profile_request(access_token, &workos_user_id) else {
        return Ok(None);
    };
    let http_client = state.http_client().await;
    execute_oauth_json_request(
        &http_client,
        ProviderType::CursorOAuth,
        &request,
        "cursor oauth profile fetch",
    )
    .await
    .map(Some)
}

pub(in crate::api) async fn mark_account_login_exchange_failed(
    state: &ServerState,
    session_id: &str,
) {
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    state
        .mutate_oauth_logins(|store| store.mark_exchange_failed(session_id))
        .await;
    drop(managed_auth_operation);
}

pub(in crate::api) async fn delete_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let managed_auth_operation = state.lock_managed_auth_operations().await;
    let reference_guard = state.lock_reference_mutations().await;
    let preview = account_delete_preview_inner(&state, &id).await?;
    if preview.blocked {
        return Err(ApiError::conflict_code(
            "cc_switch_account_in_use",
            format!(
                "account is still referenced by {} Provider(s)",
                preview.provider_keys.len()
            ),
        ));
    }
    let (deleted, removed_account) = state
        .try_mutate_accounts_immediate_under_reference_guard(|store| {
            let provider_type = store
                .accounts
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.provider_type);
            match provider_type {
                Some(provider_type) => {
                    let was_default = store
                        .accounts
                        .iter()
                        .find(|account| account.provider_type == provider_type)
                        .is_some_and(|account| account.id == id);
                    let deleted = manager_for(provider_type)
                        .revoke_or_delete(store, &id)
                        .map_err(ApiError::bad_request)?;
                    Ok((deleted, Some((provider_type, was_default))))
                }
                None => Ok((false, None)),
            }
        })
        .await
        .map_err(map_account_write_error)??;
    drop(reference_guard);
    if deleted {
        if let Some((provider_type, was_default)) = removed_account {
            state
                .cancel_managed_auth_for_principal_under_operation_guard(
                    &managed_auth_operation,
                    provider_type,
                    &principal.oauth_binding_id(),
                    now_ms() as i64,
                )
                .await;
            state
                .refresh_account_subscription_metadata_after_removal(
                    provider_type,
                    &id,
                    was_default,
                )
                .await
                .map_err(ApiError::internal)?;
        }
    }
    drop(managed_auth_operation);
    Ok(Json(DeleteResponse { ok: true, deleted }))
}

pub(in crate::api) async fn account_delete_preview(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<AccountDeletePreviewResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(AccountDeletePreviewResponse {
        ok: true,
        preview: account_delete_preview_inner(&state, &id).await?,
    }))
}

async fn account_delete_preview_inner(
    state: &ServerState,
    account_id: &str,
) -> Result<AccountDeletePreview, ApiError> {
    if !state
        .accounts
        .read()
        .await
        .accounts
        .iter()
        .any(|account| account.id == account_id)
    {
        return Err(ApiError::not_found("account not found"));
    }
    let mut provider_keys = state
        .providers
        .read()
        .await
        .providers
        .iter()
        .filter(|stored| {
            crate::domain::providers::runtime::managed_account_binding(stored)
                .is_some_and(|(_, bound_account_id)| bound_account_id == account_id)
        })
        .map(|stored| crate::domain::providers::registry::ProviderKey {
            app: stored.app,
            provider_id: stored.provider.id.clone(),
        })
        .collect::<Vec<_>>();
    provider_keys.sort();
    Ok(AccountDeletePreview {
        account_id: account_id.to_string(),
        blocked: !provider_keys.is_empty(),
        provider_keys,
    })
}

pub(in crate::api) async fn refresh_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let existing = state
        .find_account_by_id(&id)
        .await
        .ok_or_else(|| ApiError::not_found("account not found"))?;

    if existing.provider_type == ProviderType::GitHubCopilot {
        ensure_managed_account_outbound_allowed(&state, &existing).await?;
        state
            .refresh_copilot_upstream_auth_now(
                &existing.id,
                existing.auth_identity_generation,
                None,
            )
            .await
            .map_err(map_copilot_account_refresh_error)?;
        let account = state
            .find_account_by_id(&id)
            .await
            .filter(|account| {
                account.provider_type == existing.provider_type
                    && account.auth_identity_generation == existing.auth_identity_generation
            })
            .ok_or_else(|| ApiError::not_found("account not found"))?;
        return Ok(Json(UpsertAccountResponse {
            ok: true,
            account: account.into(),
        }));
    }

    if provider_native_refresh_available(existing.provider_type) {
        ensure_managed_account_outbound_allowed(&state, &existing).await?;
        state
            .refresh_active_account_now_for_generation(
                existing.provider_type,
                &existing.id,
                existing.auth_identity_generation,
            )
            .await
            .map_err(map_account_managed_refresh_error)?;
        let account = state
            .find_account_by_id(&id)
            .await
            .filter(|account| {
                account.provider_type == existing.provider_type
                    && account.auth_identity_generation == existing.auth_identity_generation
            })
            .ok_or_else(|| ApiError::not_found("account not found"))?;
        return Ok(Json(UpsertAccountResponse {
            ok: true,
            account: account.into(),
        }));
    }

    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(existing.provider_type)
                .refresh_token(store, &id, now_ms() as i64)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(map_account_write_error)??;
    state
        .refresh_account_runtime_metadata_if_changed(&existing, &account)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(UpsertAccountResponse {
        ok: true,
        account: account.into(),
    }))
}

fn map_copilot_account_refresh_error(error: crate::state::CopilotUpstreamAuthError) -> ApiError {
    use crate::state::CopilotUpstreamAuthError;

    match error {
        CopilotUpstreamAuthError::NotFound => ApiError::not_found("account not found"),
        CopilotUpstreamAuthError::IdentityChanged { .. } => ApiError::conflict_code(
            "cc_switch_account_credentials_changed",
            "account credentials changed while Copilot token exchange was in progress; retry",
        ),
        CopilotUpstreamAuthError::CredentialPersistenceDegraded => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ),
        CopilotUpstreamAuthError::MissingGitHubToken { .. } => ApiError::bad_request(
            "GitHub Copilot account lacks the GitHub OAuth token required for token exchange",
        ),
        CopilotUpstreamAuthError::TokenExchange { status_code, .. } => ApiError::new(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            if matches!(status_code, 401 | 403) {
                "GitHub credentials were rejected; sign in again"
            } else if status_code == 429 {
                "GitHub Copilot token exchange was rate limited; retry later"
            } else {
                "GitHub Copilot token exchange failed"
            },
        ),
    }
}

pub(in crate::api) fn account_refresh_api_error(error: AccountRefreshFailure) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
        oauth_error_public_message(error.kind),
    )
    .with_retry_after_ms(error.retry_after_ms)
}

fn map_account_managed_refresh_error(error: crate::state::ManagedAccountRefreshError) -> ApiError {
    use crate::state::ManagedAccountRefreshError;

    match error {
        ManagedAccountRefreshError::Conflict { provider_type } => ApiError::conflict(format!(
            "{} account refresh is already in progress",
            provider_type.as_str()
        )),
        ManagedAccountRefreshError::InactiveCodexAccount => ApiError::conflict_code(
            "cc_switch_codex_inactive_account",
            "Codex OAuth account is no longer active",
        ),
        ManagedAccountRefreshError::IdentityChanged { .. } => ApiError::conflict_code(
            "cc_switch_account_credentials_changed",
            "account credentials changed while OAuth refresh was in progress; retry",
        ),
        ManagedAccountRefreshError::NotFound => ApiError::not_found("account not found"),
        ManagedAccountRefreshError::CredentialPersistenceDegraded => ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ),
        ManagedAccountRefreshError::Refresh {
            status_code,
            message,
            retry_after_ms,
        } => {
            let public_message = if status_code == StatusCode::SERVICE_UNAVAILABLE.as_u16()
                && message == "rotated credentials are live but durable persistence is degraded"
            {
                message
            } else {
                match status_code {
                    400 | 401 | 403 => "OAuth credentials were rejected; sign in again",
                    429 => "OAuth refresh was rate limited; retry later",
                    _ => "OAuth token refresh failed",
                }
                .to_string()
            };
            ApiError::new(
                StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY),
                public_message,
            )
            .with_retry_after_ms(retry_after_ms)
        }
    }
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
    let refresh_request = if account.provider_type == ProviderType::KiroOAuth {
        Some(redact_oauth_request(OAuthHttpRequest {
            method: "POST",
            url: "kiro://dynamic-refresh".to_string(),
            headers: vec![],
            body: json!({
                "grantType": "refresh_token",
                "routing": "authMethod-specific",
                "supportedAuthMethods": ["builder-id", "idc", "social", "external_idp"],
            }),
            body_format: crate::domain::accounts::oauth::OAuthRequestBodyFormat::Json,
        }))
    } else if account.provider_type == ProviderType::GitHubCopilot {
        Some(redact_oauth_request(OAuthHttpRequest {
            method: "GET",
            url: "github-copilot://copilot_internal/v2/token".to_string(),
            headers: vec![],
            body: json!({
                "credential": "github_oauth_token",
                "result": "short_lived_copilot_token",
                "binding": "exact_account_and_identity_generation",
            }),
            body_format: crate::domain::accounts::oauth::OAuthRequestBodyFormat::Json,
        }))
    } else {
        build_refresh_request(account.provider_type, &account)
            .ok()
            .map(redact_oauth_request)
    };
    let profile_request = account
        .access_token
        .as_deref()
        .and_then(|token| build_profile_request(account.provider_type, token))
        .map(redact_oauth_request);
    let refresh_required = token_expires_soon(&account, now_ms() as i64);
    let message = if account.provider_type == ProviderType::KiroOAuth {
        "Kiro native refresh is dynamic and selected by authMethod; API key credentials do not refresh".to_string()
    } else if account.provider_type == ProviderType::GitHubCopilot {
        "GitHub Copilot refresh exchanges the bound account's GitHub OAuth token for a short-lived Copilot token; it never uses the generic OAuth refresh path".to_string()
    } else if spec.is_some_and(|item| item.server_native_refresh_enabled())
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
        .find_account_by_id(&id)
        .await
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    if !query.refresh.unwrap_or(false) {
        let store = state.accounts.read().await;
        let quota = manager_for(existing.provider_type)
            .query_quota(&store, &id)
            .map_err(ApiError::bad_request)?;
        let next_refresh_at = existing.quota_next_refresh_at;
        return Ok(Json(AccountQuotaResponse {
            ok: true,
            quota: account_quota_public_view(&existing, quota.as_ref()),
            account: Some((&existing).into()),
            refreshed: false,
            message: Some(
                "quota snapshot returned; use refresh=true to query upstream".to_string(),
            ),
            next_refresh_at,
        }));
    }

    let force = query.force.unwrap_or(false);
    let now = now_ms() as i64;
    ensure_managed_account_outbound_allowed(&state, &existing).await?;
    state
        .refresh_active_account_if_needed_for_generation(
            existing.provider_type,
            &existing.id,
            existing.auth_identity_generation,
        )
        .await
        .map_err(map_account_managed_refresh_error)?;
    let mut waited_for_in_flight = false;
    let mut quota_refresh_guard = match state
        .account_refresh_locks
        .try_lock(existing.provider_type, &existing.id)
    {
        Some(guard) => guard,
        None => {
            // Coalesce concurrent token/quota refreshes for the same account. Once the
            // in-flight request completes, inspect the persisted quota marker. The
            // same lock also protects token-only refreshes, so waiting alone does not
            // prove that this quota request has already been satisfied.
            waited_for_in_flight = true;
            state
                .account_refresh_locks
                .lock(existing.provider_type, &existing.id)
                .await
        }
    };

    // The account may have been refreshed by the background worker between the
    // initial lookup and lock acquisition. Re-read it and apply cooldown to the
    // latest persisted state while holding the per-account refresh lock.
    let mut active_account = state
        .find_account_by_id(&id)
        .await
        .ok_or_else(|| ApiError::not_found("account not found"))?;
    ensure_quota_refresh_lock_matches_account(existing.provider_type, &active_account)?;
    let native_refresh_attempted = active_account.auth_identity_generation
        != existing.auth_identity_generation
        || active_account.token_refresh_generation != existing.token_refresh_generation;
    if let Some(failure) = quota_refresh_guard.coalesced_quota_failure_for(&active_account) {
        return Err(ApiError::new(
            StatusCode::from_u16(failure.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
            failure
                .public_message
                .as_deref()
                .unwrap_or_else(|| oauth_error_public_message(failure.kind)),
        )
        .with_retry_after_ms(failure.retry_after_ms));
    }
    ensure_managed_account_outbound_allowed(&state, &active_account).await?;
    if waited_for_in_flight && quota_refresh_satisfied_by_in_flight(&existing, &active_account) {
        return Ok(Json(AccountQuotaResponse {
            ok: true,
            quota: account_quota_public_view(&active_account, active_account.quota.as_ref()),
            account: Some((&active_account).into()),
            refreshed: false,
            message: Some("quota refresh coalesced with an in-flight account refresh".to_string()),
            next_refresh_at: active_account.quota_next_refresh_at,
        }));
    }
    if !force {
        if let Some(next_refresh_at) = active_account.quota_next_refresh_at {
            if next_refresh_at > now {
                return Ok(Json(AccountQuotaResponse {
                    ok: true,
                    quota: account_quota_public_view(
                        &active_account,
                        active_account.quota.as_ref(),
                    ),
                    account: Some((&active_account).into()),
                    refreshed: false,
                    message: Some(format!("quota refresh skipped until {next_refresh_at}")),
                    next_refresh_at: Some(next_refresh_at),
                }));
            }
        }
    }

    let account_before_refresh = active_account.clone();
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;
    ensure_managed_account_outbound_allowed(&state, &active_account).await?;
    let http_client = state.http_client().await;
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    let mut quota_result = refresh_account_quota(
        &http_client,
        &active_account,
        now,
        force,
        interval_ms,
        timeout_ms,
    )
    .await;
    if !native_refresh_attempted
        && quota_result
            .as_ref()
            .is_err_and(|error| error.upstream_status == Some(StatusCode::UNAUTHORIZED.as_u16()))
        && !active_account.needs_relogin
        && provider_native_refresh_available(active_account.provider_type)
        && active_account
            .refresh_token
            .as_deref()
            .is_some_and(|token| !token.trim().is_empty())
    {
        let recovery_auth_identity_generation = active_account.auth_identity_generation;
        quota_refresh_guard.release();
        state
            .refresh_active_account_now_for_generation(
                active_account.provider_type,
                &active_account.id,
                recovery_auth_identity_generation,
            )
            .await
            .map_err(map_account_managed_refresh_error)?;
        quota_refresh_guard = state
            .account_refresh_locks
            .lock(existing.provider_type, &existing.id)
            .await;
        active_account = state
            .find_account_by_id(&id)
            .await
            .filter(|account| account.auth_identity_generation == recovery_auth_identity_generation)
            .ok_or_else(|| ApiError::not_found("account not found"))?;
        ensure_quota_refresh_lock_matches_account(existing.provider_type, &active_account)?;
        ensure_managed_account_outbound_allowed(&state, &active_account).await?;
        quota_result = refresh_account_quota(
            &http_client,
            &active_account,
            now,
            true,
            interval_ms,
            timeout_ms,
        )
        .await;
    }
    match quota_result {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let account = state
                .commit_account_quota_refresh_update(&active_account, update)
                .await
                .map_err(map_account_write_error)?
                .map_err(map_quota_commit_skip)?;
            state
                .refresh_account_runtime_metadata_if_changed(&account_before_refresh, &account)
                .await
                .map_err(ApiError::internal)?;
            state.emit_oauth_quota_updated_event(&account, true);
            Ok(Json(AccountQuotaResponse {
                ok: true,
                quota: account_quota_public_view(&account, account.quota.as_ref()),
                account: Some((&account).into()),
                refreshed: true,
                message: Some(message),
                next_refresh_at: account.quota_next_refresh_at,
            }))
        }
        Ok(QuotaRefreshResult::SkippedCooldown {
            next_refresh_at,
            message,
        }) => {
            state
                .refresh_account_runtime_metadata_if_changed(
                    &account_before_refresh,
                    &active_account,
                )
                .await
                .map_err(ApiError::internal)?;
            Ok(Json(AccountQuotaResponse {
                ok: true,
                quota: account_quota_public_view(&active_account, active_account.quota.as_ref()),
                account: Some((&active_account).into()),
                refreshed: false,
                message: Some(message),
                next_refresh_at: Some(next_refresh_at),
            }))
        }
        Err(error) => {
            let public_error = redact_account_public_diagnostic(&active_account, &error.message);
            let status = StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY);
            let next_refresh_at = Some(error.next_refresh_at.unwrap_or_else(|| {
                now.saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS)
            }));
            quota_refresh_guard.record_failure(AccountRefreshFlightFailure::for_account(
                &active_account,
                AccountRefreshFlightStage::QuotaRefresh,
                AccountRefreshFlightFailureDetails {
                    status_code: status.as_u16(),
                    upstream_status: None,
                    message: error.message.clone(),
                    public_message: Some(public_error.clone()),
                    kind: crate::domain::accounts::oauth::OAuthErrorKind::Unknown,
                    retryable: error.retryable,
                    retry_after_ms: None,
                    immediate_relogin: false,
                },
            ));
            let mut update = error.partial_update.as_deref().cloned().unwrap_or_default();
            update.quota_next_refresh_at = next_refresh_at;
            update.last_refresh_error = Some(error.message.clone());
            let updated = state
                .commit_account_quota_refresh_update(&active_account, update)
                .await
                .map_err(map_account_write_error)?
                .map_err(map_quota_commit_skip)?;
            state
                .refresh_account_runtime_metadata_if_changed(&account_before_refresh, &updated)
                .await
                .map_err(ApiError::internal)?;
            Err(ApiError::new(status, public_error))
        }
    }
}

fn ensure_quota_refresh_lock_matches_account(
    locked_provider_type: ProviderType,
    account: &Account,
) -> Result<(), ApiError> {
    if account.provider_type == locked_provider_type {
        return Ok(());
    }
    tracing::info!(
        account_id = %account.id,
        locked_provider_type = %locked_provider_type.as_str(),
        current_provider_type = %account.provider_type.as_str(),
        "account provider type changed while quota refresh waited for its lock"
    );
    Err(ApiError::conflict_code(
        "cc_switch_account_credentials_changed",
        "account provider type changed while quota refresh was waiting; retry the request",
    ))
}

async fn ensure_managed_account_outbound_allowed(
    state: &ServerState,
    account: &Account,
) -> Result<(), ApiError> {
    if state.credential_persistence_degraded() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "managed account credentials are waiting for durable persistence",
        ));
    }
    if account.provider_type == ProviderType::CodexOAuth {
        let accounts = state.accounts_snapshot().await;
        let selection = accounts.codex_oauth_selection();
        if selection.active_account_id.as_deref() != Some(account.id.as_str()) {
            let message = match selection.status {
                crate::domain::accounts::store::CodexOAuthAccountSelectionStatus::Unconfigured => {
                    "Codex OAuth account is not configured".to_string()
                }
                crate::domain::accounts::store::CodexOAuthAccountSelectionStatus::NeedsSelection => {
                    "multiple Codex OAuth accounts exist; select the active account before refreshing quota"
                        .to_string()
                }
                crate::domain::accounts::store::CodexOAuthAccountSelectionStatus::Ready => {
                    format!("Codex OAuth account {} is not active", account.id)
                }
            };
            return Err(ApiError::conflict_code(
                "cc_switch_codex_inactive_account",
                message,
            ));
        }
    }
    Ok(())
}

fn quota_refresh_satisfied_by_in_flight(before: &Account, after: &Account) -> bool {
    if crate::domain::accounts::store::effective_codex_workspace_id(before)
        != crate::domain::accounts::store::effective_codex_workspace_id(after)
    {
        return false;
    }
    timestamp_updated(before.quota_refreshed_at, after.quota_refreshed_at)
}

fn map_quota_commit_skip(skip: crate::state::AccountQuotaCommitSkip) -> ApiError {
    match skip {
        crate::state::AccountQuotaCommitSkip::NotFound => ApiError::not_found("account not found"),
        crate::state::AccountQuotaCommitSkip::Stale(current) => {
            tracing::info!(
                account_id = %current.id,
                provider_type = %current.provider_type.as_str(),
                "discarded quota response for superseded account credentials"
            );
            ApiError::conflict_code(
                "cc_switch_account_credentials_changed",
                "account credentials changed while quota refresh was in progress; retry",
            )
        }
    }
}

fn timestamp_updated(before: Option<i64>, after: Option<i64>) -> bool {
    after.is_some() && after != before
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::domain::accounts::oauth::{OAuthHttpRequest, OAuthRequestBodyFormat};

    fn account_api_test_state(name: &str) -> ServerState {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        crate::state::ServerStateInner::load(
            crate::cli::Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(
                    std::env::temp_dir()
                        .join(format!("cc-switch-server-account-api-{name}-{nanos}")),
                ),
                web_dist_dir: None,
                log_level: "warn".to_string(),
                command: None,
            },
            std::sync::Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap()
    }

    #[test]
    fn account_refresh_api_error_does_not_expose_upstream_message() {
        let error = account_refresh_api_error(AccountRefreshFailure {
            status_code: 400,
            upstream_status: Some(400),
            message: "invalid_grant refresh_token=refresh-secret".to_string(),
            kind: crate::domain::accounts::oauth::OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: false,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        });

        assert_eq!(
            error.message,
            "OAuth credentials were rejected; sign in again"
        );
        assert!(!error.message.contains("refresh-secret"));
    }

    #[test]
    fn copilot_refresh_api_error_does_not_expose_exchange_response() {
        let error = map_copilot_account_refresh_error(
            crate::state::CopilotUpstreamAuthError::TokenExchange {
                status_code: 502,
                message: "upstream body contained github-oauth-secret and copilot-sub-token"
                    .to_string(),
            },
        );

        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(error.message, "GitHub Copilot token exchange failed");
        assert!(!error.message.contains("github-oauth-secret"));
        assert!(!error.message.contains("copilot-sub-token"));
    }

    #[tokio::test]
    async fn copilot_refresh_plan_describes_specialized_dual_credential_exchange() {
        let state = account_api_test_state("copilot-refresh-plan");
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "copilot-refresh-plan-account",
                        "providerType": "github_copilot",
                        "accessToken": "expired-copilot-sub-token",
                        "refreshToken": "github-oauth-long-lived-secret",
                        "expiresAt": 1,
                        "profile": {"githubDomain": "github.com", "ghes": false},
                        "raw": {
                            "githubDomain": "github.com",
                            "githubToken": "github-oauth-long-lived-secret",
                            "copilotToken": {"token": "expired-copilot-sub-token"},
                            "copilotApiBase": "https://api.githubcopilot.com"
                        }
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-web-user-email",
            HeaderValue::from_static("owner@example.com"),
        );

        let Json(plan) = account_refresh_plan(
            State(state),
            headers,
            Path("copilot-refresh-plan-account".to_string()),
        )
        .await
        .unwrap();

        assert!(plan.ok);
        assert_eq!(plan.provider_type, ProviderType::GitHubCopilot);
        assert!(plan.refresh_required);
        assert_eq!(
            plan.server_native_stage,
            Some(crate::domain::accounts::oauth::OAuthSupportStage::NativeRefreshProfile)
        );
        assert_eq!(
            plan.quota_strategy,
            Some(crate::domain::accounts::oauth::OAuthQuotaStrategy::ProviderSpecific)
        );
        assert_eq!(
            oauth_provider_spec(ProviderType::GitHubCopilot)
                .unwrap()
                .quota_capability,
            crate::domain::accounts::oauth::OAuthQuotaCapability::LiveRefresh
        );
        let request = plan.refresh_request.as_ref().unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.url, "github-copilot://copilot_internal/v2/token");
        assert_eq!(request.body["credential"], "github_oauth_token");
        assert_eq!(request.body["result"], "short_lived_copilot_token");
        assert_eq!(
            request.body["binding"],
            "exact_account_and_identity_generation"
        );
        assert!(request.headers.is_empty());
        assert!(plan.profile_request.is_none());
        assert!(plan
            .message
            .contains("never uses the generic OAuth refresh path"));
        let serialized = serde_json::to_string(&plan).unwrap();
        assert!(!serialized.contains("github-oauth-long-lived-secret"));
        assert!(!serialized.contains("expired-copilot-sub-token"));
    }

    #[tokio::test]
    async fn copilot_account_refresh_api_uses_specialized_exchange_and_caches_sub_token() {
        #[derive(Clone, Default)]
        struct Probe {
            get_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
            post_requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
            authorizations: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        }

        let probe = Probe::default();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let get_probe = probe.clone();
        let post_probe = probe.clone();
        let app = axum::Router::new().route(
            "/token",
            axum::routing::get(move |headers: HeaderMap| {
                let probe = get_probe.clone();
                async move {
                    probe
                        .get_requests
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    probe.authorizations.lock().unwrap().push(
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                    );
                    Json(json!({
                        "token": "copilot-refreshed-sub-token",
                        "expires_at": i64::MAX / 2,
                        "endpoints": {"api": "https://api.githubcopilot.com"}
                    }))
                }
            })
            .post(move || {
                let probe = post_probe.clone();
                async move {
                    probe
                        .post_requests
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({"error": "generic refresh must not run"})),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let state = account_api_test_state("copilot-specialized-refresh");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": "copilot-specialized-refresh-account",
                        "providerType": "github_copilot",
                        "accessToken": "copilot-old-sub-token",
                        "refreshToken": "github-oauth-for-refresh",
                        "expiresAt": i64::MAX / 2,
                        "profile": {"githubDomain": "github.com", "ghes": false},
                        "raw": {
                            "githubDomain": "github.com",
                            "githubToken": "github-oauth-for-refresh",
                            "copilotToken": {"token": "copilot-old-sub-token"},
                            "copilotApiBase": "https://api.githubcopilot.com",
                            "testCopilotTokenUrl": token_url
                        }
                    }))
                    .unwrap(),
                );
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-web-user-email",
            HeaderValue::from_static("owner@example.com"),
        );

        let Json(response) = refresh_account(
            State(state.clone()),
            headers,
            Path("copilot-specialized-refresh-account".to_string()),
        )
        .await
        .unwrap();

        assert!(response.ok);
        assert_eq!(response.account.provider_type, ProviderType::GitHubCopilot);
        assert_eq!(response.account.auth_identity_generation, 1);
        assert_eq!(
            probe.get_requests.load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        assert_eq!(
            probe
                .post_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(
            probe.authorizations.lock().unwrap().as_slice(),
            ["token github-oauth-for-refresh"]
        );
        let cached = state
            .prepare_copilot_upstream_auth("copilot-specialized-refresh-account", 1)
            .await
            .unwrap();
        assert_eq!(cached.token, "copilot-refreshed-sub-token");
        assert_eq!(cached.api_endpoint, "https://api.githubcopilot.com");
        assert_eq!(
            probe.get_requests.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the freshly exchanged sub-token must be reused from the account-scoped cache"
        );
        let account = state
            .find_account_by_id("copilot-specialized-refresh-account")
            .await
            .unwrap();
        assert_eq!(account.auth_identity_generation, 1);
        assert_eq!(
            account.refresh_token.as_deref(),
            Some("github-oauth-for-refresh")
        );
        server.abort();
    }

    #[tokio::test]
    async fn grok_auth_json_rejects_missing_or_unsigned_id_token() {
        let state = account_api_test_state("grok-auth-json-signature");
        let missing =
            upsert_input_from_grok_auth_json(&state, json!({"access_token": "access-without-id"}))
                .await
                .unwrap_err();
        assert!(missing.message.contains("signed id_token"));

        let unsigned = upsert_input_from_grok_auth_json(
            &state,
            json!({
                "access_token": "access-with-unsigned-id",
                "id_token": "eyJhbGciOiJub25lIiwia2lkIjoieCJ9.eyJleHAiOjQxMDI0NDQ4MDB9."
            }),
        )
        .await
        .unwrap_err();
        assert!(unsigned.message.contains("RS256"));
    }

    #[tokio::test]
    async fn grok_exchange_missing_id_token_restores_the_login_for_retry() {
        let state = account_api_test_state("grok-exchange-missing-id-token");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..read]);
                if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"access_token":"grok-access","expires_in":3600}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let login = state
            .mutate_oauth_logins(|store| {
                store.start(
                    ProviderType::GrokOAuth,
                    Some(crate::domain::accounts::oauth::XAI_LOOPBACK_REDIRECT_URI.to_string()),
                    1_000,
                )
            })
            .await
            .unwrap();
        let mut finish = state
            .mutate_oauth_logins(|store| {
                store.finish(
                    Some(&login.session_id),
                    Some(&login.state),
                    Some("authorization-code"),
                    true,
                    2_000,
                )
            })
            .await
            .unwrap();
        finish.token_request.as_mut().unwrap().url = format!("http://{address}/token");

        let error = execute_account_login_token_exchange(&state, &mut finish, None)
            .await
            .unwrap_err();
        assert!(error.message.contains("missing id_token"));
        server.await.unwrap();

        let poll_state = state
            .mutate_oauth_logins(|store| store.poll_state_by_oauth_state(&login.state, 2_001))
            .await
            .unwrap();
        assert_eq!(poll_state, OAuthSessionPollState::Ready);
    }

    #[tokio::test]
    async fn completed_grok_device_login_forgets_flow_and_principal_after_persisting() {
        let state = account_api_test_state("grok-device-complete-cleanup");
        let device_code = "grok-device-complete";
        let principal_id = "owner:admin";
        state
            .insert_grok_device_flow(
                device_code.to_string(),
                crate::clients::oauth::grok_device::PendingGrokDeviceFlow {
                    expires_at_ms: i64::MAX,
                    interval: 5,
                },
                0,
            )
            .await;
        state
            .bind_device_flow_principal(
                ProviderType::GrokOAuth,
                device_code.to_string(),
                principal_id.to_string(),
                i64::MAX,
                0,
            )
            .await;

        let account = persist_completed_grok_device_login(
            &state,
            device_code,
            principal_id,
            serde_json::from_value(json!({
                "id": "grok-device-account",
                "providerType": "grok_oauth",
                "accessToken": "grok-device-access",
                "refreshToken": "grok-device-refresh",
                "profile": {
                    "verifiedGrokClaims": {"subject": "grok-device-subject"}
                }
            }))
            .unwrap(),
        )
        .await
        .unwrap();

        assert_eq!(account.id, "grok-device-account");
        assert!(state
            .begin_grok_device_poll(device_code, crate::infra::time::now_ms() as i64)
            .await
            .is_none());
        assert!(
            !state
                .device_flow_is_owned_by(
                    ProviderType::GrokOAuth,
                    device_code,
                    principal_id,
                    crate::infra::time::now_ms() as i64,
                )
                .await
        );
    }

    #[tokio::test]
    async fn failed_grok_device_account_persist_keeps_flow_retryable() {
        let state = account_api_test_state("grok-device-persist-retry");
        let device_code = "grok-device-retry";
        let principal_id = "owner:admin";
        state
            .insert_grok_device_flow(
                device_code.to_string(),
                crate::clients::oauth::grok_device::PendingGrokDeviceFlow {
                    expires_at_ms: i64::MAX,
                    interval: 5,
                },
                0,
            )
            .await;
        state
            .bind_device_flow_principal(
                ProviderType::GrokOAuth,
                device_code.to_string(),
                principal_id.to_string(),
                i64::MAX,
                0,
            )
            .await;
        let config_dir = state.config_dir.clone();
        std::fs::remove_dir_all(&config_dir).unwrap();
        std::fs::write(&config_dir, b"block account persistence").unwrap();

        let result = persist_completed_grok_device_login(
            &state,
            device_code,
            principal_id,
            serde_json::from_value(json!({
                "id": "grok-device-retry-account",
                "providerType": "grok_oauth",
                "accessToken": "grok-device-access",
                "refreshToken": "grok-device-refresh",
                "profile": {
                    "verifiedGrokClaims": {"subject": "grok-device-retry-subject"}
                }
            }))
            .unwrap(),
        )
        .await;

        assert!(result.is_err());
        assert!(matches!(
            state.begin_grok_device_poll(device_code, 1).await,
            Some(crate::clients::oauth::grok_device::GrokDevicePollLease::Ready(_))
        ));
        assert!(
            state
                .device_flow_is_owned_by(ProviderType::GrokOAuth, device_code, principal_id, 1)
                .await
        );
        assert!(state
            .find_account_by_id("grok-device-retry-account")
            .await
            .is_none());

        drop(state);
        std::fs::remove_file(config_dir).unwrap();
    }

    #[tokio::test]
    async fn cancelled_device_flow_completion_cannot_persist_any_provider_account() {
        for provider_type in [
            ProviderType::GitHubCopilot,
            ProviderType::KiroOAuth,
            ProviderType::CodexOAuth,
            ProviderType::GrokOAuth,
        ] {
            let state = account_api_test_state(&format!(
                "cancelled-device-completion-{}",
                provider_type.as_str()
            ));
            let device_code = format!("cancelled-device-{}", provider_type.as_str());
            let principal_id = "owner@example.com:owner";
            let account_id = format!("late-account-{}", provider_type.as_str());
            state
                .bind_device_flow_principal(
                    provider_type,
                    device_code.clone(),
                    principal_id.to_string(),
                    i64::MAX,
                    0,
                )
                .await;
            assert!(
                state
                    .remove_device_flow_principal(provider_type, &device_code, principal_id, 1,)
                    .await
            );

            let error = persist_completed_device_login(
                &state,
                provider_type,
                &device_code,
                principal_id,
                serde_json::from_value(json!({
                    "id": account_id,
                    "providerType": provider_type.as_str(),
                    "accessToken": "late-access-token",
                    "refreshToken": "late-refresh-token"
                }))
                .unwrap(),
            )
            .await
            .unwrap_err();

            assert_eq!(error.status, StatusCode::UNAUTHORIZED);
            assert!(state.find_account_by_id(&account_id).await.is_none());
        }
    }

    #[tokio::test]
    async fn device_flow_completion_rejects_cross_provider_account_input() {
        let state = account_api_test_state("device-cross-provider-account");
        let device_code = "copilot-device-cross-provider";
        let principal_id = "owner@example.com:owner";
        state
            .bind_device_flow_principal(
                ProviderType::GitHubCopilot,
                device_code.to_string(),
                principal_id.to_string(),
                i64::MAX,
                0,
            )
            .await;

        let error = persist_completed_device_login(
            &state,
            ProviderType::GitHubCopilot,
            device_code,
            principal_id,
            serde_json::from_value(json!({
                "id": "cross-provider-account",
                "providerType": "kiro_oauth",
                "accessToken": "cross-provider-access"
            }))
            .unwrap(),
        )
        .await
        .unwrap_err();

        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(
            state
                .device_flow_is_owned_by(ProviderType::GitHubCopilot, device_code, principal_id, 1,)
                .await
        );
        assert!(state
            .find_account_by_id("cross-provider-account")
            .await
            .is_none());
    }

    async fn assert_quota_waiter_reuses_refresh_failure_once(
        name: &str,
        error_description: &'static str,
        expected_needs_relogin: bool,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "error": "invalid_grant",
                            "error_description": error_description
                        })),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = account_api_test_state(name);
        let config_dir = state.config_dir.clone();
        let account_id = format!("quota-waiter-{name}");
        let refresh_token = format!("quota-waiter-refresh-{name}");
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate({
                let account_id = account_id.clone();
                move |accounts| {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": account_id,
                            "providerType": "cursor_oauth",
                            "accessToken": "quota-waiter-expired-access",
                            "refreshToken": refresh_token,
                            "expiresAt": 1,
                            "raw": {
                                "testOAuthTokenUrl": token_url,
                                "currentPeriodUsage": {
                                    "planUsage": {"limit": 1000, "used": 100}
                                }
                            }
                        }))
                        .unwrap(),
                    )
                }
            })
            .await
            .unwrap();
        let account = state.find_account_by_id(&account_id).await.unwrap();
        let mut leader_guard = state
            .account_refresh_locks
            .lock(account.provider_type, &account.id)
            .await;
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-web-user-email",
            HeaderValue::from_static("owner@example.com"),
        );
        let waiter_state = state.clone();
        let waiter_account_id = account_id.clone();
        let mut waiter = tokio::spawn(async move {
            account_quota(
                State(waiter_state),
                headers,
                Path(waiter_account_id),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: Some(true),
                }),
            )
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut waiter)
                .await
                .is_err()
        );

        let http_client = state.http_client().await;
        let error = state
            .execute_native_account_refresh_with_recovery(
                &http_client,
                &account,
                now_ms() as i64,
                state.oauth_quota_refresh_interval_ms().await,
                &mut leader_guard,
            )
            .await
            .unwrap_err();
        state
            .commit_native_refresh_failure(
                &account,
                error.message,
                error.kind,
                error.immediate_relogin,
            )
            .await
            .unwrap();
        leader_guard.release();

        let waiter_error = waiter.await.unwrap().unwrap_err();
        assert_eq!(waiter_error.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            waiter_error.message,
            "OAuth credentials were rejected; sign in again"
        );
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        let account = state.find_account_by_id(&account_id).await.unwrap();
        assert_eq!(account.refresh_consecutive_failures, 1);
        assert_eq!(account.needs_relogin, expected_needs_relogin);

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn quota_waiter_does_not_commit_shared_refresh_failure_twice() {
        assert_quota_waiter_reuses_refresh_failure_once(
            "single-invalid-grant",
            "refresh token was rejected",
            false,
        )
        .await;
    }

    #[tokio::test]
    async fn quota_waiter_propagates_refresh_failure_after_account_needs_relogin() {
        assert_quota_waiter_reuses_refresh_failure_once(
            "reused-invalid-grant",
            "refresh token has already been used",
            true,
        )
        .await;
    }

    #[tokio::test]
    async fn quota_waiter_reuses_native_commit_persistence_failure() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let request_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let request_started_for_route = std::sync::Arc::clone(&request_started);
        let release_response = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_response_for_route = std::sync::Arc::clone(&release_response);
        let upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                let request_started = std::sync::Arc::clone(&request_started_for_route);
                let release_response = std::sync::Arc::clone(&release_response_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    request_started.notify_waiters();
                    release_response.notified().await;
                    Json(json!({
                        "access_token": "quota-commit-degraded-access",
                        "refresh_token": "quota-commit-degraded-refresh",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "account": {"uuid": "quota-commit-degraded-principal"}
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = account_api_test_state("quota-commit-degraded");
        let config_dir = state.config_dir.clone();
        let account_id = "quota-commit-degraded-account";
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "claude_oauth",
                        "accessToken": "quota-commit-old-access",
                        "refreshToken": "quota-commit-old-refresh",
                        "profile": {"accountUUID": "quota-commit-degraded-principal"},
                        "expiresAt": 1,
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                )
            })
            .await
            .unwrap();
        state.inject_account_refresh_persist_failures(1);

        let quota_request = |state: ServerState| async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-cc-switch-web-user-email",
                HeaderValue::from_static("owner@example.com"),
            );
            account_quota(
                State(state),
                headers,
                Path(account_id.to_string()),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: Some(true),
                }),
            )
            .await
        };
        let request_started_signal = request_started.notified();
        tokio::pin!(request_started_signal);
        let first = tokio::spawn(quota_request(state.clone()));
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            &mut request_started_signal,
        )
        .await
        .expect("quota refresh did not reach the token endpoint");
        let mut second = tokio::spawn(quota_request(state.clone()));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut second)
                .await
                .is_err()
        );
        release_response.notify_one();

        let first_error = first.await.unwrap().unwrap_err();
        let second_error = second.await.unwrap().unwrap_err();
        assert_eq!(first_error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(second_error.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            first_error.message,
            "rotated credentials are live but durable persistence is degraded"
        );
        assert_eq!(second_error.message, first_error.message);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);

        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while state.credential_persistence_degraded() {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .unwrap();
        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn refresh_waiter_reuses_recovered_native_commit_state() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let request_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let request_started_for_route = std::sync::Arc::clone(&request_started);
        let release_response = std::sync::Arc::new(tokio::sync::Notify::new());
        let release_response_for_route = std::sync::Arc::clone(&release_response);
        let upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                let request_started = std::sync::Arc::clone(&request_started_for_route);
                let release_response = std::sync::Arc::clone(&release_response_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    request_started.notify_waiters();
                    release_response.notified().await;
                    Json(json!({
                        "access_token": "quota-commit-state-access",
                        "refresh_token": "quota-commit-state-refresh",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "account": {"uuid": "quota-commit-state-principal"}
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = account_api_test_state("quota-commit-state");
        let config_dir = state.config_dir.clone();
        let account_id = "quota-commit-state-account";
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "claude_oauth",
                        "accessToken": "quota-commit-state-old-access",
                        "refreshToken": "quota-commit-state-old-refresh",
                        "profile": {"accountUUID": "quota-commit-state-principal"},
                        "expiresAt": 1,
                        "raw": {"testOAuthTokenUrl": token_url}
                    }))
                    .unwrap(),
                )
            })
            .await
            .unwrap();
        state.inject_account_refresh_commit_state_failures(1);

        let refresh = |state: ServerState| async move {
            state
                .refresh_managed_account_now(ProviderType::ClaudeOAuth, account_id)
                .await
        };
        let request_started_signal = request_started.notified();
        tokio::pin!(request_started_signal);
        let first = tokio::spawn(refresh(state.clone()));
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            &mut request_started_signal,
        )
        .await
        .expect("quota refresh did not reach the token endpoint");
        let mut second = tokio::spawn(refresh(state.clone()));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut second)
                .await
                .is_err()
        );
        release_response.notify_one();

        first.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        let account = state.find_account_by_id(account_id).await.unwrap();
        assert_eq!(
            account.access_token.as_deref(),
            Some("quota-commit-state-access")
        );
        assert_eq!(
            account.refresh_token.as_deref(),
            Some("quota-commit-state-refresh")
        );

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn gemini_quota_401_refreshes_the_same_account_and_replays_once() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let token_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let load_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let quota_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let token_requests_for_route = std::sync::Arc::clone(&token_requests);
        let load_requests_for_route = std::sync::Arc::clone(&load_requests);
        let quota_requests_for_route = std::sync::Arc::clone(&quota_requests);
        let upstream = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: HeaderMap, body: bytes::Bytes| {
                let token_requests = std::sync::Arc::clone(&token_requests_for_route);
                let load_requests = std::sync::Arc::clone(&load_requests_for_route);
                let quota_requests = std::sync::Arc::clone(&quota_requests_for_route);
                async move {
                    let authorization = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok());
                    match uri.path() {
                        "/token" => {
                            assert_eq!(
                                token_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                                0
                            );
                            let body = std::str::from_utf8(&body).unwrap();
                            assert!(body.contains("grant_type=refresh_token"), "{body}");
                            assert!(body.contains("refresh_token=gemini-old-refresh"), "{body}");
                            assert!(body.contains("client_id=gemini-test-client"), "{body}");
                            assert!(body.contains("client_secret=gemini-test-secret"), "{body}");
                            (
                                StatusCode::OK,
                                Json(json!({
                                    "access_token": "gemini-new-access",
                                    "refresh_token": "gemini-new-refresh",
                                    "token_type": "Bearer",
                                    "expires_in": 3600
                                })),
                            )
                        }
                        "/v1internal:loadCodeAssist" => {
                            let request = load_requests
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                                + 1;
                            match request {
                                1 => {
                                    assert_eq!(authorization, Some("Bearer gemini-old-access"));
                                    (
                                        StatusCode::UNAUTHORIZED,
                                        Json(json!({"error": {"message": "expired"}})),
                                    )
                                }
                                2 => {
                                    assert_eq!(authorization, Some("Bearer gemini-new-access"));
                                    (
                                        StatusCode::OK,
                                        Json(json!({
                                            "cloudaicompanionProject": {"id": "gemini-test-project"},
                                            "currentTier": {"name": "PRO"}
                                        })),
                                    )
                                }
                                _ => panic!("unexpected loadCodeAssist replay {request}"),
                            }
                        }
                        "/v1internal:retrieveUserQuota" => {
                            assert_eq!(
                                quota_requests
                                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
                                0
                            );
                            assert_eq!(authorization, Some("Bearer gemini-new-access"));
                            let body: Value = serde_json::from_slice(&body).unwrap();
                            assert_eq!(body["project"], "gemini-test-project");
                            (
                                StatusCode::OK,
                                Json(json!({
                                    "buckets": [{
                                        "modelId": "gemini-2.5-pro",
                                        "remainingFraction": 0.75
                                    }]
                                })),
                            )
                        }
                        path => panic!("unexpected Gemini quota test request: {path}"),
                    }
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = account_api_test_state("gemini-quota-401-replay");
        let config_dir = state.config_dir.clone();
        let account_id = "gemini-quota-401-account";
        let token_url = format!("http://{address}/token");
        let code_assist_base_url = format!("http://{address}");
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "gemini_cli",
                        "accessToken": "gemini-old-access",
                        "refreshToken": "gemini-old-refresh",
                        "tokenType": "Bearer",
                        "expiresAt": i64::MAX,
                        "raw": {
                            "clientId": "gemini-test-client",
                            "clientSecret": "gemini-test-secret",
                            "testOAuthTokenUrl": token_url,
                            "testGeminiCodeAssistBaseUrl": code_assist_base_url
                        }
                    }))
                    .unwrap(),
                )
            })
            .await
            .unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-web-user-email",
            HeaderValue::from_static("owner@example.com"),
        );
        let Json(response) = account_quota(
            State(state.clone()),
            headers,
            Path(account_id.to_string()),
            Query(AccountQuotaQuery {
                refresh: Some(true),
                force: Some(true),
            }),
        )
        .await
        .unwrap();

        assert!(response.ok);
        assert!(response.refreshed);
        let quota = response.quota.unwrap();
        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(token_requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(load_requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(quota_requests.load(std::sync::atomic::Ordering::SeqCst), 1);

        let live = state.find_account_by_id(account_id).await.unwrap();
        assert_eq!(live.access_token.as_deref(), Some("gemini-new-access"));
        assert_eq!(live.refresh_token.as_deref(), Some("gemini-new-refresh"));
        let persisted =
            crate::domain::accounts::store::AccountStore::load_or_default(&config_dir).unwrap();
        let persisted = persisted
            .find_for_provider(ProviderType::GeminiCli, Some(account_id))
            .unwrap();
        assert_eq!(persisted.access_token.as_deref(), Some("gemini-new-access"));
        assert_eq!(
            persisted.refresh_token.as_deref(),
            Some("gemini-new-refresh")
        );

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn codex_quota_requires_the_active_account_after_waiting_for_refresh_lock() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let upstream = axum::Router::new().route(
            "/token",
            axum::routing::post(move || {
                let requests = std::sync::Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    (
                        StatusCode::BAD_REQUEST,
                        Json(json!({"error": "invalid_grant"})),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = account_api_test_state("codex-quota-active-gate");
        let config_dir = state.config_dir.clone();
        let token_url = format!("http://{address}/token");
        state
            .mutate_accounts_immediate(move |accounts| {
                for account_id in ["codex-quota-active-a", "codex-quota-active-b"] {
                    accounts.upsert(
                        serde_json::from_value(json!({
                            "id": account_id,
                            "providerType": "codex_oauth",
                            "accessToken": format!("{account_id}-access"),
                            "refreshToken": format!("{account_id}-refresh"),
                            "expiresAt": 1,
                            "profile": {
                                "verifiedOpenAIClaims": {
                                    "subject": format!("{account_id}-subject"),
                                    "chatgpt_account_id": format!("{account_id}-workspace")
                                }
                            },
                            "raw": {"testOAuthTokenUrl": token_url}
                        }))
                        .unwrap(),
                    );
                }
            })
            .await
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-web-user-email",
            HeaderValue::from_static("owner@example.com"),
        );
        let quota = |state: ServerState, headers: HeaderMap| async move {
            account_quota(
                State(state),
                headers,
                Path("codex-quota-active-a".to_string()),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: Some(true),
                }),
            )
            .await
        };

        let needs_selection = quota(state.clone(), headers.clone()).await.unwrap_err();
        assert_eq!(needs_selection.status, StatusCode::CONFLICT);
        assert_eq!(
            needs_selection.code,
            Some("cc_switch_codex_inactive_account")
        );
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);

        state
            .mutate_accounts_immediate(|accounts| {
                accounts.select_active_codex_oauth_account("codex-quota-active-b")
            })
            .await
            .unwrap()
            .unwrap();
        let inactive = quota(state.clone(), headers.clone()).await.unwrap_err();
        assert_eq!(inactive.status, StatusCode::CONFLICT);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);

        state
            .select_active_codex_oauth_account_command("codex-quota-active-a")
            .await
            .unwrap()
            .unwrap();
        let account = state
            .find_account_by_id("codex-quota-active-a")
            .await
            .unwrap();
        let guard = state
            .account_refresh_locks
            .lock(account.provider_type, &account.id)
            .await;
        let mut waiter = tokio::spawn(quota(state.clone(), headers));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut waiter)
                .await
                .is_err()
        );
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.select_active_codex_oauth_account("codex-quota-active-b")
            })
            .await
            .unwrap()
            .unwrap();
        guard.release();

        let switched = waiter.await.unwrap().unwrap_err();
        assert_eq!(switched.status, StatusCode::CONFLICT);
        assert_eq!(switched.code, Some("cc_switch_codex_inactive_account"));
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn quota_waiter_reuses_quota_failure_instead_of_returning_stale_snapshot() {
        let state = account_api_test_state("quota-upstream-failure-flight");
        let config_dir = state.config_dir.clone();
        let account_id = "quota-upstream-failure-flight-account";
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "cursor_oauth",
                        "accessToken": "quota-upstream-failure-access",
                        "expiresAt": i64::MAX,
                        "raw": {
                            "testQuotaRefreshDelayMs": 250,
                            "currentPeriodUsage": {}
                        }
                    }))
                    .unwrap(),
                )
            })
            .await
            .unwrap();

        let quota_request = |state: ServerState| async move {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-cc-switch-web-user-email",
                HeaderValue::from_static("owner@example.com"),
            );
            account_quota(
                State(state),
                headers,
                Path(account_id.to_string()),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: Some(true),
                }),
            )
            .await
        };
        let first = tokio::spawn(quota_request(state.clone()));
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !state
                .account_refresh_locks
                .is_locked(ProviderType::CursorOAuth, account_id)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("quota failure leader did not acquire the account refresh lock");
        let mut second = tokio::spawn(quota_request(state.clone()));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut second)
                .await
                .is_err()
        );

        let first_error = first.await.unwrap().unwrap_err();
        let second_error = tokio::time::timeout(std::time::Duration::from_millis(100), second)
            .await
            .expect("quota failure waiter issued a second provider quota refresh")
            .unwrap()
            .unwrap_err();
        assert_eq!(first_error.status, StatusCode::BAD_REQUEST);
        assert_eq!(second_error.status, first_error.status);
        assert_eq!(second_error.message, first_error.message);
        let account = state.find_account_by_id(account_id).await.unwrap();
        assert!(account.quota_next_refresh_at.is_some());
        assert!(account.quota_refreshed_at.is_none());

        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[tokio::test]
    async fn quota_waiter_rejects_same_id_recreated_for_another_provider() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream_requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let upstream_requests_for_route = std::sync::Arc::clone(&upstream_requests);
        let upstream = axum::Router::new().fallback(move || {
            let upstream_requests = std::sync::Arc::clone(&upstream_requests_for_route);
            async move {
                upstream_requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Json(json!({}))
            }
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let state = account_api_test_state("quota-provider-type-replacement");
        let config_dir = state.config_dir.clone();
        let account_id = "quota-provider-type-replacement-account";
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "cursor_oauth",
                        "accessToken": "old-provider-access",
                        "expiresAt": i64::MAX,
                        "raw": {
                            "currentPeriodUsage": {
                                "planUsage": {"limit": 1000, "used": 100}
                            }
                        }
                    }))
                    .unwrap(),
                )
            })
            .await
            .unwrap();
        let old_provider_guard = state
            .account_refresh_locks
            .lock(ProviderType::CursorOAuth, account_id)
            .await;

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-cc-switch-web-user-email",
            HeaderValue::from_static("owner@example.com"),
        );
        let waiter_state = state.clone();
        let mut waiter = tokio::spawn(async move {
            account_quota(
                State(waiter_state),
                headers,
                Path(account_id.to_string()),
                Query(AccountQuotaQuery {
                    refresh: Some(true),
                    force: Some(true),
                }),
            )
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(25), &mut waiter)
                .await
                .is_err()
        );

        let gemini_base_url = format!("http://{address}");
        state
            .mutate_accounts_immediate(move |accounts| {
                assert!(accounts.delete(account_id));
                accounts.upsert(
                    serde_json::from_value(json!({
                        "id": account_id,
                        "providerType": "gemini_cli",
                        "accessToken": "replacement-provider-access",
                        "expiresAt": i64::MAX,
                        "raw": {"testGeminiCodeAssistBaseUrl": gemini_base_url}
                    }))
                    .unwrap(),
                )
            })
            .await
            .unwrap();
        old_provider_guard.release();

        let error = tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("quota waiter remained blocked on the superseded provider lock")
            .unwrap()
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.code, Some("cc_switch_account_credentials_changed"));
        assert_eq!(
            error.message,
            "account provider type changed while quota refresh was waiting; retry the request"
        );
        assert_eq!(
            upstream_requests.load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        let replacement = state.find_account_by_id(account_id).await.unwrap();
        assert_eq!(replacement.provider_type, ProviderType::GeminiCli);
        assert!(replacement.quota_refreshed_at.is_none());

        server.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn quota_refresh_lock_binding_rejects_provider_type_changes() {
        let account: Account = serde_json::from_value(json!({
            "id": "quota-lock-binding-account",
            "providerType": "gemini_cli",
            "accessToken": "replacement-provider-access"
        }))
        .unwrap();

        assert!(
            ensure_quota_refresh_lock_matches_account(ProviderType::GeminiCli, &account).is_ok()
        );
        let error = ensure_quota_refresh_lock_matches_account(ProviderType::CursorOAuth, &account)
            .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.code, Some("cc_switch_account_credentials_changed"));
    }

    #[test]
    fn quota_singleflight_only_coalesces_when_quota_marker_advanced() {
        let before: Account = serde_json::from_value(json!({
            "id": "acct-codex",
            "providerType": "codex_oauth",
            "accessToken": "old-token",
            "lastRefreshError": "old quota error"
        }))
        .unwrap();
        let mut token_only = before.clone();
        token_only.access_token = Some("new-token".to_string());
        token_only.last_refresh_error = None;
        assert!(!quota_refresh_satisfied_by_in_flight(&before, &token_only));

        let mut quota_success = token_only.clone();
        quota_success.quota_refreshed_at = Some(1_000);
        quota_success.quota_next_refresh_at = Some(2_000);
        assert!(quota_refresh_satisfied_by_in_flight(
            &before,
            &quota_success
        ));

        let mut quota_failure = token_only;
        quota_failure.quota_next_refresh_at = Some(3_000);
        assert!(!quota_refresh_satisfied_by_in_flight(
            &before,
            &quota_failure
        ));

        let mut prior_long_cooldown = quota_failure.clone();
        prior_long_cooldown.quota_next_refresh_at = Some(10_000);
        assert!(!quota_refresh_satisfied_by_in_flight(
            &prior_long_cooldown,
            &quota_failure
        ));

        let mut cache_cleared = quota_success.clone();
        cache_cleared.quota_refreshed_at = None;
        cache_cleared.quota_next_refresh_at = None;
        assert!(!quota_refresh_satisfied_by_in_flight(
            &quota_success,
            &cache_cleared
        ));

        let mut workspace_a = quota_success;
        workspace_a.profile = Some(json!({
            "verifiedOpenAiClaims": {
                "chatgpt_account_id": "workspace-a"
            },
            "codexWorkspaceProvenance": {
                "workspaceId": "workspace-b",
                "source": "authenticated_discovery",
                "verifiedAt": 123
            },
            "selectedChatgptAccountId": "workspace-a"
        }));
        let mut workspace_b = workspace_a.clone();
        workspace_b.profile.as_mut().unwrap()["selectedChatgptAccountId"] = json!("workspace-b");
        workspace_b.quota_refreshed_at = Some(4_000);
        workspace_b.quota_next_refresh_at = Some(5_000);
        assert!(!quota_refresh_satisfied_by_in_flight(
            &workspace_a,
            &workspace_b
        ));
    }

    #[tokio::test]
    async fn quota_failure_flight_is_replayed_instead_of_coalescing_cooldown_marker() {
        let before: Account = serde_json::from_value(json!({
            "id": "quota-failure-flight-account",
            "providerType": "cursor_oauth",
            "accessToken": "quota-failure-access"
        }))
        .unwrap();
        let mut after = before.clone();
        after.quota_next_refresh_at = Some(10_000);
        after.last_refresh_error = Some("upstream quota failed".to_string());
        let locks =
            std::sync::Arc::new(crate::domain::accounts::managers::AccountRefreshLocks::default());
        let mut leader = locks.lock(before.provider_type, &before.id).await;
        let waiter_started = std::sync::Arc::new(tokio::sync::Notify::new());
        let waiter_started_signal = waiter_started.notified();
        tokio::pin!(waiter_started_signal);
        let waiter_locks = std::sync::Arc::clone(&locks);
        let waiter_started_for_task = std::sync::Arc::clone(&waiter_started);
        let provider_type = before.provider_type;
        let account_id = before.id.clone();
        let waiter = tokio::spawn(async move {
            waiter_started_for_task.notify_waiters();
            waiter_locks.lock(provider_type, &account_id).await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            &mut waiter_started_signal,
        )
        .await
        .expect("quota failure waiter did not start");

        leader.record_failure(AccountRefreshFlightFailure::for_account(
            &after,
            AccountRefreshFlightStage::QuotaRefresh,
            AccountRefreshFlightFailureDetails {
                status_code: StatusCode::BAD_GATEWAY.as_u16(),
                upstream_status: Some(StatusCode::INTERNAL_SERVER_ERROR.as_u16()),
                message: "upstream quota failed".to_string(),
                public_message: Some("quota refresh failed".to_string()),
                kind: crate::domain::accounts::oauth::OAuthErrorKind::Unknown,
                retryable: true,
                retry_after_ms: None,
                immediate_relogin: false,
            },
        ));
        leader.release();
        let waiter = waiter.await.unwrap();
        assert!(waiter.coalesced_native_failure_for(&after).is_none());
        let failure = waiter.coalesced_quota_failure_for(&after).unwrap();
        assert_eq!(failure.status_code, StatusCode::BAD_GATEWAY.as_u16());
        assert_eq!(
            failure.public_message.as_deref(),
            Some("quota refresh failed")
        );
        assert!(!quota_refresh_satisfied_by_in_flight(&before, &after));
    }

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

    #[test]
    fn openai_manual_callback_requires_exact_url_code_and_state() {
        let (code, state) = parse_openai_cli_callback_input(
            "http://localhost:1455/auth/callback?code=code%2Fvalue&state=state-value",
        )
        .unwrap();
        assert_eq!(code, "code/value");
        assert_eq!(state, "state-value");

        for invalid in [
            "code-only",
            "http://127.0.0.1:1455/auth/callback?code=x&state=y",
            "http://localhost:1455/other?code=x&state=y",
            "http://localhost:1455/auth/callback?code=x",
            "http://localhost:1455/auth/callback?code=x&state=y&state=z",
        ] {
            assert!(
                parse_openai_cli_callback_input(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn oauth_callback_error_redacts_provider_diagnostics() {
        let message = oauth_callback_public_error(
            "access_denied".to_string(),
            Some("api_key=secret-provider-detail".to_string()),
        );

        assert!(message.contains("access_denied"));
        assert!(!message.contains("secret-provider-detail"));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn verified_openai_identity_preserves_explicit_local_record_id() {
        let mut aliased: UpsertAccountInput = serde_json::from_value(json!({
            "id": "local-account-alias",
            "providerType": "codex_oauth",
            "accessToken": "signed-access-token"
        }))
        .unwrap();
        apply_verified_codex_identity(
            &mut aliased,
            crate::clients::oauth::openai_jwks::VerifiedOpenAiIdentity {
                identity: crate::domain::accounts::oauth::OAuthIdentity {
                    account_id: Some("workspace-verified".to_string()),
                    subject: Some("user-verified".to_string()),
                    ..Default::default()
                },
                canonical_claims: json!({
                    "subject": "user-verified",
                    "chatgpt_account_id": "workspace-verified"
                }),
            },
            false,
        );

        assert_eq!(aliased.id.as_deref(), Some("local-account-alias"));
        assert_eq!(
            aliased
                .profile
                .as_ref()
                .and_then(|profile| profile.pointer("/verifiedOpenAiClaims/chatgpt_account_id"))
                .and_then(Value::as_str),
            Some("workspace-verified")
        );

        let mut login = aliased;
        apply_verified_codex_identity(
            &mut login,
            crate::clients::oauth::openai_jwks::VerifiedOpenAiIdentity {
                identity: crate::domain::accounts::oauth::OAuthIdentity {
                    account_id: Some("workspace-login".to_string()),
                    subject: Some("user-login".to_string()),
                    ..Default::default()
                },
                canonical_claims: json!({
                    "subject": "user-login",
                    "chatgpt_account_id": "workspace-login"
                }),
            },
            true,
        );
        assert_eq!(
            login.id,
            crate::domain::accounts::oauth::openai_account_record_id_from_subject("user-login")
        );
    }

    #[test]
    fn managed_codex_login_reuses_legacy_record_for_verified_subject() {
        let mut store = crate::domain::accounts::store::AccountStore::default();
        let existing: UpsertAccountInput = serde_json::from_value(json!({
            "id": "legacy-workspace-id",
            "providerType": "codex_oauth",
            "accessToken": "old-access-token",
            "profile": {
                "verifiedOpenAiClaims": {
                    "subject": "user-legacy",
                    "chatgpt_account_id": "workspace-shared"
                }
            }
        }))
        .unwrap();
        store.upsert(existing);
        let mut login: UpsertAccountInput = serde_json::from_value(json!({
            "id": "codex-oauth-new-id",
            "providerType": "codex_oauth",
            "accessToken": "new-access-token"
        }))
        .unwrap();

        reuse_existing_codex_subject_account(&store, &mut login, "user-legacy");

        assert_eq!(login.id.as_deref(), Some("legacy-workspace-id"));
    }
}
