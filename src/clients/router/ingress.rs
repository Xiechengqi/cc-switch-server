use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const INGRESS_CONTEXT_HEADER: &str = "x-cc-switch-ingress-context";
pub const INGRESS_SIGNATURE_HEADER: &str = "x-cc-switch-ingress-signature";
pub const DEFAULT_MAX_CONTEXT_AGE_MS: i64 = 30_000;
pub const DEFAULT_FUTURE_CLOCK_SKEW_MS: i64 = 5_000;
const SIGNING_DOMAIN: &str = "cc-switch-router-ingress-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IngressContext {
    pub protocol_epoch: String,
    pub router_id: String,
    pub route_id: String,
    pub installation_id: String,
    pub target_lane_id: String,
    pub public_host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    pub issued_at_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum IngressContextError {
    #[error("ingress context header is not valid base64url")]
    InvalidEncoding,
    #[error("ingress context signature is invalid")]
    InvalidSignature,
    #[error("ingress context JSON is invalid: {0}")]
    InvalidJson(#[source] serde_json::Error),
    #[error("ingress context protocol epoch is unsupported")]
    UnsupportedEpoch,
    #[error("ingress context is expired")]
    Expired,
    #[error("ingress context timestamp is in the future")]
    FutureTimestamp,
    #[error("ingress context router does not match the receiving binding")]
    RouterMismatch,
    #[error("ingress context installation does not match the receiving binding")]
    InstallationMismatch,
    #[error("ingress context contains an invalid required field")]
    InvalidField,
}

pub fn verify(
    encoded_context: &str,
    signature: &str,
    control_secret: &str,
    expected_router_id: &str,
    expected_installation_id: &str,
    now_ms: i64,
) -> Result<IngressContext, IngressContextError> {
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| IngressContextError::InvalidEncoding)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(control_secret.as_bytes())
        .map_err(|_| IngressContextError::InvalidSignature)?;
    mac.update(SIGNING_DOMAIN.as_bytes());
    mac.update(b"\n");
    mac.update(PROTOCOL_EPOCH.as_bytes());
    mac.update(b"\n");
    mac.update(encoded_context.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| IngressContextError::InvalidSignature)?;

    let json = URL_SAFE_NO_PAD
        .decode(encoded_context)
        .map_err(|_| IngressContextError::InvalidEncoding)?;
    let context = serde_json::from_slice::<IngressContext>(&json)
        .map_err(IngressContextError::InvalidJson)?;
    validate(
        &context,
        expected_router_id,
        expected_installation_id,
        now_ms,
    )?;
    Ok(context)
}

fn validate(
    context: &IngressContext,
    expected_router_id: &str,
    expected_installation_id: &str,
    now_ms: i64,
) -> Result<(), IngressContextError> {
    if context.protocol_epoch != PROTOCOL_EPOCH {
        return Err(IngressContextError::UnsupportedEpoch);
    }
    if context.router_id != expected_router_id {
        return Err(IngressContextError::RouterMismatch);
    }
    if context.installation_id != expected_installation_id {
        return Err(IngressContextError::InstallationMismatch);
    }
    if context.route_id.trim().is_empty()
        || context.target_lane_id.trim().is_empty()
        || context.public_host.trim().is_empty()
        || context.public_host != context.public_host.to_ascii_lowercase()
        || context.request_id.trim().is_empty()
        || context.issued_at_ms <= 0
    {
        return Err(IngressContextError::InvalidField);
    }
    if context.issued_at_ms > now_ms.saturating_add(DEFAULT_FUTURE_CLOCK_SKEW_MS) {
        return Err(IngressContextError::FutureTimestamp);
    }
    if now_ms.saturating_sub(context.issued_at_ms) > DEFAULT_MAX_CONTEXT_AGE_MS {
        return Err(IngressContextError::Expired);
    }
    if context.route_id.starts_with("share:") != context.share_id.is_some() {
        return Err(IngressContextError::InvalidField);
    }
    if context
        .user_role
        .as_deref()
        .is_some_and(|value| !matches!(value, "owner" | "admin"))
    {
        return Err(IngressContextError::InvalidField);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGH";
    const ENCODED: &str = "eyJwcm90b2NvbEVwb2NoIjoibmFtZXNwYWNlLWZsYXQtMSIsInJvdXRlcklkIjoicm91dGVyLWpwIiwicm91dGVJZCI6InNoYXJlOnNoYXJlLTEiLCJpbnN0YWxsYXRpb25JZCI6Imluc3RhbGxhdGlvbi0xIiwidGFyZ2V0TGFuZUlkIjoiaW5zdGFsbGF0aW9uLTE6bmFtZXNwYWNlLWRhdGEiLCJwdWJsaWNIb3N0IjoiY29kZXgtLWFscGhhLWlvc2c2aGlpZHV0cWNtaGNlZWZiLnJvdXRlci50ZXN0Iiwic2hhcmVJZCI6InNoYXJlLTEiLCJyZXF1ZXN0SWQiOiJyZXFfMTIzIiwidXNlckVtYWlsIjoib3duZXJAZXhhbXBsZS5jb20iLCJ1c2VyQ291bnRyeSI6IkpQIiwiaXNzdWVkQXRNcyI6MTc1MDAwMDAwMDAwMH0";
    const SIGNATURE: &str = "RvdTGpCCJwSxo7Kn8meZ0Vx3MaHf3YocqnzKyqJxTeU";

    #[test]
    fn verifies_the_router_test_vector() {
        let context = verify(
            ENCODED,
            SIGNATURE,
            SECRET,
            "router-jp",
            "installation-1",
            1_750_000_001_000,
        )
        .unwrap();
        assert_eq!(context.share_id.as_deref(), Some("share-1"));
        assert_eq!(context.request_id, "req_123");
    }

    #[test]
    fn rejects_tampering_cross_router_replay_and_expiry() {
        assert!(matches!(
            verify(
                ENCODED,
                SIGNATURE,
                SECRET,
                "router-us",
                "installation-1",
                1_750_000_001_000,
            ),
            Err(IngressContextError::RouterMismatch)
        ));
        assert!(matches!(
            verify(
                ENCODED,
                SIGNATURE,
                SECRET,
                "router-jp",
                "installation-1",
                1_750_000_100_000,
            ),
            Err(IngressContextError::Expired)
        ));
        let mut signature = SIGNATURE.to_string();
        signature.replace_range(..1, "A");
        assert!(matches!(
            verify(
                ENCODED,
                &signature,
                SECRET,
                "router-jp",
                "installation-1",
                1_750_000_001_000,
            ),
            Err(IngressContextError::InvalidSignature)
        ));
    }
}
