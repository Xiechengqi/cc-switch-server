use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use serde::Deserialize;
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

#[derive(Debug, Clone)]
pub struct KiroModelCatalog {
    pub models: Vec<String>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct CachedModels {
    models: Vec<String>,
    fetched_at_ms: i64,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ModelCacheKey {
    account_id: String,
    auth_identity_generation: u64,
    token_refresh_generation: u64,
    profile_scope: String,
    runtime_region: String,
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
    #[serde(default)]
    models: Vec<AvailableModel>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AvailableModel {
    model_id: String,
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
}

impl ModelDiscoveryFailure {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            kind: ModelDiscoveryFailureKind::Transient,
            message: message.into(),
        }
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: ModelDiscoveryFailureKind::Unavailable,
            message: message.into(),
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

pub async fn model_catalog(
    http: &reqwest::Client,
    account: &Account,
    endpoint_override: Option<&str>,
) -> KiroModelCatalog {
    model_catalog_with_timeout(http, account, endpoint_override, Duration::from_secs(10)).await
}

pub async fn model_catalog_with_timeout(
    http: &reqwest::Client,
    account: &Account,
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
            models: cached.models,
            source: "kiro_account_cache",
            stale: false,
            fetched_at_ms: Some(cached.fetched_at_ms),
        };
    }

    let fetched = fetch_models(
        http,
        account,
        &identity.runtime_region,
        access_token,
        endpoint_override,
        request_timeout,
    )
    .await;
    match fetched {
        Ok(models) => {
            CACHE.lock().await.models.insert(
                cache_key,
                CachedModels {
                    models: models.clone(),
                    fetched_at_ms: now,
                },
            );
            KiroModelCatalog {
                models,
                source: "kiro_list_available_models",
                stale: false,
                fetched_at_ms: Some(now),
            }
        }
        Err(error) if error.kind == ModelDiscoveryFailureKind::Unavailable => {
            tracing::warn!(account_id = %account.id, error = %error, "Kiro model discovery rejected bound account capability");
            unavailable_model_catalog("kiro_model_access_unavailable")
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
                    models: cached.models,
                    source: "kiro_account_cache",
                    stale: true,
                    fetched_at_ms: Some(cached.fetched_at_ms),
                }
            } else {
                static_model_catalog("kiro_static_fallback")
            }
        }
    }
}

pub fn static_model_catalog(source: &'static str) -> KiroModelCatalog {
    KiroModelCatalog {
        models: crate::domain::providers::kiro::STATIC_MODEL_IDS
            .iter()
            .map(|model| (*model).to_string())
            .collect(),
        source,
        stale: true,
        fetched_at_ms: None,
    }
}

pub fn unavailable_model_catalog(source: &'static str) -> KiroModelCatalog {
    KiroModelCatalog {
        models: Vec::new(),
        source,
        stale: false,
        fetched_at_ms: None,
    }
}

async fn fetch_models(
    http: &reqwest::Client,
    account: &Account,
    region: &str,
    access_token: &str,
    endpoint_override: Option<&str>,
    request_timeout: Duration,
) -> Result<Vec<String>, ModelDiscoveryFailure> {
    let host = format!("q.{region}.amazonaws.com");
    let url = endpoint_override
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://{host}/ListAvailableModels?origin=AI_EDITOR"));
    let machine_id = account_string(account, &["/machineId", "/machine_id"])
        .unwrap_or_else(|| account.id.clone());
    fetch_models_request(
        http,
        &url,
        &host,
        access_token,
        &machine_id,
        kiro_token_type(account),
        request_timeout,
    )
    .await
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
) -> Result<Vec<String>, ModelDiscoveryFailure> {
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
        .map_err(|error| ModelDiscoveryFailure::transient(error.to_string()))?;
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
                ModelDiscoveryFailure::transient(message)
            } else {
                ModelDiscoveryFailure::unavailable(message)
            },
        );
    }
    let payload = response
        .json::<AvailableModelsResponse>()
        .await
        .map_err(|error| ModelDiscoveryFailure::transient(error.to_string()))?;
    let mut models = payload
        .models
        .into_iter()
        .map(|model| model.model_id.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

pub fn model_discovery_url(account: &Account) -> Result<String, String> {
    let region = account_runtime_region(account)?;
    Ok(format!(
        "https://q.{region}.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"
    ))
}

fn account_runtime_region(account: &Account) -> Result<String, String> {
    crate::domain::providers::kiro::operational_runtime_identity_from_account(account)
        .map(|identity| identity.runtime_region)
        .map_err(|error| format!("bound account has {error}"))
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
        let mut sorted = catalog.models.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(catalog.models, sorted);
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
            "validated_identity_and_credential_after_upstream_failure_only"
        );
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
        assert_eq!(
            model_discovery_url(&account).unwrap(),
            "https://q.us-east-1.amazonaws.com/ListAvailableModels?origin=AI_EDITOR"
        );
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
        assert_eq!(
            model_discovery_url(&account).unwrap(),
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
                    axum::Json(json!({"models": [{"modelId": model}]}))
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

        assert_eq!(first_a.models, ["model-account-a"]);
        assert_eq!(cached_a.models, ["model-account-a"]);
        assert_eq!(cached_a.source, "kiro_account_cache");
        assert_eq!(first_b.models, ["model-account-b"]);
        assert_eq!(refreshed.models, ["model-account-a-refreshed"]);
        assert_eq!(second_generation.models, ["model-account-a-generation-2"]);
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
        assert_eq!(profile_a.models, ["profile-a-model"]);
        assert_eq!(profile_b.models, ["profile-b-model"]);
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
        assert!(catalog.models.is_empty());
        assert_eq!(catalog.source, "kiro_identity_unresolved");
        assert!(!catalog.stale);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
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
        assert!(empty.models.is_empty());
        assert_eq!(empty.source, "kiro_list_available_models");
        assert!(!empty.stale);

        let denied = model_catalog(
            &http,
            &account("denied"),
            Some(&format!("http://{address}/denied")),
        )
        .await;
        assert!(denied.models.is_empty());
        assert_eq!(denied.source, "kiro_model_access_unavailable");
        assert!(!denied.stale);

        let transient = model_catalog(
            &http,
            &account("transient"),
            Some(&format!("http://{address}/transient")),
        )
        .await;
        assert_eq!(
            transient.models,
            crate::domain::providers::kiro::STATIC_MODEL_IDS
        );
        assert_eq!(transient.source, "kiro_static_fallback");
        assert!(transient.stale);
        server.abort();
    }
}
