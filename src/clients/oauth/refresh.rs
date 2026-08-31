use std::collections::HashMap;
use std::sync::{Mutex as StdMutex, OnceLock};

use crate::domain::accounts::managers::{
    AccountRefreshFlightFailure, AccountRefreshFlightFailureDetails, AccountRefreshFlightStage,
    AccountRefreshGuard,
};
use crate::domain::accounts::oauth::{
    build_refresh_request, build_refresh_request_for_token_url, classify_oauth_error,
    merge_account_refresh_raw, oauth_provider_spec, refresh_update_from_token_response,
    token_expires_soon, OAuthErrorClassification, OAuthErrorKind, OAuthHttpRequest,
    OAuthRequestBodyFormat, OAuthTokenResponse,
};
use crate::domain::accounts::store::{Account, AccountRefreshUpdate};
use crate::domain::providers::model::ProviderType;
use sha2::{Digest, Sha256};

const REFRESH_INITIAL_BACKOFF_MS: i64 = 5_000;
const REFRESH_MAX_BACKOFF_MS: i64 = 5 * 60_000;
const MAX_OAUTH_RESPONSE_BODY_BYTES: usize = 1024 * 1024;
const OAUTH_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct AccountRefreshFailure {
    pub status_code: u16,
    pub upstream_status: Option<u16>,
    pub message: String,
    pub kind: OAuthErrorKind,
    pub retryable: bool,
    pub retry_after_ms: Option<i64>,
    pub immediate_relogin: bool,
    /// The token endpoint may have accepted and rotated the refresh token, but no
    /// complete credential receipt was obtained. Reusing the old token is unsafe.
    pub outcome_unknown: bool,
    pub(crate) endpoint_fallback_safe: bool,
}

impl AccountRefreshFailure {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status_code: 400,
            upstream_status: None,
            message: message.into(),
            kind: OAuthErrorKind::Unsupported,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: false,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        }
    }

    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status_code: 502,
            upstream_status: None,
            message: message.into(),
            kind: OAuthErrorKind::Network,
            retryable: true,
            retry_after_ms: None,
            immediate_relogin: false,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        }
    }

    fn request_failed(
        context: impl Into<String>,
        error: crate::infra::http::BoundedResponseBodyError,
    ) -> Self {
        let endpoint_fallback_safe = error.is_connect();
        let outcome_unknown = !endpoint_fallback_safe;
        let (kind, retryable) = match &error {
            crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => {
                (OAuthErrorKind::Parse, false)
            }
            crate::infra::http::BoundedResponseBodyError::Request(_) => {
                (OAuthErrorKind::Network, true)
            }
        };
        Self {
            status_code: 502,
            upstream_status: None,
            message: format!("{}: {error}", context.into()),
            kind,
            retryable,
            retry_after_ms: None,
            immediate_relogin: outcome_unknown,
            outcome_unknown,
            endpoint_fallback_safe,
        }
    }

    pub fn authorization_pending(message: impl Into<String>) -> Self {
        Self {
            status_code: 409,
            upstream_status: None,
            message: message.into(),
            kind: OAuthErrorKind::AuthorizationPending,
            retryable: true,
            retry_after_ms: None,
            immediate_relogin: false,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        }
    }

    fn rate_limited(message: impl Into<String>) -> Self {
        Self {
            status_code: 429,
            upstream_status: None,
            message: message.into(),
            kind: OAuthErrorKind::RateLimited,
            retryable: true,
            retry_after_ms: None,
            immediate_relogin: false,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        }
    }

    pub(crate) fn parse(message: impl Into<String>) -> Self {
        Self {
            status_code: 502,
            upstream_status: None,
            message: message.into(),
            kind: OAuthErrorKind::Parse,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: false,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        }
    }

    fn from_classification(
        upstream_status: Option<u16>,
        classification: OAuthErrorClassification,
        context: impl Into<String>,
    ) -> Self {
        let status_code = refresh_status_code(upstream_status, classification.kind);
        let context = context.into();
        let immediate_relogin = classification.immediate_relogin;
        Self {
            status_code,
            upstream_status,
            message: if context.is_empty() {
                classification.message
            } else {
                format!("{context}: {}", classification.message)
            },
            kind: classification.kind,
            retryable: classification.retryable,
            retry_after_ms: None,
            immediate_relogin,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        }
    }

    fn with_retry_after(mut self, retry_after_ms: Option<i64>) -> Self {
        self.retry_after_ms = retry_after_ms;
        self
    }

    pub(crate) fn outcome_unknown(message: impl Into<String>) -> Self {
        Self {
            status_code: 502,
            upstream_status: None,
            message: message.into(),
            kind: OAuthErrorKind::Unknown,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        }
    }
}

pub fn provider_native_refresh_available(provider_type: ProviderType) -> bool {
    if matches!(
        provider_type,
        ProviderType::KiroOAuth | ProviderType::AmazonQOAuth | ProviderType::QoderCosy
    ) {
        return true;
    }
    oauth_provider_spec(provider_type)
        .is_some_and(|spec| spec.server_native_refresh_enabled() && !spec.token_urls.is_empty())
}

