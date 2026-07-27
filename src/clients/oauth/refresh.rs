use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, Weak};

use crate::domain::accounts::oauth::{
    build_refresh_request_for_token_url, classify_oauth_error, oauth_provider_spec,
    refresh_update_from_token_response, token_expires_soon, OAuthErrorClassification,
    OAuthErrorKind, OAuthHttpRequest, OAuthRequestBodyFormat, OAuthTokenResponse,
};
use crate::domain::accounts::store::{Account, AccountRefreshUpdate};
use crate::domain::providers::model::ProviderType;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex as AsyncMutex;

const REFRESH_RECENT_SUCCESS_TTL_MS: i64 = 10_000;
const REFRESH_INITIAL_BACKOFF_MS: i64 = 5_000;
const REFRESH_MAX_BACKOFF_MS: i64 = 5 * 60_000;

#[derive(Debug, Clone)]
pub struct AccountRefreshFailure {
    pub status_code: u16,
    pub upstream_status: Option<u16>,
    pub message: String,
    pub kind: OAuthErrorKind,
    pub retryable: bool,
    pub retry_after_ms: Option<i64>,
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
            endpoint_fallback_safe: false,
        }
    }

    fn request_failed(context: impl Into<String>, error: reqwest::Error) -> Self {
        let endpoint_fallback_safe = error.is_connect();
        Self {
            status_code: 502,
            upstream_status: None,
            message: format!("{}: {error}", context.into()),
            kind: OAuthErrorKind::Network,
            retryable: true,
            retry_after_ms: None,
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
            endpoint_fallback_safe: false,
        }
    }

    fn with_retry_after(mut self, retry_after_ms: Option<i64>) -> Self {
        self.retry_after_ms = retry_after_ms;
        self
    }
}

pub fn provider_native_refresh_available(provider_type: ProviderType) -> bool {
    if provider_type == ProviderType::KiroOAuth {
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
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
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
        return execute_native_account_refresh_with_token_urls(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
            Some(&token_urls),
        )
        .await;
    }
    execute_native_account_refresh_with_token_urls(
        http,
        account,
        now_ms,
        quota_refresh_interval_ms,
        None,
    )
    .await
}

async fn execute_native_account_refresh_with_token_urls(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    token_urls: Option<&[&str]>,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    let Some(refresh_key) = refresh_lock_key(account) else {
        return execute_native_account_refresh_inner(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
            token_urls,
        )
        .await;
    };

    if let Some(blocked) = refresh_backoff_blocked(&refresh_key, now_ms) {
        return Err(blocked);
    }

    let lock = refresh_lock(&refresh_key);
    let _guard = lock.lock().await;

    if let Some(update) = recent_refresh_success(&refresh_key, now_ms) {
        return Ok(update);
    }
    if let Some(blocked) = refresh_backoff_blocked(&refresh_key, now_ms) {
        return Err(blocked);
    }

    let result = execute_native_account_refresh_inner(
        http,
        account,
        now_ms,
        quota_refresh_interval_ms,
        token_urls,
    )
    .await;
    match &result {
        Ok(update) => {
            remember_refresh_success(&refresh_key, now_ms, update);
            clear_refresh_backoff(&refresh_key);
        }
        Err(error) => remember_refresh_failure(&refresh_key, now_ms, error),
    }
    result
}

async fn execute_native_account_refresh_inner(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
    token_urls: Option<&[&str]>,
) -> Result<AccountRefreshUpdate, AccountRefreshFailure> {
    if account.provider_type == ProviderType::KiroOAuth {
        return crate::clients::oauth::kiro::refresh_kiro_account(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
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

    let mut last_error = None;
    for token_url in token_urls.unwrap_or(spec.token_urls) {
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
            AccountRefreshFailure::parse(format!(
                "OAuth refresh response is not valid JSON: {error}"
            ))
        })?;
        let token_response: OAuthTokenResponse =
            serde_json::from_value(raw.clone()).map_err(|error| {
                AccountRefreshFailure::parse(format!(
                    "OAuth refresh response is missing token fields: {error}"
                ))
            })?;
        let verified_openai_identity = if account.provider_type == ProviderType::CodexOAuth {
            let verified = crate::clients::oauth::openai_jwks::verify_openai_identity_tokens(
                http,
                token_response.id_token.as_deref(),
                &token_response.access_token,
            )
            .await
            .map_err(|error| AccountRefreshFailure {
                status_code: 400,
                upstream_status: None,
                message: error.to_string(),
                kind: OAuthErrorKind::InvalidGrant,
                retryable: false,
                retry_after_ms: None,
                endpoint_fallback_safe: false,
            })?;
            ensure_openai_refresh_subject_matches(account, &verified.identity)?;
            Some(verified)
        } else {
            None
        };
        let mut update = if let Some(verified) = verified_openai_identity.as_ref() {
            crate::domain::accounts::oauth::refresh_update_from_verified_openai_token_response(
                &token_response,
                raw,
                &verified.identity,
                now_ms,
                quota_refresh_interval_ms,
            )
        } else {
            refresh_update_from_token_response(
                account.provider_type,
                &token_response,
                raw,
                now_ms,
                quota_refresh_interval_ms,
            )
        };

        if let Some(verified) = verified_openai_identity {
            if verified.identity.email.is_some() {
                update.email = verified.identity.email;
            }
            if verified.identity.plan_type.is_some() {
                update.subscription_level = verified.identity.plan_type;
            }
            crate::domain::accounts::store::set_verified_openai_claims(
                &mut update.profile,
                Some(verified.canonical_claims),
            );
        }
        if crate::domain::accounts::store::account_refresh_replaces_auth_identity(account, &update)
        {
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
                endpoint_fallback_safe: false,
            });
        }

        return Ok(update);
    }

    Err(last_error.unwrap_or_else(|| {
        AccountRefreshFailure::bad_request("OAuth refresh did not produce a request")
    }))
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

