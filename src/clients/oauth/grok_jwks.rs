use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{
    AlgorithmParameters, EllipticCurve, Jwk, JwkSet, KeyAlgorithm, KeyOperations, PublicKeyUse,
};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::RwLock;
use url::Url;

const DEFAULT_XAI_DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const DEFAULT_XAI_ISSUER: &str = "https://auth.x.ai";
const DEFAULT_XAI_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_ID_TOKEN_ALGORITHM: Algorithm = Algorithm::ES256;
const XAI_ID_TOKEN_ALGORITHM_NAME: &str = "ES256";
const CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DISCOVERY_RESPONSE_BODY_BYTES: usize = 64 * 1024;
const MAX_JWKS_RESPONSE_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum GrokJwtError {
    #[error("Grok token header is invalid: {0}")]
    InvalidHeader(String),
    #[error("Grok ID token must use ES256")]
    UnsupportedAlgorithm,
    #[error("Grok ID token header is missing kid")]
    MissingKeyId,
    #[error("Grok OIDC discovery failed: {0}")]
    Discovery(String),
    #[error("Grok OIDC metadata is invalid: {0}")]
    InvalidMetadata(String),
    #[error("fetch Grok JWKS failed: {0}")]
    Fetch(String),
    #[error("Grok JWKS does not contain key {0}")]
    UnknownKey(String),
    #[error("Grok JWKS key is invalid: {0}")]
    InvalidKey(String),
    #[error("Grok ID token verification failed: {0}")]
    Verification(String),
    #[error("Grok ID token identity is invalid: {0}")]
    Identity(String),
}

#[derive(Debug, Clone)]
pub struct VerifiedGrokIdentity {
    pub identity: crate::domain::accounts::oauth::OAuthIdentity,
    pub canonical_claims: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct DiscoveryDocument {
    issuer: String,
    jwks_uri: String,
    #[serde(default)]
    id_token_signing_alg_values_supported: Vec<String>,
}

#[derive(Debug, Clone)]
struct ProviderMetadata {
    issuer: String,
    jwks_uri: Url,
    algorithms: Vec<String>,
}

#[derive(Clone)]
struct CachedProvider {
    fetched_at: Instant,
    metadata: ProviderMetadata,
}

#[derive(Clone)]
struct CachedJwks {
    fetched_at: Instant,
    set: JwkSet,
}

fn provider_cache() -> &'static RwLock<BTreeMap<String, CachedProvider>> {
    static CACHE: OnceLock<RwLock<BTreeMap<String, CachedProvider>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn jwks_cache() -> &'static RwLock<BTreeMap<String, CachedJwks>> {
    static CACHE: OnceLock<RwLock<BTreeMap<String, CachedJwks>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(BTreeMap::new()))
}

