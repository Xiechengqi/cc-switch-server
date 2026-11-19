//! Independent execution authority and signed, side-effect-free configuration previews.
use super::*;
use crate::domain::providers::share_operations::implemented;
use crate::domain::sharing::capabilities::*;
use crate::domain::sharing::capability_intent::{binding_fingerprint, grant_binding_fingerprint};
use crate::domain::sharing::router_contract::{
    ShareGrantManager, ShareSettingsPatch, ShareUserPolicy,
};
use crate::domain::usage::value_pricing::{PricingSnapshot, ResourcePlan};
use crate::infra::time::now_ms;
use crate::repository::server_sqlite::value_quota as ledger;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

mod grants;
pub(crate) use grants::migrate_grant_capabilities;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityPreviewInput {
    pub expected_config_revision: u64,
    #[serde(default)]
    pub share_token_limit: Option<i64>,
    #[serde(default)]
    pub policy: Option<ShareCapabilityPolicy>,
    #[serde(default)]
    pub policies: BTreeMap<String, ShareUserPolicy>,
    #[serde(default)]
    pub replace_policies: bool,
}
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewTicket {
    domain: String,
    share_id: String,
    revision: u64,
    target: String,
    conditions: String,
    expires_at_ms: i64,
}
fn digest(value: &impl Serialize) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(value).expect("serializable capability facts"),
    ))
}
fn secret(config: &ServerConfig) -> anyhow::Result<&str> {
    config
        .registered_router_identity()
        .map(|i| i.private_key.as_str())
        .context("CAPABILITY_UPGRADE_REQUIRED")
}
fn sign(config: &ServerConfig, ticket: &PreviewTicket) -> anyhow::Result<String> {
    let raw = serde_json::to_vec(ticket)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret(config)?.as_bytes())?;
    mac.update(b"share-capability-preview-v1\0");
    mac.update(&raw);
    Ok(format!(
        "{}.{}",
        hex::encode(&raw),
        hex::encode(mac.finalize().into_bytes())
    ))
}
fn verify(config: &ServerConfig, credential: &str) -> anyhow::Result<PreviewTicket> {
    anyhow::ensure!(credential.len() <= 8192, "CAPABILITY_PREVIEW_EXPIRED");
    let (raw, signature) = credential
        .split_once('.')
        .context("CAPABILITY_PREVIEW_EXPIRED")?;
    let raw = hex::decode(raw).map_err(|_| anyhow::anyhow!("CAPABILITY_PREVIEW_EXPIRED"))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret(config)?.as_bytes())?;
    mac.update(b"share-capability-preview-v1\0");
    mac.update(&raw);
    let signature =
        hex::decode(signature).map_err(|_| anyhow::anyhow!("CAPABILITY_PREVIEW_EXPIRED"))?;
    mac.verify_slice(&signature)
        .map_err(|_| anyhow::anyhow!("CAPABILITY_PREVIEW_EXPIRED"))?;
    let ticket: PreviewTicket =
        serde_json::from_slice(&raw).map_err(|_| anyhow::anyhow!("CAPABILITY_PREVIEW_EXPIRED"))?;
    anyhow::ensure!(
        ticket.domain == "share-capability-preview-v1" && ticket.expires_at_ms > now_ms() as i64,
        "CAPABILITY_PREVIEW_EXPIRED"
    );
    Ok(ticket)
}
fn app_name(app: AppKind) -> String {
    serde_json::to_value(app).unwrap().as_str().unwrap().into()
}

fn unit(share: &Share, policy: Option<&ShareUserPolicy>) -> &'static str {
    if policy.is_some_and(|p| p.value_quota.is_some()) {
        "usdEquivalent"
    } else if share.token_limit.is_some() || policy.is_some_and(|p| p.token_limit.is_some()) {
        "tokens"
    } else {
        "unlimited"
    }
}
fn condition(scope: &str, unit: &str, op: ShareOperation, binding: &str, gap: &str) -> String {
    // Runtime fingerprint includes model policy and collector version. A price amount
    // update does not invalidate consent; a new resource dimension or model range does.
    digest(&(CAPABILITY_CATALOG_VERSION, scope, unit, op, binding, gap))
}
fn generic_resource_priced(
    snapshot: Option<&PricingSnapshot>,
    model: Option<&str>,
    op: ShareOperation,
) -> bool {
    model
        .and_then(|m| snapshot.and_then(|s| s.model(m).ok()))
        .is_some_and(|m| m.resources.contains_key(op.resource()))
}

