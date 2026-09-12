use std::fmt;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::{managed_account_binding_with_generation, RuntimeAuthRef};
use crate::domain::providers::store::StoredProvider;

use super::super::provider_ops::ProviderExecution;
pub(in crate::proxy) use super::recovery::{
    AttemptBudget, AttemptLimits, DelaySource, RecoveryStage, RetryDecision,
};
pub(in crate::proxy) use super::terminal::CommitGuard;

#[derive(Clone, PartialEq, Eq)]
pub(in crate::proxy) struct ManagedAccountSnapshot {
    provider_type: ProviderType,
    account_id: String,
    auth_identity_generation: u64,
    token_generation: u64,
}

impl fmt::Debug for ManagedAccountSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedAccountSnapshot")
            .field("provider_type", &self.provider_type)
            .field("account_id", &"<opaque>")
            .field("auth_identity_generation", &self.auth_identity_generation)
            .field("token_generation", &self.token_generation)
            .finish()
    }
}

/// Immutable identity fence captured when a request selects its Provider.
/// IDs are deliberately redacted from Debug output: the value is request-local
/// and only equality comparisons are permitted outside this module.
#[derive(Clone, PartialEq, Eq)]
pub(in crate::proxy) struct BindingSnapshot {
    app: AppKind,
    provider_id: String,
    provider_type: ProviderType,
    provider_revision: u64,
    credential_generation: u64,
    rail: String,
    account: Option<ManagedAccountSnapshot>,
}

impl fmt::Debug for BindingSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BindingSnapshot")
            .field("app", &self.app)
            .field("provider_id", &"<opaque>")
            .field("provider_type", &self.provider_type)
            .field("provider_revision", &self.provider_revision)
            .field("credential_generation", &self.credential_generation)
            .field("rail", &self.rail)
            .field("account", &self.account)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::proxy) enum BindingSnapshotError {
    AccountMissing,
    AuthIdentityChanged,
    BindingChanged,
    TokenGenerationRegressed,
}

impl BindingSnapshot {
    /// Capture the same immutable fence for a Provider-owned executor that
    /// does not enter through `ProviderExecution` (currently Cursor
    /// AgentService). The caller supplies a stable protocol rail identifier;
    /// it must include every runtime choice that can change the credential or
    /// wire endpoint used by a transparent replay.
    pub(in crate::proxy) fn capture_stored(
        stored: &StoredProvider,
        accounts: &AccountStore,
        rail: impl Into<String>,
    ) -> Result<Self, BindingSnapshotError> {
        let account = match managed_account_binding_with_generation(stored) {
            Some((provider_type, account_id, auth_identity_generation)) => {
                let current = accounts
                    .find_for_provider(provider_type, Some(account_id))
                    .ok_or(BindingSnapshotError::AccountMissing)?;
                if current.auth_identity_generation != auth_identity_generation {
                    return Err(BindingSnapshotError::AuthIdentityChanged);
                }
                Some(ManagedAccountSnapshot {
                    provider_type,
                    account_id: account_id.to_string(),
                    auth_identity_generation,
                    token_generation: current.token_refresh_generation,
                })
            }
            None => None,
        };
        Ok(Self {
            app: stored.app,
            provider_id: stored.provider.id.clone(),
            provider_type: stored.provider_type,
            provider_revision: stored.resource.revision,
            credential_generation: stored.resource.credential_generation,
            rail: rail.into(),
            account,
        })
    }

    pub(in crate::proxy) fn capture(
        execution: &ProviderExecution,
        accounts: &AccountStore,
    ) -> Result<Self, BindingSnapshotError> {
        let account = match &execution.plan.auth_ref {
            RuntimeAuthRef::ManagedAccount {
                account_id,
                expected_provider_type,
                auth_identity_generation,
            } => {
                let current = accounts
                    .find_for_provider(*expected_provider_type, Some(account_id))
                    .ok_or(BindingSnapshotError::AccountMissing)?;
                if current.auth_identity_generation != *auth_identity_generation {
                    return Err(BindingSnapshotError::AuthIdentityChanged);
                }
                Some(ManagedAccountSnapshot {
                    provider_type: *expected_provider_type,
                    account_id: account_id.clone(),
                    auth_identity_generation: *auth_identity_generation,
                    token_generation: current.token_refresh_generation,
                })
            }
            RuntimeAuthRef::Legacy {
                account_id: Some(account_id),
                ..
            } if execution.managed_account_target().is_some() => {
                let provider_type = execution.stored.provider_type;
                let current = accounts
                    .find_for_provider(provider_type, Some(account_id))
                    .ok_or(BindingSnapshotError::AccountMissing)?;
                Some(ManagedAccountSnapshot {
                    provider_type,
                    account_id: account_id.clone(),
                    auth_identity_generation: current.auth_identity_generation,
                    token_generation: current.token_refresh_generation,
                })
            }
            _ => None,
        };
        Ok(Self {
            app: execution.stored.app,
            provider_id: execution.stored.provider.id.clone(),
            provider_type: execution.stored.provider_type,
            provider_revision: execution.stored.resource.revision,
            credential_generation: execution.stored.resource.credential_generation,
            rail: format!(
                "{}:{}:{:?}",
                execution.plan.profile_id.as_str(),
                execution.plan.driver_id.as_str(),
                execution.plan.upstream_protocol,
            ),
            account,
        })
    }

