//! Server-owned Share quota orchestration; no wallet or price-source access.
//!
//! USD-equivalent quotas are a request-level soft cap: prefer a small,
//! bounded overshoot and never falsely lock a user out. Only grants with an
//! active `policy.valueQuota` or a `pendingValueQuota` are metered; Token
//! grants carry no USD metering (legacy bindings are left untouched).
use super::*;
use crate::domain::sharing::router_contract::{
    PendingValueQuota, ShareGrantManager, ShareTokenPeriod, ShareUserGrant, ShareUserPolicy,
};
use crate::domain::usage::value_pricing::{
    PricingManifest, RiskEstimate, ValueError, ValueMeteringBinding, ValueQuotaRejection,
    ValueQuotaRejectionKind, ValueQuotaStatus,
};
use crate::repository::server_sqlite::{self, value_quota as ledger};
use std::sync::atomic::{AtomicU8, Ordering as AtomicOrdering};

/// Error prefixes the admin API maps to typed 409/400 answers (never 500).
pub(crate) const VALUE_QUOTA_NOT_ACTIVE: &str = "VALUE_QUOTA_NOT_ACTIVE";
pub(crate) const VALUE_QUOTA_TARGET_INVALID: &str = "VALUE_QUOTA_TARGET_INVALID";
/// Preview check advertising that managed (Share Market) grants may carry a
/// USD-equivalent quota on this build.
pub(crate) const MARKET_SEAT_QUOTA_CHECK: &str = "market_seat_quota";
/// Preview check advertising that a Share Market one-time trial may be
/// budgeted in the seat's USD-equivalent unit (`trialValue*UsdMicros`).
pub(crate) const MARKET_TRIAL_VALUE_CHECK: &str = "market_trial_value";
/// Margin added to the longest provider request timeout before a pending
/// attempt counts as stuck (sweeper and `set` reconcile share it).
const STALE_ATTEMPT_MARGIN_MS: i64 = 5 * 60 * 1000;
mod tariff;
fn scope(config: &ServerConfig, share_id: &str, email: &str) -> anyhow::Result<ledger::Scope> {
    let identity = config
        .registered_router_identity()
        .context("VALUE_QUOTA_ROUTER_BINDING_REQUIRED")?;
    Ok(ledger::Scope {
        router_url: config
            .router_api_base()
            .context("VALUE_QUOTA_ROUTER_BINDING_REQUIRED")?
            .into(),
        installation_id: identity.installation_id.clone(),
        share_id: share_id.into(),
        email: email.trim().to_ascii_lowercase(),
        entitlement_id: None,
    })
}

fn grant_scope(
    config: &ServerConfig,
    share_id: &str,
    email: &str,
    grant: &ShareUserGrant,
) -> anyhow::Result<ledger::Scope> {
    let mut identity = scope(config, share_id, email)?;
    if grant.manager == ShareGrantManager::RouterShareMarket {
        identity.entitlement_id = Some(
            grant
                .entitlement_id
                .clone()
                .filter(|id| !id.is_empty())
                .context("VALUE_QUOTA_ENTITLEMENT_REQUIRED")?,
        );
    }
    Ok(identity)
}
/// Persisted window ids embed this label; it must stay byte-identical to the
/// historical `{:?}` rendering of the period.
fn period_label(period: ShareTokenPeriod) -> &'static str {
    match period {
        ShareTokenPeriod::Lifetime => "Lifetime",
        ShareTokenPeriod::Day => "Day",
        ShareTokenPeriod::Week => "Week",
        ShareTokenPeriod::SevenDays => "SevenDays",
        ShareTokenPeriod::CalendarMonth => "CalendarMonth",
        ShareTokenPeriod::ThirtyDays => "ThirtyDays",
        ShareTokenPeriod::AccountReset => "AccountReset",
    }
}
fn window(grant: &ShareUserGrant, now: i64) -> anyhow::Result<ledger::Window> {
    let binding = grant
        .value_metering
        .as_ref()
        .context("VALUE_QUOTA_BINDING_UNAVAILABLE")?;
    if grant.policy.token_period == ShareTokenPeriod::AccountReset {
        let cycle = grant
            .account_quota_cycle
            .as_ref()
            .context("VALUE_QUOTA_ACCOUNT_CYCLE_UNAVAILABLE")?;
        anyhow::ensure!(
            cycle.starts_at_ms <= now && cycle.ends_at_ms.is_some_and(|end| now < end),
            "VALUE_QUOTA_ACCOUNT_CYCLE_UNAVAILABLE"
        );
        return Ok(ledger::Window {
            id: format!(
                "account:{}:{}:{}:{}",
                cycle.source_ref, cycle.source_epoch, cycle.bucket_id, cycle.cycle_id
            ),
            starts_at_ms: cycle.starts_at_ms,
            ends_at_ms: cycle.ends_at_ms,
            metered_from_ms: binding.value_metered_from_ms,
        });
    }
    let period = crate::domain::sharing::token_period::token_period_window(&grant.policy, now)
        .map_err(anyhow::Error::msg)?;
    Ok(ledger::Window {
        id: format!(
            "{}:{}:{}",
            period_label(grant.policy.token_period),
            period.starts_at_ms.unwrap_or(0),
            period.ends_at_ms.unwrap_or(0)
        ),
        starts_at_ms: period.starts_at_ms.unwrap_or(0),
        ends_at_ms: period.ends_at_ms,
        metered_from_ms: binding.value_metered_from_ms,
    })
}
/// Bindings that can carry traffic. A bound but disabled app (often a
/// Provider Bundle surface without credentials) serves no request, so it
/// neither proves nor blocks USD metering; every request still checks its
/// own runtime in the forwarder.
fn enabled_bindings(
    share: &Share,
) -> impl Iterator<Item = &crate::domain::sharing::shares::ShareBinding> {
    share
        .bindings
        .iter()
        .filter(|binding| crate::domain::sharing::shares::share_app_api_enabled(share, binding.app))
}
fn verified_text_bindings(share: &Share, providers: &ProviderStore) -> bool {
    enabled_bindings(share).next().is_some()
        && enabled_bindings(share).all(|binding| {
            providers
                .runtime_plan(binding.app, &binding.provider_id)
                .is_some_and(|plan| {
                    crate::domain::usage::value_pricing::supports_value_text_runtime(
                        plan.profile_id.as_str(),
                        plan.driver_id.as_str(),
                        plan.coding_plan.is_some(),
                    )
                })
        })
}
/// Shared by save and preview. Without shadow history:
/// - same USD window (only the limit changed): immediate, origin kept;
/// - lifetime / accountReset: immediate, metered from now (or the existing
///   active USD origin, so editing never resets consumption);
/// - natural periods: immediate from the window start when the Server proved
///   the user has no recorded Share request in the current window (nothing
///   to miss, so the window is exact), otherwise scheduled for the start of
///   the next window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activation {
    Immediate {
        metered_from_ms: i64,
        reason_code: &'static str,
        /// Coverage must start at the window start (proved-empty history).
        cover_window: bool,
    },
    NextWindow,
}
/// Current window start of a natural-period policy when `usage` proves that
/// this Share user has no recorded request (and no quota tokens) in
/// [window start, now]. A newly added grant has no history by definition.
/// Without a usage snapshot nothing can be proved.
fn proven_empty_window_start(
    usage: Option<&crate::domain::usage::store::UsageStore>,
    share_id: &str,
    email: &str,
    new_grant: bool,
    policy: &ShareUserPolicy,
    now: i64,
) -> Option<i64> {
    let start = crate::domain::sharing::token_period::token_period_window(policy, now)
        .ok()?
        .starts_at_ms
        .filter(|start| *start <= now)?;
    if new_grant {
        return Some(start);
    }
    let (tokens, requests) =
        usage?.share_user_quota_usage(share_id, email, start, now.saturating_add(1));
    (tokens == 0 && requests == 0).then_some(start)
}
fn activation_rule(
    old: Option<&ShareUserGrant>,
    policy: &ShareUserPolicy,
    now: i64,
    empty_window_start: Option<i64>,
) -> Activation {
    let prior_active = old
        .filter(|g| g.active && g.policy.value_quota.is_some())
        .and_then(|g| g.value_metering.as_ref().map(|b| (g, b)));
    if let Some((g, binding)) = prior_active {
        if g.policy.token_period == policy.token_period
            && g.policy.token_period_anchor_at_ms == policy.token_period_anchor_at_ms
        {
            return Activation::Immediate {
                metered_from_ms: binding.value_metered_from_ms,
                reason_code: "ready",
                cover_window: false,
            };
        }
    }
    if matches!(
        policy.token_period,
        ShareTokenPeriod::Lifetime | ShareTokenPeriod::AccountReset
    ) {
        return match prior_active {
            Some((_, binding)) => Activation::Immediate {
                metered_from_ms: binding.value_metered_from_ms,
                reason_code: "ready",
                cover_window: false,
            },
            None => Activation::Immediate {
                metered_from_ms: now,
                reason_code: "metered_from_now",
                cover_window: false,
            },
        };
    }
    match empty_window_start {
        Some(start) => Activation::Immediate {
            metered_from_ms: start,
            reason_code: "ready",
            cover_window: true,
        },
        None => Activation::NextWindow,
    }
}
fn status_reason_code(error: &anyhow::Error) -> &'static str {
    let message = error.to_string();
    if message.contains("SNAPSHOT") || message == "price_unavailable" {
        "pricing_unavailable"
    } else if message.contains("ROUTER") {
        "router_upgrade_required"
    } else {
        "storage_unavailable"
    }
}
fn value_status(
    dir: &std::path::Path,
    config: &ServerConfig,
    share_id: &str,
    email: &str,
    grant: &ShareUserGrant,
    now: i64,
) -> anyhow::Result<ValueQuotaStatus> {
    let binding = grant
        .value_metering
        .as_ref()
        .context("VALUE_QUOTA_BINDING_UNAVAILABLE")?;
    let identity = grant_scope(config, share_id, email, grant)?;
    let w = window(grant, now)?;
    let revision =
        ledger::active_revision(dir, &identity)?.context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
    let status = ledger::status(dir, &identity, &w)?;
    Ok(ValueQuotaStatus {
        email: email.into(),
        policy_epoch: binding.quota_policy_epoch.clone(),
        window_id: w.id,
        window_starts_at_ms: w.starts_at_ms,
        resets_at_ms: w.ends_at_ms,
        metered_from_ms: binding.value_metered_from_ms,
        active_pricing_revision: revision,
        usage_pricing_revisions_digest: status.usage_pricing_revisions_digest,
        usage_watermark: status.usage_watermark,
        limit_usd_micros: grant
            .policy
            .value_quota
            .as_ref()
            .map(|q| q.limit_usd_micros.clone()),
        used_picos: status.used_picos,
        observed_picos: status.observed_picos,
        manual_offset_picos: status.manual_offset_picos,
        pending_risk_picos: status.pending_risk_picos,
        metering_status: metering_status_for(grant, binding, status.metering_status, now),
        synced_at_ms: now,
    })
}
/// The real metering status of the current window whenever a USD policy is
/// enforced (even if another USD policy is pending, so corrections stay
/// possible); `scheduled` only while the enforced policy is not USD (or not
/// yet effective) and a USD policy waits. Pending details are reported in
/// their own fields, never through this status.
fn metering_status_for(
    grant: &ShareUserGrant,
    binding: &ValueMeteringBinding,
    window_status: String,
    now: i64,
) -> String {
    let enforced = grant.policy.value_quota.is_some() && now >= binding.effective_from_ms;
    if window_status == "blocked" || enforced {
        return window_status;
    }
    if grant.pending_value_quota.is_some() || grant.policy.value_quota.is_some() {
        return "scheduled".into();
    }
    window_status
}
fn metered_grant(grant: &ShareUserGrant) -> bool {
    grant.active
        && grant.value_metering.is_some()
        && (grant.policy.value_quota.is_some() || grant.pending_value_quota.is_some())
}

