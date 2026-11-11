//! Read-only signed previews, resumable historical preparation and atomic
//! ordinary-grant unit changes. Market contracts retain their own semantics.
use super::*;
use crate::domain::sharing::router_contract::{
    ShareUsageRebaseSource, ShareUserQuotaConversion, ShareUserUsageRebase,
};
use crate::domain::usage::store::UsageStore;
use hmac::{Hmac, Mac};
use sha2::Sha256;

const PREVIEW_TTL_MS: i64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ticket {
    scope: ledger::Scope,
    config_revision: u64,
    grant_revision: u64,
    source_policy: ShareUserPolicy,
    target_policy: ShareUserPolicy,
    window_id: String,
    metered_from_ms: i64,
    adjustment_fingerprint: String,
    pricing_basis: Option<String>,
    active_pricing_revision: String,
    window_ends_at_ms: Option<i64>,
    account_cycle_revision: Option<u64>,
    observed_picos: String,
    observed_tokens: u64,
    non_token_basis: Option<String>,
    expires_at_ms: i64,
}

pub(super) fn needed(old: Option<&ShareUserGrant>, policy: &ShareUserPolicy) -> bool {
    let old_policy = old.map(|g| &g.policy);
    old_policy.is_some_and(|old| old.value_quota.is_some()) != policy.value_quota.is_some()
        || (policy.value_quota.is_some()
            && old_policy.is_some_and(|old| {
                old.token_period != policy.token_period
                    || old.token_period_anchor_at_ms != policy.token_period_anchor_at_ms
            }))
}

fn unit(policy: &ShareUserPolicy) -> &'static str {
    if policy.value_quota.is_some() {
        "usdEquivalent"
    } else {
        "tokens"
    }
}
fn secret(config: &ServerConfig) -> anyhow::Result<&str> {
    config
        .registered_router_identity()
        .map(|i| i.private_key.as_str())
        .context("QUOTA_CONVERSION_ROUTER_REQUIRED")
}
fn encode(config: &ServerConfig, ticket: &Ticket) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(ticket)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret(config)?.as_bytes())?;
    mac.update(&bytes);
    Ok(format!(
        "{}.{}",
        hex::encode(&bytes),
        hex::encode(mac.finalize().into_bytes())
    ))
}
fn decode(config: &ServerConfig, input: &str) -> anyhow::Result<Ticket> {
    anyhow::ensure!(input.len() <= 32768, "QUOTA_CONVERSION_PREVIEW_INVALID");
    let (raw, sig) = input
        .split_once('.')
        .context("QUOTA_CONVERSION_PREVIEW_INVALID")?;
    let bytes = hex::decode(raw).context("QUOTA_CONVERSION_PREVIEW_INVALID")?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret(config)?.as_bytes())?;
    mac.update(&bytes);
    mac.verify_slice(&hex::decode(sig)?)
        .map_err(|_| anyhow::anyhow!("QUOTA_CONVERSION_PREVIEW_INVALID"))?;
    Ok(serde_json::from_slice(&bytes)?)
}
fn digest<T: Serialize>(value: &T) -> anyhow::Result<String> {
    use sha2::Digest;
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(value)?)))
}

fn token_adjustment(grant: &ShareUserGrant) -> anyhow::Result<Option<String>> {
    if grant.policy.token_period == ShareTokenPeriod::AccountReset {
        return grant
            .account_usage_baseline
            .as_ref()
            .filter(|b| {
                grant
                    .account_quota_cycle
                    .as_ref()
                    .is_some_and(|c| b.source_ref == c.source_ref && b.cycle_id == c.cycle_id)
            })
            .map(|b| digest(b).map(|hash| format!("token:{hash}")))
            .transpose();
    }
    let w = crate::domain::sharing::token_period::token_period_window(&grant.policy, now_ms_i64())
        .map_err(anyhow::Error::msg)?;
    grant
        .usage_rebase
        .as_ref()
        .filter(|r| {
            r.period == grant.policy.token_period
                && r.anchor_at_ms == grant.policy.token_period_anchor_at_ms
                && r.window_starts_at_ms == w.starts_at_ms
                && r.window_ends_at_ms == w.ends_at_ms
        })
        .map(|r| digest(r).map(|hash| format!("token:{hash}")))
        .transpose()
}

