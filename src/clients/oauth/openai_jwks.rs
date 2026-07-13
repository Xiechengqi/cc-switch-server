use std::sync::OnceLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde_json::Value;
use tokio::sync::RwLock;

const DEFAULT_OPENAI_JWKS_URL: &str = "https://auth.openai.com/.well-known/jwks.json";
const DEFAULT_OPENAI_ISSUER: &str = "https://auth.openai.com";
const DEFAULT_OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const JWKS_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum OpenAiJwtError {
    #[error("OpenAI id_token header is invalid: {0}")]
    InvalidHeader(String),
    #[error("OpenAI id_token must use RS256")]
    UnsupportedAlgorithm,
    #[error("OpenAI id_token header is missing kid")]
    MissingKeyId,
    #[error("fetch OpenAI JWKS failed: {0}")]
    Fetch(String),
    #[error("OpenAI JWKS does not contain key {0}")]
    UnknownKey(String),
    #[error("OpenAI JWKS key is invalid: {0}")]
    InvalidKey(String),
    #[error("OpenAI id_token verification failed: {0}")]
    Verification(String),
}

#[derive(Clone)]
struct CachedJwks {
    fetched_at: Instant,
    set: JwkSet,
}

fn cache() -> &'static RwLock<Option<CachedJwks>> {
    static CACHE: OnceLock<RwLock<Option<CachedJwks>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(None))
}

pub async fn verify_openai_id_token(
    http: &reqwest::Client,
    token: &str,
) -> Result<Value, OpenAiJwtError> {
    let header =
        decode_header(token).map_err(|error| OpenAiJwtError::InvalidHeader(error.to_string()))?;
    if header.alg != Algorithm::RS256 {
        return Err(OpenAiJwtError::UnsupportedAlgorithm);
    }
    let kid = header.kid.ok_or(OpenAiJwtError::MissingKeyId)?;
    let mut jwk = cached_key(&kid).await;
    if jwk.is_none() {
        refresh_jwks(http).await?;
        jwk = cached_key(&kid).await;
    }
    let jwk = jwk.ok_or_else(|| OpenAiJwtError::UnknownKey(kid.clone()))?;
    let key = DecodingKey::from_jwk(&jwk)
        .map_err(|error| OpenAiJwtError::InvalidKey(error.to_string()))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[openai_issuer()]);
    validation.set_audience(&[openai_client_id()]);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.required_spec_claims = ["exp", "iss", "aud"]
        .into_iter()
        .map(str::to_string)
        .collect();
    decode::<Value>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|error| OpenAiJwtError::Verification(error.to_string()))
}

async fn cached_key(kid: &str) -> Option<Jwk> {
    let guard = cache().read().await;
    let cached = guard.as_ref()?;
    if cached.fetched_at.elapsed() >= JWKS_CACHE_TTL {
        return None;
    }
    cached
        .set
        .keys
        .iter()
        .find(|key| key.common.key_id.as_deref() == Some(kid))
        .cloned()
}

async fn refresh_jwks(http: &reqwest::Client) -> Result<(), OpenAiJwtError> {
    let response = http
        .get(openai_jwks_url())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| OpenAiJwtError::Fetch(error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(OpenAiJwtError::Fetch(format!("HTTP {status}")));
    }
    let set = response
        .json::<JwkSet>()
        .await
        .map_err(|error| OpenAiJwtError::Fetch(error.to_string()))?;
    if set.keys.is_empty() {
        return Err(OpenAiJwtError::Fetch("empty key set".to_string()));
    }
    *cache().write().await = Some(CachedJwks {
        fetched_at: Instant::now(),
        set,
    });
    Ok(())
}

fn openai_jwks_url() -> String {
    std::env::var("CC_SWITCH_OPENAI_JWKS_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_JWKS_URL.to_string())
}

fn openai_issuer() -> String {
    std::env::var("CC_SWITCH_OPENAI_TOKEN_ISSUER")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_ISSUER.to_string())
}

fn openai_client_id() -> String {
    std::env::var("CODEX_OAUTH_CLIENT_ID")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_CLIENT_ID.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsigned_tokens_before_network_access() {
        let token = "eyJhbGciOiJIUzI1NiIsImtpZCI6InRlc3QifQ.eyJleHAiOjQxMDI0NDQ4MDB9.sig";
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let error = runtime
            .block_on(verify_openai_id_token(&reqwest::Client::new(), token))
            .unwrap_err();
        assert!(matches!(error, OpenAiJwtError::UnsupportedAlgorithm));
    }
}