pub async fn verify_grok_id_token(
    http: &reqwest::Client,
    token: &str,
    expected_nonce: Option<&str>,
) -> Result<VerifiedGrokIdentity, GrokJwtError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(GrokJwtError::Identity("id_token is required".to_string()));
    }
    let header =
        decode_header(token).map_err(|error| GrokJwtError::InvalidHeader(error.to_string()))?;
    if header.alg != XAI_ID_TOKEN_ALGORITHM {
        return Err(GrokJwtError::UnsupportedAlgorithm);
    }
    let kid = header.kid.ok_or(GrokJwtError::MissingKeyId)?;
    let provider = load_provider(http).await?;
    if !provider
        .algorithms
        .iter()
        .any(|algorithm| algorithm == XAI_ID_TOKEN_ALGORITHM_NAME)
    {
        return Err(GrokJwtError::InvalidMetadata(
            "provider does not advertise ES256 for ID tokens".to_string(),
        ));
    }
    let jwk = load_key(http, &provider.jwks_uri, &kid).await?;
    validate_grok_signing_key(&jwk)?;
    let key =
        DecodingKey::from_jwk(&jwk).map_err(|error| GrokJwtError::InvalidKey(error.to_string()))?;
    let mut validation = Validation::new(XAI_ID_TOKEN_ALGORITHM);
    validation.set_issuer(&[provider.issuer]);
    validation.set_audience(&[xai_client_id()]);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.required_spec_claims = ["exp", "iss", "aud", "sub"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let claims = decode::<Value>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|error| GrokJwtError::Verification(error.to_string()))?;

    if let Some(expected_nonce) = expected_nonce
        .map(str::trim)
        .filter(|nonce| !nonce.is_empty())
    {
        let actual_nonce = claims
            .get("nonce")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|nonce| !nonce.is_empty());
        if actual_nonce != Some(expected_nonce) {
            return Err(GrokJwtError::Verification(
                "nonce does not match the OAuth login session".to_string(),
            ));
        }
    }

    let identity = crate::domain::accounts::oauth::grok_identity_from_claims(&claims);
    let subject = identity
        .subject
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if subject.is_empty() {
        return Err(GrokJwtError::Identity(
            "verified ID token does not contain subject".to_string(),
        ));
    }
    Ok(VerifiedGrokIdentity {
        canonical_claims: crate::domain::accounts::oauth::canonical_grok_claims(&identity),
        identity,
    })
}

fn validate_grok_signing_key(jwk: &Jwk) -> Result<(), GrokJwtError> {
    if jwk.common.key_algorithm != Some(KeyAlgorithm::ES256) {
        return Err(GrokJwtError::InvalidKey(
            "key must declare alg=ES256".to_string(),
        ));
    }
    if jwk
        .common
        .public_key_use
        .as_ref()
        .is_some_and(|key_use| key_use != &PublicKeyUse::Signature)
    {
        return Err(GrokJwtError::InvalidKey(
            "key use must be sig when present".to_string(),
        ));
    }
    if jwk
        .common
        .key_operations
        .as_ref()
        .is_some_and(|operations| !operations.contains(&KeyOperations::Verify))
    {
        return Err(GrokJwtError::InvalidKey(
            "key_ops must include verify when present".to_string(),
        ));
    }
    match &jwk.algorithm {
        AlgorithmParameters::EllipticCurve(parameters)
            if parameters.curve == EllipticCurve::P256 =>
        {
            Ok(())
        }
        _ => Err(GrokJwtError::InvalidKey(
            "key must use EC P-256 parameters".to_string(),
        )),
    }
}

async fn load_provider(http: &reqwest::Client) -> Result<ProviderMetadata, GrokJwtError> {
    let discovery_url = xai_discovery_url();
    validate_xai_endpoint(&discovery_url, &xai_discovery_hosts())?;
    if let Some(cached) = provider_cache().read().await.get(&discovery_url).cloned() {
        if cached.fetched_at.elapsed() < CACHE_TTL {
            return Ok(cached.metadata);
        }
    }

    let document = fetch_discovery_document(http, &discovery_url).await?;
    let expected_issuer = xai_issuer();
    if document.issuer.trim_end_matches('/') != expected_issuer {
        return Err(GrokJwtError::InvalidMetadata(format!(
            "issuer does not match {expected_issuer}"
        )));
    }
    validate_xai_endpoint(&document.issuer, &xai_discovery_hosts())?;
    let jwks_uri = validate_xai_endpoint(&document.jwks_uri, &xai_jwks_hosts())?;
    let metadata = ProviderMetadata {
        issuer: document.issuer.trim_end_matches('/').to_string(),
        jwks_uri,
        algorithms: document.id_token_signing_alg_values_supported,
    };
    provider_cache().write().await.insert(
        discovery_url,
        CachedProvider {
            fetched_at: Instant::now(),
            metadata: metadata.clone(),
        },
    );
    Ok(metadata)
}

