use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::capability_evidence::{
    observation_key, AccountCapabilityObservationState, ANTIGRAVITY_CODE_PLAN_CAPABILITY,
};
use super::store::Account;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AntigravityService {
    Inference,
    CountTokens,
}

impl AntigravityService {
    pub fn dimension(self) -> &'static str {
        match self {
            Self::Inference => "inference_access",
            Self::CountTokens => "count_tokens_access",
        }
    }
}

/// Require the complete Google RPC signature, never a free-text substring.
pub fn is_tos_violation(value: &Value) -> bool {
    [value.get("error"), value.pointer("/response/error")]
        .into_iter()
        .flatten()
        .any(is_tos_error)
}

fn is_tos_error(error: &Value) -> bool {
    error.get("code").and_then(Value::as_u64) == Some(403)
        && error.get("status").and_then(Value::as_str) == Some("PERMISSION_DENIED")
        && error
            .get("details")
            .and_then(Value::as_array)
            .is_some_and(|details| {
                details.iter().any(|detail| {
                    detail.get("@type").and_then(Value::as_str)
                        == Some("type.googleapis.com/google.rpc.ErrorInfo")
                        && detail.get("reason").and_then(Value::as_str) == Some("TOS_VIOLATION")
                })
            })
}

pub fn service_blocked(account: &Account, service: AntigravityService) -> bool {
    account
        .capability_observations
        .get(&observation_key(
            ANTIGRAVITY_CODE_PLAN_CAPABILITY,
            service.dimension(),
        ))
        .is_some_and(|observation| {
            observation.auth_identity_generation == account.auth_identity_generation
                && observation.state == AccountCapabilityObservationState::Unsupported
                && observation.reason.as_deref() == Some("tos_violation")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn antigravity_tos_requires_exact_structured_signature() {
        let body = json!({"error":{"code":403,"status":"PERMISSION_DENIED","details":[{
            "@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"TOS_VIOLATION",
            "metadata":{"appeal_url":"https://example.test/?private=never-store"}}]}});
        assert!(is_tos_violation(&body));
        assert!(is_tos_violation(&json!({"response":body.clone()})));
        assert!(is_tos_violation(
            &json!({"error":null,"response":body.clone()})
        ));
        for pointer in [
            "/error/code",
            "/error/status",
            "/error/details/0/@type",
            "/error/details/0/reason",
        ] {
            let mut invalid = body.clone();
            *invalid.pointer_mut(pointer).unwrap() = Value::Null;
            assert!(!is_tos_violation(&invalid));
        }
        assert!(!is_tos_violation(
            &json!({"error":{"code":403,"message":"TOS_VIOLATION"}})
        ));
    }
}
