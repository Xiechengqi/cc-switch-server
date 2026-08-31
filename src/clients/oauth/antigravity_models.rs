use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::domain::accounts::store::{gemini_v1internal_project_id, Account};
use crate::domain::providers::model::ProviderType;

pub const ANTIGRAVITY_MODELS_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
const MAX_MODELS_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_STALE_CATALOG_AGE_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityModelDescriptor {
    pub model_id: String,
    pub family: &'static str,
    pub display_name: Option<String>,
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<String>,
    pub supports_images: Option<bool>,
    pub supports_thinking: Option<bool>,
    pub thinking_budget: Option<u64>,
    pub recommended: Option<bool>,
    pub max_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supported_mime_types: BTreeMap<String, bool>,
    pub deprecated_aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityModelCatalog {
    pub descriptors: Vec<AntigravityModelDescriptor>,
    pub source: &'static str,
    pub source_url: String,
    pub stale: bool,
    pub fetched_at_ms: i64,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct AntigravityModelCatalogFailure {
    pub status_code: u16,
    pub retryable: bool,
    message: String,
}

impl AntigravityModelCatalogFailure {
    pub fn is_unauthorized(&self) -> bool {
        self.status_code == 401
    }

    fn new(status_code: u16, retryable: bool, message: impl Into<String>) -> Self {
        Self {
            status_code,
            retryable,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    provider_type: String,
    account_id: String,
    auth_identity_generation: u64,
}

#[derive(Debug, Clone)]
struct CachedCatalog {
    descriptors: Vec<AntigravityModelDescriptor>,
    source_url: String,
    fetched_at_ms: i64,
}

fn cache() -> &'static Mutex<BTreeMap<CacheKey, CachedCatalog>> {
    static CACHE: OnceLock<Mutex<BTreeMap<CacheKey, CachedCatalog>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub async fn model_catalog(
    http: &reqwest::Client,
    account: &Account,
    timeout: Duration,
    endpoint_override: Option<&str>,
) -> Result<AntigravityModelCatalog, AntigravityModelCatalogFailure> {
    if !matches!(
        account.provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    ) {
        return Err(AntigravityModelCatalogFailure::new(
            400,
            false,
            "model discovery requires an Antigravity or Agy account",
        ));
    }
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AntigravityModelCatalogFailure::new(401, false, "bound account has no access token")
        })?;
    let project_id = gemini_v1internal_project_id(account).ok_or_else(|| {
        AntigravityModelCatalogFailure::new(400, false, "bound account has no Code Assist project")
    })?;
    let source_url = endpoint_override.unwrap_or(ANTIGRAVITY_MODELS_URL);
    let source_url = reqwest::Url::parse(source_url).map_err(|error| {
        AntigravityModelCatalogFailure::new(
            400,
            false,
            format!("invalid Antigravity model catalog URL: {error}"),
        )
    })?;
    let key = CacheKey {
        provider_type: account.provider_type.as_str().to_string(),
        account_id: account.id.clone(),
        auth_identity_generation: account.auth_identity_generation,
    };
    let request = http
        .post(source_url.clone())
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .header(
            USER_AGENT,
            crate::provider_identity::antigravity_user_agent(),
        )
        .timeout(timeout)
        .json(&json!({"project": project_id}));
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => {
            return stale_or_failure(
                &key,
                502,
                true,
                format!("Antigravity model catalog request failed: {error}"),
            )
            .await;
        }
    };
    let status = response.status();
    let mut response = response;
    let body = match crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_MODELS_RESPONSE_BODY_BYTES,
    )
    .await
    {
        Ok(body) => body,
        Err(crate::infra::http::BoundedResponseBodyError::Request(error)) => {
            return stale_or_failure(
                &key,
                502,
                true,
                format!("Antigravity model catalog response read failed: {error}"),
            )
            .await;
        }
        Err(error @ crate::infra::http::BoundedResponseBodyError::TooLarge { .. }) => {
            return Err(AntigravityModelCatalogFailure::new(
                502,
                false,
                format!("Antigravity model catalog response was invalid: {error}"),
            ));
        }
    };
    if !status.is_success() {
        let status_code = status.as_u16();
        let retryable = status_code == 408 || status_code == 429 || status_code >= 500;
        let message = format!("Antigravity model catalog returned HTTP {status_code}");
        if retryable {
            return stale_or_failure(&key, status_code, true, message).await;
        }
        return Err(AntigravityModelCatalogFailure::new(
            status_code,
            false,
            message,
        ));
    }
    let raw = serde_json::from_slice::<Value>(&body).map_err(|error| {
        AntigravityModelCatalogFailure::new(
            502,
            false,
            format!("Antigravity model catalog JSON was invalid: {error}"),
        )
    })?;
    let descriptors = parse_descriptors(&raw)
        .map_err(|message| AntigravityModelCatalogFailure::new(502, false, message))?;
    let fetched_at_ms = chrono::Utc::now().timestamp_millis();
    let source_url = source_url.to_string();
    {
        let mut entries = cache().lock().await;
        entries.retain(|candidate, _| {
            candidate.provider_type != key.provider_type
                || candidate.account_id != key.account_id
                || candidate.auth_identity_generation == key.auth_identity_generation
        });
        entries.insert(
            key,
            CachedCatalog {
                descriptors: descriptors.clone(),
                source_url: source_url.clone(),
                fetched_at_ms,
            },
        );
    }
    Ok(AntigravityModelCatalog {
        descriptors,
        source: "authenticated_fetch_available_models",
        source_url,
        stale: false,
        fetched_at_ms,
    })
}

