//! Durable request attribution for one-time Market trials. Receipts survive
//! grant replacement and are never inferred from the bounded recent log view.
use super::{
    router_contract::{ShareMarketPhase, ShareUserUsage},
    shares::{Share, SharePatchError},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// Keep exact revisions longer than the 32-day Usage detail/revision window.
// Older final receipts fold into lifetime totals; trial counters never expire.
pub const RECEIPT_RETENTION_MS: i64 = 35 * 86_400_000;
pub const MAX_MARKET_REQUESTS: usize = 1_000_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompactedUsage {
    pub trial_tokens: u64,
    pub trial_requests: u64,
    pub paid_tokens: u64,
    pub paid_requests: u64,
}

pub fn uncertain(share: &Share, id: &str) -> bool {
    share.market_entitlements.get(id).is_some_and(|state| {
        share
            .market_entitlements
            .values()
            .any(|other| other.trial_scope_id == state.trial_scope_id && other.usage_uncertain)
    })
}

pub fn exhausted(share: &Share, id: &str) -> bool {
    let Some(state) = share
        .market_entitlements
        .get(id)
        .filter(|s| s.phase == ShareMarketPhase::Trial)
    else {
        return false;
    };
    state
        .trial_token_limit
        .is_some_and(|limit| state.trial_tokens_used >= limit)
        || state.trial_token_allowance.is_some_and(|limit| {
            share
                .market_entitlements
                .values()
                .filter(|e| e.trial_scope_id == state.trial_scope_id)
                .fold(0u64, |sum, e| sum.saturating_add(e.trial_tokens_used))
                >= limit
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MarketRequestReceipt {
    pub entitlement_id: String,
    pub email: String,
    pub phase: ShareMarketPhase,
    pub started_at_ms: i64,
    pub tokens: u64,
    pub revision: u64,
    #[serde(default = "default_count_request")]
    pub count_request: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_request_id: Option<String>,
}

fn default_count_request() -> bool {
    true
}

pub fn admit(
    share: &mut Share,
    email: &str,
    request_id: &str,
    started_at_ms: i64,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        !request_id.is_empty() && request_id.len() <= 128 && started_at_ms > 0,
        "invalid market request identity"
    );
    let email = email.trim().to_ascii_lowercase();
    let Some(grant) = share.user_grants.get(&email).filter(|g| g.active) else {
        return Ok(false);
    };
    let Some(id) = grant.entitlement_id.as_ref() else {
        return Ok(false);
    };
    let Some(entitlement) = share.market_entitlements.get(id).filter(|e| e.active) else {
        return Ok(false);
    };
    anyhow::ensure!(
        !uncertain(share, id),
        "MARKET_USAGE_UNAVAILABLE: unresolved metering requires cleanup"
    );
    if let Some(saved) = share.market_requests.get(request_id) {
        anyhow::ensure!(
            saved.entitlement_id == *id && saved.email == email && saved.phase == entitlement.phase,
            "market request identity or phase changed"
        );
        return Ok(true);
    }
    let id = id.clone();
    let phase = entitlement.phase;
    compact(share, started_at_ms);
    anyhow::ensure!(
        started_at_ms >= share.market_receipts_before_ms,
        "market request precedes retained revision window"
    );
    anyhow::ensure!(
        share.market_requests.len() < MAX_MARKET_REQUESTS,
        "MARKET_USAGE_CAPACITY: unresolved or recent receipts must be retained"
    );
    anyhow::ensure!(
        !exhausted(share, &id),
        "MARKET_TRIAL_EXHAUSTED: trial conversion is pending"
    );
    share.market_requests.insert(
        request_id.into(),
        MarketRequestReceipt {
            entitlement_id: id,
            email,
            phase,
            started_at_ms,
            tokens: 0,
            revision: 0,
            count_request: true,
            parent_request_id: None,
        },
    );
    Ok(true)
}

/// Called before committing the corresponding usage journal record. Failure to
/// save fails the write; a journal failure after this point can only over-count,
/// never reset the free budget. Retrying the same revision is idempotent.
pub fn record(
    share: &mut Share,
    log: &crate::domain::usage::store::UsageLog,
) -> Result<bool, String> {
    if !share.market_requests.contains_key(&log.request_id)
        && log.record_kind == crate::domain::usage::store::UsageRecordKind::InternalSupplemental
    {
        if let Some(parent) = log
            .parent_request_id
            .as_deref()
            .and_then(|id| share.market_requests.get(id))
            .cloned()
        {
            let mut receipt = parent;
            receipt.tokens = 0;
            receipt.revision = 0;
            receipt.count_request = false;
            receipt.parent_request_id.clone_from(&log.parent_request_id);
            share
                .market_requests
                .insert(log.request_id.clone(), receipt);
        }
    }
    let Some(receipt) = share.market_requests.get_mut(&log.request_id) else {
        return Ok(false);
    };
    if (!receipt.count_request && receipt.parent_request_id != log.parent_request_id)
        || receipt.count_request
            != (log.record_kind == crate::domain::usage::store::UsageRecordKind::UserInference)
        || log.share_id.as_deref() != Some(share.id.as_str())
        || !log
            .user_email
            .as_deref()
            .is_some_and(|e| e.eq_ignore_ascii_case(&receipt.email))
    {
        return Err("market usage identity changed".into());
    }
    // Streaming progress is provisional and may already carry partial Tokens.
    // The admission remains revision zero until its final receipt arrives;
    // a crash before that is handled by recovery, never as zero final usage.
    if log.usage_state == crate::domain::usage::store::UsageState::Pending {
        return Ok(false);
    }
    let tokens = log.quota_tokens();
    let revision = log.usage_revision.max(1);
    if revision < receipt.revision {
        return Ok(false);
    }
    if revision == receipt.revision {
        if tokens != receipt.tokens {
            return Err("market usage revision changed payload".into());
        }
        return Ok(false);
    }
    if tokens < receipt.tokens {
        return Err("market usage cannot erase confirmed Tokens".into());
    }
    let count_request = receipt.revision == 0 && receipt.count_request;
    let delta = tokens - receipt.tokens;
    receipt.tokens = tokens;
    receipt.revision = revision;
    // Unknown trial consumption must never reopen a free budget. Ordinary
    // paid-request cancellation still follows existing paid quota semantics;
    // it must not turn an otherwise healthy paid rental into trial cleanup.
    let uncertain = receipt.phase == ShareMarketPhase::Trial
        && (matches!(
            log.usage_state,
            crate::domain::usage::store::UsageState::Interrupted
                | crate::domain::usage::store::UsageState::ParseError
        ) || (log.usage_state == crate::domain::usage::store::UsageState::Missing
            && log.status_code >= 200
            && log.status_code < 300));
    let entitlement = share
        .market_entitlements
        .get_mut(&receipt.entitlement_id)
        .ok_or("market entitlement missing")?;
    entitlement.usage_uncertain |= uncertain;
    if receipt.phase == ShareMarketPhase::Trial {
        entitlement.trial_tokens_used = entitlement
            .trial_tokens_used
            .checked_add(delta)
            .ok_or("market Tokens overflow")?;
        entitlement.usage_revision = entitlement
            .usage_revision
            .checked_add(1)
            .ok_or("market usage revision overflow")?;
    }
    if let Some(grant) = share.user_grants.get_mut(&receipt.email).filter(|g| {
        g.entitlement_id.as_deref() == Some(receipt.entitlement_id.as_str())
            && entitlement.phase == receipt.phase
    }) {
        grant
            .usage
            .record_market_delta(&grant.policy, delta, receipt.started_at_ms, count_request);
        grant.usage_rebase = None;
        grant.usage_quota = None;
    }
    Ok(true)
}

/// The paid user quota starts fresh, without deleting the historical trial
/// counters. Late trial completions remain attached to their original phase.
pub fn project(share: &mut Share) {
    for grant in share.user_grants.values_mut() {
        let Some(id) = grant.entitlement_id.as_ref() else {
            continue;
        };
        let Some(entitlement) = share.market_entitlements.get(id) else {
            continue;
        };
        let mut receipts: Vec<_> = share
            .market_requests
            .values()
            .filter(|r| r.entitlement_id == *id && r.phase == entitlement.phase && r.revision > 0)
            .collect();
        receipts.sort_by_key(|r| r.started_at_ms);
        let mut usage = ShareUserUsage::default();
        if let Some(compacted) = share.market_compacted_usage.get(id) {
            let (tokens, requests) = if entitlement.phase == ShareMarketPhase::Trial {
                (compacted.trial_tokens, compacted.trial_requests)
            } else {
                (compacted.paid_tokens, compacted.paid_requests)
            };
            usage.lifetime.tokens_used = tokens;
            usage.lifetime.requests_count = requests;
        }
        for receipt in receipts {
            if receipt.count_request {
                usage.record_for_policy(&grant.policy, receipt.tokens, receipt.started_at_ms);
            } else {
                usage.record_supplemental_for_policy(
                    &grant.policy,
                    receipt.tokens,
                    receipt.started_at_ms,
                );
            }
        }
        grant.usage = usage;
        grant.usage_rebase = None;
        grant.usage_quota = None;
    }
}

/// Only the real process startup (or a backup restore), never a snapshot read,
/// may declare in-flight receipts interrupted. Unknown usage cannot become a
/// fresh zero budget after restart. Router receives this flag and revokes safely.
pub fn recover(
    share: &mut Share,
    usage: &crate::domain::usage::store::UsageStore,
    restored: bool,
) -> Result<bool, String> {
    let mut changed = false;
    let pending: Vec<_> = share
        .market_requests
        .iter()
        .filter(|(_, r)| r.revision == 0)
        .map(|(id, _)| id.clone())
        .collect();
    for id in pending {
        if let Some(log) = usage.logs.iter().find(|l| {
            l.request_id == id && l.usage_state != crate::domain::usage::store::UsageState::Pending
        }) {
            changed |= record(share, log)?;
        }
        let receipt = &share.market_requests[&id];
        if receipt.revision == 0 && receipt.phase == ShareMarketPhase::Trial {
            let state = share
                .market_entitlements
                .get_mut(&receipt.entitlement_id)
                .ok_or("market entitlement missing during recovery")?;
            changed |= !state.usage_uncertain;
            state.usage_uncertain = true;
        }
    }
    if restored {
        for state in share.market_entitlements.values_mut().filter(|s| s.active) {
            changed |= !state.usage_uncertain;
            state.usage_uncertain = true;
        }
    }
    Ok(changed)
}

fn compact(share: &mut Share, at: i64) {
    let cutoff = at.saturating_sub(RECEIPT_RETENTION_MS).max(0);
    // Bound full scans to once per day, rather than scanning on every admission.
    if cutoff < share.market_receipts_before_ms.saturating_add(86_400_000) {
        return;
    }
    let mut retained = BTreeMap::new();
    for (id, receipt) in std::mem::take(&mut share.market_requests) {
        if receipt.started_at_ms >= cutoff || receipt.revision == 0 {
            retained.insert(id, receipt);
            continue;
        }
        let totals = share
            .market_compacted_usage
            .entry(receipt.entitlement_id)
            .or_default();
        let (tokens, requests) = if receipt.phase == ShareMarketPhase::Trial {
            (&mut totals.trial_tokens, &mut totals.trial_requests)
        } else {
            (&mut totals.paid_tokens, &mut totals.paid_requests)
        };
        *tokens = tokens.saturating_add(receipt.tokens);
        *requests = requests.saturating_add(u64::from(receipt.count_request));
    }
    share.market_requests = retained;
    share.market_receipts_before_ms = cutoff;
}

pub fn validate_transition(
    share: &Share,
    id: &str,
    email: &str,
    phase: ShareMarketPhase,
    token_limit: Option<u64>,
) -> Result<(), SharePatchError> {
    if token_limit == Some(0) || (phase == ShareMarketPhase::Paid && token_limit.is_some()) {
        return Err(SharePatchError::Invalid(
            "trial Token cap must be positive or unlimited, and belongs only to the trial phase"
                .into(),
        ));
    }
    if let Some(existing) = share.market_entitlements.get(id) {
        if !existing.email.eq_ignore_ascii_case(email)
            || (existing.phase == ShareMarketPhase::Paid && phase == ShareMarketPhase::Trial)
            || (phase == ShareMarketPhase::Trial && existing.trial_token_limit != token_limit)
        {
            return Err(SharePatchError::Invalid(
                "market entitlement phase or budget cannot be reset".into(),
            ));
        }
    }
    Ok(())
}
