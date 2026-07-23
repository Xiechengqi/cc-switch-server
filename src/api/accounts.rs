use super::*;
use axum::response::Html;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

pub(in crate::api) async fn list_accounts(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<ListAccountsResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let accounts = state
        .accounts_snapshot()
        .await
        .accounts
        .iter()
        .map(AccountPublicView::from)
        .collect();
    Ok(Json(ListAccountsResponse { ok: true, accounts }))
}

pub(in crate::api) async fn upsert_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(mut input): Json<UpsertAccountInput>,
) -> Result<Json<UpsertAccountResponse>, ApiError> {
    require_session(&state, &headers).await?;
    verify_and_mark_codex_account_input(&state, &mut input, false).await?;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            let manager = manager_for(input.provider_type);
            manager
                .finish_login(store, input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::ClaudeOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let upsert = upsert_input_from_grok_auth_json(input.auth_json)?;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::GrokOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::KiroOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(ProviderType::CursorOAuth)
                .finish_login(store, upsert)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let redirect_uri = if input.provider_type == ProviderType::CodexOAuth {
        Some(crate::domain::accounts::oauth::CODEX_CLI_REDIRECT_URI.to_string())
    } else {
        input.redirect_uri.or_else(|| {
            if input.provider_type == ProviderType::GrokOAuth {
                Some(crate::domain::accounts::oauth::XAI_LOOPBACK_REDIRECT_URI.to_string())
            } else {
                Some(default_account_login_redirect_uri(&state))
            }
        })
    };
    let principal_id = principal.oauth_binding_id();
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
    Ok(Json(StartAccountLoginResponse { ok: true, login }))
}

pub(in crate::api) async fn cancel_account_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelAccountLoginRequest>,
) -> Result<Json<CancelAccountLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let principal_id = principal.oauth_binding_id();
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
        return Err(ApiError::bad_request(message));
    }
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
        return Ok(Html(oauth_callback_html(label, false, &message)));
    }
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
    let account = execute_account_login_token_exchange(&state, &mut finish).await?;
    Ok(Html(oauth_callback_html(
        label,
        true,
        &format!("{label} OAuth login completed for {}", account.id),
    )))
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

fn upsert_input_from_grok_auth_json(auth_json: Value) -> Result<UpsertAccountInput, ApiError> {
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
    let id_token = first_json_string(entry, &["/idToken", "/id_token"]);
    let identity = id_token
        .as_deref()
        .and_then(identity_from_jwt)
        .or_else(|| access_token.as_deref().and_then(identity_from_jwt));
    let account_id = first_json_string(entry, &["/id", "/accountId", "/account_id", "/sub"])
        .or_else(|| identity.as_ref().and_then(|item| item.account_id.clone()))
        .unwrap_or_else(|| {
            stable_grok_import_account_id(access_token.as_deref(), refresh_token.as_deref())
        });
    let email = first_json_string(
        entry,
        &[
            "/email",
            "/preferredUsername",
            "/preferred_username",
            "/profile/email",
        ],
    )
    .or_else(|| identity.as_ref().and_then(|item| item.email.clone()));
    let subscription_level = first_json_string(
        entry,
        &[
            "/tier",
            "/subscriptionTier",
            "/subscription_tier",
            "/profile/tier",
        ],
    )
    .or_else(|| identity.as_ref().and_then(|item| item.plan_type.clone()));
    let entitlement_status = first_json_string(
        entry,
        &[
            "/entitlementStatus",
            "/entitlement_status",
            "/profile/entitlementStatus",
            "/profile/entitlement_status",
        ],
    );
    let expires_at = normalize_oauth_expires_at(first_json_i64(
        entry,
        &["/expiresAt", "/expires_at", "/expires"],
    ));
    Ok(UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::GrokOAuth,
        email: email.clone(),
        access_token,
        refresh_token,
        id_token,
        token_type: first_json_string(entry, &["/tokenType", "/token_type"])
            .or_else(|| Some("Bearer".to_string())),
        api_key: None,
        extra_headers: None,
        scopes: first_json_string(entry, &["/scope", "/scopes"])
            .map(|scope| scope.split_whitespace().map(str::to_string).collect())
            .unwrap_or_default(),
        profile: Some(json!({
            "providerType": ProviderType::GrokOAuth.as_str(),
            "source": "grok_auth_json_import",
            "accountId": identity.as_ref().and_then(|item| item.account_id.clone()),
            "email": email,
            "planType": subscription_level.clone(),
            "entitlementStatus": entitlement_status.clone(),
            "poid": identity.as_ref().and_then(|item| item.poid.clone()),
            "organizations": identity.as_ref().and_then(|item| item.organizations.clone()),
        })),
        raw: Some(json!({
            "source": "grok_auth_json_import",
            "importedAtMs": now_ms(),
            "entry": entry,
        })),
        subscription_level,
        entitlement_status,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    })
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

