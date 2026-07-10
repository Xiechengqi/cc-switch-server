use super::*;

type HmacSha256 = Hmac<Sha256>;

const CONTROL_SIGNATURE_WINDOW_MS: i64 = 5 * 60 * 1000;
pub(crate) async fn control_apply_share_settings(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ControlApplyShareSettingsResponse>, ApiError> {
    verify_control_request(&state, APPLY_SHARE_SETTINGS_PATH, &headers, &body).await?;
    let input: ControlApplyShareSettingsInput =
        serde_json::from_slice(&body).map_err(ApiError::bad_request)?;
    let share = state
        .mutate_shares_immediate(|shares| {
            shares
                .apply_settings_patch(&input.share_id, input.patch)
                .map_err(|error| match error {
                    crate::domain::sharing::shares::SharePatchError::NotFound => {
                        ApiError::not_found("share not found")
                    }
                    crate::domain::sharing::shares::SharePatchError::Invalid(message) => {
                        ApiError::bad_request(message)
                    }
                })
        })
        .await
        .map_err(ApiError::internal)??;
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

pub(crate) async fn verify_control_request(
    state: &ServerState,
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
    let expected = control_signature(path, secret, body, timestamp_ms, nonce)?;
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
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|_| ApiError::unauthorized("bad control secret"))?;
    mac.update(b"POST\n");
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
    let account_id_hint = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    let mut account = {
        let accounts = state.accounts.read().await;
        accounts
            .find_for_provider(provider.provider_type, account_id_hint)
            .cloned()
    };
    let provider_id = provider.provider.id.clone();
    let provider_name = Some(provider.provider.name.clone());
    let auth_provider = Some(provider.provider_type_id.clone());
    let Some(mut active_account) = account.take() else {
        return ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: account_id_hint.map(str::to_string),
            refreshed: false,
            error: Some("account_not_found".to_string()),
            message: None,
        };
    };
    let account_id = active_account.id.clone();
    let now = now_ms() as i64;
    let interval_ms = state.oauth_quota_refresh_interval_ms().await;

    if account_needs_native_refresh(&active_account, now) {
        let Some(_refresh_guard) = state
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
        let latest_account = state
            .find_account_for_provider(provider.provider_type, Some(&active_account.id))
            .await;
        let Some(latest_account) = latest_account else {
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
        if account_needs_native_refresh(&active_account, now) {
            let http_client = state.http_client().await;
            match execute_native_account_refresh(&http_client, &active_account, now, interval_ms)
                .await
            {
                Ok(update) => {
                    let updated = state
                        .mutate_accounts_debounced(|accounts| {
                            accounts.mark_native_refresh_success(&active_account.id, update)
                        })
                        .await;
                    if let Some(updated) = updated {
                        active_account = updated;
                    }
                }
                Err(error) => {
                    state
                        .mutate_accounts_debounced(|accounts| {
                            accounts.mark_native_refresh_failure(
                                &active_account.id,
                                error.message.clone(),
                                error.kind,
                            );
                        })
                        .await;
                    return ControlRefreshShareUsageItem {
                        app: app.as_str().to_string(),
                        provider_id: Some(provider_id),
                        provider_name,
                        auth_provider,
                        account_id: Some(account_id),
                        refreshed: false,
                        error: Some(error.message),
                        message: None,
                    };
                }
            }
        }
    }

    let http_client = state.http_client().await;
    let timeout_ms = state.oauth_quota_refresh_timeout_ms().await;
    match refresh_account_quota(
        &http_client,
        &active_account,
        now,
        true,
        interval_ms,
        timeout_ms,
    )
    .await
    {
        Ok(QuotaRefreshResult::Updated { update, message }) => {
            let updated = state
                .mutate_accounts_debounced(|accounts| {
                    accounts.mark_refresh_success(&active_account.id, update)
                })
                .await;
            if let Some(ref account) = updated {
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
                error: updated.is_none().then(|| "account_not_found".to_string()),
                message: updated.map(|_| message),
            }
        }
        Ok(QuotaRefreshResult::SkippedCooldown { message, .. }) => ControlRefreshShareUsageItem {
            app: app.as_str().to_string(),
            provider_id: Some(provider_id),
            provider_name,
            auth_provider,
            account_id: Some(account_id),
            refreshed: false,
            error: Some(message),
            message: None,
        },
        Err(error) => {
            mark_quota_refresh_error(state, &active_account.id, &error).await;
            ControlRefreshShareUsageItem {
                app: app.as_str().to_string(),
                provider_id: Some(provider_id),
                provider_name,
                auth_provider,
                account_id: Some(account_id),
                refreshed: false,
                error: Some(error.message),
                message: None,
            }
        }
    }
}

pub(crate) async fn mark_quota_refresh_error(
    state: &ServerState,
    account_id: &str,
    error: &QuotaRefreshFailure,
) {
    state
        .mutate_accounts_debounced(|accounts| {
            accounts.mark_refresh_success(
                account_id,
                AccountRefreshUpdate {
                    quota_next_refresh_at: error.next_refresh_at,
                    last_refresh_error: Some(error.message.clone()),
                    ..Default::default()
                },
            );
        })
        .await;
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlApplyShareSettingsInput {
    share_id: String,
    patch: ShareSettingsPatch,
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