    pub(in crate::proxy) fn validate(
        &self,
        execution: &ProviderExecution,
        accounts: &AccountStore,
    ) -> Result<(), BindingSnapshotError> {
        let current = Self::capture(execution, accounts)?;
        if *self == current {
            Ok(())
        } else {
            Err(BindingSnapshotError::BindingChanged)
        }
    }

    /// Token refresh is the only permitted in-place generation transition.
    /// Provider, account, rail, credential and auth-identity generations must
    /// remain identical, and token generation may only move forward.
    pub(in crate::proxy) fn advance_token_generation(
        &mut self,
        execution: &ProviderExecution,
        accounts: &AccountStore,
    ) -> Result<(), BindingSnapshotError> {
        let current = Self::capture(execution, accounts)?;
        if !self.same_identity(&current) {
            return Err(BindingSnapshotError::BindingChanged);
        }
        match (&self.account, &current.account) {
            (Some(previous), Some(next)) if next.token_generation < previous.token_generation => {
                Err(BindingSnapshotError::TokenGenerationRegressed)
            }
            (Some(_), Some(_)) => {
                *self = current;
                Ok(())
            }
            (None, None) => Ok(()),
            _ => Err(BindingSnapshotError::BindingChanged),
        }
    }

    pub(in crate::proxy) fn advance_stored_token_generation(
        &mut self,
        stored: &StoredProvider,
        accounts: &AccountStore,
        rail: impl Into<String>,
    ) -> Result<(), BindingSnapshotError> {
        let current = Self::capture_stored(stored, accounts, rail)?;
        if !self.same_identity(&current) {
            return Err(BindingSnapshotError::BindingChanged);
        }
        match (&self.account, &current.account) {
            (Some(previous), Some(next)) if next.token_generation < previous.token_generation => {
                Err(BindingSnapshotError::TokenGenerationRegressed)
            }
            (Some(_), Some(_)) => {
                *self = current;
                Ok(())
            }
            (None, None) => Ok(()),
            _ => Err(BindingSnapshotError::BindingChanged),
        }
    }

    fn same_identity(&self, other: &Self) -> bool {
        self.app == other.app
            && self.provider_id == other.provider_id
            && self.provider_type == other.provider_type
            && self.provider_revision == other.provider_revision
            && self.credential_generation == other.credential_generation
            && self.rail == other.rail
            && match (&self.account, &other.account) {
                (Some(left), Some(right)) => {
                    left.provider_type == right.provider_type
                        && left.account_id == right.account_id
                        && left.auth_identity_generation == right.auth_identity_generation
                }
                (None, None) => true,
                _ => false,
            }
    }
}

/// Request-local proof that a cache hit owns one exact snapshot mutation.
/// The scope digest must already be domain-separated by the Provider cache.
#[derive(Clone)]
pub(in crate::proxy) struct CacheSnapshotOwnership {
    domain: &'static str,
    scope_digest: [u8; 32],
    generation: u64,
    consumed: Arc<AtomicBool>,
}

impl PartialEq for CacheSnapshotOwnership {
    fn eq(&self, other: &Self) -> bool {
        self.domain == other.domain
            && self.scope_digest == other.scope_digest
            && self.generation == other.generation
            && self.consumed.load(Ordering::Acquire) == other.consumed.load(Ordering::Acquire)
    }
}

impl Eq for CacheSnapshotOwnership {}

impl fmt::Debug for CacheSnapshotOwnership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheSnapshotOwnership")
            .field("domain", &self.domain)
            .field("scope_digest", &"<opaque>")
            .field("generation", &self.generation)
            .field("consumed", &self.consumed.load(Ordering::Acquire))
            .finish()
    }
}