const PHASE_NOT_SENT: u8 = 0;
const PHASE_SENT: u8 = 1;
const PHASE_UPSTREAM_ERROR: u8 = 2;
/// Terminal guard for one admitted USD attempt. Unless the attempt is settled
/// by its usage log first, dropping the guard (normal return, early `?`, or
/// a dropped request future) schedules an outcome settlement: never sent =>
/// $0, upstream error (error status, or an error/protocol failure before any
/// output on a 200 stream) => $0 unless usage is logged, otherwise outcome
/// unknown => reserved risk. Before a retry the forwarder calls
/// [`ValueAttemptGuard::settle_now`] instead, so the retry's admission never
/// races a still-pending row. Settlement is idempotent with usage-log
/// settlement.
pub(crate) struct ValueAttemptGuard {
    state: ServerState,
    request_id: String,
    attempt_id: String,
    phase: AtomicU8,
}
impl ValueAttemptGuard {
    /// The request is about to be written upstream.
    pub(crate) fn mark_sent(&self) {
        self.phase.store(PHASE_SENT, AtomicOrdering::Release);
    }
    /// The connection failed before the request was written.
    pub(crate) fn mark_not_sent(&self) {
        self.phase.store(PHASE_NOT_SENT, AtomicOrdering::Release);
    }
    /// Upstream response headers arrived.
    pub(crate) fn mark_upstream_status(&self, status: u16) {
        if status >= 400 {
            self.mark_upstream_error();
        }
    }
    /// Upstream failed before producing any output (e.g. an Anthropic
    /// `error` event, a semantic/protocol failure or a first-event read
    /// error on an HTTP 200 stream). Settles at $0 unless a usage log
    /// reports usage for this attempt.
    pub(crate) fn mark_upstream_error(&self) {
        self.phase
            .store(PHASE_UPSTREAM_ERROR, AtomicOrdering::Release);
    }
    fn outcome(&self) -> ledger::AttemptOutcome {
        match self.phase.load(AtomicOrdering::Acquire) {
            PHASE_NOT_SENT => ledger::AttemptOutcome::NotSent,
            PHASE_UPSTREAM_ERROR => ledger::AttemptOutcome::UpstreamError,
            _ => ledger::AttemptOutcome::Unknown,
        }
    }
    /// Settles the attempt now (awaited) and disarms the drop hook. Used
    /// before a retry so the next attempt's admission sees this row settled
    /// (no stale concurrency slot or reservation). Returns whether the
    /// attempt settles at $0 (never sent, or upstream error before output),
    /// which makes the retry admissible for a USD user.
    pub(crate) async fn settle_now(mut self) -> bool {
        let outcome = self.outcome();
        let request_id = std::mem::take(&mut self.request_id);
        let attempt_id = std::mem::take(&mut self.attempt_id);
        let state = self.state.clone();
        drop(self);
        state
            .settle_value_attempt_outcome(&request_id, &attempt_id, outcome)
            .await;
        outcome != ledger::AttemptOutcome::Unknown
    }
}
impl Drop for ValueAttemptGuard {
    fn drop(&mut self) {
        if self.request_id.is_empty() {
            // Disarmed by `settle_now`.
            return;
        }
        let outcome = self.outcome();
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(request_id = %self.request_id, "USD attempt guard dropped outside a runtime; the sweeper will settle it");
            return;
        };
        let state = self.state.clone();
        let request_id = std::mem::take(&mut self.request_id);
        let attempt_id = std::mem::take(&mut self.attempt_id);
        handle.spawn(async move {
            state
                .settle_value_attempt_outcome(&request_id, &attempt_id, outcome)
                .await;
        });
    }
}
/// Deletes the pre-v2 `share-value-quota-degraded.json` marker. Nothing
/// writes it any more (settlement failures block one (scope, window)), and a
/// stale copy must not keep USD activation disabled forever.
pub(super) fn remove_legacy_degraded_marker(config_dir: &std::path::Path) {
    let path = config_dir.join("share-value-quota-degraded.json");
    match std::fs::remove_file(&path) {
        Ok(()) => tracing::info!(
            path = %path.display(),
            "removed the legacy USD metering degraded marker; failures are now scoped to one user window"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%error, path = %path.display(),
            "legacy USD metering degraded marker could not be removed; it is ignored"),
    }
}

/// (request, Share, email, entitlement) of every retained Trial receipt that counts toward
/// a USD trial budget, or only of `only` (a direct lookup, no scan).
fn market_trial_value_requests(
    store: &crate::domain::sharing::shares::ShareStore,
    only: Option<&str>,
) -> Vec<(String, String, String, String)> {
    use crate::domain::sharing::market_trials::{value_receipt, value_receipts};
    let mut requests = Vec::new();
    for share in &store.shares {
        let mut push = |id: &str| {
            if let Some(receipt) = value_receipt(share, id) {
                requests.push((
                    id.to_owned(),
                    share.id.clone(),
                    receipt.email.clone(),
                    receipt.entitlement_id.clone(),
                ));
            }
        };
        match only {
            Some(id) => push(id),
            None => value_receipts(share).for_each(push),
        }
    }
    requests
}

fn read_market_trial_value(
    config_dir: &std::path::Path,
    requests: &[(String, String, String, String)],
) -> anyhow::Result<BTreeMap<String, i128>> {
    let requests: Vec<_> = requests
        .iter()
        .map(
            |(request_id, share_id, email, entitlement_id)| ledger::SettledRequest {
                request_id,
                share_id,
                email,
                entitlement_id: Some(entitlement_id),
            },
        )
        .collect();
    ledger::settled_request_picos(config_dir, &requests)
}

/// Whether any receipt would grow; avoids a Share save when nothing did.
fn market_trial_value_grows(
    store: &crate::domain::sharing::shares::ShareStore,
    totals: &BTreeMap<String, i128>,
) -> bool {
    use crate::domain::sharing::market_trials::value_receipt;
    totals.iter().any(|(id, picos)| {
        store.shares.iter().any(|share| {
            value_receipt(share, id).is_some_and(|receipt| {
                crate::domain::usage::value_pricing::unsigned_decimal(&receipt.value_picos)
                    .map_or(true, |seen| *picos > seen)
            })
        })
    })
}

fn apply_market_trial_value(
    store: &mut crate::domain::sharing::shares::ShareStore,
    totals: &BTreeMap<String, i128>,
) -> anyhow::Result<bool> {
    let mut changed = false;
    for share in &mut store.shares {
        for (id, picos) in totals {
            changed |= crate::domain::sharing::market_trials::record_value(share, id, *picos)
                .map_err(anyhow::Error::msg)?;
        }
    }
    Ok(changed)
}

/// Startup pass (before the state is published): re-derives USD trial usage
/// from the durable value ledger so a crash between a ledger settlement and
/// the Share save that mirrors it cannot under-count a trial budget.
pub(super) fn reconcile_market_trial_value_on_load(
    config_dir: &std::path::Path,
    store: &mut crate::domain::sharing::shares::ShareStore,
) -> anyhow::Result<bool> {
    if !server_sqlite::is_committed(config_dir)? {
        return Ok(false);
    }
    let requests = market_trial_value_requests(store, None);
    if requests.is_empty() {
        return Ok(false);
    }
    let totals = read_market_trial_value(config_dir, &requests)?;
    apply_market_trial_value(store, &totals)
}
fn rejection_reason(error: &anyhow::Error) -> &'static str {
    if error.is::<crate::domain::usage::value_pricing::PriceRejection>() {
        return "price_unavailable";
    }
    match error.downcast_ref::<ValueQuotaRejection>().map(|r| r.kind) {
        Some(ValueQuotaRejectionKind::Exhausted) => "exhausted",
        Some(ValueQuotaRejectionKind::Busy) => "busy",
        _ if error.downcast_ref::<ValueError>() == Some(&ValueError::PriceUnavailable) => {
            "price_unavailable"
        }
        _ => "quota_unavailable",
    }
}

