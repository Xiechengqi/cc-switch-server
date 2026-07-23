use std::sync::OnceLock;
use std::time::{Duration, Instant};

use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use serde_json::Value;
use tokio::sync::RwLock;

const DEFAULT_OPENAI_JWKS_URL: &str = "https://auth.openai.com/.well-known/jwks.json";
const DEFAULT_OPENAI_ISSUER: &str = "https://auth.openai.com";
const DEFAULT_OPENAI_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_OPENAI_ACCESS_TOKEN_AUDIENCE: &str = "https://api.openai.com/v1";
const JWKS_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum OpenAiJwtError {
    #[error("OpenAI token header is invalid: {0}")]
    InvalidHeader(String),
    #[error("OpenAI token must use RS256")]
    UnsupportedAlgorithm,
    #[error("OpenAI token header is missing kid")]
    MissingKeyId,
    #[error("fetch OpenAI JWKS failed: {0}")]
    Fetch(String),
    #[error("OpenAI JWKS does not contain key {0}")]
    UnknownKey(String),
    #[error("OpenAI JWKS key is invalid: {0}")]
    InvalidKey(String),
    #[error("OpenAI token verification failed: {0}")]
    Verification(String),
    #[error("OpenAI token identity is invalid: {0}")]
    Identity(String),
}

#[derive(Debug, Clone)]
pub struct VerifiedOpenAiIdentity {
    pub identity: crate::domain::accounts::oauth::OAuthIdentity,
    pub canonical_claims: Value,
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
    verify_openai_id_token_with_expiry(http, token, true).await
}

/// Recover signed OpenAI identity claims from an expired persisted ID token.
/// This is only suitable for workspace identity migration, never authentication.
pub async fn verify_openai_id_token_identity(
    http: &reqwest::Client,
    token: &str,
) -> Result<Value, OpenAiJwtError> {
    verify_openai_id_token_with_expiry(http, token, false).await
}

async fn verify_openai_id_token_with_expiry(
    http: &reqwest::Client,
    token: &str,
    validate_exp: bool,
) -> Result<Value, OpenAiJwtError> {
    verify_openai_token(http, token, openai_id_token_validation(validate_exp)).await
}

pub async fn verify_openai_access_token(
    http: &reqwest::Client,
    token: &str,
) -> Result<Value, OpenAiJwtError> {
    verify_openai_token(http, token, openai_access_token_validation(true)).await
}

pub async fn verify_openai_identity_tokens(
    http: &reqwest::Client,
    id_token: Option<&str>,
    access_token: &str,
) -> Result<VerifiedOpenAiIdentity, OpenAiJwtError> {
    let id_identity =
        if let Some(id_token) = id_token.map(str::trim).filter(|token| !token.is_empty()) {
            let claims = verify_openai_id_token(http, id_token).await?;
            crate::domain::accounts::oauth::openai_identity_from_claims(&claims)
        } else {
            Default::default()
        };
    let access_token = access_token.trim();
    if access_token.is_empty() {
        return Err(OpenAiJwtError::Identity(
            "access_token is required".to_string(),
        ));
    }
    let access_claims = verify_openai_access_token(http, access_token).await?;
    let access_identity =
        crate::domain::accounts::oauth::openai_identity_from_claims(&access_claims);
    let identity = crate::domain::accounts::oauth::merge_verified_openai_identities(
        id_identity,
        access_identity,
    )
    .map_err(OpenAiJwtError::Identity)?;
    validate_verified_openai_identity(&identity)?;
    let canonical_claims = crate::domain::accounts::oauth::canonical_openai_claims(&identity);
    Ok(VerifiedOpenAiIdentity {
        identity,
        canonical_claims,
    })
}

