use super::*;
use crate::domain::accounts::antigravity_access::AntigravityService;
use crate::domain::accounts::capability_evidence::{
    observation_key, AccountCapabilityObservationDraft, AccountCapabilityObservationState,
    ANTIGRAVITY_CODE_PLAN_CAPABILITY,
};
use crate::domain::antigravity_identity::AntigravityClientIdentity;

#[derive(Clone)]
pub(crate) struct AntigravityServiceFence {
    account_id: String,
    provider_type: ProviderType,
    auth_generation: u64,
    refresh_generation: u64,
    incarnation: u64,
    fingerprint: [u8; 32],
    client: AntigravityClientIdentity,
    access_generations: std::collections::BTreeMap<String, u64>,
}

impl AntigravityServiceFence {
    pub(crate) fn capture(
        account: &Account,
        incarnation: u64,
        client: AntigravityClientIdentity,
    ) -> Option<Self> {
        if !matches!(
            account.provider_type,
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
        ) {
            return None;
        }
        Some(Self {
            account_id: account.id.clone(),
            provider_type: account.provider_type,
            auth_generation: account.auth_identity_generation,
            refresh_generation: account.token_refresh_generation,
            incarnation,
            fingerprint: crate::clients::oauth::antigravity_models::request_identity_fingerprint(
                account,
            )?,
            client,
            access_generations: account.antigravity_access_generations.clone(),
        })
    }
}

impl ServerStateInner {
    pub(crate) async fn block_antigravity_service(
        &self,
        fence: &AntigravityServiceFence,
        service: AntigravityService,
    ) -> anyhow::Result<bool> {
        let fence = fence.clone();
        self.mutate_accounts_immediate(move |accounts| {
            if !fence.client.is_current_for(fence.provider_type) || accounts.account_incarnation(&fence.account_id) != Some(fence.incarnation)
                || accounts.find_for_provider(fence.provider_type, Some(&fence.account_id)).is_none_or(|account| {
                    account.auth_identity_generation != fence.auth_generation || account.token_refresh_generation != fence.refresh_generation
                        || account.antigravity_access_generations.get(service.dimension()).copied().unwrap_or(0)
                            != fence.access_generations.get(service.dimension()).copied().unwrap_or(0)
                        || crate::clients::oauth::antigravity_models::request_identity_fingerprint(account) != Some(fence.fingerprint)
                }) { return false; }
            accounts.record_account_capability_observations(&fence.account_id,
                [AccountCapabilityObservationDraft::antigravity_feature(service.dimension(), AccountCapabilityObservationState::Unsupported,
                    "google_rpc_error_info", Some("tos_violation"), crate::infra::time::now_ms() as i64, Some(i64::MAX))
                    .with_antigravity_client_identity(&fence.client)])
        }).await
    }

    /// An explicit administrator retry, not an OAuth validity decision. Normal
    /// token/quota refresh never clears this service-specific observation.
    pub(crate) async fn reset_antigravity_service(
        &self,
        id: &str,
        generation: u64,
        service: AntigravityService,
    ) -> anyhow::Result<bool> {
        self.try_mutate_accounts_immediate(|accounts| {
            let account = accounts
                .accounts
                .iter_mut()
                .find(|account| account.id == id)
                .context("account not found")?;
            anyhow::ensure!(
                matches!(
                    account.provider_type,
                    ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
                ),
                "not an Antigravity account"
            );
            anyhow::ensure!(
                account.auth_identity_generation == generation,
                "account identity changed; reload before retry"
            );
            let epoch = account
                .antigravity_access_generations
                .entry(service.dimension().to_string())
                .or_default();
            *epoch = epoch
                .checked_add(1)
                .context("service retry generation exhausted")?;
            Ok::<_, anyhow::Error>(
                account
                    .capability_observations
                    .remove(&observation_key(
                        ANTIGRAVITY_CODE_PLAN_CAPABILITY,
                        service.dimension(),
                    ))
                    .is_some(),
            )
        })
        .await?
    }
}
