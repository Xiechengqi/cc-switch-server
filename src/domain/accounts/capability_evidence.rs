use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::domain::accounts::store::Account;
use crate::domain::providers::model::ProviderType;

pub const GEMINI_CODE_PLAN_CAPABILITY: &str = "gemini_code_plan";
pub const GROK_CODE_PLAN_CAPABILITY: &str = "grok_code_plan";
pub const ANTIGRAVITY_CODE_PLAN_CAPABILITY: &str = "antigravity_code_plan";
pub const GITHUB_COPILOT_CODE_PLAN_CAPABILITY: &str = "github_copilot_code_plan";
pub const KIMI_CODE_PLAN_CAPABILITY: &str = "kimi_code_plan";
pub const CREDENTIAL_FLOW_DIMENSION: &str = "credential_flow";
pub const TOKEN_EXCHANGE_DIMENSION: &str = "token_exchange";
pub const ENDPOINT_PROVENANCE_DIMENSION: &str = "endpoint_provenance";
pub const MODEL_CATALOG_DIMENSION: &str = "model_catalog";
pub const PREMIUM_INTERACTIONS_DIMENSION: &str = "premium_interactions";
pub const PROJECT_PROVISIONING_DIMENSION: &str = "project_provisioning";
pub const MODEL_ENTITLEMENT_DIMENSION: &str = "model_entitlement";
pub const PROJECT_BOOTSTRAP_DIMENSION: &str = "project_bootstrap";
pub const PRIVACY_DIMENSION: &str = "privacy";
pub const TIER_ENTITLEMENT_DIMENSION: &str = "tier_entitlement";
pub const GEMINI_QUOTA_FAMILY_DIMENSION: &str = "gemini_quota_family";
pub const CLAUDE_QUOTA_FAMILY_DIMENSION: &str = "claude_quota_family";
pub const GPT_QUOTA_FAMILY_DIMENSION: &str = "gpt_quota_family";
pub const MODEL_CAPACITY_DIMENSION: &str = "model_capacity";
pub const WEBSOCKET_DIMENSION: &str = "websocket";
pub const IMAGE_GENERATION_DIMENSION: &str = "image_generation";
pub const IMAGE_EDIT_DIMENSION: &str = "image_edit";
pub const VIDEO_GENERATION_DIMENSION: &str = "video_generation";
pub const SEARCH_DIMENSION: &str = "search";
pub const MEDIA_ENTITLEMENT_DIMENSION: &str = "media_entitlement";
pub const GEMINI_PROJECT_EVIDENCE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const GROK_CAPABILITY_EVIDENCE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const ANTIGRAVITY_CAPABILITY_EVIDENCE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const COPILOT_CAPABILITY_EVIDENCE_TTL_MS: i64 = 24 * 60 * 60 * 1000;
pub const KIMI_CAPABILITY_EVIDENCE_TTL_MS: i64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountCapabilityObservationState {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCapabilityObservation {
    pub capability: String,
    pub dimension: String,
    pub state: AccountCapabilityObservationState,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub observed_at_ms: i64,
    pub expires_at_ms: i64,
    pub auth_identity_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCapabilityObservationDraft {
    pub capability: String,
    pub dimension: String,
    pub state: AccountCapabilityObservationState,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub observed_at_ms: i64,
    pub expires_at_ms: i64,
}

impl AccountCapabilityObservationDraft {
    pub fn new(
        capability: &str,
        dimension: &str,
        state: AccountCapabilityObservationState,
        source: &str,
        reason: Option<&str>,
        observed_at_ms: i64,
        expires_at_ms: i64,
    ) -> Self {
        Self {
            capability: capability.to_string(),
            dimension: dimension.to_string(),
            state,
            source: source.to_string(),
            reason: reason.map(str::to_string),
            observed_at_ms,
            expires_at_ms: expires_at_ms.max(observed_at_ms),
        }
    }

    pub fn gemini_project(supported: bool, observed_at_ms: i64, reason: Option<&str>) -> Self {
        Self::new(
            GEMINI_CODE_PLAN_CAPABILITY,
            PROJECT_PROVISIONING_DIMENSION,
            if supported {
                AccountCapabilityObservationState::Supported
            } else {
                AccountCapabilityObservationState::Unknown
            },
            "load_code_assist",
            reason,
            observed_at_ms,
            observed_at_ms.saturating_add(GEMINI_PROJECT_EVIDENCE_TTL_MS),
        )
    }

    pub fn gemini_model_entitlement(
        supported: bool,
        observed_at_ms: i64,
        expires_at_ms: i64,
    ) -> Self {
        Self::new(
            GEMINI_CODE_PLAN_CAPABILITY,
            MODEL_ENTITLEMENT_DIMENSION,
            if supported {
                AccountCapabilityObservationState::Supported
            } else {
                AccountCapabilityObservationState::Unknown
            },
            "retrieve_user_quota",
            (!supported).then_some("quota_has_no_model_buckets"),
            observed_at_ms,
            expires_at_ms,
        )
    }

    pub fn grok_feature(
        dimension: &str,
        state: AccountCapabilityObservationState,
        source: &str,
        reason: Option<&str>,
        observed_at_ms: i64,
    ) -> Self {
        Self::new(
            GROK_CODE_PLAN_CAPABILITY,
            dimension,
            state,
            source,
            reason,
            observed_at_ms,
            observed_at_ms.saturating_add(GROK_CAPABILITY_EVIDENCE_TTL_MS),
        )
    }

    pub fn antigravity_feature(
        dimension: &str,
        state: AccountCapabilityObservationState,
        source: &str,
        reason: Option<&str>,
        observed_at_ms: i64,
        expires_at_ms: Option<i64>,
    ) -> Self {
        Self::new(
            ANTIGRAVITY_CODE_PLAN_CAPABILITY,
            dimension,
            state,
            source,
            reason,
            observed_at_ms,
            expires_at_ms.unwrap_or_else(|| {
                observed_at_ms.saturating_add(ANTIGRAVITY_CAPABILITY_EVIDENCE_TTL_MS)
            }),
        )
    }

    pub fn copilot_feature(
        dimension: &str,
        state: AccountCapabilityObservationState,
        source: &str,
        reason: Option<&str>,
        observed_at_ms: i64,
    ) -> Self {
        Self::new(
            GITHUB_COPILOT_CODE_PLAN_CAPABILITY,
            dimension,
            state,
            source,
            reason,
            observed_at_ms,
            observed_at_ms.saturating_add(COPILOT_CAPABILITY_EVIDENCE_TTL_MS),
        )
    }

    pub fn kimi_feature(
        dimension: &str,
        state: AccountCapabilityObservationState,
        source: &str,
        reason: Option<&str>,
        observed_at_ms: i64,
    ) -> Self {
        Self::new(
            KIMI_CODE_PLAN_CAPABILITY,
            dimension,
            state,
            source,
            reason,
            observed_at_ms,
            observed_at_ms.saturating_add(KIMI_CAPABILITY_EVIDENCE_TTL_MS),
        )
    }
}

pub fn observation_key(capability: &str, dimension: &str) -> String {
    format!("{capability}:{dimension}")
}

pub fn record_observation_drafts(
    account: &mut Account,
    drafts: impl IntoIterator<Item = AccountCapabilityObservationDraft>,
) -> usize {
    let mut recorded = 0;
    for draft in drafts {
        let key = observation_key(&draft.capability, &draft.dimension);
        let observation = AccountCapabilityObservation {
            capability: draft.capability,
            dimension: draft.dimension,
            state: draft.state,
            source: draft.source,
            reason: draft.reason,
            observed_at_ms: draft.observed_at_ms,
            expires_at_ms: draft.expires_at_ms,
            auth_identity_generation: account.auth_identity_generation,
        };
        let should_replace = account
            .capability_observations
            .get(&key)
            .is_none_or(|current| {
                current.auth_identity_generation != account.auth_identity_generation
                    || current.observed_at_ms <= observation.observed_at_ms
            });
        if should_replace {
            account.capability_observations.insert(key, observation);
            recorded += 1;
        }
    }
    recorded
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountCapabilityState {
    Supported,
    Unsupported,
    Unknown,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountCapabilityFreshness {
    Fresh,
    Unknown,
    Stale,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCapabilityEvidenceProjection {
    pub state: AccountCapabilityState,
    pub freshness: AccountCapabilityFreshness,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_auth_identity_generation: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCapabilityProjection {
    pub capability: String,
    pub rail: String,
    pub state: AccountCapabilityState,
    pub auth_identity_generation: u64,
    pub dimensions: BTreeMap<String, AccountCapabilityEvidenceProjection>,
}

pub fn account_capability_projections(
    account: &Account,
    now_ms: i64,
) -> Vec<AccountCapabilityProjection> {
    if let Some((rail, needs_project, code_plan_supported)) = gemini_rail(account.provider_type) {
        let mut projections = vec![gemini_capability_projection(
            account,
            now_ms,
            rail,
            needs_project,
            code_plan_supported,
        )];
        if matches!(
            account.provider_type,
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
        ) {
            projections.push(antigravity_capability_projection(account, now_ms, rail));
        }
        return projections;
    }
    if account.provider_type == ProviderType::GrokOAuth {
        return vec![grok_capability_projection(account, now_ms)];
    }
    if account.provider_type == ProviderType::GitHubCopilot {
        return vec![copilot_capability_projection(account, now_ms)];
    }
    if account.provider_type == ProviderType::KimiCode {
        return vec![kimi_capability_projection(account, now_ms)];
    }
    Vec::new()
}

fn kimi_capability_projection(account: &Account, now_ms: i64) -> AccountCapabilityProjection {
    let mut dimensions = BTreeMap::new();
    dimensions.insert(
        CREDENTIAL_FLOW_DIMENSION.to_string(),
        credential_projection(account),
    );
    for dimension in [MODEL_CATALOG_DIMENSION, MODEL_ENTITLEMENT_DIMENSION] {
        dimensions.insert(
            dimension.to_string(),
            observation_projection(account, KIMI_CODE_PLAN_CAPABILITY, dimension, now_ms),
        );
    }
    let state = aggregate_state(dimensions.values().map(|evidence| evidence.state));
    AccountCapabilityProjection {
        capability: KIMI_CODE_PLAN_CAPABILITY.to_string(),
        rail: "kimi_code_device_oauth".to_string(),
        state,
        auth_identity_generation: account.auth_identity_generation,
        dimensions,
    }
}

fn copilot_capability_projection(account: &Account, now_ms: i64) -> AccountCapabilityProjection {
    let mut dimensions = BTreeMap::new();
    dimensions.insert(
        CREDENTIAL_FLOW_DIMENSION.to_string(),
        credential_projection(account),
    );
    for dimension in [
        TOKEN_EXCHANGE_DIMENSION,
        ENDPOINT_PROVENANCE_DIMENSION,
        MODEL_CATALOG_DIMENSION,
        MODEL_ENTITLEMENT_DIMENSION,
        PREMIUM_INTERACTIONS_DIMENSION,
    ] {
        dimensions.insert(
            dimension.to_string(),
            observation_projection(
                account,
                GITHUB_COPILOT_CODE_PLAN_CAPABILITY,
                dimension,
                now_ms,
            ),
        );
    }
    let state = aggregate_state(dimensions.values().map(|evidence| evidence.state));
    AccountCapabilityProjection {
        capability: GITHUB_COPILOT_CODE_PLAN_CAPABILITY.to_string(),
        rail: if copilot_github_domain(account).eq_ignore_ascii_case("github.com") {
            "github_com_device_oauth"
        } else {
            "ghes_device_oauth"
        }
        .to_string(),
        state,
        auth_identity_generation: account.auth_identity_generation,
        dimensions,
    }
}

fn copilot_github_domain(account: &Account) -> &str {
    account
        .raw
        .as_ref()
        .and_then(|raw| {
            raw.get("githubDomain")
                .or_else(|| raw.get("github_domain"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| {
            account.profile.as_ref().and_then(|profile| {
                profile
                    .get("githubDomain")
                    .or_else(|| profile.get("github_domain"))
                    .and_then(serde_json::Value::as_str)
            })
        })
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
        .unwrap_or("github.com")
}

fn antigravity_capability_projection(
    account: &Account,
    now_ms: i64,
    rail: &str,
) -> AccountCapabilityProjection {
    let mut dimensions = BTreeMap::new();
    dimensions.insert(
        CREDENTIAL_FLOW_DIMENSION.to_string(),
        credential_projection(account),
    );
    for dimension in [
        PROJECT_BOOTSTRAP_DIMENSION,
        PRIVACY_DIMENSION,
        TIER_ENTITLEMENT_DIMENSION,
        MODEL_CATALOG_DIMENSION,
        GEMINI_QUOTA_FAMILY_DIMENSION,
        CLAUDE_QUOTA_FAMILY_DIMENSION,
        GPT_QUOTA_FAMILY_DIMENSION,
        MODEL_CAPACITY_DIMENSION,
    ] {
        dimensions.insert(
            dimension.to_string(),
            observation_projection(account, ANTIGRAVITY_CODE_PLAN_CAPABILITY, dimension, now_ms),
        );
    }
    let state = aggregate_state(dimensions.values().map(|evidence| evidence.state));
    AccountCapabilityProjection {
        capability: ANTIGRAVITY_CODE_PLAN_CAPABILITY.to_string(),
        rail: rail.to_string(),
        state,
        auth_identity_generation: account.auth_identity_generation,
        dimensions,
    }
}

fn gemini_capability_projection(
    account: &Account,
    now_ms: i64,
    rail: &str,
    needs_project: bool,
    code_plan_supported: bool,
) -> AccountCapabilityProjection {
    let mut dimensions = BTreeMap::new();
    dimensions.insert(
        CREDENTIAL_FLOW_DIMENSION.to_string(),
        credential_projection(account),
    );
    dimensions.insert(
        PROJECT_PROVISIONING_DIMENSION.to_string(),
        if needs_project {
            observation_projection(
                account,
                GEMINI_CODE_PLAN_CAPABILITY,
                PROJECT_PROVISIONING_DIMENSION,
                now_ms,
            )
        } else {
            contract_projection(
                AccountCapabilityState::Unsupported,
                Some("credential_rail_has_no_code_assist_project"),
            )
        },
    );
    dimensions.insert(
        MODEL_ENTITLEMENT_DIMENSION.to_string(),
        if code_plan_supported {
            observation_projection(
                account,
                GEMINI_CODE_PLAN_CAPABILITY,
                MODEL_ENTITLEMENT_DIMENSION,
                now_ms,
            )
        } else {
            contract_projection(
                AccountCapabilityState::Unsupported,
                Some("credential_rail_is_not_a_code_plan"),
            )
        },
    );
    let state = aggregate_state(dimensions.values().map(|evidence| evidence.state));
    AccountCapabilityProjection {
        capability: GEMINI_CODE_PLAN_CAPABILITY.to_string(),
        rail: rail.to_string(),
        state,
        auth_identity_generation: account.auth_identity_generation,
        dimensions,
    }
}

fn grok_capability_projection(account: &Account, now_ms: i64) -> AccountCapabilityProjection {
    let mut dimensions = BTreeMap::new();
    dimensions.insert(
        CREDENTIAL_FLOW_DIMENSION.to_string(),
        credential_projection(account),
    );
    for dimension in [
        WEBSOCKET_DIMENSION,
        IMAGE_GENERATION_DIMENSION,
        IMAGE_EDIT_DIMENSION,
        VIDEO_GENERATION_DIMENSION,
        SEARCH_DIMENSION,
        MEDIA_ENTITLEMENT_DIMENSION,
    ] {
        dimensions.insert(
            dimension.to_string(),
            observation_projection_with_legacy_grok(account, dimension, now_ms),
        );
    }
    let state = aggregate_state(dimensions.values().map(|evidence| evidence.state));
    AccountCapabilityProjection {
        capability: GROK_CODE_PLAN_CAPABILITY.to_string(),
        rail: "xai_oauth_subscription".to_string(),
        state,
        auth_identity_generation: account.auth_identity_generation,
        dimensions,
    }
}

fn gemini_rail(provider_type: ProviderType) -> Option<(&'static str, bool, bool)> {
    match provider_type {
        ProviderType::GeminiCli => Some(("code_assist_oauth", true, true)),
        ProviderType::AntigravityOAuth => Some(("antigravity_oauth", true, true)),
        ProviderType::AgyOAuth => Some(("agy_oauth", true, true)),
        ProviderType::Gemini => Some(("ai_studio_api_key", false, false)),
        _ => None,
    }
}

fn contract_projection(
    state: AccountCapabilityState,
    reason: Option<&str>,
) -> AccountCapabilityEvidenceProjection {
    AccountCapabilityEvidenceProjection {
        state,
        freshness: AccountCapabilityFreshness::Fresh,
        source: "server_contract".to_string(),
        reason: reason.map(str::to_string),
        observed_at_ms: None,
        expires_at_ms: None,
        evidence_auth_identity_generation: None,
    }
}

fn credential_projection(account: &Account) -> AccountCapabilityEvidenceProjection {
    let has_credential = match account.provider_type {
        ProviderType::Gemini => account
            .api_key
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty()),
        ProviderType::GitHubCopilot => account
            .raw
            .as_ref()
            .and_then(|raw| {
                raw.get("githubToken")
                    .or_else(|| raw.get("github_token"))
                    .and_then(serde_json::Value::as_str)
            })
            .or(account.refresh_token.as_deref())
            .or(account.api_key.as_deref())
            .or_else(|| {
                account
                    .profile
                    .as_ref()
                    .and_then(|profile| profile.get("ghes"))
                    .and_then(serde_json::Value::as_bool)
                    .filter(|value| *value)
                    .and(account.access_token.as_deref())
            })
            .is_some_and(|value| !value.trim().is_empty()),
        ProviderType::GeminiCli
        | ProviderType::AntigravityOAuth
        | ProviderType::AgyOAuth
        | ProviderType::GrokOAuth
        | ProviderType::KimiCode => [
            account.access_token.as_deref(),
            account.refresh_token.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|value| !value.trim().is_empty()),
        _ => false,
    };
    let (state, freshness, reason) = if account.needs_relogin {
        (
            AccountCapabilityState::Stale,
            AccountCapabilityFreshness::Stale,
            Some("account_requires_relogin"),
        )
    } else if has_credential {
        (
            AccountCapabilityState::Supported,
            AccountCapabilityFreshness::Fresh,
            None,
        )
    } else {
        (
            AccountCapabilityState::Unknown,
            AccountCapabilityFreshness::Unknown,
            Some("credential_not_configured"),
        )
    };
    AccountCapabilityEvidenceProjection {
        state,
        freshness,
        source: "account_credential_state".to_string(),
        reason: reason.map(str::to_string),
        observed_at_ms: None,
        expires_at_ms: None,
        evidence_auth_identity_generation: Some(account.auth_identity_generation),
    }
}

fn observation_projection(
    account: &Account,
    capability: &str,
    dimension: &str,
    now_ms: i64,
) -> AccountCapabilityEvidenceProjection {
    let Some(observation) = account
        .capability_observations
        .get(&observation_key(capability, dimension))
    else {
        return AccountCapabilityEvidenceProjection {
            state: AccountCapabilityState::Unknown,
            freshness: AccountCapabilityFreshness::Unknown,
            source: "none".to_string(),
            reason: Some("no_current_identity_evidence".to_string()),
            observed_at_ms: None,
            expires_at_ms: None,
            evidence_auth_identity_generation: None,
        };
    };
    let (state, freshness) = if observation.auth_identity_generation
        != account.auth_identity_generation
    {
        (
            AccountCapabilityState::Stale,
            AccountCapabilityFreshness::Superseded,
        )
    } else if observation.expires_at_ms <= now_ms {
        (
            AccountCapabilityState::Stale,
            AccountCapabilityFreshness::Stale,
        )
    } else {
        (
            match observation.state {
                AccountCapabilityObservationState::Supported => AccountCapabilityState::Supported,
                AccountCapabilityObservationState::Unsupported => {
                    AccountCapabilityState::Unsupported
                }
                AccountCapabilityObservationState::Unknown => AccountCapabilityState::Unknown,
            },
            AccountCapabilityFreshness::Fresh,
        )
    };
    AccountCapabilityEvidenceProjection {
        state,
        freshness,
        source: observation.source.clone(),
        reason: observation.reason.clone(),
        observed_at_ms: Some(observation.observed_at_ms),
        expires_at_ms: Some(observation.expires_at_ms),
        evidence_auth_identity_generation: Some(observation.auth_identity_generation),
    }
}

fn observation_projection_with_legacy_grok(
    account: &Account,
    dimension: &str,
    now_ms: i64,
) -> AccountCapabilityEvidenceProjection {
    let projection = observation_projection(account, GROK_CODE_PLAN_CAPABILITY, dimension, now_ms);
    if projection.source != "none" {
        return projection;
    }
    let legacy_supported = account
        .profile
        .as_ref()
        .and_then(|profile| profile.get("grokCapabilities"))
        .and_then(|capabilities| capabilities.get(dimension))
        .and_then(|evidence| evidence.get("status"))
        .and_then(serde_json::Value::as_str)
        == Some("supported");
    if !legacy_supported {
        return projection;
    }
    AccountCapabilityEvidenceProjection {
        state: AccountCapabilityState::Stale,
        freshness: AccountCapabilityFreshness::Stale,
        source: "legacy_profile".to_string(),
        reason: Some("legacy_unscoped_evidence".to_string()),
        observed_at_ms: account
            .profile
            .as_ref()
            .and_then(|profile| {
                profile.pointer(&format!("/grokCapabilities/{dimension}/observedAtMs"))
            })
            .and_then(serde_json::Value::as_i64),
        expires_at_ms: None,
        evidence_auth_identity_generation: None,
    }
}

pub fn has_fresh_supported_observation(
    account: &Account,
    capability: &str,
    dimension: &str,
    now_ms: i64,
) -> bool {
    fresh_observation_state(account, capability, dimension, now_ms)
        == Some(AccountCapabilityObservationState::Supported)
}

pub fn fresh_observation_state(
    account: &Account,
    capability: &str,
    dimension: &str,
    now_ms: i64,
) -> Option<AccountCapabilityObservationState> {
    account
        .capability_observations
        .get(&observation_key(capability, dimension))
        .filter(|observation| {
            observation.auth_identity_generation == account.auth_identity_generation
                && observation.expires_at_ms > now_ms
        })
        .map(|observation| observation.state)
}

fn aggregate_state(
    states: impl IntoIterator<Item = AccountCapabilityState>,
) -> AccountCapabilityState {
    let states = states.into_iter().collect::<Vec<_>>();
    if states.contains(&AccountCapabilityState::Unsupported) {
        AccountCapabilityState::Unsupported
    } else if states.contains(&AccountCapabilityState::Stale) {
        AccountCapabilityState::Stale
    } else if states.contains(&AccountCapabilityState::Unknown) {
        AccountCapabilityState::Unknown
    } else {
        AccountCapabilityState::Supported
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::accounts::store::AccountStore;
    use serde_json::json;

    fn account(provider_type: ProviderType) -> Account {
        serde_json::from_value(json!({
            "id": "capability-account",
            "providerType": provider_type.as_str(),
            "authIdentityGeneration": 4
        }))
        .unwrap()
    }

    #[test]
    fn token_presence_does_not_claim_project_or_model_entitlement() {
        let mut account = account(ProviderType::GeminiCli);
        account.access_token = Some("secret-token".to_string());

        let projection = account_capability_projections(&account, 1_000)
            .pop()
            .unwrap();

        assert_eq!(projection.rail, "code_assist_oauth");
        assert_eq!(projection.state, AccountCapabilityState::Unknown);
        assert_eq!(
            projection.dimensions[PROJECT_PROVISIONING_DIMENSION].state,
            AccountCapabilityState::Unknown
        );
        assert_eq!(
            projection.dimensions[MODEL_ENTITLEMENT_DIMENSION].state,
            AccountCapabilityState::Unknown
        );
    }

    #[test]
    fn copilot_token_presence_does_not_claim_exchange_catalog_or_entitlement() {
        let mut account = account(ProviderType::GitHubCopilot);
        account.access_token = Some("copilot-subtoken".to_string());
        account.refresh_token = Some("github-oauth-token".to_string());
        account.raw = Some(json!({
            "githubDomain": "github.com",
            "githubToken": "github-oauth-token",
            "copilotToken": {"token": "copilot-subtoken"}
        }));

        let projection = account_capability_projections(&account, 1_000)
            .pop()
            .unwrap();
        assert_eq!(projection.rail, "github_com_device_oauth");
        assert_eq!(
            projection.dimensions[CREDENTIAL_FLOW_DIMENSION].state,
            AccountCapabilityState::Supported
        );
        for dimension in [
            TOKEN_EXCHANGE_DIMENSION,
            ENDPOINT_PROVENANCE_DIMENSION,
            MODEL_CATALOG_DIMENSION,
            MODEL_ENTITLEMENT_DIMENSION,
            PREMIUM_INTERACTIONS_DIMENSION,
        ] {
            assert_eq!(
                projection.dimensions[dimension].state,
                AccountCapabilityState::Unknown
            );
        }
        assert_eq!(projection.state, AccountCapabilityState::Unknown);
    }

    #[test]
    fn copilot_capability_evidence_is_current_generation_only() {
        let mut account = account(ProviderType::GitHubCopilot);
        account.refresh_token = Some("github-oauth-token".to_string());
        record_observation_drafts(
            &mut account,
            [
                AccountCapabilityObservationDraft::copilot_feature(
                    TOKEN_EXCHANGE_DIMENSION,
                    AccountCapabilityObservationState::Supported,
                    "copilot_runtime_token_exchange",
                    None,
                    1_000,
                ),
                AccountCapabilityObservationDraft::copilot_feature(
                    PREMIUM_INTERACTIONS_DIMENSION,
                    AccountCapabilityObservationState::Supported,
                    "copilot_internal_user",
                    None,
                    1_000,
                ),
            ],
        );

        let fresh = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();
        assert_eq!(
            fresh.dimensions[TOKEN_EXCHANGE_DIMENSION].freshness,
            AccountCapabilityFreshness::Fresh
        );
        account.auth_identity_generation += 1;
        let superseded = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();
        for dimension in [TOKEN_EXCHANGE_DIMENSION, PREMIUM_INTERACTIONS_DIMENSION] {
            assert_eq!(
                superseded.dimensions[dimension].freshness,
                AccountCapabilityFreshness::Superseded
            );
            assert_eq!(
                superseded.dimensions[dimension].evidence_auth_identity_generation,
                Some(4)
            );
        }
    }

    #[test]
    fn observations_are_fresh_only_for_the_current_identity_generation() {
        let mut account = account(ProviderType::GeminiCli);
        account.access_token = Some("configured-token".to_string());
        record_observation_drafts(
            &mut account,
            [
                AccountCapabilityObservationDraft::gemini_project(true, 1_000, None),
                AccountCapabilityObservationDraft::gemini_model_entitlement(true, 1_000, 5_000),
            ],
        );
        let fresh = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();
        assert_eq!(fresh.state, AccountCapabilityState::Supported);

        account.auth_identity_generation += 1;
        let superseded = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();
        assert_eq!(superseded.state, AccountCapabilityState::Stale);
        for dimension in [PROJECT_PROVISIONING_DIMENSION, MODEL_ENTITLEMENT_DIMENSION] {
            assert_eq!(
                superseded.dimensions[dimension].freshness,
                AccountCapabilityFreshness::Superseded
            );
            assert_eq!(
                superseded.dimensions[dimension].evidence_auth_identity_generation,
                Some(4)
            );
        }
    }

    #[test]
    fn expired_observation_is_stale_and_api_key_code_plan_is_unsupported() {
        let mut oauth = account(ProviderType::GeminiCli);
        record_observation_drafts(
            &mut oauth,
            [AccountCapabilityObservationDraft::gemini_model_entitlement(
                true, 1_000, 2_000,
            )],
        );
        let expired = account_capability_projections(&oauth, 2_000).pop().unwrap();
        assert_eq!(
            expired.dimensions[MODEL_ENTITLEMENT_DIMENSION].freshness,
            AccountCapabilityFreshness::Stale
        );

        let api_key = account_capability_projections(&account(ProviderType::Gemini), 1_000)
            .pop()
            .unwrap();
        assert_eq!(api_key.state, AccountCapabilityState::Unsupported);
        assert_eq!(api_key.rail, "ai_studio_api_key");
    }

    #[test]
    fn credential_flow_is_distinct_from_entitlement_and_relogin_is_stale() {
        let mut api_key_account = account(ProviderType::Gemini);
        api_key_account.api_key = Some("configured-key".to_string());
        let api_key = account_capability_projections(&api_key_account, 1_000)
            .pop()
            .unwrap();
        assert_eq!(
            api_key.dimensions[CREDENTIAL_FLOW_DIMENSION].state,
            AccountCapabilityState::Supported
        );
        assert_eq!(api_key.state, AccountCapabilityState::Unsupported);

        let mut oauth = account(ProviderType::GeminiCli);
        oauth.access_token = Some("configured-token".to_string());
        oauth.needs_relogin = true;
        let relogin = account_capability_projections(&oauth, 1_000).pop().unwrap();
        assert_eq!(
            relogin.dimensions[CREDENTIAL_FLOW_DIMENSION].state,
            AccountCapabilityState::Stale
        );
        assert_eq!(
            relogin.dimensions[CREDENTIAL_FLOW_DIMENSION]
                .reason
                .as_deref(),
            Some("account_requires_relogin")
        );
    }

    #[test]
    fn observation_store_round_trips_without_secrets() {
        let mut account = account(ProviderType::GeminiCli);
        record_observation_drafts(
            &mut account,
            [AccountCapabilityObservationDraft::gemini_project(
                true, 1_000, None,
            )],
        );
        let encoded = serde_json::to_value(AccountStore {
            accounts: vec![account],
            ..AccountStore::default()
        })
        .unwrap();
        let decoded: AccountStore = serde_json::from_value(encoded).unwrap();
        let observation = decoded.accounts[0]
            .capability_observations
            .values()
            .next()
            .unwrap();
        assert_eq!(observation.source, "load_code_assist");
        assert_eq!(observation.auth_identity_generation, 4);
    }

    #[test]
    fn antigravity_and_agy_publish_distinct_generic_and_plan_projections() {
        for (provider_type, expected_rail) in [
            (ProviderType::AntigravityOAuth, "antigravity_oauth"),
            (ProviderType::AgyOAuth, "agy_oauth"),
        ] {
            let mut account = account(provider_type);
            account.access_token = Some("configured-token".to_string());
            record_observation_drafts(
                &mut account,
                [
                    AccountCapabilityObservationDraft::gemini_project(true, 1_000, None),
                    AccountCapabilityObservationDraft::gemini_model_entitlement(true, 1_000, 5_000),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        PROJECT_BOOTSTRAP_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "load_code_assist",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        PRIVACY_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "fetch_user_info_read_only",
                        Some("telemetry_setting_absent"),
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        TIER_ENTITLEMENT_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "load_code_assist",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        MODEL_CATALOG_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "fetch_available_models",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        GEMINI_QUOTA_FAMILY_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "retrieve_user_quota",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        CLAUDE_QUOTA_FAMILY_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "retrieve_user_quota",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        GPT_QUOTA_FAMILY_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "fetch_available_models",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                    AccountCapabilityObservationDraft::antigravity_feature(
                        MODEL_CAPACITY_DIMENSION,
                        AccountCapabilityObservationState::Supported,
                        "retrieve_user_quota",
                        None,
                        1_000,
                        Some(5_000),
                    ),
                ],
            );

            let projections = account_capability_projections(&account, 2_000);
            assert_eq!(projections.len(), 2);
            assert_eq!(projections[0].capability, GEMINI_CODE_PLAN_CAPABILITY);
            assert_eq!(projections[1].capability, ANTIGRAVITY_CODE_PLAN_CAPABILITY);
            assert!(projections
                .iter()
                .all(|projection| projection.rail == expected_rail));
            assert!(projections
                .iter()
                .all(|projection| projection.state == AccountCapabilityState::Supported));
        }
    }

    #[test]
    fn antigravity_projection_fences_generation_expiry_and_missing_family_evidence() {
        let mut account = account(ProviderType::AntigravityOAuth);
        account.access_token = Some("configured-token".to_string());
        record_observation_drafts(
            &mut account,
            [
                AccountCapabilityObservationDraft::antigravity_feature(
                    GEMINI_QUOTA_FAMILY_DIMENSION,
                    AccountCapabilityObservationState::Supported,
                    "retrieve_user_quota",
                    None,
                    1_000,
                    Some(5_000),
                ),
                AccountCapabilityObservationDraft::antigravity_feature(
                    MODEL_CAPACITY_DIMENSION,
                    AccountCapabilityObservationState::Unsupported,
                    "google_rpc_error_info",
                    Some("model_capacity_exhausted"),
                    1_000,
                    Some(2_000),
                ),
            ],
        );

        let projection = account_capability_projections(&account, 1_500)
            .into_iter()
            .find(|projection| projection.capability == ANTIGRAVITY_CODE_PLAN_CAPABILITY)
            .unwrap();
        assert_eq!(
            projection.dimensions[CLAUDE_QUOTA_FAMILY_DIMENSION].state,
            AccountCapabilityState::Unknown
        );
        assert_eq!(
            projection.dimensions[MODEL_CAPACITY_DIMENSION].state,
            AccountCapabilityState::Unsupported
        );

        let expired = account_capability_projections(&account, 2_000)
            .into_iter()
            .find(|projection| projection.capability == ANTIGRAVITY_CODE_PLAN_CAPABILITY)
            .unwrap();
        assert_eq!(
            expired.dimensions[MODEL_CAPACITY_DIMENSION].freshness,
            AccountCapabilityFreshness::Stale
        );

        account.auth_identity_generation += 1;
        let superseded = account_capability_projections(&account, 1_500)
            .into_iter()
            .find(|projection| projection.capability == ANTIGRAVITY_CODE_PLAN_CAPABILITY)
            .unwrap();
        assert_eq!(
            superseded.dimensions[GEMINI_QUOTA_FAMILY_DIMENSION].freshness,
            AccountCapabilityFreshness::Superseded
        );
        assert_eq!(
            superseded.dimensions[GEMINI_QUOTA_FAMILY_DIMENSION].evidence_auth_identity_generation,
            Some(4)
        );
    }

    #[test]
    fn grok_features_require_current_unexpired_observations() {
        let mut account = account(ProviderType::GrokOAuth);
        account.access_token = Some("grok-access".to_string());
        record_observation_drafts(
            &mut account,
            [AccountCapabilityObservationDraft::grok_feature(
                WEBSOCKET_DIMENSION,
                AccountCapabilityObservationState::Supported,
                "upstream_success",
                None,
                1_000,
            )],
        );

        let fresh = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();
        assert_eq!(fresh.rail, "xai_oauth_subscription");
        assert_eq!(
            fresh.dimensions[WEBSOCKET_DIMENSION].state,
            AccountCapabilityState::Supported
        );
        assert_eq!(
            fresh.dimensions[IMAGE_GENERATION_DIMENSION].state,
            AccountCapabilityState::Unknown
        );
        assert!(has_fresh_supported_observation(
            &account,
            GROK_CODE_PLAN_CAPABILITY,
            WEBSOCKET_DIMENSION,
            2_000,
        ));

        account.auth_identity_generation += 1;
        let superseded = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();
        assert_eq!(
            superseded.dimensions[WEBSOCKET_DIMENSION].freshness,
            AccountCapabilityFreshness::Superseded
        );
        assert!(!has_fresh_supported_observation(
            &account,
            GROK_CODE_PLAN_CAPABILITY,
            WEBSOCKET_DIMENSION,
            2_000,
        ));
    }

    #[test]
    fn legacy_grok_profile_evidence_is_diagnostic_only() {
        let mut account = account(ProviderType::GrokOAuth);
        account.profile = Some(json!({
            "grokCapabilities": {
                "video_generation": {
                    "status": "supported",
                    "source": "upstream_success",
                    "observedAtMs": 1234
                }
            }
        }));

        let projection = account_capability_projections(&account, 2_000)
            .pop()
            .unwrap();

        assert_eq!(
            projection.dimensions[VIDEO_GENERATION_DIMENSION].state,
            AccountCapabilityState::Stale
        );
        assert_eq!(
            projection.dimensions[VIDEO_GENERATION_DIMENSION]
                .reason
                .as_deref(),
            Some("legacy_unscoped_evidence")
        );
        assert!(!has_fresh_supported_observation(
            &account,
            GROK_CODE_PLAN_CAPABILITY,
            VIDEO_GENERATION_DIMENSION,
            2_000,
        ));
    }

    #[test]
    fn newer_explicit_negative_grok_evidence_replaces_supported_state() {
        let mut account = account(ProviderType::GrokOAuth);
        record_observation_drafts(
            &mut account,
            [AccountCapabilityObservationDraft::grok_feature(
                SEARCH_DIMENSION,
                AccountCapabilityObservationState::Supported,
                "upstream_success",
                None,
                1_000,
            )],
        );
        record_observation_drafts(
            &mut account,
            [AccountCapabilityObservationDraft::grok_feature(
                SEARCH_DIMENSION,
                AccountCapabilityObservationState::Unsupported,
                "upstream_rejected",
                Some("search_not_entitled"),
                2_000,
            )],
        );

        let projection = account_capability_projections(&account, 3_000)
            .pop()
            .unwrap();
        assert_eq!(
            projection.dimensions[SEARCH_DIMENSION].state,
            AccountCapabilityState::Unsupported
        );
        assert_eq!(
            projection.dimensions[SEARCH_DIMENSION].reason.as_deref(),
            Some("search_not_entitled")
        );
    }
}
