use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};

pub const PROTOCOL_EPOCH: &str = "namespace-flat-1";
pub const INGRESS_CONTEXT_HEADER: &str = "x-cc-switch-ingress-context";
pub const INGRESS_SIGNATURE_HEADER: &str = "x-cc-switch-ingress-signature";
pub const INTERNAL_INGRESS_ERROR_HEADER: &str = "x-cc-switch-internal-ingress-error";
pub const INTERNAL_INGRESS_AGE_MS_HEADER: &str = "x-cc-switch-internal-ingress-age-ms";
pub const INTERNAL_INGRESS_SERVER_TIME_MS_HEADER: &str =
    "x-cc-switch-internal-ingress-server-time-ms";
pub const DEFAULT_MAX_CONTEXT_AGE_MS: i64 = 30_000;
pub const DEFAULT_FUTURE_CLOCK_SKEW_MS: i64 = 5_000;
pub const SIGNATURE_VERSION_V1: u8 = 1;
pub const SIGNATURE_VERSION_V2: u8 = 2;
pub const CURRENT_SIGNATURE_VERSION: u8 = SIGNATURE_VERSION_V2;
pub const V1_COMPAT_UNTIL_MS: i64 = 1_788_825_600_000;
const V1_SIGNING_DOMAIN: &str = "cc-switch-router-ingress-v1";
const V2_SIGNING_DOMAIN: &str = "cc-switch-router-ingress-v2";
const SHA256_HEX_LENGTH: usize = 64;
const MAX_REQUEST_ID_BYTES: usize = 128;
const MAX_PATH_AND_QUERY_BYTES: usize = 16 * 1024;
const MAX_REPLAY_ENTRIES: usize = 16_384;

