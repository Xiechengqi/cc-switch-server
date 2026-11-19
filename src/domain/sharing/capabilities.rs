//! Independent Share execution permissions. Missing policy is legacy mode;
//! an explicit empty set denies all extended operations, regardless of quota unit.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const CAPABILITY_CATALOG_VERSION: &str = "share-operations-v1";
pub const CAPABILITY_CONTRACT_VERSION: u16 = 13;
pub const GRANT_CAPABILITY_CONTRACT_VERSION: u16 = 14;

/// Server-owned approvals keyed by the normalized identity of an active grant.
/// Kept outside the quota policy so scheduled quota activation cannot copy
/// older execution permissions back into effect.
pub type GrantCapabilities = BTreeMap<String, ShareCapabilityPolicy>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantCapabilityEdit {
    pub operations: Vec<ShareOperation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconfirm: Vec<ShareOperation>,
    pub expected_revision: u64,
}

impl GrantCapabilityEdit {
    pub fn validate(&self) -> Result<(), String> {
        validate_operations(&self.operations)?;
        validate_operations(&self.reconfirm)?;
        if self
            .reconfirm
            .iter()
            .any(|op| !self.operations.contains(op))
        {
            return Err("reconfirmation requires an enabled operation".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ShareOperation {
    #[serde(rename = "image.generate")]
    ImageGenerate,
    #[serde(rename = "image.edit")]
    ImageEdit,
    #[serde(rename = "video.generate")]
    VideoGenerate,
    #[serde(rename = "audio.generate")]
    AudioGenerate,
    #[serde(rename = "tool.web_search")]
    WebSearch,
    #[serde(rename = "tool.web_fetch")]
    WebFetch,
    #[serde(rename = "tool.file_search")]
    FileSearch,
    #[serde(rename = "tool.code_execution")]
    CodeExecution,
    #[serde(rename = "tool.computer")]
    Computer,
    #[serde(rename = "tool.remote_mcp")]
    RemoteMcp,
}
impl ShareOperation {
    pub const ALL: [Self; 10] = [
        Self::ImageGenerate,
        Self::ImageEdit,
        Self::VideoGenerate,
        Self::AudioGenerate,
        Self::WebSearch,
        Self::WebFetch,
        Self::FileSearch,
        Self::CodeExecution,
        Self::Computer,
        Self::RemoteMcp,
    ];
    pub fn resource(self) -> &'static str {
        match self {
            Self::ImageGenerate | Self::ImageEdit => "image.output",
            Self::VideoGenerate => "video.output_millisecond",
            Self::AudioGenerate => "audio.output_token",
            Self::WebSearch => "tool.web_search",
            Self::WebFetch => "tool.web_fetch",
            Self::FileSearch => "tool.file_search",
            Self::CodeExecution => "tool.code_execution",
            Self::Computer => "tool.computer",
            Self::RemoteMcp => "tool.mcp",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OperationBinding {
    pub app: String,
    /// Opaque identity: provider, Driver, auth identity generation and policy.
    pub fingerprint: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoverageAcknowledgement {
    /// Server-derived scope/unit/operation/runtime/gap identity.
    pub condition: String,
    pub scope: String,
    pub operation: ShareOperation,
    pub quota_unit: String,
    pub binding_fingerprint: String,
    pub actor: String,
    pub accepted_at_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareCapabilityPolicy {
    pub version: u16,
    pub allowed_operations: Vec<ShareOperation>,
    #[serde(default)]
    pub operation_bindings: BTreeMap<ShareOperation, Vec<OperationBinding>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coverage_acknowledgements: Vec<CoverageAcknowledgement>,
}
impl Default for ShareCapabilityPolicy {
    fn default() -> Self {
        Self {
            version: 1,
            allowed_operations: vec![],
            operation_bindings: BTreeMap::new(),
            coverage_acknowledgements: vec![],
        }
    }
}
impl ShareCapabilityPolicy {
    pub fn validate(&self) -> Result<(), String> {
        validate_operations(&self.allowed_operations)?;
        if self.version != 1
            || self.operation_bindings.len() > 10
            || self.coverage_acknowledgements.len() > 16_384
        {
            return Err("invalid capability policy version or size".into());
        }
        for (operation, bindings) in &self.operation_bindings {
            if !self.allowed_operations.contains(operation) || bindings.len() > 3 {
                return Err("invalid capability operation bindings".into());
            }
            let mut seen = BTreeSet::new();
            for binding in bindings {
                if !["claude", "codex", "gemini"].contains(&binding.app.as_str())
                    || binding.fingerprint.len() != 64
                    || !binding.fingerprint.bytes().all(|c| c.is_ascii_hexdigit())
                    || !seen.insert(&binding.app)
                {
                    return Err("invalid capability binding identity".into());
                }
            }
        }
        let mut acknowledged = BTreeSet::new();
        for acknowledgement in &self.coverage_acknowledgements {
            let hex_id = |id: &str| id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit());
            if !hex_id(&acknowledgement.condition)
                || !hex_id(&acknowledgement.binding_fingerprint)
                || !self.allowed_operations.contains(&acknowledgement.operation)
                || !["tokens", "usdEquivalent"].contains(&acknowledgement.quota_unit.as_str())
                || acknowledgement.scope.is_empty()
                || acknowledgement.scope.len() > 254
                || acknowledgement.actor.len() > 254
                || acknowledgement.actor.chars().any(char::is_control)
                || acknowledgement.accepted_at_ms < 0
                || !acknowledged.insert(&acknowledgement.condition)
            {
                return Err("invalid capability coverage acknowledgement".into());
            }
        }
        Ok(())
    }
}
pub fn validate_operations(operations: &[ShareOperation]) -> Result<(), String> {
    if operations.len() > 10 || operations.iter().collect::<BTreeSet<_>>().len() != operations.len()
    {
        return Err("duplicate or too many capability operations".into());
    }
    Ok(())
}

/// Financial terms are unchanged by an execution permission edit.
pub fn same_quota_terms(
    a: &super::router_contract::ShareUserPolicy,
    b: &super::router_contract::ShareUserPolicy,
) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    a.allowed_operations = None;
    b.allowed_operations = None;
    if b.value_quota.as_ref().is_some_and(|q| {
        q.tariff_policy_id == crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY
    }) {
        if let Some(q) = a.value_quota.as_mut() {
            q.tariff_policy_id = crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY.into();
        }
    }
    a == b
}

/// Recognize an independently reviewed capability edit to a legacy schedule.
/// Neither arbitrary future terms nor a new activation date are accepted here.
pub fn scheduled_capability_policy(
    grant: &super::router_contract::ShareUserGrant,
    requested: &super::router_contract::ShareUserPolicy,
) -> Option<super::router_contract::ShareUserPolicy> {
    let pending = grant.pending_value_quota.as_ref()?;
    let mut scheduled = pending.policy.clone();
    scheduled.allowed_operations = requested.allowed_operations.clone();
    let mut current = grant.policy.clone();
    current.allowed_operations = requested.allowed_operations.clone();
    if requested.value_quota.as_ref().is_some_and(|q| {
        q.tariff_policy_id == crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY
    }) {
        for policy in [&mut scheduled, &mut current] {
            if let Some(quota) = policy.value_quota.as_mut() {
                quota.tariff_policy_id =
                    crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY.into();
            }
        }
    }
    (requested == &scheduled || requested == &current).then_some(scheduled)
}

pub fn capability_only_change(
    original: &super::router_contract::ShareUserPolicy,
    requested: &super::router_contract::ShareUserPolicy,
) -> bool {
    same_quota_terms(original, requested)
        && (original.allowed_operations != requested.allowed_operations
            || (requested.value_quota.as_ref().is_some_and(|q| {
                q.tariff_policy_id == crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY
            }) && original.value_quota.as_ref().is_some_and(|q| {
                q.tariff_policy_id != crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY
            })))
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityConsent {
    pub credential: String,
    #[serde(default)]
    pub accept_uncovered: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct CapabilityRejection {
    pub status: u16,
    pub code: &'static str,
}
impl CapabilityRejection {
    pub fn from_message(message: &str) -> Option<Self> {
        let code = match message {
            "SHARE_CAPABILITY_DISABLED" => "SHARE_CAPABILITY_DISABLED",
            "CAPABILITY_NOT_SUPPORTED" => "CAPABILITY_NOT_SUPPORTED",
            "CAPABILITY_CONTEXT_REQUIRED" => "CAPABILITY_CONTEXT_REQUIRED",
            "CAPABILITY_REQUEST_INVALID" => "CAPABILITY_REQUEST_INVALID",
            "CAPABILITY_EXECUTION_QUARANTINED" => "CAPABILITY_EXECUTION_QUARANTINED",
            "CAPABILITY_STATE_CHANGED" => "CAPABILITY_STATE_CHANGED",
            "CAPABILITY_UPGRADE_REQUIRED" => "CAPABILITY_UPGRADE_REQUIRED",
            "QUOTA_COVERAGE_REQUIRED" => "QUOTA_COVERAGE_REQUIRED",
            "CAPABILITY_PREVIEW_EXPIRED" => "CAPABILITY_PREVIEW_EXPIRED",
            "MARKET_CAPABILITY_CONFLICT" => "MARKET_CAPABILITY_CONFLICT",
            _ => return None,
        };
        Some(Self {
            status: if code == "CAPABILITY_REQUEST_INVALID" {
                400
            } else if code == "CAPABILITY_EXECUTION_QUARANTINED" {
                503
            } else if code == "SHARE_CAPABILITY_DISABLED" {
                403
            } else if code == "CAPABILITY_NOT_SUPPORTED" {
                422
            } else {
                409
            },
            code,
        })
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn capability_policy_shared_fixture_preserves_empty_set_and_rejects_unknown_authority() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/share-capability-policy-v1.json"
        ))
        .unwrap();
        let empty: ShareCapabilityPolicy =
            serde_json::from_value(fixture["emptyPolicy"].clone()).unwrap();
        empty.validate().unwrap();
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            fixture["emptyPolicy"]
        );
        assert_eq!(
            serde_json::to_value(ShareOperation::ALL).unwrap(),
            fixture["operations"]
        );
        assert!(serde_json::from_value::<ShareCapabilityPolicy>(
            json!({"version":1,"allowedOperations":["future.operation"],"operationBindings":{}})
        )
        .is_err());
        assert!(serde_json::from_value::<ShareCapabilityPolicy>(json!({"version":1,"allowedOperations":[],"operationBindings":{},"permissionBypass":true})).is_err());
        let duplicate:ShareCapabilityPolicy=serde_json::from_value(json!({"version":1,"allowedOperations":["image.generate","image.generate"],"operationBindings":{}})).unwrap();
        assert!(duplicate.validate().is_err());
    }
}
