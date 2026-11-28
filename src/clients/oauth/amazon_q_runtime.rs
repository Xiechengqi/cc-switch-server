use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::domain::accounts::store::Account;
pub(crate) use crate::domain::providers::amazon_q::{
    AMAZON_Q_LIST_MODELS_TARGET, AMAZON_Q_ORIGIN, AMAZON_Q_RUNTIME_REGIONS, AMAZON_Q_USAGE_TARGET,
};
use crate::domain::providers::model::ProviderType;
use crate::infra::resident_flights::CatalogFetchMemory;
use crate::infra::resident_memory::{ResidentMemoryBudget, ResidentMemoryReservation};
use crate::infra::time::now_ms;

#[cfg(test)]
mod r7_tests;
mod resident;

const CACHE_TTL_MS: i64 = 5 * 60 * 1000;
const STALE_CACHE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PAGES: usize = 16;
const MAX_MODELS: usize = 1_000;
const DEFAULT_PAGE_SIZE: u64 = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AmazonQModelDescriptor {
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_input_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_prompt_cache: Option<bool>,
}

#[derive(Debug)]
pub struct AmazonQModelCatalogData {
    pub descriptors: Vec<AmazonQModelDescriptor>,
    pub default_model_id: Option<String>,
    retained_bytes: usize,
    _memory: Option<ResidentMemoryReservation>,
}

#[derive(Debug, Clone)]
pub struct AmazonQModelCatalog {
    data: Arc<AmazonQModelCatalogData>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
    failure_status: Option<reqwest::StatusCode>,
    capacity_rejected: bool,
}

impl std::ops::Deref for AmazonQModelCatalog {
    type Target = AmazonQModelCatalogData;
    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl AmazonQModelCatalog {
    pub(crate) fn is_capacity_rejected(&self) -> bool {
        self.capacity_rejected
    }

    pub fn is_unauthorized(&self) -> bool {
        matches!(
            self.failure_status,
            Some(reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN)
        )
    }

    pub fn model(&self, model_id: &str) -> Option<&AmazonQModelDescriptor> {
        self.descriptors
            .iter()
            .find(|model| model.model_id.eq_ignore_ascii_case(model_id))
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct AmazonQModelCatalogScope {
    account_incarnation: u64,
    app: String,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
}

impl AmazonQModelCatalogScope {
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_incarnation: u64,
    ) -> Self {
        Self {
            account_incarnation,
            app: app.trim().to_string(),
            provider_id: provider_id.trim().to_string(),
            provider_revision,
            runtime_fingerprint: runtime_fingerprint.trim().to_string(),
        }
    }

    #[cfg(test)]
    fn fixture() -> Self {
        Self::derive("claude", "amazon-q-fixture", 1, "runtime-fixture", 1)
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    account_incarnation: u64,
    app: String,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
    account_id: String,
    auth_identity_generation: u64,
    token_refresh_generation: u64,
    credential_fingerprint: [u8; 32],
    profile_scope: String,
    runtime_region: String,
}

static CACHE: LazyLock<resident::RuntimeCache> = LazyLock::new(resident::RuntimeCache::default);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    Transient,
    Unavailable,
    Capacity,
    Invalidated,
}

#[derive(Debug, Clone)]
struct DiscoveryFailure {
    kind: FailureKind,
    message: String,
    status: Option<reqwest::StatusCode>,
}

impl DiscoveryFailure {
    fn capacity() -> Self {
        Self {
            kind: FailureKind::Capacity,
            message: "Amazon Q catalog memory or flight capacity exhausted".into(),
            status: None,
        }
    }
    fn invalidated() -> Self {
        Self {
            kind: FailureKind::Invalidated,
            message: "Amazon Q catalog identity invalidated during discovery".into(),
            status: None,
        }
    }

    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Transient,
            message: message.into(),
            status: None,
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Unavailable,
            message: message.into(),
            status: None,
        }
    }

    fn with_status(
        kind: FailureKind,
        status: reqwest::StatusCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            message: message.into(),
            status: Some(status),
        }
    }
}

