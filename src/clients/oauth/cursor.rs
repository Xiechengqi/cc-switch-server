use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const CURSOR_PUBLIC_API_ORIGIN: &str = "https://api.cursor.com";
const MAX_CURSOR_PUBLIC_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct VerifiedCursorApiKey {
    pub account_id: String,
    pub email: Option<String>,
    pub profile: Value,
}

#[derive(Debug, Clone)]
pub struct CursorPublicApiError {
    pub status_code: u16,
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
    let profile = cursor_public_json(client, api_key, "/v1/me").await?;
    let principal = first_string(&profile, &["/id", "/sub", "/user_id", "/userId", "/email"])
        .unwrap_or_else(|| sha256_hex(api_key));
    let email = first_string(&profile, &["/email", "/user/email"]);
    Ok(VerifiedCursorApiKey {
        account_id: format!("cursor_apikey_{}", &sha256_hex(&principal)[..24]),
        email,
        profile: json!({
            "providerType": "cursor_apikey",
            "source": "cursor_public_api",
            "cursorMe": profile,
        }),
    })
}

pub async fn available_models(
    client: &reqwest::Client,
    api_key: &str,
) -> Result<Vec<String>, CursorPublicApiError> {
    let value = cursor_public_json(client, api_key, "/v1/models").await?;
    let items = value
        .get("data")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| CursorPublicApiError {
            status_code: 502,
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

async fn cursor_public_json(
    client: &reqwest::Client,
    api_key: &str,
    path: &str,
) -> Result<Value, CursorPublicApiError> {
    let response = client
        .get(format!("{CURSOR_PUBLIC_API_ORIGIN}{path}"))
        .timeout(std::time::Duration::from_secs(10))
        .bearer_auth(api_key)
        .header("accept", "application/json")
        .send()
        .await
        .map_err(|error| CursorPublicApiError {
            status_code: 502,
            message: format!("Cursor public API request failed: {error}"),
        })?;
    let status = response.status();
    let body = read_limited(response).await?;
    if !status.is_success() {
        return Err(CursorPublicApiError {
            status_code: status.as_u16(),
            message: match status.as_u16() {
                401 | 403 => "Cursor API key was rejected".to_string(),
                429 => "Cursor API key validation was rate limited".to_string(),
                _ => format!("Cursor public API returned HTTP {}", status.as_u16()),
            },
        });
    }
    serde_json::from_slice(&body).map_err(|error| CursorPublicApiError {
        status_code: 502,
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

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_shapes_and_principal_fields_are_stable() {
        let value = json!({"user": {"email": "owner@example.com"}});
        assert_eq!(
            first_string(&value, &["/id", "/user/email"]).as_deref(),
            Some("owner@example.com")
        );
        assert_eq!(sha256_hex("key").len(), 64);
    }
}
