use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cursor_client_contract::{
    CLIENT_TYPE_HEADER, CLIENT_VERSION_HEADER, DEFAULT_API_KEY_EXCHANGE_URL, GHOST_MODE_ENABLED,
    GHOST_MODE_HEADER, PUBLIC_API_CLIENT_VERSION, SDK_CLIENT_TYPE,
};

const CURSOR_PUBLIC_API_ORIGIN: &str = "https://api.cursor.com";
const MAX_CURSOR_PUBLIC_BODY_BYTES: usize = 1024 * 1024;
const MAX_CURSOR_ERROR_DETAIL_CHARS: usize = 512;

#[derive(Debug, Clone)]
pub struct VerifiedCursorApiKey {
    pub account_id: String,
    pub principal_source: String,
    pub email: Option<String>,
    pub profile: Value,
}

#[derive(Debug, Clone)]
pub struct CursorPublicApiError {
    pub status_code: u16,
    pub retryable: bool,
    pub message: String,
}

impl std::fmt::Display for CursorPublicApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CursorPublicApiError {}

#[async_trait]
pub trait CursorApiKeyVerifier: std::fmt::Debug + Send + Sync {
    async fn verify(
        &self,
        client: &reqwest::Client,
        api_key: &str,
    ) -> Result<VerifiedCursorApiKey, CursorPublicApiError>;
}

#[derive(Debug, Default)]
pub struct OfficialCursorApiKeyVerifier;

#[async_trait]
impl CursorApiKeyVerifier for OfficialCursorApiKeyVerifier {
    async fn verify(
        &self,
        client: &reqwest::Client,
        api_key: &str,
    ) -> Result<VerifiedCursorApiKey, CursorPublicApiError> {
        verify_api_key(client, api_key).await
    }
}