fn target_grant(
    old: Option<&ShareUserGrant>,
    email: &str,
    policy: &ShareUserPolicy,
    revision: &str,
    cycle: Option<&crate::domain::sharing::quota_cycle::AccountQuotaCycle>,
    now: i64,
) -> anyhow::Result<ShareUserGrant> {
    let mut grant = old.cloned().unwrap_or_default();
    grant.email = email.into();
    grant.active = true;
    grant.policy = policy.clone();
    if policy.token_period == ShareTokenPeriod::AccountReset {
        grant.account_quota_cycle = cycle.cloned().or(grant.account_quota_cycle);
    }
    let mut binding = old
        .and_then(|g| g.value_metering.clone())
        .unwrap_or(ValueMeteringBinding {
            quota_policy_epoch: "preview".into(),
            value_metered_from_ms: 0,
            effective_from_ms: now,
            pricing_revision: revision.into(),
        });
    // Backfilling may extend a legacy origin backwards. Never move it forward:
    // deleting/re-adding, suspending or a round trip cannot erase usage.
    let mut probe = grant.clone();
    probe.value_metering = Some(binding.clone());
    let start = window(&probe, now)?.starts_at_ms;
    binding.value_metered_from_ms = binding.value_metered_from_ms.min(start);
    binding.effective_from_ms = now;
    binding.pricing_revision = revision.into();
    grant.value_metering = Some(binding);
    grant.pending_value_quota = None;
    Ok(grant)
}

fn proof(
    dir: &Path,
    identity: &ledger::Scope,
    w: &ledger::Window,
    usage: &UsageStore,
    grant: &ShareUserGrant,
    now: i64,
) -> anyhow::Result<ledger::conversion::CoverageProof> {
    let from = if grant.policy.value_quota.is_some() {
        ledger::conversion::proof_start(dir, identity, w)?
    } else {
        w.starts_at_ms.max(w.metered_from_ms)
    };
    let until = w.ends_at_ms.unwrap_or(i64::MAX).min(now.saturating_add(1));
    let (tokens, requests) = if let Some(cycle) = &w.account {
        usage
            .quota_ledger
            .query(
                cycle,
                &identity.share_id,
                &identity.email,
                usage.quota_ledger.watermark,
            )
            .map_err(anyhow::Error::msg)?
    } else {
        usage.share_user_quota_usage(&identity.share_id, &identity.email, from, until)
    };
    Ok(ledger::conversion::CoverageProof {
        from_ms: from,
        until_ms: until,
        tokens,
        requests,
        minimum_tokens: if from == w.starts_at_ms.max(w.metered_from_ms) && w.account.is_none() {
            grant.usage.tokens_for_policy(&grant.policy, now)
        } else {
            0
        },
    })
}

impl ServerStateInner {
    pub(crate) async fn quota_conversion_router_negotiated(&self) -> bool {
        let config = self.config_snapshot().await;
        self.router_quota_capability
            .read()
            .await
            .as_ref()
            .is_some_and(|(base, id, at, supported)| {
                supported.8
                    && at.elapsed() < Duration::from_secs(300)
                    && config.router_api_base() == Some(base.as_str())
                    && config
                        .registered_router_identity()
                        .is_some_and(|identity| identity.installation_id == *id)
            })
    }

