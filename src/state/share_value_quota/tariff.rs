//! Pricing readiness and offline activation, kept separate from user quotas.
use super::*;
use crate::domain::providers::runtime::RuntimeModelPolicy;

/// Owner-managed grants meter a missing price as an explicit $0, so the
/// check does not block: `ready:true, state:"unpriced"` keeps the reason for
/// a warning. Market seats treat `state:"unpriced"` as not ready
/// (`market_price_ready`). A missing tariff is never downgraded here.
fn owner_unpriced(check: &mut Value) {
    if check["ready"] != true {
        check["ready"] = true.into();
        check["state"] = "unpriced".into();
    }
}
pub(super) fn market_price_ready(check: &Value) -> bool {
    check["ready"] == true && check["state"] != "unpriced"
}

impl ServerStateInner {
    pub(super) fn value_model_coverage(
        &self,
        share: &Share,
        providers: &ProviderStore,
        config: &ServerConfig,
        now: i64,
    ) -> anyhow::Result<Vec<Value>> {
        if !server_sqlite::is_committed(&self.config_dir).unwrap_or(false) {
            return Ok(Vec::new());
        }
        let identity = match scope(config, &share.id, "coverage@local.invalid") {
            Ok(scope) => scope,
            Err(_) => return Ok(Vec::new()),
        };
        let mut checks = Vec::new();
        for binding in enabled_bindings(share) {
            let Some(plan) = providers.runtime_plan(binding.app, &binding.provider_id) else {
                continue;
            };
            if !crate::domain::usage::value_pricing::supports_value_text_runtime(
                plan.profile_id.as_str(),
                plan.driver_id.as_str(),
                plan.coding_plan.is_some(),
            ) {
                continue;
            }
            match &plan.model_policy {
                RuntimeModelPolicy::Single { upstream_model } => {
                    let Some(model) =
                        crate::domain::usage::value_pricing::value_execution_model_hint(
                            plan.profile_id.as_str(),
                            upstream_model,
                        )
                    else {
                        let mut check = serde_json::json!({"code":"model_price_coverage","ready":false,
                            "app":binding.app,"providerId":binding.provider_id,"profileId":plan.profile_id.as_str(),
                            "driverId":plan.driver_id.as_str(),"model":upstream_model,
                            "scope":"configured_model","reasonCode":"model_unresolved"});
                        owner_unpriced(&mut check);
                        checks.push(check);
                        continue;
                    };
                    let coverage = ledger::model_coverage(
                        &self.config_dir,
                        &identity,
                        &model,
                        "standard",
                        now,
                    )?;
                    let mut check = serde_json::to_value(coverage)?;
                    check["code"] = "model_price_coverage".into();
                    check["app"] = serde_json::to_value(binding.app)?;
                    check["providerId"] = binding.provider_id.clone().into();
                    check["profileId"] = plan.profile_id.as_str().into();
                    check["driverId"] = plan.driver_id.as_str().into();
                    check["scope"] = "configured_model".into();
                    if check["activePricingRevision"].is_null() {
                        check["blockedBy"] = "acknowledged_tariff".into();
                    } else {
                        owner_unpriced(&mut check);
                    }
                    checks.push(check);
                }
                RuntimeModelPolicy::Passthrough => {
                    // Caller-chosen models cannot be enumerated safely. Each
                    // request still proves its exact execution model and tier.
                    checks.push(serde_json::json!({"code":"model_price_coverage","ready":true,"app":binding.app,
                        "providerId":binding.provider_id,"profileId":plan.profile_id.as_str(),
                        "driverId":plan.driver_id.as_str(),"scope":"request_time"}));
                }
            }
        }
        Ok(checks)
    }

    pub(crate) fn spawn_value_tariff_activation(state: ServerState) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if state.is_shutting_down() {
                    return;
                }
                let config = state.config_snapshot().await;
                let Ok(identity) = scope(&config, "pricing", "pricing@local.invalid") else {
                    continue;
                };
                let dir = state.config_dir.clone();
                let activation = identity.clone();
                let result = tokio::task::spawn_blocking(move || {
                    if !server_sqlite::is_committed(&dir)? {
                        return Ok(false);
                    }
                    ledger::activate_due_tariff(&dir, &activation, now_ms_i64())?;
                    ledger::reprice_due(&dir, &activation)
                })
                .await;
                match result {
                    Ok(Ok(true)) => state.reprice_unpriced_value_usage(&config, identity).await,
                    Ok(Err(error)) => {
                        tracing::debug!(%error,"local value tariff activation pending")
                    }
                    _ => {}
                }
            }
        });
    }

    /// Owner-managed `$0` attempts inside the open window of each grant are
    /// re-priced after a covering tariff activates. Bounded per tick; the
    /// durable cursor resumes the job on the next tick or after a restart.
    async fn reprice_unpriced_value_usage(&self, config: &ServerConfig, identity: ledger::Scope) {
        let now = now_ms_i64();
        let windows = {
            let shares = self.shares.read().await;
            reprice_windows(&shares, config, now)
        };
        let dir = self.config_dir.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
            let mut repriced = 0;
            for _ in 0..10 {
                let page = ledger::reprice_page(&dir, &identity, &windows, now_ms_i64())?;
                repriced += page.repriced;
                if page.done {
                    break;
                }
            }
            Ok(repriced)
        })
        .await;
        match result {
            Ok(Ok(0)) => {}
            Ok(Ok(repriced)) => {
                self.value_watermark_dirty.notify_one();
                tracing::info!(
                    repriced,
                    "unpriced USD-equivalent usage re-priced after a tariff activation"
                )
            }
            Ok(Err(error)) => {
                tracing::warn!(%error, "unpriced USD-equivalent re-pricing will resume on the next tick")
            }
            Err(error) => tracing::warn!(%error, "unpriced USD-equivalent re-pricing task failed"),
        }
    }

    pub(crate) async fn report_active_value_tariff(
        &self,
        config: &ServerConfig,
    ) -> anyhow::Result<()> {
        let identity = config
            .registered_router_identity()
            .context("Router identity unavailable")?;
        let base = config.router_api_base().context("Router URL unavailable")?;
        let pricing_scope = scope(config, "pricing", "pricing@local.invalid")?;
        let Some(revision) = ledger::active_revision(&self.config_dir, &pricing_scope)? else {
            return Ok(());
        };
        let request = client::signed_canonical_request(
            identity,
            "share_value_pricing_active",
            serde_json::json!({"revision":revision,"storageSchema":6}),
        )?;
        let response: Value = self
            .http_client()
            .await
            .post(format!(
                "{}/v1/shares/value-pricing/active",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(10))
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        anyhow::ensure!(
            response["revision"].as_str() == Some(&revision),
            "value pricing activation receipt mismatch"
        );
        Ok(())
    }
}