impl ServerStateInner {
    #[cfg(test)]
    pub(crate) async fn configure_value_quota_router_for_test(&self, installation_id: &str) {
        let mut config = self.config_snapshot().await;
        config.router.url = Some("http://127.0.0.1:9".into());
        let mut identity = client::generate_identity_without_installation();
        identity.installation_id = installation_id.into();
        config.router.identity = Some(identity);
        self.replace_config(config.clone()).await.unwrap();
        self.record_router_quota_capability(
            &config,
            client::RouterShareCapabilities {
                share_contract_version: 10,
                capabilities: vec!["usdEquivalentQuotaV1".into()],
            },
        )
        .await;
    }
    pub(crate) async fn router_supports_cache_creation_ttl(&self) -> bool {
        let config = self.config_snapshot().await;
        self.router_quota_capability
            .read()
            .await
            .as_ref()
            .is_some_and(|(base, id, at, c)| {
                c.3 && at.elapsed() < Duration::from_secs(300)
                    && config.router_api_base() == Some(base.as_str())
                    && config
                        .registered_router_identity()
                        .is_some_and(|identity| identity.installation_id == *id)
            })
    }
    /// Owner calibration. `set` doubles as the window reconcile: it resolves
    /// stale pending attempts, rebuilds checkpoints and clears the scoped
    /// block and restore fence (see `ledger::apply_adjustments`).
    pub async fn apply_share_value_usage_edits(
        &self,
        share_id: &str,
        expected_revision: u64,
        edits: BTreeMap<String, crate::domain::usage::value_pricing::ValueUsageEdit>,
        actor: &str,
    ) -> anyhow::Result<()> {
        let config = self.config_snapshot().await;
        let _workspace = self.codex_workspace_rebind_transactions.lock().await;
        self.recover_pending_codex_workspace_rebind_transaction("Value quota correction")?;
        let _quota = self.share_quota_mutations.lock().await;
        let _commit = self.shares_persistence.gate.lock().await;
        let share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        anyhow::ensure!(
            share.config_revision == expected_revision,
            "VALUE_QUOTA_REVISION_CONFLICT"
        );
        let now = now_ms_i64();
        let mut inputs = Vec::new();
        for (email, edit) in &edits {
            let email = email.trim().to_ascii_lowercase();
            let grant = share
                .user_grants
                .get(&email)
                .filter(|g| g.active && g.policy.value_quota.is_some())
                .with_context(|| {
                    format!("{VALUE_QUOTA_NOT_ACTIVE}: active USD-equivalent grant required for {email}")
                })?;
            anyhow::ensure!(
                grant.manager != ShareGrantManager::RouterShareMarket,
                "{VALUE_QUOTA_TARGET_INVALID}: Share Market managed users are read-only"
            );
            let w = window(grant, now).map_err(|error| {
                anyhow::anyhow!(
                    "{VALUE_QUOTA_NOT_ACTIVE}: the current USD-equivalent window is unavailable ({error})"
                )
            })?;
            inputs.push((
                scope(&config, share_id, &email)?,
                w,
                grant.revision,
                edit.clone(),
            ));
        }
        let stale_before = self.value_quota_stale_before_ms(now).await;
        let config_dir = self.config_dir.clone();
        let actor = actor.to_owned();
        tokio::task::spawn_blocking(move || {
            let batch = inputs
                .iter()
                .map(|(scope, window, revision, edit)| ledger::Adjustment {
                    scope,
                    window,
                    grant_revision: *revision,
                    edit,
                })
                .collect::<Vec<_>>();
            ledger::apply_adjustments(&config_dir, &batch, &actor, now, stale_before)
        })
        .await
        .context("value quota adjustment task panicked")??;
        self.router_share_sync_wakeup.notify_one();
        Ok(())
    }
    /// Attempts started before this are stuck rather than live: the later of
    /// process start and now - (longest provider request timeout + 5 min).
    /// Shared by the sweeper and the `set` reconcile.
    pub(crate) async fn value_quota_stale_before_ms(&self, now: i64) -> i64 {
        let max_timeout_ms = {
            let providers = self.providers.read().await;
            providers
                .providers
                .iter()
                .filter_map(|stored| providers.runtime_plan(stored.app, &stored.provider.id))
                .map(|plan| plan.transport_policy.timeout_ms)
                .max()
                .unwrap_or(0)
        };
        let process_started_ms =
            now.saturating_sub(i64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(0));
        now.saturating_sub(i64::try_from(max_timeout_ms).unwrap_or(i64::MAX / 2))
            .saturating_sub(STALE_ATTEMPT_MARGIN_MS)
            .max(process_started_ms)
    }
    pub async fn preview_share_value_quota(
        &self,
        share_id: &str,
        expected_revision: u64,
        policies: BTreeMap<String, ShareUserPolicy>,
    ) -> anyhow::Result<Value> {
        let share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        anyhow::ensure!(
            share.config_revision == expected_revision,
            "VALUE_QUOTA_REVISION_CONFLICT"
        );
        let providers = self.providers.read().await.clone();
        let config = self.config_snapshot().await;
        let now = now_ms_i64();
        let tariff = scope(&config, share_id, "preview@local.invalid").and_then(|identity| {
            ledger::active_revision(&self.config_dir, &identity)?
                .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")
        });
        let authority = server_sqlite::read_marker(&self.config_dir)
            .ok()
            .flatten()
            .map(|marker| marker.authority);
        let storage = match authority {
            Some(server_sqlite::AuthorityState::Committed) => "committed",
            Some(server_sqlite::AuthorityState::Prepared) => "prepared",
            Some(server_sqlite::AuthorityState::ShadowVerified) => "shadow_verified",
            None => "unavailable",
        };
        let surfaces = enabled_bindings(&share)
            .map(|binding| {
                let plan = providers.runtime_plan(binding.app, &binding.provider_id);
                let ready = plan.as_ref().is_some_and(|plan| {
                    crate::domain::usage::value_pricing::supports_value_text_runtime(
                        plan.profile_id.as_str(),
                        plan.driver_id.as_str(),
                        plan.coding_plan.is_some(),
                    )
                });
                serde_json::json!({"code":"text_surface", "ready":ready,
                "app":binding.app, "profileId":plan.as_ref().map(|p|p.profile_id.as_str()),
                "driverId":plan.as_ref().map(|p|p.driver_id.as_str())})
            })
            .collect::<Vec<_>>();
        let router_negotiated = self.value_quota_router_negotiated().await;
        let mut tariff_check =
            serde_json::json!({"code":"acknowledged_tariff", "ready":tariff.is_ok()});
        // Pricing sync needs both a committed ledger and a negotiated Router,
        // so a missing tariff names the check it is waiting on.
        if tariff.is_err() {
            if storage != "committed" {
                tariff_check["blockedBy"] = "sqlite_authority".into();
            } else if !router_negotiated {
                tariff_check["blockedBy"] = "router_protocol".into();
            }
        }
        let mut checks = vec![
            serde_json::json!({"code":"router_protocol", "ready":router_negotiated}),
            serde_json::json!({"code":"sqlite_authority", "ready":storage == "committed", "state":storage}),
            serde_json::json!({"code":"metering_storage", "ready":!self.quota_usage_degraded.load(std::sync::atomic::Ordering::Acquire)}),
            tariff_check,
        ];
        if surfaces.is_empty() {
            checks.push(serde_json::json!({"code":"text_surface", "ready":false}));
        }
        checks.extend(surfaces);
        checks.extend(self.value_model_coverage(&share, &providers, &config, now)?);
        // Build capability, not readiness: this Server can meter Router
        // Share Market seats sold with a USD-equivalent quota. Always ready,
        // so it never changes `capabilityReady`; a Router treats its absence
        // as "this Server cannot run USD seats".
        checks.push(serde_json::json!({"code":MARKET_SEAT_QUOTA_CHECK, "ready":true}));
        // Same kind of build capability: one-time Market trials of a USD seat
        // may carry a USD-equivalent budget. Always ready.
        checks.push(serde_json::json!({"code":MARKET_TRIAL_VALUE_CHECK, "ready":true}));
        let capability_ready = checks.iter().all(|check| check["ready"] == true);
        let first_failed = checks
            .iter()
            .find(|check| check["ready"] != true)
            .and_then(|check| check["code"].as_str());
        let capability_code = match first_failed {
            Some("router_protocol") => "router_upgrade_required",
            Some("sqlite_authority" | "metering_storage") => "storage_unavailable",
            Some("acknowledged_tariff") => "pricing_unavailable",
            Some("model_price_coverage") => "pricing_unavailable",
            Some(_) => "unverified_surface",
            None => "ready",
        };
        let invalid_row = |email: &str, reason: String| serde_json::json!({"email":email,"ready":false,"reason":reason,"reasonCode":"invalid_policy"});
        if !capability_ready {
            // Validate the proposed policies without opening the ledger.
            let rows = policies
                .iter()
                .filter_map(|(email, policy)| {
                    crate::domain::sharing::token_period::validate_user_policy(policy, now)
                        .err()
                        .map(|error| invalid_row(email, error.to_string()))
                })
                .collect::<Vec<_>>();
            return Ok(
                serde_json::json!({"shareId":share_id,"configRevision":share.config_revision,
                "ready":false,"capabilityReady":false,"checks":checks,"reason":"capability_unavailable",
                "reasonCode":capability_code,"invalidRows":rows,
                "activePricingRevision":tariff.as_ref().ok(),
                "meteredFromConfirmationRequired":true,"softLimit":true,"maximumParallel":4,"rows":[]}),
            );
        }
        // The same proof the save path uses (see `activation_rule`); the
        // usage read lock is held only for this computation.
        let empty_windows: BTreeMap<String, Option<i64>> = {
            let usage = self.usage.read().await;
            policies
                .iter()
                .filter(|(_, policy)| policy.value_quota.is_some())
                .map(|(email, policy)| {
                    let email = email.trim().to_ascii_lowercase();
                    let new_grant = !share.user_grants.contains_key(&email);
                    let start = proven_empty_window_start(
                        Some(&usage),
                        share_id,
                        &email,
                        new_grant,
                        policy,
                        now,
                    );
                    (email, start)
                })
                .collect()
        };
        let mut rows = Vec::new();
        let mut first_blocker: Option<&'static str> = None;
        for (email, policy) in &policies {
            let email = email.trim().to_ascii_lowercase();
            if let Err(error) =
                crate::domain::sharing::token_period::validate_user_policy(policy, now)
            {
                first_blocker.get_or_insert("invalid_policy");
                rows.push(invalid_row(&email, error.to_string()));
                continue;
            }
            let previous = share.user_grants.get(&email);
            if previous
                .is_some_and(|g| g.active && g.manager == ShareGrantManager::RouterShareMarket)
            {
                first_blocker.get_or_insert("invalid_policy");
                rows.push(invalid_row(
                    &email,
                    "Share Market managed users are read-only".into(),
                ));
                continue;
            }
            let mut grant = previous.cloned().unwrap_or_default();
            grant.email = email.clone();
            grant.policy = policy.clone();
            if policy.value_quota.is_none() {
                rows.push(
                    serde_json::json!({"email":email,"effectiveFromMs":now,"ready":true,
                    "reason":null,"reasonCode":"ready"}),
                );
                continue;
            }
            let activation = activation_rule(
                previous,
                policy,
                now,
                empty_windows.get(&email).copied().flatten(),
            );
            let metered_from = match activation {
                Activation::Immediate {
                    metered_from_ms, ..
                } => metered_from_ms,
                Activation::NextWindow => now,
            };
            grant.value_metering = Some(ValueMeteringBinding {
                quota_policy_epoch: "preview-only".into(),
                value_metered_from_ms: metered_from,
                effective_from_ms: now,
                pricing_revision: tariff.as_ref().cloned().unwrap_or_default(),
            });
            let w = match window(&grant, now) {
                Ok(w) => w,
                Err(error) => {
                    first_blocker.get_or_insert("invalid_policy");
                    rows.push(invalid_row(&email, error.to_string()));
                    continue;
                }
            };
            let identity = scope(&config, share_id, &email)?;
            let (effective_from, mut code, reason) = match activation {
                Activation::Immediate { reason_code, .. } => (
                    now,
                    reason_code,
                    if reason_code == "metered_from_now" {
                        Some("USD-equivalent metering starts now; earlier Token usage is not converted")
                    } else {
                        None
                    },
                ),
                Activation::NextWindow => (
                    w.ends_at_ms.unwrap_or(now),
                    "scheduled_next_window",
                    Some("USD-equivalent metering starts at the beginning of the next window; the current policy stays enforced until then"),
                ),
            };
            let status = matches!(activation, Activation::Immediate { .. })
                .then(|| ledger::status(&self.config_dir, &identity, &w));
            if status
                .as_ref()
                .and_then(|s| s.as_ref().ok())
                .is_some_and(|s| s.metering_status == "blocked")
            {
                code = "blocked_needs_reconcile";
            }
            let row_ready = code != "blocked_needs_reconcile";
            if !row_ready {
                first_blocker.get_or_insert(code);
            }
            let used = status.as_ref().and_then(|s| s.as_ref().ok());
            rows.push(serde_json::json!({"email":email,"windowId":w.id,"windowStartsAtMs":w.starts_at_ms,"resetsAtMs":w.ends_at_ms,
                "meteredFromMs":metered_from,"effectiveFromMs":effective_from,"usedPicos":used.map(|s|s.used_picos.clone()),
                "usageWatermark":used.map(|s|s.usage_watermark),"ready":row_ready,
                "reason":if row_ready {reason} else {Some("This user's window is blocked; reconcile it with a usage set before enforcing")},
                "reasonCode":code}));
        }
        let ready = first_blocker.is_none();
        let top_code = first_blocker.unwrap_or_else(|| {
            rows.iter()
                .filter_map(|row| row["reasonCode"].as_str())
                .find(|code| *code != "ready")
                .and_then(|code| match code {
                    "scheduled_next_window" => Some("scheduled_next_window"),
                    "metered_from_now" => Some("metered_from_now"),
                    _ => None,
                })
                .unwrap_or("ready")
        });
        Ok(
            serde_json::json!({"shareId":share_id,"configRevision":share.config_revision,"ready":ready,
            "capabilityReady":capability_ready,"checks":checks,
            "reason":if ready {None} else {Some("Some policies are invalid or a user's window needs reconcile")},
            "reasonCode":top_code,
            "activePricingRevision":tariff.as_ref().ok(),"meteredFromConfirmationRequired":true,
            "softLimit":true,"maximumParallel":4,"rows":rows}),
        )
    }
    pub async fn router_supports_value_quotas(&self) -> bool {
        if !server_sqlite::is_committed(&self.config_dir).unwrap_or(false) {
            static REPORTED: std::sync::Once = std::sync::Once::new();
            REPORTED.call_once(|| {
                tracing::info!(
                    "USD-equivalent Share quotas stay unavailable until the Server SQLite authority is committed; pricing and watermark sync are skipped (Settings -> Storage)"
                )
            });
            return false;
        }
        self.value_quota_router_negotiated().await
    }
    async fn value_quota_router_negotiated(&self) -> bool {
        let config = self.config_snapshot().await;
        self.router_quota_capability
            .read()
            .await
            .as_ref()
            .is_some_and(|(base, id, at, supported)| {
                supported.2
                    && at.elapsed() < Duration::from_secs(300)
                    && config.router_api_base() == Some(base.as_str())
                    && config
                        .registered_router_identity()
                        .is_some_and(|identity| identity.installation_id == *id)
            })
    }
    pub(crate) async fn share_uses_value_quota(&self, share_id: &str, email: Option<&str>) -> bool {
        self.shares
            .read()
            .await
            .get(share_id)
            .and_then(|s| email.and_then(|e| s.user_grants.get(&e.to_ascii_lowercase())))
            .is_some_and(|g| g.active && g.policy.value_quota.is_some())
    }
    /// Called before persisting a candidate. Browser-supplied bindings are
    /// discarded by ShareStore; policy changes cannot reset collection.
    #[cfg(test)]
    pub(super) async fn prepare_value_bindings_at(
        &self,
        candidate: &mut ShareStore,
        previous: &ShareStore,
        now: i64,
    ) -> anyhow::Result<()> {
        let config = self.config_snapshot().await;
        let capable = self.router_supports_value_quotas().await;
        let providers = self.providers.read().await.clone();
        let usage = self.usage.read().await.clone();
        self.prepare_value_bindings_with_context(
            candidate,
            previous,
            now,
            &config,
            capable,
            &providers,
            Some(&usage),
        )
    }
    /// Only USD grants (active policy or pending) get or keep a binding.
    /// A user's own policy edit that cannot be prepared fails the save; any
    /// other grant error (e.g. a background commit activating a due policy)
    /// leaves that grant untouched and never aborts the whole ShareStore.
    /// `usage` (when available) lets a natural-period policy start now from
    /// a provably empty window; without it the policy is scheduled.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare_value_bindings_with_context(
        &self,
        candidate: &mut ShareStore,
        previous: &ShareStore,
        now: i64,
        config: &ServerConfig,
        capable: bool,
        providers: &ProviderStore,
        usage: Option<&crate::domain::usage::store::UsageStore>,
    ) -> anyhow::Result<()> {
        for share in &mut candidate.shares {
            let verified_surface = verified_text_bindings(share, providers);
            let old_share = previous.get(&share.id);
            let needs_prices = share.user_grants.iter().any(|(email, g)| {
                g.active
                    && g.policy.value_quota.is_some()
                    && old_share
                        .and_then(|s| s.user_grants.get(email))
                        .is_none_or(|old| {
                            !old.active
                                || old.policy != g.policy
                                || old.entitlement_id != g.entitlement_id
                                || (g.manager == ShareGrantManager::RouterShareMarket
                                    && g.value_metering.is_none())
                        })
            });
            let price_checks = if needs_prices && capable && verified_surface {
                self.value_model_coverage(share, providers, config, now)?
            } else {
                Vec::new()
            };
            let require_prices = || -> anyhow::Result<()> {
                if let Some(check) = price_checks.iter().find(|check| check["ready"] != true) {
                    anyhow::bail!(
                        "VALUE_QUOTA_PRICE_COVERAGE: {}",
                        serde_json::to_string(check)?
                    );
                }
                Ok(())
            };
            let mut changed = false;
            for (email, grant) in &mut share.user_grants {
                if grant.active
                    && grant.manager == ShareGrantManager::RouterShareMarket
                    && grant.policy.value_quota.is_some()
                {
                    let old = old_share.and_then(|s| s.user_grants.get(email));
                    let original = grant.clone();
                    // A managed operation re-inserts the grant without a
                    // binding; that (or a policy change) is the edit case
                    // and must fail the save, so the Router sees the
                    // operation fail and an active managed USD grant is never
                    // persisted unbound. A background save of an already
                    // bound grant never aborts the store.
                    let managed_edit = grant.value_metering.is_none()
                        || old.is_none_or(|g| {
                            !g.active
                                || g.policy != grant.policy
                                || g.entitlement_id != grant.entitlement_id
                        });
                    if managed_edit {
                        require_prices()?;
                    }
                    match self.prepare_managed_value_grant(
                        &share.id,
                        email,
                        grant,
                        old,
                        now,
                        config,
                        capable,
                        verified_surface,
                    ) {
                        Ok(grant_changed) => changed |= grant_changed,
                        Err(error) if managed_edit => return Err(error),
                        Err(error) => {
                            *grant = original;
                            tracing::warn!(%error, share_id = %share.id, "USD quota preparation skipped for one managed grant; its stored state is unchanged");
                        }
                    }
                    continue;
                }
                if !grant.active
                    || grant.manager == ShareGrantManager::RouterShareMarket
                    || (grant.policy.value_quota.is_none() && grant.pending_value_quota.is_none())
                {
                    continue;
                }
                let old = old_share.and_then(|s| s.user_grants.get(email));
                let original = grant.clone();
                let requested = grant.policy.clone();
                let user_edit = old.is_none_or(|g| {
                    (!g.active || g.policy != requested)
                        && g.pending_value_quota.as_ref().map(|p| &p.policy) != Some(&requested)
                });
                if user_edit && grant.policy.value_quota.is_some() {
                    require_prices()?;
                }
                match self.prepare_value_grant(
                    &share.id,
                    email,
                    grant,
                    old,
                    now,
                    config,
                    capable,
                    verified_surface,
                    usage,
                ) {
                    Ok(grant_changed) => changed |= grant_changed,
                    Err(error) if user_edit => return Err(error),
                    Err(error) => {
                        *grant = original;
                        tracing::warn!(%error, share_id = %share.id, "USD quota preparation skipped for one grant; its stored state is unchanged");
                    }
                }
            }
            if changed && old_share.is_none_or(|old| share.config_revision <= old.config_revision) {
                share.config_revision = share.config_revision.saturating_add(1).max(1);
            }
        }
        Ok(())
    }
    /// Router Share Market seat with a USD-equivalent quota. Metering is per
    /// contract (entitlement): the first USD upsert of a contract starts a
    /// binding metered from now. The ledger scope includes entitlementId, so
    /// old requests cannot leak into a new contract even at the same millisecond.
    /// Later upserts of the same entitlement (trial -> paid, suspend/resume,
    /// app-scope or integrity re-upserts) reuse that binding; only a changed
    /// amount, period or anchor starts a new policy epoch. Never scheduled,
    /// never back-dated to the window start, never calibrated by hand.
    #[allow(clippy::too_many_arguments)]
    fn prepare_managed_value_grant(
        &self,
        share_id: &str,
        email: &str,
        grant: &mut ShareUserGrant,
        old: Option<&ShareUserGrant>,
        now: i64,
        config: &ServerConfig,
        capable: bool,
        verified_surface: bool,
    ) -> anyhow::Result<bool> {
        let original = grant.clone();
        grant.pending_value_quota = None;
        if original.value_metering.is_none() {
            anyhow::ensure!(verified_surface, "VALUE_QUOTA_UNSUPPORTED_PROVIDER_SURFACE");
            anyhow::ensure!(
                capable,
                "VALUE_QUOTA_ROUTER_UPGRADE_OR_COMMITTED_STORAGE_REQUIRED"
            );
            let identity = grant_scope(config, share_id, email, grant)?;
            ledger::active_revision(&self.config_dir, &identity)?
                .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
        }
        let same_contract = old.filter(|g| {
            g.manager == ShareGrantManager::RouterShareMarket
                && g.entitlement_id.is_some()
                && g.entitlement_id == grant.entitlement_id
                && g.policy.value_quota.is_some()
        });
        let reusable = same_contract.and_then(|g| g.value_metering.clone().map(|b| (g, b)));
        let policy_changed = old.is_none_or(|g| {
            !g.active || g.policy != grant.policy || g.entitlement_id != grant.entitlement_id
        });
        if !policy_changed {
            if grant.value_metering.is_some() {
                // Background save of an unchanged, already bound seat.
                return Ok(*grant != original);
            }
            if let Some((_, mut binding)) = reusable.clone() {
                // Re-upsert of the same contract and policy: keep the binding
                // and refresh the pricing revision when it can be read.
                if let Some(revision) = scope(config, share_id, email)
                    .and_then(|identity| ledger::active_revision(&self.config_dir, &identity))
                    .ok()
                    .flatten()
                {
                    binding.pricing_revision = revision;
                }
                grant.value_metering = Some(binding);
                return Ok(*grant != original);
            }
        }
        anyhow::ensure!(verified_surface, "VALUE_QUOTA_UNSUPPORTED_PROVIDER_SURFACE");
        anyhow::ensure!(
            capable,
            "VALUE_QUOTA_ROUTER_UPGRADE_OR_COMMITTED_STORAGE_REQUIRED"
        );
        let identity = grant_scope(config, share_id, email, grant)?;
        let revision = ledger::active_revision(&self.config_dir, &identity)?
            .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
        let mut binding = match reusable {
            Some((previous, mut binding)) => {
                if previous.policy.value_quota != grant.policy.value_quota
                    || previous.policy.token_period != grant.policy.token_period
                    || previous.policy.token_period_anchor_at_ms
                        != grant.policy.token_period_anchor_at_ms
                {
                    binding.quota_policy_epoch = crate::domain::sharing::quota_cycle::opaque_id();
                }
                binding
            }
            None => {
                ledger::begin_coverage(&self.config_dir, &identity, now)?;
                ValueMeteringBinding {
                    quota_policy_epoch: crate::domain::sharing::quota_cycle::opaque_id(),
                    value_metered_from_ms: now,
                    effective_from_ms: now,
                    pricing_revision: revision.clone(),
                }
            }
        };
        binding.pricing_revision = revision;
        grant.value_metering = Some(binding);
        // Validate that the window exists for the stored policy.
        window(grant, now)?;
        Ok(*grant != original)
    }
    #[allow(clippy::too_many_arguments)]
    fn prepare_value_grant(
        &self,
        share_id: &str,
        email: &str,
        grant: &mut ShareUserGrant,
        old: Option<&ShareUserGrant>,
        now: i64,
        config: &ServerConfig,
        capable: bool,
        verified_surface: bool,
        usage: Option<&crate::domain::usage::store::UsageStore>,
    ) -> anyhow::Result<bool> {
        let original = grant.clone();
        let requested = grant.policy.clone();
        let mut policy_changed = old.is_none_or(|g| g.policy != grant.policy || !g.active);
        let mut activating = None;
        // Keep the previous policy enforced until the scheduled activation.
        if let Some(pending) = grant.pending_value_quota.clone() {
            let background = old.is_some_and(|g| g.policy == requested);
            if now >= pending.effective_from_ms && (background || requested == pending.policy) {
                grant.policy = pending.policy.clone();
                grant.pending_value_quota = None;
                grant.revision = grant.revision.saturating_add(1);
                policy_changed = true;
                activating = Some(pending.effective_from_ms);
            } else if requested == pending.policy {
                grant.policy = old.map(|g| g.policy.clone()).unwrap_or_default();
                policy_changed = false;
            } else if !background {
                // The owner replaced the scheduled policy with a new one.
                grant.pending_value_quota = None;
            }
        }
        if grant.policy.value_quota.is_none() {
            if grant.value_metering.is_none() {
                grant.value_metering = old.and_then(|g| g.value_metering.clone());
            }
            return Ok(*grant != original);
        }
        if policy_changed && activating.is_none() {
            anyhow::ensure!(verified_surface, "VALUE_QUOTA_UNSUPPORTED_PROVIDER_SURFACE");
            anyhow::ensure!(
                capable,
                "VALUE_QUOTA_ROUTER_UPGRADE_OR_COMMITTED_STORAGE_REQUIRED"
            );
        }
        let stored = old.and_then(|g| g.value_metering.clone());
        if !policy_changed {
            if grant.value_metering.is_none() {
                grant.value_metering = stored.clone();
            }
            if grant.value_metering.is_some() {
                return Ok(*grant != original);
            }
        }
        let identity = scope(config, share_id, email)?;
        let revision = ledger::active_revision(&self.config_dir, &identity)?
            .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
        let prior_active = old
            .filter(|g| g.active && g.policy.value_quota.is_some())
            .and_then(|g| g.value_metering.clone());
        let reuse = prior_active.clone().or_else(|| {
            (activating.is_some() || !policy_changed)
                .then(|| stored.clone())
                .flatten()
        });
        let mut binding = match reuse {
            Some(binding) => binding,
            None => {
                ledger::begin_coverage(&self.config_dir, &identity, now)?;
                ValueMeteringBinding {
                    quota_policy_epoch: crate::domain::sharing::quota_cycle::opaque_id(),
                    value_metered_from_ms: now,
                    effective_from_ms: now,
                    pricing_revision: revision.clone(),
                }
            }
        };
        if policy_changed {
            binding.quota_policy_epoch = crate::domain::sharing::quota_cycle::opaque_id();
        }
        binding.pricing_revision = revision;
        if let Some(effective_from) = activating {
            binding.effective_from_ms = effective_from.min(now);
            grant.value_metering = Some(binding);
        } else if policy_changed {
            let empty_window = proven_empty_window_start(
                usage,
                share_id,
                email,
                old.is_none(),
                &grant.policy,
                now,
            );
            match activation_rule(old, &grant.policy, now, empty_window) {
                Activation::Immediate {
                    metered_from_ms,
                    cover_window,
                    ..
                } => {
                    if cover_window {
                        ledger::cover_from(&self.config_dir, &identity, metered_from_ms)?;
                    }
                    binding.value_metered_from_ms = metered_from_ms;
                    binding.effective_from_ms = now;
                    grant.value_metering = Some(binding);
                    // Validate that the window exists (e.g. an account cycle).
                    window(grant, now)?;
                }
                Activation::NextWindow => {
                    grant.value_metering = Some(binding.clone());
                    let effective = window(grant, now)?
                        .ends_at_ms
                        .context("VALUE_QUOTA_HISTORY_INCOMPLETE")?;
                    grant.pending_value_quota = Some(PendingValueQuota {
                        policy: grant.policy.clone(),
                        effective_from_ms: effective,
                        retain_access: old.is_some_and(|g| g.active),
                    });
                    binding.effective_from_ms = prior_active
                        .as_ref()
                        .map_or(effective, |b| b.effective_from_ms);
                    grant.policy = old.map(|g| g.policy.clone()).unwrap_or_default();
                    grant.usage = old.map(|g| g.usage.clone()).unwrap_or_default();
                    grant.usage_rebase = old.and_then(|g| g.usage_rebase.clone());
                    grant.usage_quota = old.and_then(|g| g.usage_quota);
                    grant.value_metering = Some(binding);
                }
            }
        } else {
            grant.value_metering = Some(binding);
        }
        Ok(*grant != original)
    }
    pub(crate) async fn activate_due_value_policies(
        &self,
        share_id: &str,
        now: i64,
    ) -> anyhow::Result<()> {
        let due = self.shares.read().await.get(share_id).is_some_and(|share| {
            share.user_grants.values().any(|grant| {
                grant.active
                    && grant
                        .pending_value_quota
                        .as_ref()
                        .is_some_and(|pending| pending.effective_from_ms <= now)
            })
        });
        if due {
            self.mutate_shares_immediate(|_| {}).await?;
        }
        Ok(())
    }
    fn known_revision_unsupported(&self, base: &str) -> bool {
        self.value_pricing_known_revision_unsupported
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(base)
    }
    pub(super) fn forget_value_pricing_known_revision_fallback(&self, base: &str) {
        if self
            .value_pricing_known_revision_unsupported
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(base)
        {
            tracing::info!(router = %base, "Router advertises knownRevision pricing fetch; leaving the {{}} fallback");
        }
    }
    async fn post_value_pricing_fetch(
        &self,
        http: &reqwest::Client,
        identity: &crate::domain::settings::config::RouterIdentity,
        base: &str,
        known: Option<&str>,
    ) -> anyhow::Result<reqwest::Response> {
        let request = client::signed_canonical_request(
            identity,
            "share_value_pricing_fetch",
            match known {
                Some(revision) => serde_json::json!({"knownRevision":revision}),
                None => serde_json::json!({}),
            },
        )?;
        Ok(http
            .post(format!(
                "{}/v1/shares/value-pricing/fetch",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(20))
            .json(&request)
            .send()
            .await?)
    }
    /// Sends the signed pricing ACK (`{"revision","storageSchema":3}`) and
    /// checks the echoed revision.
    async fn send_value_pricing_ack(
        http: &reqwest::Client,
        identity: &crate::domain::settings::config::RouterIdentity,
        base: &str,
        revision: &str,
    ) -> anyhow::Result<()> {
        let ack = client::signed_canonical_request(
            identity,
            "share_value_pricing_ack",
            serde_json::json!({"revision":revision,"storageSchema":3}),
        )?;
        let response: serde_json::Value = http
            .post(format!(
                "{}/v1/shares/value-pricing/ack",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(20))
            .json(&ack)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        anyhow::ensure!(
            response.get("revision").and_then(Value::as_str) == Some(revision),
            "value pricing ACK mismatch"
        );
        Ok(())
    }
    /// Recovery for a Router 409 `value_pricing_ack_missing`: re-sends the
    /// normal signed ACK for every listed revision installed locally.
    /// Revisions this installation never stored are logged and left alone
    /// (their grants keep their binding). Returns the number re-acknowledged.
    pub(super) async fn reacknowledge_value_pricing(
        &self,
        http: &reqwest::Client,
        config: &ServerConfig,
        revisions: &[String],
    ) -> usize {
        let (Some(identity), Some(base)) = (
            config.registered_router_identity(),
            config.router_api_base(),
        ) else {
            return 0;
        };
        let Ok(pricing_scope) = scope(config, "pricing", "pricing@local.invalid") else {
            return 0;
        };
        let mut acknowledged = 0;
        // The Router lists distinct revisions; a bounded count keeps a
        // malformed answer from fanning out into many requests.
        for revision in revisions.iter().take(16) {
            match ledger::snapshot_installed(&self.config_dir, &pricing_scope, revision) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::warn!(%revision, "Router reports a missing pricing ACK for a revision this Server never installed; the grant is left unchanged");
                    continue;
                }
                Err(error) => {
                    tracing::warn!(%error, %revision, "pricing ACK recovery could not read local snapshots");
                    continue;
                }
            }
            match Self::send_value_pricing_ack(http, identity, base, revision).await {
                Ok(()) => {
                    acknowledged += 1;
                    tracing::info!(%revision, "re-acknowledged value pricing revision after Router ACK loss");
                }
                Err(error) => {
                    tracing::warn!(%error, %revision, "value pricing re-ACK failed; descriptor sync will retry later")
                }
            }
        }
        acknowledged
    }
    /// Heartbeat pricing sync. The fetch names the newest installed revision
    /// (`{"knownRevision"}`; `{}` before any install or for a Router that
    /// rejects the key). `{"unchanged":true}` installs and writes nothing.
    /// A full response for an installed revision re-ACKs it (the Router may
    /// have restored its database). Future revisions are validated, stored
    /// and acknowledged on receipt and activated locally at `effectiveFrom`,
    /// independently of later heartbeats. Until then, or offline, execution
    /// uses the last locally acknowledged active snapshot.
    pub(super) async fn sync_share_value_pricing(
        &self,
        config: &ServerConfig,
    ) -> anyhow::Result<()> {
        if !self.router_supports_value_quotas().await {
            return Ok(());
        }
        let identity = config
            .registered_router_identity()
            .context("Router identity unavailable")?;
        let base = config.router_api_base().context("Router URL unavailable")?;
        let pricing_scope = scope(config, "pricing", "pricing@local.invalid")?;
        let active = ledger::active_revision(&self.config_dir, &pricing_scope)?;
        let pending = ledger::pending_tariff(&self.config_dir, &pricing_scope)?;
        let newest = pending
            .as_ref()
            .map(|pending| pending.revision.clone())
            .or_else(|| active.clone());
        let http = self.http_client().await;
        let mut known = newest
            .as_deref()
            .filter(|_| !self.known_revision_unsupported(base));
        let mut response = self
            .post_value_pricing_fetch(&http, identity, base, known)
            .await?;
        if known.is_some() && response.status() == reqwest::StatusCode::BAD_REQUEST {
            // A Router built before knownRevision rejects any non-empty
            // fetch payload. Remember that for this Router (in memory) and
            // retry with the original empty payload right away.
            self.value_pricing_known_revision_unsupported
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(base.to_owned());
            tracing::warn!(router = %base, "Router rejected knownRevision pricing fetch; using the empty fetch payload for this Router");
            known = None;
            response = self
                .post_value_pricing_fetch(&http, identity, base, None)
                .await?;
        }
        let mut response = response.error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len().saturating_add(chunk.len())
                    <= crate::domain::usage::value_pricing::MAX_SNAPSHOT_BYTES + 65536,
                "value pricing response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let body: Value = serde_json::from_slice(&bytes)?;
        if body.get("unchanged") == Some(&Value::Bool(true)) {
            anyhow::ensure!(
                known.is_some() && body.get("revision").and_then(Value::as_str) == known,
                "value pricing unchanged-revision mismatch"
            );
            return Ok(());
        }
        let manifest: Option<PricingManifest> = serde_json::from_value(body)?;
        let Some(manifest) = manifest else {
            return Ok(());
        };
        anyhow::ensure!(
            manifest.installation_id == identity.installation_id
                && manifest.snapshot.revision()? == manifest.revision,
            "value pricing binding/hash mismatch"
        );
        // Already active, or already installed+acknowledged and scheduled.
        if active.as_deref() == Some(manifest.revision.as_str())
            || pending
                .as_ref()
                .is_some_and(|pending| pending.revision == manifest.revision)
        {
            Self::send_value_pricing_ack(&http, identity, base, &manifest.revision).await?;
            // Router rollback to an older content revision can supersede a
            // different pending revision. Never activate the withdrawn one.
            if pending
                .as_ref()
                .is_some_and(|p| p.revision != manifest.revision)
            {
                ledger::schedule_tariff(
                    &self.config_dir,
                    &pricing_scope,
                    &ledger::PendingTariff {
                        revision: manifest.revision.clone(),
                        effective_from_ms: manifest.effective_from_ms,
                    },
                    now_ms_i64(),
                )?;
            }
            return Ok(());
        }
        let config_dir = self.config_dir.clone();
        let install_scope = pricing_scope.clone();
        let install_manifest = manifest.clone();
        tokio::task::spawn_blocking(move || {
            ledger::install_snapshot(
                &config_dir,
                &install_scope,
                &install_manifest.revision,
                &install_manifest.snapshot,
                now_ms_i64(),
            )
        })
        .await
        .context("value pricing install task panicked")??;
        Self::send_value_pricing_ack(&http, identity, base, &manifest.revision).await?;
        ledger::schedule_tariff(
            &self.config_dir,
            &pricing_scope,
            &ledger::PendingTariff {
                revision: manifest.revision.clone(),
                effective_from_ms: manifest.effective_from_ms,
            },
            now_ms_i64(),
        )?;
        // Admission always prices against the active revision; bindings do
        // not need rewriting, so no Share mutation happens here.
        Ok(())
    }
    /// Settles an admitted attempt that produced no usage log (see
    /// `ValueAttemptGuard`). A no-op when the usage log already settled it.
    pub(crate) async fn settle_value_attempt_outcome(
        &self,
        request_id: &str,
        attempt_id: &str,
        outcome: ledger::AttemptOutcome,
    ) {
        let config_dir = self.config_dir.clone();
        let (request, attempt) = (request_id.to_owned(), attempt_id.to_owned());
        let result = tokio::task::spawn_blocking(move || {
            ledger::settle_attempt_outcome(&config_dir, &request, &attempt, outcome)
        })
        .await;
        match result {
            Ok(Ok(true)) => {
                tracing::debug!(%request_id, ?outcome, "USD attempt settled without a usage log");
                // A sent attempt with an unknown outcome is charged its
                // reservation; a USD trial budget must see that charge.
                if let Err(error) = self.sync_market_trial_value(Some(request_id), false).await {
                    tracing::error!(%error, %request_id, "USD trial usage could not be mirrored; Market admission is paused until restart re-derives it");
                    self.mark_quota_usage_degraded();
                }
            }
            Ok(Ok(false)) => {}
            Ok(Err(error)) => {
                tracing::warn!(%error, %request_id, "USD attempt outcome settlement failed; the sweeper will retry")
            }
            Err(error) => tracing::warn!(%error, "USD attempt outcome task panicked"),
        }
    }
    /// Periodic/startup recovery for attempts left pending (see
    /// `ledger::sweep_stale_attempts`).
    pub(crate) async fn sweep_value_quota_attempts(&self, started_before_ms: i64) {
        if !server_sqlite::is_committed(&self.config_dir).unwrap_or(false) {
            return;
        }
        let config_dir = self.config_dir.clone();
        match tokio::task::spawn_blocking(move || {
            ledger::sweep_stale_attempts(&config_dir, started_before_ms)
        })
        .await
        {
            Ok(Ok(report))
                if report.from_usage_log + report.outcome_unknown + report.blocked > 0 =>
            {
                tracing::info!(
                    from_usage_log = report.from_usage_log,
                    outcome_unknown = report.outcome_unknown,
                    blocked_windows = report.blocked,
                    "settled stale USD-equivalent quota attempts"
                );
            }
            Ok(Ok(_)) => {}
            Ok(Err(error)) => tracing::warn!(%error, "USD quota sweeper failed; will retry"),
            Err(error) => tracing::warn!(%error, "USD quota sweeper task panicked"),
        }
        // Every run (including the startup one) re-derives USD trial usage,
        // which also covers settlements whose direct mirror failed.
        if let Err(error) = self.sync_market_trial_value(None, false).await {
            tracing::warn!(%error, "USD trial usage reconciliation failed; will retry");
        }
    }
    /// Folds settled USD-equivalent usage of Trial-phase requests from the
    /// value ledger into `trialValueUsedPicos` (`market_trials::record_value`).
    /// The ledger is the durable source and the entitlement counter a
    /// monotone mirror: the startup pass and every sweeper run re-derive it,
    /// so a crash between a ledger commit and this Share save is repaired
    /// before it can under-count, and replays at worst keep a higher amount.
    /// `in_workspace` is set by callers already holding the workspace gate.
    pub(crate) async fn sync_market_trial_value(
        &self,
        only: Option<&str>,
        in_workspace: bool,
    ) -> anyhow::Result<bool> {
        if !server_sqlite::is_committed(&self.config_dir)? {
            return Ok(false);
        }
        let requests = market_trial_value_requests(&*self.shares.read().await, only);
        if requests.is_empty() {
            return Ok(false);
        }
        let config_dir = self.config_dir.clone();
        let totals =
            tokio::task::spawn_blocking(move || read_market_trial_value(&config_dir, &requests))
                .await
                .context("USD trial usage read task panicked")??;
        if !market_trial_value_grows(&*self.shares.read().await, &totals) {
            return Ok(false);
        }
        let apply = |store: &mut crate::domain::sharing::shares::ShareStore| {
            apply_market_trial_value(store, &totals)
        };
        let changed = if in_workspace {
            self.try_mutate_shares_in_workspace_transaction(apply)
                .await??
        } else {
            self.try_mutate_shares_immediate(apply).await??
        };
        if changed {
            self.router_share_sync_wakeup.notify_one();
        }
        Ok(changed)
    }
    /// Pre-send USD admission for one attempt. Only enforced USD grants are
    /// metered. Reads the grant under the shares read lock only (the ledger
    /// IMMEDIATE transaction serializes admissions; windows aggregate across
    /// policy epochs, so a racing policy edit cannot erase usage) and runs
    /// SQLite on the blocking pool. Returns the attempt's terminal guard.
    pub(crate) async fn admit_share_value_text(
        self: &Arc<Self>,
        context: &UsageLogContext,
        effective_model: &str,
        effective_tier: Option<&'static str>,
        estimate: RiskEstimate,
        verified_surface: bool,
    ) -> anyhow::Result<Option<ValueAttemptGuard>> {
        if context.is_health_check {
            return Ok(None);
        }
        let (Some(share_id), Some(email)) = (&context.share_id, &context.user_email) else {
            return Ok(None);
        };
        let (free_access, grant, trial) = {
            let shares = self.shares.read().await;
            let share = shares.get(share_id).context("VALUE_QUOTA_SHARE_REVOKED")?;
            let grant = share.user_grants.get(&email.to_ascii_lowercase()).cloned();
            let mut trial = None;
            if let Some(grant) = grant.as_ref().filter(|g| {
                g.manager == ShareGrantManager::RouterShareMarket && g.policy.value_quota.is_some()
            }) {
                let id = grant
                    .entitlement_id
                    .as_deref()
                    .context("VALUE_QUOTA_ENTITLEMENT_REQUIRED")?;
                if context
                    .request_id
                    .as_ref()
                    .and_then(|id| share.market_requests.get(id))
                    .is_none_or(|receipt| {
                        receipt.entitlement_id != id || !receipt.email.eq_ignore_ascii_case(email)
                    })
                {
                    return Err(ValueQuotaRejection::new(
                        ValueQuotaRejectionKind::AuthorizationRevoked,
                    )
                    .into());
                }
                if let Some(state) = share.market_entitlements.get(id).filter(|e| {
                    e.phase == crate::domain::sharing::router_contract::ShareMarketPhase::Trial
                }) {
                    if let (Some(allowance), Some(limit)) = (
                        &state.trial_value_allowance_usd_micros,
                        &state.trial_value_limit_usd_micros,
                    ) {
                        let scope_used = share
                            .market_entitlements
                            .values()
                            .filter(|e| e.trial_scope_id == state.trial_scope_id)
                            .try_fold(0i128, |total, e| -> anyhow::Result<i128> {
                                total
                                    .checked_add(
                                        crate::domain::usage::value_pricing::unsigned_decimal(
                                            &e.trial_value_used_picos,
                                        )?,
                                    )
                                    .context("value quota arithmetic overflow")
                            })?;
                        trial = Some(ledger::MarketTrialBudget {
                            scope_id: state.trial_scope_id.clone(),
                            entitlement_id: id.into(),
                            allowance_micros: allowance.clone(),
                            limit_micros: limit.clone(),
                            scope_used_picos: scope_used.to_string(),
                            used_picos: state.trial_value_used_picos.clone(),
                        });
                    }
                }
            }
            (share.free_access, grant, trial)
        };
        let Some(grant) = grant.filter(|grant| grant.active) else {
            // Free-access traffic does not acquire a revoked individual's
            // policy. Private Shares still reject a revocation that raced
            // the initial authorization check.
            if !free_access {
                return Err(ValueQuotaRejection::new(
                    ValueQuotaRejectionKind::AuthorizationRevoked,
                )
                .into());
            }
            return Ok(None);
        };
        let Some(quota) = grant.policy.value_quota.clone() else {
            return Ok(None);
        };
        let result = async {
            let binding = grant
                .value_metering
                .clone()
                .context("VALUE_QUOTA_BINDING_UNAVAILABLE")?;
            anyhow::ensure!(verified_surface, "VALUE_QUOTA_UNVERIFIED_SURFACE");
            let tier = effective_tier.ok_or_else(|| {
                crate::domain::usage::value_pricing::PriceRejection {
                    reason_code: "tier_unknown",
                    model: crate::domain::usage::value_pricing::diagnostic_identity(
                        effective_model,
                    ),
                    tier: "unknown".into(),
                    active_pricing_revision: None,
                    pending_pricing_revision: None,
                    effective_from_ms: None,
                }
            })?;
            let started = i64::try_from(
                context
                    .started_at_ms
                    .context("value quota start time missing")?,
            )?;
            anyhow::ensure!(
                started >= binding.effective_from_ms,
                "VALUE_QUOTA_SCHEDULED"
            );
            let config = self.config_snapshot().await;
            let identity = grant_scope(&config, share_id, email, &grant)?;
            let w = window(&grant, started)?;
            let request_id = context
                .request_id
                .clone()
                .context("value quota request identity missing")?;
            let attempt_id = context.attempt_count.unwrap_or(1).max(1).to_string();
            let parallel_limit = grant.policy.parallel_limit.unwrap_or(4).min(4);
            let config_dir = self.config_dir.clone();
            let model = effective_model.to_owned();
            let (blocking_request, blocking_attempt) = (request_id.clone(), attempt_id.clone());
            tokio::task::spawn_blocking(move || {
                let revision = ledger::active_revision(&config_dir, &identity)?
                    .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
                let admission = ledger::Admission {
                    enforce: true,
                    scope: &identity,
                    window: &w,
                    policy_epoch: &binding.quota_policy_epoch,
                    quota: &quota,
                    pricing_revision: &revision,
                    request_id: &blocking_request,
                    attempt_id: &blocking_attempt,
                    effective_model: &model,
                    effective_tier: tier,
                    started_at_ms: started,
                    estimate,
                    parallel_limit,
                };
                match trial.as_ref() {
                    Some(trial) => {
                        ledger::admit_with_market_trial(&config_dir, &admission, Some(trial))
                    }
                    None => ledger::admit(&config_dir, &admission),
                }
            })
            .await
            .context("value quota admission task panicked")??;
            Ok::<_, anyhow::Error>(ValueAttemptGuard {
                state: self.clone(),
                request_id,
                attempt_id,
                phase: AtomicU8::new(PHASE_NOT_SENT),
            })
        }
        .await
        .map_err(|error| {
            if ledger::is_storage_busy(&error) {
                ValueQuotaRejection::new(ValueQuotaRejectionKind::StorageBusy).into()
            } else {
                error
            }
        });
        if let Err(error) = &result {
            metrics::counter!("share_value_quota_rejections_total","reason"=>rejection_reason(error))
                .increment(1);
        }
        result.map(Some)
    }
    /// One user's status error never fails the list: that row reports
    /// `meteringStatus: "unavailable"` with a machine `reasonCode`.
    pub(crate) async fn value_quota_statuses(&self, share_id: &str) -> anyhow::Result<Vec<Value>> {
        self.value_quota_rows(share_id, false).await
    }
    /// Rows returned after a usage edit. A row whose status cannot be read
    /// is `{"email","error":{"code"}}` instead of a degraded status, so a
    /// client merging the response never overwrites good data with it.
    pub(crate) async fn value_quota_edit_statuses(
        &self,
        share_id: &str,
    ) -> anyhow::Result<Vec<Value>> {
        self.value_quota_rows(share_id, true).await
    }
    async fn value_quota_rows(
        &self,
        share_id: &str,
        edit_response: bool,
    ) -> anyhow::Result<Vec<Value>> {
        let config = self.config_snapshot().await;
        let now = now_ms_i64();
        let share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        let mut result = Vec::new();
        for (email, grant) in share.user_grants.iter().filter(|(_, g)| metered_grant(g)) {
            let row = match value_status(&self.config_dir, &config, share_id, email, grant, now) {
                Ok(status) => {
                    let code = match status.metering_status.as_str() {
                        "blocked" => "blocked_needs_reconcile",
                        "scheduled" => "scheduled_next_window",
                        _ => "ready",
                    };
                    let mut row = serde_json::to_value(status)?;
                    row["reasonCode"] = Value::from(code);
                    if let Some(pending) = &grant.pending_value_quota {
                        row["pendingValueQuota"] = serde_json::json!({
                            "effectiveFromMs": pending.effective_from_ms,
                            "limitUsdMicros": pending.policy.value_quota.as_ref().map(|q| q.limit_usd_micros.clone()),
                        });
                    }
                    row
                }
                Err(error) if edit_response => {
                    tracing::debug!(%error, %share_id, "one USD quota status is unavailable after an edit");
                    serde_json::json!({"email":email,"error":{"code":status_reason_code(&error)}})
                }
                Err(error) => {
                    tracing::debug!(%error, %share_id, "one USD quota status is unavailable");
                    serde_json::json!({"email":email,"meteringStatus":"unavailable",
                        "reasonCode":status_reason_code(&error),"syncedAtMs":now})
                }
            };
            result.push(row);
        }
        Ok(result)
    }
    pub(crate) fn value_quota_diagnostics(&self, share_id: &str) -> anyhow::Result<Value> {
        ledger::diagnostics(&self.config_dir, share_id)
    }
    pub(super) async fn sync_share_value_watermarks(
        &self,
        config: &ServerConfig,
    ) -> anyhow::Result<()> {
        if !self.router_supports_value_quotas().await {
            return Ok(());
        }
        if let Err(error) = ledger::compact_terminal_history(&self.config_dir, now_ms_i64()) {
            tracing::warn!(%error,"value quota maintenance failed; no history is discarded");
        }
        let identity = config
            .registered_router_identity()
            .context("Router binding unavailable")?;
        let base = config.router_api_base().context("Router URL unavailable")?;
        let mut grants = self
            .shares
            .read()
            .await
            .shares
            .iter()
            .flat_map(|s| {
                s.user_grants
                    .iter()
                    .filter(|(_, g)| metered_grant(g))
                    .map(|(email, g)| ((s.id.clone(), email.clone()), g.clone()))
            })
            .collect::<Vec<_>>();
        let sync_started = std::time::Instant::now();
        let sync_scope = scope(config, "sync", "sync@local.invalid")?;
        let last: Option<(String, String)> =
            serde_json::from_str(&ledger::value_sync_cursor(&self.config_dir, &sync_scope)?).ok();
        grants.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(position) = last.and_then(|last| grants.iter().position(|(key, _)| key > &last))
        {
            grants.rotate_left(position);
        }
        let mut sent = 0;
        for ((id, email), grant) in grants {
            if sent >= 16 || sync_started.elapsed() >= Duration::from_secs(5) {
                return Ok(());
            }
            ledger::set_value_sync_cursor(
                &self.config_dir,
                &sync_scope,
                &serde_json::to_string(&(&id, &email))?,
            )?;
            let status = match value_status(
                &self.config_dir,
                config,
                &id,
                &email,
                &grant,
                now_ms_i64(),
            ) {
                Ok(status) => status,
                Err(error) => {
                    tracing::debug!(%error,"one value quota projection is unavailable; continuing other users");
                    continue;
                }
            };
            let identity_scope = grant_scope(config, &id, &status.email, &grant)?;
            let needs_sync = match ledger::projection_needs_sync(
                &self.config_dir,
                &identity_scope,
                &status,
                now_ms_i64(),
            ) {
                Ok(needs) => needs,
                Err(error) => {
                    tracing::warn!(%error,"value quota projection retry state unavailable");
                    continue;
                }
            };
            if !needs_sync {
                continue;
            }
            let request = client::signed_canonical_request(
                identity,
                "share_value_quota_watermark",
                serde_json::json!({"shareId":id,"status":status}),
            )?;
            sent += 1;
            let response = self
                .http_client()
                .await
                .post(format!(
                    "{}/v1/shares/{id}/value-quota-watermarks",
                    base.trim_end_matches('/')
                ))
                .timeout(Duration::from_secs(3))
                .json(&request)
                .send()
                .await;
            match response {
                Ok(response) if response.status().is_success() => {
                    let ack = response.json::<Value>().await;
                    let mark = ack
                        .as_ref()
                        .ok()
                        .and_then(|ack| ack.get("usageWatermark").and_then(Value::as_u64));
                    let Some(mark) = mark else {
                        tracing::warn!("invalid value watermark ACK; retaining retry");
                        continue;
                    };
                    if let Err(error) = ledger::ack_projection(
                        &self.config_dir,
                        &identity_scope,
                        &status,
                        mark,
                        now_ms_i64(),
                    ) {
                        tracing::warn!(%error,"value watermark ACK could not be committed");
                    }
                }
                Ok(response) => {
                    tracing::debug!(status=%response.status(),"value watermark awaits descriptor synchronization")
                }
                Err(error) => {
                    tracing::debug!(%error,"value watermark will retry without blocking heartbeat")
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::usage::value_pricing::ValueQuota;

    const DAY: i64 = 86_400_000;

    fn usd_policy(period: ShareTokenPeriod) -> ShareUserPolicy {
        ShareUserPolicy {
            value_quota: Some(ValueQuota {
                limit_usd_micros: "1000000".into(),
                tariff_policy_id: "api-equivalent-v1".into(),
            }),
            token_period: period,
            ..Default::default()
        }
    }

    fn binding(effective_from_ms: i64) -> ValueMeteringBinding {
        ValueMeteringBinding {
            quota_policy_epoch: "epoch".into(),
            value_metered_from_ms: 0,
            effective_from_ms,
            pricing_revision: "rev".into(),
        }
    }

    #[test]
    fn natural_period_activation_is_immediate_only_for_a_proven_empty_window() {
        let now = 20 * DAY + 5_000;
        let policy = usd_policy(ShareTokenPeriod::Day);
        let token_grant = ShareUserGrant {
            email: "user@example.com".into(),
            active: true,
            ..Default::default()
        };
        // A brand-new grant trivially has no history in the window.
        let start =
            proven_empty_window_start(None, "share", "user@example.com", true, &policy, now);
        assert_eq!(start, Some(20 * DAY));
        assert_eq!(
            activation_rule(None, &policy, now, start),
            Activation::Immediate {
                metered_from_ms: 20 * DAY,
                reason_code: "ready",
                cover_window: true,
            }
        );
        // An existing grant without a usage snapshot proves nothing.
        assert_eq!(
            proven_empty_window_start(None, "share", "user@example.com", false, &policy, now),
            None
        );
        let empty = crate::domain::usage::store::UsageStore::default();
        let start = proven_empty_window_start(
            Some(&empty),
            "share",
            "user@example.com",
            false,
            &policy,
            now,
        );
        assert_eq!(start, Some(20 * DAY));
        assert!(matches!(
            activation_rule(Some(&token_grant), &policy, now, start),
            Activation::Immediate {
                metered_from_ms,
                cover_window: true,
                ..
            } if metered_from_ms == 20 * DAY
        ));
        assert_eq!(
            activation_rule(Some(&token_grant), &policy, now, None),
            Activation::NextWindow
        );
        // Lifetime keeps "metered from now" regardless of the proof.
        assert!(matches!(
            activation_rule(
                Some(&token_grant),
                &usd_policy(ShareTokenPeriod::Lifetime),
                now,
                Some(0)
            ),
            Activation::Immediate {
                reason_code: "metered_from_now",
                cover_window: false,
                ..
            }
        ));
    }

    #[test]
    fn metering_status_is_real_while_a_usd_policy_is_enforced() {
        let now = 10 * DAY;
        let mut grant = ShareUserGrant {
            active: true,
            policy: usd_policy(ShareTokenPeriod::Day),
            pending_value_quota: Some(PendingValueQuota {
                policy: usd_policy(ShareTokenPeriod::Week),
                effective_from_ms: 11 * DAY,
                retain_access: true,
            }),
            ..Default::default()
        };
        // Enforced USD + pending USD: the current window's real status.
        assert_eq!(
            metering_status_for(&grant, &binding(0), "pending".into(), now),
            "pending"
        );
        // Token enforced, USD pending: scheduled.
        grant.policy = ShareUserPolicy::default();
        assert_eq!(
            metering_status_for(&grant, &binding(11 * DAY), "observed".into(), now),
            "scheduled"
        );
        // A blocked window is always reported.
        assert_eq!(
            metering_status_for(&grant, &binding(11 * DAY), "blocked".into(), now),
            "blocked"
        );
    }

    #[test]
    fn legacy_degraded_marker_is_removed_at_startup() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-value-degraded-{}",
            crate::domain::sharing::quota_cycle::opaque_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("share-value-quota-degraded.json");
        std::fs::write(&marker, br#"{"schemaVersion":1}"#).unwrap();
        remove_legacy_degraded_marker(&dir);
        assert!(!marker.exists());
        // Idempotent when absent.
        remove_legacy_degraded_marker(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