    /// Explicit history preparation is separate from preview. A page is
    /// bounded and durable; repeated calls resume it after a timeout/restart.
    pub(crate) async fn prepare_share_quota_history(
        &self,
        share_id: &str,
        expected: u64,
        emails: Vec<String>,
        restart: bool,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.quota_conversion_router_negotiated().await,
            "QUOTA_CONVERSION_ROUTER_UPGRADE_REQUIRED"
        );
        anyhow::ensure!(
            !emails.is_empty() && emails.len() <= 500,
            "QUOTA_CONVERSION_INVALID_USERS"
        );
        let config = self.config_snapshot().await;
        let share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        anyhow::ensure!(
            share.config_revision == expected,
            "QUOTA_CONVERSION_REVISION_CONFLICT"
        );
        let mut scopes = Vec::new();
        for email in emails {
            let email = email.trim().to_ascii_lowercase();
            anyhow::ensure!(
                share
                    .user_grants
                    .get(&email)
                    .is_none_or(|g| g.manager != ShareGrantManager::RouterShareMarket),
                "QUOTA_CONVERSION_MANAGED_READ_ONLY"
            );
            scopes.push(scope(&config, share_id, &email)?);
        }
        let dir = self.config_dir.clone();
        let rows = tokio::task::spawn_blocking(move || -> anyhow::Result<Vec<Value>> {
            scopes
                .into_iter()
                .map(|identity| {
                    let progress =
                        ledger::evidence::prepare_page(&dir, &identity, restart, now_ms_i64())?;
                    Ok(serde_json::json!({"email":identity.email,"preparation":progress}))
                })
                .collect()
        })
        .await??;
        Ok(serde_json::json!({"shareId":share_id,"configRevision":expected,"items":rows}))
    }

    pub(crate) async fn share_quota_conversion_result(
        &self,
        share_id: &str,
        operation: &str,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            !operation.is_empty() && operation.len() <= 128,
            "QUOTA_CONVERSION_INVALID_INPUT"
        );
        let config = self.config_snapshot().await;
        let receipt =
            ledger::conversion::settings_receipt(&self.config_dir, operation, share_id, &config)?;
        let conversions =
            ledger::conversion::receipts(&self.config_dir, operation, share_id, &config)?;
        let statuses = if conversions.is_empty() {
            Vec::new()
        } else {
            self.value_quota_statuses(share_id).await?
        };
        Ok(
            serde_json::json!({"committed":receipt.is_some(),"settingsReceipt":receipt,"quotaConversions":conversions,"valueQuotaStatuses":statuses}),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn conversion_preview_row(
        &self,
        config: &ServerConfig,
        share: &Share,
        email: &str,
        policy: &ShareUserPolicy,
        usage: &UsageStore,
        now: i64,
        negotiated: bool,
    ) -> anyhow::Result<Value> {
        let previous = share.user_grants.get(email);
        let identity = scope(config, &share.id, email)?;
        let revision = ledger::active_revision(&self.config_dir, &identity)?
            .or_else(|| {
                policy
                    .value_quota
                    .is_none()
                    .then(|| {
                        previous
                            .and_then(|g| g.value_metering.as_ref())
                            .map(|b| b.pricing_revision.clone())
                    })
                    .flatten()
            })
            .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
        let cycle = self.shares.try_read().ok().and_then(|s| {
            s.quota_cycle_sources
                .get(&share.id)
                .and_then(|source| s.quota_cycle_ledger.source(source))
                .and_then(|source| source.cycle.clone())
        });
        let grant = target_grant(previous, email, policy, &revision, cycle.as_ref(), now)?;
        let w = window(&grant, now)?;
        let proof = proof(&self.config_dir, &identity, &w, usage, &grant, now)?;
        let preview = ledger::conversion::preview(&self.config_dir, &identity, &w, &proof)?;
        let old = previous.cloned().unwrap_or_default();
        let source_preview =
            source_preview(&self.config_dir, &identity, &w, &preview, &old, usage, now)?;
        let source_adjustment = if old.policy.value_quota.is_some() {
            source_value_adjustment(&source_preview)?
        } else {
            token_adjustment(&old)?
        };
        let source_offset = if old.policy.value_quota.is_some() {
            source_preview.status.manual_offset_picos.clone()
        } else {
            crate::domain::sharing::shares::quota_view_from_history(
                usage, &share.id, email, &old, now,
            )
            .map_err(anyhow::Error::msg)?
            .manual_offset_tokens
            .to_string()
        };
        let mapped = source_adjustment
            .as_deref()
            .map(|source| {
                ledger::conversion::mapped_offset(
                    &self.config_dir,
                    &identity,
                    &w,
                    source,
                    unit(policy),
                )
            })
            .transpose()?
            .flatten();
        let calibration = (source_offset != "0"
            || preview.non_token_basis.is_some() && policy.value_quota.is_none())
            && mapped.is_none();
        let history_needed = policy.value_quota.is_some();
        let ready = negotiated
            && (history_needed && preview.complete
                || !history_needed && preview.token_history_complete && preview.pending_complete)
            && preview.status.metering_status != "blocked"
            && source_preview.status.metering_status != "blocked";
        let code = if !negotiated {
            "conversion_upgrade_required"
        } else if preview.status.metering_status == "blocked"
            || source_preview.status.metering_status == "blocked"
        {
            "blocked_needs_reconcile"
        } else if history_needed && !preview.complete
            || !history_needed && !preview.token_history_complete
        {
            if preview.preparation_state.as_deref() == Some("preparing") {
                "history_preparing"
            } else {
                "history_incomplete"
            }
        } else if !history_needed && !preview.pending_complete {
            "pending_evidence_missing"
        } else {
            "ready"
        };
        let observed_tokens = crate::domain::sharing::shares::quota_view_from_history(
            usage, &share.id, email, &grant, now,
        )
        .map_err(anyhow::Error::msg)?
        .observed_tokens_used;
        let ticket = Ticket {
            active_pricing_revision: revision,
            window_ends_at_ms: w.ends_at_ms,
            account_cycle_revision: w.account.as_ref().map(|c| c.cycle_revision),
            non_token_basis: preview.non_token_basis.clone(),
            observed_picos: preview.status.observed_picos.clone(),
            observed_tokens,
            scope: identity,
            config_revision: share.config_revision,
            grant_revision: old.revision,
            source_policy: old.policy.clone(),
            target_policy: policy.clone(),
            window_id: w.id.clone(),
            metered_from_ms: w.metered_from_ms,
            adjustment_fingerprint: digest(&(
                token_adjustment(&old)?,
                &source_preview.source_adjustment,
                &preview.source_adjustment,
            ))?,
            pricing_basis: preview.pricing_basis.clone(),
            expires_at_ms: w
                .ends_at_ms
                .unwrap_or(i64::MAX)
                .min(now.saturating_add(PREVIEW_TTL_MS)),
        };
        let offset = mapped
            .as_deref()
            .unwrap_or(&preview.status.manual_offset_picos);
        let used = if policy.value_quota.is_some() {
            crate::domain::usage::value_pricing::unsigned_decimal(&preview.status.observed_picos)?
                .checked_add(crate::domain::usage::value_pricing::unsigned_decimal(
                    offset,
                )?)
                .context("value quota arithmetic overflow")?
                .to_string()
        } else {
            preview.status.used_picos.clone()
        };
        let observed_tokens = crate::domain::sharing::shares::quota_view_from_history(
            usage, &share.id, email, &grant, now,
        )
        .map_err(anyhow::Error::msg)?
        .observed_tokens_used;
        let effective_tokens = if policy.value_quota.is_none() {
            let offset = mapped.as_deref().unwrap_or("0").parse::<i128>()?;
            u64::try_from(
                i128::from(observed_tokens)
                    .checked_add(offset)
                    .context("Token arithmetic overflow")?
                    .max(0),
            )?
        } else {
            old.usage_quota
                .map_or(observed_tokens, |q| q.effective_tokens_used)
        };
        let used_amount = crate::domain::usage::value_pricing::unsigned_decimal(&used)?;
        let pending_amount = crate::domain::usage::value_pricing::unsigned_decimal(
            &preview.status.pending_risk_picos,
        )?;
        let remaining = policy
            .value_quota
            .as_ref()
            .map(|q| q.limit_picos())
            .transpose()?
            .map(|limit| {
                limit
                    .saturating_sub(used_amount)
                    .saturating_sub(pending_amount)
                    .max(0)
                    .to_string()
            });
        Ok(
            serde_json::json!({"email":email,"ready":ready,"reasonCode":code,"effectiveFromMs":now,"meteredFromMs":w.metered_from_ms,
            "windowId":w.id,"windowStartsAtMs":w.starts_at_ms,"resetsAtMs":w.ends_at_ms,"usedPicos":if ready {Some(used)}else {None},
            "pendingRiskPicos":preview.status.pending_risk_picos,"pendingTokens":preview.pending_tokens,"tokenLimit":policy.token_limit,"limitUsdMicros":policy.value_quota.as_ref().map(|q| &q.limit_usd_micros),"remainingTokens":policy.token_limit.map(|cap|cap.saturating_sub(effective_tokens).saturating_sub(preview.pending_tokens)),"remainingPicos":remaining,"observedTokens":observed_tokens,"effectiveTokens":effective_tokens,
            "sourceUnit":unit(&old.policy),"targetUnit":unit(policy),"conversionRequired":true,"calibrationRequired":calibration,
            "conversionCredential":encode(config,&ticket)?,"credentialExpiresAtMs":ticket.expires_at_ms,
            "coverage":preview,"canInitializeUsage":previous.is_none() && proof.tokens==0 && proof.requests==0 && ledger::can_initialize_usage(&self.config_dir,&ticket.scope).unwrap_or(false)}),
        )
    }

    /// Invoked under the usage + Share commit gates, before bindings are
    /// prepared. Browser values never enter the candidate without verification.
    pub(in crate::state) fn prepare_unit_conversions(
        &self,
        request: &ValueUsageInitializationRequest,
        previous: &ShareStore,
        candidate: &mut ShareStore,
        config: &ServerConfig,
        usage: Option<&UsageStore>,
        now: i64,
    ) -> anyhow::Result<Vec<ledger::conversion::Conversion>> {
        if request.conversions.is_empty() {
            return Ok(Vec::new());
        }
        anyhow::ensure!(
            !request.operation_id.is_empty()
                && request.operation_id.len() <= 128
                && request.operation_id.trim() == request.operation_id
                && !request.operation_id.chars().any(char::is_control)
                && !request.actor.trim().is_empty()
                && request.conversions.len() <= 500,
            "QUOTA_CONVERSION_INVALID_INPUT"
        );
        if previous
            .applied_settings_operations
            .contains_key(&request.operation_id)
        {
            return Ok(Vec::new());
        }
        let usage = usage.context("QUOTA_CONVERSION_HISTORY_BUSY")?;
        anyhow::ensure!(
            !self
                .quota_usage_degraded
                .load(std::sync::atomic::Ordering::Acquire),
            "QUOTA_CONVERSION_HISTORY_DEGRADED"
        );
        let before = previous.get(&request.share_id).context("Share not found")?;
        let after = candidate
            .shares
            .iter_mut()
            .find(|s| s.id == request.share_id)
            .context("Share not found")?;
        let mut prepared = Vec::new();
        for (email, input) in &request.conversions {
            let old = before.user_grants.get(email);
            let grant = after
                .user_grants
                .get_mut(email)
                .filter(|g| g.active && g.manager != ShareGrantManager::RouterShareMarket)
                .context("QUOTA_CONVERSION_MANAGED_READ_ONLY")?;
            let ticket = decode(config, &input.credential)?;
            anyhow::ensure!(
                ticket.expires_at_ms > now
                    && ticket.config_revision == before.config_revision
                    && ticket.scope == scope(config, &request.share_id, email)?
                    && ticket.grant_revision == old.map_or(0, |g| g.revision)
                    && ticket.source_policy == old.map(|g| g.policy.clone()).unwrap_or_default()
                    && ticket.target_policy == grant.policy,
                "QUOTA_CONVERSION_PREVIEW_STALE"
            );
            anyhow::ensure!(needed(old, &grant.policy), "QUOTA_CONVERSION_NOT_REQUIRED");
            let revision = ledger::active_revision(&self.config_dir, &ticket.scope)?
                .or_else(|| {
                    grant
                        .policy
                        .value_quota
                        .is_none()
                        .then(|| {
                            old.and_then(|g| g.value_metering.as_ref())
                                .map(|b| b.pricing_revision.clone())
                        })
                        .flatten()
                })
                .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
            let proposed = target_grant(
                old,
                email,
                &grant.policy,
                &revision,
                grant.account_quota_cycle.as_ref(),
                now,
            )?;
            let w = window(&proposed, now)?;
            anyhow::ensure!(
                w.id == ticket.window_id
                    && w.metered_from_ms == ticket.metered_from_ms
                    && w.ends_at_ms == ticket.window_ends_at_ms
                    && w.account.as_ref().map(|c| c.cycle_revision)
                        == ticket.account_cycle_revision,
                "QUOTA_CONVERSION_WINDOW_CHANGED"
            );
            let proof = proof(&self.config_dir, &ticket.scope, &w, usage, &proposed, now)?;
            let preview = ledger::conversion::preview(&self.config_dir, &ticket.scope, &w, &proof)?;
            let old_grant = old.cloned().unwrap_or_default();
            let source_preview = source_preview(
                &self.config_dir,
                &ticket.scope,
                &w,
                &preview,
                &old_grant,
                usage,
                now,
            )?;
            anyhow::ensure!(
                source_preview.status.metering_status != "blocked",
                "QUOTA_CONVERSION_RESTORE_FENCED"
            );
            anyhow::ensure!(
                ticket.adjustment_fingerprint
                    == digest(&(
                        old.map(token_adjustment).transpose()?.flatten(),
                        &source_preview.source_adjustment,
                        &preview.source_adjustment
                    ))?
                    && ticket.pricing_basis == preview.pricing_basis
                    && ticket.active_pricing_revision == revision,
                "QUOTA_CONVERSION_PREVIEW_STALE"
            );
            if grant.policy.value_quota.is_some() {
                anyhow::ensure!(preview.complete, "QUOTA_CONVERSION_HISTORY_INCOMPLETE");
            }
            if grant.policy.value_quota.is_none() {
                anyhow::ensure!(
                    preview.token_history_complete,
                    "QUOTA_CONVERSION_HISTORY_INCOMPLETE"
                );
                anyhow::ensure!(
                    preview.pending_complete,
                    "QUOTA_CONVERSION_PENDING_EVIDENCE_MISSING"
                );
                anyhow::ensure!(
                    ticket.non_token_basis == preview.non_token_basis,
                    "QUOTA_CONVERSION_PREVIEW_STALE"
                );
            }
            let source_usd = old.is_some_and(|g| g.policy.value_quota.is_some());
            let source_adjustment = if source_usd {
                source_value_adjustment(&source_preview)?
            } else {
                old.map(token_adjustment).transpose()?.flatten()
            };
            let source_offset = if source_usd {
                source_preview.status.manual_offset_picos.clone()
            } else {
                old.map(|g| {
                    crate::domain::sharing::shares::quota_view_from_history(
                        usage,
                        &request.share_id,
                        email,
                        g,
                        now,
                    )
                })
                .transpose()
                .map_err(anyhow::Error::msg)?
                .map_or(0, |q| q.manual_offset_tokens)
                .to_string()
            };
            let target_unit = unit(&grant.policy);
            let mapped = source_adjustment
                .as_deref()
                .map(|source| {
                    ledger::conversion::mapped_offset(
                        &self.config_dir,
                        &ticket.scope,
                        &w,
                        source,
                        target_unit,
                    )
                })
                .transpose()?
                .flatten();
            let observed_tokens = crate::domain::sharing::shares::quota_view_from_history(
                usage,
                &request.share_id,
                email,
                grant,
                now,
            )
            .map_err(anyhow::Error::msg)?
            .observed_tokens_used;
            if target_unit == "tokens" && preview.non_token_basis.is_some() && mapped.is_none() {
                anyhow::ensure!(
                    input.target_used_tokens.is_some(),
                    "QUOTA_CONVERSION_CALIBRATION_REQUIRED"
                );
            }
            let mut target_offset = calibration_offset(
                input,
                target_unit,
                &source_offset,
                mapped,
                &ticket.observed_picos,
                ticket.observed_tokens,
            )?;
            if target_offset.is_none()
                && target_unit == "usdEquivalent"
                && preview.status.manual_offset_picos != "0"
            {
                target_offset = Some("0".into());
            }
            let reason = input
                .reason
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "Carry audited quota adjustment across units".into());
            let mut binding = proposed
                .value_metering
                .context("VALUE_QUOTA_BINDING_UNAVAILABLE")?;
            binding.quota_policy_epoch = crate::domain::sharing::quota_cycle::opaque_id();
            grant.value_metering = Some(binding);
            grant.pending_value_quota = None;
            grant.quota_conversion_id = Some(request.operation_id.clone());
            if target_unit == "tokens"
                && grant.policy.token_period == ShareTokenPeriod::AccountReset
            {
                let cycle = proposed
                    .account_quota_cycle
                    .as_ref()
                    .context("VALUE_QUOTA_ACCOUNT_CYCLE_UNAVAILABLE")?;
                grant.account_quota_cycle = Some(cycle.clone());
                let offset = target_offset.as_deref().unwrap_or("0").parse::<i128>()?;
                grant.usage_rebase = None;
                grant.account_usage_baseline = if target_offset.is_none() {
                    None
                } else {
                    Some(crate::domain::sharing::quota_cycle::AccountQuotaBaseline {
                        source_ref: cycle.source_ref.clone(),
                        cycle_id: cycle.cycle_id.clone(),
                        target_tokens: u64::try_from(
                            i128::from(observed_tokens)
                                .checked_add(offset)
                                .context("Token arithmetic overflow")?
                                .max(0),
                        )?,
                        usage_watermark: usage.quota_ledger.watermark,
                        applied_at_ms: now,
                        applied_by: Some(request.actor.clone()),
                    })
                };
            } else if target_unit == "tokens" {
                let offset = target_offset.as_deref().unwrap_or("0").parse::<i128>()?;
                grant.usage_rebase = if target_offset.is_none() {
                    None
                } else {
                    Some(ShareUserUsageRebase {
                        period: grant.policy.token_period,
                        anchor_at_ms: grant.policy.token_period_anchor_at_ms,
                        window_starts_at_ms: (w.starts_at_ms != 0).then_some(w.starts_at_ms),
                        window_ends_at_ms: w.ends_at_ms,
                        target_tokens: u64::try_from(
                            i128::from(observed_tokens)
                                .checked_add(offset)
                                .context("Token arithmetic overflow")?
                                .max(0),
                        )?,
                        observed_tokens_at_rebase: observed_tokens,
                        observed_requests_at_rebase: proof.requests,
                        usage_watermark: usage.quota_ledger.watermark,
                        applied_at_ms: now,
                        applied_by: Some(request.actor.clone()),
                        source: ShareUsageRebaseSource::Manual,
                    })
                };
            }
            prepared.push(ledger::conversion::Conversion {
                scope: ticket.scope,
                window: w,
                proof,
                require_history: grant.policy.value_quota.is_some(),
                non_token_basis: preview.non_token_basis.clone(),
                operation_id: request.operation_id.clone(),
                payload_sha256: digest(input)?,
                source_unit: unit(&ticket.source_policy).into(),
                target_unit: target_unit.into(),
                target_limit: grant
                    .policy
                    .value_quota
                    .as_ref()
                    .map(|q| q.limit_usd_micros.clone())
                    .or_else(|| grant.policy.token_limit.map(|n| n.to_string())),
                target_adjustment: if target_unit == "tokens" {
                    token_adjustment(grant)?
                } else {
                    None
                },
                target_offset,
                source_adjustment,
                source_offset,
                actor: request.actor.clone(),
                reason,
                now,
            });
        }
        candidate
            .rebuild_user_usage_from_history(&request.share_id, usage, now)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(prepared)
    }
}