async fn fetch_discovery_document(
    http: &reqwest::Client,
    discovery_url: &str,
) -> Result<DiscoveryDocument, GrokJwtError> {
    let mut response = http
        .get(discovery_url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|error| GrokJwtError::Discovery(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(GrokJwtError::Discovery(format!("HTTP {status}")));
    }
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_DISCOVERY_RESPONSE_BODY_BYTES,
    )
    .await
    .map_err(|error| GrokJwtError::Discovery(error.to_string()))?;
    let document = serde_json::from_slice::<DiscoveryDocument>(&body)
        .map_err(|error| GrokJwtError::Discovery(error.to_string()))?;
    Ok(document)
}

async fn load_key(http: &reqwest::Client, jwks_uri: &Url, kid: &str) -> Result<Jwk, GrokJwtError> {
    let cache_key = jwks_uri.as_str();
    if let Some(key) = cached_key(cache_key, kid).await {
        return Ok(key);
    }
    let mut response = http
        .get(jwks_uri.clone())
        .timeout(FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|error| GrokJwtError::Fetch(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(GrokJwtError::Fetch(format!("HTTP {status}")));
    }
    let body =
        crate::infra::http::read_response_body_limited(&mut response, MAX_JWKS_RESPONSE_BODY_BYTES)
            .await
            .map_err(|error| GrokJwtError::Fetch(error.to_string()))?;
    let set = serde_json::from_slice::<JwkSet>(&body)
        .map_err(|error| GrokJwtError::Fetch(error.to_string()))?;
    if set.keys.is_empty() {
        return Err(GrokJwtError::Fetch("empty key set".to_string()));
    }
    let key = set
        .keys
        .iter()
        .find(|key| key.common.key_id.as_deref() == Some(kid))
        .cloned()
        .ok_or_else(|| GrokJwtError::UnknownKey(kid.to_string()))?;
    jwks_cache().write().await.insert(
        cache_key.to_string(),
        CachedJwks {
            fetched_at: Instant::now(),
            set,
        },
    );
    Ok(key)
}

async fn cached_key(jwks_uri: &str, kid: &str) -> Option<Jwk> {
    let guard = jwks_cache().read().await;
    let cached = guard.get(jwks_uri)?;
    if cached.fetched_at.elapsed() >= CACHE_TTL {
        return None;
    }
    cached
        .set
        .keys
        .iter()
        .find(|key| key.common.key_id.as_deref() == Some(kid))
        .cloned()
}

fn validate_xai_endpoint(raw: &str, allowed_hosts: &[String]) -> Result<Url, GrokJwtError> {
    let url = Url::parse(raw).map_err(|error| GrokJwtError::InvalidMetadata(error.to_string()))?;
    let host = url
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| GrokJwtError::InvalidMetadata("endpoint has no host".to_string()))?;
    #[cfg(test)]
    let loopback_override =
        url.scheme() == "http" && matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    #[cfg(not(test))]
    let loopback_override = false;
    if !loopback_override
        && (url.scheme() != "https"
            || url.port_or_known_default() != Some(443)
            || !allowed_hosts.iter().any(|allowed| allowed == &host))
    {
        return Err(GrokJwtError::InvalidMetadata(format!(
            "endpoint host is not allowed: {raw}"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(GrokJwtError::InvalidMetadata(
            "endpoint must not contain credentials or a fragment".to_string(),
        ));
    }
    Ok(url)
}

fn xai_discovery_url() -> String {
    #[cfg(test)]
    if let Some(value) = env_non_empty("CC_SWITCH_XAI_OIDC_DISCOVERY_URL") {
        return value;
    }
    DEFAULT_XAI_DISCOVERY_URL.to_string()
}

fn xai_issuer() -> String {
    #[cfg(test)]
    if let Some(value) = env_non_empty("CC_SWITCH_XAI_OIDC_ISSUER") {
        return value.trim_end_matches('/').to_string();
    }
    DEFAULT_XAI_ISSUER.to_string()
}

fn xai_client_id() -> String {
    env_non_empty("CC_SWITCH_SERVER_XAI_CLIENT_ID")
        .unwrap_or_else(|| DEFAULT_XAI_CLIENT_ID.to_string())
}

fn xai_discovery_hosts() -> Vec<String> {
    vec!["auth.x.ai".to_string()]
}

fn xai_jwks_hosts() -> Vec<String> {
    vec!["auth.x.ai".to_string()]
}

fn env_non_empty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
pub(crate) async fn install_test_provider(jwk: Jwk) {
    let discovery_url = xai_discovery_url();
    let jwks_uri = Url::parse("https://auth.x.ai/.well-known/jwks.json").unwrap();
    provider_cache().write().await.insert(
        discovery_url,
        CachedProvider {
            fetched_at: Instant::now(),
            metadata: ProviderMetadata {
                issuer: DEFAULT_XAI_ISSUER.to_string(),
                jwks_uri: jwks_uri.clone(),
                algorithms: vec![XAI_ID_TOKEN_ALGORITHM_NAME.to_string()],
            },
        },
    );
    jwks_cache().write().await.insert(
        jwks_uri.to_string(),
        CachedJwks {
            fetched_at: Instant::now(),
            set: JwkSet { keys: vec![jwk] },
        },
    );
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const TEST_KID: &str = "cc-switch-grok-test-ec";
    const TEST_EC_X: &str = "w7JAoU_gJbZJvV-zCOvU9yFJq0FNC_edCMRM78P8eQQ";
    const TEST_EC_Y: &str = "wQg1EytcsEmGrM70Gb53oluoDbVhCZ3Uq3hHMslHVb4";
    const TEST_EC_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgWTFfCGljY6aw3Hrt
kHmPRiazukxPLb6ilpRAewjW8nihRANCAATDskChT+Altkm9X7MI69T3IUmrQU0L
950IxEzvw/x5BMEINRMrXLBJhqzO9Bm+d6JbqA21YQmd1Kt4RzLJR1W+
-----END PRIVATE KEY-----"#;

    pub(crate) async fn install_test_key() {
        let jwk = serde_json::from_value(serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": TEST_EC_X,
            "y": TEST_EC_Y,
            "kid": TEST_KID,
            "alg": "ES256",
            "use": "sig"
        }))
        .unwrap();
        install_test_provider(jwk).await;
    }

    pub(crate) fn signed_token(audience: &str, nonce: &str, expires_at: i64) -> String {
        let now = chrono::Utc::now().timestamp();
        let mut header = jsonwebtoken::Header::new(XAI_ID_TOKEN_ALGORITHM);
        header.kid = Some(TEST_KID.to_string());
        jsonwebtoken::encode(
            &header,
            &serde_json::json!({
                "iss": DEFAULT_XAI_ISSUER,
                "aud": audience,
                "sub": "xai-user-123",
                "email": "verified@example.com",
                "tier": "supergrok",
                "nonce": nonce,
                "iat": now,
                "nbf": now - 1,
                "exp": expires_at,
            }),
            &jsonwebtoken::EncodingKey::from_ec_pem(TEST_EC_PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn endpoint_policy_rejects_non_https_and_unlisted_hosts() {
        let hosts = vec!["auth.x.ai".to_string()];
        assert!(validate_xai_endpoint("https://auth.x.ai/jwks", &hosts).is_ok());
        assert!(validate_xai_endpoint("http://auth.x.ai/jwks", &hosts).is_err());
        assert!(validate_xai_endpoint("https://auth.x.ai.example/jwks", &hosts).is_err());
        assert!(validate_xai_endpoint("https://user@auth.x.ai/jwks", &hosts).is_err());
        assert!(validate_xai_endpoint("http://127.0.0.1:1234/jwks", &hosts).is_ok());
    }

    #[test]
    fn rejects_non_es256_tokens_before_discovery_access() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(verify_grok_id_token(
                &reqwest::Client::new(),
                "eyJhbGciOiJSUzI1NiIsImtpZCI6InRlc3QifQ.eyJleHAiOjQxMDI0NDQ4MDB9.sig",
                None,
            ))
            .unwrap_err();
        assert!(matches!(error, GrokJwtError::UnsupportedAlgorithm));
    }

    #[test]
    fn rejects_jwks_outside_the_es256_verification_contract() {
        let rsa: Jwk = serde_json::from_value(serde_json::json!({
            "kty": "RSA",
            "n": "AQAB",
            "e": "AQAB",
            "kid": TEST_KID,
            "alg": "RS256",
            "use": "sig"
        }))
        .unwrap();
        assert!(matches!(
            validate_grok_signing_key(&rsa),
            Err(GrokJwtError::InvalidKey(_))
        ));

        let p384: Jwk = serde_json::from_value(serde_json::json!({
            "kty": "EC",
            "crv": "P-384",
            "x": TEST_EC_X,
            "y": TEST_EC_Y,
            "kid": TEST_KID,
            "alg": "ES256",
            "use": "sig"
        }))
        .unwrap();
        assert!(matches!(
            validate_grok_signing_key(&p384),
            Err(GrokJwtError::InvalidKey(_))
        ));

        let signing_only: Jwk = serde_json::from_value(serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": TEST_EC_X,
            "y": TEST_EC_Y,
            "kid": TEST_KID,
            "alg": "ES256",
            "key_ops": ["sign"]
        }))
        .unwrap();
        assert!(matches!(
            validate_grok_signing_key(&signing_only),
            Err(GrokJwtError::InvalidKey(_))
        ));

        let verification_key: Jwk = serde_json::from_value(serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": TEST_EC_X,
            "y": TEST_EC_Y,
            "kid": TEST_KID,
            "alg": "ES256",
            "key_ops": ["verify"]
        }))
        .unwrap();
        assert!(validate_grok_signing_key(&verification_key).is_ok());
    }

    #[tokio::test]
    async fn discovery_response_body_limit_is_enforced() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_DISCOVERY_RESPONSE_BODY_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let error = fetch_discovery_document(
            &reqwest::Client::new(),
            &format!("http://{address}/.well-known/openid-configuration"),
        )
        .await
        .unwrap_err();
        assert!(matches!(error, GrokJwtError::Discovery(_)));
        assert!(error.to_string().contains("response body exceeds"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn jwks_response_body_limit_is_enforced() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_JWKS_RESPONSE_BODY_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        let uri = Url::parse(&format!("http://{address}/jwks")).unwrap();
        let error = load_key(&reqwest::Client::new(), &uri, "unknown-kid")
            .await
            .unwrap_err();
        assert!(matches!(error, GrokJwtError::Fetch(_)));
        assert!(error.to_string().contains("response body exceeds"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn verifies_signature_issuer_audience_expiry_and_nonce() {
        install_test_key().await;
        let now = chrono::Utc::now().timestamp();
        let valid = signed_token(DEFAULT_XAI_CLIENT_ID, "oauth-state", now + 3_600);
        let verified = verify_grok_id_token(&reqwest::Client::new(), &valid, Some("oauth-state"))
            .await
            .unwrap();
        assert_eq!(verified.identity.subject.as_deref(), Some("xai-user-123"));
        assert_eq!(verified.canonical_claims["email"], "verified@example.com");

        let wrong_audience = signed_token("another-client", "oauth-state", now + 3_600);
        assert!(matches!(
            verify_grok_id_token(
                &reqwest::Client::new(),
                &wrong_audience,
                Some("oauth-state")
            )
            .await,
            Err(GrokJwtError::Verification(_))
        ));
        assert!(matches!(
            verify_grok_id_token(&reqwest::Client::new(), &valid, Some("different-state")).await,
            Err(GrokJwtError::Verification(_))
        ));
        let expired = signed_token(DEFAULT_XAI_CLIENT_ID, "oauth-state", now - 3_600);
        assert!(matches!(
            verify_grok_id_token(&reqwest::Client::new(), &expired, Some("oauth-state")).await,
            Err(GrokJwtError::Verification(_))
        ));
    }
}
