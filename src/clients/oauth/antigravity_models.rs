use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::domain::accounts::store::{gemini_v1internal_project_id, Account};
use crate::domain::antigravity_identity::{antigravity_client_identity, AntigravityClientIdentity};
use crate::domain::providers::model::ProviderType;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};

pub const ANTIGRAVITY_MODELS_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels";
const MAX_MODELS_RESPONSE_BODY_BYTES: usize = 8 * 1024 * 1024;
const MAX_STALE_CATALOG_AGE_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_CACHED_CATALOGS: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub struct AntigravityModelDescriptor {
    pub model_id: String,
    pub family: &'static str,
    pub display_name: Option<String>,
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<String>,
    pub supports_images: Option<bool>,
    pub supports_thinking: Option<bool>,
    pub supports_web_search: Option<bool>,
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
    pub request_revision: u64,
    pub client_identity: AntigravityClientIdentity,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct AntigravityModelCatalogFailure {
    pub status_code: u16,
    pub retryable: bool,
    superseded_identity: bool,
    message: String,
}

impl AntigravityModelCatalogFailure {
    pub fn is_unauthorized(&self) -> bool {
        self.status_code == 401
    }

    pub fn is_superseded_identity(&self) -> bool {
        self.superseded_identity
    }

    fn new(status_code: u16, retryable: bool, message: impl Into<String>) -> Self {
        Self {
            status_code,
            retryable,
            superseded_identity: false,
            message: message.into(),
        }
    }

    fn superseded_identity() -> Self {
        Self {
            status_code: 409,
            retryable: false,
            superseded_identity: true,
            message: "Antigravity model catalog request identity was superseded".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    provider_type: String,
    account_id: String,
    account_incarnation: u64,
    auth_identity_generation: u64,
    token_refresh_generation: u64,
    request_identity_fingerprint: [u8; 32],
    project_id: String,
    source_url: String,
    client_identity: AntigravityClientIdentity,
}

#[derive(Debug, Clone)]
struct CachedCatalog {
    descriptors: Arc<Vec<AntigravityModelDescriptor>>,
    source_url: String,
    fetched_at_ms: i64,
    request_revision: u64,
    _memory: Option<ResidentMemoryReservation>,
}

static REQUEST_REVISION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn request_identity_fingerprint(account: &Account) -> Option<[u8; 32]> {
    let bearer = account.access_token.as_deref()?.trim();
    if bearer.is_empty() {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(b"cc-switch-server:antigravity-model-request-identity:v1\0");
    digest.update((bearer.len() as u64).to_be_bytes());
    digest.update(bearer.as_bytes());
    digest.update([u8::from(account.needs_relogin)]);
    Some(digest.finalize().into())
}

#[derive(Default)]
struct CatalogCache {
    entries: BTreeMap<CacheKey, CachedCatalog>,
    // After a refusal, older in-flight results may still be used by their
    // original callers (whose state publication has its own revision fence),
    // but cannot reinstall a stale cache authorization. Fixed-size metadata
    // avoids an unaccounted negative-entry registry under memory pressure.
    admission_floor: u64,
}

impl std::ops::Deref for CatalogCache {
    type Target = BTreeMap<CacheKey, CachedCatalog>;
    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}
impl std::ops::DerefMut for CatalogCache {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

fn cache() -> &'static Mutex<CatalogCache> {
    static CACHE: OnceLock<Mutex<CatalogCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(CatalogCache::default()))
}

fn publish_catalog_with_parent(
    entries: &mut CatalogCache,
    key: CacheKey,
    catalog: &mut CachedCatalog,
    parent: &ResidentMemoryBudget,
) -> bool {
    let newer_published = entries.iter().any(|(candidate, cached)| {
        let same_account =
            candidate.provider_type == key.provider_type && candidate.account_id == key.account_id;
        let candidate_epoch = (
            candidate.account_incarnation,
            candidate.auth_identity_generation,
            candidate.token_refresh_generation,
        );
        let key_epoch = (
            key.account_incarnation,
            key.auth_identity_generation,
            key.token_refresh_generation,
        );
        same_account
            && (candidate_epoch > key_epoch
                || (candidate_epoch == key_epoch
                    && candidate == &key
                    && cached.request_revision > catalog.request_revision))
    });
    if newer_published {
        return false;
    }
    entries.retain(|candidate, _| {
        if candidate.provider_type != key.provider_type || candidate.account_id != key.account_id {
            return true;
        }
        let candidate_epoch = (
            candidate.account_incarnation,
            candidate.auth_identity_generation,
            candidate.token_refresh_generation,
        );
        let key_epoch = (
            key.account_incarnation,
            key.auth_identity_generation,
            key.token_refresh_generation,
        );
        candidate_epoch > key_epoch || (candidate_epoch == key_epoch && candidate != &key)
    });
    // `false` exclusively means identity/revision supersession. Optional
    // admission failure still returns the current fetched result to its caller.
    if catalog.request_revision < entries.admission_floor {
        return true;
    }
    let Some(memory) = parent.reserve(catalog_retained_bytes(&key, catalog)) else {
        entries.admission_floor = entries.admission_floor.max(catalog.request_revision);
        return true;
    };
    catalog._memory = Some(memory);
    entries.insert(key, catalog.clone());
    while entries.len() > MAX_CACHED_CATALOGS {
        let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, cached)| (cached.fetched_at_ms, cached.request_revision))
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        entries.remove(&oldest);
    }
    true
}

#[cfg(test)]
fn publish_catalog(entries: &mut CatalogCache, key: CacheKey, mut catalog: CachedCatalog) -> bool {
    publish_catalog_with_parent(entries, key, &mut catalog, &process_resident_memory())
}

fn prune_catalogs(entries: &mut BTreeMap<CacheKey, CachedCatalog>, now_ms: i64) {
    entries.retain(|_, cached| {
        now_ms.saturating_sub(cached.fetched_at_ms) <= MAX_STALE_CATALOG_AGE_MS
    });
}

fn catalog_retained_bytes(key: &CacheKey, catalog: &CachedCatalog) -> usize {
    let text = [
        &key.provider_type,
        &key.account_id,
        &key.project_id,
        &key.source_url,
        &key.client_identity.version,
        &key.client_identity.user_agent,
        &key.client_identity.generation,
        &catalog.source_url,
    ]
    .iter()
    .fold(0_usize, |bytes, value| {
        bytes.saturating_add(value.capacity())
    });
    let descriptors = catalog.descriptors.iter().fold(
        catalog
            .descriptors
            .capacity()
            .saturating_mul(std::mem::size_of::<AntigravityModelDescriptor>()),
        |bytes, descriptor| {
            bytes
                .saturating_add(descriptor.model_id.capacity())
                .saturating_add(descriptor.display_name.as_ref().map_or(0, String::capacity))
                .saturating_add(descriptor.reset_time.as_ref().map_or(0, String::capacity))
                .saturating_add(descriptor.supported_mime_types.iter().fold(
                    0_usize,
                    |bytes, (mime, _)| {
                        bytes.saturating_add(mime.capacity()).saturating_add(
                            (std::mem::size_of::<(String, bool)>()
                                + std::mem::size_of::<usize>() * 2)
                                * 12,
                        )
                    },
                ))
                .saturating_add(
                    descriptor.deprecated_aliases.iter().fold(
                        descriptor
                            .deprecated_aliases
                            .capacity()
                            .saturating_mul(std::mem::size_of::<String>()),
                        |bytes, alias| bytes.saturating_add(alias.capacity()),
                    ),
                )
        },
    );
    (std::mem::size_of::<(CacheKey, CachedCatalog)>() + std::mem::size_of::<usize>() * 2)
        .saturating_mul(12)
        .saturating_add(text)
        .saturating_add(descriptors)
        .saturating_add(
            std::mem::size_of::<Vec<AntigravityModelDescriptor>>()
                + std::mem::size_of::<usize>() * 2,
        )
}

pub async fn model_catalog(
    http: &reqwest::Client,
    account: &Account,
    account_incarnation: u64,
    timeout: Duration,
    endpoint_override: Option<&str>,
) -> Result<AntigravityModelCatalog, AntigravityModelCatalogFailure> {
    let client_identity = antigravity_client_identity();
    model_catalog_for_identity(
        http,
        account,
        account_incarnation,
        timeout,
        endpoint_override,
        client_identity,
    )
    .await
}

async fn model_catalog_for_identity(
    http: &reqwest::Client,
    account: &Account,
    account_incarnation: u64,
    timeout: Duration,
    endpoint_override: Option<&str>,
    client_identity: AntigravityClientIdentity,
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
        account_incarnation,
        auth_identity_generation: account.auth_identity_generation,
        token_refresh_generation: account.token_refresh_generation,
        request_identity_fingerprint: request_identity_fingerprint(account).ok_or_else(|| {
            AntigravityModelCatalogFailure::new(401, false, "bound account has no access token")
        })?,
        project_id: project_id.clone(),
        source_url: source_url.to_string(),
        client_identity,
    };
    let request_revision = REQUEST_REVISION.fetch_add(1, Ordering::Relaxed);
    let request = http
        .post(source_url.clone())
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .header(USER_AGENT, &key.client_identity.user_agent)
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
    if !key.client_identity.is_current() {
        return Err(AntigravityModelCatalogFailure::superseded_identity());
    }
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
    let mut current = CachedCatalog {
        descriptors: Arc::new(descriptors),
        source_url,
        fetched_at_ms,
        request_revision,
        _memory: None,
    };
    let replacement = {
        let mut entries = cache().lock().await;
        if !key.client_identity.is_current() {
            return Err(AntigravityModelCatalogFailure::superseded_identity());
        }
        prune_catalogs(&mut entries, fetched_at_ms);
        let published = publish_catalog_with_parent(
            &mut entries,
            key.clone(),
            &mut current,
            &process_resident_memory(),
        );
        if published {
            Ok(None)
        } else {
            entries
                .get(&key)
                .cloned()
                .map(Some)
                .ok_or_else(AntigravityModelCatalogFailure::superseded_identity)
        }
    };
    if let Some(cached) = replacement? {
        return Ok(AntigravityModelCatalog {
            descriptors: cached.descriptors.as_ref().clone(),
            source: "authenticated_fetch_available_models",
            source_url: cached.source_url,
            stale: false,
            fetched_at_ms: cached.fetched_at_ms,
            request_revision: cached.request_revision,
            client_identity: key.client_identity,
        });
    }
    Ok(AntigravityModelCatalog {
        descriptors: current.descriptors.as_ref().clone(),
        source: "authenticated_fetch_available_models",
        source_url: current.source_url,
        stale: false,
        fetched_at_ms,
        request_revision,
        client_identity: key.client_identity,
    })
}