fn calibration_offset(
    input: &ShareUserQuotaConversion,
    target: &str,
    source_offset: &str,
    mapped: Option<String>,
    observed_picos: &str,
    observed_tokens: u64,
) -> anyhow::Result<Option<String>> {
    anyhow::ensure!(
        input.reason.as_ref().is_none_or(|s| s.len() <= 1000),
        "QUOTA_CONVERSION_INVALID_REASON"
    );
    let supplied = if target == "usdEquivalent" {
        anyhow::ensure!(
            input.target_used_tokens.is_none(),
            "QUOTA_CONVERSION_WRONG_CALIBRATION_UNIT"
        );
        input
            .target_used_picos
            .as_deref()
            .map(|n| -> anyhow::Result<String> {
                let total = crate::domain::usage::value_pricing::unsigned_decimal(n)?;
                let observed =
                    crate::domain::usage::value_pricing::unsigned_decimal(observed_picos)?;
                anyhow::ensure!(
                    total >= observed,
                    "QUOTA_CONVERSION_CALIBRATION_BELOW_OBSERVED"
                );
                Ok((total - observed).to_string())
            })
            .transpose()?
    } else {
        anyhow::ensure!(
            input.target_used_picos.is_none(),
            "QUOTA_CONVERSION_WRONG_CALIBRATION_UNIT"
        );
        input
            .target_used_tokens
            .map(|total| -> anyhow::Result<String> {
                anyhow::ensure!(
                    total >= observed_tokens,
                    "QUOTA_CONVERSION_CALIBRATION_BELOW_OBSERVED"
                );
                Ok((total - observed_tokens).to_string())
            })
            .transpose()?
    };
    if supplied.is_some() {
        anyhow::ensure!(
            input.reason.as_ref().is_some_and(|s| !s.trim().is_empty()),
            "QUOTA_CONVERSION_ADJUSTMENT_REASON_REQUIRED"
        );
    }
    let result = supplied.or(mapped);
    anyhow::ensure!(
        source_offset == "0" || result.is_some(),
        "QUOTA_CONVERSION_CALIBRATION_REQUIRED"
    );
    // Explicit zero also clears a stale adjustment left by a round trip.
    Ok(result)
}