impl std::fmt::Display for DiscoveryFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub async fn model_catalog_scoped(
    http: &reqwest::Client,
    account: &Account,
    scope: &AmazonQModelCatalogScope,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> AmazonQModelCatalog {
    model_catalog_cached(
        &CACHE,
        http,
        account,
        scope,
        endpoint_override,
        request_timeout,
    )
    .await
}

async fn model_catalog_cached(
    cache: &resident::RuntimeCache,
    http: &reqwest::Client,
    account: &Account,
    scope: &AmazonQModelCatalogScope,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> AmazonQModelCatalog {
    if account.provider_type != ProviderType::AmazonQOAuth {
        return unavailable_catalog("amazon_q_account_type_mismatch", None);
    }
    let access_token = match account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => value,
        None => return unavailable_catalog("amazon_q_credential_unavailable", None),
    };
    let identity = match runtime_identity(account) {
        Ok(identity) => identity,
        Err(error) => {
            tracing::warn!(account_id=%account.id,%error,"Amazon Q runtime identity rejected");
            return unavailable_catalog("amazon_q_identity_unresolved", None);
        }
    };
    let key = CacheKey {
        account_incarnation: scope.account_incarnation,
        app: scope.app.clone(),
        provider_id: scope.provider_id.clone(),
        provider_revision: scope.provider_revision,
        runtime_fingerprint: scope.runtime_fingerprint.clone(),
        account_id: account.id.clone(),
        auth_identity_generation: account.auth_identity_generation,
        token_refresh_generation: account.token_refresh_generation,
        credential_fingerprint: {
            use sha2::{Digest, Sha256};
            Sha256::digest(access_token.as_bytes()).into()
        },
        profile_scope: identity
            .profile_arn
            .clone()
            .unwrap_or_else(|| "builder_id_profileless".into()),
        runtime_region: identity.region.clone(),
    };
    let deadline = tokio::time::Instant::now() + request_timeout;
    let current_time = || now_ms().min(i64::MAX as u128) as i64;
    if let Some(snapshot) = cache.snapshot(&key, current_time(), CACHE_TTL_MS).await {
        return catalog_from_snapshot(snapshot, false, "amazon_q_account_cache");
    }
    let Some(lease) = cache.flight(&key).await else {
        return catalog_from_failure(cache, &key, DiscoveryFailure::capacity(), current_time())
            .await;
    };
    let _flight_guard = match tokio::time::timeout_at(deadline, lease.lock()).await {
        Ok(guard) => guard,
        Err(_) => {
            return catalog_from_failure(
                cache,
                &key,
                DiscoveryFailure::transient(
                    "Amazon Q catalog deadline exceeded waiting for discovery",
                ),
                current_time(),
            )
            .await
        }
    };
    if !lease.is_current() {
        return catalog_from_failure(cache, &key, DiscoveryFailure::invalidated(), current_time())
            .await;
    }
    let outcome = if let Some(outcome) = lease.outcome() {
        outcome
    } else {
        if let Some(snapshot) = cache.snapshot(&key, current_time(), CACHE_TTL_MS).await {
            return catalog_from_snapshot(snapshot, false, "amazon_q_account_cache");
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let mut outcome = match tokio::time::timeout_at(
            deadline,
            fetch_catalog(
                http,
                access_token,
                &identity,
                endpoint_override,
                remaining,
                &cache.parent,
            ),
        )
        .await
        {
            Ok(Ok(data)) => Ok(resident::CatalogSnapshot {
                data,
                fetched_at_ms: current_time(),
            }),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(DiscoveryFailure::transient(
                "Amazon Q catalog pagination exceeded the discovery deadline",
            )),
        };
        if !lease.is_current() {
            outcome = Err(DiscoveryFailure::invalidated());
        }
        if let Err(error) = &outcome {
            // One sanitized diagnostic per leader, rather than one per waiter.
            // Do not include tokens, upstream bodies or pagination cursors.
            tracing::warn!(account_id=%account.id, kind=?error.kind,
                status=?error.status, "Amazon Q model discovery did not produce a catalog");
        }
        if let Ok(snapshot) = &outcome {
            cache.publish(key.clone(), snapshot, &lease).await;
        }
        if !lease.is_current() {
            outcome = Err(DiscoveryFailure::invalidated());
        }
        // Concurrent callers share failures as well as successful catalogs.
        // Last-lease drop clears this result, so no permanent negative cache.
        lease.complete(outcome.clone());
        outcome
    };
    if !lease.is_current() {
        return unavailable_catalog("amazon_q_catalog_invalidated", None);
    }
    match outcome {
        Ok(snapshot) => catalog_from_snapshot(snapshot, false, "amazon_q_list_available_models"),
        Err(error) => catalog_from_failure(cache, &key, error, current_time()).await,
    }
}

async fn catalog_from_failure(
    cache: &resident::RuntimeCache,
    key: &CacheKey,
    error: DiscoveryFailure,
    now: i64,
) -> AmazonQModelCatalog {
    match error.kind {
        FailureKind::Transient => {
            if let Some(snapshot) = cache.snapshot(key, now, STALE_CACHE_TTL_MS).await {
                return catalog_from_snapshot(snapshot, true, "amazon_q_account_cache");
            }
            unavailable_catalog(
                "amazon_q_model_discovery_transient_without_cache",
                error.status,
            )
        }
        FailureKind::Capacity => {
            crate::metrics::record_resident_retention_refused("amazon_q_catalog");
            let mut result = unavailable_catalog("amazon_q_models_capacity_rejected", None);
            result.capacity_rejected = true;
            result
        }
        FailureKind::Invalidated => unavailable_catalog("amazon_q_catalog_invalidated", None),
        FailureKind::Unavailable => {
            cache.discard(key).await;
            unavailable_catalog("amazon_q_model_access_unavailable", error.status)
        }
    }
}

#[cfg(test)]
pub async fn model_catalog(
    http: &reqwest::Client,
    account: &Account,
    endpoint_override: Option<&str>,
) -> AmazonQModelCatalog {
    model_catalog_scoped(
        http,
        account,
        &AmazonQModelCatalogScope::fixture(),
        endpoint_override,
        Duration::from_secs(10),
    )
    .await
}

pub async fn usage_snapshot(
    http: &reqwest::Client,
    account: &Account,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> Result<Value, String> {
    if account.provider_type != ProviderType::AmazonQOAuth {
        return Err("Amazon Q usage requires amazon_q_oauth account".to_string());
    }
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Amazon Q access token is unavailable".to_string())?;
    let identity = runtime_identity(account).map_err(|error| error.to_string())?;
    let endpoint =
        runtime_endpoint(&identity.region, endpoint_override).map_err(|error| error.to_string())?;
    let mut body = json!({"origin": AMAZON_Q_ORIGIN, "isEmailRequired": true});
    if let Some(profile_arn) = identity.profile_arn.as_deref() {
        body["profileArn"] = json!(profile_arn);
    }
    let response = send_operation(
        http,
        &endpoint,
        access_token,
        AMAZON_Q_USAGE_TARGET,
        &body,
        request_timeout,
    )
    .await
    .map_err(|error| error.to_string())?;
    if !response.status.is_success() {
        return Err(format!(
            "Amazon Q GetUsageLimits returned {}",
            response.status
        ));
    }
    serde_json::from_slice(&response.body)
        .map_err(|error| format!("parse Amazon Q GetUsageLimits: {error}"))
}

pub async fn clear_account(account_id: &str) {
    CACHE.clear_account(account_id).await;
}

pub(crate) fn catalog_identity_matches(left: &Account, right: &Account) -> bool {
    left.provider_type == right.provider_type
        && left.auth_identity_generation == right.auth_identity_generation
        && left.token_refresh_generation == right.token_refresh_generation
        && left.access_token == right.access_token
        && left.needs_relogin == right.needs_relogin
        && runtime_identity(left).ok() == runtime_identity(right).ok()
}

fn catalog_from_snapshot(
    snapshot: resident::CatalogSnapshot,
    stale: bool,
    source: &'static str,
) -> AmazonQModelCatalog {
    AmazonQModelCatalog {
        data: snapshot.data,
        source,
        stale,
        fetched_at_ms: Some(snapshot.fetched_at_ms),
        failure_status: None,
        capacity_rejected: false,
    }
}

fn unavailable_catalog(
    source: &'static str,
    failure_status: Option<reqwest::StatusCode>,
) -> AmazonQModelCatalog {
    static EMPTY: LazyLock<Arc<AmazonQModelCatalogData>> = LazyLock::new(|| {
        Arc::new(AmazonQModelCatalogData {
            descriptors: Vec::new(),
            default_model_id: None,
            retained_bytes: 0,
            _memory: None,
        })
    });
    AmazonQModelCatalog {
        data: EMPTY.clone(),
        source,
        stale: false,
        fetched_at_ms: None,
        failure_status,
        capacity_rejected: false,
    }
}

pub fn unavailable_model_catalog(source: &'static str) -> AmazonQModelCatalog {
    unavailable_catalog(source, None)
}

pub fn model_discovery_url(account: &Account) -> Result<String, String> {
    if account.provider_type != ProviderType::AmazonQOAuth {
        return Err("Amazon Q model discovery requires amazon_q_oauth account".to_string());
    }
    let identity = runtime_identity(account).map_err(|error| error.to_string())?;
    runtime_endpoint(&identity.region, None).map_err(|error| error.to_string())
}

#[derive(Debug, PartialEq, Eq)]
struct RuntimeIdentity {
    region: String,
    profile_arn: Option<String>,
}

fn runtime_identity(account: &Account) -> Result<RuntimeIdentity, DiscoveryFailure> {
    if account.provider_type != ProviderType::AmazonQOAuth {
        return Err(DiscoveryFailure::unavailable(
            "Kiro or other account cannot be used for Amazon Q",
        ));
    }
    let region = account_string(account, &["/runtimeRegion", "/apiRegion", "/region"])
        .unwrap_or_else(|| "us-east-1".to_string());
    if !AMAZON_Q_RUNTIME_REGIONS.contains(&region.as_str()) {
        return Err(DiscoveryFailure::unavailable(format!(
            "unsupported Amazon Q runtime region: {region}"
        )));
    }
    let profile_arn = account_string(account, &["/profileArn", "/resolvedProfileArn"]);
    if profile_arn.as_deref().is_some_and(|value| {
        value.len() > 2_048
            || value.chars().any(|character| character.is_control())
            || !value.starts_with("arn:")
    }) {
        return Err(DiscoveryFailure::unavailable(
            "Amazon Q profileArn is malformed",
        ));
    }
    Ok(RuntimeIdentity {
        region,
        profile_arn,
    })
}

async fn fetch_catalog(
    http: &reqwest::Client,
    access_token: &str,
    identity: &RuntimeIdentity,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
    parent: &ResidentMemoryBudget,
) -> Result<Arc<AmazonQModelCatalogData>, DiscoveryFailure> {
    let endpoint = runtime_endpoint(&identity.region, endpoint_override)?;
    let mut descriptors = Vec::new();
    let mut seen_models = BTreeSet::new();
    let mut seen_tokens = BTreeSet::new();
    let mut next_token: Option<String> = None;
    let mut default_model_id = None;
    let mut retained = parent.reserve(0).ok_or_else(DiscoveryFailure::capacity)?;
    let deadline = tokio::time::Instant::now() + request_timeout;

    for page in 0..MAX_PAGES {
        let memory = CatalogFetchMemory::new(parent.clone(), MAX_RESPONSE_BYTES)
            .ok_or_else(DiscoveryFailure::capacity)?;
        let _request_memory = memory
            .payload(
                next_token
                    .as_ref()
                    .map_or(0, String::capacity)
                    .saturating_mul(3)
                    .saturating_add(16384),
            )
            .ok_or_else(DiscoveryFailure::capacity)?;

        let mut body = json!({
            "origin": AMAZON_Q_ORIGIN,
            "maxResults": DEFAULT_PAGE_SIZE,
        });
        if let Some(profile_arn) = identity.profile_arn.as_deref() {
            body["profileArn"] = json!(profile_arn);
        }
        if let Some(token) = next_token.as_deref() {
            body["nextToken"] = json!(token);
        }
        let response = send_operation(
            http,
            &endpoint,
            access_token,
            AMAZON_Q_LIST_MODELS_TARGET,
            &body,
            deadline.saturating_duration_since(tokio::time::Instant::now()),
        )
        .await?;
        if !response.status.is_success() {
            let kind = if response.status == reqwest::StatusCode::REQUEST_TIMEOUT
                || response.status == reqwest::StatusCode::TOO_MANY_REQUESTS
                || response.status.is_server_error()
            {
                FailureKind::Transient
            } else {
                FailureKind::Unavailable
            };
            return Err(DiscoveryFailure::with_status(
                kind,
                response.status,
                format!("ListAvailableModels returned {}", response.status),
            ));
        }
        let _parsing = memory
            .parsing(response.body.len())
            .ok_or_else(DiscoveryFailure::capacity)?;
        let value: Value = serde_json::from_slice(&response.body).map_err(|error| {
            DiscoveryFailure::unavailable(format!(
                "parse Amazon Q ListAvailableModels page {}: {error}",
                page + 1
            ))
        })?;
        let models = value
            .get("models")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                DiscoveryFailure::unavailable("Amazon Q ListAvailableModels response lacks models")
            })?;
        if page == 0 {
            default_model_id = value
                .get("defaultModel")
                .and_then(|model| model.get("modelId"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            if !models.is_empty() && default_model_id.is_none() {
                return Err(DiscoveryFailure::unavailable(
                    "Amazon Q ListAvailableModels response lacks defaultModel",
                ));
            }
        }
        for model in models {
            let descriptor = parse_model(model)?;
            if seen_models.insert(descriptor.model_id.to_ascii_lowercase()) {
                descriptors.push(descriptor);
                if descriptors.len() > MAX_MODELS {
                    return Err(DiscoveryFailure::unavailable(
                        "Amazon Q model catalog exceeds model limit",
                    ));
                }
            }
        }
        next_token = value
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let Some(token) = next_token.as_ref() else {
            if default_model_id.as_ref().is_some_and(|default| {
                !descriptors
                    .iter()
                    .any(|model| model.model_id.eq_ignore_ascii_case(default))
            }) {
                return Err(DiscoveryFailure::unavailable(
                    "Amazon Q defaultModel is not present in the paginated model catalog",
                ));
            }
            drop(seen_models);
            drop(seen_tokens);
            let bytes = resident::payload_bytes(&descriptors, default_model_id.as_ref());
            if !retained.try_resize(bytes) {
                return Err(DiscoveryFailure::capacity());
            }
            return Ok(Arc::new(AmazonQModelCatalogData {
                descriptors,
                default_model_id,
                retained_bytes: bytes,
                _memory: Some(retained),
            }));
        };
        if !seen_tokens.insert(token.clone()) {
            return Err(DiscoveryFailure::unavailable(
                "Amazon Q model pagination repeated nextToken",
            ));
        }
        let lookup_bytes =
            seen_models
                .iter()
                .chain(seen_tokens.iter())
                .fold(0_usize, |n, value| {
                    n.saturating_add(crate::infra::resident_flights::node_bytes::<String, ()>(
                        value.capacity(),
                    ))
                });
        let bytes = resident::payload_bytes(&descriptors, default_model_id.as_ref())
            .saturating_add(lookup_bytes)
            .saturating_add(token.capacity());
        if !retained.try_resize(bytes) {
            return Err(DiscoveryFailure::capacity());
        }
    }
    Err(DiscoveryFailure::unavailable(
        "Amazon Q model pagination exceeds page limit",
    ))
}

fn parse_model(value: &Value) -> Result<AmazonQModelDescriptor, DiscoveryFailure> {
    let model_id = value
        .get("modelId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DiscoveryFailure::unavailable("Amazon Q model lacks modelId"))?;
    if model_id.len() > 512
        || model_id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(DiscoveryFailure::unavailable(
            "Amazon Q modelId is malformed",
        ));
    }
    let token_limits = value.get("tokenLimits");
    let supported_input_types = value
        .get("supportedInputTypes")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(AmazonQModelDescriptor {
        model_id: model_id.to_string(),
        display_name: optional_bounded_string(value.get("modelName"), 512)?,
        description: optional_bounded_string(value.get("description"), 8 * 1024)?,
        max_input_tokens: optional_positive_u64(
            token_limits.and_then(|limits| limits.get("maxInputTokens")),
        )?,
        max_output_tokens: optional_positive_u64(
            token_limits.and_then(|limits| limits.get("maxOutputTokens")),
        )?,
        supported_input_types,
        supports_prompt_cache: value.get("supportsPromptCache").and_then(Value::as_bool),
    })
}

fn optional_bounded_string(
    value: Option<&Value>,
    limit: usize,
) -> Result<Option<String>, DiscoveryFailure> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(DiscoveryFailure::unavailable(
            "Amazon Q model metadata has wrong type",
        ));
    };
    let value = value.trim();
    if value.len() > limit || value.chars().any(|character| character.is_control()) {
        return Err(DiscoveryFailure::unavailable(
            "Amazon Q model metadata exceeds bounds",
        ));
    }
    Ok((!value.is_empty()).then(|| value.to_string()))
}

fn optional_positive_u64(value: Option<&Value>) -> Result<Option<u64>, DiscoveryFailure> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.as_u64().ok_or_else(|| {
        DiscoveryFailure::unavailable("Amazon Q token limit must be an unsigned integer")
    })?;
    if value == 0 || value > 100_000_000 {
        return Err(DiscoveryFailure::unavailable(
            "Amazon Q token limit is outside accepted bounds",
        ));
    }
    Ok(Some(value))
}