async fn stale_or_failure(
    key: &CacheKey,
    status_code: u16,
    retryable: bool,
    message: String,
) -> Result<AntigravityModelCatalog, AntigravityModelCatalogFailure> {
    if !key.client_identity.is_current() {
        return Err(AntigravityModelCatalogFailure::superseded_identity());
    }
    if retryable {
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut entries = cache().lock().await;
        prune_catalogs(&mut entries, now_ms);
        let cached = entries.get(key).cloned().filter(|cached| {
            now_ms.saturating_sub(cached.fetched_at_ms) <= MAX_STALE_CATALOG_AGE_MS
        });
        if cached.is_none() {
            entries.remove(key);
        }
        drop(entries);
        if let Some(cached) = cached {
            if !key.client_identity.is_current() {
                return Err(AntigravityModelCatalogFailure::superseded_identity());
            }
            return Ok(AntigravityModelCatalog {
                descriptors: cached.descriptors.as_ref().clone(),
                source: "same_identity_cached_fetch_available_models",
                source_url: cached.source_url,
                stale: true,
                fetched_at_ms: cached.fetched_at_ms,
                request_revision: cached.request_revision,
                client_identity: key.client_identity.clone(),
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
            supports_web_search: optional_web_search_capability(object),
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

fn optional_web_search_capability(object: Option<&serde_json::Map<String, Value>>) -> Option<bool> {
    let object = object?;
    let values = [
        object.get("supportsWebSearch").and_then(Value::as_bool),
        object.get("supports_web_search").and_then(Value::as_bool),
        object.get("webSearchSupported").and_then(Value::as_bool),
        object
            .get("nativeCapabilities")
            .and_then(Value::as_object)
            .and_then(|capabilities| capabilities.get("webSearch"))
            .and_then(Value::as_bool),
    ]
    .into_iter()
    .flatten()
    .collect::<BTreeSet<_>>();
    (values.len() == 1).then(|| *values.first().expect("one capability value"))
}

fn optional_u64(object: Option<&serde_json::Map<String, Value>>, field: &str) -> Option<u64> {
    object?.get(field)?.as_u64()
}

#[cfg(test)]
#[path = "antigravity_models/resident_tests.rs"]
mod resident_tests;

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    #[tokio::test]
    async fn late_same_scope_success_returns_newer_published_catalog() {
        run_late_catalog_race(false, None).await;
    }

    #[tokio::test]
    async fn late_old_generation_success_fails_closed() {
        run_late_catalog_race(true, None).await;
    }

    #[tokio::test]
    async fn antigravity_r4_late_old_client_cannot_publish_into_new_generation() {
        let versions = crate::domain::antigravity_identity::TestAntigravityIdentity::new("2.2.1");
        versions
            .scope(run_late_catalog_race(false, Some(versions.clone())))
            .await;
    }

    async fn run_late_catalog_race(
        newer_epoch: bool,
        versions: Option<crate::domain::antigravity_identity::TestAntigravityIdentity>,
    ) {
        let first_seen = Arc::new(tokio::sync::Notify::new());
        let release_first = Arc::new(tokio::sync::Notify::new());
        let sequence = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(axum::routing::post({
            let first_seen = Arc::clone(&first_seen);
            let release_first = Arc::clone(&release_first);
            let sequence = Arc::clone(&sequence);
            move || {
                let first_seen = Arc::clone(&first_seen);
                let release_first = Arc::clone(&release_first);
                let index = sequence.fetch_add(1, Ordering::Relaxed);
                async move {
                    if index == 0 {
                        first_seen.notify_one();
                        release_first.notified().await;
                        axum::Json(json!({"models": {"gemini-old": {}}}))
                    } else {
                        axum::Json(json!({"models": {"gemini-new": {}}}))
                    }
                }
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let account: Account = serde_json::from_value(json!({
            "id": format!("ag-late-same-{}", REQUEST_REVISION.fetch_add(1, Ordering::Relaxed)),
            "providerType": "antigravity_oauth",
            "accessToken": "same-token",
            "authIdentityGeneration": 1,
            "tokenRefreshGeneration": 1,
            "raw": {"projectId": "same-project"}
        }))
        .unwrap();
        let source_url = format!("http://{address}/v1internal:fetchAvailableModels");
        let first = tokio::spawn({
            let account = account.clone();
            let source_url = source_url.clone();
            let versions = versions.clone();
            async move {
                let http = reqwest::Client::new();
                let request = model_catalog(
                    &http,
                    &account,
                    1,
                    Duration::from_secs(10),
                    Some(&source_url),
                );
                if let Some(versions) = versions {
                    versions.scope(request).await
                } else {
                    request.await
                }
            }
        });
        first_seen.notified().await;
        let mut newer_account = account.clone();
        if newer_epoch {
            newer_account.token_refresh_generation += 1;
            newer_account.access_token = Some("new-token".to_string());
        }
        if let Some(versions) = &versions {
            assert!(versions.update("2.3.4"));
        }
        let newer = model_catalog(
            &reqwest::Client::new(),
            &newer_account,
            1,
            Duration::from_secs(10),
            Some(&source_url),
        )
        .await
        .unwrap();
        release_first.notify_one();
        assert_eq!(newer.descriptors[0].model_id, "gemini-new");
        let late = first.await.unwrap();
        if newer_epoch || versions.is_some() {
            let error = late.unwrap_err();
            assert_eq!(error.status_code, 409);
            assert!(!error.retryable);
            assert!(error.is_superseded_identity());
        } else {
            let late = late.unwrap();
            assert_eq!(late.descriptors, newer.descriptors);
            assert_eq!(late.request_revision, newer.request_revision);
            assert_eq!(late.fetched_at_ms, newer.fetched_at_ms);
            assert!(!late.stale);
        }
        server.abort();
    }

    #[tokio::test]
    async fn antigravity_r4_catalog_snapshot_stale_and_empty_are_exactly_scoped() {
        use crate::domain::antigravity_identity::TestAntigravityIdentity;
        for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
            let versions = TestAntigravityIdentity::new("2.2.1");
            let failure = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let user_agents = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let app = axum::Router::new().fallback(axum::routing::post({
                let failure = Arc::clone(&failure);
                let user_agents = Arc::clone(&user_agents);
                move |headers: axum::http::HeaderMap| {
                    let failure = Arc::clone(&failure);
                    let user_agents = Arc::clone(&user_agents);
                    async move {
                        let ua = headers
                            .get(USER_AGENT)
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .to_string();
                        user_agents.lock().unwrap().push(ua.clone());
                        let status = if failure.load(Ordering::SeqCst) {
                            axum::http::StatusCode::SERVICE_UNAVAILABLE
                        } else {
                            axum::http::StatusCode::OK
                        };
                        let models = if ua.contains("/2.2.1 ") {
                            json!({"gemini-version-a":{}})
                        } else {
                            json!({})
                        };
                        (status, axum::Json(json!({"models":models})))
                    }
                }
            }));
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let source_url = format!("http://{address}/models");
            let account: Account = serde_json::from_value(json!({
                "id":format!("r4-version-{}-{address}",rail.as_str()),"providerType":rail.as_str(),
                "accessToken":"same-version-token","authIdentityGeneration":1,
                "tokenRefreshGeneration":1,"raw":{"projectId":"same-project"}
            }))
            .unwrap();
            versions
                .scope(async {
                    let http = reqwest::Client::new();
                    let fetch = |account: Account, url: String| {
                        let http = http.clone();
                        async move {
                            model_catalog(&http, &account, 1, Duration::from_secs(2), Some(&url))
                                .await
                        }
                    };
                    let first = fetch(account.clone(), source_url.clone()).await.unwrap();
                    assert!(!first.stale);
                    failure.store(true, Ordering::SeqCst);
                    assert!(
                        fetch(account.clone(), source_url.clone())
                            .await
                            .unwrap()
                            .stale
                    );
                    assert!(!versions.update("invalid version"));
                    assert!(
                        fetch(account.clone(), source_url.clone())
                            .await
                            .unwrap()
                            .stale
                    );
                    versions.update("2.3.4");
                    assert!(fetch(account.clone(), source_url.clone()).await.is_err());
                    let obsolete = model_catalog_for_identity(
                        &http,
                        &account,
                        1,
                        Duration::from_secs(2),
                        Some(&source_url),
                        first.client_identity.clone(),
                    )
                    .await
                    .unwrap_err();
                    assert!(obsolete.is_superseded_identity());
                    assert_eq!(
                        user_agents.lock().unwrap().last(),
                        Some(&first.client_identity.user_agent)
                    );
                    failure.store(false, Ordering::SeqCst);
                    let empty = fetch(account.clone(), source_url.clone()).await.unwrap();
                    assert!(!empty.stale);
                    assert!(empty.descriptors.is_empty());
                    assert_eq!(empty.client_identity, antigravity_client_identity());
                    failure.store(true, Ordering::SeqCst);
                    let stale = fetch(account.clone(), source_url.clone()).await.unwrap();
                    assert!(stale.stale && stale.descriptors.is_empty());
                    assert_eq!(stale.client_identity, empty.client_identity);
                    let mut other_project = account.clone();
                    other_project.raw = Some(json!({"projectId":"other-project"}));
                    assert!(fetch(other_project, source_url.clone()).await.is_err());
                    assert!(
                        fetch(account.clone(), format!("http://{address}/other-models"))
                            .await
                            .is_err()
                    );
                    let mut other_rail = account.clone();
                    other_rail.provider_type = if rail == ProviderType::AntigravityOAuth {
                        ProviderType::AgyOAuth
                    } else {
                        ProviderType::AntigravityOAuth
                    };
                    assert!(fetch(other_rail, source_url.clone()).await.is_err());
                    versions.update("2.2.1");
                    assert!(
                        fetch(account.clone(), source_url.clone()).await.is_err(),
                        "returning to version A must not resurrect its old generation"
                    );
                })
                .await;
            server.abort();
        }
    }

    #[test]
    fn upstream_http_409_is_not_a_local_identity_conflict() {
        let upstream = AntigravityModelCatalogFailure::new(409, false, "upstream conflict");
        assert!(!upstream.is_superseded_identity());
        assert!(AntigravityModelCatalogFailure::superseded_identity().is_superseded_identity());
    }

    #[tokio::test]
    async fn stale_catalog_never_crosses_same_generation_bearer_change() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().fallback(axum::routing::post(
            |headers: axum::http::HeaderMap| async move {
                if headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    == Some("Bearer first-token")
                {
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(json!({"models": {"gemini-3.1-pro": {}}})),
                    )
                } else {
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(json!({"error": "unavailable"})),
                    )
                }
            },
        ));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let account_id = format!(
            "ag-token-scope-{}",
            REQUEST_REVISION.fetch_add(1, Ordering::Relaxed)
        );
        let mut account: Account = serde_json::from_value(json!({
            "id": account_id,
            "providerType": "antigravity_oauth",
            "accessToken": "first-token",
            "authIdentityGeneration": 1,
            "tokenRefreshGeneration": 1,
            "raw": {"projectId": "same-project"}
        }))
        .unwrap();
        let source_url = format!("http://{address}/v1internal:fetchAvailableModels");
        let first = model_catalog(
            &reqwest::Client::new(),
            &account,
            1,
            Duration::from_secs(2),
            Some(&source_url),
        )
        .await
        .unwrap();
        assert!(!first.stale);
        assert_eq!(first.descriptors.len(), 1);
        account.access_token = Some("second-token".to_string());
        let second = model_catalog(
            &reqwest::Client::new(),
            &account,
            1,
            Duration::from_secs(2),
            Some(&source_url),
        )
        .await;
        assert!(
            second.is_err(),
            "a new bearer must not inherit the old catalog"
        );
        server.abort();
    }

    #[test]
    fn newer_credential_generation_wins_even_if_old_request_started_later() {
        let old = CacheKey {
            provider_type: ProviderType::AntigravityOAuth.as_str().to_string(),
            account_id: "generation-order".to_string(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            request_identity_fingerprint: [1; 32],
            project_id: "project".to_string(),
            source_url: "https://example.test/models".to_string(),
            client_identity: antigravity_client_identity(),
        };
        let new = CacheKey {
            token_refresh_generation: 2,
            ..old.clone()
        };
        let cached = |revision| CachedCatalog {
            descriptors: Arc::new(Vec::new()),
            _memory: None,
            source_url: old.source_url.clone(),
            fetched_at_ms: 1,
            request_revision: revision,
        };
        let mut entries = CatalogCache::default();
        assert!(publish_catalog(&mut entries, old.clone(), cached(20)));
        assert!(publish_catalog(&mut entries, new.clone(), cached(10)));
        assert!(!publish_catalog(&mut entries, old.clone(), cached(30)));
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key(&new));
        let another_route = CacheKey {
            source_url: "https://other.example.test/models".to_string(),
            ..new.clone()
        };
        assert!(publish_catalog(
            &mut entries,
            another_route.clone(),
            cached(5),
        ));
        assert!(entries.contains_key(&new));
        assert!(entries.contains_key(&another_route));
        let recreated = CacheKey {
            account_incarnation: 2,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            ..old.clone()
        };
        assert!(publish_catalog(&mut entries, recreated.clone(), cached(1)));
        assert!(!publish_catalog(&mut entries, old.clone(), cached(40)));
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key(&recreated));
    }

    #[test]
    fn same_generation_bearer_publications_do_not_evict_each_other() {
        let old = CacheKey {
            provider_type: ProviderType::AgyOAuth.as_str().to_string(),
            account_id: "same-generation-token-order".to_string(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            request_identity_fingerprint: [1; 32],
            project_id: "project".to_string(),
            source_url: "https://example.test/models".to_string(),
            client_identity: antigravity_client_identity(),
        };
        let new = CacheKey {
            request_identity_fingerprint: [2; 32],
            ..old.clone()
        };
        let cached = |request_revision| CachedCatalog {
            descriptors: Arc::new(Vec::new()),
            _memory: None,
            source_url: old.source_url.clone(),
            fetched_at_ms: 1,
            request_revision,
        };
        let mut entries = CatalogCache::default();
        assert!(publish_catalog(&mut entries, old.clone(), cached(10)));
        assert!(publish_catalog(&mut entries, new.clone(), cached(11)));
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key(&new));
        assert!(publish_catalog(&mut entries, old.clone(), cached(12)));
        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key(&new));
        assert!(entries.contains_key(&old));
    }

    #[test]
    fn cache_entry_count_is_bounded_across_same_generation_bearers() {
        let key = CacheKey {
            provider_type: ProviderType::AntigravityOAuth.as_str().to_string(),
            account_id: "many-credential-variants".to_string(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            request_identity_fingerprint: [0; 32],
            project_id: "project".to_string(),
            source_url: "https://example.test/models".to_string(),
            client_identity: antigravity_client_identity(),
        };
        let mut entries = CatalogCache::default();
        for index in 0..=MAX_CACHED_CATALOGS {
            let variant = CacheKey {
                request_identity_fingerprint: [index as u8; 32],
                ..key.clone()
            };
            assert!(publish_catalog(
                &mut entries,
                variant,
                CachedCatalog {
                    descriptors: Arc::new(Vec::new()),
                    _memory: None,
                    source_url: key.source_url.clone(),
                    fetched_at_ms: index as i64,
                    request_revision: index as u64,
                }
            ));
        }
        assert_eq!(entries.len(), MAX_CACHED_CATALOGS);
        assert!(!entries.contains_key(&key));
        assert!(entries.contains_key(&CacheKey {
            request_identity_fingerprint: [MAX_CACHED_CATALOGS as u8; 32],
            ..key
        }));
    }

    #[tokio::test]
    async fn expired_old_generation_cannot_delete_new_catalog() {
        let account_id = format!(
            "ag-cache-fence-{}",
            REQUEST_REVISION.fetch_add(1, Ordering::Relaxed)
        );
        let old = CacheKey {
            provider_type: ProviderType::AntigravityOAuth.as_str().to_string(),
            account_id: account_id.clone(),
            account_incarnation: 1,
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            request_identity_fingerprint: [1; 32],
            project_id: "project".to_string(),
            source_url: "https://example.test/models".to_string(),
            client_identity: antigravity_client_identity(),
        };
        let new = CacheKey {
            auth_identity_generation: 2,
            token_refresh_generation: 2,
            ..old.clone()
        };
        cache().lock().await.insert(
            new.clone(),
            CachedCatalog {
                descriptors: Arc::new(Vec::new()),
                _memory: None,
                source_url: new.source_url.clone(),
                fetched_at_ms: chrono::Utc::now().timestamp_millis(),
                request_revision: REQUEST_REVISION.fetch_add(1, Ordering::Relaxed),
            },
        );
        assert!(stale_or_failure(&old, 503, true, "old failure".to_string())
            .await
            .is_err());
        assert!(cache().lock().await.contains_key(&new));
        cache().lock().await.remove(&new);
    }

    #[test]
    fn parses_capabilities_families_zero_quota_and_deprecated_aliases() {
        let descriptors = parse_descriptors(&json!({
            "models": {
                "models/gemini-3.1-pro": {
                    "displayName": "Gemini 3.1 Pro",
                    "quotaInfo": {"remainingFraction": 0.0, "resetTime": "2026-09-01T00:00:00Z"},
                    "supportsImages": true,
                    "supportsThinking": true,
                    "nativeCapabilities": {"webSearch": true},
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
        assert_eq!(descriptors[1].supports_web_search, Some(true));
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

    #[test]
    fn web_search_capability_accepts_trusted_aliases_and_rejects_conflicts() {
        let aliases = parse_descriptors(&json!({
            "models": {
                "gemini-camel": {"supportsWebSearch": true},
                "gemini-snake": {"supports_web_search": false},
                "gemini-suffix": {"webSearchSupported": true}
            }
        }))
        .unwrap();
        assert_eq!(aliases[0].supports_web_search, Some(true));
        assert_eq!(aliases[1].supports_web_search, Some(false));
        assert_eq!(aliases[2].supports_web_search, Some(true));

        let nested = parse_descriptors(&json!({
            "models": {"gemini-test": {"nativeCapabilities": {"webSearch": false}}}
        }))
        .unwrap();
        assert_eq!(nested[0].supports_web_search, Some(false));

        let conflicted = parse_descriptors(&json!({
            "models": {"gemini-test": {
                "supportsWebSearch": true,
                "webSearchSupported": false
            }}
        }))
        .unwrap();
        assert_eq!(conflicted[0].supports_web_search, None);
    }
}
