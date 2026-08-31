use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::domain::accounts::store::Account;
use crate::infra::time::now_ms;

pub const FALLBACK_IDE_VERSION: &str = "0.9.2";
#[cfg(not(test))]
const IDE_METADATA_URL: &str =
    "https://prod.download.desktop.kiro.dev/stable/metadata-linux-x64-stable.json";
const MODEL_CACHE_TTL_MS: i64 = 5 * 60 * 1000;
const STALE_MODEL_CACHE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_MODEL_CATALOG_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct KiroModelCatalog {
    pub descriptors: Vec<KiroModelDescriptor>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
    failure_status: Option<reqwest::StatusCode>,
}

#[derive(Debug, Clone)]
struct CachedModels {
    descriptors: Vec<KiroModelDescriptor>,
    fetched_at_ms: i64,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ModelCacheKey {
    app: String,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
    account_id: String,
    auth_identity_generation: u64,
    token_refresh_generation: u64,
    profile_scope: String,
    runtime_region: String,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct KiroModelCatalogScope {
    app: String,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
}

impl KiroModelCatalogScope {
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
    ) -> Self {
        Self {
            app: app.trim().to_string(),
            provider_id: provider_id.trim().to_string(),
            provider_revision,
            runtime_fingerprint: runtime_fingerprint.trim().to_string(),
        }
    }

    #[cfg(test)]
    fn fixture() -> Self {
        Self::derive("claude", "kiro-fixture-provider", 1, "kiro-fixture-runtime")
    }
}

#[derive(Debug, Default)]
struct RuntimeCache {
    ide_version: Option<(String, i64)>,
    models: HashMap<ModelCacheKey, CachedModels>,
}

static CACHE: LazyLock<Mutex<RuntimeCache>> = LazyLock::new(|| Mutex::new(RuntimeCache::default()));

#[cfg(not(test))]
#[derive(Deserialize)]
struct IdeMetadata {
    #[serde(rename = "currentRelease")]
    current_release: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailableModelsResponse {
    #[serde(alias = "availableModels")]
    models: Vec<AvailableModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailableModel {
    model_id: String,
    #[serde(default, alias = "name")]
    model_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    token_limits: Option<AvailableModelTokenLimits>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailableModelTokenLimits {
    #[serde(default)]
    max_input_tokens: Option<i64>,
    #[serde(default)]
    max_output_tokens: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KiroModelDescriptor {
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
}

impl KiroModelCatalog {
    pub fn model_ids(&self) -> impl Iterator<Item = &str> {
        self.descriptors
            .iter()
            .map(|descriptor| descriptor.model_id.as_str())
    }

    pub fn is_unauthorized(&self) -> bool {
        self.failure_status == Some(reqwest::StatusCode::UNAUTHORIZED)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelDiscoveryFailureKind {
    Transient,
    Unavailable,
}

#[derive(Debug)]
struct ModelDiscoveryFailure {
    kind: ModelDiscoveryFailureKind,
    message: String,
    status: Option<reqwest::StatusCode>,
}

impl ModelDiscoveryFailure {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: ModelDiscoveryFailureKind::Transient,
            message: message.into(),
            status: None,
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: ModelDiscoveryFailureKind::Unavailable,
            message: message.into(),
            status: None,
        }
    }

    fn transient_with_status(message: impl Into<String>, status: reqwest::StatusCode) -> Self {
        Self {
            kind: ModelDiscoveryFailureKind::Transient,
            message: message.into(),
            status: Some(status),
        }
    }

    fn unavailable_with_status(message: impl Into<String>, status: reqwest::StatusCode) -> Self {
        Self {
            kind: ModelDiscoveryFailureKind::Unavailable,
            message: message.into(),
            status: Some(status),
        }
    }
}

impl std::fmt::Display for ModelDiscoveryFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

pub async fn effective_ide_version(http: &reqwest::Client) -> String {
    #[cfg(test)]
    let _ = http;
    let now = now_ms().min(i64::MAX as u128) as i64;
    if let Some(version) = CACHE
        .lock()
        .await
        .ide_version
        .as_ref()
        .filter(|(_, fetched)| now.saturating_sub(*fetched) < 6 * 60 * 60 * 1000)
        .map(|(version, _)| version.clone())
    {
        return version;
    }
    #[cfg(test)]
    return FALLBACK_IDE_VERSION.to_string();

    #[cfg(not(test))]
    {
        let fetched =
            tokio::time::timeout(Duration::from_secs(2), http.get(IDE_METADATA_URL).send())
                .await
                .ok()
                .and_then(Result::ok);
        let version = match fetched {
            Some(response) if response.status().is_success() => response
                .json::<IdeMetadata>()
                .await
                .ok()
                .and_then(|metadata| metadata.current_release)
                .map(|value| value.trim().to_string())
                .filter(|value| valid_version(value)),
            _ => None,
        };
        if let Some(version) = version {
            CACHE.lock().await.ide_version = Some((version.clone(), now));
            version
        } else {
            CACHE
                .lock()
                .await
                .ide_version
                .as_ref()
                .map(|(version, _)| version.clone())
                .unwrap_or_else(|| FALLBACK_IDE_VERSION.to_string())
        }
    }
}

#[cfg(test)]
pub async fn model_catalog(
    http: &reqwest::Client,
    account: &Account,
    endpoint_override: Option<&str>,
) -> KiroModelCatalog {
    model_catalog_scoped(
        http,
        account,
        &KiroModelCatalogScope::fixture(),
        endpoint_override,
        Duration::from_secs(10),
    )
    .await
}

#[cfg(test)]
pub async fn model_catalog_with_timeout(
    http: &reqwest::Client,
    account: &Account,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> KiroModelCatalog {
    model_catalog_scoped(
        http,
        account,
        &KiroModelCatalogScope::fixture(),
        endpoint_override,
        request_timeout,
    )
    .await
}

pub async fn model_catalog_scoped(
    http: &reqwest::Client,
    account: &Account,
    scope: &KiroModelCatalogScope,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> KiroModelCatalog {
    let now = now_ms().min(i64::MAX as u128) as i64;
    let identity =
        match crate::domain::providers::kiro::operational_runtime_identity_from_account(account) {
            Ok(identity) => identity,
            Err(error) => {
                tracing::warn!(
                    account_id = %account.id,
                    error = %error,
                    "Kiro model discovery rejected unresolved runtime identity"
                );
                return unavailable_model_catalog("kiro_identity_unresolved");
            }
        };
    let access_token = match account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        Some(access_token) => access_token,
        None => return unavailable_model_catalog("kiro_credential_unavailable"),
    };
    let cache_key = ModelCacheKey {
        app: scope.app.clone(),
        provider_id: scope.provider_id.clone(),
        provider_revision: scope.provider_revision,
        runtime_fingerprint: scope.runtime_fingerprint.clone(),
        account_id: account.id.clone(),
        auth_identity_generation: account.auth_identity_generation,
        token_refresh_generation: account.token_refresh_generation,
        profile_scope: identity
            .profile_arn
            .clone()
            .unwrap_or_else(|| "profileless_api_key".to_string()),
        runtime_region: identity.runtime_region.clone(),
    };
    if let Some(cached) = CACHE
        .lock()
        .await
        .models
        .get(&cache_key)
        .filter(|cached| now.saturating_sub(cached.fetched_at_ms) < MODEL_CACHE_TTL_MS)
        .cloned()
    {
        return KiroModelCatalog {
            descriptors: cached.descriptors,
            source: "kiro_account_cache",
            stale: false,
            fetched_at_ms: Some(cached.fetched_at_ms),
            failure_status: None,
        };
    }

    let fetched = fetch_models(
        http,
        account,
        &identity,
        access_token,
        endpoint_override,
        request_timeout,
    )
    .await;
    match fetched {
        Ok(descriptors) => {
            CACHE.lock().await.models.insert(
                cache_key,
                CachedModels {
                    descriptors: descriptors.clone(),
                    fetched_at_ms: now,
                },
            );
            KiroModelCatalog {
                descriptors,
                source: "kiro_list_available_models",
                stale: false,
                fetched_at_ms: Some(now),
                failure_status: None,
            }
        }
        Err(error) if error.kind == ModelDiscoveryFailureKind::Unavailable => {
            tracing::warn!(account_id = %account.id, error = %error, "Kiro model discovery rejected bound account capability");
            unavailable_model_catalog_with_status("kiro_model_access_unavailable", error.status)
        }
        Err(error) => {
            tracing::warn!(account_id = %account.id, error = %error, "Kiro model discovery failed transiently");
            if let Some(cached) = CACHE
                .lock()
                .await
                .models
                .get(&cache_key)
                .filter(|cached| {
                    now.saturating_sub(cached.fetched_at_ms) < STALE_MODEL_CACHE_TTL_MS
                })
                .cloned()
            {
                KiroModelCatalog {
                    descriptors: cached.descriptors,
                    source: "kiro_account_cache",
                    stale: true,
                    fetched_at_ms: Some(cached.fetched_at_ms),
                    failure_status: None,
                }
            } else {
                unavailable_model_catalog_with_status(
                    "kiro_model_discovery_transient_without_cache",
                    error.status,
                )
            }
        }
    }
}

#[cfg(test)]
async fn cached_model_descriptor(
    account: &Account,
    scope: &KiroModelCatalogScope,
    model_id: &str,
) -> Option<Option<KiroModelDescriptor>> {
    let now = now_ms().min(i64::MAX as u128) as i64;
    let identity =
        crate::domain::providers::kiro::operational_runtime_identity_from_account(account).ok()?;
    let cache_key = ModelCacheKey {
        app: scope.app.clone(),
        provider_id: scope.provider_id.clone(),
        provider_revision: scope.provider_revision,
        runtime_fingerprint: scope.runtime_fingerprint.clone(),
        account_id: account.id.clone(),
        auth_identity_generation: account.auth_identity_generation,
        token_refresh_generation: account.token_refresh_generation,
        profile_scope: identity
            .profile_arn
            .unwrap_or_else(|| "profileless_api_key".to_string()),
        runtime_region: identity.runtime_region,
    };
    CACHE
        .lock()
        .await
        .models
        .get(&cache_key)
        .filter(|cached| now.saturating_sub(cached.fetched_at_ms) < STALE_MODEL_CACHE_TTL_MS)
        .map(|cached| {
            cached
                .descriptors
                .iter()
                .find(|model| model.model_id.eq_ignore_ascii_case(model_id))
                .cloned()
        })
}

pub fn static_model_catalog(source: &'static str) -> KiroModelCatalog {
    let descriptors = crate::domain::providers::kiro::STATIC_MODEL_IDS
        .iter()
        .map(|model_id| KiroModelDescriptor {
            model_id: (*model_id).to_string(),
            display_name: None,
            description: None,
            max_input_tokens: None,
            max_output_tokens: None,
        })
        .collect();
    KiroModelCatalog {
        descriptors,
        source,
        stale: true,
        fetched_at_ms: None,
        failure_status: None,
    }
}

pub fn unavailable_model_catalog(source: &'static str) -> KiroModelCatalog {
    unavailable_model_catalog_with_status(source, None)
}

fn unavailable_model_catalog_with_status(
    source: &'static str,
    failure_status: Option<reqwest::StatusCode>,
) -> KiroModelCatalog {
    KiroModelCatalog {
        descriptors: Vec::new(),
        source,
        stale: false,
        fetched_at_ms: None,
        failure_status,
    }
}

async fn fetch_models(
    http: &reqwest::Client,
    account: &Account,
    identity: &crate::domain::providers::kiro::KiroRuntimeIdentity,
    access_token: &str,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> Result<Vec<KiroModelDescriptor>, ModelDiscoveryFailure> {
    let machine_id = account_string(account, &["/machineId", "/machine_id"])
        .unwrap_or_else(|| account.id.clone());
    if let Some(endpoint) = endpoint_override {
        let host = reqwest::Url::parse(endpoint)
            .ok()
            .and_then(|url| url.host_str().map(str::to_string))
            .unwrap_or_else(|| format!("q.{}.amazonaws.com", identity.runtime_region));
        let mut attempts = Vec::new();
        if let Some(profile_arn) = identity.profile_arn.as_deref() {
            attempts.push(Some(profile_arn));
        }
        attempts.push(None);
        let mut last_failure = None;
        for profile_arn in attempts {
            let url = discovery_url(endpoint, profile_arn)?;
            match fetch_models_request(
                http,
                &url,
                &host,
                access_token,
                &machine_id,
                kiro_token_type(account),
                request_timeout,
            )
            .await
            {
                Ok(models) => return Ok(models),
                Err(error)
                    if error.status == Some(reqwest::StatusCode::FORBIDDEN)
                        && profile_arn.is_some() =>
                {
                    last_failure = Some(error);
                }
                Err(error) => return Err(error),
            }
        }
        return Err(last_failure.unwrap_or_else(|| {
            ModelDiscoveryFailure::transient("Kiro model discovery produced no attempt")
        }));
    }

    let mut last_failure = None;
    for region in discovery_region_candidates(&identity.runtime_region) {
        let host = format!("q.{region}.amazonaws.com");
        let base = format!("https://{host}/ListAvailableModels?origin=AI_EDITOR");
        let mut profile_attempts = Vec::new();
        if let Some(profile_arn) = identity.profile_arn.as_deref() {
            profile_attempts.push(Some(profile_arn));
        }
        profile_attempts.push(None);
        for profile_arn in profile_attempts {
            let url = discovery_url(&base, profile_arn)?;
            match fetch_models_request(
                http,
                &url,
                &host,
                access_token,
                &machine_id,
                kiro_token_type(account),
                request_timeout,
            )
            .await
            {
                Ok(models) => return Ok(models),
                Err(error)
                    if error.status == Some(reqwest::StatusCode::FORBIDDEN)
                        && profile_arn.is_some() =>
                {
                    last_failure = Some(error);
                }
                Err(error) => {
                    let retry_region = error.status == Some(reqwest::StatusCode::FORBIDDEN);
                    last_failure = Some(error);
                    if !retry_region {
                        return Err(last_failure.expect("failure was set"));
                    }
                }
            }
        }
    }
    Err(last_failure.unwrap_or_else(|| {
        ModelDiscoveryFailure::transient("no Kiro discovery endpoint candidates")
    }))
}

pub(crate) async fn fetch_models_for_api_key(
    http: &reqwest::Client,
    region: &str,
    api_key: &str,
    request_timeout: Duration,
) -> Result<Vec<String>, String> {
    let region = crate::domain::providers::kiro::normalize_region(region)
        .ok_or_else(|| "invalid Kiro runtime region".to_string())?;
    let host = format!("q.{region}.amazonaws.com");
    let url = format!("https://{host}/ListAvailableModels?origin=AI_EDITOR");
    fetch_models_request(
        http,
        &url,
        &host,
        api_key,
        "kiro-api-key",
        Some("API_KEY"),
        request_timeout,
    )
    .await
    .map(|models| models.into_iter().map(|model| model.model_id).collect())
    .map_err(|error| error.to_string())
}

async fn fetch_models_request(
    http: &reqwest::Client,
    url: &str,
    host: &str,
    access_token: &str,
    machine_id: &str,
    token_type: Option<&str>,
    request_timeout: Duration,
) -> Result<Vec<KiroModelDescriptor>, ModelDiscoveryFailure> {
    let mut request = http
        .get(url)
        .header(
            "x-amz-user-agent",
            format!("aws-sdk-js/1.0.0 KiroIDE-{FALLBACK_IDE_VERSION}-{machine_id}"),
        )
        .header(
            "user-agent",
            format!(
                "aws-sdk-js/1.0.0 ua/2.1 os/macos lang/js md/nodejs#22.22.0 api/codewhispererruntime#1.0.0 m/N,E KiroIDE-{FALLBACK_IDE_VERSION}-{machine_id}"
            ),
        )
        .header("host", host)
        .header("authorization", format!("Bearer {access_token}"))
        .header("connection", "close");
    if let Some(token_type) = token_type {
        request = request.header("tokentype", token_type);
    }
    let response = tokio::time::timeout(request_timeout, request.send())
        .await
        .map_err(|_| ModelDiscoveryFailure::transient("model discovery timed out"))?
        .map_err(|error| {
            let kind = if error.is_timeout() {
                "timeout"
            } else if error.is_connect() {
                "connect"
            } else if error.is_request() {
                "request"
            } else if error.is_body() {
                "body"
            } else {
                "transport"
            };
            ModelDiscoveryFailure::transient(format!("model discovery transport failed ({kind})"))
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let message = format!("model discovery returned {status}");
        return Err(
            if status.is_server_error()
                || matches!(
                    status,
                    reqwest::StatusCode::REQUEST_TIMEOUT | reqwest::StatusCode::TOO_MANY_REQUESTS
                )
            {
                ModelDiscoveryFailure::transient_with_status(message, status)
            } else {
                ModelDiscoveryFailure::unavailable_with_status(message, status)
            },
        );
    }
    let payload = response.bytes().await.map_err(|error| {
        ModelDiscoveryFailure::transient(format!("model discovery response body failed: {error}"))
    })?;
    if payload.len() > MAX_MODEL_CATALOG_BYTES {
        return Err(ModelDiscoveryFailure::unavailable(format!(
            "model discovery response exceeded {MAX_MODEL_CATALOG_BYTES} bytes"
        )));
    }
    let payload = serde_json::from_slice::<AvailableModelsResponse>(&payload).map_err(|_| {
        ModelDiscoveryFailure::unavailable(
            "model discovery response did not satisfy the models contract",
        )
    })?;
    let models = payload
        .models
        .into_iter()
        .filter_map(|model| {
            let model_id = model.model_id.trim().to_string();
            valid_model_id(&model_id).then(|| KiroModelDescriptor {
                model_id,
                display_name: clean_optional(model.model_name),
                description: clean_optional(model.description),
                max_input_tokens: valid_token_limit(
                    model
                        .token_limits
                        .as_ref()
                        .and_then(|limits| limits.max_input_tokens),
                ),
                max_output_tokens: valid_token_limit(
                    model
                        .token_limits
                        .as_ref()
                        .and_then(|limits| limits.max_output_tokens),
                ),
            })
        })
        .collect::<Vec<_>>();
    let mut by_id = BTreeMap::<String, KiroModelDescriptor>::new();
    for descriptor in models {
        by_id
            .entry(descriptor.model_id.to_ascii_lowercase())
            .and_modify(|existing| merge_descriptor(existing, &descriptor))
            .or_insert(descriptor);
    }
    Ok(by_id.into_values().collect())
}

pub fn model_discovery_url(account: &Account) -> Result<String, String> {
    let identity =
        crate::domain::providers::kiro::operational_runtime_identity_from_account(account)
            .map_err(|error| format!("bound account has {error}"))?;
    let region = discovery_region_candidates(&identity.runtime_region)[0];
    discovery_url(
        &format!("https://q.{region}.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"),
        identity.profile_arn.as_deref(),
    )
    .map_err(|error| error.to_string())
}

fn discovery_region_candidates(region: &str) -> [&'static str; 2] {
    if region.starts_with("eu-") {
        ["eu-central-1", "us-east-1"]
    } else {
        ["us-east-1", "eu-central-1"]
    }
}

fn discovery_url(base: &str, profile_arn: Option<&str>) -> Result<String, ModelDiscoveryFailure> {
    let mut url = reqwest::Url::parse(base).map_err(|error| {
        ModelDiscoveryFailure::unavailable(format!("invalid model discovery URL: {error}"))
    })?;
    if !url.query_pairs().any(|(key, _)| key == "origin") {
        url.query_pairs_mut().append_pair("origin", "AI_EDITOR");
    }
    if let Some(profile_arn) = profile_arn {
        url.query_pairs_mut().append_pair("profileArn", profile_arn);
    }
    Ok(url.to_string())
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn valid_token_limit(value: Option<i64>) -> Option<u64> {
    value
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| (1..=10_000_000).contains(value))
}

fn valid_model_id(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && model.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'[' | b']')
        })
}

fn merge_descriptor(target: &mut KiroModelDescriptor, source: &KiroModelDescriptor) {
    if source.model_id < target.model_id {
        target.model_id.clone_from(&source.model_id);
    }
    merge_optional_text(&mut target.display_name, &source.display_name);
    merge_optional_text(&mut target.description, &source.description);
    target.max_input_tokens = minimum_optional(target.max_input_tokens, source.max_input_tokens);
    target.max_output_tokens = minimum_optional(target.max_output_tokens, source.max_output_tokens);
}

fn merge_optional_text(target: &mut Option<String>, source: &Option<String>) {
    let replace = match (target.as_deref(), source.as_deref()) {
        (None, Some(_)) => true,
        (Some(current), Some(candidate)) => {
            candidate.len() > current.len()
                || (candidate.len() == current.len() && candidate < current)
        }
        _ => false,
    };
    if replace {
        target.clone_from(source);
    }
}

fn minimum_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn account_string(account: &Account, pointers: &[&str]) -> Option<String> {
    [&account.profile, &account.raw]
        .into_iter()
        .filter_map(|value| value.as_ref())
        .find_map(|value| {
            pointers.iter().find_map(|pointer| {
                value
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
        })
}

fn kiro_token_type(account: &Account) -> Option<&'static str> {
    match account_string(account, &["/authMethod", "/auth_method"])?
        .to_ascii_lowercase()
        .as_str()
    {
        "api_key" | "api-key" | "apikey" => Some("API_KEY"),
        "external_idp" | "external-idp" | "externalidp" => Some("EXTERNAL_IDP"),
        _ => None,
    }
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.' || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const WIRE_PROTOCOL_JSON: &str =
        include_str!("../../../assets/contract/kiro-wire-protocol.json");

    #[test]
    fn static_catalog_is_sorted_and_unique() {
        let catalog = static_model_catalog("test");
        let ids = catalog.model_ids().collect::<Vec<_>>();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn duplicate_descriptors_merge_deterministically_and_conservatively() {
        let mut first = KiroModelDescriptor {
            model_id: "Model-A".to_string(),
            display_name: Some("A".to_string()),
            description: None,
            max_input_tokens: Some(1_000_000),
            max_output_tokens: None,
        };
        let second = KiroModelDescriptor {
            model_id: "model-a".to_string(),
            display_name: Some("Model A".to_string()),
            description: Some("complete description".to_string()),
            max_input_tokens: Some(200_000),
            max_output_tokens: Some(32_000),
        };
        merge_descriptor(&mut first, &second);
        assert_eq!(first.model_id, "Model-A");
        assert_eq!(first.display_name.as_deref(), Some("Model A"));
        assert_eq!(first.description.as_deref(), Some("complete description"));
        assert_eq!(first.max_input_tokens, Some(200_000));
        assert_eq!(first.max_output_tokens, Some(32_000));
    }

    #[test]
    fn descriptor_token_limits_reject_invalid_values() {
        assert_eq!(valid_token_limit(Some(-1)), None);
        assert_eq!(valid_token_limit(Some(0)), None);
        assert_eq!(valid_token_limit(Some(10_000_001)), None);
        assert_eq!(valid_token_limit(Some(1_000_000)), Some(1_000_000));
    }

    #[test]
    fn version_validation_rejects_header_injection() {
        assert!(valid_version("0.12.3"));
        assert!(!valid_version("0.12.3\r\nx-bad: yes"));
    }

    #[test]
    fn discovery_scope_endpoint_and_ttls_match_the_wire_protocol_fixture() {
        let fixture: Value = serde_json::from_str(WIRE_PROTOCOL_JSON).unwrap();
        let discovery = &fixture["modelDiscovery"];
        assert_eq!(
            discovery["cacheScope"],
            json!([
                "app",
                "provider_id",
                "provider_revision",
                "runtime_fingerprint",
                "account_id",
                "auth_identity_generation",
                "token_refresh_generation",
                "authoritative_profile_arn_or_profileless_api_key",
                "runtime_region"
            ])
        );
        assert_eq!(discovery["freshTtlMs"], MODEL_CACHE_TTL_MS);
        assert_eq!(discovery["staleTtlMs"], STALE_MODEL_CACHE_TTL_MS);
        assert_eq!(discovery["liveCatalogPolicy"], "replace_static");
        assert_eq!(discovery["crossAccountUnion"], false);
        assert_eq!(
            discovery["fallbackEligibility"],
            "network_timeout_408_429_5xx_only"
        );
        assert_eq!(discovery["staticFallbackAllowed"], false);
        assert_eq!(
            discovery["malformedSuccessPolicy"],
            "fail_closed_without_stale"
        );
        assert_eq!(discovery["sameAccount401Replay"], 1);
        assert_eq!(
            discovery["unresolvedIdentityPolicy"],
            json!({
                "catalog": "empty",
                "source": "kiro_identity_unresolved",
                "stale": false,
                "fallbackAllowed": false
            })
        );
        assert_eq!(
            discovery["unavailableCredentialPolicy"],
            json!({
                "catalog": "empty",
                "source": "kiro_credential_unavailable",
                "stale": false,
                "fallbackAllowed": false
            })
        );

        let account: Account = serde_json::from_value(json!({
            "id": "fixture-account",
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "profile": {"apiRegion": "eu-west-1"}
        }))
        .unwrap();
        let url = model_discovery_url(&account).unwrap();
        assert!(url.starts_with(
            "https://q.us-east-1.amazonaws.com/ListAvailableModels?origin=AI_EDITOR&profileArn="
        ));
        assert!(url.contains("arn%3Aaws%3Acodewhisperer"));
        assert_eq!(
            fixture["endpoints"]["models"]["url"],
            "https://q.{region}.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"
        );
    }

    #[test]
    fn profile_arn_region_drives_model_discovery_across_idc_regions() {
        let account: Account = serde_json::from_value(json!({
            "id": "cross-region-idc",
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "profile": {
                "authRegion": "eu-north-1",
                "apiRegion": "us-east-1",
                "profileArn": "arn:aws:codewhisperer:eu-central-1:123456789012:profile/profile-id"
            }
        }))
        .unwrap();
        let url = model_discovery_url(&account).unwrap();
        assert!(url.starts_with(
            "https://q.eu-central-1.amazonaws.com/ListAvailableModels?origin=AI_EDITOR&profileArn="
        ));
        assert!(url.contains("profile%2Fprofile-id"));
    }

    #[test]
    fn api_key_model_discovery_remains_profileless() {
        let account: Account = serde_json::from_value(json!({
            "id": "profileless-api-key",
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "accessToken": "ksk_fixture",
            "profile": {"authMethod":"api_key","apiRegion":"eu-west-1"},
            "raw": {"authMethod":"api_key"}
        }))
        .unwrap();
        let url = model_discovery_url(&account).unwrap();
        assert_eq!(
            url,
            "https://q.eu-central-1.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"
        );
    }

    #[test]
    fn discovery_url_rejects_unsafe_region_from_stored_account() {
        let account: Account = serde_json::from_value(json!({
            "id": "invalid-region-account",
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "profile": {"apiRegion": "us-east-1.attacker.invalid"}
        }))
        .unwrap();
        assert!(model_discovery_url(&account).is_err());
    }

    #[test]
    fn discovery_rejects_profileless_and_legacy_fallback_idc_accounts() {
        for profile in [
            json!({
                "authMethod": "idc",
                "runtimeRegion": "eu-central-1"
            }),
            json!({
                "authMethod": "idc",
                "profileArn": "arn:aws:codewhisperer:eu-central-1:610548660232:profile/VNECVYCYYAWN",
                "profileProvenance": "auth_method_default"
            }),
        ] {
            let account: Account = serde_json::from_value(json!({
                "id": "unresolved-idc",
                "providerType": "kiro_oauth",
                "authIdentityGeneration": 1,
                "profile": profile,
                "raw": {"authMethod": "idc"}
            }))
            .unwrap();
            let error = model_discovery_url(&account).unwrap_err();
            assert!(
                error.contains("organization profile ARN")
                    || error.contains("legacy shared fallback"),
                "{error}"
            );
        }
    }

    #[tokio::test]
    async fn model_catalog_isolated_by_bound_account_and_identity_generation() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_for_route = std::sync::Arc::clone(&observed);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    let authorization = headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or_default()
                        .to_string();
                    observed.lock().unwrap().push(authorization.clone());
                    let model = match authorization.as_str() {
                        "Bearer account-a-token" => "model-account-a",
                        "Bearer account-a-refreshed-token" => "model-account-a-refreshed",
                        "Bearer account-a-generation-2-token" => "model-account-a-generation-2",
                        "Bearer account-b-token" => "model-account-b",
                        _ => "unexpected-model",
                    };
                    axum::Json(json!({"models": [{
                        "modelId": model,
                        "modelName": "Bound model",
                        "description": "sanitized fixture",
                        "tokenLimits": {"maxInputTokens": 900000, "maxOutputTokens": 32000}
                    }]}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let unique = crate::infra::time::now_ms();
        let account =
            |suffix: &str, generation: u64, token_generation: u64, token: &str| -> Account {
                serde_json::from_value(json!({
                    "id": format!("kiro-model-{unique}-{suffix}"),
                    "providerType": "kiro_oauth",
                    "authIdentityGeneration": generation,
                    "tokenRefreshGeneration": token_generation,
                    "accessToken": token,
                    "profile": {
                        "apiRegion": "us-east-1",
                        "machineId": format!("machine-{suffix}"),
                        "authMethod": "social"
                    }
                }))
                .unwrap()
            };
        let account_a = account("a", 1, 0, "account-a-token");
        let account_b = account("b", 1, 0, "account-b-token");
        let endpoint = format!("http://{address}/models");
        let http = reqwest::Client::new();

        let first_a = model_catalog(&http, &account_a, Some(&endpoint)).await;
        let cached_a = model_catalog(&http, &account_a, Some(&endpoint)).await;
        let first_b = model_catalog(&http, &account_b, Some(&endpoint)).await;
        let account_a_refreshed = account("a", 1, 1, "account-a-refreshed-token");
        let refreshed = model_catalog(&http, &account_a_refreshed, Some(&endpoint)).await;
        let account_a_generation_2 = account("a", 2, 0, "account-a-generation-2-token");
        let second_generation =
            model_catalog(&http, &account_a_generation_2, Some(&endpoint)).await;

        assert_eq!(first_a.model_ids().collect::<Vec<_>>(), ["model-account-a"]);
        assert_eq!(
            first_a.descriptors[0].display_name.as_deref(),
            Some("Bound model")
        );
        assert_eq!(first_a.descriptors[0].max_input_tokens, Some(900_000));
        assert_eq!(first_a.descriptors[0].max_output_tokens, Some(32_000));
        assert_eq!(
            cached_a.model_ids().collect::<Vec<_>>(),
            ["model-account-a"]
        );
        assert_eq!(cached_a.source, "kiro_account_cache");
        assert_eq!(first_b.model_ids().collect::<Vec<_>>(), ["model-account-b"]);
        assert_eq!(
            refreshed.model_ids().collect::<Vec<_>>(),
            ["model-account-a-refreshed"]
        );
        assert_eq!(
            second_generation.model_ids().collect::<Vec<_>>(),
            ["model-account-a-generation-2"]
        );
        assert_eq!(
            observed.lock().unwrap().as_slice(),
            [
                "Bearer account-a-token",
                "Bearer account-b-token",
                "Bearer account-a-refreshed-token",
                "Bearer account-a-generation-2-token"
            ]
        );
        server.abort();
    }

    #[tokio::test]
    async fn model_catalog_cache_isolated_by_provider_runtime_scope() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let app =
            axum::Router::new().route(
                "/models",
                axum::routing::get(move || {
                    let request =
                        requests_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                    async move {
                        axum::Json(json!({"models": [{"modelId": format!("model-{request}")}]}))
                    }
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let account: Account = serde_json::from_value(json!({
            "id": format!("kiro-runtime-scope-{}", crate::infra::time::now_ms()),
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 3,
            "tokenRefreshGeneration": 5,
            "accessToken": "scope-token",
            "profile": {
                "authMethod": "social",
                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/scope"
            }
        }))
        .unwrap();
        let endpoint = format!("http://{address}/models");
        let scope_a = KiroModelCatalogScope::derive("claude", "provider-a", 1, "runtime-a");
        let scope_b = KiroModelCatalogScope::derive("claude", "provider-b", 1, "runtime-b");
        let http = reqwest::Client::new();

        let first_a = model_catalog_scoped(
            &http,
            &account,
            &scope_a,
            Some(&endpoint),
            Duration::from_secs(2),
        )
        .await;
        let first_b = model_catalog_scoped(
            &http,
            &account,
            &scope_b,
            Some(&endpoint),
            Duration::from_secs(2),
        )
        .await;
        let cached_a = model_catalog_scoped(
            &http,
            &account,
            &scope_a,
            Some(&endpoint),
            Duration::from_secs(2),
        )
        .await;

        assert_eq!(first_a.model_ids().collect::<Vec<_>>(), ["model-1"]);
        assert_eq!(first_b.model_ids().collect::<Vec<_>>(), ["model-2"]);
        assert_eq!(cached_a.model_ids().collect::<Vec<_>>(), ["model-1"]);
        assert_eq!(cached_a.source, "kiro_account_cache");
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn malformed_success_does_not_reuse_same_scope_stale_catalog() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route(
                "/good",
                axum::routing::get(|| async {
                    axum::Json(json!({"models": [{"modelId": "good-model"}]}))
                }),
            )
            .route(
                "/malformed",
                axum::routing::get(|| async { axum::Json(json!({"unexpected": []})) }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let account: Account = serde_json::from_value(json!({
            "id": format!("kiro-malformed-scope-{}", crate::infra::time::now_ms()),
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "accessToken": "malformed-token",
            "profile": {
                "authMethod": "social",
                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/malformed"
            }
        }))
        .unwrap();
        let scope = KiroModelCatalogScope::derive("codex", "provider", 9, "runtime");
        let http = reqwest::Client::new();
        let good = model_catalog_scoped(
            &http,
            &account,
            &scope,
            Some(&format!("http://{address}/good")),
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(good.model_ids().collect::<Vec<_>>(), ["good-model"]);

        let identity =
            crate::domain::providers::kiro::operational_runtime_identity_from_account(&account)
                .unwrap();
        let key = ModelCacheKey {
            app: scope.app.clone(),
            provider_id: scope.provider_id.clone(),
            provider_revision: scope.provider_revision,
            runtime_fingerprint: scope.runtime_fingerprint.clone(),
            account_id: account.id.clone(),
            auth_identity_generation: account.auth_identity_generation,
            token_refresh_generation: account.token_refresh_generation,
            profile_scope: identity.profile_arn.unwrap(),
            runtime_region: identity.runtime_region,
        };
        CACHE
            .lock()
            .await
            .models
            .get_mut(&key)
            .unwrap()
            .fetched_at_ms =
            (now_ms().min(i64::MAX as u128) as i64).saturating_sub(MODEL_CACHE_TTL_MS);

        let malformed = model_catalog_scoped(
            &http,
            &account,
            &scope,
            Some(&format!("http://{address}/malformed")),
            Duration::from_secs(2),
        )
        .await;
        assert!(malformed.descriptors.is_empty());
        assert_eq!(malformed.source, "kiro_model_access_unavailable");
        assert!(!malformed.stale);
        server.abort();
    }

    #[tokio::test]
    async fn model_catalog_isolated_by_authoritative_profile_and_fails_closed_when_unresolved() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let requests_for_route = std::sync::Arc::clone(&requests);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let requests = std::sync::Arc::clone(&requests_for_route);
                async move {
                    requests.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let model = match headers
                        .get(axum::http::header::AUTHORIZATION)
                        .and_then(|value| value.to_str().ok())
                    {
                        Some("Bearer profile-a-token") => "profile-a-model",
                        Some("Bearer profile-b-token") => "profile-b-model",
                        _ => "unexpected-model",
                    };
                    axum::Json(json!({"models": [{"modelId": model}]}))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let endpoint = format!("http://{address}/models");
        let unique = crate::infra::time::now_ms();
        let account = |profile_id: &str, token: &str| -> Account {
            serde_json::from_value(json!({
                "id": format!("kiro-model-profile-scope-{unique}"),
                "providerType": "kiro_oauth",
                "authIdentityGeneration": 1,
                "accessToken": token,
                "profile": {
                    "authMethod": "idc",
                    "profileArn": format!(
                        "arn:aws:codewhisperer:eu-central-1:123456789012:profile/{profile_id}"
                    ),
                    "profileProvenance": "profile_discovery"
                }
            }))
            .unwrap()
        };
        let http = reqwest::Client::new();

        let profile_a = model_catalog(
            &http,
            &account("profile-a", "profile-a-token"),
            Some(&endpoint),
        )
        .await;
        let profile_b = model_catalog(
            &http,
            &account("profile-b", "profile-b-token"),
            Some(&endpoint),
        )
        .await;
        assert_eq!(
            profile_a.model_ids().collect::<Vec<_>>(),
            ["profile-a-model"]
        );
        assert_eq!(
            profile_b.model_ids().collect::<Vec<_>>(),
            ["profile-b-model"]
        );
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);

        let unresolved: Account = serde_json::from_value(json!({
            "id": format!("kiro-model-profile-scope-{unique}"),
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "accessToken": "profile-a-token",
            "profile": {
                "authMethod": "idc",
                "runtimeRegion": "eu-central-1",
                "profileProvenance": "profile_resolution_required"
            }
        }))
        .unwrap();
        let catalog = model_catalog(&http, &unresolved, Some(&endpoint)).await;
        assert!(catalog.descriptors.is_empty());
        assert_eq!(catalog.source, "kiro_identity_unresolved");
        assert!(!catalog.stale);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn discovery_retries_profileless_only_after_profile_forbidden() {
        use axum::response::IntoResponse;

        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed_for_route = std::sync::Arc::clone(&observed);
        let app = axum::Router::new().route(
            "/models",
            axum::routing::get(move |uri: axum::http::Uri| {
                let observed = std::sync::Arc::clone(&observed_for_route);
                async move {
                    let query = uri.query().unwrap_or_default().to_string();
                    observed.lock().unwrap().push(query.clone());
                    if query.contains("profileArn=") {
                        (
                            axum::http::StatusCode::FORBIDDEN,
                            axum::Json(json!({"message":"profile denied"})),
                        )
                            .into_response()
                    } else {
                        axum::Json(json!({"models":[{"modelId":"profileless-model"}]}))
                            .into_response()
                    }
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let account: Account = serde_json::from_value(json!({
            "id": format!("kiro-profile-fallback-{}", crate::infra::time::now_ms()),
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 1,
            "accessToken": "valid-token",
            "profile": {
                "authMethod": "social",
                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test"
            }
        }))
        .unwrap();
        let catalog = model_catalog(
            &reqwest::Client::new(),
            &account,
            Some(&format!("http://{address}/models?origin=AI_EDITOR")),
        )
        .await;
        assert_eq!(
            catalog.model_ids().collect::<Vec<_>>(),
            ["profileless-model"]
        );
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 2);
        assert!(observed[0].contains("profileArn="));
        assert!(!observed[1].contains("profileArn="));
        server.abort();
    }

    #[tokio::test]
    async fn model_catalog_distinguishes_authoritative_empty_denial_and_transient_failure() {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new()
            .route(
                "/empty",
                axum::routing::get(|| async { axum::Json(json!({"models": []})) }),
            )
            .route(
                "/denied",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::UNAUTHORIZED,
                        axum::Json(json!({"message": "expired"})),
                    )
                }),
            )
            .route(
                "/transient",
                axum::routing::get(|| async {
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(json!({"message": "retry later"})),
                    )
                }),
            );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let unique = crate::infra::time::now_ms();
        let account = |suffix: &str| -> Account {
            serde_json::from_value(json!({
                "id": format!("kiro-model-failure-{unique}-{suffix}"),
                "providerType": "kiro_oauth",
                "authIdentityGeneration": 1,
                "accessToken": "valid-token",
                "profile": {
                    "authMethod": "social",
                    "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test"
                }
            }))
            .unwrap()
        };
        let http = reqwest::Client::new();

        let empty = model_catalog(
            &http,
            &account("empty"),
            Some(&format!("http://{address}/empty")),
        )
        .await;
        assert!(empty.descriptors.is_empty());
        assert_eq!(empty.source, "kiro_list_available_models");
        assert!(!empty.stale);

        let denied = model_catalog(
            &http,
            &account("denied"),
            Some(&format!("http://{address}/denied")),
        )
        .await;
        assert!(denied.descriptors.is_empty());
        assert_eq!(denied.source, "kiro_model_access_unavailable");
        assert!(!denied.stale);

        let transient = model_catalog(
            &http,
            &account("transient"),
            Some(&format!("http://{address}/transient")),
        )
        .await;
        assert!(transient.descriptors.is_empty());
        assert_eq!(
            transient.source,
            "kiro_model_discovery_transient_without_cache"
        );
        assert!(!transient.stale);
        server.abort();
    }

    #[tokio::test]
    async fn hot_path_ignores_catalog_entries_beyond_the_stale_ttl() {
        let account: Account = serde_json::from_value(json!({
            "id": format!("kiro-expired-cache-{}", crate::infra::time::now_ms()),
            "providerType": "kiro_oauth",
            "authIdentityGeneration": 7,
            "tokenRefreshGeneration": 3,
            "accessToken": "fixture-token",
            "profile": {
                "authMethod": "social",
                "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/test"
            }
        }))
        .unwrap();
        let identity =
            crate::domain::providers::kiro::operational_runtime_identity_from_account(&account)
                .unwrap();
        let scope = KiroModelCatalogScope::fixture();
        let key = ModelCacheKey {
            app: scope.app.clone(),
            provider_id: scope.provider_id.clone(),
            provider_revision: scope.provider_revision,
            runtime_fingerprint: scope.runtime_fingerprint.clone(),
            account_id: account.id.clone(),
            auth_identity_generation: account.auth_identity_generation,
            token_refresh_generation: account.token_refresh_generation,
            profile_scope: identity.profile_arn.unwrap(),
            runtime_region: identity.runtime_region,
        };
        let descriptor = KiroModelDescriptor {
            model_id: "expired-model".to_string(),
            display_name: None,
            description: None,
            max_input_tokens: None,
            max_output_tokens: None,
        };
        CACHE.lock().await.models.insert(
            key,
            CachedModels {
                descriptors: vec![descriptor],
                fetched_at_ms: (now_ms().min(i64::MAX as u128) as i64)
                    .saturating_sub(STALE_MODEL_CACHE_TTL_MS),
            },
        );

        assert_eq!(
            cached_model_descriptor(&account, &scope, "expired-model").await,
            None
        );
    }
}