fn default_signature_version() -> u8 {
    SIGNATURE_VERSION_V1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IngressContext {
    #[serde(default = "default_signature_version")]
    pub signature_version: u8,
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path_and_query: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body_sha256: String,
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
    #[error("ingress context signature version is unsupported")]
    UnsupportedSignatureVersion,
    #[error("ingress v1 compatibility window has ended")]
    V1CompatibilityEnded,
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
    #[error("ingress context method does not match the request")]
    MethodMismatch,
    #[error("ingress context path and query do not match the request")]
    PathMismatch,
    #[error("ingress context body digest does not match the request")]
    BodyDigestMismatch,
}

impl IngressContextError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidEncoding => "invalid_encoding",
            Self::InvalidSignature => "invalid_signature",
            Self::InvalidJson(_) => "invalid_json",
            Self::UnsupportedEpoch => "unsupported_epoch",
            Self::UnsupportedSignatureVersion => "unsupported_signature_version",
            Self::V1CompatibilityEnded => "v1_compatibility_ended",
            Self::Expired { .. } => "expired",
            Self::FutureTimestamp { .. } => "future_timestamp",
            Self::RouterMismatch => "router_mismatch",
            Self::InstallationMismatch => "installation_mismatch",
            Self::InvalidField => "invalid_field",
            Self::MethodMismatch => "method_mismatch",
            Self::PathMismatch => "path_mismatch",
            Self::BodyDigestMismatch => "body_digest_mismatch",
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

pub fn verify_envelope(
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
    let json = URL_SAFE_NO_PAD
        .decode(encoded_context)
        .map_err(|_| IngressContextError::InvalidEncoding)?;
    let context = serde_json::from_slice::<IngressContext>(&json)
        .map_err(IngressContextError::InvalidJson)?;
    let signing_domain = match context.signature_version {
        SIGNATURE_VERSION_V1 if now_ms <= V1_COMPAT_UNTIL_MS => V1_SIGNING_DOMAIN,
        SIGNATURE_VERSION_V1 => return Err(IngressContextError::V1CompatibilityEnded),
        SIGNATURE_VERSION_V2 => V2_SIGNING_DOMAIN,
        _ => return Err(IngressContextError::UnsupportedSignatureVersion),
    };
    let mut mac = Hmac::<Sha256>::new_from_slice(control_secret.as_bytes())
        .map_err(|_| IngressContextError::InvalidSignature)?;
    mac.update(signing_domain.as_bytes());
    mac.update(b"\n");
    mac.update(PROTOCOL_EPOCH.as_bytes());
    mac.update(b"\n");
    mac.update(encoded_context.as_bytes());
    mac.verify_slice(&signature)
        .map_err(|_| IngressContextError::InvalidSignature)?;

    validate(
        &context,
        expected_router_id,
        expected_installation_id,
        now_ms,
    )?;
    Ok(context)
}

pub fn verify_request_binding(
    context: &IngressContext,
    method: &str,
    path_and_query: &str,
    body_sha256: &str,
) -> Result<(), IngressContextError> {
    if context.signature_version == SIGNATURE_VERSION_V1 {
        return Ok(());
    }
    if context.method != normalize_method(method).ok_or(IngressContextError::InvalidField)? {
        return Err(IngressContextError::MethodMismatch);
    }
    if context.path_and_query
        != normalize_path_and_query(path_and_query).ok_or(IngressContextError::InvalidField)?
    {
        return Err(IngressContextError::PathMismatch);
    }
    if context.body_sha256 != body_sha256 {
        return Err(IngressContextError::BodyDigestMismatch);
    }
    Ok(())
}

pub fn body_sha256_hex(body: &[u8]) -> String {
    hex::encode(Sha256::digest(body))
}

pub fn normalize_method(method: &str) -> Option<String> {
    let method = method.trim();
    (!method.is_empty()
        && method.len() <= 16
        && method.bytes().all(|byte| byte.is_ascii_uppercase()))
    .then(|| method.to_string())
}

pub fn normalize_path_and_query(path_and_query: &str) -> Option<String> {
    let target = path_and_query.trim();
    (target.starts_with('/')
        && target.len() <= MAX_PATH_AND_QUERY_BYTES
        && !target.contains('#')
        && !target.bytes().any(|byte| byte.is_ascii_control()))
    .then(|| target.to_string())
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
        || context.request_id.len() > MAX_REQUEST_ID_BYTES
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
    if context.signature_version == SIGNATURE_VERSION_V2
        && (normalize_method(&context.method).as_deref() != Some(context.method.as_str())
            || normalize_path_and_query(&context.path_and_query).as_deref()
                != Some(context.path_and_query.as_str())
            || context.body_sha256.len() != SHA256_HEX_LENGTH
            || !context
                .body_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    {
        return Err(IngressContextError::InvalidField);
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct IngressReplayCache {
    entries: HashMap<String, i64>,
    order: VecDeque<(String, i64)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngressReplayDecision {
    Accepted,
    Replay,
    Capacity,
}

impl IngressReplayCache {
    pub fn accept(&mut self, context: &IngressContext, now_ms: i64) -> IngressReplayDecision {
        if context.signature_version != SIGNATURE_VERSION_V2 {
            return IngressReplayDecision::Accepted;
        }
        self.prune(now_ms);
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            context.router_id, context.installation_id, context.request_id
        );
        if self
            .entries
            .get(&key)
            .is_some_and(|expires_at| *expires_at > now_ms)
        {
            return IngressReplayDecision::Replay;
        }
        if self.entries.len() >= MAX_REPLAY_ENTRIES {
            return IngressReplayDecision::Capacity;
        }
        let expires_at = context
            .issued_at_ms
            .saturating_add(DEFAULT_MAX_CONTEXT_AGE_MS)
            .saturating_add(DEFAULT_FUTURE_CLOCK_SKEW_MS);
        self.entries.insert(key.clone(), expires_at);
        self.order.push_back((key, expires_at));
        IngressReplayDecision::Accepted
    }

    fn prune(&mut self, now_ms: i64) {
        while let Some((key, expiry)) = self.order.front() {
            if *expiry > now_ms {
                break;
            }
            let key = key.clone();
            let expiry = *expiry;
            self.order.pop_front();
            if self.entries.get(&key) == Some(&expiry) {
                self.entries.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGH";
    const V1_ENCODED: &str = "eyJwcm90b2NvbEVwb2NoIjoibmFtZXNwYWNlLWZsYXQtMSIsInJvdXRlcklkIjoicm91dGVyLWpwIiwicm91dGVJZCI6InNoYXJlOnNoYXJlLTEiLCJpbnN0YWxsYXRpb25JZCI6Imluc3RhbGxhdGlvbi0xIiwidGFyZ2V0TGFuZUlkIjoiaW5zdGFsbGF0aW9uLTE6bmFtZXNwYWNlLWRhdGEiLCJwdWJsaWNIb3N0IjoiY29kZXgtLWFscGhhLWlvc2c2aGlpZHV0cWNtaGNlZWZiLnJvdXRlci50ZXN0Iiwic2hhcmVJZCI6InNoYXJlLTEiLCJyZXF1ZXN0SWQiOiJyZXFfMTIzIiwidXNlckVtYWlsIjoib3duZXJAZXhhbXBsZS5jb20iLCJ1c2VyQ291bnRyeSI6IkpQIiwiaXNzdWVkQXRNcyI6MTc1MDAwMDAwMDAwMH0";
    const V1_SIGNATURE: &str = "RvdTGpCCJwSxo7Kn8meZ0Vx3MaHf3YocqnzKyqJxTeU";
    const V2_ENCODED: &str = "eyJzaWduYXR1cmVWZXJzaW9uIjoyLCJwcm90b2NvbEVwb2NoIjoibmFtZXNwYWNlLWZsYXQtMSIsInJvdXRlcklkIjoicm91dGVyLWpwIiwicm91dGVJZCI6InNoYXJlOnNoYXJlLTEiLCJpbnN0YWxsYXRpb25JZCI6Imluc3RhbGxhdGlvbi0xIiwidGFyZ2V0TGFuZUlkIjoiaW5zdGFsbGF0aW9uLTE6bmFtZXNwYWNlLWRhdGEiLCJwdWJsaWNIb3N0IjoiY29kZXgtLWFscGhhLWlvc2c2aGlpZHV0cWNtaGNlZWZiLnJvdXRlci50ZXN0Iiwic2hhcmVJZCI6InNoYXJlLTEiLCJyZXF1ZXN0SWQiOiJyZXFfMTIzIiwidXNlckVtYWlsIjoib3duZXJAZXhhbXBsZS5jb20iLCJ1c2VyQ291bnRyeSI6IkpQIiwibWV0aG9kIjoiUE9TVCIsInBhdGhBbmRRdWVyeSI6Ii92MS9tZXNzYWdlcz9iZXRhPXRydWUiLCJib2R5U2hhMjU2IjoiOTNlOTEzNGUxMWIxNWVkM2JjZGZlNWNiYjUxZmVhYjdmMWY3MzMwOWJmOGE4NWE5ZjM4ZjgxMzNlMmVlZTNjYyIsImlzc3VlZEF0TXMiOjE3NTAwMDAwMDAwMDB9";
    const V2_SIGNATURE: &str = "J1a63NviixVTTd2fuMrF3P696OeA-JP_abaLzW7PVEg";

    fn valid_context() -> IngressContext {
        IngressContext {
            signature_version: SIGNATURE_VERSION_V2,
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
            method: "POST".to_string(),
            path_and_query: "/v1/messages".to_string(),
            body_sha256: body_sha256_hex(b""),
            issued_at_ms: 1_750_000_000_000,
        }
    }

    #[test]
    fn verifies_the_router_test_vectors() {
        let legacy = verify_envelope(
            V1_ENCODED,
            V1_SIGNATURE,
            SECRET,
            "router-jp",
            "installation-1",
            1_750_000_001_000,
        )
        .unwrap();
        assert_eq!(legacy.signature_version, SIGNATURE_VERSION_V1);

        let current = verify_envelope(
            V2_ENCODED,
            V2_SIGNATURE,
            SECRET,
            "router-jp",
            "installation-1",
            1_750_000_001_000,
        )
        .unwrap();
        assert_eq!(current.signature_version, SIGNATURE_VERSION_V2);
        assert_eq!(current.share_id.as_deref(), Some("share-1"));
        assert_eq!(current.request_id, "req_123");
        assert_eq!(current.method, "POST");
        assert_eq!(current.path_and_query, "/v1/messages?beta=true");
    }

    #[test]
    fn rejects_tampering_cross_router_replay_and_expiry() {
        assert!(matches!(
            verify_envelope(
                V1_ENCODED,
                V1_SIGNATURE,
                SECRET,
                "router-us",
                "installation-1",
                1_750_000_001_000,
            ),
            Err(IngressContextError::RouterMismatch)
        ));
        assert!(matches!(
            verify_envelope(
                V1_ENCODED,
                V1_SIGNATURE,
                SECRET,
                "router-jp",
                "installation-1",
                1_750_000_100_000,
            ),
            Err(IngressContextError::Expired { .. })
        ));
        let mut signature = V1_SIGNATURE.to_string();
        signature.replace_range(..1, "A");
        assert!(matches!(
            verify_envelope(
                V1_ENCODED,
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

    #[test]
    fn v2_binding_covers_method_path_query_and_body() {
        let context = valid_context();
        assert!(
            verify_request_binding(&context, "POST", "/v1/messages", &body_sha256_hex(b"")).is_ok()
        );
        assert!(matches!(
            verify_request_binding(&context, "GET", "/v1/messages", &body_sha256_hex(b"")),
            Err(IngressContextError::MethodMismatch)
        ));
        assert!(matches!(
            verify_request_binding(
                &context,
                "POST",
                "/v1/messages?beta=true",
                &body_sha256_hex(b"")
            ),
            Err(IngressContextError::PathMismatch)
        ));
        assert!(matches!(
            verify_request_binding(
                &context,
                "POST",
                "/v1/messages",
                &body_sha256_hex(b"changed")
            ),
            Err(IngressContextError::BodyDigestMismatch)
        ));
    }

    #[test]
    fn v1_compatibility_deadline_is_inclusive() {
        let mut context = valid_context();
        context.signature_version = SIGNATURE_VERSION_V1;
        context.method.clear();
        context.path_and_query.clear();
        context.body_sha256.clear();
        context.issued_at_ms = V1_COMPAT_UNTIL_MS;
        let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&context).unwrap());
        let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
        mac.update(V1_SIGNING_DOMAIN.as_bytes());
        mac.update(b"\n");
        mac.update(PROTOCOL_EPOCH.as_bytes());
        mac.update(b"\n");
        mac.update(encoded.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

        assert!(verify_envelope(
            &encoded,
            &signature,
            SECRET,
            "router-jp",
            "installation-1",
            V1_COMPAT_UNTIL_MS,
        )
        .is_ok());
        assert!(matches!(
            verify_envelope(
                &encoded,
                &signature,
                SECRET,
                "router-jp",
                "installation-1",
                V1_COMPAT_UNTIL_MS + 1,
            ),
            Err(IngressContextError::V1CompatibilityEnded)
        ));
    }

    #[test]
    fn v2_request_ids_are_single_use_within_the_freshness_window() {
        let context = valid_context();
        let mut cache = IngressReplayCache::default();
        let now_ms = context.issued_at_ms + 1_000;
        assert_eq!(
            cache.accept(&context, now_ms),
            IngressReplayDecision::Accepted
        );
        assert_eq!(
            cache.accept(&context, now_ms),
            IngressReplayDecision::Replay
        );

        let mut independent = context.clone();
        independent.request_id = "req-2".to_string();
        assert_eq!(
            cache.accept(&independent, now_ms),
            IngressReplayDecision::Accepted
        );

        let mut legacy = context;
        legacy.signature_version = SIGNATURE_VERSION_V1;
        assert_eq!(
            cache.accept(&legacy, now_ms),
            IngressReplayDecision::Accepted
        );
        assert_eq!(
            cache.accept(&legacy, now_ms),
            IngressReplayDecision::Accepted
        );
    }

    #[test]
    fn full_v2_cache_rejects_new_ids_without_evicting_live_entries() {
        let mut context = valid_context();
        let mut cache = IngressReplayCache::default();
        let now_ms = context.issued_at_ms + 1_000;
        for index in 0..MAX_REPLAY_ENTRIES {
            context.request_id = format!("capacity-{index}");
            assert_eq!(
                cache.accept(&context, now_ms),
                IngressReplayDecision::Accepted
            );
        }

        context.request_id = "capacity-overflow".to_string();
        assert_eq!(
            cache.accept(&context, now_ms),
            IngressReplayDecision::Capacity
        );
        context.request_id = "capacity-0".to_string();
        assert_eq!(
            cache.accept(&context, now_ms),
            IngressReplayDecision::Replay
        );

        let expiry_ms = context
            .issued_at_ms
            .saturating_add(DEFAULT_MAX_CONTEXT_AGE_MS)
            .saturating_add(DEFAULT_FUTURE_CLOCK_SKEW_MS);
        context.issued_at_ms = expiry_ms;
        context.request_id = "capacity-after-expiry".to_string();
        assert_eq!(
            cache.accept(&context, expiry_ms),
            IngressReplayDecision::Accepted
        );
    }
}