#[derive(Debug, Clone)]
struct RefreshRecentSuccess {
    completed_at_ms: i64,
    update: AccountRefreshUpdate,
}

fn refresh_locks() -> &'static StdMutex<HashMap<String, Weak<AsyncMutex<()>>>> {
    static LOCKS: OnceLock<StdMutex<HashMap<String, Weak<AsyncMutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn refresh_backoffs() -> &'static StdMutex<HashMap<String, RefreshBackoffState>> {
    static BACKOFFS: OnceLock<StdMutex<HashMap<String, RefreshBackoffState>>> = OnceLock::new();
    BACKOFFS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn refresh_recent_successes() -> &'static StdMutex<HashMap<String, RefreshRecentSuccess>> {
    static SUCCESSES: OnceLock<StdMutex<HashMap<String, RefreshRecentSuccess>>> = OnceLock::new();
    SUCCESSES.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn refresh_lock(key: &str) -> Arc<AsyncMutex<()>> {
    let mut locks = refresh_locks()
        .lock()
        .expect("refresh lock registry poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(AsyncMutex::new(()));
    locks.insert(key.to_string(), Arc::downgrade(&lock));
    lock
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

fn recent_refresh_success(key: &str, now_ms: i64) -> Option<AccountRefreshUpdate> {
    let mut successes = refresh_recent_successes()
        .lock()
        .expect("refresh success registry poisoned");
    successes.retain(|_, success| {
        now_ms.saturating_sub(success.completed_at_ms) <= REFRESH_RECENT_SUCCESS_TTL_MS
    });
    successes.get(key).and_then(|success| {
        (now_ms.saturating_sub(success.completed_at_ms) <= REFRESH_RECENT_SUCCESS_TTL_MS)
            .then(|| success.update.clone())
    })
}

fn remember_refresh_success(key: &str, now_ms: i64, update: &AccountRefreshUpdate) {
    let mut successes = refresh_recent_successes()
        .lock()
        .expect("refresh success registry poisoned");
    successes.insert(
        key.to_string(),
        RefreshRecentSuccess {
            completed_at_ms: now_ms,
            update: update.clone(),
        },
    );
}

fn refresh_backoff_blocked(key: &str, now_ms: i64) -> Option<AccountRefreshFailure> {
    let backoffs = refresh_backoffs()
        .lock()
        .expect("refresh backoff registry poisoned");
    let state = backoffs.get(key)?;
    if now_ms < state.blocked_until_ms {
        let retry_after_ms = state.blocked_until_ms.saturating_sub(now_ms);
        return Some(AccountRefreshFailure::rate_limited(format!(
            "OAuth refresh temporarily blocked for {retry_after_ms}ms after recent failure"
        )));
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
) -> Result<OAuthHttpResponse, reqwest::Error> {
    let method = match request.method {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        _ => reqwest::Method::POST,
    };
    let mut builder = http.request(method, &request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    if request.method != "GET" {
        builder = match request.body_format {
            OAuthRequestBodyFormat::Form => builder.form(&oauth_form_pairs(&request.body)),
            OAuthRequestBodyFormat::Json => builder.json(&request.body),
        };
    }
    let response = builder.send().await?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response.text().await?;
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
    use std::time::Duration;

    use axum::routing::post;
    use axum::{Json, Router};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

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
        }
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
    fn token_endpoint_requests_get_provider_fallbacks() {
        let requests = oauth_token_request_fallbacks(
            ProviderType::ClaudeOAuth,
            &request("https://api.anthropic.com/v1/oauth/token"),
        );

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].url, "https://api.anthropic.com/v1/oauth/token");
        assert_eq!(
            requests[1].url,
            "https://platform.claude.com/v1/oauth/token"
        );
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
            .any(|(name, value)| name == "User-Agent" && value == "axios/1.13.6"));
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
    async fn claude_refresh_rotates_tokens_and_is_singleflight() {
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
        account.id = "claude-refresh-singleflight".to_string();
        let http = reqwest::Client::new();
        let token_url = format!("http://{address}/token");
        let token_urls = [token_url.as_str()];
        let now_ms = 10_000_000;

        let (first, second) = tokio::join!(
            execute_native_account_refresh_with_token_urls(
                &http,
                &account,
                now_ms,
                60_000,
                Some(&token_urls),
            ),
            execute_native_account_refresh_with_token_urls(
                &http,
                &account,
                now_ms,
                60_000,
                Some(&token_urls),
            )
        );
        let first = first.unwrap();
        let second = second.unwrap();

        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(first.access_token.as_deref(), Some("rotated-access-token"));
        assert_eq!(
            first.refresh_token.as_deref(),
            Some("rotated-refresh-token")
        );
        assert_eq!(first.token_type.as_deref(), Some("Bearer"));
        assert!(first
            .expires_at
            .is_some_and(|expires_at| expires_at > now_ms));
        assert_eq!(second.access_token, first.access_token);
        assert_eq!(second.refresh_token, first.refresh_token);
        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(
            captured[0]["refresh_token"],
            serde_json::json!("original-refresh-token")
        );
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
}