/// Display evidence only. Admission still prices the actual request dimensions.
fn pricing_coverage(
    snapshot: Option<&PricingSnapshot>,
    model: Option<&str>,
    op: ShareOperation,
) -> &'static str {
    let Some(snapshot) = snapshot else {
        return "tariff_unavailable";
    };
    let Some(model) = model else {
        return "request_dependent";
    };
    let Ok(model) = snapshot.model(model) else {
        return "missing";
    };
    if model.resources.contains_key(op.resource()) {
        "base_rate"
    } else if model.resources.keys().any(|key| {
        key.strip_prefix(op.resource())
            .is_some_and(|tail| tail.starts_with('/'))
    }) {
        "parameter_rates"
    } else {
        "missing"
    }
}
fn coverage_gap(quota_unit: &str, priced: bool) -> Option<&'static str> {
    match quota_unit {
        "tokens" => Some("resource_not_in_token_ledger"),
        "usdEquivalent" if !priced => Some("model_or_resource_price_unknown"),
        // Estimated quantities remain explicitly labelled soft-limit coverage.
        "usdEquivalent" | "unlimited" => None,
        _ => Some("measurement_unknown"),
    }
}
fn scope_blocker(p: Option<&ShareUserPolicy>, _priced: bool) -> Option<&'static str> {
    p.and_then(|p| p.value_quota.as_ref()).and_then(|q| {
        if q.tariff_policy_id != crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY {
            Some("VALUE_QUOTA_RESOURCE_POLICY_REQUIRED")
        } else {
            None
        }
    })
}
fn policies(
    share: &Share,
    changes: &BTreeMap<String, ShareUserPolicy>,
    replace: bool,
) -> BTreeMap<String, ShareUserPolicy> {
    if replace {
        return changes.clone();
    }
    let mut policies = share
        .user_grants
        .iter()
        .filter(|(_, g)| g.active)
        .map(|(email, g)| (email.clone(), g.policy.clone()))
        .collect::<BTreeMap<_, _>>();
    policies.extend(changes.clone());
    policies
}
fn target(
    policy: &ShareCapabilityPolicy,
    policies: &BTreeMap<String, ShareUserPolicy>,
    pricing: Option<&str>,
) -> String {
    // The server owns acknowledgement audit fields, never the browser.
    let mut policy = policy.clone();
    policy.coverage_acknowledgements.clear();
    digest(&(policy, policies, pricing))
}
fn tariff(
    config_dir: &std::path::Path,
    config: &ServerConfig,
    share: &Share,
) -> anyhow::Result<Option<(String, PricingSnapshot)>> {
    let Some(identity) = config.registered_router_identity() else {
        return Ok(None);
    };
    let Some(base) = config.router_api_base() else {
        return Ok(None);
    };
    ledger::capability_snapshot(
        config_dir,
        &ledger::Scope {
            router_url: base.into(),
            installation_id: identity.installation_id.clone(),
            share_id: share.id.clone(),
            email: "preview@local.invalid".into(),
            entitlement_id: None,
        },
    )
}

impl ServerStateInner {
    pub(crate) async fn capability_policy_router_negotiated(&self) -> bool {
        let config = self.config_snapshot().await;
        self.router_quota_capability
            .read()
            .await
            .as_ref()
            .is_some_and(|(base, id, at, c)| {
                c.share_capabilities
                    && at.elapsed() < std::time::Duration::from_secs(300)
                    && config.router_api_base() == Some(base.as_str())
                    && config
                        .registered_router_identity()
                        .is_some_and(|i| i.installation_id == *id)
            })
    }
    pub async fn preview_share_permissions(
        &self,
        share_id: &str,
        input: Value,
    ) -> anyhow::Result<Value> {
        if let Some(request) = input.get("requestId").and_then(Value::as_str) {
            return self.share_request_metering(share_id, request).await;
        }
        if input.get("grantCapabilityEdits").is_some() || input.get("subject").is_some() {
            self.preview_grant_capabilities(share_id, serde_json::from_value(input)?)
                .await
        } else {
            self.preview_share_capabilities(share_id, serde_json::from_value(input)?)
                .await
        }
    }

