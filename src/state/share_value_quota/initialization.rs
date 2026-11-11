//! A staged opening balance is not an immediate owner reconcile. It is only
//! accepted for a new ordinary grant, and commits with the Share and receipt.
use super::*;
use crate::domain::sharing::router_contract::{
    ShareSettingsPatch, ShareUserQuotaConversion, ShareUserValueUsageInitialization,
};
use crate::domain::usage::value_pricing::{ValueUsageEdit, ValueUsageEditAction};
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub(crate) struct ValueUsageInitializationRequest {
    pub share_id: String,
    pub operation_id: String,
    pub actor: String,
    pub edits: BTreeMap<String, ShareUserValueUsageInitialization>,
    pub conversions: BTreeMap<String, ShareUserQuotaConversion>,
}

impl ValueUsageInitializationRequest {
    pub(crate) fn from_patch(
        share_id: &str,
        patch: &ShareSettingsPatch,
    ) -> anyhow::Result<Option<Self>> {
        if patch.user_value_usage_initializations.is_none()
            && patch.user_quota_conversions.is_none()
        {
            return Ok(None);
        }
        let edits = patch
            .user_value_usage_initializations
            .clone()
            .unwrap_or_default();
        let conversions = patch.user_quota_conversions.clone().unwrap_or_default();
        anyhow::ensure!(
            conversions.len() <= 500
                && conversions
                    .iter()
                    .all(|(email, input)| email == &email.trim().to_ascii_lowercase()
                        && !input.credential.is_empty()
                        && input.credential.len() <= 32768),
            "QUOTA_CONVERSION_INVALID_INPUT"
        );
        anyhow::ensure!(
            patch
                .user_quota_conversions
                .as_ref()
                .is_none_or(|c| !c.is_empty()),
            "QUOTA_CONVERSION_INVALID_INPUT"
        );
        for email in conversions.keys() {
            anyhow::ensure!(
                patch
                    .user_grant_edits
                    .as_ref()
                    .is_some_and(|edits| edits.get(email).is_some_and(|e| e.policy.is_some()))
                    || patch
                        .user_grants
                        .as_ref()
                        .is_some_and(|grants| grants.get(email).is_some_and(|g| g.active)),
                "QUOTA_CONVERSION_INVALID_INPUT"
            );
        }
        let context = patch
            .edit_context
            .as_ref()
            .context("VALUE_QUOTA_TARGET_INVALID: initialization requires an edit context")?;
        if !conversions.is_empty() {
            anyhow::ensure!(
                !context.operation_id.is_empty()
                    && context.operation_id.len() <= 128
                    && context.operation_id.trim() == context.operation_id
                    && !context.operation_id.chars().any(char::is_control),
                "QUOTA_CONVERSION_INVALID_INPUT"
            );
        }
        anyhow::ensure!(
            patch.managed_grant.is_none(),
            "VALUE_QUOTA_TARGET_INVALID: managed grants are read-only"
        );
        anyhow::ensure!(
            patch
                .user_usage_edits
                .as_ref()
                .is_none_or(
                    |token_edits| !edits
                        .keys()
                        .chain(conversions.keys())
                        .any(|email| token_edits
                            .keys()
                            .any(|key| key.eq_ignore_ascii_case(email)))
                ),
            "VALUE_QUOTA_TARGET_INVALID: do not mix Token and USD usage edits"
        );
        for input in edits.values() {
            anyhow::ensure!(
                !input.reason.trim().is_empty()
                    && input.reason.len() <= 1000
                    && !input.window_id.is_empty()
                    && input.window_id.len() <= 512,
                "VALUE_QUOTA_TARGET_INVALID: a valid window and audit reason are required"
            );
        }
        Ok(Some(Self {
            share_id: share_id.into(),
            operation_id: context.operation_id.clone(),
            actor: context
                .actor
                .clone()
                .context("VALUE_QUOTA_TARGET_INVALID: initialization requires an actor")?,
            edits,
            conversions,
        }))
    }