async fn stale_or_failure(
    key: &CacheKey,
    status_code: u16,
    retryable: bool,
    message: String,
) -> Result<AntigravityModelCatalog, AntigravityModelCatalogFailure> {
    if retryable {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut entries = cache().lock().await;
        let cached = entries.get(key).cloned().filter(|cached| {
            now_ms.saturating_sub(cached.fetched_at_ms) <= MAX_STALE_CATALOG_AGE_MS
        });
        if cached.is_none() {
            entries.remove(key);
        }
        drop(entries);
        if let Some(cached) = cached {
            return Ok(AntigravityModelCatalog {
                descriptors: cached.descriptors,
                source: "same_identity_cached_fetch_available_models",
                source_url: cached.source_url,
                stale: true,
                fetched_at_ms: cached.fetched_at_ms,
            });
        }
    }
    Err(AntigravityModelCatalogFailure::new(
        status_code,
        retryable,
        message,
    ))
}

fn parse_descriptors(raw: &Value) -> Result<Vec<AntigravityModelDescriptor>, String> {
    let models = raw
        .get("models")
        .and_then(Value::as_object)
        .ok_or_else(|| "Antigravity model catalog omitted the models object".to_string())?;
    let deprecated = deprecated_aliases(raw);
    let mut descriptors = BTreeMap::<String, AntigravityModelDescriptor>::new();
    for (raw_model_id, model) in models {
        let Some(model_id) = normalize_model_id(raw_model_id) else {
            continue;
        };
        let object = model.as_object();
        let quota = object
            .and_then(|object| object.get("quotaInfo"))
            .and_then(Value::as_object);
        let remaining_fraction = quota
            .and_then(|quota| quota.get("remainingFraction"))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && (0.0..=1.0).contains(value));
        let descriptor = AntigravityModelDescriptor {
            family: model_family(&model_id),
            display_name: optional_string(object, "displayName"),
            remaining_fraction,
            reset_time: quota
                .and_then(|quota| quota.get("resetTime"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            supports_images: optional_bool(object, "supportsImages"),
            supports_thinking: optional_bool(object, "supportsThinking"),
            thinking_budget: optional_u64(object, "thinkingBudget"),
            recommended: optional_bool(object, "recommended"),
            max_tokens: optional_u64(object, "maxTokens"),
            max_output_tokens: optional_u64(object, "maxOutputTokens"),
            supported_mime_types: object
                .and_then(|object| object.get("supportedMimeTypes"))
                .and_then(Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|(mime, value)| {
                            value.as_bool().map(|supported| (mime.clone(), supported))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            deprecated_aliases: deprecated.get(&model_id).cloned().unwrap_or_default(),
            model_id: model_id.clone(),
        };
        descriptors.insert(model_id, descriptor);
    }
    Ok(descriptors.into_values().collect())
}

fn deprecated_aliases(raw: &Value) -> BTreeMap<String, Vec<String>> {
    let mut aliases = BTreeMap::<String, BTreeSet<String>>::new();
    let Some(entries) = raw.get("deprecatedModelIds").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    for (old_id, value) in entries {
        let Some(old_id) = normalize_model_id(old_id) else {
            continue;
        };
        let Some(new_id) = value
            .get("newModelId")
            .and_then(Value::as_str)
            .and_then(normalize_model_id)
        else {
            continue;
        };
        aliases.entry(new_id).or_default().insert(old_id);
    }
    aliases
        .into_iter()
        .map(|(model, aliases)| (model, aliases.into_iter().collect()))
        .collect()
}

fn normalize_model_id(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value
        .rsplit_once("/models/")
        .map(|(_, model)| model)
        .unwrap_or_else(|| value.strip_prefix("models/").unwrap_or(value));
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    {
        return None;
    }
    Some(value.to_string())
}

fn model_family(model_id: &str) -> &'static str {
    let lower = model_id.to_ascii_lowercase();
    if lower.starts_with("gemini-") {
        "gemini"
    } else if lower.starts_with("claude-") {
        "claude"
    } else if lower.starts_with("gpt-")
        || lower.starts_with("openai-")
        || lower.starts_with("o1-")
        || lower.starts_with("o3-")
        || lower.starts_with("o4-")
    {
        "gpt"
    } else {
        "other"
    }
}

fn optional_string(object: Option<&serde_json::Map<String, Value>>, field: &str) -> Option<String> {
    object?
        .get(field)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn optional_bool(object: Option<&serde_json::Map<String, Value>>, field: &str) -> Option<bool> {
    object?.get(field)?.as_bool()
}

fn optional_u64(object: Option<&serde_json::Map<String, Value>>, field: &str) -> Option<u64> {
    object?.get(field)?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capabilities_families_zero_quota_and_deprecated_aliases() {
        let descriptors = parse_descriptors(&json!({
            "models": {
                "models/gemini-3.1-pro": {
                    "displayName": "Gemini 3.1 Pro",
                    "quotaInfo": {"remainingFraction": 0.0, "resetTime": "2026-09-01T00:00:00Z"},
                    "supportsImages": true,
                    "supportsThinking": true,
                    "thinkingBudget": 32768,
                    "maxTokens": 1048576,
                    "maxOutputTokens": 65536,
                    "supportedMimeTypes": {"image/png": true, "video/mp4": false}
                },
                "claude-sonnet-4-6": {"supportsThinking": true},
                "gpt-oss-120b-medium": {"recommended": true},
                "bad model id": {}
            },
            "deprecatedModelIds": {
                "gemini-old": {"newModelId": "gemini-3.1-pro"}
            }
        }))
        .unwrap();

        assert_eq!(descriptors.len(), 3);
        assert_eq!(descriptors[0].family, "claude");
        assert_eq!(descriptors[1].family, "gemini");
        assert_eq!(descriptors[1].remaining_fraction, Some(0.0));
        assert_eq!(descriptors[1].deprecated_aliases, ["gemini-old"]);
        assert_eq!(descriptors[2].family, "gpt");
    }

    #[test]
    fn successful_empty_catalog_is_authoritative() {
        assert!(parse_descriptors(&json!({"models": {}}))
            .unwrap()
            .is_empty());
        assert!(parse_descriptors(&json!({"data": []})).is_err());
    }
}