pub fn account_has_refresh_token(account: &Account) -> bool {
    account
        .refresh_token
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

pub fn account_needs_native_refresh(account: &Account, now_ms: i64) -> bool {
    !account.needs_relogin
        && provider_native_refresh_available(account.provider_type)
        && account_has_refresh_token(account)
        && (account
            .access_token
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
            || token_expires_soon(account, now_ms))
}

pub async fn execute_native_account_refresh(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    refresh_guard: &mut AccountRefreshGuard,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    execute_native_account_refresh_with_receipt_hook(
        http,
        account,
        now_ms,
        quota_refresh_interval_ms,
        refresh_guard,
        |_| Ok(()),
    )
    .await
}

pub async fn execute_native_account_refresh_with_receipt_hook<F>(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    refresh_guard: &mut AccountRefreshGuard,
    mut receipt_hook: F,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure>
where
    F: FnMut(&AccountRefreshUpdate) -> Result<(), AccountRefreshFailure>,
{
    if let Some(failure) = refresh_guard.coalesced_native_failure_for(account) {
        return Err(AccountRefreshFailure {
            status_code: failure.status_code,
            upstream_status: failure.upstream_status,
            message: failure.message.clone(),
            kind: failure.kind,
            retryable: failure.retryable,
            retry_after_ms: failure.retry_after_ms,
            immediate_relogin: failure.immediate_relogin,
            outcome_unknown: false,
            endpoint_fallback_safe: false,
        });
    }

    #[cfg(test)]
    if let Some(token_url) = account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("testOAuthTokenUrl"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let token_urls = [token_url];
        let result = execute_native_account_refresh_with_token_urls_and_receipt_hook(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
            Some(&token_urls),
            &mut receipt_hook,
        )
        .await;
        record_refresh_flight_failure(refresh_guard, account, &result);
        return result;
    }
    let result = execute_native_account_refresh_with_token_urls_and_receipt_hook(
        http,
        account,
        now_ms,
        quota_refresh_interval_ms,
        None,
        &mut receipt_hook,
    )
    .await;
    record_refresh_flight_failure(refresh_guard, account, &result);
    result
}

pub(crate) fn record_refresh_flight_failure(
    refresh_guard: &mut AccountRefreshGuard,
    account: &Account,
    result: &Result<AccountRefreshUpdate, AccountRefreshFailure>,
) {
    if let Err(error) = result {
        refresh_guard.record_failure(AccountRefreshFlightFailure::for_account(
            account,
            AccountRefreshFlightStage::NativeRefresh,
            AccountRefreshFlightFailureDetails {
                status_code: error.status_code,
                upstream_status: error.upstream_status,
                message: error.message.clone(),
                public_message: None,
                kind: error.kind,
                retryable: error.retryable,
                retry_after_ms: error.retry_after_ms,
                immediate_relogin: error.immediate_relogin,
            },
        ));
    }
}

#[cfg(test)]
async fn execute_native_account_refresh_with_token_urls(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    token_urls: Option<&[&str]>,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    execute_native_account_refresh_with_token_urls_and_receipt_hook(
        http,
        account,
        now_ms,
        quota_refresh_interval_ms,
        token_urls,
        &mut |_| Ok(()),
    )
    .await
}

async fn execute_native_account_refresh_with_token_urls_and_receipt_hook<F>(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    token_urls: Option<&[&str]>,
    receipt_hook: &mut F,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure>
where
    F: FnMut(&AccountRefreshUpdate) -> Result<(), AccountRefreshFailure>,
{
    let Some(refresh_key) = refresh_lock_key(account) else {
        return execute_native_account_refresh_inner(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
            token_urls,
            receipt_hook,
        )
        .await;
    };

    if let Some(blocked) = refresh_backoff_blocked(&refresh_key, now_ms) {
        return Err(blocked);
    }

    let result = execute_native_account_refresh_inner(
        http,
        account,
        now_ms,
        quota_refresh_interval_ms,
        token_urls,
        receipt_hook,
    )
    .await;
    match &result {
        Ok(_) => {
            clear_refresh_backoff(&refresh_key);
        }
        Err(error) => remember_refresh_failure(&refresh_key, now_ms, error),
    }
    result
}

async fn execute_native_account_refresh_inner<F>(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    token_urls: Option<&[&str]>,
    receipt_hook: &mut F,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure>
where
    F: FnMut(&AccountRefreshUpdate) -> Result<(), AccountRefreshFailure>,
{
    if account.provider_type == ProviderType::KiroOAuth {
        let receipt = crate::clients::oauth::kiro::refresh_kiro_account(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
            receipt_hook,
        )
        .await?;
        return validate_native_account_refresh_receipt(http, account, receipt).await;
    }
    if account.provider_type == ProviderType::AmazonQOAuth {
        let receipt =
            crate::clients::oauth::amazon_q_device::refresh_amazon_q_account(http, account, now_ms)
                .await?;
        receipt_hook(&receipt)?;
        return validate_native_account_refresh_receipt(http, account, receipt).await;
    }
    if account.provider_type == ProviderType::QoderCosy {
        return crate::clients::oauth::qoder::refresh_qoder_account(
            http,
            account,
            now_ms,
            receipt_hook,
        )
        .await;
    }

    let spec = oauth_provider_spec(account.provider_type).ok_or_else(|| {
        AccountRefreshFailure::bad_request(format!(
            "{} does not have an OAuth refresh spec",
            account.provider_type.as_str()
        ))
    })?;
    if !spec.server_native_refresh_enabled() || spec.token_urls.is_empty() {
        return Err(AccountRefreshFailure::bad_request(format!(
            "{} native refresh is not enabled",
            account.provider_type.as_str()
        )));
    }

    let effective_token_urls = match token_urls {
        Some(token_urls) => token_urls
            .iter()
            .map(|url| (*url).to_string())
            .collect::<Vec<_>>(),
        None if account.provider_type == ProviderType::GrokOAuth => vec![
            build_refresh_request(account.provider_type, account)
                .map_err(|error| {
                    AccountRefreshFailure::from_classification(None, error, "OAuth refresh request")
                })?
                .url,
        ],
        None => spec
            .token_urls
            .iter()
            .map(|url| (*url).to_string())
            .collect(),
    };

    let mut last_error = None;
    for token_url in &effective_token_urls {
        let request =
            build_refresh_request_for_token_url(account.provider_type, account, token_url)
                .map_err(|error| {
                    AccountRefreshFailure::from_classification(None, error, "OAuth refresh request")
                })?;
        let response = match execute_oauth_request(http, &request).await {
            Ok(response) => response,
            Err(error) => {
                let failure = AccountRefreshFailure::request_failed(
                    format!("OAuth refresh request failed at {token_url}"),
                    error,
                );
                let try_fallback = oauth_endpoint_fallback_allowed(&failure);
                last_error = Some(failure);
                if try_fallback {
                    continue;
                }
                break;
            }
        };
        if !response.status.is_success() {
            let classified = classify_oauth_error(Some(response.status.as_u16()), &response.body);
            let failure = AccountRefreshFailure::from_classification(
                Some(response.status.as_u16()),
                classified,
                format!("OAuth refresh failed at {token_url}"),
            )
            .with_retry_after(response.retry_after_ms());
            let try_fallback = oauth_endpoint_fallback_allowed(&failure);
            last_error = Some(failure);
            if try_fallback {
                continue;
            }
            break;
        }

        let raw: serde_json::Value = serde_json::from_str(&response.body).map_err(|error| {
            AccountRefreshFailure::outcome_unknown(format!(
                "OAuth refresh response is not valid JSON: {error}"
            ))
        })?;
        let token_response: OAuthTokenResponse =
            serde_json::from_value(raw.clone()).map_err(|error| {
                AccountRefreshFailure::outcome_unknown(format!(
                    "OAuth refresh response is missing token fields: {error}"
                ))
            })?;
        let receipt = refresh_update_from_token_response(
            account.provider_type,
            &token_response,
            raw,
            now_ms,
            quota_refresh_interval_ms,
        );
        receipt_hook(&receipt)?;
        return validate_native_account_refresh_receipt(http, account, receipt).await;
    }

    Err(last_error.unwrap_or_else(|| {
        AccountRefreshFailure::bad_request("OAuth refresh did not produce a request")
    }))
}

pub async fn validate_native_account_refresh_receipt(
    http: &reqwest::Client,
    account: &Account,
    mut update: AccountRefreshUpdate,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    let raw = update.raw.clone().unwrap_or(serde_json::Value::Null);
    if account.provider_type == ProviderType::CodexOAuth {
        let access_token = update
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                AccountRefreshFailure::parse("Codex refresh receipt has no access token")
            })?;
        let verified = crate::clients::oauth::openai_jwks::verify_openai_identity_tokens(
            http,
            update.id_token.as_deref(),
            access_token,
        )
        .await
        .map_err(openai_jwt_refresh_failure)?;
        ensure_openai_refresh_subject_matches(account, &verified.identity)?;
        crate::domain::accounts::oauth::enrich_refresh_update_with_verified_openai_identity(
            &mut update,
            &raw,
            &verified.identity,
            verified.canonical_claims,
        );
    } else if account.provider_type == ProviderType::GrokOAuth {
        if let Some(id_token) = update
            .id_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
        {
            let verified =
                crate::clients::oauth::grok_jwks::verify_grok_id_token(http, id_token, None)
                    .await
                    .map_err(grok_jwt_refresh_failure)?;
            ensure_grok_refresh_subject_matches(account, &verified.identity)?;
            crate::domain::accounts::oauth::enrich_refresh_update_with_verified_grok_identity(
                &mut update,
                &raw,
                &verified.identity,
                verified.canonical_claims,
            );
        } else {
            require_existing_verified_grok_subject(account)?;
        }
    } else if account.provider_type == ProviderType::CursorOAuth {
        let existing_subject =
            crate::domain::accounts::cursor_import::cursor_subject_from_account(account);
        let refreshed_subject =
            crate::domain::accounts::cursor_import::cursor_subject_from_refresh_update(&update);
        match (existing_subject.as_deref(), refreshed_subject.as_deref()) {
            (Some(existing), Some(refreshed)) if existing != refreshed => {
                return Err(cursor_refresh_identity_failure(
                    "Cursor OAuth refresh subject does not match the existing account",
                ));
            }
            (Some(_), None) => {
                return Err(cursor_refresh_identity_failure(
                    "Cursor OAuth refresh did not produce a stable subject",
                ));
            }
            _ => {}
        }
    } else if account.provider_type == ProviderType::KimiCode {
        let access_token = update
            .access_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                AccountRefreshFailure::parse("Kimi refresh receipt has no access token")
            })?;
        let refreshed_user_id =
            crate::domain::kimi_cli::extract_user_id(access_token).ok_or_else(|| {
                AccountRefreshFailure::parse(
                    "Kimi refresh access token has no stable userId identity claim",
                )
            })?;
        let previous_user_id =
            crate::domain::kimi_cli::user_id_from_profile(account.profile.as_ref()).or_else(|| {
                account
                    .access_token
                    .as_deref()
                    .and_then(crate::domain::kimi_cli::extract_user_id)
            });
        if previous_user_id
            .as_deref()
            .is_some_and(|previous| previous != refreshed_user_id)
        {
            return Err(AccountRefreshFailure {
                status_code: 409,
                upstream_status: None,
                message: "Kimi OAuth refresh returned a different user identity; re-login as a new account"
                    .to_string(),
                kind: OAuthErrorKind::InvalidGrant,
                retryable: false,
                retry_after_ms: None,
                immediate_relogin: true,
                outcome_unknown: true,
                endpoint_fallback_safe: false,
            });
        }
        let device =
            crate::domain::kimi_cli::device_identity_from_profile(account.profile.as_ref())
                .ok_or_else(|| {
                    AccountRefreshFailure::parse(
                        "Kimi account is missing its account-scoped device identity",
                    )
                })?;
        crate::domain::kimi_cli::enrich_profile(
            &mut update.profile,
            Some(&refreshed_user_id),
            &device,
        );
    } else if account.provider_type == ProviderType::KiroOAuth {
        let previous = crate::domain::providers::kiro::runtime_identity_from_account(account)
            .map_err(|error| AccountRefreshFailure::parse(error.to_string()))?;
        let candidate = crate::domain::providers::kiro::runtime_identity_from_values(
            update.profile.as_ref(),
            update.raw.as_ref(),
        )
        .map_err(|error| AccountRefreshFailure::parse(error.to_string()))?;
        let previous_profile_is_authoritative =
            previous.profile_arn.as_deref().is_some_and(|arn| {
                !crate::domain::providers::kiro::is_legacy_enterprise_fallback_profile(arn)
            }) && account
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileProvenance"))
                .or_else(|| {
                    account
                        .raw
                        .as_ref()
                        .and_then(|value| value.pointer("/profileProvenance"))
                })
                .and_then(serde_json::Value::as_str)
                .is_none_or(|value| !value.eq_ignore_ascii_case("auth_method_default"));
        if previous_profile_is_authoritative
            && previous
                .profile_arn
                .as_deref()
                .zip(candidate.profile_arn.as_deref())
                .is_some_and(|(previous, candidate)| previous != candidate)
        {
            return Err(AccountRefreshFailure {
                status_code: 409,
                upstream_status: None,
                message: "Kiro OAuth refresh returned a different profile identity; re-login as a new account"
                    .to_string(),
                kind: OAuthErrorKind::InvalidGrant,
                retryable: false,
                retry_after_ms: None,
                immediate_relogin: true,
                outcome_unknown: true,
                endpoint_fallback_safe: false,
            });
        }
    } else if account.provider_type == ProviderType::QoderCosy {
        update =
            crate::clients::oauth::qoder::complete_qoder_refresh_receipt(http, account, update)
                .await?;
    }
    if let Some(raw) = update.raw.take() {
        update.raw = Some(merge_account_refresh_raw(account.raw.as_ref(), raw));
    }
    if crate::domain::accounts::store::account_refresh_replaces_auth_identity(account, &update) {
        return Err(AccountRefreshFailure {
            status_code: 409,
            upstream_status: None,
            message: format!(
                "{} OAuth refresh returned a different subscription identity; re-login as a new account",
                account.provider_type.as_str()
            ),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        });
    }
    Ok(update)
}

fn cursor_refresh_identity_failure(message: &str) -> AccountRefreshFailure {
    AccountRefreshFailure {
        status_code: 409,
        upstream_status: None,
        message: message.to_string(),
        kind: OAuthErrorKind::InvalidGrant,
        retryable: false,
        retry_after_ms: None,
        immediate_relogin: true,
        outcome_unknown: true,
        endpoint_fallback_safe: false,
    }
}