impl CacheSnapshotOwnership {
    pub(in crate::proxy) fn from_hit(
        domain: &'static str,
        scope_digest: [u8; 32],
        generation: u64,
    ) -> Self {
        Self {
            domain,
            scope_digest,
            generation,
            consumed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(in crate::proxy) fn authorize_mutation(
        &self,
        domain: &'static str,
        scope_digest: &[u8; 32],
        generation: u64,
    ) -> bool {
        if self.domain != domain
            || self.scope_digest != *scope_digest
            || self.generation != generation
        {
            return false;
        }
        self.consumed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::accounts::store::Account;
    use crate::domain::providers::registry::{
        DriverId, ManagedIdentityFamily, OutboundIdentityPolicy, ProfileId, UpstreamProtocol,
    };
    use crate::domain::providers::runtime::{
        ProviderRuntimePlan, RuntimeConfigurationState, RuntimeModelPolicy, RuntimeTransportPolicy,
    };
    use crate::domain::providers::store::StoredProvider;

    fn budget(total: u32) -> AttemptBudget {
        AttemptBudget::new(AttemptLimits::for_test(total, 100, total), 1_000)
    }

    #[test]
    fn mixed_recovery_sequences_cannot_exceed_total_budget() {
        let stages = [
            RecoveryStage::Auth,
            RecoveryStage::Capacity,
            RecoveryStage::BodyCompatibility,
            RecoveryStage::Reasoning,
            RecoveryStage::SessionRollover,
            RecoveryStage::Transport,
            RecoveryStage::WebsocketToHttp,
        ];
        for first in stages {
            for second in stages {
                for third in stages {
                    let mut budget = budget(2);
                    let guard = CommitGuard::default();
                    assert_eq!(
                        budget.reserve(first, "test", DelaySource::None, 1_001, &guard, None, None),
                        RetryDecision::Reserved
                    );
                    assert_eq!(
                        budget.reserve(
                            second,
                            "test",
                            DelaySource::None,
                            1_002,
                            &guard,
                            None,
                            None
                        ),
                        RetryDecision::Reserved
                    );
                    assert_eq!(
                        budget.reserve(third, "test", DelaySource::None, 1_003, &guard, None, None),
                        RetryDecision::DeniedTotalLimit
                    );
                    assert_eq!(budget.retries_used(), 2);
                }
            }
        }
    }

    #[test]
    fn per_stage_elapsed_and_commit_fences_are_fail_closed() {
        let mut budget = AttemptBudget::new(AttemptLimits::for_test(8, 10, 1), 50);
        let mut guard = CommitGuard::default();
        assert_eq!(
            budget.reserve(
                RecoveryStage::Auth,
                "unauthorized",
                DelaySource::Immediate,
                51,
                &guard,
                None,
                None,
            ),
            RetryDecision::Reserved
        );
        assert_eq!(
            budget.reserve(
                RecoveryStage::Auth,
                "unauthorized",
                DelaySource::Immediate,
                52,
                &guard,
                None,
                None,
            ),
            RetryDecision::DeniedStageLimit
        );
        assert_eq!(
            budget.reserve(
                RecoveryStage::Capacity,
                "overloaded",
                DelaySource::BoundedBackoff,
                60,
                &guard,
                None,
                None,
            ),
            RetryDecision::DeniedElapsedLimit
        );
        assert!(guard.commit_business_output(2));
        assert!(!guard.commit_business_output(3));
        assert_eq!(
            budget.reserve(
                RecoveryStage::Reasoning,
                "rejected",
                DelaySource::Immediate,
                53,
                &guard,
                None,
                None,
            ),
            RetryDecision::DeniedCommitted
        );
    }

    #[test]
    fn binding_drift_is_rejected_before_budget_is_consumed() {
        let mut budget = budget(2);
        let guard = CommitGuard::default();
        let expected = fixture_binding("provider-a", "account-a", 1, 1, 1, "rail-a");
        for current in [
            fixture_binding("provider-b", "account-a", 1, 1, 1, "rail-a"),
            fixture_binding("provider-a", "account-b", 1, 1, 1, "rail-a"),
            fixture_binding("provider-a", "account-a", 2, 1, 1, "rail-a"),
            fixture_binding("provider-a", "account-a", 1, 2, 1, "rail-a"),
            fixture_binding("provider-a", "account-a", 1, 1, 2, "rail-a"),
            fixture_binding("provider-a", "account-a", 1, 1, 1, "rail-b"),
        ] {
            assert_eq!(
                budget.reserve(
                    RecoveryStage::Auth,
                    "test",
                    DelaySource::None,
                    1_001,
                    &guard,
                    Some(&expected),
                    Some(&current),
                ),
                RetryDecision::DeniedBindingDrift
            );
        }
        assert_eq!(budget.retries_used(), 0);
    }

    #[test]
    fn token_refresh_can_only_advance_same_binding() {
        let execution = fixture_execution("provider-a", "account-a", 4);
        let mut accounts = AccountStore {
            accounts: vec![fixture_account("account-a", 4, 8)],
            ..AccountStore::default()
        };
        let mut snapshot = BindingSnapshot::capture(&execution, &accounts).unwrap();
        accounts.accounts[0].token_refresh_generation = 9;
        snapshot
            .advance_token_generation(&execution, &accounts)
            .unwrap();
        snapshot.validate(&execution, &accounts).unwrap();

        accounts.accounts[0].auth_identity_generation = 5;
        assert_eq!(
            snapshot.advance_token_generation(&execution, &accounts),
            Err(BindingSnapshotError::AuthIdentityChanged)
        );
    }

    #[test]
    fn cache_snapshot_mutation_is_exact_and_single_use() {
        let digest = [7; 32];
        let ownership = CacheSnapshotOwnership::from_hit("grok_reasoning", digest, 3);
        let cloned_ownership = ownership.clone();
        assert!(!ownership.authorize_mutation("other", &digest, 3));
        assert!(!ownership.authorize_mutation("grok_reasoning", &[8; 32], 3));
        assert!(!ownership.authorize_mutation("grok_reasoning", &digest, 4));
        assert!(ownership.authorize_mutation("grok_reasoning", &digest, 3));
        assert!(!cloned_ownership.authorize_mutation("grok_reasoning", &digest, 3));
        let debug = format!("{ownership:?}");
        assert!(!debug.contains("070707"));
    }

    #[test]
    fn binding_debug_redacts_provider_and_account_ids() {
        let binding = fixture_binding("provider-secret", "account-secret", 1, 2, 3, "rail");
        let debug = format!("{binding:?}");
        assert!(!debug.contains("provider-secret"));
        assert!(!debug.contains("account-secret"));
    }

    fn fixture_binding(
        provider_id: &str,
        account_id: &str,
        auth_generation: u64,
        token_generation: u64,
        credential_generation: u64,
        rail: &str,
    ) -> BindingSnapshot {
        BindingSnapshot {
            app: AppKind::Codex,
            provider_id: provider_id.to_string(),
            provider_type: ProviderType::CodexOAuth,
            provider_revision: 1,
            credential_generation,
            rail: rail.to_string(),
            account: Some(ManagedAccountSnapshot {
                provider_type: ProviderType::CodexOAuth,
                account_id: account_id.to_string(),
                auth_identity_generation: auth_generation,
                token_generation,
            }),
        }
    }

    fn fixture_account(id: &str, auth_generation: u64, token_generation: u64) -> Account {
        Account {
            id: id.to_string(),
            provider_type: ProviderType::CodexOAuth,
            auth_identity_generation: auth_generation,
            token_refresh_generation: token_generation,
            email: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capacity_pool_limits: Default::default(),
            quota_window_observations: Default::default(),
            capability_observations: Default::default(),
        }
    }

    fn fixture_execution(
        provider_id: &str,
        account_id: &str,
        auth_generation: u64,
    ) -> ProviderExecution {
        let mut stored: StoredProvider = serde_json::from_value(serde_json::json!({
            "app": "codex",
            "provider": {"id": provider_id, "name": "fixture", "settingsConfig": {}},
            "providerType": "codex_oauth",
            "providerTypeId": "codex_oauth",
            "revision": 1,
            "credentialGeneration": 1
        }))
        .unwrap();
        stored.provider.id = provider_id.to_string();
        ProviderExecution {
            stored,
            plan: Arc::new(ProviderRuntimePlan {
                provider_key: crate::domain::providers::registry::ProviderKey::new(
                    AppKind::Codex,
                    provider_id,
                )
                .unwrap(),
                provider_revision: 1,
                profile_id: ProfileId::parse("codex.openai_oauth").unwrap(),
                profile_schema_revision: 1,
                driver_id: DriverId::parse("oauth.openai_codex").unwrap(),
                driver_contract_revision: 1,
                endpoint: "https://example.invalid".to_string(),
                upstream_protocol: UpstreamProtocol::OpenAiResponses,
                outbound_identity_policy: OutboundIdentityPolicy::ManagedIdentity {
                    family: ManagedIdentityFamily::CodexCli,
                },
                auth_ref: RuntimeAuthRef::ManagedAccount {
                    account_id: account_id.to_string(),
                    expected_provider_type: ProviderType::CodexOAuth,
                    auth_identity_generation: auth_generation,
                },
                model_policy: RuntimeModelPolicy::Passthrough,
                coding_plan: None,
                test_model: None,
                probe_policy_fingerprint: String::new(),
                aws_region: None,
                media_policy: None,
                transport_policy: RuntimeTransportPolicy::default(),
                extra_headers: Vec::new(),
                driver_options: Default::default(),
                configuration_state: RuntimeConfigurationState::Ready,
                warnings: Vec::new(),
                runtime_fingerprint: "fixture".to_string(),
            }),
        }
    }
}
