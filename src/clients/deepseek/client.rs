use super::pow::{solve_and_build_header, DeepSeekPowChallenge};
use futures_util::StreamExt;
use reqwest::{header::HeaderMap, Client, Response};
use serde_json::Value;
use std::time::Duration;
use thiserror::Error;

pub(crate) const COMPLETION_TARGET_PATH: &str = "/api/v0/chat/completion";
const DEEPSEEK_WEB_ORIGIN: &str = "https://chat.deepseek.com";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const COMPLETION_HEADER_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONTROL_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum DeepSeekClientError {
    #[error("DeepSeek {operation} network error: {message}")]
    Network {
        operation: &'static str,
        message: String,
    },
    #[error("DeepSeek {operation} returned HTTP {status_code}: {message}")]
    Upstream {
        operation: &'static str,
        status_code: u16,
        message: String,
    },
    #[error("DeepSeek {operation} protocol error: {message}")]
    Protocol {
        operation: &'static str,
        message: String,
    },
}

impl DeepSeekClientError {
    pub fn status_code(&self) -> Option<u16> {
        match self {
            Self::Upstream { status_code, .. } => Some(*status_code),
            Self::Network { .. } | Self::Protocol { .. } => None,
        }
    }

    pub fn is_authentication_failure(&self) -> bool {
        matches!(self.status_code(), Some(401 | 403))
    }

    pub fn operation(&self) -> &'static str {
        match self {
            Self::Network { operation, .. }
            | Self::Upstream { operation, .. }
            | Self::Protocol { operation, .. } => operation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekCompletionRequest<'a> {
    pub session_id: &'a str,
    pub model: &'a str,
    pub prompt: &'a str,
    pub thinking_enabled: bool,
    pub search_enabled: bool,
}

#[derive(Clone)]
pub struct DeepSeekWebClient {
    http: Client,
    api_base: String,
}

impl DeepSeekWebClient {
    pub fn new() -> Self {
        let http = crate::infra::http::direct_client_builder()
            .build()
            .unwrap_or_else(|_| {
                crate::infra::http::direct_client().expect("default direct HTTP client must build")
            });
        Self::with_http_client_and_api_base(http, DEEPSEEK_WEB_ORIGIN)
    }

    pub fn with_http_client(http: Client) -> Self {
        Self::with_http_client_and_api_base(http, DEEPSEEK_WEB_ORIGIN)
    }

    #[cfg(test)]
    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        let http = crate::infra::http::direct_client_builder()
            .build()
            .unwrap_or_else(|_| {
                crate::infra::http::direct_client().expect("default direct HTTP client must build")
            });
        Self::with_http_client_and_api_base(http, api_base)
    }

    pub(crate) fn with_http_client_and_api_base(http: Client, api_base: impl Into<String>) -> Self {
        let requested = api_base.into().trim_end_matches('/').to_string();
        #[cfg(test)]
        let api_base = requested;
        #[cfg(not(test))]
        let api_base = {
            let _ = requested;
            DEEPSEEK_WEB_ORIGIN.to_string()
        };
        Self { http, api_base }
    }

    pub async fn start_completion(
        &self,
        token: &str,
        model: &str,
        prompt: &str,
    ) -> Result<Response, DeepSeekClientError> {
        let session_id = self.create_session(token).await?;
        let pow = self.create_pow_header(token).await?;
        self.completion(
            token,
            &pow,
            DeepSeekCompletionRequest {
                session_id: &session_id,
                model,
                prompt,
                thinking_enabled: false,
                search_enabled: false,
            },
        )
        .await
    }

    pub(crate) async fn create_session(&self, token: &str) -> Result<String, DeepSeekClientError> {
        let value = self
            .post_json(
                &format!("{}/api/v0/chat_session/create", self.api_base),
                token,
                &serde_json::json!({"agent":"chat"}),
            )
            .await?;
        ensure_ok(&value, "create_session")?;
        extract_session_id(&value).ok_or_else(|| DeepSeekClientError::Protocol {
            operation: "create_session",
            message: "response is missing a session id".to_string(),
        })
    }

