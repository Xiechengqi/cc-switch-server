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
pub struct CopilotModelCatalog {
    pub models: Vec<String>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
    pub api_origin: Option<String>,
}

#[derive(Debug, Clone)]
struct CachedCopilotModelCatalog {
    models: Vec<String>,
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
            .map(|catalog| CopilotModelCatalog {
                models: catalog.models.clone(),
                source: "copilot_account_cache",
                stale: false,
                fetched_at_ms: Some(catalog.fetched_at_ms),
                api_origin: Some(key.api_origin.clone()),
            })
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
            .map(|catalog| CopilotModelCatalog {
                models: catalog.models.clone(),
                source: "copilot_account_cache",
                stale: true,
                fetched_at_ms: Some(catalog.fetched_at_ms),
                api_origin: Some(key.api_origin.clone()),
            })
    }

    pub async fn insert(
        &self,
        key: CopilotModelCatalogKey,
        models: Vec<String>,
        fetched_at_ms: i64,
    ) -> CopilotModelCatalog {
        let api_origin = key.api_origin.clone();
        let cached = CachedCopilotModelCatalog {
            models: models.clone(),
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
            models,
            source: "copilot_models_api",
            stale: false,
            fetched_at_ms: Some(fetched_at_ms),
            api_origin: Some(api_origin),
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

pub async fn fetch_copilot_models(
    http: &reqwest::Client,
    url: &str,
    copilot_token: &str,
    timeout: Duration,
) -> Result<Vec<String>, CopilotModelFetchError> {
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
    let models = parse_copilot_models(&payload);
    if models.is_empty() {
        return Err(CopilotModelFetchError {
            status: Some(status),
            message: "Copilot model response contains no usable model IDs".to_string(),
        });
    }
    Ok(models)
}

pub fn parse_copilot_models(payload: &Value) -> Vec<String> {
    let items = payload
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| payload.get("models").and_then(Value::as_array));
    let mut models = items
        .into_iter()
        .flatten()
        .filter_map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("model").and_then(Value::as_str))
                .or_else(|| item.get("name").and_then(Value::as_str))
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        })
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

pub fn public_static_catalog() -> CopilotModelCatalog {
    CopilotModelCatalog {
        models: PUBLIC_STATIC_MODEL_IDS
            .iter()
            .map(|model| (*model).to_string())
            .collect(),
        source: "copilot_public_static_compatibility",
        stale: true,
        fetched_at_ms: None,
        api_origin: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parser_accepts_public_and_enterprise_shapes_and_deduplicates() {
        assert_eq!(
            parse_copilot_models(&json!({
                "data": [
                    {"id": "claude-sonnet-5"},
                    {"id": " claude-sonnet-5 "},
                    {"model": "gpt-enterprise"},
                    {"id": ""},
                    {"name": "ignored-when-id-is-empty", "id": ""}
                ]
            })),
            ["claude-sonnet-5", "gpt-enterprise"]
        );
        assert_eq!(
            parse_copilot_models(&json!({"models": [{"name": "ghe-model"}]})),
            ["ghe-model"]
        );
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
        assert_eq!(models, ["model-a", "model-b"]);
        server.abort();
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
            .insert(key.clone(), vec!["model-a".to_string()], 1_000)
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
