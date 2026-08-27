use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cursor_client_contract::{
    sdk_client_version, CLIENT_TYPE_HEADER, CLIENT_VERSION_HEADER, GHOST_MODE_ENABLED,
    GHOST_MODE_HEADER, SDK_CLIENT_TYPE,
};

const CURSOR_PUBLIC_API_ORIGIN: &str = "https://api.cursor.com";
const MAX_CURSOR_PUBLIC_BODY_BYTES: usize = 1024 * 1024;

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
    verify_api_key_at_origin(client, api_key, CURSOR_PUBLIC_API_ORIGIN).await
}

async fn verify_api_key_at_origin(
    client: &reqwest::Client,
    api_key: &str,
    origin: &str,
) -> Result<VerifiedCursorApiKey, CursorPublicApiError> {
    let profile = cursor_public_json_at_origin(client, api_key, origin, "/v1/me").await?;
    let (principal, principal_source) = verified_principal(&profile, api_key);
    let email = first_string(&profile, &["/userEmail", "/email", "/user/email"]);
    Ok(VerifiedCursorApiKey {
        account_id: format!("cursor_apikey_{}", &sha256_hex(&principal)[..24]),
        principal_source: principal_source.to_string(),
        email,
        profile: json!({
            "providerType": "cursor_apikey",
            "source": "cursor_public_api",
            "cursorMe": profile,
        }),
    })
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
        .header(CLIENT_TYPE_HEADER, SDK_CLIENT_TYPE)
        .header(CLIENT_VERSION_HEADER, sdk_client_version())
        .header(GHOST_MODE_HEADER, GHOST_MODE_ENABLED)
        .send()
        .await
        .map_err(|error| CursorPublicApiError {
            status_code: 502,
            retryable: true,
            message: format!("Cursor public API request failed: {error}"),
        })?;
    let status = response.status();
    let body = read_limited(response).await?;
    if !status.is_success() {
        return Err(CursorPublicApiError {
            status_code: status.as_u16(),
            retryable: status.as_u16() == 429 || status.is_server_error(),
            message: match status.as_u16() {
                401 => "Cursor API key was rejected".to_string(),
                403 => "Cursor API key validation was forbidden; verify API access and SDK client permissions".to_string(),
                429 => "Cursor API key validation was rate limited".to_string(),
                _ => format!("Cursor public API returned HTTP {}", status.as_u16()),
            },
        });
    }
    serde_json::from_slice(&body).map_err(|error| CursorPublicApiError {
        status_code: 502,
        retryable: false,
        message: format!("Cursor public API returned invalid JSON: {error}"),
    })
}

async fn read_limited(response: reqwest::Response) -> Result<Vec<u8>, CursorPublicApiError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CURSOR_PUBLIC_BODY_BYTES as u64)
    {
        return Err(body_too_large());
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| CursorPublicApiError {
            status_code: 502,
            retryable: true,
            message: format!("Cursor public API response read failed: {error}"),
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_CURSOR_PUBLIC_BODY_BYTES {
            return Err(body_too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn body_too_large() -> CursorPublicApiError {
    CursorPublicApiError {
        status_code: 502,
        retryable: false,
        message: "Cursor public API response exceeded 1 MiB".to_string(),
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
    use axum::routing::get;
    use axum::{Json, Router};

    #[derive(Clone)]
    struct ExpectedSdkIdentity {
        authorization: String,
        version: String,
    }

    async fn sdk_identity_gate(
        State(expected): State<ExpectedSdkIdentity>,
        uri: Uri,
        headers: HeaderMap,
    ) -> Response {
        let matches = header_matches(&headers, AUTHORIZATION.as_str(), &expected.authorization)
            && header_matches(&headers, CLIENT_TYPE_HEADER, SDK_CLIENT_TYPE)
            && header_matches(&headers, CLIENT_VERSION_HEADER, &expected.version)
            && header_matches(&headers, GHOST_MODE_HEADER, GHOST_MODE_ENABLED)
            && header_matches(&headers, "accept", "application/json");
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
    async fn public_api_profile_and_models_send_the_sdk_identity_contract() {
        let api_key = "fixture-cursor-key";
        let version = sdk_client_version();
        let router = Router::new()
            .route("/v1/me", get(sdk_identity_gate))
            .route("/v1/models", get(sdk_identity_gate))
            .with_state(ExpectedSdkIdentity {
                authorization: format!("Bearer {api_key}"),
                version,
            });
        let (origin, server) = spawn_public_api(router).await;
        let client = reqwest::Client::new();

        let profile = verify_api_key_at_origin(&client, api_key, &origin)
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
        assert_eq!(unauthorized.message, "Cursor API key was rejected");

        let forbidden = cursor_public_json_at_origin(&client, "fixture-key", &origin, "/forbidden")
            .await
            .unwrap_err();
        assert_eq!(forbidden.status_code, 403);
        assert!(!forbidden.retryable);
        assert!(forbidden.message.contains("SDK client permissions"));
        server.abort();
    }
}
