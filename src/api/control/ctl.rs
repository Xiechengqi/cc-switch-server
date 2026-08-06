use super::*;
use crate::api::{oauth_error_public_message, redact_account_public_diagnostic};
use crate::domain::accounts::store::Account;
use crate::domain::providers::runtime::{
    authoritative_managed_account, managed_account_binding_with_generation,
    managed_account_provider_type,
};

type HmacSha256 = Hmac<Sha256>;

const CONTROL_SIGNATURE_WINDOW_MS: i64 = 5 * 60 * 1000;

pub(crate) async fn control_prepare_client_subdomain_adoption(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlClientSubdomainAdoptionResponse>, ApiError> {
    verify_control_request(
        &state,
        PREPARE_CLIENT_SUBDOMAIN_ADOPTION_PATH,
        &headers,
        &body,
    )
    .await?;
    let input: ControlPrepareClientSubdomainAdoptionInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let adoption = state
        .prepare_client_subdomain_adoption(
            &input.takeover_id,
            &input.from_subdomain,
            &input.to_subdomain,
            input.activate_at_ms,
        )
        .await
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    crate::state::schedule_client_subdomain_adoption(state, adoption.clone());
    Ok(Json(adoption.into()))
}

pub(crate) async fn control_commit_client_subdomain_adoption(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlClientSubdomainAdoptionResponse>, ApiError> {
    verify_control_request(
        &state,
        COMMIT_CLIENT_SUBDOMAIN_ADOPTION_PATH,
        &headers,
        &body,
    )
    .await?;
    let input: ControlClientSubdomainAdoptionIdInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let (adoption, activated) = state
        .commit_client_subdomain_adoption(&input.takeover_id)
        .await
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    if activated {
        crate::state::spawn_client_subdomain_adoption_reconnect(state, "subdomain_adoption_commit");
    }
    Ok(Json(adoption.into()))
}

pub(crate) async fn control_abort_client_subdomain_adoption(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlAbortClientSubdomainAdoptionResponse>, ApiError> {
    verify_control_request(
        &state,
        ABORT_CLIENT_SUBDOMAIN_ADOPTION_PATH,
        &headers,
        &body,
    )
    .await?;
    let input: ControlClientSubdomainAdoptionIdInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let aborted = state
        .abort_client_subdomain_adoption(&input.takeover_id)
        .await
        .map_err(|error| ApiError::conflict(error.to_string()))?;
    Ok(Json(ControlAbortClientSubdomainAdoptionResponse {
        ok: true,
        takeover_id: input.takeover_id,
        aborted,
    }))
}

pub(crate) async fn control_apply_share_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlApplyShareSettingsResponse>, ApiError> {
    verify_control_request(&state, APPLY_SHARE_SETTINGS_PATH, &headers, &body).await?;
    let input: ControlApplyShareSettingsInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    if let Some(requested_owner) = input.patch.owner_email.as_deref() {
        let configured_owner = state
            .config
            .read()
            .await
            .owner
            .email
            .clone()
            .ok_or_else(|| ApiError::conflict("client owner email is not configured"))?;
        if !configured_owner.eq_ignore_ascii_case(requested_owner.trim()) {
            return Err(ApiError::conflict(
                "share owner is managed by the client owner",
            ));
        }
    }
    // Control RPC is Router → Client only. Managed Share Market grants must be
    // applied here too; the ordinary dashboard patch helper rejects them.
    let share = state
        .apply_router_share_settings_patch_immediate(&input.share_id, input.patch)
        .await
        .map_err(ApiError::internal)?
        .map_err(|error| match error {
            crate::domain::sharing::shares::SharePatchError::NotFound => {
                ApiError::not_found("share not found")
            }
            crate::domain::sharing::shares::SharePatchError::BindingImmutable => {
                ApiError::conflict_code("cc_switch_share_binding_immutable", error.to_string())
            }
            crate::domain::sharing::shares::SharePatchError::Invalid(message) => {
                ApiError::bad_request(message)
            }
        })?;
    let providers = state.providers.read().await.clone();
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|item| item.id == share.id)
        .cloned()
        .unwrap_or(share);
    let descriptor = descriptor_for_share_with_accounts_and_usage(
        &share,
        &providers,
        Some(&accounts),
        Some(&usage),
    );
    Ok(Json(ControlApplyShareSettingsResponse {
        ok: true,
        share: descriptor,
    }))
}

pub(crate) async fn control_refresh_share_usage(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlRefreshShareUsageResponse>, ApiError> {
    verify_control_request(&state, REFRESH_SHARE_USAGE_PATH, &headers, &body).await?;
    let input: ControlRefreshShareUsageInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let providers = state.providers.read().await.clone();
    let share = state
        .shares
        .read()
        .await
        .shares
        .iter()
        .find(|item| item.id == input.share_id)
        .cloned()
        .ok_or_else(|| ApiError::not_found("share not found"))?;
    let refreshed =
        refresh_share_usage_items(&state, &share, input.app.as_deref(), &providers).await;
    let accounts = state.accounts.read().await.clone();
    let usage = state.usage.read().await.clone();
    state
        .mutate_shares_immediate(|shares| {
            shares.refresh_runtime_snapshots(&providers, Some(&accounts), &usage);
        })
        .await
        .map_err(ApiError::internal)?;
    Ok(Json(ControlRefreshShareUsageResponse {
        ok: true,
        refreshed,
    }))
}

const CONTROL_LOG_MAX_LINES: usize = 100;
const CONTROL_LOG_MAX_LINE_BYTES: usize = 16 * 1024;
const CONTROL_LOG_MAX_RESPONSE_BYTES: usize = 256 * 1024;

pub(crate) async fn control_client_log_tail(
    State(state): State<ServerState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    Query(query): Query<ControlClientLogTailQuery>,
) -> Result<Response, ApiError> {
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(CLIENT_LOG_TAIL_PATH);
    verify_control_request_for_method(&state, "GET", path_and_query, &headers, &[]).await?;
    if !(1..=CONTROL_LOG_MAX_LINES).contains(&query.lines) {
        return Err(ApiError::bad_request(format!(
            "lines must be between 1 and {CONTROL_LOG_MAX_LINES}"
        )));
    }
    let tail = state
        .read_router_log_tail(query.lines)
        .await
        .map_err(|error| match error {
            crate::logging::LogTailAccessError::Disabled => {
                ApiError::forbidden("router log collection is disabled")
            }
        })?;
    let (content, lines, response_truncated) = bounded_control_log_content(&tail.content);
    let payload = fit_control_log_response(ControlClientLogTailResponse {
        ok: true,
        lines,
        truncated: tail.truncated || response_truncated,
        content,
    });
    let mut response = Json(payload).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

fn fit_control_log_response(
    mut response: ControlClientLogTailResponse,
) -> ControlClientLogTailResponse {
    while serde_json::to_vec(&response)
        .expect("serialize Client log response")
        .len()
        > CONTROL_LOG_MAX_RESPONSE_BYTES
    {
        response.truncated = true;
        response.content = response
            .content
            .split_once('\n')
            .map(|(_, remaining)| remaining.to_string())
            .unwrap_or_default();
        response.lines = response.content.lines().count();
    }
    response
}

fn bounded_control_log_content(content: &str) -> (String, usize, bool) {
    let mut truncated = false;
    let mut used_bytes = 0_usize;
    let mut lines = Vec::new();
    for raw_line in content.lines().rev().take(CONTROL_LOG_MAX_LINES) {
        let redacted = crate::logging::redact_sensitive_text(&raw_line.replace('\r', "\\r"));
        if redacted.len() > CONTROL_LOG_MAX_LINE_BYTES {
            truncated = true;
        }
        let line = bounded_utf8(&redacted, CONTROL_LOG_MAX_LINE_BYTES);
        let additional = line.len() + usize::from(!lines.is_empty());
        if used_bytes.saturating_add(additional) > CONTROL_LOG_MAX_RESPONSE_BYTES {
            truncated = true;
            break;
        }
        used_bytes += additional;
        lines.push(line);
    }
    if content.lines().count() > lines.len() {
        truncated = true;
    }
    lines.reverse();
    let line_count = lines.len();
    (lines.join("\n"), line_count, truncated)
}

fn bounded_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub(crate) async fn verify_control_request(
    state: &ServerState,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ApiError> {
    verify_control_request_for_method(state, "POST", path, headers, body).await
}

pub(crate) async fn verify_control_request_for_method(
    state: &ServerState,
    method: &str,
    path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), ApiError> {
    let installation_id = required_header(headers, "x-ctl-installation-id")?;
    let timestamp_raw = required_header(headers, "x-ctl-timestamp-ms")?;
    let nonce = required_header(headers, "x-ctl-nonce")?;
    let signature_raw = required_header(headers, "x-ctl-signature")?;
    let timestamp_ms = timestamp_raw
        .parse::<i64>()
        .map_err(|_| ApiError::unauthorized("bad control timestamp"))?;
    let now = now_ms() as i64;
    let delta = if now >= timestamp_ms {
        now - timestamp_ms
    } else {
        timestamp_ms - now
    };
    if delta > CONTROL_SIGNATURE_WINDOW_MS {
        return Err(ApiError::unauthorized("stale control request"));
    }
    if nonce.trim().is_empty() {
        return Err(ApiError::unauthorized("missing control nonce"));
    }

    let config = state.config.read().await;
    let identity = config
        .router
        .identity
        .as_ref()
        .ok_or_else(|| ApiError::unauthorized("router identity is not registered"))?;
    if identity.installation_id != installation_id {
        return Err(ApiError::unauthorized("control installation mismatch"));
    }
    let secret = identity
        .control_secret
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| ApiError::unauthorized("router control secret is unavailable"))?;
    let provided = BASE64_STANDARD
        .decode(signature_raw)
        .map_err(|_| ApiError::unauthorized("bad control signature"))?;
    let expected = control_signature_for_method(method, path, secret, body, timestamp_ms, nonce)?;
    if !constant_time_eq(&provided, &expected) {
        return Err(ApiError::unauthorized("bad control signature"));
    }
    if !state
        .control_nonces
        .register(installation_id, nonce, now, CONTROL_SIGNATURE_WINDOW_MS)
    {
        return Err(ApiError::unauthorized("replay control request"));
    }
    Ok(())
}

pub(crate) fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::unauthorized(format!("missing {name}")))
}

pub fn control_signature(
    path: &str,
    secret: &str,
    body: &[u8],
    timestamp_ms: i64,
    nonce: &str,
) -> Result<Vec<u8>, ApiError> {
    control_signature_for_method("POST", path, secret, body, timestamp_ms, nonce)
}

pub fn control_signature_for_method(
    method: &str,
    path: &str,
    secret: &str,
    body: &[u8],
    timestamp_ms: i64,
    nonce: &str,
) -> Result<Vec<u8>, ApiError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::unauthorized("bad control secret"))?;
    mac.update(method.as_bytes());
    mac.update(b"\n");
    mac.update(path.as_bytes());
    mac.update(b"\n");
    mac.update(body);
    mac.update(b"\n");
    mac.update(timestamp_ms.to_string().as_bytes());
    mac.update(b"\n");
    mac.update(nonce.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right.iter())
            .fold(0_u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

// --- control usage helpers ---

pub async fn refresh_share_usage_items(
    state: &ServerState,
    share: &Share,
    app: Option<&str>,
    providers: &crate::domain::providers::store::ProviderStore,
) -> Vec<ControlRefreshShareUsageItem> {
    let requested_app = app
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let mut bindings = if share.bindings.is_empty() {
        vec![ShareBinding {
            app: share.app,
            provider_id: share.provider_id.clone(),
            provider_type: share.provider_type,
        }]
    } else {
        share.bindings.clone()
    };
    bindings.sort_by(|left, right| left.app.as_str().cmp(right.app.as_str()));
    let mut items = Vec::new();
    for binding in bindings.into_iter().filter(|binding| {
        requested_app
            .as_deref()
            .is_none_or(|app| binding.app.as_str() == app)
    }) {
        let provider = providers.providers.iter().find(|provider| {
            provider.app == binding.app && provider.provider.id == binding.provider_id
        });
        let Some(provider) = provider.cloned() else {
            items.push(ControlRefreshShareUsageItem {
                app: binding.app.as_str().to_string(),
                provider_id: Some(binding.provider_id),
                provider_name: None,
                auth_provider: None,
                account_id: None,
                refreshed: false,
                error: Some("provider not found".to_string()),
                message: None,
            });
            continue;
        };
        items.push(refresh_share_usage_item(state, binding.app, &provider).await);
    }
    items
}

pub(crate) async fn refresh_share_usage_item(
    state: &ServerState,
    app: AppKind,
    provider: &StoredProvider,
) -> ControlRefreshShareUsageItem {
    let provider_id = provider.provider.id.clone();
    let provider_name = Some(provider.provider.name.clone());
    let Some(account_provider_type) = managed_account_provider_type(provider) else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider: None,
            account_id: None,
            refreshed: false,
            error: Some("provider_has_no_managed_account".to_string()),
            message: None,
        };
    };
    let Some((_, account_id, expected_generation)) =
        managed_account_binding_with_generation(provider)
    else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider: Some(account_provider_type.as_str().to_string()),
            account_id: None,
            refreshed: false,
            error: Some("account_binding_required".to_string()),
            message: None,
        };
    };
    let mut account = {
        let accounts = state.accounts.read().await;
        authoritative_managed_account(provider, &accounts).cloned()
    };
    let auth_provider = Some(account_provider_type.as_str().to_string());
    let Some(mut active_account) = account.take() else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id.to_string()),
            refreshed: false,
            error: Some("account_not_found".to_string()),
            message: None,
        };
    };
    let account_id = active_account.id.clone();
    let refresh_generation_before_lock = (
        active_account.auth_identity_generation,
        active_account.token_refresh_generation,
    );
    let Some(mut refresh_guard) = state
        .account_refresh_locks
        .try_lock(active_account.provider_type, &active_account.id)
    else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id),
            refreshed: false,
            error: Some("account_refresh_in_progress".to_string()),
            message: None,
        };
    };
    let latest_account = state.find_account_by_id(&active_account.id).await;
    let Some(latest_account) = latest_account.filter(|account| {
        account.provider_type == account_provider_type
            && account.auth_identity_generation == expected_generation
    }) else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id),
            refreshed: false,
            error: Some("account_not_found".to_string()),
            message: None,
        };
    };
    active_account = latest_account;
    let mut native_refresh_attempted = (
        active_account.auth_identity_generation,
        active_account.token_refresh_generation,
    ) != refresh_generation_before_lock;
    if state.credential_persistence_degraded() {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id),
            refreshed: false,
            error: Some("credential_persistence_degraded".to_string()),
            message: None,
        };
    }
    let account_before_refresh = active_account.clone();
    let now = now_ms() as i64;
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;

    if account_needs_native_refresh(&active_account, now) {
        let http_client = state.http_client().await;
        match state
            .execute_native_account_refresh_with_recovery(
                &http_client,
                &active_account,
                now,
                interval_ms,
                &mut refresh_guard,
            )
            .await
        {
            Ok(update) => {
                match state
                    .commit_native_refresh_success(&active_account, update)
                    .await
                {
                    Ok(updated) => {
                        native_refresh_attempted = true;
                        active_account = updated;
                    }
                    Err(error) => {
                        if error.is_superseded() {
                            return ControlRefreshShareUsageItem {
                                app: app.as_str().to_string(),
                                provider_id: Some(provider_id),
                                provider_name,
                                auth_provider,
                                account_id: Some(account_id),
                                refreshed: false,
                                error: Some("account_credentials_changed".to_string()),
                                message: None,
                            };
                        }
                        let persistence_degraded = error.is_persistence_degraded();
                        if persistence_degraded {
                            tracing::error!(
                                account_id = %active_account.id,
                                %error,
                                "control OAuth refresh persistence degraded"
                            );
                        } else {
                            tracing::error!(
                                account_id = %active_account.id,
                                %error,
                                "control OAuth credential state commit failed"
                            );
                        }
                        return ControlRefreshShareUsageItem {
                            app: app.as_str().to_string(),
                            provider_id: Some(provider_id),
                            provider_name,
                            auth_provider,
                            account_id: Some(account_id),
                            refreshed: false,
                            error: Some(
                                if persistence_degraded {
                                    "credential_persistence_degraded"
                                } else {
                                    "credential_commit_failed"
                                }
                                .to_string(),
                            ),
                            message: None,
                        };
                    }
                }
            }
            Err(error) => {
                let public_error = oauth_error_public_message(error.kind).to_string();
                let updated = state
                    .commit_native_refresh_failure(
                        &active_account,
                        error.message.clone(),
                        error.kind,
                        error.immediate_relogin,
                    )
                    .await;
                match updated {
                    Ok(Some(updated)) => {
                        if let Err(sync_error) = state
                            .refresh_account_runtime_metadata_if_changed(
                                &account_before_refresh,
                                &updated,
                            )
                            .await
                        {
                            tracing::warn!(
                                account_id = %active_account.id,
                                error = %sync_error,
                                "control OAuth refresh failure Share descriptor sync remains pending"
                            );
                        }
                    }
                    Ok(None) => {}
                    Err(persist_error) => {
                        tracing::error!(
                            account_id = %active_account.id,
                            error = %persist_error,
                            "persisting control OAuth refresh failure state failed"
                        );
                    }
                }
                return ControlRefreshShareUsageItem {
                    app: app.as_str().to_string(),
                    provider_id: Some(provider_id),
                    provider_name,
                    auth_provider,
                    account_id: Some(account_id),
                    refreshed: false,
                    error: Some(public_error),
                    message: None,
                };
            }
        }
    }

    if state.credential_persistence_degraded() {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id),
            refreshed: false,
            error: Some("credential_persistence_degraded".to_string()),
            message: None,
        };
    }
    let http_client = state.http_client().await;
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    let mut quota_result = refresh_account_quota(
        &http_client,
        &active_account,
        now,
        true,
        interval_ms,
        timeout_ms,
    )
    .await;
    if !native_refresh_attempted
        && quota_result
            .as_ref()
            .is_err_and(|error| error.upstream_status == Some(StatusCode::UNAUTHORIZED.as_u16()))
    {
        match state
            .recover_quota_unauthorized(
                &http_client,
                &active_account,
                now,
                interval_ms,
                &mut refresh_guard,
            )
            .await
        {
            Ok(refreshed) => {
                active_account = refreshed;
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
            Err(crate::state::QuotaUnauthorizedRecoveryError::Unavailable) => {}
            Err(crate::state::QuotaUnauthorizedRecoveryError::Refresh { failure, updated }) => {
                if let Some(updated) = updated {
                    if let Err(error) = state
                        .refresh_account_runtime_metadata_if_changed(
                            &account_before_refresh,
                            &updated,
                        )
                        .await
                    {
                        tracing::warn!(account_id = %updated.id, %error, "control quota auth recovery Share descriptor sync remains pending");
                    }
                }
                return ControlRefreshShareUsageItem {
                    app: app.as_str().to_string(),
                    provider_id: Some(provider_id),
                    provider_name,
                    auth_provider,
                    account_id: Some(account_id),
                    refreshed: false,
                    error: Some(oauth_error_public_message(failure.kind).to_string()),
                    message: None,
                };
            }
            Err(crate::state::QuotaUnauthorizedRecoveryError::Commit(error)) => {
                let error_code = if error.is_superseded() {
                    "account_credentials_changed"
                } else if error.is_persistence_degraded() {
                    "credential_persistence_degraded"
                } else {
                    "credential_commit_failed"
                };
                return ControlRefreshShareUsageItem {
                    app: app.as_str().to_string(),
                    provider_id: Some(provider_id),
                    provider_name,
                    auth_provider,
                    account_id: Some(account_id),
                    refreshed: false,
                    error: Some(error_code.to_string()),
                    message: None,
                };
            }
            Err(crate::state::QuotaUnauthorizedRecoveryError::State(error)) => {
                tracing::warn!(account_id, %error, "control quota auth recovery state update failed");
                return ControlRefreshShareUsageItem {
                    app: app.as_str().to_string(),
                    provider_id: Some(provider_id),
                    provider_name,
                    auth_provider,
                    account_id: Some(account_id),
                    refreshed: false,
                    error: Some("account_state_update_failed".to_string()),
                    message: None,
                };
            }
        }
    }
    match quota_result {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let public_message = redact_account_public_diagnostic(&active_account, &message);
            let (updated, commit_error) =
                match commit_control_quota_update(state, &active_account, update).await {
                    Ok(account) => (Some(account), None),
                    Err(error) => (None, Some(error.to_string())),
                };
            if let Some(ref account) = updated {
                if let Err(error) = state
                    .refresh_account_runtime_metadata_if_changed(&account_before_refresh, account)
                    .await
                {
                    tracing::warn!(
                        account_id = %account.id,
                        %error,
                        "control quota refresh could not persist account runtime metadata change"
                    );
                }
                state.emit_oauth_quota_updated_event(account, true);
            }
            ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(
                    updated
                        .as_ref()
                        .map(|account| account.id.clone())
                        .unwrap_or(account_id),
                ),
                refreshed: updated.is_some(),
                error: commit_error,
                message: updated.map(|_| public_message),
            }
        }
        Ok(QuotaRefreshResult::SkippedCooldown { message, .. }) => {
            let public_error = redact_account_public_diagnostic(&active_account, &message);
            if let Err(error) = state
                .refresh_account_runtime_metadata_if_changed(
                    &account_before_refresh,
                    &active_account,
                )
                .await
            {
                tracing::warn!(
                    account_id = %active_account.id,
                    %error,
                    "control OAuth refresh Share descriptor sync remains pending"
                );
            }
            ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(account_id),
                refreshed: false,
                error: Some(public_error),
                message: None,
            }
        }
        Err(error) => {
            let public_error = redact_account_public_diagnostic(&active_account, &error.message);
            let updated = mark_quota_refresh_error(state, &active_account, &error).await;
            if let Some(updated) = updated {
                if let Err(sync_error) = state
                    .refresh_account_runtime_metadata_if_changed(&account_before_refresh, &updated)
                    .await
                {
                    tracing::warn!(
                        account_id = %active_account.id,
                        error = %sync_error,
                        "control quota refresh failure Share descriptor sync remains pending"
                    );
                }
            }
            ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(account_id),
                refreshed: false,
                error: Some(public_error),
                message: None,
            }
        }
    }
}

