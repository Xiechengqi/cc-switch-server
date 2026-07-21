use super::pow::{solve_and_build_header, DeepSeekPowChallenge};
use reqwest::{header::HeaderMap, Client, Response};
use serde_json::Value;
use thiserror::Error;

const COMPLETION_TARGET_PATH: &str = "/api/v0/chat/completion";

#[derive(Debug, Error)]
pub enum DeepSeekClientError {
    #[error("network error: {0}")]
    Network(String),
    #[error("api error: {0}")]
    Api(String),
}

impl From<reqwest::Error> for DeepSeekClientError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.to_string())
    }
}

#[derive(Clone)]
pub struct DeepSeekWebClient {
    http: Client,
    api_base: String,
}

impl DeepSeekWebClient {
    pub fn new() -> Self {
        Self::with_api_base("https://chat.deepseek.com")
    }

    pub fn with_api_base(api_base: impl Into<String>) -> Self {
        Self {
            http: crate::infra::http::direct_client_builder()
                .user_agent("DeepSeek/2.0.4 Android/35")
                .build()
                .unwrap_or_else(|_| {
                    crate::infra::http::direct_client()
                        .expect("default direct HTTP client must build")
                }),
            api_base: api_base.into().trim_end_matches('/').to_string(),
        }
    }

    pub async fn start_completion(
        &self,
        token: &str,
        model: &str,
        prompt: &str,
    ) -> Result<Response, DeepSeekClientError> {
        let session_id = self.create_session(token).await?;
        let pow = self.create_pow_header(token).await?;
        self.completion(token, &session_id, &pow, model, prompt)
            .await
    }

    async fn create_session(&self, token: &str) -> Result<String, DeepSeekClientError> {
        let value = self
            .post_json(
                &format!("{}/api/v0/chat_session/create", self.api_base),
                token,
                &serde_json::json!({"agent":"chat"}),
            )
            .await?;
        ensure_ok(&value, "create_session")?;
        extract_session_id(&value)
            .ok_or_else(|| DeepSeekClientError::Api("create_session missing id".to_string()))
    }

    async fn create_pow_header(&self, token: &str) -> Result<String, DeepSeekClientError> {
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
            .ok_or_else(|| DeepSeekClientError::Api("create_pow missing challenge".to_string()))?
            .clone();
        let challenge: DeepSeekPowChallenge = serde_json::from_value(challenge_value)
            .map_err(|error| DeepSeekClientError::Api(error.to_string()))?;
        solve_and_build_header(&challenge)
            .await
            .map_err(|error| DeepSeekClientError::Api(error.to_string()))
    }

    async fn completion(
        &self,
        token: &str,
        session_id: &str,
        pow_header: &str,
        model: &str,
        prompt: &str,
    ) -> Result<Response, DeepSeekClientError> {
        let payload = completion_payload(session_id, model, prompt);
        Ok(self
            .http
            .post(format!("{}/api/v0/chat/completion", self.api_base))
            .headers(deepseek_base_headers())
            .bearer_auth(token)
            .header("x-ds-pow-response", pow_header)
            .json(&payload)
            .send()
            .await?)
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
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(DeepSeekClientError::Api(format!(
                "{url} returned HTTP {status}: {body}"
            )));
        }
        serde_json::from_str(&body).map_err(|error| DeepSeekClientError::Api(error.to_string()))
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
    Err(DeepSeekClientError::Api(format!(
        "{op} failed: code={code} biz_code={biz_code} msg={msg}"
    )))
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

fn completion_payload(session_id: &str, model: &str, prompt: &str) -> Value {
    serde_json::json!({
        "chat_session_id": session_id,
        "parent_message_id": null,
        "prompt": prompt,
        "ref_file_ids": [],
        "thinking_enabled": false,
        "search_enabled": false,
        "model_type": model_type(model),
    })
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
        let expire_at = 1_700_000_000_i64;
        let answer = 42_i64;
        let digest = deepseek_hash_v1(format!("{salt}_{expire_at}_{answer}").as_bytes());
        DeepSeekPowChallenge {
            algorithm: "DeepSeekHashV1".to_string(),
            challenge: hex::encode(digest),
            salt: salt.to_string(),
            expire_at,
            difficulty: 0,
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
