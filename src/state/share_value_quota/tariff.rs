//! Pricing readiness and offline activation, kept separate from user quotas.
use super::*;
use crate::domain::providers::runtime::RuntimeModelPolicy;

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
                    let coverage = ledger::model_coverage(
                        &self.config_dir,
                        &identity,
                        upstream_model,
                        "standard",
                        now,
                    )?;
                    let mut check = serde_json::to_value(coverage)?;
                    check["code"] = "model_price_coverage".into();
                    check["app"] = serde_json::to_value(binding.app)?;
                    check["scope"] = "configured_model".into();
                    if check["activePricingRevision"].is_null() {
                        check["blockedBy"] = "acknowledged_tariff".into();
                    }
                    checks.push(check);
                }
                RuntimeModelPolicy::Passthrough => {
                    // Caller-chosen models cannot be enumerated safely. Each
                    // request still proves its exact execution model and tier.
                    checks.push(serde_json::json!({"code":"model_price_coverage","ready":true,"app":binding.app,"scope":"request_time"}));
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
                let result = tokio::task::spawn_blocking(move || {
                    if !server_sqlite::is_committed(&dir)? {
                        return Ok(());
                    }
                    ledger::activate_due_tariff(&dir, &identity, now_ms_i64()).map(|_| ())
                })
                .await;
                if let Ok(Err(error)) = result {
                    tracing::debug!(%error,"local value tariff activation pending");
                }
            }
        });
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
            serde_json::json!({"revision":revision,"storageSchema":3}),
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