pub(crate) async fn mark_quota_refresh_error(
    state: &ServerState,
    expected: &Account,
    error: &QuotaRefreshFailure,
) -> Option<Account> {
    let next_refresh_at = Some(error.next_refresh_at.unwrap_or_else(|| {
        (now_ms() as i64).saturating_add(crate::clients::oauth::quota::QUOTA_FAILURE_COOLDOWN_MS)
    }));
    let mut update = error.partial_update.as_deref().cloned().unwrap_or_default();
    update.quota_next_refresh_at = next_refresh_at;
    update.last_refresh_error = Some(error.message.clone());
    commit_control_quota_update(state, expected, update)
        .await
        .ok()
}

async fn commit_control_quota_update(
    state: &ServerState,
    expected: &Account,
    update: crate::domain::accounts::store::AccountRefreshUpdate,
) -> Result<Account, &'static str> {
    match state
        .commit_account_quota_refresh_update(expected, update)
        .await
    {
        Ok(Ok(account)) => Ok(account),
        Ok(Err(crate::state::AccountQuotaCommitSkip::NotFound)) => Err("account_not_found"),
        Ok(Err(crate::state::AccountQuotaCommitSkip::Stale(current))) => {
            tracing::info!(
                account_id = %current.id,
                provider_type = %current.provider_type.as_str(),
                "discarded control quota response for superseded account credentials"
            );
            Err("account_credentials_changed")
        }
        Err(error) => {
            tracing::warn!(
                account_id = %expected.id,
                provider_type = %expected.provider_type.as_str(),
                %error,
                "control quota response could not be persisted"
            );
            Err("account_update_failed")
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlApplyShareSettingsInput {
    share_id: String,
    patch: ShareSettingsPatch,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ControlPrepareClientSubdomainAdoptionInput {
    takeover_id: String,
    from_subdomain: String,
    to_subdomain: String,
    activate_at_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ControlClientSubdomainAdoptionIdInput {
    takeover_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlClientSubdomainAdoptionResponse {
    ok: bool,
    takeover_id: String,
    from_subdomain: String,
    to_subdomain: String,
    status: crate::domain::settings::config::ClientSubdomainAdoptionStatus,
    activate_at_ms: i64,
    prepared_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    committed_at_ms: Option<i64>,
}

impl From<crate::domain::settings::config::ClientSubdomainAdoption>
    for ControlClientSubdomainAdoptionResponse
{
    fn from(value: crate::domain::settings::config::ClientSubdomainAdoption) -> Self {
        Self {
            ok: true,
            takeover_id: value.takeover_id,
            from_subdomain: value.from_subdomain,
            to_subdomain: value.to_subdomain,
            status: value.status,
            activate_at_ms: value.activate_at_ms,
            prepared_at_ms: value.prepared_at_ms,
            committed_at_ms: value.committed_at_ms,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlAbortClientSubdomainAdoptionResponse {
    ok: bool,
    takeover_id: String,
    aborted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlApplyShareSettingsResponse {
    ok: bool,
    share: ShareDescriptor,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlRefreshShareUsageInput {
    share_id: String,
    #[serde(default)]
    app: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlRefreshShareUsageItem {
    pub app: String,
    pub provider_id: Option<String>,
    pub provider_name: Option<String>,
    pub auth_provider: Option<String>,
    pub account_id: Option<String>,
    pub refreshed: bool,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlRefreshShareUsageResponse {
    ok: bool,
    refreshed: Vec<ControlRefreshShareUsageItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ControlClientLogTailQuery {
    lines: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlClientLogTailResponse {
    ok: bool,
    lines: usize,
    truncated: bool,
    content: String,
}

#[cfg(test)]
mod control_log_tests {
    use super::*;

    #[test]
    fn control_log_tail_redacts_secrets_and_keeps_latest_lines() {
        let oversized = "x".repeat(CONTROL_LOG_MAX_LINE_BYTES + 20);
        let content = format!("first\nauthorization: Bearer secret\n{oversized}\nlast");
        let (content, lines, truncated) = bounded_control_log_content(&content);
        assert_eq!(lines, 4);
        assert!(truncated);
        assert!(!content.contains("secret"));
        assert!(content.contains("Bearer [REDACTED]"));
        assert!(content.ends_with("last"));
        assert!(content.len() <= CONTROL_LOG_MAX_RESPONSE_BYTES);
    }

    #[test]
    fn control_log_response_limit_includes_json_escaping() {
        let quoted = "\"".repeat(CONTROL_LOG_MAX_LINE_BYTES);
        let content = (0..20)
            .map(|index| format!("{index}:{quoted}"))
            .chain(std::iter::once("latest".to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        let (content, lines, truncated) = bounded_control_log_content(&content);
        let response = fit_control_log_response(ControlClientLogTailResponse {
            ok: true,
            lines,
            truncated,
            content,
        });

        assert!(serde_json::to_vec(&response).unwrap().len() <= CONTROL_LOG_MAX_RESPONSE_BYTES);
        assert!(response.truncated);
        assert!(response.content.ends_with("latest"));
        assert_eq!(response.lines, response.content.lines().count());
    }
}