    pub(crate) async fn create_pow_header(
        &self,
        token: &str,
    ) -> Result<String, DeepSeekClientError> {
        let value = self
            .post_json(
                &format!("{}/api/v0/chat/create_pow_challenge", self.api_base),
                token,
                &serde_json::json!({"target_path": COMPLETION_TARGET_PATH}),
            )
            .await?;
        ensure_ok(&value, "create_pow")?;
        let challenge_value = value
            .pointer("/data/biz_data/challenge")
            .ok_or_else(|| DeepSeekClientError::Protocol {
                operation: "create_pow",
                message: "response is missing a challenge".to_string(),
            })?
            .clone();
        let challenge: DeepSeekPowChallenge =
            serde_json::from_value(challenge_value).map_err(|error| {
                DeepSeekClientError::Protocol {
                    operation: "create_pow",
                    message: error.to_string(),
                }
            })?;
        solve_and_build_header(&challenge)
            .await
            .map_err(|error| DeepSeekClientError::Protocol {
                operation: "create_pow",
                message: error.to_string(),
            })
    }

    pub(crate) async fn completion(
        &self,
        token: &str,
        pow_header: &str,
        request: DeepSeekCompletionRequest<'_>,
    ) -> Result<Response, DeepSeekClientError> {
        let payload = completion_payload(&request);
        let response = self
            .http
            .post(format!("{}/api/v0/chat/completion", self.api_base))
            .headers(deepseek_base_headers())
            .bearer_auth(token)
            .header("x-ds-pow-response", pow_header)
            .json(&payload)
            .timeout(COMPLETION_HEADER_TIMEOUT)
            .send()
            .await
            .map_err(|error| DeepSeekClientError::Network {
                operation: "completion",
                message: error.to_string(),
            })?;
        if response
            .content_length()
            .is_some_and(|length| length > 64 * 1024 * 1024)
        {
            return Err(DeepSeekClientError::Protocol {
                operation: "completion",
                message: "declared response body exceeds 64 MiB".to_string(),
            });
        }
        Ok(response)
    }

    async fn post_json(
        &self,
        url: &str,
        token: &str,
        payload: &Value,
    ) -> Result<Value, DeepSeekClientError> {
        let resp = self
            .http
            .post(url)
            .headers(deepseek_base_headers())
            .bearer_auth(token)
            .json(payload)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| DeepSeekClientError::Network {
                operation: control_operation(url),
                message: error.to_string(),
            })?;
        let status = resp.status();
        let operation = control_operation(url);
        let body = read_bounded_body(resp, operation).await?;
        if !status.is_success() {
            return Err(DeepSeekClientError::Upstream {
                operation,
                status_code: status.as_u16(),
                message: sanitized_upstream_message(&body),
            });
        }
        serde_json::from_slice(&body).map_err(|error| DeepSeekClientError::Protocol {
            operation,
            message: format!("response is not valid JSON: {error}"),
        })
    }
}

impl Default for DeepSeekWebClient {
    fn default() -> Self {
        Self::new()
    }
}

pub fn deepseek_base_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_static("DeepSeek/2.0.4 Android/35"),
    );
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    headers.insert(
        "accept-charset",
        reqwest::header::HeaderValue::from_static("UTF-8"),
    );
    headers.insert(
        "x-client-platform",
        reqwest::header::HeaderValue::from_static("android"),
    );
    headers.insert(
        "x-client-version",
        reqwest::header::HeaderValue::from_static("2.0.4"),
    );
    headers.insert(
        "x-client-locale",
        reqwest::header::HeaderValue::from_static("zh_CN"),
    );
    headers
}