struct OperationResponse {
    status: reqwest::StatusCode,
    body: bytes::Bytes,
}

async fn send_operation(
    http: &reqwest::Client,
    endpoint: &str,
    access_token: &str,
    target: &str,
    body: &Value,
    request_timeout: Duration,
) -> Result<OperationResponse, DiscoveryFailure> {
    let mut response = http
        .post(endpoint)
        .header("content-type", "application/x-amz-json-1.0")
        .header("accept", "application/json")
        .header("x-amz-target", target)
        .header("x-amz-user-agent", amazon_q_cli_user_agent())
        .header("user-agent", amazon_q_cli_user_agent())
        .header("authorization", format!("Bearer {access_token}"))
        .header("connection", "close")
        .timeout(request_timeout)
        .json(body)
        .send()
        .await
        .map_err(|error| {
            DiscoveryFailure::transient(format!("Amazon Q {target} request failed: {error}"))
        })?;
    let status = response.status();
    let body = crate::infra::http::read_response_body_limited(&mut response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            crate::infra::http::BoundedResponseBodyError::Request(error) => {
                DiscoveryFailure::transient(format!("Amazon Q {target} response failed: {error}"))
            }
            crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => {
                DiscoveryFailure::unavailable(format!(
                    "Amazon Q {target} response exceeds {MAX_RESPONSE_BYTES} bytes"
                ))
            }
        })?;
    Ok(OperationResponse { status, body })
}

