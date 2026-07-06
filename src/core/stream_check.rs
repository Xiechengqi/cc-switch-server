use std::time::{Duration, Instant};

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::core::provider::AppKind;
use crate::core::providers::StoredProvider;
use crate::proxy::{self, adapters::ProviderAdapter, ProxyRoute};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCheckConfig {
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_degraded_threshold_ms")]
    pub degraded_threshold_ms: u64,
    #[serde(default = "default_claude_model")]
    pub claude_model: String,
    #[serde(default = "default_codex_model")]
    pub codex_model: String,
    #[serde(default = "default_gemini_model")]
    pub gemini_model: String,
    #[serde(default = "default_test_prompt")]
    pub test_prompt: String,
}

fn default_timeout_secs() -> u64 {
    8
}

fn default_max_retries() -> u32 {
    1
}

fn default_degraded_threshold_ms() -> u64 {
    6000
}

fn default_claude_model() -> String {
    "claude-haiku-4-5-20251001".to_string()
}

fn default_codex_model() -> String {
    "gpt-5.5@low".to_string()
}

fn default_gemini_model() -> String {
    "gemini-3.5-flash".to_string()
}

fn default_test_prompt() -> String {
    "Who are you?".to_string()
}

impl Default for StreamCheckConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout_secs(),
            max_retries: default_max_retries(),
            degraded_threshold_ms: default_degraded_threshold_ms(),
            claude_model: default_claude_model(),
            codex_model: default_codex_model(),
            gemini_model: default_gemini_model(),
            test_prompt: default_test_prompt(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Operational,
    Degraded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamCheckResult {
    pub status: HealthStatus,
    pub success: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_time_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(default)]
    pub model_used: String,
    pub tested_at: i64,
    pub retry_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_category: Option<String>,
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub cache_read_tokens: u32,
    #[serde(default)]
    pub cache_creation_tokens: u32,
}

pub fn stream_check_config_from_value(value: &Value) -> StreamCheckConfig {
    serde_json::from_value(value.clone()).unwrap_or_default()
}

pub fn resolve_test_model(
    app: AppKind,
    stored: &StoredProvider,
    config: &StreamCheckConfig,
) -> String {
    stored
        .provider
        .extra
        .get("testModel")
        .or_else(|| stored.provider.settings_config.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match app {
            AppKind::Claude => config.claude_model.clone(),
            AppKind::Codex => config.codex_model.clone(),
            AppKind::Gemini => config.gemini_model.clone(),
        })
}

pub async fn check_provider_reachability(
    http_client: &Client,
    stored: &StoredProvider,
    config: &StreamCheckConfig,
) -> StreamCheckResult {
    let effective = merge_provider_config(stored, config);
    let mut last_result = None;
    for attempt in 0..=effective.max_retries {
        let result = check_once(http_client, stored, &effective).await;
        if result.success || attempt >= effective.max_retries {
            return StreamCheckResult {
                retry_count: attempt,
                ..result
            };
        }
        if should_retry(&result.message) {
            last_result = Some(result);
            continue;
        }
        return StreamCheckResult {
            retry_count: attempt,
            ..result
        };
    }
    last_result.unwrap_or_else(|| failed_result("Check failed", effective.max_retries))
}