fn ensure_ok(value: &Value, op: &str) -> Result<(), DeepSeekClientError> {
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
    let biz_code = value
        .pointer("/data/biz_code")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if code == 0 && biz_code == 0 {
        return Ok(());
    }
    let msg = value
        .pointer("/data/biz_msg")
        .or_else(|| value.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("unknown error");
    Err(DeepSeekClientError::Protocol {
        operation: match op {
            "create_session" => "create_session",
            "create_pow" => "create_pow",
            _ => "control",
        },
        message: format!("code={code} biz_code={biz_code} msg={msg}"),
    })
}

fn extract_session_id(value: &Value) -> Option<String> {
    value
        .pointer("/data/biz_data/id")
        .or_else(|| value.pointer("/data/biz_data/chat_session/id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn completion_payload(request: &DeepSeekCompletionRequest<'_>) -> Value {
    serde_json::json!({
        "chat_session_id": request.session_id,
        "parent_message_id": null,
        "prompt": request.prompt,
        "ref_file_ids": [],
        "thinking_enabled": request.thinking_enabled,
        "search_enabled": request.search_enabled,
        "preempt": false,
        "model_type": model_type(request.model),
    })
}

fn control_operation(url: &str) -> &'static str {
    if url.ends_with("/chat_session/create") {
        "create_session"
    } else if url.ends_with("/chat/create_pow_challenge") {
        "create_pow"
    } else {
        "control"
    }
}

async fn read_bounded_body(
    response: Response,
    operation: &'static str,
) -> Result<Vec<u8>, DeepSeekClientError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CONTROL_BODY_BYTES as u64)
    {
        return Err(DeepSeekClientError::Protocol {
            operation,
            message: "declared response body exceeds 1 MiB".to_string(),
        });
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| DeepSeekClientError::Network {
            operation,
            message: error.to_string(),
        })?;
        if body.len().saturating_add(chunk.len()) > MAX_CONTROL_BODY_BYTES {
            return Err(DeepSeekClientError::Protocol {
                operation,
                message: "response body exceeds 1 MiB".to_string(),
            });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn sanitized_upstream_message(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.is_empty() {
        "empty error response".to_string()
    } else {
        text.chars().take(1_024).collect()
    }
}

fn model_type(model: &str) -> &'static str {
    match model {
        "deepseek-v4-pro"
        | "deepseek-v4-pro-nothinking"
        | "deepseek-v4-pro-search"
        | "deepseek-v4-pro-search-nothinking" => "expert",
        "deepseek-v4-vision" | "deepseek-v4-vision-nothinking" => "vision",
        _ => "default",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::deepseek::pow::{deepseek_hash_v1, DeepSeekPowChallenge};
    use axum::routing::post;
    use axum::{Json, Router};
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn solvable_pow_challenge() -> DeepSeekPowChallenge {
        let salt = "cc_switch_test";
        let expire_at = chrono::Utc::now().timestamp() + 60;
        let answer = 42_i64;
        let digest = deepseek_hash_v1(format!("{salt}_{expire_at}_{answer}").as_bytes());
        DeepSeekPowChallenge {
            algorithm: "DeepSeekHashV1".to_string(),
            challenge: hex::encode(digest),
            salt: salt.to_string(),
            expire_at,
            difficulty: answer + 1,
            expire_after: Some(60),
            signature: "test-signature".to_string(),
            target_path: COMPLETION_TARGET_PATH.to_string(),
        }
    }

    #[tokio::test]
    async fn start_completion_hits_mocked_deepseek_endpoints() {
        let challenge = solvable_pow_challenge();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let completion_hits = Arc::new(AtomicUsize::new(0));
        let completion_hits_for_route = completion_hits.clone();
        let app = Router::new()
            .route(
                "/api/v0/chat_session/create",
                post(|| async {
                    Json(json!({
                        "code": 0,
                        "data": {"biz_code": 0, "biz_data": {"id": "session-test"}}
                    }))
                }),
            )
            .route(
                "/api/v0/chat/create_pow_challenge",
                post({
                    let challenge = challenge.clone();
                    move || {
                        let challenge = challenge.clone();
                        async move {
                            Json(json!({
                                "code": 0,
                                "data": {
                                    "biz_code": 0,
                                    "biz_data": {"challenge": challenge}
                                }
                            }))
                        }
                    }
                }),
            )
            .route(
                "/api/v0/chat/completion",
                post({
                    let completion_hits_for_route = completion_hits_for_route.clone();
                    move |headers: axum::http::HeaderMap| {
                        let completion_hits_for_route = completion_hits_for_route.clone();
                        async move {
                            assert_eq!(
                                headers
                                    .get("authorization")
                                    .and_then(|value| value.to_str().ok()),
                                Some("Bearer imported-token")
                            );
                            assert!(headers.contains_key("x-ds-pow-response"));
                            completion_hits_for_route.fetch_add(1, Ordering::SeqCst);
                            (
                                axum::http::StatusCode::OK,
                                "data: {\"p\":\"response/content\",\"v\":\"hello\"}\ndata: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n",
                            )
                        }
                    }
                }),
            );
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = DeepSeekWebClient::with_api_base(&base);
        let response = client
            .start_completion("imported-token", "deepseek-v4-flash", "User: ping")
            .await
            .unwrap();
        let body = response.text().await.unwrap_or_default();
        assert!(body.contains("hello"));
        assert_eq!(completion_hits.load(Ordering::SeqCst), 1);
    }
}