    pub(crate) fn prepare(
        &self,
        previous: &ShareStore,
        candidate: &ShareStore,
        config: &ServerConfig,
        usage: Option<&crate::domain::usage::store::UsageStore>,
        now: i64,
    ) -> anyhow::Result<Vec<ledger::Initialization>> {
        if self.edits.is_empty() {
            return Ok(Vec::new());
        }
        anyhow::ensure!(
            !self.operation_id.is_empty()
                && self.operation_id.len() <= 128
                && !self.actor.trim().is_empty()
                && !self.edits.is_empty()
                && self.edits.len() <= 500,
            "VALUE_QUOTA_TARGET_INVALID: invalid initialization context"
        );
        // The mutation already verified the full payload fingerprint. A saved
        // receipt proves the opening balances committed along with the grant.
        if previous
            .applied_settings_operations
            .contains_key(&self.operation_id)
        {
            return Ok(Vec::new());
        }
        let usage =
            usage.context("VALUE_QUOTA_NOT_ACTIVE: usage history is busy; preview again")?;
        let before = previous.get(&self.share_id);
        let share = candidate
            .get(&self.share_id)
            .context("VALUE_QUOTA_NOT_ACTIVE: Share not found")?;
        self.edits
            .iter()
            .map(|(email, input)| {
                anyhow::ensure!(
                    !input.reason.trim().is_empty()
                        && input.reason.len() <= 1000
                        && !input.window_id.is_empty()
                        && input.window_id.len() <= 512,
                    "VALUE_QUOTA_TARGET_INVALID: a valid window and audit reason are required"
                );
                anyhow::ensure!(
                    email == &email.trim().to_ascii_lowercase()
                        && before.is_none_or(|share| !share.user_grants.contains_key(email))
                        && usage.share_user_quota_usage(&self.share_id, email, 0, i64::MAX)
                            == (0, 0),
                    "VALUE_QUOTA_TARGET_INVALID: only genuinely new users can initialize usage"
                );
                let grant = share
                    .user_grants
                    .get(email)
                    .filter(|grant| {
                        grant.active
                            && grant.policy.value_quota.is_some()
                            && grant.manager != ShareGrantManager::RouterShareMarket
                            && grant.pending_value_quota.is_none()
                    })
                    .context("VALUE_QUOTA_NOT_ACTIVE: the new USD grant must take effect now")?;
                let window = window(grant, now)?;
                anyhow::ensure!(
                    window.id == input.window_id,
                    "VALUE_QUOTA_REVISION_CONFLICT: initialization window changed; preview again"
                );
                let scope = scope(config, &self.share_id, email)?;
                let operation_id = format!(
                    "initial:{:x}",
                    Sha256::digest(serde_json::to_vec(&(
                        &self.operation_id,
                        &self.share_id,
                        email
                    ))?)
                );
                Ok(ledger::Initialization {
                    scope,
                    window,
                    grant_revision: grant.revision,
                    edit: ValueUsageEdit {
                        action: ValueUsageEditAction::Set,
                        target_picos: Some(input.target_picos.clone()),
                        expected_grant_revision: grant.revision,
                        window_id: input.window_id.clone(),
                        expected_watermark: 0,
                        operation_id,
                        reason: input.reason.trim().into(),
                    },
                })
            })
            .collect()
    }
}

#[derive(Clone)]
pub(crate) struct InitializedShareSnapshot {
    pub shares: ShareStore,
    pub initializations: Vec<ledger::Initialization>,
    pub conversions: Vec<ledger::conversion::Conversion>,
    pub actor: String,
    pub now: i64,
}
impl PersistedStateSnapshot for InitializedShareSnapshot {
    const NAME: &'static str = "shares and initial USD usage";
    fn save_snapshot(&self, config_dir: &Path) -> anyhow::Result<()> {
        server_sqlite::persist_shares_with_value_initializations(
            config_dir,
            &self.shares,
            &self.initializations,
            &self.conversions,
            &self.actor,
            self.now,
        )
    }
    fn load_snapshot(config_dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            shares: <ShareStore as PersistedStateSnapshot>::load_snapshot(config_dir)?,
            initializations: Vec::new(),
            conversions: Vec::new(),
            actor: String::new(),
            now: 0,
        })
    }
    fn reconciliation_value(&self) -> Option<Value> {
        serde_json::to_value(&self.shares).ok()
    }
}