pub async fn verify_api_key(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<VerifiedCursorApiKey, CursorPublicApiError> {
    verify_api_key_at_endpoints(
        client,
        api_key,
        CURSOR_PUBLIC_API_ORIGIN,
        DEFAULT_API_KEY_EXCHANGE_URL,
    )
    .await
}

async fn verify_api_key_at_endpoints(
    client: &reqwest::Client,
    api_key: &str,
    public_origin: &str,
    exchange_url: &str,
) -> Result<VerifiedCursorApiKey, CursorPublicApiError> {
    let profile = match cursor_public_json_at_origin(client, api_key, public_origin, "/v1/me").await
    {
        Ok(profile) => profile,
        Err(public_error) if public_error.status_code == 403 => {
            verify_api_key_exchange(client, api_key, exchange_url)
                .await
                .map_err(|exchange_error| {
                    cursor_fallback_verification_error(public_error, exchange_error)
                })?;
            return Ok(verified_api_key_from_exchange(api_key));
        }
        Err(error) => return Err(error),
    };
    Ok(verified_api_key_from_profile(profile, api_key))
}

fn verified_api_key_from_profile(profile: Value, api_key: &str) -> VerifiedCursorApiKey {
    let (principal, principal_source) = verified_principal(&profile, api_key);
    let email = first_string(&profile, &["/userEmail", "/email", "/user/email"]);
    VerifiedCursorApiKey {
        account_id: format!("cursor_apikey_{}", &sha256_hex(&principal)[..24]),
        principal_source: principal_source.to_string(),
        email,
        profile: json!({
            "providerType": "cursor_apikey",
            "source": "cursor_public_api",
            "cursorMe": profile,
        }),
    }
}

fn verified_api_key_from_exchange(api_key: &str) -> VerifiedCursorApiKey {
    let (principal, principal_source) = verified_principal(&Value::Null, api_key);
    VerifiedCursorApiKey {
        account_id: format!("cursor_apikey_{}", &sha256_hex(&principal)[..24]),
        principal_source: principal_source.to_string(),
        email: None,
        profile: json!({
            "providerType": "cursor_apikey",
            "source": "cursor_api_key_exchange",
        }),
    }
}

fn verified_principal(profile: &Value, api_key: &str) -> (String, &'static str) {
    if let Some(value) = first_scalar_string(profile, &["/userId", "/id", "/sub", "/user_id"]) {
        (value, "user_id")
    } else if let Some(value) = first_string(profile, &["/userEmail", "/email", "/user/email"]) {
        (value.trim().to_ascii_lowercase(), "email")
    } else {
        (sha256_hex(api_key), "api_key_fallback")
    }
}

pub async fn available_models(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<String>, CursorPublicApiError> {
    available_models_at_origin(client, api_key, CURSOR_PUBLIC_API_ORIGIN).await
}

async fn available_models_at_origin(
    client: &reqwest::Client,
    api_key: &str,
    origin: &str,
) -> Result<Vec<String>, CursorPublicApiError> {
    let value = cursor_public_json_at_origin(client, api_key, origin, "/v1/models").await?;
    let items = model_items(&value).ok_or_else(|| CursorPublicApiError {
        status_code: 502,
        retryable: false,
        message: "Cursor /v1/models returned an unsupported response shape".to_string(),
    })?;
    let mut models = items
        .iter()
        .filter_map(|item| {
            item.as_str()
                .map(str::to_string)
                .or_else(|| first_string(item, &["/id", "/model", "/name"]))
        })
        .filter(|model| !model.trim().is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

async fn cursor_public_json_at_origin(
    client: &reqwest::Client,
    api_key: &str,
    origin: &str,
    path: &str,
) -> Result<Value, CursorPublicApiError> {
    let response = client
        .get(format!("{}{path}", origin.trim_end_matches('/')))
        .timeout(std::time::Duration::from_secs(10))
        .bearer_auth(api_key)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header(CLIENT_TYPE_HEADER, SDK_CLIENT_TYPE)
        .header(CLIENT_VERSION_HEADER, PUBLIC_API_CLIENT_VERSION)
        .header(GHOST_MODE_HEADER, GHOST_MODE_ENABLED)
        .send()
        .await
        .map_err(|error| CursorPublicApiError {
            status_code: 502,
            retryable: true,
            message: format!("Cursor public API request failed: {error}"),
        })?;
    let status = response.status();
    let body = read_limited(response, "public API").await?;
    if !status.is_success() {
        return Err(cursor_http_error(
            "public API validation",
            status,
            &body,
            api_key,
        ));
    }
    serde_json::from_slice(&body).map_err(|error| CursorPublicApiError {
        status_code: 502,
        retryable: false,
        message: format!("Cursor public API returned invalid JSON: {error}"),
    })
}

async fn verify_api_key_exchange(
    client: &reqwest::Client,
    api_key: &str,
    exchange_url: &str,
) -> Result<(), CursorPublicApiError> {
    let response = client
        .post(exchange_url)
        .timeout(std::time::Duration::from_secs(10))
        .bearer_auth(api_key)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|error| CursorPublicApiError {
            status_code: 502,
            retryable: true,
            message: format!("Cursor API key exchange request failed: {error}"),
        })?;
    let status = response.status();
    let body = read_limited(response, "API key exchange").await?;
    if !status.is_success() {
        return Err(cursor_http_error(
            "API key exchange",
            status,
            &body,
            api_key,
        ));
    }
    let value = serde_json::from_slice::<Value>(&body).map_err(|error| CursorPublicApiError {
        status_code: 502,
        retryable: false,
        message: format!("Cursor API key exchange returned invalid JSON: {error}"),
    })?;
    if first_string(&value, &["/accessToken", "/access_token"]).is_none() {
        return Err(CursorPublicApiError {
            status_code: 502,
            retryable: false,
            message: "Cursor API key exchange response missing access token".to_string(),
        });
    }
    Ok(())
}

fn cursor_http_error(
    surface: &str,
    status: reqwest::StatusCode,
    body: &[u8],
    api_key: &str,
) -> CursorPublicApiError {
    let base = match status.as_u16() {
        401 => format!("Cursor {surface} rejected the API key"),
        403 => format!("Cursor {surface} was forbidden"),
        429 => format!("Cursor {surface} was rate limited"),
        _ => format!("Cursor {surface} returned HTTP {}", status.as_u16()),
    };
    let detail = cursor_error_detail(body, api_key)
        .map(|detail| format!("; {detail}"))
        .unwrap_or_default();
    CursorPublicApiError {
        status_code: status.as_u16(),
        retryable: status.as_u16() == 429 || status.is_server_error(),
        message: format!("{base}{detail}"),
    }
}

fn cursor_fallback_verification_error(
    public_error: CursorPublicApiError,
    exchange_error: CursorPublicApiError,
) -> CursorPublicApiError {
    CursorPublicApiError {
        status_code: exchange_error.status_code,
        retryable: exchange_error.retryable,
        message: format!(
            "{}; same-key exchange verification also failed: {}",
            public_error.message, exchange_error.message
        ),
    }
}

fn cursor_error_detail(body: &[u8], api_key: &str) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let message = first_scalar_string(
        &value,
        &["/error/message", "/message", "/details/0/message", "/error"],
    );
    let code = first_scalar_string(&value, &["/error/code", "/code"]);
    let detail = match (code, message) {
        (Some(code), Some(message)) if code != message => format!("{code}: {message}"),
        (_, Some(message)) => message,
        (Some(code), None) => code,
        _ => return None,
    };
    let redacted = crate::logging::redact_sensitive_text_with_values(&detail, [api_key]);
    let bounded = redacted
        .chars()
        .take(MAX_CURSOR_ERROR_DETAIL_CHARS)
        .collect::<String>();
    (!bounded.trim().is_empty()).then_some(bounded)
}

async fn read_limited(
    response: reqwest::Response,
    surface: &str,
) -> Result<Vec<u8>, CursorPublicApiError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CURSOR_PUBLIC_BODY_BYTES as u64)
    {
        return Err(body_too_large(surface));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| CursorPublicApiError {
            status_code: 502,
            retryable: true,
            message: format!("Cursor {surface} response read failed: {error}"),
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_CURSOR_PUBLIC_BODY_BYTES {
            return Err(body_too_large(surface));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn body_too_large(surface: &str) -> CursorPublicApiError {
    CursorPublicApiError {
        status_code: 502,
        retryable: false,
        message: format!("Cursor {surface} response exceeded 1 MiB"),
    }
}

fn first_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn first_scalar_string(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        match value {
            Value::String(value) => Some(value.trim().to_string()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        }
        .filter(|value| !value.is_empty())
    })
}

fn model_items(value: &Value) -> Option<&Vec<Value>> {
    value
        .get("items")
        .or_else(|| value.get("data"))
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode, Uri};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use axum::{Json, Router};

    #[derive(Clone)]
    struct ExpectedPublicIdentity {
        authorization: String,
    }

    async fn public_identity_gate(
        State(expected): State<ExpectedPublicIdentity>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
        let matches = header_matches(&headers, AUTHORIZATION.as_str(), &expected.authorization)
            && header_matches(&headers, CLIENT_TYPE_HEADER, SDK_CLIENT_TYPE)
            && header_matches(&headers, CLIENT_VERSION_HEADER, PUBLIC_API_CLIENT_VERSION)
            && header_matches(&headers, GHOST_MODE_HEADER, GHOST_MODE_ENABLED)
            && header_matches(&headers, "accept", "application/json")
            && header_matches(&headers, "content-type", "application/json");
        if !matches {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        match uri.path() {
            "/v1/me" => Json(json!({
                "userId":"verified-user",
                "userEmail":"owner@example.com"
            }))
            .into_response(),
            "/v1/models" => Json(json!({
                "items":[{"id":"composer-2.5"}]
            }))
            .into_response(),
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }

    async fn exchange_identity_gate(
        State(expected): State<ExpectedPublicIdentity>,
        headers: HeaderMap,
        body: String,
    ) -> Response {
        let matches = header_matches(&headers, AUTHORIZATION.as_str(), &expected.authorization)
            && header_matches(&headers, "accept", "application/json")
            && header_matches(&headers, "content-type", "application/json")
            && headers.get(CLIENT_TYPE_HEADER).is_none()
            && headers.get(CLIENT_VERSION_HEADER).is_none()
            && headers.get(GHOST_MODE_HEADER).is_none()
            && body == "{}";
        if !matches {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(json!({"accessToken":"fixture-exchanged-token"})).into_response()
    }

    async fn forbidden_with_sensitive_detail() -> Response {
        (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": {
                    "code": "permission_denied",
                    "message": "fixture-cursor-key denied for owner@example.com"
                }
            })),
        )
            .into_response()
    }

    fn header_matches(headers: &HeaderMap, name: &str, expected: &str) -> bool {
        headers.get(name).and_then(|value| value.to_str().ok()) == Some(expected)
    }

    async fn spawn_public_api(router: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (origin, server)
    }

    #[test]
    fn model_shapes_and_principal_fields_are_stable() {
        let value = json!({"user": {"email": "owner@example.com"}});
        assert_eq!(
            first_string(&value, &["/id", "/user/email"]).as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(sha256_hex("key").len(), 64);

        let cursor_me = json!({
            "apiKeyName": "server",
            "userId": 42,
            "userEmail": "cursor@example.com"
        });
        assert_eq!(
            first_scalar_string(&cursor_me, &["/userId"]).as_deref(),
            Some("42")
        );
        assert_eq!(
            first_string(&cursor_me, &["/userEmail"]).as_deref(),
            Some("cursor@example.com")
        );
        assert_eq!(
            model_items(&json!({"items": [{"id": "composer-2.5"}]})).map(Vec::len),
            Some(1)
        );
    }

    #[test]
    fn public_api_error_retryability_distinguishes_transport_from_protocol() {
        let transient = CursorPublicApiError {
            status_code: 502,
            retryable: true,
            message: "transport".to_string(),
        };
        let malformed = CursorPublicApiError {
            status_code: 502,
            retryable: false,
            message: "invalid json".to_string(),
        };
        let auth = CursorPublicApiError {
            status_code: 401,
            retryable: false,
            message: "rejected".to_string(),
        };
        assert!(transient.retryable);
        assert!(!malformed.retryable);
        assert!(!auth.retryable);
    }

    #[test]
    fn principal_selection_prefers_user_id_then_normalized_email_then_key_fallback() {
        assert_eq!(
            verified_principal(
                &json!({"userId":"stable-user", "userEmail":"UPPER@EXAMPLE.COM"}),
                "key-one"
            ),
            ("stable-user".to_string(), "user_id")
        );
        assert_eq!(
            verified_principal(&json!({"email":" UPPER@EXAMPLE.COM "}), "key-one"),
            ("upper@example.com".to_string(), "email")
        );
        let (fallback, source) = verified_principal(&json!({}), "key-one");
        assert_eq!(source, "api_key_fallback");
        assert_eq!(fallback, sha256_hex("key-one"));
    }

    #[tokio::test]
    async fn public_api_profile_and_models_send_the_public_identity_contract() {
        let api_key = "fixture-cursor-key";
        let router = Router::new()
            .route("/v1/me", get(public_identity_gate))
            .route("/v1/models", get(public_identity_gate))
            .with_state(ExpectedPublicIdentity {
                authorization: format!("Bearer {api_key}"),
            });
        let (origin, server) = spawn_public_api(router).await;
        let client = reqwest::Client::new();

        let profile = verify_api_key_at_endpoints(
            &client,
            api_key,
            &origin,
            &format!("{origin}/unused-exchange"),
        )
        .await
        .unwrap();
        assert_eq!(profile.principal_source, "user_id");
        assert_eq!(profile.email.as_deref(), Some("owner@example.com"));
        let models = available_models_at_origin(&client, api_key, &origin)
            .await
            .unwrap();
        assert_eq!(models, vec!["composer-2.5"]);
        server.abort();
    }

    #[tokio::test]
    async fn public_api_distinguishes_invalid_keys_from_forbidden_sdk_access() {
        let router = Router::new()
            .route("/unauthorized", get(|| async { StatusCode::UNAUTHORIZED }))
            .route("/forbidden", get(|| async { StatusCode::FORBIDDEN }));
        let (origin, server) = spawn_public_api(router).await;
        let client = reqwest::Client::new();

        let unauthorized =
            cursor_public_json_at_origin(&client, "fixture-key", &origin, "/unauthorized")
                .await
                .unwrap_err();
        assert_eq!(unauthorized.status_code, 401);
        assert!(!unauthorized.retryable);
        assert_eq!(
            unauthorized.message,
            "Cursor public API validation rejected the API key"
        );

        let forbidden = cursor_public_json_at_origin(&client, "fixture-key", &origin, "/forbidden")
            .await
            .unwrap_err();
        assert_eq!(forbidden.status_code, 403);
        assert!(!forbidden.retryable);
        assert_eq!(
            forbidden.message,
            "Cursor public API validation was forbidden"
        );
        server.abort();
    }

    #[tokio::test]
    async fn forbidden_profile_falls_back_to_strict_same_key_exchange() {
        let api_key = "fixture-cursor-key";
        let router = Router::new()
            .route(
                "/v1/me",
                get(|| async { StatusCode::FORBIDDEN.into_response() }),
            )
            .route("/auth/exchange_user_api_key", post(exchange_identity_gate))
            .with_state(ExpectedPublicIdentity {
                authorization: format!("Bearer {api_key}"),
            });
        let (origin, server) = spawn_public_api(router).await;

        let verified = verify_api_key_at_endpoints(
            &reqwest::Client::new(),
            api_key,
            &origin,
            &format!("{origin}/auth/exchange_user_api_key"),
        )
        .await
        .unwrap();
        assert_eq!(verified.principal_source, "api_key_fallback");
        assert!(verified.email.is_none());
        assert_eq!(verified.profile["source"], "cursor_api_key_exchange");
        assert!(verified.profile.get("accessToken").is_none());
        server.abort();
    }

    #[tokio::test]
    async fn unauthorized_profile_never_falls_back_to_exchange() {
        let router = Router::new()
            .route(
                "/v1/me",
                get(|| async { StatusCode::UNAUTHORIZED.into_response() }),
            )
            .route(
                "/auth/exchange_user_api_key",
                post(|| async { Json(json!({"accessToken":"must-not-be-used"})) }),
            );
        let (origin, server) = spawn_public_api(router).await;

        let error = verify_api_key_at_endpoints(
            &reqwest::Client::new(),
            "fixture-cursor-key",
            &origin,
            &format!("{origin}/auth/exchange_user_api_key"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status_code, 401);
        server.abort();
    }

    #[tokio::test]
    async fn exchange_fallback_fails_closed_without_an_access_token() {
        let router = Router::new()
            .route(
                "/v1/me",
                get(|| async { StatusCode::FORBIDDEN.into_response() }),
            )
            .route(
                "/auth/exchange_user_api_key",
                post(|| async { Json(json!({})) }),
            );
        let (origin, server) = spawn_public_api(router).await;

        let error = verify_api_key_at_endpoints(
            &reqwest::Client::new(),
            "fixture-cursor-key",
            &origin,
            &format!("{origin}/auth/exchange_user_api_key"),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status_code, 502);
        assert!(error.message.contains("missing access token"));
        server.abort();
    }

    #[tokio::test]
    async fn fallback_errors_include_bounded_diagnostics_without_secrets() {
        let router = Router::new()
            .route("/v1/me", get(forbidden_with_sensitive_detail))
            .route(
                "/auth/exchange_user_api_key",
                post(forbidden_with_sensitive_detail),
            );
        let (origin, server) = spawn_public_api(router).await;

        let error = verify_api_key_at_endpoints(
            &reqwest::Client::new(),
            "fixture-cursor-key",
            &origin,
            &format!("{origin}/auth/exchange_user_api_key"),
        )
        .await
        .unwrap_err();
        assert!(!error.message.contains("fixture-cursor-key"));
        assert!(!error.message.contains("owner@example.com"));
        assert!(error.message.contains("[REDACTED]"));
        assert!(error.message.contains("same-key exchange verification"));
        server.abort();
    }
}