fn openai_jwt_refresh_failure(
    error: crate::clients::oauth::openai_jwks::OpenAiJwtError,
) -> AccountRefreshFailure {
    let retryable = matches!(
        &error,
        crate::clients::oauth::openai_jwks::OpenAiJwtError::Fetch(_)
    );
    AccountRefreshFailure {
        status_code: if retryable { 502 } else { 400 },
        upstream_status: None,
        message: error.to_string(),
        kind: if retryable {
            OAuthErrorKind::Network
        } else {
            OAuthErrorKind::InvalidGrant
        },
        retryable,
        retry_after_ms: None,
        immediate_relogin: !retryable,
        outcome_unknown: !retryable,
        endpoint_fallback_safe: false,
    }
}

fn grok_jwt_refresh_failure(
    error: crate::clients::oauth::grok_jwks::GrokJwtError,
) -> AccountRefreshFailure {
    let retryable = matches!(
        &error,
        crate::clients::oauth::grok_jwks::GrokJwtError::Discovery(_)
            | crate::clients::oauth::grok_jwks::GrokJwtError::Fetch(_)
    );
    AccountRefreshFailure {
        status_code: if retryable { 502 } else { 400 },
        upstream_status: None,
        message: error.to_string(),
        kind: if retryable {
            OAuthErrorKind::Network
        } else {
            OAuthErrorKind::InvalidGrant
        },
        retryable,
        retry_after_ms: None,
        immediate_relogin: !retryable,
        outcome_unknown: !retryable,
        endpoint_fallback_safe: false,
    }
}

fn ensure_openai_refresh_subject_matches(
    account: &Account,
    refreshed_identity: &crate::domain::accounts::oauth::OAuthIdentity,
) -> Result<(), AccountRefreshFailure> {
    let existing_subject = account
        .profile
        .as_ref()
        .and_then(|profile| profile.pointer("/verifiedOpenAiClaims/subject"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|subject| !subject.is_empty());
    let refreshed_subject = refreshed_identity
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .ok_or_else(|| AccountRefreshFailure {
            status_code: 400,
            upstream_status: None,
            message: "OpenAI OAuth refresh did not produce a verified subject".to_string(),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        })?;

    if existing_subject.is_some_and(|subject| subject != refreshed_subject) {
        return Err(AccountRefreshFailure {
            status_code: 400,
            upstream_status: None,
            message: "OpenAI OAuth refresh subject does not match the existing account".to_string(),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        });
    }
    Ok(())
}

fn require_existing_verified_grok_subject(
    account: &Account,
) -> Result<String, AccountRefreshFailure> {
    crate::domain::accounts::store::verified_grok_subject(account).ok_or_else(|| {
        AccountRefreshFailure {
            status_code: 400,
            upstream_status: None,
            message: "Grok OAuth account has no verified subject; sign in again".to_string(),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        }
    })
}

fn ensure_grok_refresh_subject_matches(
    account: &Account,
    refreshed_identity: &crate::domain::accounts::oauth::OAuthIdentity,
) -> Result<(), AccountRefreshFailure> {
    let existing_subject = require_existing_verified_grok_subject(account)?;
    let refreshed_subject = refreshed_identity
        .subject
        .as_deref()
        .map(str::trim)
        .filter(|subject| !subject.is_empty())
        .ok_or_else(|| AccountRefreshFailure {
            status_code: 400,
            upstream_status: None,
            message: "Grok OAuth refresh did not produce a verified subject".to_string(),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        })?;
    if existing_subject != refreshed_subject {
        return Err(AccountRefreshFailure {
            status_code: 409,
            upstream_status: None,
            message: "Grok OAuth refresh subject does not match the existing account".to_string(),
            kind: OAuthErrorKind::InvalidGrant,
            retryable: false,
            retry_after_ms: None,
            immediate_relogin: true,
            outcome_unknown: true,
            endpoint_fallback_safe: false,
        });
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct RefreshBackoffState {
    blocked_until_ms: i64,
    next_delay_ms: i64,
}

fn refresh_backoffs() -> &'static StdMutex<HashMap<String, RefreshBackoffState>> {
    static BACKOFFS: OnceLock<StdMutex<HashMap<String, RefreshBackoffState>>> = OnceLock::new();
    BACKOFFS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn refresh_lock_key(account: &Account) -> Option<String> {
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let mut hasher = Sha256::new();
    hasher.update(account.provider_type.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(account.id.as_bytes());
    hasher.update([0]);
    hasher.update(refresh_token.as_bytes());
    let digest = hasher.finalize();
    Some(format!(
        "{}:{}",
        account.provider_type.as_str(),
        hex_prefix(&digest, 32)
    ))
}

fn hex_prefix(bytes: &[u8], max_chars: usize) -> String {
    let mut output = String::with_capacity(max_chars);
    for byte in bytes {
        if output.len() >= max_chars {
            break;
        }
        output.push_str(&format!("{byte:02x}"));
    }
    output.truncate(max_chars);
    output
}

fn refresh_backoff_blocked(key: &str, now_ms: i64) -> Option<AccountRefreshFailure> {
    let backoffs = refresh_backoffs()
        .lock()
        .expect("refresh backoff registry poisoned");
    let state = backoffs.get(key)?;
    if now_ms < state.blocked_until_ms {
        let retry_after_ms = state.blocked_until_ms.saturating_sub(now_ms);
        return Some(
            AccountRefreshFailure::rate_limited(format!(
                "OAuth refresh temporarily blocked for {retry_after_ms}ms after recent failure"
            ))
            .with_retry_after(Some(retry_after_ms)),
        );
    }
    None
}

fn clear_refresh_backoff(key: &str) {
    refresh_backoffs()
        .lock()
        .expect("refresh backoff registry poisoned")
        .remove(key);
}

fn remember_refresh_failure(key: &str, now_ms: i64, error: &AccountRefreshFailure) {
    if !refresh_failure_should_backoff(error) {
        return;
    }
    let mut backoffs = refresh_backoffs()
        .lock()
        .expect("refresh backoff registry poisoned");
    let previous_delay = backoffs
        .get(key)
        .map(|state| state.next_delay_ms)
        .unwrap_or(REFRESH_INITIAL_BACKOFF_MS);
    let delay = if let Some(retry_after_ms) = error.retry_after_ms {
        retry_after_ms.clamp(1_000, 24 * 60 * 60 * 1_000)
    } else if error.kind == OAuthErrorKind::InvalidGrant {
        REFRESH_MAX_BACKOFF_MS
    } else {
        previous_delay.clamp(REFRESH_INITIAL_BACKOFF_MS, REFRESH_MAX_BACKOFF_MS)
    };
    let next_delay = delay.saturating_mul(2).min(REFRESH_MAX_BACKOFF_MS);
    backoffs.insert(
        key.to_string(),
        RefreshBackoffState {
            blocked_until_ms: now_ms.saturating_add(delay),
            next_delay_ms: next_delay,
        },
    );
}

fn refresh_failure_should_backoff(error: &AccountRefreshFailure) -> bool {
    error.retryable
        || matches!(
            error.kind,
            OAuthErrorKind::InvalidGrant
                | OAuthErrorKind::RateLimited
                | OAuthErrorKind::ExpiredToken
                | OAuthErrorKind::Network
        )
}

pub async fn execute_oauth_token_request(
    http: &reqwest::Client,
    provider_type: ProviderType,
    request: &OAuthHttpRequest,
    context: impl Into<String>,
) -> Result<(OAuthTokenResponse, serde_json::Value), AccountRefreshFailure> {
    let context = context.into();
    let requests = oauth_token_request_fallbacks(provider_type, request);
    let mut last_error = None;
    for request in requests {
        match execute_single_oauth_token_request(http, provider_type, &request, &context).await {
            Ok(response) => return Ok(response),
            Err(error) if error.kind == OAuthErrorKind::AuthorizationPending => return Err(error),
            Err(error) => {
                let try_fallback = oauth_endpoint_fallback_allowed(&error);
                last_error = Some(error);
                if !try_fallback {
                    break;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        AccountRefreshFailure::bad_request(format!(
            "{context}: OAuth token exchange did not produce a request"
        ))
    }))
}

pub async fn execute_oauth_json_request(
    http: &reqwest::Client,
    provider_type: ProviderType,
    request: &OAuthHttpRequest,
    context: impl Into<String>,
) -> Result<serde_json::Value, AccountRefreshFailure> {
    let context = context.into();
    let response = execute_oauth_request(http, request)
        .await
        .map_err(|error| AccountRefreshFailure::request_failed(context.clone(), error))?;
    if !response.status.is_success() {
        let classified = classify_oauth_error(Some(response.status.as_u16()), &response.body);
        return Err(AccountRefreshFailure::from_classification(
            Some(response.status.as_u16()),
            classified,
            format!("{context} failed at {}", request.url),
        )
        .with_retry_after(response.retry_after_ms()));
    }
    serde_json::from_str(&response.body).map_err(|error| {
        AccountRefreshFailure::parse(format!(
            "{context} response is not valid JSON for {}: {error}",
            provider_type.as_str()
        ))
    })
}

async fn execute_single_oauth_token_request(
    http: &reqwest::Client,
    provider_type: ProviderType,
    request: &OAuthHttpRequest,
    context: &str,
) -> Result<(OAuthTokenResponse, serde_json::Value), AccountRefreshFailure> {
    let response = execute_oauth_request(http, request)
        .await
        .map_err(|error| AccountRefreshFailure::request_failed(context.to_string(), error))?;
    if cursor_login_is_pending(provider_type, response.status, &response.body) {
        return Err(AccountRefreshFailure::authorization_pending(
            "cursor oauth authorization is still pending",
        ));
    }
    if !response.status.is_success() {
        let classified = classify_oauth_error(Some(response.status.as_u16()), &response.body);
        return Err(AccountRefreshFailure::from_classification(
            Some(response.status.as_u16()),
            classified,
            format!("{context} failed at {}", request.url),
        )
        .with_retry_after(response.retry_after_ms()));
    }

    let raw: serde_json::Value = serde_json::from_str(&response.body).map_err(|error| {
        AccountRefreshFailure::parse(format!("{context} response is not valid JSON: {error}"))
    })?;
    let token_response: OAuthTokenResponse =
        serde_json::from_value(raw.clone()).map_err(|error| {
            AccountRefreshFailure::parse(format!(
                "{context} response is missing token fields for {}: {error}",
                provider_type.as_str()
            ))
        })?;
    Ok((token_response, raw))
}

fn oauth_token_request_fallbacks(
    provider_type: ProviderType,
    request: &OAuthHttpRequest,
) -> Vec<OAuthHttpRequest> {
    if provider_type == ProviderType::ClaudeOAuth {
        let redirect_uri = request
            .body
            .get("redirect_uri")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        let ordered =
            crate::domain::accounts::oauth::claude_oauth_token_urls_for_redirect(redirect_uri);
        let mut requests: Vec<OAuthHttpRequest> = Vec::new();
        for token_url in ordered {
            if requests.iter().any(|item| item.url == *token_url) {
                continue;
            }
            let mut next = if request.url == *token_url {
                request.clone()
            } else {
                let mut cloned = request.clone();
                cloned.url = (*token_url).to_string();
                cloned
            };
            set_claude_oauth_user_agent(&mut next, token_url);
            requests.push(next);
        }
        if requests.is_empty() {
            requests.push(request.clone());
        }
        return requests;
    }

    let mut requests = vec![request.clone()];
    if let Some(spec) = oauth_provider_spec(provider_type) {
        if !spec
            .token_urls
            .iter()
            .any(|token_url| request.url == *token_url)
        {
            return requests;
        }
        for token_url in spec.token_urls {
            if *token_url != request.url && !requests.iter().any(|item| item.url == *token_url) {
                let mut next = request.clone();
                next.url = (*token_url).to_string();
                requests.push(next);
            }
        }
    }
    requests
}

fn set_claude_oauth_user_agent(request: &mut OAuthHttpRequest, token_url: &str) {
    let user_agent =
        crate::domain::accounts::oauth::claude_oauth_user_agent_for_token_url(token_url);
    if let Some(entry) = request
        .headers
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
    {
        entry.1 = user_agent.to_string();
    } else {
        request
            .headers
            .push(("User-Agent".to_string(), user_agent.to_string()));
    }
}

fn cursor_login_is_pending(
    provider_type: ProviderType,
    status: reqwest::StatusCode,
    body: &str,
) -> bool {
    provider_type == ProviderType::CursorOAuth
        && (status == reqwest::StatusCode::ACCEPTED
            || status == reqwest::StatusCode::NOT_FOUND
            || (status.is_success() && body.trim().is_empty()))
}

async fn execute_oauth_request(
    http: &reqwest::Client,
    request: &OAuthHttpRequest,
) -> Result<OAuthHttpResponse, crate::infra::http::BoundedResponseBodyError> {
    let method = match request.method {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        _ => reqwest::Method::POST,
    };
    let mut builder = http
        .request(method, &request.url)
        .timeout(OAUTH_REQUEST_TIMEOUT);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if request.method != "GET" {
        builder = match request.body_format {
            OAuthRequestBodyFormat::Form => builder.form(&oauth_form_pairs(&request.body)),
            OAuthRequestBodyFormat::Json => builder.json(&request.body),
        };
    }
    let mut response = builder.send().await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_OAUTH_RESPONSE_BODY_BYTES,
    )
    .await?;
    let body = String::from_utf8_lossy(&body).into_owned();
    Ok(OAuthHttpResponse {
        status,
        headers,
        body,
    })
}

struct OAuthHttpResponse {
    status: reqwest::StatusCode,
    headers: reqwest::header::HeaderMap,
    body: String,
}

impl OAuthHttpResponse {
    fn retry_after_ms(&self) -> Option<i64> {
        parse_retry_after_ms(&self.headers)
    }
}

fn oauth_endpoint_fallback_allowed(error: &AccountRefreshFailure) -> bool {
    if matches!(
        error.kind,
        OAuthErrorKind::InvalidGrant
            | OAuthErrorKind::AccessDenied
            | OAuthErrorKind::RateLimited
            | OAuthErrorKind::ExpiredToken
            | OAuthErrorKind::AuthorizationPending
    ) {
        return false;
    }
    if error
        .upstream_status
        .and_then(|status| reqwest::StatusCode::from_u16(status).ok())
        .is_some_and(|status| {
            matches!(
                status,
                reqwest::StatusCode::NOT_FOUND
                    | reqwest::StatusCode::METHOD_NOT_ALLOWED
                    | reqwest::StatusCode::GONE
                    | reqwest::StatusCode::NOT_IMPLEMENTED
                    | reqwest::StatusCode::BAD_GATEWAY
                    | reqwest::StatusCode::SERVICE_UNAVAILABLE
                    | reqwest::StatusCode::GATEWAY_TIMEOUT
            )
        })
    {
        return true;
    }
    error.kind == OAuthErrorKind::Network && error.endpoint_fallback_safe
}

fn parse_retry_after_ms(headers: &reqwest::header::HeaderMap) -> Option<i64> {
    const MAX_RETRY_AFTER_MS: i64 = 24 * 60 * 60 * 1_000;
    if let Some(value) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
    {
        return Some(value.clamp(0, MAX_RETRY_AFTER_MS));
    }
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(seconds.saturating_mul(1_000).clamp(0, MAX_RETRY_AFTER_MS));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .as_millis()
            .min(MAX_RETRY_AFTER_MS as u128) as i64,
    )
}

fn oauth_form_pairs(value: &serde_json::Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(_, item)| !item.is_null())
                .map(|(key, item)| (key.clone(), oauth_value_to_string(item)))
                .collect()
        })
        .unwrap_or_default()
}