fn stable_grok_import_account_id(
    access_token: Option<&str>,
    refresh_token: Option<&str>,
) -> String {
    let seed = refresh_token.or(access_token).unwrap_or("grok-oauth");
    let digest = Sha256::digest(seed.as_bytes());
    let suffix = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("grok-oauth-{suffix}")
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
    if state
        .device_flow_is_owned_by(provider_type, device_code, principal_id, now_ms)
        .await
    {
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
    if provider_type == ProviderType::CodexOAuth {
        crate::api::invoke::handlers::require_secure_manual_cli_origin(&state, &headers).await?;
    }
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
            Some(execute_account_login_token_exchange(&state, &mut finish).await?)
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
    state
        .bind_device_flow_principal(
            ProviderType::GitHubCopilot,
            device.device_code.clone(),
            principal.oauth_binding_id(),
            device_flow_expires_at(now, device.expires_in),
            now,
        )
        .await;
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
                state
                    .remove_device_flow_principal(
                        ProviderType::GitHubCopilot,
                        &input.device_code,
                        &principal_id,
                        now,
                    )
                    .await;
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
    state
        .remove_device_flow_principal(
            ProviderType::GitHubCopilot,
            &input.device_code,
            &principal_id,
            now,
        )
        .await;
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("copilot device flow completed without account"))?;
    let provider_type = account_input.provider_type;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(provider_type)
                .finish_login(store, account_input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let (result, social) =
        if let Some(flow) = state.get_kiro_device_flow(&input.device_code, now).await {
            (
                crate::clients::oauth::kiro_device::poll_device_flow(
                    &http_client,
                    &input.device_code,
                    flow,
                    now,
                )
                .await,
                false,
            )
        } else if let Some(flow) = state
            .get_kiro_social_device_flow(&input.device_code, now)
            .await
        {
            (
                crate::clients::oauth::kiro_device::poll_social_device_flow(
                    &http_client,
                    &input.device_code,
                    flow,
                    now,
                )
                .await,
                true,
            )
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
                state
                    .remove_device_flow_principal(
                        ProviderType::KiroOAuth,
                        &input.device_code,
                        &principal_id,
                        now,
                    )
                    .await;
                if social {
                    state
                        .remove_kiro_social_device_flow(&input.device_code)
                        .await;
                } else {
                    state.remove_kiro_device_flow(&input.device_code).await;
                }
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
    if social {
        state
            .remove_kiro_social_device_flow(&input.device_code)
            .await;
    } else {
        state.remove_kiro_device_flow(&input.device_code).await;
    }
    state
        .remove_device_flow_principal(
            ProviderType::KiroOAuth,
            &input.device_code,
            &principal_id,
            now,
        )
        .await;
    let account_input = result
        .account_input
        .ok_or_else(|| ApiError::bad_gateway("kiro device flow completed without account"))?;
    let provider_type = account_input.provider_type;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(provider_type)
                .finish_login(store, account_input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
                        match verify_and_mark_codex_account_input(&state, account_input, true).await
                        {
                            Ok(()) => {}
                            Err(error) => {
                                state.fail_codex_device_poll(&input.device_code, true).await;
                                state
                                    .remove_device_flow_principal(
                                        ProviderType::CodexOAuth,
                                        &input.device_code,
                                        &principal_id,
                                        now,
                                    )
                                    .await;
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
                    state
                        .fail_codex_device_poll(&input.device_code, terminal)
                        .await;
                    if terminal {
                        state
                            .remove_device_flow_principal(
                                ProviderType::CodexOAuth,
                                &input.device_code,
                                &principal_id,
                                now,
                            )
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
    let mut account_input = result
        .account_input
        .clone()
        .ok_or_else(|| ApiError::bad_gateway("codex device flow completed without account"))?;
    let verified_subject = verified_codex_subject(&account_input);
    let provider_type = account_input.provider_type;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            if let Some(subject) = verified_subject.as_deref() {
                reuse_existing_codex_subject_account(store, &mut account_input, subject);
            }
            manager_for(provider_type)
                .finish_login(store, account_input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
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
    let owned = state
        .remove_device_flow_principal(
            ProviderType::CodexOAuth,
            &input.device_code,
            &principal.oauth_binding_id(),
            now,
        )
        .await;
    let cancelled = if owned {
        state.cancel_codex_device_flow(&input.device_code).await
    } else {
        false
    };
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
                    if !state
                        .finish_grok_device_poll(&input.device_code, result.clone())
                        .await
                    {
                        return Err(ApiError::unauthorized(
                            "grok device flow was cancelled while polling",
                        ));
                    }
                    result
                }
                Err(error) => {
                    let terminal = matches!(
                        error.status,
                        StatusCode::UNAUTHORIZED | StatusCode::BAD_REQUEST
                    );
                    state
                        .fail_grok_device_poll(&input.device_code, terminal)
                        .await;
                    if terminal {
                        state
                            .remove_device_flow_principal(
                                ProviderType::GrokOAuth,
                                &input.device_code,
                                &principal_id,
                                now,
                            )
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
    let provider_type = account_input.provider_type;
    let account = state
        .try_mutate_accounts_immediate(|store| {
            manager_for(provider_type)
                .finish_login(store, account_input)
                .map_err(ApiError::bad_request)
        })
        .await
        .map_err(ApiError::internal)??;
    Ok(Json(PollGrokDeviceLoginResponse {
        ok: true,
        pending: false,
        message: result.message,
        retry_after_secs: None,
        account: Some(AccountLoginAccountSummary::from_account(&account)),
    }))
}

pub(in crate::api) async fn cancel_grok_device_login(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<CancelGrokDeviceLoginRequest>,
) -> Result<Json<CancelGrokDeviceLoginResponse>, ApiError> {
    let principal = require_web_admin_session(&state, &headers).await?;
    let now = now_ms() as i64;
    let owned = state
        .remove_device_flow_principal(
            ProviderType::GrokOAuth,
            &input.device_code,
            &principal.oauth_binding_id(),
            now,
        )
        .await;
    let cancelled = if owned {
        state.cancel_grok_device_flow(&input.device_code).await
    } else {
        false
    };
    Ok(Json(CancelGrokDeviceLoginResponse {
        ok: true,
        cancelled,
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
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::internal(error));
        }
    };
    let account = match account_result {
        Ok(account) => account,
        Err(error) => {
            mark_account_login_exchange_failed(state, &finish.session_id).await;
            return Err(ApiError::bad_request(error));
        }
    };
    state
        .mutate_oauth_logins(|store| store.mark_exchanged(&finish.session_id, &account.id))
        .await
        .map_err(oauth_login_api_error)?;

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

async fn verify_and_mark_codex_account_input(
    state: &ServerState,
    input: &mut UpsertAccountInput,
    replace_account_record_id: bool,
) -> Result<(), ApiError> {
    if input.provider_type != ProviderType::CodexOAuth {
        return Ok(());
    }
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
    state
        .mutate_oauth_logins(|store| store.mark_exchange_failed(session_id))
        .await;
}

pub(in crate::api) async fn delete_account(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<DeleteResponse>, ApiError> {
    require_session(&state, &headers).await?;
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
        .try_mutate_accounts_immediate(|store| {
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
        .map_err(ApiError::internal)??;
    drop(reference_guard);
    if deleted {
        if let Some((provider_type, was_default)) = removed_account {
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
            stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
                .and_then(|binding| binding.account_id.as_deref())
                == Some(account_id)
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

    if provider_native_refresh_available(existing.provider_type) {
        let now = now_ms() as i64;
        let _refresh_guard = state
            .account_refresh_locks
            .try_lock(existing.provider_type, &existing.id)
            .ok_or_else(|| ApiError::conflict("account refresh is already in progress"))?;
        let http_client = state.http_client().await;
        let interval_ms = state.oauth_quota_refresh_interval_ms().await;
        let update = match execute_native_account_refresh(&http_client, &existing, now, interval_ms)
            .await
        {
            Ok(update) => update,
            Err(error) => {
                let updated = state
                    .mutate_accounts_immediate(|store| {
                        store.mark_native_refresh_failure(&id, error.message.clone(), error.kind)
                    })
                    .await
                    .map_err(ApiError::internal)?;
                if let Some(updated) = updated {
                    state
                        .refresh_account_runtime_metadata_if_changed(&existing, &updated)
                        .await
                        .map_err(ApiError::internal)?;
                }
                return Err(account_refresh_api_error(error));
            }
        };
        let account = state
            .try_mutate_accounts_immediate(|store| {
                store
                    .mark_native_refresh_success(&id, update)
                    .ok_or_else(|| ApiError::not_found("account not found"))
            })
            .await
            .map_err(ApiError::internal)??;
        state
            .refresh_account_runtime_metadata_if_changed(&existing, &account)
            .await
            .map_err(ApiError::internal)?;
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
        .map_err(ApiError::internal)??;
    state
        .refresh_account_runtime_metadata_if_changed(&existing, &account)
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(UpsertAccountResponse {
        ok: true,
        account: account.into(),
    }))
}

pub(in crate::api) fn account_refresh_api_error(error: AccountRefreshFailure) -> ApiError {
    ApiError::new(
        StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
        oauth_error_public_message(error.kind),
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
    let mut waited_for_in_flight = false;
    let _quota_refresh_guard = match state
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
    let now = now_ms() as i64;
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
    if account_needs_native_refresh(&active_account, now) {
        let http_client = state.http_client().await;
        let update =
            match execute_native_account_refresh(&http_client, &active_account, now, interval_ms)
                .await
            {
                Ok(update) => update,
                Err(error) => {
                    let updated = state
                        .mutate_accounts_immediate(|store| {
                            store.mark_native_refresh_failure(
                                &id,
                                error.message.clone(),
                                error.kind,
                            )
                        })
                        .await
                        .map_err(ApiError::internal)?;
                    if let Some(updated) = updated {
                        state
                            .refresh_account_runtime_metadata_if_changed(
                                &account_before_refresh,
                                &updated,
                            )
                            .await
                            .map_err(ApiError::internal)?;
                    }
                    return Err(account_refresh_api_error(error));
                }
            };
        active_account = state
            .try_mutate_accounts_immediate(|store| {
                store
                    .mark_native_refresh_success(&id, update)
                    .ok_or_else(|| ApiError::not_found("account not found"))
            })
            .await
            .map_err(ApiError::internal)??;
    }

    let http_client = state.http_client().await;
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    match refresh_account_quota(
        &http_client,
        &active_account,
        now,
        force,
        interval_ms,
        timeout_ms,
    )
    .await
    {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let account = state
                .try_mutate_accounts_immediate(|store| {
                    store
                        .mark_refresh_success(&id, update)
                        .ok_or_else(|| ApiError::not_found("account not found"))
                })
                .await
                .map_err(ApiError::internal)??;
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
            let next_refresh_at = Some(error.next_refresh_at.unwrap_or_else(|| {
                now.saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS)
            }));
            let updated = state
                .mutate_accounts_immediate(|store| {
                    store.mark_refresh_success(
                        &id,
                        AccountRefreshUpdate {
                            quota_next_refresh_at: next_refresh_at,
                            last_refresh_error: Some(error.message.clone()),
                            ..Default::default()
                        },
                    )
                })
                .await
                .map_err(ApiError::internal)?;
            if let Some(updated) = updated {
                state
                    .refresh_account_runtime_metadata_if_changed(&account_before_refresh, &updated)
                    .await
                    .map_err(ApiError::internal)?;
            }
            Err(ApiError::new(
                StatusCode::from_u16(error.status_code).unwrap_or(StatusCode::BAD_GATEWAY),
                public_error,
            ))
        }
    }
}

fn quota_refresh_satisfied_by_in_flight(before: &Account, after: &Account) -> bool {
    if crate::domain::accounts::store::effective_codex_workspace_id(before)
        != crate::domain::accounts::store::effective_codex_workspace_id(after)
    {
        return false;
    }
    timestamp_updated(before.quota_refreshed_at, after.quota_refreshed_at)
        || timestamp_updated(before.quota_next_refresh_at, after.quota_next_refresh_at)
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

    use super::*;
    use crate::domain::accounts::oauth::{OAuthHttpRequest, OAuthRequestBodyFormat};

    #[test]
    fn account_refresh_api_error_does_not_expose_upstream_message() {
        let error = account_refresh_api_error(AccountRefreshFailure {
            status_code: 400,
            message: "invalid_grant refresh_token=refresh-secret".to_string(),
            kind: crate::domain::accounts::oauth::OAuthErrorKind::InvalidGrant,
            retryable: false,
        });

        assert_eq!(
            error.message,
            "OAuth credentials were rejected; sign in again"
        );
        assert!(!error.message.contains("refresh-secret"));
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
        assert!(quota_refresh_satisfied_by_in_flight(
            &before,
            &quota_failure
        ));

        let mut prior_long_cooldown = quota_failure.clone();
        prior_long_cooldown.quota_next_refresh_at = Some(10_000);
        assert!(quota_refresh_satisfied_by_in_flight(
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
                "chatgpt_account_id": "workspace-a",
                "organizations": [{"id": "workspace-b"}]
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