fn validate_verified_openai_identity(
    identity: &crate::domain::accounts::oauth::OAuthIdentity,
) -> Result<(), OpenAiJwtError> {
    if identity
        .subject
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(OpenAiJwtError::Identity(
            "verified ID and access tokens do not contain subject".to_string(),
        ));
    }
    if identity
        .account_id
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(OpenAiJwtError::Identity(
            "verified ID and access tokens do not contain chatgpt_account_id".to_string(),
        ));
    }
    Ok(())
}

async fn verify_openai_token(
    http: &reqwest::Client,
    token: &str,
    validation: Validation,
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
    decode::<Value>(token, &key, &validation)
        .map(|data| data.claims)
        .map_err(|error| OpenAiJwtError::Verification(error.to_string()))
}

fn openai_id_token_validation(validate_exp: bool) -> Validation {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[openai_issuer()]);
    validation.set_audience(&[openai_client_id()]);
    validation.validate_exp = validate_exp;
    validation.validate_nbf = true;
    validation.required_spec_claims = ["exp", "iss", "aud"]
        .into_iter()
        .map(str::to_string)
        .collect();
    validation
}

fn openai_access_token_validation(validate_exp: bool) -> Validation {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[openai_issuer()]);
    validation.set_audience(&openai_access_token_audiences());
    validation.validate_exp = validate_exp;
    validation.validate_nbf = true;
    validation.required_spec_claims = ["exp", "iss", "aud"]
        .into_iter()
        .map(str::to_string)
        .collect();
    validation
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

fn openai_access_token_audiences() -> Vec<String> {
    std::env::var("CC_SWITCH_OPENAI_ACCESS_TOKEN_AUDIENCES")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| vec![DEFAULT_OPENAI_ACCESS_TOKEN_AUDIENCE.to_string()])
}

#[cfg(test)]
pub(crate) async fn install_test_jwk(jwk: Jwk) {
    let mut guard = cache().write().await;
    let mut set = guard
        .as_ref()
        .map(|cached| cached.set.clone())
        .unwrap_or(JwkSet { keys: Vec::new() });
    set.keys
        .retain(|existing| existing.common.key_id != jwk.common.key_id);
    set.keys.push(jwk);
    *guard = Some(CachedJwks {
        fetched_at: Instant::now(),
        set,
    });
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

    #[test]
    fn identity_migration_relaxes_only_expiry_validation() {
        let login = openai_id_token_validation(true);
        let migration = openai_id_token_validation(false);

        assert!(login.validate_exp);
        assert!(!migration.validate_exp);
        assert!(migration.validate_nbf);
        assert!(migration.required_spec_claims.contains("exp"));
        assert!(migration.required_spec_claims.contains("iss"));
        assert!(migration.required_spec_claims.contains("aud"));
    }

    #[test]
    fn access_tokens_use_an_explicit_api_audience_policy() {
        let validation = openai_access_token_validation(true);
        assert!(validation.validate_exp);
        assert!(validation.validate_aud);
        assert!(validation.required_spec_claims.contains("aud"));
        assert_eq!(
            openai_access_token_audiences(),
            vec![DEFAULT_OPENAI_ACCESS_TOKEN_AUDIENCE.to_string()]
        );
    }

    #[test]
    fn verified_identity_requires_distinct_subject_and_workspace() {
        let missing_subject = crate::domain::accounts::oauth::OAuthIdentity {
            account_id: Some("workspace-1".to_string()),
            ..Default::default()
        };
        let error = validate_verified_openai_identity(&missing_subject).unwrap_err();
        assert!(error.to_string().contains("subject"));

        let missing_workspace = crate::domain::accounts::oauth::OAuthIdentity {
            subject: Some("user-1".to_string()),
            ..Default::default()
        };
        let error = validate_verified_openai_identity(&missing_workspace).unwrap_err();
        assert!(error.to_string().contains("chatgpt_account_id"));

        validate_verified_openai_identity(&crate::domain::accounts::oauth::OAuthIdentity {
            subject: Some("user-1".to_string()),
            account_id: Some("workspace-1".to_string()),
            ..Default::default()
        })
        .unwrap();
    }
}