fn oauth_value_to_string(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| value.to_string())
}

fn refresh_status_code(upstream_status: Option<u16>, kind: OAuthErrorKind) -> u16 {
    match kind {
        OAuthErrorKind::MissingCredential
        | OAuthErrorKind::Unsupported
        | OAuthErrorKind::InvalidGrant
        | OAuthErrorKind::ExpiredToken => 400,
        OAuthErrorKind::AccessDenied => 403,
        OAuthErrorKind::RateLimited => 429,
        OAuthErrorKind::ProviderRejected | OAuthErrorKind::Network | OAuthErrorKind::Parse => 502,
        OAuthErrorKind::AuthorizationPending => 409,
        OAuthErrorKind::Unknown => upstream_status
            .filter(|status| (400..500).contains(status))
            .unwrap_or(502),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::routing::{get, post};
    use axum::{Json, Router};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::domain::accounts::managers::AccountRefreshLocks;

    fn request(url: &str) -> OAuthHttpRequest {
        OAuthHttpRequest {
            method: "POST",
            url: url.to_string(),
            headers: Vec::new(),
            body: serde_json::Value::Null,
            body_format: OAuthRequestBodyFormat::Json,
        }
    }

    fn account(
        provider_type: ProviderType,
        access_token: Option<&str>,
        refresh_token: Option<&str>,
        expires_at: Option<i64>,
    ) -> Account {
        Account {
            id: "acct-1".to_string(),
            provider_type,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            email: Some("test@example.com".to_string()),
            access_token: access_token.map(str::to_string),
            refresh_token: refresh_token.map(str::to_string),
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capacity_pool_limits: Default::default(),
            capability_observations: Default::default(),
        }
    }

    #[tokio::test]
    async fn cursor_refresh_receipt_rejects_subject_switch() {
        let mut existing = account(
            ProviderType::CursorOAuth,
            Some("old-access-token"),
            Some("refresh-token"),
            None,
        );
        existing.profile = Some(serde_json::json!({
            "cursorIdentity": {"subject": "cursor-subject-a", "source": "oauth_login"}
        }));
        let update = AccountRefreshUpdate {
            access_token: Some("new-access-token".to_string()),
            raw: Some(serde_json::json!({
                "accessToken": "cursor-subject-b::new-access-token"
            })),
            ..Default::default()
        };
        let error =
            validate_native_account_refresh_receipt(&reqwest::Client::new(), &existing, update)
                .await
                .unwrap_err();
        assert_eq!(error.status_code, 409);
        assert!(error.immediate_relogin);
        assert!(error.message.contains("does not match"));
    }

    #[tokio::test]
    async fn oauth_response_body_limit_is_enforced_before_json_parsing() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_OAUTH_RESPONSE_BODY_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let error = execute_oauth_token_request(
            &reqwest::Client::new(),
            ProviderType::GrokOAuth,
            &request(&format!("http://{address}/token")),
            "Grok OAuth test",
        )
        .await
        .unwrap_err();
        assert_eq!(error.status_code, 502);
        assert_eq!(error.kind, OAuthErrorKind::Parse);
        assert!(!error.retryable);
        assert!(error.message.contains("response body exceeds"));
        assert!(!error.endpoint_fallback_safe);
        server.await.unwrap();
    }

    #[test]
    fn native_refresh_decision_requires_refresh_token_and_expired_or_missing_access() {
        let now_ms = 1_000_000;

        assert!(account_needs_native_refresh(
            &account(ProviderType::CodexOAuth, None, Some("refresh"), None),
            now_ms
        ));
        assert!(account_needs_native_refresh(
            &account(
                ProviderType::CodexOAuth,
                Some("access"),
                Some("refresh"),
                Some(now_ms + 1_000)
            ),
            now_ms
        ));
        assert!(!account_needs_native_refresh(
            &account(
                ProviderType::CodexOAuth,
                Some("access"),
                Some("refresh"),
                Some(now_ms + 3_600_000)
            ),
            now_ms
        ));
        assert!(!account_needs_native_refresh(
            &account(ProviderType::CodexOAuth, None, None, None),
            now_ms
        ));
        assert!(!account_needs_native_refresh(
            &account(ProviderType::Codex, None, Some("refresh"), None),
            now_ms
        ));
        let mut relogin = account(ProviderType::CodexOAuth, None, Some("refresh"), None);
        relogin.needs_relogin = true;
        assert!(!account_needs_native_refresh(&relogin, now_ms));
    }

    #[test]
    fn refresh_singleflight_key_is_scoped_to_the_account_record() {
        let first = account(
            ProviderType::ClaudeOAuth,
            Some("access"),
            Some("shared-refresh-token"),
            Some(1),
        );
        let mut second = first.clone();
        second.id = "acct-2".to_string();

        assert_ne!(refresh_lock_key(&first), refresh_lock_key(&second));
        assert_eq!(refresh_lock_key(&first), refresh_lock_key(&first.clone()));
    }

    #[test]
    fn openai_refresh_rejects_verified_subject_switch_but_allows_legacy_migration() {
        let mut existing = account(
            ProviderType::CodexOAuth,
            Some("access"),
            Some("refresh"),
            None,
        );
        existing.profile = Some(serde_json::json!({
            "verifiedOpenAiClaims": {"subject": "subject-a"}
        }));
        let matching = crate::domain::accounts::oauth::OAuthIdentity {
            subject: Some("subject-a".to_string()),
            ..Default::default()
        };
        ensure_openai_refresh_subject_matches(&existing, &matching).unwrap();

        let switched = crate::domain::accounts::oauth::OAuthIdentity {
            subject: Some("subject-b".to_string()),
            ..Default::default()
        };
        let error = ensure_openai_refresh_subject_matches(&existing, &switched).unwrap_err();
        assert_eq!(error.kind, OAuthErrorKind::InvalidGrant);
        assert!(!error.retryable);

        existing.profile = None;
        ensure_openai_refresh_subject_matches(&existing, &switched).unwrap();
    }

    #[test]
    fn grok_refresh_requires_and_preserves_the_verified_subject() {
        let mut existing = account(
            ProviderType::GrokOAuth,
            Some("access"),
            Some("refresh"),
            None,
        );
        assert!(require_existing_verified_grok_subject(&existing).is_err());

        existing.profile = Some(serde_json::json!({
            "verifiedGrokClaims": {"subject": "subject-a"}
        }));
        assert_eq!(
            require_existing_verified_grok_subject(&existing).unwrap(),
            "subject-a"
        );
        let matching = crate::domain::accounts::oauth::OAuthIdentity {
            subject: Some("subject-a".to_string()),
            ..Default::default()
        };
        ensure_grok_refresh_subject_matches(&existing, &matching).unwrap();

        let switched = crate::domain::accounts::oauth::OAuthIdentity {
            subject: Some("subject-b".to_string()),
            ..Default::default()
        };
        let error = ensure_grok_refresh_subject_matches(&existing, &switched).unwrap_err();
        assert_eq!(error.status_code, 409);
        assert_eq!(error.kind, OAuthErrorKind::InvalidGrant);
        assert!(!error.retryable);
    }

    #[test]
    fn token_endpoint_requests_get_provider_fallbacks() {
        let requests = oauth_token_request_fallbacks(
            ProviderType::ClaudeOAuth,
            &request("https://api.anthropic.com/v1/oauth/token"),
        );

        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].url,
            "https://platform.claude.com/v1/oauth/token"
        );
        assert_eq!(requests[1].url, "https://api.anthropic.com/v1/oauth/token");
    }

    #[test]
    fn web_paste_token_endpoint_requests_prefer_platform_first() {
        let mut request = request("https://platform.claude.com/v1/oauth/token");
        request.body["redirect_uri"] = serde_json::Value::String(
            crate::domain::accounts::oauth::CLAUDE_WEB_PASTE_REDIRECT_URI.to_string(),
        );
        let requests = oauth_token_request_fallbacks(ProviderType::ClaudeOAuth, &request);

        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].url,
            "https://platform.claude.com/v1/oauth/token"
        );
        assert_eq!(requests[1].url, "https://api.anthropic.com/v1/oauth/token");
        assert!(requests[0]
            .headers
            .iter()
            .any(|(name, value)| name == "User-Agent" && value == "axios/1.15.2"));
    }

    #[test]
    fn cursor_poll_request_does_not_fallback_to_refresh_token_endpoint() {
        let requests = oauth_token_request_fallbacks(
            ProviderType::CursorOAuth,
            &request("https://api2.cursor.sh/auth/poll?uuid=session&verifier=secret"),
        );

        assert_eq!(requests.len(), 1);
        assert!(requests[0].url.contains("/auth/poll?"));
    }

    #[tokio::test]
    async fn claude_refresh_rotates_tokens() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_route = Arc::clone(&requests);
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let captured_for_route = Arc::clone(&captured);
        let upstream = Router::new().route(
            "/token",
            post(move |Json(body): Json<serde_json::Value>| {
                let requests = Arc::clone(&requests_for_route);
                let captured = Arc::clone(&captured_for_route);
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    captured.lock().unwrap().push(body);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Json(serde_json::json!({
                        "access_token": "rotated-access-token",
                        "refresh_token": "rotated-refresh-token",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "account": {"uuid": "principal-1"}
                    }))
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired-access-token"),
            Some("original-refresh-token"),
            Some(1),
        );
        account.id = "claude-refresh-rotation".to_string();
        let http = reqwest::Client::new();
        let token_url = format!("http://{address}/token");
        let token_urls = [token_url.as_str()];
        let now_ms = 10_000_000;

        let update = execute_native_account_refresh_with_token_urls(
            &http,
            &account,
            now_ms,
            60_000,
            Some(&token_urls),
        )
        .await
        .unwrap();

        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(update.access_token.as_deref(), Some("rotated-access-token"));
        assert_eq!(
            update.refresh_token.as_deref(),
            Some("rotated-refresh-token")
        );
        assert_eq!(update.token_type.as_deref(), Some("Bearer"));
        assert!(update
            .expires_at
            .is_some_and(|expires_at| expires_at > now_ms));
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0]["refresh_token"],
            serde_json::json!("original-refresh-token")
        );
    }

    #[tokio::test]
    async fn failed_refresh_is_replayed_to_waiters_without_second_request() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_route = Arc::clone(&requests);
        let upstream = Router::new().route(
            "/token",
            post(move || {
                let requests = Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "invalid_grant",
                            "error_description": "shared refresh token was rejected"
                        })),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired-access-token"),
            Some("flight-failure-refresh-token"),
            Some(1),
        );
        account.id = "refresh-flight-failure".to_string();
        account.raw = Some(serde_json::json!({
            "testOAuthTokenUrl": format!("http://{address}/token")
        }));
        let locks = Arc::new(AccountRefreshLocks::default());
        let mut first_guard = locks.lock(account.provider_type, &account.id).await;
        let waiter_started = Arc::new(tokio::sync::Notify::new());
        let waiter_started_signal = waiter_started.notified();
        tokio::pin!(waiter_started_signal);
        let waiter_locks = Arc::clone(&locks);
        let waiter_provider_type = account.provider_type;
        let waiter_account_id = account.id.clone();
        let waiter_started_for_task = Arc::clone(&waiter_started);
        let waiter = tokio::spawn(async move {
            waiter_started_for_task.notify_waiters();
            waiter_locks
                .lock(waiter_provider_type, &waiter_account_id)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), &mut waiter_started_signal)
            .await
            .expect("refresh waiter did not start");

        let first_error = execute_native_account_refresh(
            &reqwest::Client::new(),
            &account,
            1_000_000,
            60_000,
            &mut first_guard,
        )
        .await
        .unwrap_err();
        first_guard.release();
        let mut second_guard = waiter.await.unwrap();
        let second_error = execute_native_account_refresh(
            &reqwest::Client::new(),
            &account,
            1_000_000,
            60_000,
            &mut second_guard,
        )
        .await
        .unwrap_err();

        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(second_error.status_code, first_error.status_code);
        assert_eq!(second_error.upstream_status, first_error.upstream_status);
        assert_eq!(second_error.message, first_error.message);
        assert_eq!(second_error.kind, first_error.kind);
        assert_eq!(second_error.retryable, first_error.retryable);
        assert_eq!(second_error.retry_after_ms, first_error.retry_after_ms);
        assert_eq!(
            second_error.immediate_relogin,
            first_error.immediate_relogin
        );
        server.abort();
    }

    #[tokio::test]
    async fn failed_refresh_is_not_replayed_after_account_credentials_change() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_route = Arc::clone(&requests);
        let upstream = Router::new().route(
            "/token",
            post(move || {
                let requests = Arc::clone(&requests_for_route);
                async move {
                    let attempt = requests.fetch_add(1, Ordering::SeqCst) + 1;
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "invalid_grant",
                            "error_description": format!("refresh attempt {attempt} was rejected")
                        })),
                    )
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired-access-token"),
            Some("old-refresh-token"),
            Some(1),
        );
        account.id = "refresh-flight-generation-change".to_string();
        account.raw = Some(serde_json::json!({
            "testOAuthTokenUrl": format!("http://{address}/token")
        }));
        let locks = Arc::new(AccountRefreshLocks::default());
        let mut leader_guard = locks.lock(account.provider_type, &account.id).await;
        let waiter_started = Arc::new(tokio::sync::Notify::new());
        let waiter_started_signal = waiter_started.notified();
        tokio::pin!(waiter_started_signal);
        let waiter_locks = Arc::clone(&locks);
        let waiter_provider_type = account.provider_type;
        let waiter_account_id = account.id.clone();
        let waiter_started_for_task = Arc::clone(&waiter_started);
        let waiter = tokio::spawn(async move {
            waiter_started_for_task.notify_waiters();
            waiter_locks
                .lock(waiter_provider_type, &waiter_account_id)
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), &mut waiter_started_signal)
            .await
            .expect("refresh generation waiter did not start");

        let first_error = execute_native_account_refresh(
            &reqwest::Client::new(),
            &account,
            1_000_000,
            60_000,
            &mut leader_guard,
        )
        .await
        .unwrap_err();
        account.refresh_token = Some("new-refresh-token".to_string());
        account.token_refresh_generation = account.token_refresh_generation.saturating_add(1);
        leader_guard.release();
        let mut waiter_guard = waiter.await.unwrap();
        let second_error = execute_native_account_refresh(
            &reqwest::Client::new(),
            &account,
            1_000_000,
            60_000,
            &mut waiter_guard,
        )
        .await
        .unwrap_err();

        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_ne!(second_error.message, first_error.message);
        assert!(second_error.message.contains("attempt 2"));
        server.abort();
    }

    #[tokio::test]
    async fn google_refresh_preserves_imported_client_credentials_across_rotations() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let requests_for_route = Arc::clone(&requests);
        let bodies = Arc::new(StdMutex::new(Vec::new()));
        let bodies_for_route = Arc::clone(&bodies);
        let upstream = Router::new().route(
            "/token",
            post(move |body: String| {
                let requests = Arc::clone(&requests_for_route);
                let bodies = Arc::clone(&bodies_for_route);
                async move {
                    let request = requests.fetch_add(1, Ordering::SeqCst) + 1;
                    bodies.lock().unwrap().push(body);
                    Json(serde_json::json!({
                        "access_token": format!("google-access-{request}"),
                        "token_type": "Bearer",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let token_url = format!("http://{address}/token");

        for (index, provider_type) in [ProviderType::GeminiCli, ProviderType::AntigravityOAuth]
            .into_iter()
            .enumerate()
        {
            let mut account = account(
                provider_type,
                Some("expired-google-access"),
                Some("manual-google-refresh"),
                Some(1),
            );
            account.id = format!("google-import-refresh-{index}");
            account.raw = Some(serde_json::json!({
                "clientId": "manual-google-client",
                "clientSecret": "manual-google-secret",
                "testOAuthTokenUrl": token_url.clone(),
                "profile": {"email": "google@example.com"},
                "token": {
                    "access_token": "expired-google-access",
                    "refresh_token": "manual-google-refresh"
                }
            }));
            let locks = AccountRefreshLocks::default();

            for refresh_index in 0..2 {
                let mut guard = locks.lock(provider_type, &account.id).await;
                let update = execute_native_account_refresh(
                    &reqwest::Client::new(),
                    &account,
                    1_000_000 + refresh_index,
                    60_000,
                    &mut guard,
                )
                .await
                .unwrap();
                if let Some(access_token) = update.access_token {
                    account.access_token = Some(access_token);
                }
                if let Some(refresh_token) = update.refresh_token {
                    account.refresh_token = Some(refresh_token);
                }
                if let Some(expires_at) = update.expires_at {
                    account.expires_at = Some(expires_at);
                }
                if let Some(raw) = update.raw {
                    account.raw = Some(raw);
                }
            }

            let raw = account.raw.as_ref().unwrap();
            assert_eq!(raw["clientId"], "manual-google-client");
            assert_eq!(raw["clientSecret"], "manual-google-secret");
            assert_eq!(raw["profile"]["email"], "google@example.com");
            assert!(raw["token"]["access_token"]
                .as_str()
                .is_some_and(|value| value.starts_with("google-access-")));
        }

        assert_eq!(requests.load(Ordering::SeqCst), 4);
        for body in bodies.lock().unwrap().iter() {
            assert!(body.contains("client_id=manual-google-client"));
            assert!(body.contains("client_secret=manual-google-secret"));
            assert!(body.contains("refresh_token=manual-google-refresh"));
        }
        server.abort();
    }

    #[test]
    fn retry_after_headers_are_bounded_and_prefer_milliseconds() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "12".parse().unwrap());
        assert_eq!(parse_retry_after_ms(&headers), Some(12_000));
        headers.insert("retry-after-ms", "2500".parse().unwrap());
        assert_eq!(parse_retry_after_ms(&headers), Some(2_500));
        headers.insert("retry-after-ms", "999999999".parse().unwrap());
        assert_eq!(parse_retry_after_ms(&headers), Some(24 * 60 * 60 * 1_000));
    }

    #[test]
    fn refresh_backoff_reports_the_remaining_retry_after() {
        let key = "refresh-backoff-retry-after";
        clear_refresh_backoff(key);
        let now_ms = 1_000_000;
        let failure =
            AccountRefreshFailure::rate_limited("upstream cooldown").with_retry_after(Some(45_000));
        remember_refresh_failure(key, now_ms, &failure);

        let blocked = refresh_backoff_blocked(key, now_ms + 5_000).unwrap();
        assert_eq!(blocked.retry_after_ms, Some(40_000));
        clear_refresh_backoff(key);
    }

    #[test]
    fn deterministic_oauth_errors_never_enable_endpoint_fallback() {
        for kind in [
            OAuthErrorKind::InvalidGrant,
            OAuthErrorKind::AccessDenied,
            OAuthErrorKind::RateLimited,
            OAuthErrorKind::ExpiredToken,
        ] {
            let error = AccountRefreshFailure {
                status_code: 503,
                upstream_status: Some(503),
                message: "deterministic rejection".to_string(),
                kind,
                retryable: true,
                retry_after_ms: None,
                immediate_relogin: false,
                outcome_unknown: false,
                endpoint_fallback_safe: false,
            };
            assert!(!oauth_endpoint_fallback_allowed(&error), "{kind:?}");
        }
    }

    #[tokio::test]
    async fn deterministic_refresh_rejection_does_not_try_fallback_endpoint() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let fallback_requests = Arc::new(AtomicUsize::new(0));
        let fallback_requests_for_route = Arc::clone(&fallback_requests);
        let upstream = Router::new()
            .route(
                "/invalid",
                post(|| async {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({
                            "error": "invalid_grant",
                            "error_description": "refresh token was rejected"
                        })),
                    )
                }),
            )
            .route(
                "/fallback",
                post(move || {
                    let requests = Arc::clone(&fallback_requests_for_route);
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({
                            "access_token": "must-not-be-used",
                            "refresh_token": "must-not-be-used",
                            "expires_in": 3600
                        }))
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired"),
            Some("deterministic-rejection-refresh"),
            Some(1),
        );
        account.id = "deterministic-no-fallback".to_string();
        let first = format!("http://{address}/invalid");
        let second = format!("http://{address}/fallback");
        let urls = [first.as_str(), second.as_str()];
        let error = execute_native_account_refresh_with_token_urls(
            &reqwest::Client::new(),
            &account,
            1_000_000,
            60_000,
            Some(&urls),
        )
        .await
        .unwrap_err();

        assert_eq!(error.kind, OAuthErrorKind::InvalidGrant);
        assert_eq!(fallback_requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refresh_rate_limit_preserves_retry_after_and_stops_fallback() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let fallback_requests = Arc::new(AtomicUsize::new(0));
        let fallback_requests_for_route = Arc::clone(&fallback_requests);
        let upstream = Router::new()
            .route(
                "/limited",
                post(|| async {
                    (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        [("retry-after-ms", "2500")],
                        Json(serde_json::json!({"error": "rate_limit_error"})),
                    )
                }),
            )
            .route(
                "/fallback",
                post(move || {
                    let requests = Arc::clone(&fallback_requests_for_route);
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({"access_token": "unused"}))
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired"),
            Some("rate-limit-refresh"),
            Some(1),
        );
        account.id = "rate-limit-no-fallback".to_string();
        let first = format!("http://{address}/limited");
        let second = format!("http://{address}/fallback");
        let urls = [first.as_str(), second.as_str()];
        let error = execute_native_account_refresh_with_token_urls(
            &reqwest::Client::new(),
            &account,
            2_000_000,
            60_000,
            Some(&urls),
        )
        .await
        .unwrap_err();

        assert_eq!(error.kind, OAuthErrorKind::RateLimited);
        assert_eq!(error.retry_after_ms, Some(2_500));
        assert_eq!(fallback_requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refresh_response_body_failure_does_not_try_fallback_endpoint() {
        let broken_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let broken_address = broken_listener.local_addr().unwrap();
        let broken_server = tokio::spawn(async move {
            let (mut stream, _) = broken_listener.accept().await.unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await.unwrap();
            let body = r#"{"access_token":"rotated-access","refresh_token":"rotated-refresh""#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n{:X}\r\n{body}\r\nZZ\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        });

        let fallback_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let fallback_address = fallback_listener.local_addr().unwrap();
        let fallback_requests = Arc::new(AtomicUsize::new(0));
        let fallback_requests_for_route = Arc::clone(&fallback_requests);
        let fallback = Router::new().route(
            "/fallback",
            post(move || {
                let requests = Arc::clone(&fallback_requests_for_route);
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "access_token": "must-not-be-used",
                        "refresh_token": "must-not-be-used",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let fallback_server =
            tokio::spawn(async move { axum::serve(fallback_listener, fallback).await.unwrap() });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired"),
            Some("body-failure-refresh-token"),
            Some(1),
        );
        account.id = "body-failure-no-fallback".to_string();
        let first = format!("http://{broken_address}/token");
        let second = format!("http://{fallback_address}/fallback");
        let urls = [first.as_str(), second.as_str()];
        let error = execute_native_account_refresh_with_token_urls(
            &reqwest::Client::new(),
            &account,
            3_000_000,
            60_000,
            Some(&urls),
        )
        .await
        .unwrap_err();

        assert_eq!(error.kind, OAuthErrorKind::Network);
        assert!(!error.endpoint_fallback_safe);
        assert_eq!(fallback_requests.load(Ordering::SeqCst), 0);
        broken_server.await.unwrap();
        fallback_server.abort();
    }

    #[tokio::test]
    async fn refresh_connect_failure_tries_fallback_endpoint() {
        let unavailable_listener =
            tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
        let unavailable_address = unavailable_listener.local_addr().unwrap();
        drop(unavailable_listener);

        let fallback_listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let fallback_address = fallback_listener.local_addr().unwrap();
        let fallback_requests = Arc::new(AtomicUsize::new(0));
        let fallback_requests_for_route = Arc::clone(&fallback_requests);
        let fallback = Router::new().route(
            "/fallback",
            post(move || {
                let requests = Arc::clone(&fallback_requests_for_route);
                async move {
                    requests.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "access_token": "fallback-access",
                        "refresh_token": "fallback-refresh",
                        "expires_in": 3600
                    }))
                }
            }),
        );
        let fallback_server =
            tokio::spawn(async move { axum::serve(fallback_listener, fallback).await.unwrap() });

        let mut account = account(
            ProviderType::ClaudeOAuth,
            Some("expired"),
            Some("connect-failure-refresh-token"),
            Some(1),
        );
        account.id = "connect-failure-allows-fallback".to_string();
        let first = format!("http://{unavailable_address}/token");
        let second = format!("http://{fallback_address}/fallback");
        let urls = [first.as_str(), second.as_str()];
        let update = execute_native_account_refresh_with_token_urls(
            &reqwest::Client::new(),
            &account,
            4_000_000,
            60_000,
            Some(&urls),
        )
        .await
        .unwrap();

        assert_eq!(update.access_token.as_deref(), Some("fallback-access"));
        assert_eq!(fallback_requests.load(Ordering::SeqCst), 1);
        fallback_server.abort();
    }

    #[tokio::test]
    async fn kiro_rotated_token_receipt_precedes_blocked_usage_failure() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let usage_started = Arc::new(tokio::sync::Notify::new());
        let usage_started_for_route = Arc::clone(&usage_started);
        let release_usage = Arc::new(tokio::sync::Notify::new());
        let release_usage_for_route = Arc::clone(&release_usage);
        let usage_requests = Arc::new(AtomicUsize::new(0));
        let usage_requests_for_route = Arc::clone(&usage_requests);
        let upstream = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(serde_json::json!({
                        "accessToken": "kiro-rotated-access",
                        "refreshToken": "kiro-rotated-refresh",
                        "expiresIn": 3600
                    }))
                }),
            )
            .route(
                "/usage",
                get(move || {
                    let usage_started = Arc::clone(&usage_started_for_route);
                    let release_usage = Arc::clone(&release_usage_for_route);
                    let usage_requests = Arc::clone(&usage_requests_for_route);
                    async move {
                        usage_requests.fetch_add(1, Ordering::SeqCst);
                        usage_started.notify_one();
                        release_usage.notified().await;
                        (
                            axum::http::StatusCode::BAD_GATEWAY,
                            Json(serde_json::json!({"error": "usage unavailable"})),
                        )
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::KiroOAuth,
            Some("expired-kiro-access"),
            Some("kiro-old-refresh-blocked-usage"),
            Some(1),
        );
        account.id = "kiro-receipt-before-blocked-usage".to_string();
        account.raw = Some(serde_json::json!({
            "authMethod": "social",
            "apiRegion": "us-east-1",
            "machineId": "kiro-receipt-test-machine",
            "testOAuthTokenUrl": format!("http://{address}/token"),
            "testKiroUsageUrl": format!("http://{address}/usage")
        }));
        let locks = AccountRefreshLocks::default();
        let mut guard = locks.lock(account.provider_type, &account.id).await;
        let receipts = Arc::new(StdMutex::new(Vec::<AccountRefreshUpdate>::new()));
        let receipts_for_hook = Arc::clone(&receipts);
        let http = reqwest::Client::new();
        let refresh = execute_native_account_refresh_with_receipt_hook(
            &http,
            &account,
            7_000_000,
            60_000,
            &mut guard,
            move |update| {
                receipts_for_hook.lock().unwrap().push(update.clone());
                Ok(())
            },
        );
        tokio::pin!(refresh);

        tokio::time::timeout(Duration::from_secs(1), async {
            tokio::select! {
                _ = usage_started.notified() => {}
                result = &mut refresh => panic!("refresh completed before usage blocked: {result:?}"),
            }
        })
        .await
        .expect("Kiro usage request did not start");

        let receipt = receipts
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("rotated token receipt was not recorded before usage");
        assert_eq!(receipt.access_token.as_deref(), Some("kiro-rotated-access"));
        assert_eq!(
            receipt.refresh_token.as_deref(),
            Some("kiro-rotated-refresh")
        );
        assert_eq!(usage_requests.load(Ordering::SeqCst), 1);

        release_usage.notify_one();
        let update = tokio::time::timeout(Duration::from_secs(1), &mut refresh)
            .await
            .expect("Kiro refresh did not finish after failed usage response")
            .unwrap();
        assert_eq!(update.access_token, receipt.access_token);
        assert_eq!(update.refresh_token, receipt.refresh_token);
        assert!(update.quota.is_none());
        server.abort();
    }

    #[tokio::test]
    async fn kiro_receipt_failure_prevents_usage_enrichment_request() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let usage_requests = Arc::new(AtomicUsize::new(0));
        let usage_requests_for_route = Arc::clone(&usage_requests);
        let upstream = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(serde_json::json!({
                        "accessToken": "kiro-unpersisted-access",
                        "refreshToken": "kiro-unpersisted-refresh",
                        "expiresIn": 3600
                    }))
                }),
            )
            .route(
                "/usage",
                get(move || {
                    let usage_requests = Arc::clone(&usage_requests_for_route);
                    async move {
                        usage_requests.fetch_add(1, Ordering::SeqCst);
                        Json(serde_json::json!({}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::KiroOAuth,
            Some("expired-kiro-access"),
            Some("kiro-old-refresh-receipt-failure"),
            Some(1),
        );
        account.id = "kiro-receipt-failure-stops-usage".to_string();
        account.raw = Some(serde_json::json!({
            "authMethod": "social",
            "apiRegion": "us-east-1",
            "machineId": "kiro-receipt-failure-machine",
            "testOAuthTokenUrl": format!("http://{address}/token"),
            "testKiroUsageUrl": format!("http://{address}/usage")
        }));
        let locks = AccountRefreshLocks::default();
        let mut guard = locks.lock(account.provider_type, &account.id).await;
        let receipt_attempts = Arc::new(AtomicUsize::new(0));
        let receipt_attempts_for_hook = Arc::clone(&receipt_attempts);
        let error = execute_native_account_refresh_with_receipt_hook(
            &reqwest::Client::new(),
            &account,
            8_000_000,
            60_000,
            &mut guard,
            move |update| {
                receipt_attempts_for_hook.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    update.access_token.as_deref(),
                    Some("kiro-unpersisted-access")
                );
                assert_eq!(
                    update.refresh_token.as_deref(),
                    Some("kiro-unpersisted-refresh")
                );
                Err(AccountRefreshFailure::bad_gateway(
                    "rotated token receipt persistence failed",
                ))
            },
        )
        .await
        .unwrap_err();

        assert_eq!(error.message, "rotated token receipt persistence failed");
        assert_eq!(receipt_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(usage_requests.load(Ordering::SeqCst), 0);
        server.abort();
    }

    #[tokio::test]
    async fn kiro_legacy_idc_refresh_preserves_rotated_token_and_clears_fake_profile() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new().route(
            "/token",
            post(|| async {
                Json(serde_json::json!({
                    "accessToken": "kiro-idc-rotated-access",
                    "refreshToken": "kiro-idc-rotated-refresh",
                    "expiresIn": 3600
                }))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::KiroOAuth,
            Some("expired-kiro-access"),
            Some("kiro-idc-old-refresh"),
            Some(1),
        );
        account.id = "kiro-legacy-idc-profile-migration".to_string();
        account.profile = Some(serde_json::json!({
            "authMethod": "idc",
            "authRegion": "eu-north-1",
            "runtimeRegion": "eu-central-1",
            "apiRegion": "eu-central-1",
            "profileArn": "arn:aws:codewhisperer:eu-central-1:610548660232:profile/VNECVYCYYAWN",
            "profileProvenance": "auth_method_default"
        }));
        account.raw = Some(serde_json::json!({
            "authMethod": "idc",
            "authRegion": "eu-north-1",
            "resolvedProfileArn": "arn:aws:codewhisperer:eu-central-1:610548660232:profile/VNECVYCYYAWN",
            "profileProvenance": "auth_method_default",
            "testOAuthTokenUrl": format!("http://{address}/token")
        }));
        let locks = AccountRefreshLocks::default();
        let mut guard = locks.lock(account.provider_type, &account.id).await;
        let receipts = Arc::new(StdMutex::new(Vec::<AccountRefreshUpdate>::new()));
        let receipts_for_hook = Arc::clone(&receipts);

        let update = execute_native_account_refresh_with_receipt_hook(
            &reqwest::Client::new(),
            &account,
            8_500_000,
            60_000,
            &mut guard,
            move |update| {
                receipts_for_hook.lock().unwrap().push(update.clone());
                Ok(())
            },
        )
        .await
        .unwrap();

        assert_eq!(receipts.lock().unwrap().len(), 1);
        assert_eq!(
            update.access_token.as_deref(),
            Some("kiro-idc-rotated-access")
        );
        assert_eq!(
            update.refresh_token.as_deref(),
            Some("kiro-idc-rotated-refresh")
        );
        assert_eq!(
            update
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileArn")),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            update
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileProvenance"))
                .and_then(serde_json::Value::as_str),
            Some("profile_resolution_required")
        );
        let candidate = crate::domain::providers::kiro::operational_runtime_identity_from_values(
            update.profile.as_ref(),
            update.raw.as_ref(),
        );
        assert!(candidate.is_err());
        server.abort();
    }

    #[tokio::test]
    async fn kiro_legacy_idc_refresh_records_token_before_real_profile_enrichment() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let discovery_authorization = Arc::new(StdMutex::new(Vec::new()));
        let discovery_authorization_for_route = Arc::clone(&discovery_authorization);
        let upstream = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(serde_json::json!({
                        "accessToken": "kiro-idc-discovery-access",
                        "refreshToken": "kiro-idc-discovery-refresh",
                        "expiresIn": 3600
                    }))
                }),
            )
            .route(
                "/profile",
                get(move |headers: axum::http::HeaderMap| {
                    let observed = Arc::clone(&discovery_authorization_for_route);
                    async move {
                        observed.lock().unwrap().push(
                            headers
                                .get(axum::http::header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default()
                                .to_string(),
                        );
                        Json(serde_json::json!({
                            "profileArn": "arn:aws:codewhisperer:eu-central-1:123456789012:profile/organization-profile"
                        }))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::KiroOAuth,
            Some("expired-kiro-access"),
            Some("kiro-idc-discovery-old-refresh"),
            Some(1),
        );
        account.id = "kiro-legacy-idc-profile-discovery".to_string();
        account.profile = Some(serde_json::json!({
            "authMethod": "idc",
            "authRegion": "eu-north-1",
            "runtimeRegion": "us-east-1",
            "apiRegion": "us-east-1",
            "profileArn": "arn:aws:codewhisperer:eu-central-1:610548660232:profile/VNECVYCYYAWN",
            "profileProvenance": "auth_method_default"
        }));
        account.raw = Some(serde_json::json!({
            "authMethod": "idc",
            "authRegion": "eu-north-1",
            "apiRegion": "us-east-1",
            "testOAuthTokenUrl": format!("http://{address}/token"),
            "testKiroProfileDiscoveryUrl": format!("http://{address}/profile")
        }));
        let locks = AccountRefreshLocks::default();
        let mut guard = locks.lock(account.provider_type, &account.id).await;
        let receipts = Arc::new(StdMutex::new(Vec::<AccountRefreshUpdate>::new()));
        let receipts_for_hook = Arc::clone(&receipts);

        let update = execute_native_account_refresh_with_receipt_hook(
            &reqwest::Client::new(),
            &account,
            8_600_000,
            60_000,
            &mut guard,
            move |update| {
                receipts_for_hook.lock().unwrap().push(update.clone());
                Ok(())
            },
        )
        .await
        .unwrap();

        let receipts = receipts.lock().unwrap();
        assert_eq!(receipts.len(), 2);
        assert_eq!(
            receipts[0]
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileArn")),
            Some(&serde_json::Value::Null)
        );
        assert_eq!(
            receipts[1]
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileArn"))
                .and_then(serde_json::Value::as_str),
            Some("arn:aws:codewhisperer:eu-central-1:123456789012:profile/organization-profile")
        );
        assert!(receipts.iter().all(|receipt| {
            receipt.access_token.as_deref() == Some("kiro-idc-discovery-access")
                && receipt.refresh_token.as_deref() == Some("kiro-idc-discovery-refresh")
        }));
        drop(receipts);
        assert_eq!(
            discovery_authorization.lock().unwrap().as_slice(),
            ["Bearer kiro-idc-discovery-access"]
        );
        assert_eq!(
            update
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/runtimeRegion"))
                .and_then(serde_json::Value::as_str),
            Some("eu-central-1")
        );
        assert_eq!(
            update
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileProvenance"))
                .and_then(serde_json::Value::as_str),
            Some("refresh_list_available_profiles")
        );
        server.abort();
    }

    #[tokio::test]
    async fn kiro_normal_refresh_uses_pending_receipt_identity_validator() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(serde_json::json!({
                        "accessToken": "kiro-replacement-access",
                        "refreshToken": "kiro-replacement-refresh",
                        "expiresIn": 3600,
                        "sub": "kiro-subject-b"
                    }))
                }),
            )
            .route("/usage", get(|| async { Json(serde_json::json!({})) }));
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::KiroOAuth,
            Some("expired-kiro-access"),
            Some("kiro-old-refresh"),
            Some(1),
        );
        account.id = "kiro-refresh-identity-validator".to_string();
        account.profile = Some(serde_json::json!({
            "accountId": "kiro_local_refresh_hash",
            "profileArn": "arn:aws:codewhisperer:us-east-1:699475941385:profile/EHGA3GRVQMUK"
        }));
        account.raw = Some(serde_json::json!({
            "authMethod": "social",
            "apiRegion": "us-east-1",
            "machineId": "kiro-identity-validator-machine",
            "tokenResponse": {"sub": "kiro-subject-a"},
            "testOAuthTokenUrl": format!("http://{address}/token"),
            "testKiroUsageUrl": format!("http://{address}/usage")
        }));
        let locks = AccountRefreshLocks::default();
        let mut guard = locks.lock(account.provider_type, &account.id).await;
        let http = reqwest::Client::new();

        let error = execute_native_account_refresh(&http, &account, 9_000_000, 60_000, &mut guard)
            .await
            .unwrap_err();

        assert_eq!(error.status_code, 409);
        assert!(error.message.contains("different subscription identity"));
        server.abort();
    }

    #[tokio::test]
    async fn kiro_profile_drift_is_rejected_after_rotated_receipt_is_recorded() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = Router::new()
            .route(
                "/token",
                post(|| async {
                    Json(serde_json::json!({
                        "accessToken": "kiro-drift-access",
                        "refreshToken": "kiro-drift-refresh",
                        "expiresIn": 3600,
                        "profileArn": "arn:aws:codewhisperer:eu-central-1:123456789012:profile/replacement"
                    }))
                }),
            )
            .route("/usage", get(|| async { Json(serde_json::json!({})) }));
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut account = account(
            ProviderType::KiroOAuth,
            Some("expired-kiro-access"),
            Some("kiro-old-refresh"),
            Some(1),
        );
        account.id = "kiro-profile-drift".to_string();
        account.profile = Some(serde_json::json!({
            "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/current",
            "authRegion": "eu-north-1",
            "runtimeRegion": "us-east-1",
            "authMethod": "idc"
        }));
        account.raw = Some(serde_json::json!({
            "authMethod": "idc",
            "authRegion": "eu-north-1",
            "clientId": "client",
            "clientSecret": "secret",
            "machineId": "kiro-drift-machine",
            "testOAuthTokenUrl": format!("http://{address}/token"),
            "testKiroUsageUrl": format!("http://{address}/usage")
        }));
        let locks = AccountRefreshLocks::default();
        let mut guard = locks.lock(account.provider_type, &account.id).await;
        let receipts = Arc::new(StdMutex::new(Vec::<AccountRefreshUpdate>::new()));
        let receipts_for_hook = Arc::clone(&receipts);

        let error = execute_native_account_refresh_with_receipt_hook(
            &reqwest::Client::new(),
            &account,
            10_000_000,
            60_000,
            &mut guard,
            move |update| {
                receipts_for_hook.lock().unwrap().push(update.clone());
                Ok(())
            },
        )
        .await
        .unwrap_err();

        let receipt = receipts.lock().unwrap().first().cloned().unwrap();
        assert_eq!(receipt.access_token.as_deref(), Some("kiro-drift-access"));
        assert_eq!(receipt.refresh_token.as_deref(), Some("kiro-drift-refresh"));
        assert_eq!(
            receipt
                .profile
                .as_ref()
                .and_then(|value| value.pointer("/profileArn"))
                .and_then(serde_json::Value::as_str),
            Some("arn:aws:codewhisperer:eu-central-1:123456789012:profile/replacement")
        );
        assert_eq!(error.status_code, 409);
        assert!(error.message.contains("different profile identity"));
        assert!(error.immediate_relogin);
        assert!(error.outcome_unknown);
        server.abort();
    }

    #[tokio::test]
    async fn grok_refresh_accepts_missing_id_token_but_verifies_any_replacement() {
        crate::clients::oauth::grok_jwks::tests::install_test_key().await;
        let signed_id_token = crate::clients::oauth::grok_jwks::tests::signed_token(
            "b1a00492-073a-47ea-816f-4c329264a828",
            "refresh-nonce",
            chrono::Utc::now().timestamp() + 3_600,
        );
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let signed_for_route = signed_id_token.clone();
        let upstream = Router::new()
            .route(
                "/without-id",
                post(|| async {
                    Json(serde_json::json!({
                        "access_token": "grok-access-without-id",
                        "refresh_token": "grok-refresh-without-id",
                        "expires_in": 3600
                    }))
                }),
            )
            .route(
                "/with-id",
                post(move || {
                    let id_token = signed_for_route.clone();
                    async move {
                        Json(serde_json::json!({
                            "access_token": "grok-access-with-id",
                            "refresh_token": "grok-refresh-with-id",
                            "id_token": id_token,
                            "expires_in": 3600
                        }))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });

        let mut existing = account(
            ProviderType::GrokOAuth,
            Some("expired-grok-access"),
            Some("original-grok-refresh"),
            Some(1),
        );
        existing.id = "grok-refresh-without-id-account".to_string();
        existing.profile = Some(serde_json::json!({
            "verifiedGrokClaims": {
                "subject": "xai-user-123",
                "email": "verified@example.com"
            }
        }));
        let without_url = format!("http://{address}/without-id");
        let without_urls = [without_url.as_str()];
        let without_id = execute_native_account_refresh_with_token_urls(
            &reqwest::Client::new(),
            &existing,
            5_000_000,
            60_000,
            Some(&without_urls),
        )
        .await
        .unwrap();
        assert_eq!(
            without_id.access_token.as_deref(),
            Some("grok-access-without-id")
        );
        assert!(without_id.id_token.is_none());
        let mut store = crate::domain::accounts::store::AccountStore {
            accounts: vec![existing.clone()],
            ..Default::default()
        };
        let refreshed = store
            .mark_refresh_success(&existing.id, without_id)
            .unwrap();
        assert_eq!(
            crate::domain::accounts::store::verified_grok_subject(&refreshed).as_deref(),
            Some("xai-user-123")
        );

        existing.id = "grok-refresh-with-id-account".to_string();
        existing.refresh_token = Some("original-grok-refresh-with-id".to_string());
        let with_url = format!("http://{address}/with-id");
        let with_urls = [with_url.as_str()];
        let with_id = execute_native_account_refresh_with_token_urls(
            &reqwest::Client::new(),
            &existing,
            6_000_000,
            60_000,
            Some(&with_urls),
        )
        .await
        .unwrap();
        assert_eq!(with_id.id_token.as_deref(), Some(signed_id_token.as_str()));
        assert_eq!(
            with_id
                .profile
                .as_ref()
                .and_then(|profile| profile.pointer("/verifiedGrokClaims/subject"))
                .and_then(serde_json::Value::as_str),
            Some("xai-user-123")
        );
        server.abort();
    }
}
