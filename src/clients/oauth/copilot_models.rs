use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::Value;
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use super::copilot_device::{
    COPILOT_API_VERSION, COPILOT_EDITOR_VERSION, COPILOT_PLUGIN_VERSION, COPILOT_USER_AGENT,
};

pub const MODEL_CACHE_TTL_MS: i64 = 5 * 60 * 1000;
pub const STALE_MODEL_CACHE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_MODEL_CACHE_ENTRIES: usize = 32;
const MAX_MODEL_RESPONSE_BODY_BYTES: usize = 2 * 1024 * 1024;

// Compatibility evidence only. Live account discovery remains authoritative.
pub const PUBLIC_STATIC_MODEL_IDS: &[&str] = &[
    "claude-fable-5",
    "claude-haiku-4.5",
    "claude-opus-4.5",
    "claude-opus-4.7",
    "claude-opus-4.8",
    "claude-opus-4.8-fast",
    "claude-opus-5",
    "claude-sonnet-4.5",
    "claude-sonnet-4.6",
    "claude-sonnet-5",
    "gemini-3.1-pro-preview",
    "gemini-3.5-flash",
    "gpt-4-0125-preview",
    "gpt-4o-2024-11-20",
    "gpt-4o-mini",
    "gpt-5-mini",
    "gpt-5.3-codex",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.5",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "kimi-k2.7-code",
    "mai-code-1-flash",
    "oswe-vscode-prime",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CopilotModelCatalogKey {
    pub account_id: String,
    pub auth_identity_generation: u64,
    pub account_token_refresh_generation: u64,
    pub copilot_token_generation: u64,
    pub github_domain: String,
    pub api_origin: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotModelDescriptor {
    pub model_id: String,
    pub display_name: Option<String>,
    pub vendor: Option<String>,
    pub model_picker_enabled: Option<bool>,
    pub policy_state: Option<String>,
    pub preview: Option<bool>,
    pub model_type: Option<String>,
    pub supported_endpoints: Vec<String>,
    pub max_context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_tools: Option<bool>,
    pub supports_vision: Option<bool>,
    pub supports_reasoning: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopilotModelCatalog {
    pub models: Vec<String>,
    pub descriptors: Vec<CopilotModelDescriptor>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
    pub api_origin: Option<String>,
    pub github_domain: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedCopilotModelCatalog {
    descriptors: Vec<CopilotModelDescriptor>,
    fetched_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct CopilotModelCatalogCache {
    catalogs: RwLock<HashMap<CopilotModelCatalogKey, CachedCopilotModelCatalog>>,
    flights: Mutex<HashMap<CopilotModelCatalogKey, Arc<Mutex<()>>>>,
}

impl CopilotModelCatalogCache {
    pub async fn fresh(
        &self,
        key: &CopilotModelCatalogKey,
        now_ms: i64,
    ) -> Option<CopilotModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(key)
            .filter(|catalog| now_ms.saturating_sub(catalog.fetched_at_ms) < MODEL_CACHE_TTL_MS)
            .map(|catalog| cached_catalog(key, catalog, false))
    }

    pub async fn stale(
        &self,
        key: &CopilotModelCatalogKey,
        now_ms: i64,
    ) -> Option<CopilotModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(key)
            .filter(|catalog| {
                now_ms.saturating_sub(catalog.fetched_at_ms) < STALE_MODEL_CACHE_TTL_MS
            })
            .map(|catalog| cached_catalog(key, catalog, true))
    }

    pub async fn insert(
        &self,
        key: CopilotModelCatalogKey,
        descriptors: Vec<CopilotModelDescriptor>,
        fetched_at_ms: i64,
    ) -> CopilotModelCatalog {
        let api_origin = key.api_origin.clone();
        let github_domain = key.github_domain.clone();
        let cached = CachedCopilotModelCatalog {
            descriptors: descriptors.clone(),
            fetched_at_ms,
        };
        let mut catalogs = self.catalogs.write().await;
        catalogs.insert(key, cached);
        while catalogs.len() > MAX_MODEL_CACHE_ENTRIES {
            let Some(oldest) = catalogs
                .iter()
                .min_by_key(|(_, catalog)| catalog.fetched_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            catalogs.remove(&oldest);
        }
        CopilotModelCatalog {
            models: descriptors
                .iter()
                .map(|descriptor| descriptor.model_id.clone())
                .collect(),
            descriptors,
            source: "copilot_models_api",
            stale: false,
            fetched_at_ms: Some(fetched_at_ms),
            api_origin: Some(api_origin),
            github_domain: Some(github_domain),
        }
    }

    pub async fn lock(&self, key: &CopilotModelCatalogKey) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.flights.lock().await;
            flights.retain(|current, flight| current == key || Arc::strong_count(flight) > 1);
            flights
                .entry(key.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

fn cached_catalog(
    key: &CopilotModelCatalogKey,
    cached: &CachedCopilotModelCatalog,
    stale: bool,
) -> CopilotModelCatalog {
    CopilotModelCatalog {
        models: cached
            .descriptors
            .iter()
            .map(|descriptor| descriptor.model_id.clone())
            .collect(),
        descriptors: cached.descriptors.clone(),
        source: "copilot_account_cache",
        stale,
        fetched_at_ms: Some(cached.fetched_at_ms),
        api_origin: Some(key.api_origin.clone()),
        github_domain: Some(key.github_domain.clone()),
    }
}

#[derive(Debug, Clone)]
pub struct CopilotModelFetchError {
    pub status: Option<StatusCode>,
    pub message: String,
}

impl std::fmt::Display for CopilotModelFetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CopilotModelFetchError {}

impl CopilotModelFetchError {
    pub fn is_transient(&self) -> bool {
        self.status.is_none_or(|status| {
            status == StatusCode::REQUEST_TIMEOUT
                || status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error()
        })
    }
}

pub async fn fetch_copilot_models(
    http: &reqwest::Client,
    url: &str,
    copilot_token: &str,
    timeout: Duration,
) -> Result<Vec<CopilotModelDescriptor>, CopilotModelFetchError> {
    let mut response = http
        .get(url)
        .bearer_auth(copilot_token)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header("copilot-integration-id", "vscode-chat")
        .header("editor-version", COPILOT_EDITOR_VERSION)
        .header("editor-plugin-version", COPILOT_PLUGIN_VERSION)
        .header("user-agent", COPILOT_USER_AGENT)
        .header("openai-intent", "conversation-panel")
        .header("x-github-api-version", COPILOT_API_VERSION)
        .header("x-vscode-user-agent-library-version", "electron-fetch")
        .header("x-initiator", "user")
        .timeout(timeout)
        .send()
        .await
        .map_err(|error| CopilotModelFetchError {
            status: None,
            message: format!("Copilot model request failed: {error}"),
        })?;
    let status = response.status();
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_MODEL_RESPONSE_BODY_BYTES,
    )
    .await
    .map_err(|error| CopilotModelFetchError {
        status: Some(status),
        message: format!("Copilot model response could not be read: {error}"),
    })?;
    if !status.is_success() {
        return Err(CopilotModelFetchError {
            status: Some(status),
            message: format!("Copilot model request returned HTTP {status}"),
        });
    }
    let payload =
        serde_json::from_slice::<Value>(&body).map_err(|error| CopilotModelFetchError {
            status: Some(status),
            message: format!("Copilot model response is not valid JSON: {error}"),
        })?;
    if !payload.get("data").is_some_and(Value::is_array)
        && !payload.get("models").is_some_and(Value::is_array)
    {
        return Err(CopilotModelFetchError {
            status: Some(status),
            message: "Copilot model response does not contain a data/models array".to_string(),
        });
    }
    Ok(parse_copilot_models(&payload))
}

pub fn parse_copilot_models(payload: &Value) -> Vec<CopilotModelDescriptor> {
    let items = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.get("models").and_then(Value::as_array));
    let mut models = items
        .into_iter()
        .flatten()
        .filter_map(parse_copilot_model_descriptor)
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.model_id.cmp(&right.model_id));
    models.dedup_by(|left, right| left.model_id == right.model_id);
    models
}

fn parse_copilot_model_descriptor(item: &Value) -> Option<CopilotModelDescriptor> {
    let model_id = model_identifier(item)?;
    let capabilities = item.get("capabilities").filter(|value| value.is_object());
    let model_picker_enabled = item.get("model_picker_enabled").and_then(Value::as_bool);
    if model_picker_enabled == Some(false) {
        return None;
    }
    let policy_state = item
        .pointer("/policy/state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|state| !state.is_empty())
        .map(str::to_string);
    if policy_state.as_deref().is_some_and(|state| {
        !state.eq_ignore_ascii_case("enabled") && !state.eq_ignore_ascii_case("preview")
    }) {
        return None;
    }
    let model_type = capabilities
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if model_type
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case("chat"))
    {
        return None;
    }
    let supported_endpoints = string_array(
        item.get("supported_endpoints")
            .or_else(|| capabilities.and_then(|value| value.get("supported_endpoints"))),
    );
    if !supported_endpoints.is_empty()
        && !supported_endpoints.iter().any(|endpoint| {
            endpoint.contains("/chat/completions")
                || endpoint.contains("/responses")
                || endpoint.contains("/v1/messages")
        })
    {
        return None;
    }
    if model_type.is_none() && supported_endpoints.is_empty() {
        let normalized = model_id.to_ascii_lowercase();
        if normalized.contains("embedding") || normalized == "gpt-41-copilot" {
            return None;
        }
    }
    let preview = item
        .get("preview")
        .or_else(|| item.get("is_preview"))
        .and_then(Value::as_bool)
        .or_else(|| {
            policy_state
                .as_deref()
                .map(|state| state.eq_ignore_ascii_case("preview"))
        });
    Some(CopilotModelDescriptor {
        display_name: first_non_empty_string(item, &["name", "display_name", "label"])
            .filter(|name| name != &model_id),
        vendor: first_non_empty_string(item, &["vendor", "owned_by", "provider"]),
        model_picker_enabled,
        policy_state,
        preview,
        model_type,
        supported_endpoints,
        max_context_window_tokens: first_u64(
            item,
            &[
                "/capabilities/limits/max_context_window_tokens",
                "/capabilities/limits/max_context_tokens",
            ],
        ),
        max_output_tokens: first_u64(
            item,
            &[
                "/capabilities/limits/max_output_tokens",
                "/capabilities/limits/max_completion_tokens",
            ],
        ),
        supports_tools: first_bool(
            item,
            &[
                "/capabilities/supports/tool_calls",
                "/capabilities/supports/tools",
                "/capabilities/supports_tools",
            ],
        ),
        supports_vision: first_bool(
            item,
            &[
                "/capabilities/supports/vision",
                "/capabilities/supports/image_input",
                "/capabilities/supports_vision",
            ],
        ),
        supports_reasoning: first_bool(
            item,
            &[
                "/capabilities/supports/reasoning",
                "/capabilities/supports/thinking",
                "/capabilities/supports_reasoning",
            ],
        ),
        model_id,
    })
}

fn model_identifier(value: &Value) -> Option<String> {
    for name in ["id", "model", "name"] {
        if let Some(raw) = value.get(name) {
            return raw
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
        }
    }
    None
}

fn first_non_empty_string(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn string_array(value: Option<&Value>) -> Vec<String> {
    let mut values = value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn first_u64(value: &Value, pointers: &[&str]) -> Option<u64> {
    pointers.iter().find_map(|pointer| {
        value.pointer(pointer).and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        })
    })
}

fn first_bool(value: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_bool))
}

pub fn public_static_catalog() -> CopilotModelCatalog {
    let descriptors = PUBLIC_STATIC_MODEL_IDS
        .iter()
        .map(|model| CopilotModelDescriptor {
            model_id: (*model).to_string(),
            display_name: None,
            vendor: Some("github".to_string()),
            model_picker_enabled: None,
            policy_state: None,
            preview: None,
            model_type: Some("chat".to_string()),
            supported_endpoints: Vec::new(),
            max_context_window_tokens: None,
            max_output_tokens: None,
            supports_tools: None,
            supports_vision: None,
            supports_reasoning: None,
        })
        .collect::<Vec<_>>();
    CopilotModelCatalog {
        models: descriptors
            .iter()
            .map(|descriptor| descriptor.model_id.clone())
            .collect(),
        descriptors,
        source: "copilot_public_static_compatibility",
        stale: true,
        fetched_at_ms: None,
        api_origin: None,
        github_domain: Some("github.com".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parser_accepts_public_and_enterprise_shapes_and_deduplicates() {
        let public = parse_copilot_models(&json!({
            "data": [
                {"id": "claude-sonnet-5"},
                {"id": " claude-sonnet-5 "},
                {"model": "gpt-enterprise"},
                {"id": ""},
                {"name": "ignored-when-id-is-empty", "id": ""}
            ]
        }));
        assert_eq!(
            public
                .iter()
                .map(|descriptor| descriptor.model_id.as_str())
                .collect::<Vec<_>>(),
            ["claude-sonnet-5", "gpt-enterprise"]
        );
        let enterprise = parse_copilot_models(&json!({"models": [{"name": "ghe-model"}]}));
        assert_eq!(enterprise[0].model_id, "ghe-model");
    }

    #[test]
    fn parser_is_capability_and_policy_driven_and_preserves_safe_metadata() {
        let models = parse_copilot_models(&json!({
            "data": [
                {
                    "id": "gpt-entitled",
                    "name": "GPT Entitled",
                    "vendor": "openai",
                    "model_picker_enabled": true,
                    "preview": true,
                    "policy": {"state": "enabled"},
                    "supported_endpoints": ["/responses", "/chat/completions", "/responses"],
                    "capabilities": {
                        "type": "chat",
                        "limits": {
                            "max_context_window_tokens": 128000,
                            "max_output_tokens": 16384
                        },
                        "supports": {"tool_calls": true, "vision": false, "reasoning": true}
                    }
                },
                {"id": "disabled-picker", "model_picker_enabled": false, "capabilities": {"type": "chat"}},
                {"id": "disabled-policy", "policy": {"state": "disabled"}, "capabilities": {"type": "chat"}},
                {"id": "embedding", "capabilities": {"type": "embeddings"}},
                {"id": "completion-only", "supported_endpoints": ["/completions"]}
            ]
        }));
        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert_eq!(model.model_id, "gpt-entitled");
        assert_eq!(model.display_name.as_deref(), Some("GPT Entitled"));
        assert_eq!(model.vendor.as_deref(), Some("openai"));
        assert_eq!(model.policy_state.as_deref(), Some("enabled"));
        assert_eq!(model.preview, Some(true));
        assert_eq!(
            model.supported_endpoints,
            ["/chat/completions", "/responses"]
        );
        assert_eq!(model.max_context_window_tokens, Some(128_000));
        assert_eq!(model.max_output_tokens, Some(16_384));
        assert_eq!(model.supports_tools, Some(true));
        assert_eq!(model.supports_vision, Some(false));
        assert_eq!(model.supports_reasoning, Some(true));
    }

    #[tokio::test]
    async fn model_request_uses_short_lived_bearer_and_official_identity_headers() {
        use axum::http::HeaderMap;
        use axum::routing::get;
        use axum::Router;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/models",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer short-lived-copilot-token")
                );
                assert_eq!(
                    headers
                        .get("editor-version")
                        .and_then(|value| value.to_str().ok()),
                    Some(COPILOT_EDITOR_VERSION)
                );
                assert_eq!(
                    headers
                        .get("editor-plugin-version")
                        .and_then(|value| value.to_str().ok()),
                    Some(COPILOT_PLUGIN_VERSION)
                );
                assert_eq!(
                    headers
                        .get("copilot-integration-id")
                        .and_then(|value| value.to_str().ok()),
                    Some("vscode-chat")
                );
                axum::Json(json!({
                    "data": [
                        {"id": "model-b"},
                        {"id": "model-a"},
                        {"id": "model-b"}
                    ]
                }))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let models = fetch_copilot_models(
            &reqwest::Client::new(),
            &format!("http://{address}/models"),
            "short-lived-copilot-token",
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|descriptor| descriptor.model_id.as_str())
                .collect::<Vec<_>>(),
            ["model-a", "model-b"]
        );
        server.abort();
    }

    #[tokio::test]
    async fn successful_empty_catalog_is_authoritative_and_bad_shape_is_rejected() {
        use axum::routing::get;
        use axum::Router;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/empty", get(|| async { axum::Json(json!({"data": []})) }))
            .route("/bad", get(|| async { axum::Json(json!({"items": []})) }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let http = reqwest::Client::new();
        let empty = fetch_copilot_models(
            &http,
            &format!("http://{address}/empty"),
            "token",
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(empty.is_empty());
        let bad = fetch_copilot_models(
            &http,
            &format!("http://{address}/bad"),
            "token",
            Duration::from_secs(2),
        )
        .await
        .unwrap_err();
        assert!(!bad.is_transient());
        server.abort();
    }

    #[test]
    fn only_network_timeout_rate_limit_and_server_errors_are_transient() {
        for status in [
            None,
            Some(StatusCode::REQUEST_TIMEOUT),
            Some(StatusCode::TOO_MANY_REQUESTS),
            Some(StatusCode::BAD_GATEWAY),
        ] {
            assert!(CopilotModelFetchError {
                status,
                message: "transient".to_string()
            }
            .is_transient());
        }
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::BAD_REQUEST,
            StatusCode::OK,
        ] {
            assert!(!CopilotModelFetchError {
                status: Some(status),
                message: "terminal".to_string()
            }
            .is_transient());
        }
    }

    #[tokio::test]
    async fn cache_is_scoped_by_every_credential_and_origin_dimension() {
        let cache = CopilotModelCatalogCache::default();
        let key = CopilotModelCatalogKey {
            account_id: "account-a".to_string(),
            auth_identity_generation: 1,
            account_token_refresh_generation: 1,
            copilot_token_generation: 2,
            github_domain: "github.com".to_string(),
            api_origin: "https://api.githubcopilot.com".to_string(),
        };
        cache
            .insert(
                key.clone(),
                parse_copilot_models(&json!({"data": [{"id": "model-a"}]})),
                1_000,
            )
            .await;
        assert_eq!(cache.fresh(&key, 1_001).await.unwrap().models, ["model-a"]);

        for changed in [
            CopilotModelCatalogKey {
                account_id: "account-b".to_string(),
                ..key.clone()
            },
            CopilotModelCatalogKey {
                auth_identity_generation: 2,
                ..key.clone()
            },
            CopilotModelCatalogKey {
                account_token_refresh_generation: 2,
                ..key.clone()
            },
            CopilotModelCatalogKey {
                copilot_token_generation: 3,
                ..key.clone()
            },
            CopilotModelCatalogKey {
                github_domain: "ghe.example.com".to_string(),
                ..key.clone()
            },
            CopilotModelCatalogKey {
                api_origin: "https://api.business.githubcopilot.com".to_string(),
                ..key.clone()
            },
        ] {
            assert!(cache.fresh(&changed, 1_001).await.is_none());
            assert!(cache.stale(&changed, 1_001).await.is_none());
        }
    }
}