fn source_value_adjustment(p: &ledger::conversion::Preview) -> anyhow::Result<Option<String>> {
    if p.non_token_basis.is_some() {
        Ok(Some(format!(
            "value:{}",
            digest(&(&p.source_adjustment, &p.non_token_basis))?
        )))
    } else {
        Ok(p.source_adjustment.clone())
    }
}

fn source_preview(
    dir: &Path,
    scope: &ledger::Scope,
    target: &ledger::Window,
    preview: &ledger::conversion::Preview,
    old: &ShareUserGrant,
    usage: &UsageStore,
    now: i64,
) -> anyhow::Result<ledger::conversion::Preview> {
    if old.policy.value_quota.is_none() {
        return Ok(preview.clone());
    }
    let w = window(old, now)?;
    if w == *target {
        return Ok(preview.clone());
    }
    let proof = proof(dir, scope, &w, usage, old, now)?;
    ledger::conversion::preview(dir, scope, &w, &proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn calibration_is_an_offset_from_preview_and_cannot_erase_observed_consumption() {
        let mut input = ShareUserQuotaConversion {
            credential: "signed".into(),
            target_used_picos: Some("50000000000000".into()),
            target_used_tokens: None,
            reason: Some("Audited".into()),
        };
        let offset = calibration_offset(
            &input,
            "usdEquivalent",
            "1",
            None,
            "42000000000000",
            30_000_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(offset, "8000000000000");
        assert_eq!(
            56_000_000_000_000_i128 + offset.parse::<i128>().unwrap(),
            64_000_000_000_000
        );
        input.target_used_picos = Some("1".into());
        assert!(
            calibration_offset(&input, "usdEquivalent", "1", None, "42000000000000", 0).is_err()
        );
        input.target_used_picos = None;
        input.target_used_tokens = Some(29_000_000);
        assert!(calibration_offset(&input, "tokens", "1", None, "0", 30_000_000).is_err());
        input.target_used_tokens = None;
        assert!(calibration_offset(&input, "tokens", "1", None, "0", 30_000_000).is_err());
        assert_eq!(
            calibration_offset(&input, "tokens", "1", Some("5".into()), "0", 30_000_000).unwrap(),
            Some("5".into())
        );
    }
    #[test]
    fn credentials_bind_scope_policy_window_revision_and_expiry_without_signing_live_watermarks() {
        let directory = std::env::temp_dir().join(format!(
            "quota-ticket-{}",
            crate::domain::sharing::quota_cycle::opaque_id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut config = ServerConfig::load_or_default(&directory).unwrap();
        config.router.url = Some("https://router.test".into());
        let mut identity = crate::clients::router::client::generate_identity_without_installation();
        identity.installation_id = "installation".into();
        config.router.identity = Some(identity);
        let ticket = Ticket {
            scope: scope(&config, "share", "a@example.test").unwrap(),
            config_revision: 7,
            grant_revision: 3,
            source_policy: ShareUserPolicy::default(),
            target_policy: ShareUserPolicy::default(),
            window_id: "Day:0:86400000".into(),
            metered_from_ms: 0,
            adjustment_fingerprint: "adjustments".into(),
            pricing_basis: Some("migration".into()),
            active_pricing_revision: "active".into(),
            window_ends_at_ms: Some(86400000),
            account_cycle_revision: None,
            observed_picos: "42000000000000".into(),
            observed_tokens: 30_000_000,
            non_token_basis: None,
            expires_at_ms: 123456,
        };
        let signed = encode(&config, &ticket).unwrap();
        assert_eq!(decode(&config, &signed).unwrap(), ticket);
        let (raw, sig) = signed.split_once('.').unwrap();
        for changed in [
            serde_json::json!({"scope":{"shareId":"other"}}),
            serde_json::json!({"configRevision":8}),
            serde_json::json!({"expiresAtMs":i64::MAX}),
            serde_json::json!({"windowId":"Day:86400000:172800000"}),
            serde_json::json!({"activePricingRevision":"new"}),
        ] {
            let mut payload = serde_json::from_slice::<Value>(&hex::decode(raw).unwrap()).unwrap();
            for (key, value) in changed.as_object().unwrap() {
                payload[key] = value.clone();
            }
            assert!(decode(
                &config,
                &format!(
                    "{}.{sig}",
                    hex::encode(serde_json::to_vec(&payload).unwrap())
                )
            )
            .is_err());
        }
        config.router.identity.as_mut().unwrap().private_key = "different-key".into();
        assert!(decode(&config, &signed).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