    pub async fn share_request_metering(
        &self,
        share_id: &str,
        request: &str,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.shares.read().await.get(share_id).is_some(),
            "Share not found"
        );
        let config = self.config_snapshot().await;
        let identity = config
            .registered_router_identity()
            .context("CAPABILITY_UPGRADE_REQUIRED")?;
        let attempts = crate::repository::server_sqlite::capability_context::request_metering(
            &self.config_dir,
            config.router_api_base().unwrap_or_default(),
            &identity.installation_id,
            share_id,
            request,
        )?;
        Ok(serde_json::json!({"shareId":share_id,"requestId":request,"rows":attempts}))
    }

    pub async fn preview_share_capabilities(
        &self,
        share_id: &str,
        input: CapabilityPreviewInput,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            input.policies.len() <= 500,
            "invalid capability preview size"
        );
        let mut share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        anyhow::ensure!(
            share.grant_capabilities.is_none(),
            "CAPABILITY_UPGRADE_REQUIRED"
        );
        if let Some(limit) = input.share_token_limit {
            share.token_limit = (limit >= 0).then_some(limit as u64);
        }
        anyhow::ensure!(
            share.config_revision == input.expected_config_revision,
            "CAPABILITY_STATE_CHANGED"
        );
        let config = self.config_snapshot().await;
        let providers = self.providers.read().await.clone();
        let accounts = self.accounts.read().await.clone();
        let committed =
            crate::repository::server_sqlite::is_committed(&self.config_dir).unwrap_or(false);
        let ready = committed && self.capability_policy_router_negotiated().await;
        anyhow::ensure!(
            share.grant_capabilities.is_none(),
            "CAPABILITY_UPGRADE_REQUIRED"
        );
        let mut legacy = ShareCapabilityPolicy::default();
        // Only the old explicit Grok permits are migration evidence. Never use
        // v2 pricing, history or a different runtime as a permission source.
        for binding in &share.bindings {
            if let Some(runtime) = providers
                .runtime_plan(binding.app, &binding.provider_id)
                .filter(|r| r.driver_id.as_str() == "oauth.grok_responses")
            {
                for (enabled, op) in [
                    (
                        share.grok_media_policy.image_generation_enabled,
                        ShareOperation::ImageGenerate,
                    ),
                    (
                        share.grok_media_policy.image_edit_enabled,
                        ShareOperation::ImageEdit,
                    ),
                    (
                        share.grok_media_policy.video_generation_enabled,
                        ShareOperation::VideoGenerate,
                    ),
                ] {
                    if enabled {
                        if !legacy.allowed_operations.contains(&op) {
                            legacy.allowed_operations.push(op);
                        }
                        legacy
                            .operation_bindings
                            .entry(op)
                            .or_default()
                            .push(OperationBinding {
                                app: app_name(binding.app),
                                fingerprint: binding_fingerprint(&runtime),
                            });
                    }
                }
            }
        }
        let mut policy = input
            .policy
            .or_else(|| share.capability_policy.clone())
            .unwrap_or(legacy);
        policy.validate().map_err(anyhow::Error::msg)?;
        policy.coverage_acknowledgements.clear();
        let policies = policies(&share, &input.policies, input.replace_policies);
        // An independent capability edit must cover both currently enforced
        // and already scheduled units without activating the future quota.
        let mut coverage_policies = policies
            .iter()
            .map(|(email, policy)| (email.as_str(), policy.clone()))
            .collect::<Vec<_>>();
        for (email, requested) in &policies {
            if let Some(grant) = share.user_grants.get(email) {
                if let Some(scheduled) = scheduled_capability_policy(grant, requested) {
                    let mut active = grant.policy.clone();
                    active.allowed_operations = requested.allowed_operations.clone();
                    if requested.value_quota.as_ref().is_some_and(|q| {
                        q.tariff_policy_id
                            == crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY
                    }) {
                        if let Some(quota) = active.value_quota.as_mut() {
                            quota.tariff_policy_id =
                                crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY.into();
                        }
                    }
                    for extra in [active, scheduled] {
                        if !coverage_policies
                            .iter()
                            .any(|(scope, policy)| *scope == email.as_str() && policy == &extra)
                        {
                            coverage_policies.push((email.as_str(), extra));
                        }
                    }
                }
            }
        }
        let snapshot = if committed {
            tariff(&self.config_dir, &config, &share)?
        } else {
            None
        };
        let mut rows = Vec::new();
        let mut unavailable_bindings = Vec::new();
        let mut acknowledgements = Vec::new();
        let requested_bindings = policy.operation_bindings.clone();
        policy.operation_bindings = policy
            .allowed_operations
            .iter()
            .map(|op| (*op, Vec::new()))
            .collect();
        for binding in &share.bindings {
            let Some(plan) = providers.runtime_plan(binding.app, &binding.provider_id) else {
                unavailable_bindings.push(
                    serde_json::json!({"app":app_name(binding.app),"reason":"runtime_unavailable"}),
                );
                continue;
            };
            if plan.configuration_state
                == crate::domain::providers::runtime::RuntimeConfigurationState::NeedsAttention
            {
                unavailable_bindings.push(
                    serde_json::json!({"app":app_name(binding.app),"reason":"runtime_unavailable"}),
                );
            }
            let fingerprint = binding_fingerprint(&plan);
            let app = app_name(binding.app);
            for op in ShareOperation::ALL {
                let supported = implemented(&plan, op);
                let driver_implemented =
                    crate::domain::providers::share_operations::driver_implements(&plan, op);
                let provider_enabled =
                    crate::domain::providers::share_operations::provider_enabled(&plan, op);
                let observed = crate::domain::providers::share_operations::observed_account_state(
                    &plan,
                    &accounts,
                    op,
                    now_ms() as i64,
                );
                let execution = if !supported {
                    "unsupported"
                } else {
                    match observed {
                    Some(crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Supported)=>"supported",
                    Some(crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Unsupported)=>"unsupported",
                    _=>"unknown",
                }
                };
                let account_support = match observed {
                    Some(crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Supported) => "supported",
                    Some(crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Unsupported) => "unsupported",
                    _ => "unknown",
                };
                let unavailable_reason = if !driver_implemented {
                    Some("driver_not_implemented")
                } else if !provider_enabled {
                    Some("provider_disabled")
                } else if plan.configuration_state
                    == crate::domain::providers::runtime::RuntimeConfigurationState::NeedsAttention
                {
                    Some("runtime_unavailable")
                } else if account_support == "unsupported" {
                    Some("account_unsupported")
                } else {
                    None
                };
                let binding_selected = unavailable_reason.is_none()
                    && policy.allowed_operations.contains(&op)
                    && requested_bindings.get(&op).is_none_or(|bindings| {
                        bindings
                            .iter()
                            .any(|b| b.app == app && b.fingerprint == fingerprint)
                    });
                if binding_selected {
                    policy
                        .operation_bindings
                        .entry(op)
                        .or_default()
                        .push(OperationBinding {
                            app: app.clone(),
                            fingerprint: fingerprint.clone(),
                        });
                }
                let model = match &plan.model_policy {
                    crate::domain::providers::runtime::RuntimeModelPolicy::Single {
                        upstream_model,
                    } => Some(upstream_model.as_str()),
                    _ => None,
                };
                let priced = generic_resource_priced(snapshot.as_ref().map(|(_, s)| s), model, op);
                let mut scopes = Vec::new();
                for (scope, p) in std::iter::once(("$default", None))
                    .chain(coverage_policies.iter().map(|(e, p)| (*e, Some(p))))
                {
                    if p.and_then(|p| p.allowed_operations.as_ref())
                        .is_some_and(|ops| !ops.contains(&op))
                    {
                        continue;
                    }
                    let quota_unit = unit(&share, p);
                    let strict = p.and_then(|p| p.value_quota.as_ref()).is_some_and(|q| {
                        q.unpriced_usage
                            == crate::domain::usage::value_pricing::UnpricedUsage::Reject
                    });
                    let gap = if strict {
                        None
                    } else {
                        coverage_gap(quota_unit, priced)
                    };
                    let blocker = scope_blocker(p, priced);
                    let reason = if strict && !priced {
                        "priced_requests_only"
                    } else {
                        gap.unwrap_or(if quota_unit == "unlimited" {
                            "no_usage_limit"
                        } else {
                            "estimated_resource_quantity"
                        })
                    };
                    let identity = gap.map(|g| condition(scope, quota_unit, op, &fingerprint, g));
                    scopes.push(serde_json::json!({"scope":scope,"quotaUnit":quota_unit,
                        "coverage":if quota_unit=="unlimited"{"unlimited"}else if strict&&!priced{"unknown"}else if gap.is_none(){"full"}else if quota_unit=="tokens"{"partial"}else{"unknown"},
                        "reason":reason,"condition":identity,"blocker":blocker}));
                    if binding_selected {
                        if let Some(condition) = identity {
                            acknowledgements.push(CoverageAcknowledgement {
                                condition,
                                scope: scope.into(),
                                operation: op,
                                quota_unit: quota_unit.into(),
                                binding_fingerprint: fingerprint.clone(),
                                actor: String::new(),
                                accepted_at_ms: 0,
                            });
                        }
                    }
                }
                rows.push(serde_json::json!({"app":app,"operation":op,"bindingFingerprint":fingerprint,"execution":execution,"controllable":supported,"selected":binding_selected,
                    "implemented":driver_implemented,"providerEnabled":provider_enabled,"accountSupport":account_support,"unavailableReason":unavailable_reason,
                    "requiredProviderOption":crate::domain::providers::share_operations::required_provider_option(&plan,op),
                    "providerLabel":crate::domain::providers::registry::profile_by_id(plan.profile_id.as_str()).map(|p|p.label.as_str()),
                    "modelScope":if model.is_some(){"fixed"}else{"request"},"pricingCoverage":pricing_coverage(snapshot.as_ref().map(|(_,s)|s),model,op),
                    "source":"driver_contract","liveValidation":if execution=="supported"{"observed_account_evidence"}else{"live_pending"},"model":model,"measurement":if supported{"estimated"}else{"unavailable"},"nativeUnit":op.resource(),"pricing":if !priced{"missing"}else if model.and_then(|model|snapshot.as_ref().and_then(|(_,s)|s.model(model).ok())).and_then(|m|m.resources.get(op.resource())).is_some_and(|rate|rate=="0"){"explicit_free"}else{"priced"},"admission":if supported{"estimated_reservation"}else{"unavailable"},"scopes":scopes}));
            }
        }
        let blockers: Vec<Value> = rows
            .iter()
            .filter(|row| row["selected"] == true)
            .filter(|row| {
                serde_json::from_value::<ShareOperation>(row["operation"].clone())
                    .ok()
                    .is_some_and(|op| policy.allowed_operations.contains(&op))
            })
            .flat_map(|row| {
                row["scopes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|scope| !scope["blocker"].is_null())
                    .cloned()
            })
            .collect();
        // A policy may keep unavailable intent, but it receives no execution binding.
        let now = now_ms() as i64;
        anyhow::ensure!(
            acknowledgements.len() <= 16_384,
            "CAPABILITY_POLICY_TOO_LARGE"
        );
        acknowledgements.sort_by(|a, b| a.condition.cmp(&b.condition));
        acknowledgements.dedup_by(|a, b| a.condition == b.condition);
        let ticket = PreviewTicket {
            domain: "share-capability-preview-v1".into(),
            share_id: share.id.clone(),
            revision: share.config_revision,
            target: target(
                &policy,
                &policies,
                snapshot.as_ref().map(|(r, _)| r.as_str()),
            ),
            conditions: digest(
                &acknowledgements
                    .iter()
                    .map(|a| &a.condition)
                    .collect::<Vec<_>>(),
            ),
            expires_at_ms: now + 900_000,
        };
        let recent_usage = config
            .registered_router_identity()
            .map(|id| {
                crate::repository::server_sqlite::capability_context::recent(
                    &self.config_dir,
                    config.router_api_base().unwrap_or_default(),
                    &id.installation_id,
                    share_id,
                )
            })
            .transpose()?
            .unwrap_or_default();
        let incidents = config
            .registered_router_identity()
            .map(|id| {
                crate::repository::server_sqlite::capability_context::incidents(
                    &self.config_dir,
                    config.router_api_base().unwrap_or_default(),
                    &id.installation_id,
                    share_id,
                )
            })
            .transpose()?
            .unwrap_or_default();
        let credential = if ready {
            Some(sign(&config, &ticket)?)
        } else {
            None
        };
        Ok(
            serde_json::json!({"shareId":share_id,"configRevision":share.config_revision,"catalogVersion":CAPABILITY_CATALOG_VERSION,"displayVersion":2,"unavailableBindings":unavailable_bindings,"ready":ready,
            "legacy":share.capability_policy.is_none(),"policy":policy,"rows":rows,"requiredAcknowledgements":acknowledgements,"blockers":blockers,"recentUsage":recent_usage,"incidents":incidents,
            "previewCredential":credential,"generatedAt":now,"expiresAt":ticket.expires_at_ms,"pricingRevision":snapshot.map(|(r,_)|r),
            "reason":if ready{None}else{Some("CAPABILITY_UPGRADE_REQUIRED")}}),
        )
    }

    /// Returns the server-normalized patch. Called before all native/Router settings writes;
    /// the normal mutation transaction still rechecks config/grant revisions and receipts.
    pub(crate) async fn prepare_capability_patch(
        &self,
        share_id: &str,
        mut patch: ShareSettingsPatch,
    ) -> anyhow::Result<ShareSettingsPatch> {
        let share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        if share.grant_capabilities.is_some() {
            return self.prepare_grant_capability_patch(&share, patch).await;
        }
        anyhow::ensure!(
            patch.grant_capability_edits.is_none(),
            "CAPABILITY_UPGRADE_REQUIRED"
        );
        let mut changed = policies(&share, &BTreeMap::new(), false);
        if let Some(grants) = &patch.user_grants {
            changed.retain(|email,_|share.user_grants.get(email).is_some_and(|g|g.manager==crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket || g.role=="owner"));
            changed.extend(
                grants
                    .iter()
                    .filter(|(_, g)| g.active)
                    .map(|(e, g)| (e.clone(), g.policy.clone())),
            );
        }
        if let Some(edits) = &patch.user_grant_edits {
            for (email, edit) in edits {
                if let Some(p) = &edit.policy {
                    changed.insert(email.clone(), p.clone());
                } else {
                    changed.remove(email);
                }
            }
        }
        if let Some(policy) = patch
            .capability_policy
            .as_ref()
            .or(share.capability_policy.as_ref())
        {
            for p in changed.values() {
                if let Some(ops) = &p.allowed_operations {
                    anyhow::ensure!(
                        ops.iter().all(|op| policy.allowed_operations.contains(op)),
                        "SHARE_CAPABILITY_DISABLED"
                    );
                }
            }
        }
        if patch.capability_policy.is_none() {
            anyhow::ensure!(
                patch.capability_consent.is_none(),
                "CAPABILITY_STATE_CHANGED"
            );
            return Ok(patch);
        }
        anyhow::ensure!(patch.edit_context.is_some(), "CAPABILITY_STATE_CHANGED");
        let preview = self
            .preview_share_capabilities(
                share_id,
                CapabilityPreviewInput {
                    expected_config_revision: patch
                        .edit_context
                        .as_ref()
                        .unwrap()
                        .expected_config_revision,
                    policy: patch.capability_policy.clone(),
                    policies: changed,
                    replace_policies: true,
                    share_token_limit: patch.token_limit,
                },
            )
            .await?;
        anyhow::ensure!(preview["ready"] == true, "CAPABILITY_UPGRADE_REQUIRED");
        anyhow::ensure!(
            preview["blockers"].as_array().is_none_or(|b| b.is_empty()),
            "QUOTA_COVERAGE_REQUIRED"
        );
        let config = self.config_snapshot().await;
        let consent = patch
            .capability_consent
            .as_ref()
            .context("CAPABILITY_PREVIEW_EXPIRED")?;
        let ticket = verify(&config, &consent.credential)?;
        let current = verify(&config, preview["previewCredential"].as_str().unwrap())?;
        anyhow::ensure!(
            ticket.share_id == current.share_id
                && ticket.revision == current.revision
                && ticket.target == current.target
                && ticket.conditions == current.conditions,
            "CAPABILITY_STATE_CHANGED"
        );
        let mut policy: ShareCapabilityPolicy = serde_json::from_value(preview["policy"].clone())?;
        let mut required: Vec<CoverageAcknowledgement> =
            serde_json::from_value(preview["requiredAcknowledgements"].clone())?;
        let actor = patch
            .edit_context
            .as_ref()
            .and_then(|c| c.actor.clone())
            .unwrap_or_else(|| "server-admin".into());
        for acknowledgement in &mut required {
            let accepted = consent
                .accept_uncovered
                .contains(&acknowledgement.condition)
                || share.capability_policy.as_ref().is_some_and(|p| {
                    p.coverage_acknowledgements
                        .iter()
                        .any(|old| old.condition == acknowledgement.condition)
                });
            anyhow::ensure!(accepted, "QUOTA_COVERAGE_REQUIRED");
            if let Some(old) = share.capability_policy.as_ref().and_then(|p| {
                p.coverage_acknowledgements
                    .iter()
                    .find(|a| a.condition == acknowledgement.condition)
            }) {
                *acknowledgement = old.clone();
            } else {
                acknowledgement.actor = actor.clone();
                acknowledgement.accepted_at_ms = now_ms() as i64;
            }
        }
        // Existing legacy market contracts have no provable frozen operation set.
        for grant in share.user_grants.values().filter(|g| {
            g.active
                && g.manager
                    == crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket
        }) {
            let ops = grant
                .policy
                .allowed_operations
                .as_ref()
                .context("MARKET_CAPABILITY_CONFLICT")?;
            anyhow::ensure!(
                ops.iter().all(|op| policy.allowed_operations.contains(op)),
                "MARKET_CAPABILITY_CONFLICT"
            );
        }
        policy.coverage_acknowledgements = required;
        patch.validated_capability_policy = Some(policy);
        Ok(patch)
    }

    pub(crate) fn validate_capability_transition(
        &self,
        previous: &ShareStore,
        candidate: &ShareStore,
        providers: &ProviderStore,
        config: &ServerConfig,
    ) -> anyhow::Result<()> {
        for share in &candidate.shares {
            let before = previous.get(&share.id);
            if let Some(permissions) = &share.grant_capabilities {
                grants::validate_grant_transition(before, share, permissions, providers)?;
                continue;
            }
            anyhow::ensure!(
                before.is_none_or(|before| before.grant_capabilities.is_none()),
                "CAPABILITY_UPGRADE_REQUIRED"
            );
            anyhow::ensure!(
                before.is_none_or(|s| s.capability_policy.is_none())
                    || share.capability_policy.is_some(),
                "CAPABILITY_UPGRADE_REQUIRED"
            );
            let Some(policy) = &share.capability_policy else {
                continue;
            };
            policy.validate().map_err(anyhow::Error::msg)?;
            let active = |s: &Share| {
                s.user_grants
                    .iter()
                    .filter(|(_, g)| g.active)
                    .map(|(e, g)| (e.clone(), (g.policy.clone(), g.pending_value_quota.clone())))
                    .collect::<BTreeMap<_, _>>()
            };
            let changed = before.is_none_or(|s| {
                s.capability_policy != share.capability_policy
                    || s.token_limit != share.token_limit
                    || active(s) != active(share)
            });
            if !changed {
                continue;
            }
            for grant in share.user_grants.values().filter(|g|g.active&&g.manager==crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket) {
                let ops=grant.policy.allowed_operations.as_ref().context("MARKET_CAPABILITY_CONFLICT")?;
                anyhow::ensure!(ops.iter().all(|op|policy.allowed_operations.contains(op)),"MARKET_CAPABILITY_CONFLICT");
            }
            if policy.allowed_operations.is_empty() {
                continue;
            }
            let snapshot = tariff(&self.config_dir, config, share)?;
            for binding in &share.bindings {
                let Some(runtime) = providers.runtime_plan(binding.app, &binding.provider_id)
                else {
                    continue;
                };
                let fingerprint = binding_fingerprint(&runtime);
                let model = match &runtime.model_policy {
                    crate::domain::providers::runtime::RuntimeModelPolicy::Single {
                        upstream_model,
                    } => Some(upstream_model.as_str()),
                    _ => None,
                };
                for op in &policy.allowed_operations {
                    if !policy.operation_bindings.get(op).is_some_and(|bindings| {
                        bindings
                            .iter()
                            .any(|b| b.app == app_name(binding.app) && b.fingerprint == fingerprint)
                    }) {
                        continue;
                    }
                    let priced =
                        generic_resource_priced(snapshot.as_ref().map(|(_, s)| s), model, *op);
                    for (scope, p) in
                        std::iter::once(("$default", None)).chain(
                            share.user_grants.iter().filter(|(_, g)| g.active).flat_map(
                                |(e, g)| {
                                    std::iter::once((e.as_str(), Some(&g.policy))).chain(
                                        g.pending_value_quota
                                            .as_ref()
                                            .map(|pending| (e.as_str(), Some(&pending.policy))),
                                    )
                                },
                            ),
                        )
                    {
                        if p.and_then(|p| p.allowed_operations.as_ref())
                            .is_some_and(|ops| !ops.contains(op))
                        {
                            continue;
                        }
                        if let Some(blocker) = scope_blocker(p, priced) {
                            anyhow::bail!(blocker);
                        }
                        if p.and_then(|p| p.value_quota.as_ref()).is_some_and(|q| {
                            q.unpriced_usage
                                == crate::domain::usage::value_pricing::UnpricedUsage::Reject
                        }) {
                            continue;
                        }
                        let quota_unit = unit(share, p);
                        if let Some(gap) = coverage_gap(quota_unit, priced) {
                            let condition = condition(scope, quota_unit, *op, &fingerprint, gap);
                            anyhow::ensure!(
                                policy
                                    .coverage_acknowledgements
                                    .iter()
                                    .any(|a| a.condition == condition
                                        && !a.actor.is_empty()
                                        && a.accepted_at_ms > 0),
                                "QUOTA_COVERAGE_REQUIRED"
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
    /// Called while holding the same short commit gate as policy writes. Independent of
    /// model pricing success, user quota unit, owner privileges, and health-check flags.
    pub(crate) fn check_share_capabilities(
        &self,
        share: &Share,
        context: &UsageLogContext,
        model: &str,
        resource_plan: Option<&ResourcePlan>,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        config: &ServerConfig,
    ) -> anyhow::Result<()> {
        let email = context.user_email.as_deref();
        let app = context.quota_app;
        let operation = context.operation;
        if share.grant_capabilities.is_none() && share.capability_policy.is_none() {
            return Ok(());
        }
        if let Some(policy) = &share.capability_policy {
            policy.validate().map_err(anyhow::Error::msg)?;
        }
        anyhow::ensure!(
            resource_plan
                .and_then(|p| p.invalid_reason.as_ref())
                .is_none(),
            "CAPABILITY_REQUEST_INVALID"
        );
        anyhow::ensure!(
            !resource_plan.is_some_and(|p| p.continuation),
            "CAPABILITY_CONTEXT_REQUIRED"
        );
        let mut operations = resource_plan
            .map(|p| p.operations.clone())
            .unwrap_or_default();
        operations.extend(crate::domain::sharing::capability_intent::operations(
            &serde_json::Value::Null,
            operation,
        ));
        if !matches!(
            operation,
            crate::domain::usage::store::InferenceOperation::ImageEdit
        ) {
            operations.extend(
                crate::domain::sharing::capability_intent::model_operations_for_request(
                    model,
                    resource_plan,
                ),
            );
        }
        operations.sort();
        operations.dedup();
        if operations.is_empty() && share.grant_capabilities.is_some() {
            return Ok(());
        }
        let app = app.context("CAPABILITY_STATE_CHANGED")?;
        let binding = share
            .bindings
            .iter()
            .find(|b| b.app == app)
            .context("CAPABILITY_STATE_CHANGED")?;
        let runtime = providers
            .runtime_plan(app, &binding.provider_id)
            .context("CAPABILITY_STATE_CHANGED")?;
        let fingerprint = binding_fingerprint(&runtime);
        anyhow::ensure!(
            resource_plan.and_then(|p| p.binding_fingerprint.as_ref()) == Some(&fingerprint),
            "CAPABILITY_STATE_CHANGED"
        );
        if operations.is_empty() {
            return Ok(());
        }
        let grant = email
            .and_then(|email| share.user_grants.get(&email.to_ascii_lowercase()))
            .filter(|g| g.active);
        let scope = grant.map(|g| g.email.as_str()).unwrap_or("$default");
        let quota_unit = unit(share, grant.map(|g| &g.policy));
        if let Some(permissions) = &share.grant_capabilities {
            let grant = grant.context("SHARE_CAPABILITY_DISABLED")?;
            let policy = permissions
                .get(scope)
                .context("SHARE_CAPABILITY_DISABLED")?;
            let fingerprint = grant_binding_fingerprint(share, app, &runtime);
            anyhow::ensure!(
                crate::domain::sharing::shares::share_app_api_enabled(share, app)
                    && (grant.policy.allowed_apps.is_empty()
                        || grant.policy.allowed_apps.contains(&app)),
                "SHARE_CAPABILITY_DISABLED"
            );
            for op in operations {
                anyhow::ensure!(
                    policy.allowed_operations.contains(&op)
                        && (grant.manager != ShareGrantManager::RouterShareMarket
                            || grant
                                .policy
                                .allowed_operations
                                .as_ref()
                                .is_some_and(|ops| ops.contains(&op))),
                    "SHARE_CAPABILITY_DISABLED"
                );
                anyhow::ensure!(implemented(&runtime, op)
                    && runtime.configuration_state != crate::domain::providers::runtime::RuntimeConfigurationState::NeedsAttention
                    && accounts.and_then(|accounts| crate::domain::providers::share_operations::observed_account_state(
                        &runtime, accounts, op, now_ms() as i64
                    )) != Some(crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Unsupported),
                    "CAPABILITY_NOT_SUPPORTED");
                anyhow::ensure!(
                    policy
                        .operation_bindings
                        .get(&op)
                        .is_some_and(|bindings| bindings
                            .iter()
                            .any(|b| b.app == app_name(app) && b.fingerprint == fingerprint)),
                    "CAPABILITY_STATE_CHANGED"
                );
                if quota_unit == "usdEquivalent"
                    && !resource_plan.is_some_and(|plan| {
                        plan.requires_v2
                            && plan.quantities.iter().any(|(dimension, quantity)| {
                                *quantity > 0 && dimension.split('/').next() == Some(op.resource())
                            })
                    })
                {
                    return Err(crate::domain::usage::value_pricing::PriceRejection {
                        reason_code: "resource_unpriced",
                        model: crate::domain::usage::value_pricing::diagnostic_identity(model),
                        tier: "unknown".into(),
                        resource_dimensions: vec![op.resource().into()],
                        active_pricing_revision: None,
                        pending_pricing_revision: None,
                        effective_from_ms: None,
                    }
                    .into());
                }
                if quota_unit == "tokens" && grant.manager != ShareGrantManager::RouterShareMarket {
                    let required = condition(
                        scope,
                        quota_unit,
                        op,
                        &fingerprint,
                        "resource_not_in_token_ledger",
                    );
                    anyhow::ensure!(
                        policy
                            .coverage_acknowledgements
                            .iter()
                            .any(|a| a.condition == required
                                && a.scope == scope
                                && !a.actor.is_empty()
                                && a.accepted_at_ms > 0),
                        "QUOTA_COVERAGE_REQUIRED"
                    );
                }
            }
            return Ok(());
        }
        let policy = share
            .capability_policy
            .as_ref()
            .context("CAPABILITY_STATE_CHANGED")?;
        let snapshot = if quota_unit == "usdEquivalent" {
            tariff(&self.config_dir, config, share)?
        } else {
            None
        };
        for op in &operations {
            anyhow::ensure!(
                policy.allowed_operations.contains(op)
                    && grant
                        .and_then(|g| g.policy.allowed_operations.as_ref())
                        .is_none_or(|ops| ops.contains(op)),
                "SHARE_CAPABILITY_DISABLED"
            );
            anyhow::ensure!(implemented(&runtime, *op), "CAPABILITY_NOT_SUPPORTED");
            anyhow::ensure!(
                policy
                    .operation_bindings
                    .get(op)
                    .is_some_and(|bindings| bindings
                        .iter()
                        .any(|b| b.app == app_name(app) && b.fingerprint == fingerprint)),
                "CAPABILITY_STATE_CHANGED"
            );
            let priced = snapshot
                .as_ref()
                .and_then(|(_, s)| s.model(model).ok())
                .is_some_and(|m| {
                    let quantities = resource_plan
                        .map(|p| {
                            p.quantities
                                .iter()
                                .filter(|(key, _)| key.split('/').next() == Some(op.resource()))
                                .map(|(k, v)| (k.clone(), *v))
                                .collect::<BTreeMap<_, _>>()
                        })
                        .unwrap_or_default();
                    !quantities.is_empty()
                        && crate::domain::usage::value_pricing::price_resources(
                            &m.resources,
                            &quantities,
                        )
                        .is_ok()
                });
            if let Some(blocker) = scope_blocker(grant.map(|g| &g.policy), priced) {
                anyhow::bail!(blocker);
            }
            if !priced
                && grant
                    .and_then(|g| g.policy.value_quota.as_ref())
                    .is_some_and(|q| {
                        q.unpriced_usage
                            == crate::domain::usage::value_pricing::UnpricedUsage::Reject
                    })
            {
                anyhow::bail!("VALUE_QUOTA_RESOURCE_PRICE_UNAVAILABLE");
            }
            if grant.is_some_and(|g|g.manager==crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket) {
                anyhow::ensure!(priced && quota_unit=="usdEquivalent", "QUOTA_COVERAGE_REQUIRED");
                anyhow::ensure!(grant.and_then(|g|g.policy.allowed_operations.as_ref()).is_some_and(|ops|ops.contains(op)),"MARKET_CAPABILITY_CONFLICT");
            } else if let Some(gap)=coverage_gap(quota_unit,priced) {
                let condition=condition(scope,quota_unit,*op,&fingerprint,gap);
                anyhow::ensure!(policy.coverage_acknowledgements.iter().any(|a|a.condition==condition && !a.actor.is_empty() && a.accepted_at_ms>0),"QUOTA_COVERAGE_REQUIRED");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn capability_pricing_display_distinguishes_base_parameter_and_request_scope() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/share-value-pricing-v1.json"
        ))
        .unwrap();
        let mut snapshot: PricingSnapshot =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        let op = ShareOperation::ImageGenerate;
        assert_eq!(
            pricing_coverage(None, Some("free-text"), op),
            "tariff_unavailable"
        );
        assert_eq!(
            pricing_coverage(Some(&snapshot), None, op),
            "request_dependent"
        );
        assert_eq!(
            pricing_coverage(Some(&snapshot), Some("absent"), op),
            "missing"
        );
        let model = snapshot.models.get_mut("free-text").unwrap();
        model
            .resources
            .insert("image.output/size=1024x1024".into(), "100".into());
        assert_eq!(
            pricing_coverage(Some(&snapshot), Some("free-text"), op),
            "parameter_rates"
        );
        snapshot
            .models
            .get_mut("free-text")
            .unwrap()
            .resources
            .insert("image.output".into(), "0".into());
        assert_eq!(
            pricing_coverage(Some(&snapshot), Some("free-text"), op),
            "base_rate"
        );
    }
}