fn runtime_endpoint(
    region: &str,
    endpoint_override: Option<&str>,
) -> Result<String, DiscoveryFailure> {
    if !AMAZON_Q_RUNTIME_REGIONS.contains(&region) {
        return Err(DiscoveryFailure::unavailable(
            "unsupported Amazon Q runtime region",
        ));
    }
    #[cfg(test)]
    if let Some(endpoint) = endpoint_override {
        let parsed = url::Url::parse(endpoint)
            .map_err(|_| DiscoveryFailure::unavailable("invalid Amazon Q test endpoint"))?;
        let loopback = parsed.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        });
        if parsed.scheme() != "http"
            || !loopback
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(DiscoveryFailure::unavailable(
                "Amazon Q test endpoint must be a credential-free loopback HTTP URL",
            ));
        }
        return Ok(endpoint.to_string());
    }
    #[cfg(not(test))]
    let _ = endpoint_override;
    Ok(format!("https://q.{region}.amazonaws.com/"))
}

fn amazon_q_cli_user_agent() -> &'static str {
    "aws-sdk-rust/1.3.15 ua/2.1 api/codewhisperer/0.1.14474 os/linux lang/rust/1.92.0 m/F app/AmazonQ-For-CLI"
}

fn account_string(account: &Account, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        account
            .profile
            .as_ref()
            .and_then(|value| value.pointer(pointer))
            .or_else(|| {
                account
                    .raw
                    .as_ref()
                    .and_then(|value| value.pointer(pointer))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::accounts::store::Account;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub(super) fn account(
        provider_type: ProviderType,
        generation: u64,
        account_id: &str,
    ) -> Account {
        Account {
            id: account_id.to_string(),
            provider_type,
            auth_identity_generation: generation,
            token_refresh_generation: 1,
            email: None,
            access_token: Some("amazon-q-access".to_string()),
            refresh_token: Some("amazon-q-refresh".to_string()),
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: Some(json!({"runtimeRegion":"us-east-1","endpoint":"amazon_q_cli"})),
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capacity_pool_limits: Default::default(),
            capability_observations: Default::default(),
            antigravity_access_generations: Default::default(),
            quota_window_observations: Default::default(),
            codex_passive_quota: None,
        }
    }

    async fn spawn_pages(pages: Vec<Value>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = Vec::new();
            for page in pages {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0u8; 4096];
                loop {
                    let count = stream.read(&mut buffer).await.unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|value| value == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end + 4]);
                        let content_length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(str::trim)
                                    .and_then(|value| value.parse::<usize>().ok())
                            })
                            .unwrap_or_default();
                        if request.len() >= end + 4 + content_length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8_lossy(&request).to_string());
                let body = serde_json::to_vec(&page).unwrap();
                stream.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/x-amz-json-1.0\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
            }
            requests
        });
        (format!("http://{address}/"), task)
    }

    #[tokio::test]
    async fn r7_amazon_q_parallel_cold_discovery_is_one_flight() {
        use axum::response::IntoResponse;
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let app=axum::Router::new().fallback(axum::routing::post(move || {
            let seen=seen.clone();
            async move {
                seen.fetch_add(1,Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(75)).await;
                axum::Json(json!({"models":[{"modelId":"r7-cold-model"}],"defaultModel":{"modelId":"r7-cold-model"}})).into_response()
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let barrier = Arc::new(tokio::sync::Barrier::new(9));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let barrier = barrier.clone();
            let endpoint = endpoint.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                model_catalog(
                    &reqwest::Client::new(),
                    &account(ProviderType::AmazonQOAuth, 1, "r7-amazon-q-cold-flight"),
                    Some(&endpoint),
                )
                .await
            }));
        }
        barrier.wait().await;
        for task in tasks {
            let catalog = task.await.unwrap();
            assert_eq!(
                catalog.model("r7-cold-model").unwrap().model_id,
                "r7-cold-model"
            );
        }
        server.abort();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "same scoped cold discovery must share one upstream request"
        );
    }

    #[tokio::test]
    async fn paginates_with_cli_origin_and_preserves_default_model() {
        let (endpoint, server) = spawn_pages(vec![
            json!({
                "models":[{"modelId":"claude-one","modelName":"Claude One","tokenLimits":{"maxInputTokens":200000,"maxOutputTokens":8192}}],
                "defaultModel":{"modelId":"claude-two"},
                "nextToken":"page-two"
            }),
            json!({
                "models":[{"modelId":"claude-two","supportedInputTypes":["TEXT","IMAGE"],"supportsPromptCache":true}],
                "defaultModel":{"modelId":"claude-two"}
            }),
        ])
        .await;
        let catalog = model_catalog(
            &reqwest::Client::new(),
            &account(ProviderType::AmazonQOAuth, 1, "amazon-q-pagination-account"),
            Some(&endpoint),
        )
        .await;
        assert_eq!(catalog.descriptors.len(), 2);
        assert_eq!(catalog.default_model_id.as_deref(), Some("claude-two"));
        assert!(!catalog.stale);
        let requests = server.await.unwrap();
        assert!(requests
            .iter()
            .all(|request| request.contains("\"origin\":\"CLI\"")));
        assert!(requests
            .iter()
            .all(|request| request.contains(AMAZON_Q_LIST_MODELS_TARGET)));
        assert!(requests[1].contains("\"nextToken\":\"page-two\""));
        assert!(requests
            .iter()
            .all(|request| request.contains("AmazonQ-For-CLI")));
        assert!(requests.iter().all(|request| !request.contains("KIRO_CLI")));
    }

    #[tokio::test]
    async fn rejects_kiro_account_without_network_access() {
        let catalog = model_catalog(
            &reqwest::Client::new(),
            &account(ProviderType::KiroOAuth, 1, "amazon-q-kiro-rejection"),
            Some("http://127.0.0.1:1/"),
        )
        .await;
        assert!(catalog.descriptors.is_empty());
        assert_eq!(catalog.source, "amazon_q_account_type_mismatch");
    }

    #[tokio::test]
    async fn cache_scope_includes_auth_identity_generation() {
        let (endpoint, server) = spawn_pages(vec![json!({
            "models":[{"modelId":"generation-one"}],
            "defaultModel":{"modelId":"generation-one"}
        })])
        .await;
        let first = model_catalog(
            &reqwest::Client::new(),
            &account(ProviderType::AmazonQOAuth, 1, "amazon-q-cache-scope"),
            Some(&endpoint),
        )
        .await;
        assert_eq!(first.descriptors[0].model_id, "generation-one");
        server.await.unwrap();

        let second = model_catalog(
            &reqwest::Client::new(),
            &account(ProviderType::AmazonQOAuth, 2, "amazon-q-cache-scope"),
            Some("http://127.0.0.1:1/"),
        )
        .await;
        assert!(second.descriptors.is_empty());
        assert!(!second.stale);
    }
}