fn merge_provider_config(stored: &StoredProvider, global: &StreamCheckConfig) -> StreamCheckConfig {
    let test_model = stored
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.test_config.as_ref())
        .and_then(|value| {
            value
                .get("testModel")
                .or_else(|| value.get("test_model"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let mut config = global.clone();
    if let Some(model) = test_model {
        match stored.app {
            AppKind::Claude => config.claude_model = model,
            AppKind::Codex => config.codex_model = model,
            AppKind::Gemini => config.gemini_model = model,
        }
    }
    config
}

async fn check_once(
    http_client: &Client,
    stored: &StoredProvider,
    config: &StreamCheckConfig,
) -> StreamCheckResult {
    let started = Instant::now();
    let model = resolve_test_model(stored.app, stored, config);
    let probe_url = match resolve_probe_url(stored, &model) {
        Ok(url) => url,
        Err(message) => {
            return failed_result(message, 0);
        }
    };
    let timeout = Duration::from_secs(config.timeout_secs);
    let result = probe_reachability(http_client, &probe_url, timeout).await;
    let response_time = started.elapsed().as_millis() as u64;
    build_reachability_result(result, response_time, config.degraded_threshold_ms)
}

fn resolve_probe_url(stored: &StoredProvider, model: &str) -> Result<String, String> {
    let adapter = proxy::adapters::adapter_for(stored.app, stored.provider_type);
    let route = default_test_route(stored.app);
    let gemini_path = default_gemini_test_path(stored.app, model, false);
    let endpoint = adapter
        .resolve_endpoint(
            route,
            (!gemini_path.is_empty()).then_some(gemini_path),
            stored,
        )
        .map_err(|error| error.to_string())?;
    Ok(reachability_origin(&endpoint))
}

fn default_test_route(app: AppKind) -> ProxyRoute {
    match app {
        AppKind::Claude => ProxyRoute::ClaudeMessages,
        AppKind::Codex => ProxyRoute::CodexResponses,
        AppKind::Gemini => ProxyRoute::Gemini,
    }
}

fn default_gemini_test_path(app: AppKind, model: &str, stream: bool) -> String {
    if app != AppKind::Gemini {
        return String::new();
    }
    if stream {
        format!("/v1beta/models/{model}:streamGenerateContent")
    } else {
        format!("/v1beta/models/{model}:generateContent")
    }
}

fn reachability_origin(endpoint: &str) -> String {
    if let Ok(url) = reqwest::Url::parse(endpoint.trim()) {
        let mut origin = url.origin().ascii_serialization();
        if origin.is_empty() {
            if let Some(host) = url.host_str() {
                let scheme = url.scheme();
                origin = match url.port() {
                    Some(port) => format!("{scheme}://{host}:{port}"),
                    None => format!("{scheme}://{host}"),
                };
            }
        }
        if !origin.is_empty() {
            return origin;
        }
    }
    endpoint.trim_end_matches('/').to_string()
}

async fn probe_reachability(
    http_client: &Client,
    base_url: &str,
    timeout: Duration,
) -> Result<u16, String> {
    let url = base_url.trim();
    if url.is_empty() {
        return Err("base_url 为空".to_string());
    }
    let response = http_client
        .get(url)
        .timeout(timeout)
        .header("accept", "*/*")
        .header("accept-encoding", "identity")
        .send()
        .await
        .map_err(map_request_error)?;
    Ok(response.status().as_u16())
}

fn build_reachability_result(
    result: Result<u16, String>,
    response_time: u64,
    degraded_threshold_ms: u64,
) -> StreamCheckResult {
    let tested_at = chrono::Utc::now().timestamp();
    match result {
        Ok(status) => StreamCheckResult {
            status: determine_status(response_time, degraded_threshold_ms),
            success: true,
            message: "Reachable".to_string(),
            response_time_ms: Some(response_time),
            http_status: Some(status),
            model_used: String::new(),
            tested_at,
            retry_count: 0,
            error_category: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        },
        Err(message) => StreamCheckResult {
            status: HealthStatus::Failed,
            success: false,
            message,
            response_time_ms: Some(response_time),
            http_status: None,
            model_used: String::new(),
            tested_at,
            retry_count: 0,
            error_category: None,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        },
    }
}

fn determine_status(latency_ms: u64, threshold: u64) -> HealthStatus {
    if latency_ms <= threshold {
        HealthStatus::Operational
    } else {
        HealthStatus::Degraded
    }
}

fn should_retry(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("timeout") || lower.contains("abort") || lower.contains("timed out")
}

fn map_request_error(error: reqwest::Error) -> String {
    if error.is_timeout() {
        "Request timeout".to_string()
    } else if error.is_connect() {
        format!("Connection failed: {error}")
    } else {
        error.to_string()
    }
}

fn failed_result(message: impl Into<String>, retry_count: u32) -> StreamCheckResult {
    StreamCheckResult {
        status: HealthStatus::Failed,
        success: false,
        message: message.into(),
        response_time_ms: None,
        http_status: None,
        model_used: String::new(),
        tested_at: chrono::Utc::now().timestamp(),
        retry_count,
        error_category: None,
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reachability_origin_strips_api_path() {
        assert_eq!(
            reachability_origin("https://api.example.com/v1/messages"),
            "https://api.example.com"
        );
    }
}
