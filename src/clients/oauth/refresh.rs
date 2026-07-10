use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use crate::domain::accounts::oauth::{
    build_profile_request, build_refresh_request_for_token_url, classify_oauth_error,
    merge_refresh_updates, oauth_provider_spec, refresh_update_from_profile_response,
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
    pub message: String,
    pub kind: OAuthErrorKind,
    pub retryable: bool,
}

impl AccountRefreshFailure {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status_code: 400,
            message: message.into(),
            kind: OAuthErrorKind::Unsupported,
            retryable: false,
        }
    }

    pub fn bad_gateway(message: impl Into<String>) -> Self {
        Self {
            status_code: 502,
            message: message.into(),
            kind: OAuthErrorKind::Network,
            retryable: true,
        }
    }

    pub fn authorization_pending(message: impl Into<String>) -> Self {
        Self {
            status_code: 409,
            message: message.into(),
            kind: OAuthErrorKind::AuthorizationPending,
            retryable: true,
        }
    }

    fn rate_limited(message: impl Into<String>) -> Self {
        Self {
            status_code: 429,
            message: message.into(),
            kind: OAuthErrorKind::RateLimited,
            retryable: true,
        }
    }

    pub(crate) fn parse(message: impl Into<String>) -> Self {
        Self {
            status_code: 502,
            message: message.into(),
            kind: OAuthErrorKind::Parse,
            retryable: false,
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
            message: if context.is_empty() {
                classification.message
            } else {
                format!("{context}: {}", classification.message)
            },
            kind: classification.kind,
            retryable: classification.retryable,
        }
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
    provider_native_refresh_available(account.provider_type)
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
    let Some(refresh_key) = refresh_lock_key(account) else {
        return execute_native_account_refresh_inner(
            http,
            account,
            now_ms,
            quota_refresh_interval_ms,
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

    let result =
        execute_native_account_refresh_inner(http, account, now_ms, quota_refresh_interval_ms)
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
    for token_url in spec.token_urls {
        let request =
            build_refresh_request_for_token_url(account.provider_type, account, token_url)
                .map_err(|error| {
                    AccountRefreshFailure::from_classification(None, error, "OAuth refresh request")
                })?;
        let (status, body) = match execute_oauth_request(http, &request).await {
            Ok(response) => response,
            Err(error) => {
                last_error = Some(AccountRefreshFailure::bad_gateway(format!(
                    "OAuth refresh request failed: {error}"
                )));
                continue;
            }
        };
        if !status.is_success() {
            let classified = classify_oauth_error(Some(status.as_u16()), &body);
            last_error = Some(AccountRefreshFailure::from_classification(
                Some(status.as_u16()),
                classified,
                format!("OAuth refresh failed at {token_url}"),
            ));
            continue;
        }

        let raw: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
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
        let mut update = refresh_update_from_token_response(
            account.provider_type,
            &token_response,
            raw,
            now_ms,
            quota_refresh_interval_ms,
        );

        if let Some(profile_request) =
            build_profile_request(account.provider_type, &token_response.access_token)
        {
            update = merge_refresh_updates(
                update,
                execute_optional_profile_refresh(
                    http,
                    account.provider_type,
                    &profile_request,
                    now_ms,
                    quota_refresh_interval_ms,
                )
                .await,
            );
        }

        return Ok(update);
    }

    Err(last_error.unwrap_or_else(|| {
        AccountRefreshFailure::bad_request("OAuth refresh did not produce a request")
    }))
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

fn refresh_locks() -> &'static StdMutex<HashMap<String, Arc<AsyncMutex<()>>>> {
    static LOCKS: OnceLock<StdMutex<HashMap<String, Arc<AsyncMutex<()>>>>> = OnceLock::new();
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
    locks
        .entry(key.to_string())
        .or_insert_with(|| Arc::new(AsyncMutex::new(())))
        .clone()
}

fn refresh_lock_key(account: &Account) -> Option<String> {
    let refresh_token = account
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let digest = Sha256::digest(refresh_token.as_bytes());
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
    let delay = if error.kind == OAuthErrorKind::InvalidGrant {
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
            Err(error) => last_error = Some(error),
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
    let (status, body) = execute_oauth_request(http, request)
        .await
        .map_err(|error| AccountRefreshFailure::bad_gateway(format!("{context}: {error}")))?;
    if !status.is_success() {
        let classified = classify_oauth_error(Some(status.as_u16()), &body);
        return Err(AccountRefreshFailure::from_classification(
            Some(status.as_u16()),
            classified,
            format!("{context} failed at {}", request.url),
        ));
    }
    serde_json::from_str(&body).map_err(|error| {
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
    let (status, body) = execute_oauth_request(http, request)
        .await
        .map_err(|error| AccountRefreshFailure::bad_gateway(format!("{context}: {error}")))?;
    if cursor_login_is_pending(provider_type, status, &body) {
        return Err(AccountRefreshFailure::authorization_pending(
            "cursor oauth authorization is still pending",
        ));
    }
    if !status.is_success() {
        let classified = classify_oauth_error(Some(status.as_u16()), &body);
        return Err(AccountRefreshFailure::from_classification(
            Some(status.as_u16()),
            classified,
            format!("{context} failed at {}", request.url),
        ));
    }

    let raw: serde_json::Value = serde_json::from_str(&body).map_err(|error| {
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

async fn execute_optional_profile_refresh(
    http: &reqwest::Client,
    provider_type: ProviderType,
    request: &OAuthHttpRequest,
    now_ms: i64,
    quota_refresh_interval_ms: i64,
) -> AccountRefreshUpdate {
    match execute_oauth_request(http, request).await {
        Ok((status, body)) if status.is_success() => {
            match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(profile_raw) => refresh_update_from_profile_response(
                    provider_type,
                    profile_raw,
                    now_ms,
                    quota_refresh_interval_ms,
                ),
                Err(error) => AccountRefreshUpdate {
                    last_refresh_error: Some(format!(
                        "profile refresh warning: response is not valid JSON: {error}"
                    )),
                    ..Default::default()
                },
            }
        }
        Ok((status, body)) => {
            let classified = classify_oauth_error(Some(status.as_u16()), &body);
            AccountRefreshUpdate {
                last_refresh_error: Some(format!(
                    "profile refresh warning at {}: {}",
                    request.url, classified.message
                )),
                ..Default::default()
            }
        }
        Err(error) => AccountRefreshUpdate {
            last_refresh_error: Some(format!("profile refresh warning: {error}")),
            ..Default::default()
        },
    }
}

async fn execute_oauth_request(
    http: &reqwest::Client,
    request: &OAuthHttpRequest,
) -> Result<(reqwest::StatusCode, String), reqwest::Error> {
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
    let body = response.text().await?;
    Ok((status, body))
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
            email: Some("test@example.com".to_string()),
            access_token: access_token.map(str::to_string),
            refresh_token: refresh_token.map(str::to_string),
            id_token: None,
            token_type: None,
            api_key: None,
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
            rate_limited_until: None,
            last_refresh_error: None,
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
}
