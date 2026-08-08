use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const INGRESS_CONTEXT_HEADER: &str = "x-cc-switch-ingress-context";
pub const INGRESS_SIGNATURE_HEADER: &str = "x-cc-switch-ingress-signature";
pub const INTERNAL_INGRESS_ERROR_HEADER: &str = "x-cc-switch-internal-ingress-error";
pub const INTERNAL_INGRESS_AGE_MS_HEADER: &str = "x-cc-switch-internal-ingress-age-ms";
pub const INTERNAL_INGRESS_SERVER_TIME_MS_HEADER: &str =
    "x-cc-switch-internal-ingress-server-time-ms";
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
    Expired { issued_at_ms: i64, now_ms: i64 },
    #[error("ingress context timestamp is in the future")]
    FutureTimestamp { issued_at_ms: i64, now_ms: i64 },
    #[error("ingress context router does not match the receiving binding")]
    RouterMismatch,
    #[error("ingress context installation does not match the receiving binding")]
    InstallationMismatch,
    #[error("ingress context contains an invalid required field")]
    InvalidField,
}

impl IngressContextError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidEncoding => "invalid_encoding",
            Self::InvalidSignature => "invalid_signature",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedEpoch => "unsupported_epoch",
            Self::Expired { .. } => "expired",
            Self::FutureTimestamp { .. } => "future_timestamp",
            Self::RouterMismatch => "router_mismatch",
            Self::InstallationMismatch => "installation_mismatch",
            Self::InvalidField => "invalid_field",
        }
    }

    pub fn timing(&self) -> Option<(i64, i64)> {
        match self {
            Self::Expired {
                issued_at_ms,
                now_ms,
            }
            | Self::FutureTimestamp {
                issued_at_ms,
                now_ms,
            } => Some((*issued_at_ms, *now_ms)),
            _ => None,
        }
    }
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
        return Err(IngressContextError::FutureTimestamp {
            issued_at_ms: context.issued_at_ms,
            now_ms,
        });
    }
    if now_ms.saturating_sub(context.issued_at_ms) > DEFAULT_MAX_CONTEXT_AGE_MS {
        return Err(IngressContextError::Expired {
            issued_at_ms: context.issued_at_ms,
            now_ms,
        });
    }
    if context.route_id.starts_with("share:") != context.share_id.is_some() {
        return Err(IngressContextError::InvalidField);
    }
    match context.share_id.as_deref() {
        Some(share_id)
            if share_id.trim().is_empty() || context.route_id != format!("share:{share_id}") =>
        {
            return Err(IngressContextError::InvalidField);
        }
        None if context.route_id != format!("client:{}", context.installation_id) => {
            return Err(IngressContextError::InvalidField);
        }
        _ => {}
    }
    if context.user_email.as_deref().is_some_and(|value| {
        value.is_empty()
            || value != value.trim()
            || value != value.to_ascii_lowercase()
            || !value.contains('@')
    }) {
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

    fn valid_context() -> IngressContext {
        IngressContext {
            protocol_epoch: PROTOCOL_EPOCH.to_string(),
            router_id: "router-jp".to_string(),
            route_id: "share:share-1".to_string(),
            installation_id: "installation-1".to_string(),
            target_lane_id: "installation-1:namespace-data".to_string(),
            public_host: "share-1.router.test".to_string(),
            share_id: Some("share-1".to_string()),
            request_id: "req-1".to_string(),
            user_email: Some("owner@example.com".to_string()),
            user_role: None,
            user_country: None,
            issued_at_ms: 1_750_000_000_000,
        }
    }

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
            Err(IngressContextError::Expired { .. })
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

    #[test]
    fn timestamp_boundaries_are_inclusive() {
        let context = valid_context();
        let issued_at_ms = context.issued_at_ms;

        assert!(validate(
            &context,
            "router-jp",
            "installation-1",
            issued_at_ms + DEFAULT_MAX_CONTEXT_AGE_MS,
        )
        .is_ok());
        assert!(matches!(
            validate(
                &context,
                "router-jp",
                "installation-1",
                issued_at_ms + DEFAULT_MAX_CONTEXT_AGE_MS + 1,
            ),
            Err(IngressContextError::Expired { .. })
        ));

        let now_ms = issued_at_ms;
        let mut future = context;
        future.issued_at_ms = now_ms + DEFAULT_FUTURE_CLOCK_SKEW_MS;
        assert!(validate(&future, "router-jp", "installation-1", now_ms).is_ok());
        future.issued_at_ms += 1;
        assert!(matches!(
            validate(&future, "router-jp", "installation-1", now_ms),
            Err(IngressContextError::FutureTimestamp { .. })
        ));
    }

    #[test]
    fn rejects_route_identity_mismatches() {
        let mut context = valid_context();
        context.route_id = "share:different-share".to_string();
        assert!(matches!(
            validate(&context, "router-jp", "installation-1", 1_750_000_001_000,),
            Err(IngressContextError::InvalidField)
        ));

        context.share_id = None;
        context.route_id = "client:different-installation".to_string();
        assert!(matches!(
            validate(&context, "router-jp", "installation-1", 1_750_000_001_000,),
            Err(IngressContextError::InvalidField)
        ));
    }

    #[test]
    fn rejects_noncanonical_or_malformed_signed_emails() {
        for email in [
            "Owner@example.com",
            " owner@example.com",
            "owner.example.com",
            "",
        ] {
            let mut context = valid_context();
            context.user_email = Some(email.to_string());
            assert!(matches!(
                validate(&context, "router-jp", "installation-1", 1_750_000_001_000,),
                Err(IngressContextError::InvalidField)
            ));
        }
    }
}
