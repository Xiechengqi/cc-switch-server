//! Server-owned Share quota orchestration; no wallet or price-source access.
use super::*;
use crate::domain::sharing::router_contract::{
    PendingValueQuota, ShareGrantManager, ShareTokenPeriod, ShareUserGrant,
};
use crate::domain::usage::value_pricing::{
    PricingManifest, ValueMeteringBinding, ValueQuota, ValueQuotaStatus, MAX_LIMIT_MICROS,
    TARIFF_POLICY,
};
use crate::repository::server_sqlite::{self, value_quota as ledger};

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
    })
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
            "{:?}:{}:{}",
            grant.policy.token_period,
            period.starts_at_ms.unwrap_or(0),
            period.ends_at_ms.unwrap_or(0)
        ),
        starts_at_ms: period.starts_at_ms.unwrap_or(0),
        ends_at_ms: period.ends_at_ms,
        metered_from_ms: binding.value_metered_from_ms,
    })
}
fn verified_text_bindings(share: &Share, providers: &ProviderStore) -> bool {
    !share.bindings.is_empty()
        && share.bindings.iter().all(|binding| {
            matches!(
                binding.provider_type,
                ProviderType::Claude | ProviderType::Codex | ProviderType::Gemini
            ) && providers
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
    let identity = scope(config, share_id, email)?;
    let w = window(grant, now)?;
    let (revision, _) = ledger::active_snapshot(dir, &identity)?;
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
        metering_status: if grant.pending_value_quota.is_some()
            || (grant.policy.value_quota.is_some() && now < binding.effective_from_ms)
        {
            "scheduled".into()
        } else {
            status.metering_status
        },
        synced_at_ms: now,
    })
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
    /// Shadow-only failures must never suspend the existing Token rail. Keep
    /// an independent durable fail-closed marker for USD activation instead.
    pub(super) fn mark_value_quota_degraded(&self) {
        self.value_quota_degraded
            .store(true, std::sync::atomic::Ordering::Release);
        if let Err(error) = crate::infra::storage::write_json_pretty(
            &self.config_dir.join("share-value-quota-degraded.json"),
            &serde_json::json!({"schemaVersion":1,"reason":"metering_storage_requires_recovery"}),
        ) {
            tracing::error!(%error, "USD metering is blocked; persistent recovery marker could not be written");
        }
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
                .context("active USD-equivalent grant required")?;
            anyhow::ensure!(
                grant.manager != ShareGrantManager::RouterShareMarket,
                "Share Market managed users are read-only"
            );
            inputs.push((
                scope(&config, share_id, &email)?,
                window(grant, now)?,
                grant.revision,
                edit,
            ));
        }
        let batch = inputs
            .iter()
            .map(|(scope, window, revision, edit)| ledger::Adjustment {
                scope,
                window,
                grant_revision: *revision,
                edit,
            })
            .collect::<Vec<_>>();
        ledger::apply_adjustments(&self.config_dir, &batch, actor, now)?;
        self.router_share_sync_wakeup.notify_one();
        Ok(())
    }
    pub async fn preview_share_value_quota(
        &self,
        share_id: &str,
        expected_revision: u64,
        policies: BTreeMap<String, crate::domain::sharing::router_contract::ShareUserPolicy>,
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
        let capable = self.router_supports_value_quotas().await
            && verified_text_bindings(&share, &providers)
            && !self
                .value_quota_degraded
                .load(std::sync::atomic::Ordering::Acquire);
        let config = self.config_snapshot().await;
        let now = now_ms_i64();
        let tariff = scope(&config, share_id, "preview@local.invalid")
            .and_then(|identity| ledger::active_snapshot(&self.config_dir, &identity));
        let mut rows = Vec::new();
        let mut all_rows_ready = true;
        for (email, policy) in &policies {
            crate::domain::sharing::token_period::validate_user_policy(policy, now)
                .map_err(anyhow::Error::msg)?;
            let email = email.trim().to_ascii_lowercase();
            let previous = share.user_grants.get(&email);
            anyhow::ensure!(
                !previous
                    .is_some_and(|g| g.active && g.manager == ShareGrantManager::RouterShareMarket),
                "Share Market managed users are read-only"
            );
            let mut grant = previous.cloned().unwrap_or_default();
            grant.email = email.clone();
            grant.policy = policy.clone();
            let identity = scope(&config, share_id, &email)?;
            let coverage = ledger::coverage_start(&self.config_dir, &identity)?;
            let metered_from = grant
                .value_metering
                .as_ref()
                .map(|b| b.value_metered_from_ms)
                .or(coverage)
                .unwrap_or(now);
            grant.value_metering.get_or_insert(ValueMeteringBinding {
                quota_policy_epoch: "preview-only".into(),
                value_metered_from_ms: metered_from,
                effective_from_ms: now,
                pricing_revision: tariff.as_ref().map(|(r, _)| r.clone()).unwrap_or_default(),
            });
            let w = window(&grant, now)?;
            let status = ledger::status(&self.config_dir, &identity, &w);
            let unchanged_window = previous.is_some_and(|old| {
                old.policy.value_quota.is_some()
                    && old.policy.token_period == policy.token_period
                    && old.policy.token_period_anchor_at_ms == policy.token_period_anchor_at_ms
            });
            let first_lifetime = coverage.is_none()
                && previous.is_none_or(|g| g.value_metering.is_none())
                && policy.token_period == ShareTokenPeriod::Lifetime;
            let complete = status.as_ref().is_ok_and(|s| {
                matches!(s.metering_status.as_str(), "observed" | "conservative")
                    && (policy.token_period == ShareTokenPeriod::Lifetime
                        || metered_from <= w.starts_at_ms)
            });
            let reason = if first_lifetime && self.share_in_flight.has_in_flight(share_id) {
                Some("existing_requests_in_flight")
            } else if !complete && !first_lifetime && !unchanged_window {
                Some("coverage_incomplete")
            } else {
                None
            };
            let effective_from = if reason.is_none() || unchanged_window {
                now
            } else {
                w.ends_at_ms.unwrap_or(now)
            };
            let row_ready = reason.is_none()
                || (reason == Some("coverage_incomplete") && w.ends_at_ms.is_some());
            all_rows_ready &= row_ready;
            rows.push(serde_json::json!({"email":email,"windowId":w.id,"windowStartsAtMs":w.starts_at_ms,"resetsAtMs":w.ends_at_ms,
                "meteredFromMs":metered_from,"effectiveFromMs":effective_from,"usedPicos":status.as_ref().ok().map(|s|s.used_picos.clone()),
                "usageWatermark":status.as_ref().ok().map(|s|s.usage_watermark),"ready":row_ready,"reason":reason}));
        }
        Ok(
            serde_json::json!({"shareId":share_id,"configRevision":share.config_revision,"ready":capable && tariff.is_ok() && all_rows_ready,
            "reason":if !capable {Some("Router v10, committed SQLite storage and verified text bindings are required")} else if tariff.is_err() {Some("An acknowledged executable pricing snapshot is required")} else if !all_rows_ready {Some("Complete metering history is required; wait for in-flight requests or choose a complete natural window")} else {None},
            "activePricingRevision":tariff.as_ref().ok().map(|(r,_)|r),"meteredFromConfirmationRequired":true,
            "softLimit":true,"maximumParallel":4,"rows":rows}),
        )
    }
    pub async fn router_supports_value_quotas(&self) -> bool {
        if !server_sqlite::is_committed(&self.config_dir).unwrap_or(false) {
            return false;
        }
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
    pub(crate) async fn share_has_value_metering(
        &self,
        share_id: &str,
        email: Option<&str>,
    ) -> bool {
        self.shares
            .read()
            .await
            .get(share_id)
            .and_then(|s| email.and_then(|e| s.user_grants.get(&e.to_ascii_lowercase())))
            .is_some_and(|g| g.active && g.value_metering.is_some())
    }
    pub(crate) async fn mark_unverified_value_surface(
        &self,
        share_id: &str,
        email: Option<&str>,
        now: i64,
    ) {
        if !self.share_has_value_metering(share_id, email).await {
            return;
        }
        if let Some(email) = email {
            let result =
                scope(&self.config_snapshot().await, share_id, email).and_then(|identity| {
                    ledger::record_gap(
                        &self.config_dir,
                        &identity,
                        &format!(
                            "unverified:{}",
                            crate::domain::sharing::quota_cycle::opaque_id()
                        ),
                        now,
                    )
                });
            if let Err(error) = result {
                self.mark_value_quota_degraded();
                tracing::warn!(%error,"value shadow surface coverage unavailable");
            }
        }
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
        self.prepare_value_bindings_with_context(
            candidate, previous, now, &config, capable, &providers,
        )
    }
    pub(super) fn prepare_value_bindings_with_context(
        &self,
        candidate: &mut ShareStore,
        previous: &ShareStore,
        now: i64,
        config: &ServerConfig,
        capable: bool,
        providers: &ProviderStore,
    ) -> anyhow::Result<()> {
        for share in &mut candidate.shares {
            let verified_surface = verified_text_bindings(share, providers);
            let old_share = previous.get(&share.id);
            let mut changed = false;
            for (email, grant) in &mut share.user_grants {
                let old = old_share.and_then(|s| s.user_grants.get(email));
                let original = grant.clone();
                let required =
                    grant.policy.value_quota.is_some() || grant.pending_value_quota.is_some();
                let prepared = (|| -> anyhow::Result<bool> {
                    let mut changed = false;
                    let requested = grant.policy.clone();
                    let mut policy_changed =
                        old.is_none_or(|g| g.policy != grant.policy || !g.active);
                    let mut activating = false;
                    if !grant.active || grant.manager == ShareGrantManager::RouterShareMarket {
                        return Ok(false);
                    }
                    anyhow::ensure!(
                        !(grant.policy.value_quota.is_some() && policy_changed) || verified_surface,
                        "VALUE_QUOTA_UNSUPPORTED_PROVIDER_SURFACE"
                    );
                    anyhow::ensure!(
                        !(grant.policy.value_quota.is_some()
                            && policy_changed
                            && self
                                .value_quota_degraded
                                .load(std::sync::atomic::Ordering::Acquire)),
                        "VALUE_QUOTA_USAGE_UNAVAILABLE"
                    );
                    // Keep the previous policy enforced until durable activation.
                    if let Some(pending) = grant.pending_value_quota.clone() {
                        if now >= pending.effective_from_ms {
                            grant.policy = pending.policy;
                            grant.pending_value_quota = None;
                            grant.revision = grant.revision.saturating_add(1);
                            policy_changed = true;
                            changed = true;
                            activating = true;
                        } else if requested == pending.policy {
                            grant.policy = old.map(|g| g.policy.clone()).unwrap_or_default();
                            policy_changed = false;
                        }
                    }
                    if !capable && !activating {
                        anyhow::ensure!(
                            !(grant.policy.value_quota.is_some() && policy_changed),
                            "VALUE_QUOTA_ROUTER_UPGRADE_OR_COMMITTED_STORAGE_REQUIRED"
                        );
                        return Ok(changed);
                    }
                    let identity = scope(config, &share.id, email)?;
                    let snapshot = ledger::active_snapshot(&self.config_dir, &identity);
                    if snapshot.is_err() && grant.policy.value_quota.is_none() {
                        return Ok(changed);
                    }
                    let (revision, _) = snapshot?;
                    let mut binding = if let Some(mut current) =
                        old.and_then(|g| g.value_metering.clone())
                    {
                        if policy_changed {
                            current.quota_policy_epoch =
                                crate::domain::sharing::quota_cycle::opaque_id();
                        }
                        current
                    } else {
                        let effective_from_ms =
                            if grant.policy.token_period == ShareTokenPeriod::AccountReset {
                                let end = grant
                                    .account_quota_cycle
                                    .as_ref()
                                    .and_then(|c| c.ends_at_ms);
                                if end.is_none() && grant.policy.value_quota.is_none() {
                                    // Shadow collection must never block an existing
                                    // Token grant whose upstream cycle is unavailable.
                                    return Ok(changed);
                                }
                                end.context("VALUE_QUOTA_ACCOUNT_CYCLE_UNAVAILABLE")?
                            } else {
                                crate::domain::sharing::token_period::token_period_window(
                                    &grant.policy,
                                    now,
                                )
                                .map_err(anyhow::Error::msg)?
                                .ends_at_ms
                                .unwrap_or(now)
                            };
                        // A new lifetime origin cannot discard already admitted
                        // requests whose monetary coverage has not started yet.
                        if grant.policy.value_quota.is_some()
                            && grant.policy.token_period == ShareTokenPeriod::Lifetime
                            && ledger::coverage_start(&self.config_dir, &identity)?.is_none()
                        {
                            anyhow::ensure!(
                                !self.share_in_flight.has_in_flight(&share.id),
                                "VALUE_QUOTA_EXISTING_REQUESTS_IN_FLIGHT"
                            );
                        }
                        let metered_from =
                            ledger::begin_coverage(&self.config_dir, &identity, now)?;
                        ValueMeteringBinding {
                            quota_policy_epoch: crate::domain::sharing::quota_cycle::opaque_id(),
                            value_metered_from_ms: metered_from,
                            effective_from_ms,
                            pricing_revision: revision.clone(),
                        }
                    };
                    // Preview and save both allow immediate activation only with
                    // complete evidence in the real requested window. Re-adding a
                    // deleted grant reuses durable coverage, never a fresh origin.
                    grant.value_metering = Some(binding.clone());
                    if grant.policy.value_quota.is_some()
                        && policy_changed
                        && old.is_none_or(|g| {
                            g.policy.value_quota.is_none()
                                || g.policy.token_period != grant.policy.token_period
                                || g.policy.token_period_anchor_at_ms
                                    != grant.policy.token_period_anchor_at_ms
                        })
                    {
                        let w = window(grant, now)?;
                        let complete =
                            ledger::status(&self.config_dir, &identity, &w).is_ok_and(|s| {
                                matches!(s.metering_status.as_str(), "observed" | "conservative")
                                    && (grant.policy.token_period == ShareTokenPeriod::Lifetime
                                        || w.metered_from_ms <= w.starts_at_ms)
                            });
                        if !complete {
                            binding.effective_from_ms =
                                w.ends_at_ms.context("VALUE_QUOTA_HISTORY_INCOMPLETE")?;
                        } else {
                            binding.effective_from_ms = now;
                        }
                    }
                    binding.pricing_revision = revision;
                    if grant.policy.value_quota.is_some()
                        && binding.effective_from_ms > now
                        && grant.pending_value_quota.is_none()
                    {
                        grant.pending_value_quota = Some(PendingValueQuota {
                            policy: grant.policy.clone(),
                            effective_from_ms: binding.effective_from_ms,
                            retain_access: old.is_some_and(|g| g.active),
                        });
                        if let Some(old_binding) = old
                            .filter(|g| g.policy.value_quota.is_some())
                            .and_then(|g| g.value_metering.as_ref())
                        {
                            binding.effective_from_ms = old_binding.effective_from_ms;
                        }
                        grant.policy = old.map(|g| g.policy.clone()).unwrap_or_default();
                        grant.usage = old.map(|g| g.usage.clone()).unwrap_or_default();
                        grant.usage_rebase = old.and_then(|g| g.usage_rebase.clone());
                        grant.usage_quota = old.and_then(|g| g.usage_quota);
                        changed = true;
                    }
                    if grant.value_metering.as_ref() != Some(&binding) {
                        grant.value_metering = Some(binding);
                        changed = true;
                    }
                    Ok(changed || original.value_metering != grant.value_metering)
                })();
                match prepared {
                    Ok(grant_changed) => changed |= grant_changed,
                    Err(error) if !required => {
                        *grant = original;
                        tracing::debug!(%error, "value shadow preparation unavailable; Token policy remains unchanged");
                    }
                    Err(error) => return Err(error),
                }
            }
            if changed && old_share.is_none_or(|old| share.config_revision <= old.config_revision) {
                share.config_revision = share.config_revision.saturating_add(1).max(1);
            }
        }
        Ok(())
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
    /// The heartbeat path retries installation and ACK. Request execution uses
    /// only the last local acknowledged snapshot when Router is unavailable.
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
        let http = self.http_client().await;
        let request =
            client::signed_request(identity, "share_value_pricing_fetch", serde_json::json!({}))?;
        let mut response = http
            .post(format!(
                "{}/v1/shares/value-pricing/fetch",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(20))
            .json(&request)
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            anyhow::ensure!(
                bytes.len().saturating_add(chunk.len())
                    <= crate::domain::usage::value_pricing::MAX_SNAPSHOT_BYTES + 65536,
                "value pricing response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        let manifest: Option<PricingManifest> = serde_json::from_slice(&bytes)?;
        let Some(manifest) = manifest else {
            return Ok(());
        };
        anyhow::ensure!(
            manifest.installation_id == identity.installation_id
                && manifest.snapshot.revision()? == manifest.revision,
            "value pricing binding/hash mismatch"
        );
        if manifest.effective_from_ms > now_ms_i64() {
            return Ok(());
        }
        let identity_scope = scope(config, "pricing", "pricing@local.invalid")?;
        ledger::install_snapshot(
            &self.config_dir,
            &identity_scope,
            &manifest.revision,
            &manifest.snapshot,
            now_ms_i64(),
        )?;
        let ack = client::signed_request(
            identity,
            "share_value_pricing_ack",
            serde_json::json!({"revision":manifest.revision,"storageSchema":3}),
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
            response.get("revision").and_then(Value::as_str) == Some(manifest.revision.as_str()),
            "value pricing ACK mismatch"
        );
        ledger::activate_snapshot(&self.config_dir, &identity_scope, &manifest.revision)?;
        // Initiate shadow collection for ordinary grants. No wallet mutation.
        self.mutate_shares_immediate(|_| {}).await?;
        Ok(())
    }
    pub(crate) async fn mark_value_shadow_request(
        &self,
        context: &UsageLogContext,
    ) -> anyhow::Result<()> {
        if context.is_health_check {
            return Ok(());
        }
        let (Some(share_id), Some(email), Some(request_id), Some(at)) = (
            &context.share_id,
            &context.user_email,
            &context.request_id,
            context.started_at_ms,
        ) else {
            return Ok(());
        };
        let (bound, enforced) = {
            let shares = self.shares.read().await;
            let grant = shares
                .get(share_id)
                .and_then(|share| share.user_grants.get(&email.to_ascii_lowercase()));
            (
                grant.is_some_and(|g| g.value_metering.is_some()),
                grant.is_some_and(|g| g.active && g.policy.value_quota.is_some()),
            )
        };
        if enforced {
            return Ok(());
        }
        if !bound && !server_sqlite::is_committed(&self.config_dir)? {
            return Ok(());
        }
        let identity = match scope(&self.config_snapshot().await, share_id, email) {
            Ok(identity) => identity,
            Err(_) if !bound => return Ok(()),
            Err(error) => return Err(error),
        };
        // Preparing a batch may start durable coverage before a later policy
        // rejects or the Share commit fails. Deleting an ordinary grant can
        // also leave free-access traffic. Neither may turn that durable
        // origin into falsely complete zero-usage history on a later re-add.
        if !bound && ledger::coverage_start(&self.config_dir, &identity)?.is_none() {
            return Ok(());
        }
        ledger::record_gap(&self.config_dir, &identity, request_id, i64::try_from(at)?)?;
        Ok(())
    }
    pub(crate) async fn admit_share_value_text(
        &self,
        context: &UsageLogContext,
        effective_model: &str,
        effective_tier: &str,
        verified_surface: bool,
    ) -> anyhow::Result<()> {
        if context.is_health_check {
            return Ok(());
        }
        let (Some(share_id), Some(email)) = (&context.share_id, &context.user_email) else {
            return Ok(());
        };
        let config = self.config_snapshot().await;
        let _quota = self.share_quota_mutations.lock().await;
        let _commit = self.shares_persistence.gate.lock().await;
        let (free_access, grant) = {
            let shares = self.shares.read().await;
            let share = shares.get(share_id).context("VALUE_QUOTA_SHARE_REVOKED")?;
            (
                share.free_access,
                share.user_grants.get(&email.to_ascii_lowercase()).cloned(),
            )
        };
        let Some(grant) = grant.filter(|grant| grant.active) else {
            // Free-access traffic does not acquire a revoked individual's
            // policy. Private Shares still reject a revocation that raced
            // the initial authorization check.
            anyhow::ensure!(free_access, "VALUE_QUOTA_AUTHORIZATION_REVOKED");
            return Ok(());
        };
        let Some(binding) = &grant.value_metering else {
            anyhow::ensure!(
                grant.policy.value_quota.is_none(),
                "VALUE_QUOTA_BINDING_UNAVAILABLE"
            );
            return Ok(());
        };
        let enforce = grant.policy.value_quota.is_some();
        anyhow::ensure!(
            !enforce || verified_surface,
            "VALUE_QUOTA_UNVERIFIED_SURFACE"
        );
        if !verified_surface {
            return Ok(());
        }
        anyhow::ensure!(
            !enforce
                || (!self
                    .value_quota_degraded
                    .load(std::sync::atomic::Ordering::Acquire)
                    && !self
                        .quota_usage_degraded
                        .load(std::sync::atomic::Ordering::Acquire)),
            "VALUE_QUOTA_USAGE_UNAVAILABLE"
        );
        let result = (|| -> anyhow::Result<()> {
            let identity = scope(&config, share_id, email)?;
            let (revision, _) = ledger::active_snapshot(&self.config_dir, &identity)?;
            let started = i64::try_from(
                context
                    .started_at_ms
                    .context("value quota start time missing")?,
            )?;
            if enforce {
                anyhow::ensure!(
                    started >= binding.effective_from_ms,
                    "VALUE_QUOTA_SCHEDULED"
                );
            }
            let w = window(&grant, started)?;
            let shadow_quota = ValueQuota {
                limit_usd_micros: MAX_LIMIT_MICROS.to_string(),
                tariff_policy_id: TARIFF_POLICY.into(),
            };
            let admission = ledger::Admission {
                enforce,
                scope: &identity,
                window: &w,
                policy_epoch: &binding.quota_policy_epoch,
                quota: grant.policy.value_quota.as_ref().unwrap_or(&shadow_quota),
                pricing_revision: &revision,
                request_id: context
                    .request_id
                    .as_deref()
                    .context("value quota request identity missing")?,
                attempt_id: "1",
                effective_model,
                effective_tier,
                started_at_ms: started,
            };
            ledger::admit(&self.config_dir, &admission).map(|_| ())
        })();
        if enforce {
            if let Err(error) = &result {
                let message = error.to_string();
                let reason = if message.contains("EXHAUSTED") {
                    "exhausted"
                } else if message.contains("price_unavailable") {
                    "price_unavailable"
                } else {
                    "quota_unavailable"
                };
                metrics::counter!("share_value_quota_rejections_total","reason"=>reason)
                    .increment(1);
            }
            result
        } else {
            if let Err(error) = result {
                tracing::debug!(%error,"Share value shadow coverage remains incomplete");
            }
            Ok(())
        }
    }
    pub(crate) async fn value_quota_statuses(
        &self,
        share_id: &str,
    ) -> anyhow::Result<Vec<ValueQuotaStatus>> {
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
        for (email, grant) in share
            .user_grants
            .iter()
            .filter(|(_, g)| g.active && g.value_metering.is_some())
        {
            result.push(value_status(
                &self.config_dir,
                &config,
                share_id,
                email,
                grant,
                now,
            )?);
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
                    .filter(|(_, g)| g.active && g.value_metering.is_some())
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
            let identity_scope = scope(config, &id, &status.email)?;
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
            let request = client::signed_request(
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
