use super::*;
use crate::domain::accounts::capability_evidence::AccountCapabilityObservationState;
use crate::domain::providers::runtime::RuntimeConfigurationState;
use crate::domain::providers::share_operations::{observed_account_state, provider_enabled};
use crate::domain::sharing::router_contract::{ShareGrantManager, ShareUserGrantEdit};
use crate::domain::sharing::shares::{normalize_verified_email, share_enabled_apps, ShareBinding};
use std::collections::BTreeSet;

const TOKEN_GAP: &str = "resource_not_in_token_ledger";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct GrantPreviewInput {
    expected_config_revision: u64,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    grant_capability_edits: BTreeMap<String, GrantCapabilityEdit>,
    #[serde(default)]
    user_grant_edits: BTreeMap<String, ShareUserGrantEdit>,
    #[serde(default)]
    share_token_limit: Option<i64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionRow {
    operation: ShareOperation,
    available: bool,
    selected: bool,
    needs_reconfirm: bool,
}

struct GrantProposal {
    permissions: GrantCapabilities,
    required: Vec<CoverageAcknowledgement>,
    ticket: PreviewTicket,
}

fn eligible_bindings<'a>(
    share: &'a Share,
    user: &'a ShareUserPolicy,
) -> impl Iterator<Item = &'a ShareBinding> {
    let enabled = share_enabled_apps(share);
    share.bindings.iter().filter(move |binding| {
        enabled.contains(&binding.app)
            && (user.allowed_apps.is_empty() || user.allowed_apps.contains(&binding.app))
    })
}

fn bindings_unavailable(share: &Share, user: &ShareUserPolicy, providers: &ProviderStore) -> bool {
    eligible_bindings(share, user).any(|binding| {
        providers
            .runtime_plan(binding.app, &binding.provider_id)
            .is_none_or(|runtime| {
                runtime.configuration_state == RuntimeConfigurationState::NeedsAttention
            })
    })
}

fn available_bindings(
    share: &Share,
    user: &ShareUserPolicy,
    providers: &ProviderStore,
    accounts: &AccountStore,
    op: ShareOperation,
) -> Vec<OperationBinding> {
    let now = now_ms() as i64;
    eligible_bindings(share, user)
        .filter_map(|binding| {
            let runtime = providers.runtime_plan(binding.app, &binding.provider_id)?;
            if !implemented(&runtime, op)
                || runtime.configuration_state == RuntimeConfigurationState::NeedsAttention
                || observed_account_state(&runtime, accounts, op, now)
                    == Some(AccountCapabilityObservationState::Unsupported)
            {
                return None;
            }
            Some(OperationBinding {
                app: app_name(binding.app),
                fingerprint: grant_binding_fingerprint(share, binding.app, &runtime),
            })
        })
        .collect()
}

fn token_conditions(
    share: &Share,
    permissions: &GrantCapabilities,
) -> Vec<CoverageAcknowledgement> {
    let mut result = Vec::new();
    for (email, grant) in &share.user_grants {
        let Some(policy) = permissions
            .get(email)
            .filter(|_| grant.active && grant.manager != ShareGrantManager::RouterShareMarket)
        else {
            continue;
        };
        let uses_tokens = std::iter::once(&grant.policy)
            .chain(grant.pending_value_quota.as_ref().map(|p| &p.policy))
            .any(|terms| unit(share, Some(terms)) == "tokens");
        if !uses_tokens {
            continue;
        }
        for (op, bindings) in &policy.operation_bindings {
            for binding in bindings {
                let id = condition(email, "tokens", *op, &binding.fingerprint, TOKEN_GAP);
                if !policy.coverage_acknowledgements.iter().any(|a| {
                    a.condition == id
                        && a.scope == *email
                        && !a.actor.is_empty()
                        && a.accepted_at_ms > 0
                }) {
                    result.push(CoverageAcknowledgement {
                        condition: id,
                        scope: email.clone(),
                        operation: *op,
                        quota_unit: "tokens".into(),
                        binding_fingerprint: binding.fingerprint.clone(),
                        actor: String::new(),
                        accepted_at_ms: 0,
                    });
                }
            }
        }
    }
    result.sort_by(|a, b| a.condition.cmp(&b.condition));
    result
}

fn stage_grants(
    share: &Share,
    user_grant_edits: Option<&BTreeMap<String, ShareUserGrantEdit>>,
    user_grants: Option<&BTreeMap<String, crate::domain::sharing::router_contract::ShareUserGrant>>,
    share_token_limit: Option<i64>,
) -> anyhow::Result<Share> {
    let mut draft = ShareStore {
        shares: vec![share.clone()],
        ..Default::default()
    };
    draft
        .apply_settings_patch(
            &share.id,
            ShareSettingsPatch {
                user_grant_edits: user_grant_edits.filter(|e| !e.is_empty()).cloned(),
                user_grants: user_grants.cloned(),
                token_limit: share_token_limit,
                ..Default::default()
            },
        )
        .map_err(|error| anyhow::anyhow!(error))
}

fn grant_proposal(
    original: &Share,
    draft: &Share,
    edits: &BTreeMap<String, GrantCapabilityEdit>,
    providers: &ProviderStore,
    accounts: &AccountStore,
    pricing: Option<&str>,
) -> anyhow::Result<GrantProposal> {
    anyhow::ensure!(edits.len() <= 500, "invalid capability preview size");
    let mut permissions = draft
        .grant_capabilities
        .clone()
        .context("CAPABILITY_UPGRADE_REQUIRED")?;
    for (email, edit) in edits {
        anyhow::ensure!(
            normalize_verified_email(email).map_err(|_| anyhow::anyhow!("invalid grant email"))?
                == *email,
            "invalid grant email"
        );
        edit.validate().map_err(anyhow::Error::msg)?;
        let previous = original.user_grants.get(email);
        anyhow::ensure!(
            previous.map_or(0, |g| g.revision) == edit.expected_revision,
            "CAPABILITY_STATE_CHANGED"
        );
        let grant = draft
            .user_grants
            .get(email)
            .filter(|g| g.active && g.manager != ShareGrantManager::RouterShareMarket)
            .context("CAPABILITY_STATE_CHANGED")?;
        let mut policy = permissions.remove(email).unwrap_or_default();
        let mut operations = edit.operations.clone();
        operations.sort_unstable();
        let additions = operations.iter().filter(|operation| {
            !policy.allowed_operations.contains(operation) || edit.reconfirm.contains(operation)
        });
        let mut additions = additions.peekable();
        if additions.peek().is_some() {
            anyhow::ensure!(
                !bindings_unavailable(draft, &grant.policy, providers),
                "CAPABILITY_STATE_CHANGED"
            );
            for operation in additions {
                let bindings =
                    available_bindings(draft, &grant.policy, providers, accounts, *operation);
                anyhow::ensure!(!bindings.is_empty(), "CAPABILITY_NOT_SUPPORTED");
                policy.operation_bindings.insert(*operation, bindings);
            }
        }
        policy
            .operation_bindings
            .retain(|op, _| operations.contains(op));
        policy.coverage_acknowledgements.retain(|ack| {
            policy
                .operation_bindings
                .get(&ack.operation)
                .is_some_and(|bindings| {
                    bindings
                        .iter()
                        .any(|b| b.fingerprint == ack.binding_fingerprint)
                })
        });
        policy.allowed_operations = operations;
        if !policy.allowed_operations.is_empty() {
            permissions.insert(email.clone(), policy);
        }
    }
    let required = token_conditions(draft, &permissions);
    anyhow::ensure!(required.len() <= 16_384, "CAPABILITY_POLICY_TOO_LARGE");
    let terms = permissions
        .keys()
        .filter_map(|email| {
            draft
                .user_grants
                .get(email)
                .map(|g| (email, &g.policy, &g.pending_value_quota))
        })
        .collect::<Vec<_>>();
    let ticket = PreviewTicket {
        domain: "share-capability-preview-v1".into(),
        share_id: original.id.clone(),
        revision: original.config_revision,
        target: digest(&(
            "grant-capabilities-v14",
            edits,
            &permissions,
            terms,
            draft.token_limit,
            pricing,
        )),
        conditions: digest(&required.iter().map(|a| &a.condition).collect::<Vec<_>>()),
        expires_at_ms: now_ms() as i64 + 900_000,
    };
    Ok(GrantProposal {
        permissions,
        required,
        ticket,
    })
}

impl ServerStateInner {
    pub(super) async fn preview_grant_capabilities(
        &self,
        share_id: &str,
        input: GrantPreviewInput,
    ) -> anyhow::Result<Value> {
        let config = self.config_snapshot().await;
        let providers = self.providers.read().await.clone();
        let accounts = self.accounts.read().await.clone();
        let share = self
            .shares
            .read()
            .await
            .get(share_id)
            .cloned()
            .context("Share not found")?;
        anyhow::ensure!(
            share.grant_capabilities.is_some(),
            "CAPABILITY_UPGRADE_REQUIRED"
        );
        anyhow::ensure!(
            share.config_revision == input.expected_config_revision,
            "CAPABILITY_STATE_CHANGED"
        );
        let ready = self.grant_capability_router_negotiated().await
            && crate::repository::server_sqlite::is_committed(&self.config_dir)?;
        let draft = stage_grants(
            &share,
            Some(&input.user_grant_edits),
            None,
            input.share_token_limit,
        )?;
        let mut unavailable = false;
        let mut rows = Vec::new();
        if let Some(email) = &input.subject {
            anyhow::ensure!(
                normalize_verified_email(email)
                    .map_err(|_| anyhow::anyhow!("invalid grant email"))?
                    == *email,
                "invalid grant email"
            );
            let grant = draft.user_grants.get(email);
            anyhow::ensure!(
                grant.is_none_or(|g| g.manager != ShareGrantManager::RouterShareMarket),
                "MARKET_CAPABILITY_CONFLICT"
            );
            let terms = grant.map(|g| &g.policy).cloned().unwrap_or_default();
            let saved = share
                .grant_capabilities
                .as_ref()
                .and_then(|map| map.get(email));
            unavailable = bindings_unavailable(&draft, &terms, &providers);
            for operation in ShareOperation::ALL {
                let bindings = available_bindings(&draft, &terms, &providers, &accounts, operation);
                let selected = saved.is_some_and(|p| p.allowed_operations.contains(&operation));
                if selected || !bindings.is_empty() {
                    let approved = saved.and_then(|p| p.operation_bindings.get(&operation));
                    rows.push(PermissionRow {
                        operation,
                        available: !bindings.is_empty(),
                        selected,
                        needs_reconfirm: selected
                            && bindings
                                .iter()
                                .any(|b| approved.is_none_or(|list| !list.contains(b))),
                    });
                }
            }
            return Ok(
                serde_json::json!({"shareId":share_id,"configRevision":share.config_revision,
                "subject":email,"ready":ready,"unavailable":unavailable,"rows":rows,"displayVersion":3}),
            );
        }

        anyhow::ensure!(ready, "CAPABILITY_UPGRADE_REQUIRED");
        let pricing = proposal_pricing(
            &self.config_dir,
            &config,
            &draft,
            &input.grant_capability_edits,
        )?;
        let proposal = grant_proposal(
            &share,
            &draft,
            &input.grant_capability_edits,
            &providers,
            &accounts,
            pricing.as_deref(),
        )?;
        Ok(
            serde_json::json!({"shareId":share_id,"configRevision":share.config_revision,
            "ready":true,"unavailable":unavailable,"rows":rows,"displayVersion":3,
            "requiredAcknowledgements":proposal.required,"previewCredential":sign(&config,&proposal.ticket)?,
            "expiresAt":proposal.ticket.expires_at_ms}),
        )
    }

    pub(super) async fn prepare_grant_capability_patch(
        &self,
        share: &Share,
        mut patch: ShareSettingsPatch,
    ) -> anyhow::Result<ShareSettingsPatch> {
        anyhow::ensure!(
            patch.capability_policy.is_none() && patch.grok_media_policy.is_none(),
            "CAPABILITY_UPGRADE_REQUIRED"
        );
        if patch.grant_capability_edits.is_none()
            && patch.capability_consent.is_none()
            && patch.user_grant_edits.is_none()
            && patch.user_grants.is_none()
            && patch.token_limit.is_none()
        {
            return Ok(patch);
        }
        let config = self.config_snapshot().await;
        let providers = self.providers.read().await.clone();
        let accounts = self.accounts.read().await.clone();
        let edits = patch
            .grant_capability_edits
            .as_ref()
            .cloned()
            .unwrap_or_default();
        let draft = stage_grants(
            share,
            patch.user_grant_edits.as_ref(),
            patch.user_grants.as_ref(),
            patch.token_limit,
        )?;
        let pricing = proposal_pricing(&self.config_dir, &config, &draft, &edits)?;
        let mut proposal = grant_proposal(
            share,
            &draft,
            &edits,
            &providers,
            &accounts,
            pricing.as_deref(),
        )?;
        if edits.is_empty() && proposal.required.is_empty() && patch.capability_consent.is_none() {
            return Ok(patch);
        }
        anyhow::ensure!(
            self.grant_capability_router_negotiated().await
                && crate::repository::server_sqlite::is_committed(&self.config_dir)?,
            "CAPABILITY_UPGRADE_REQUIRED"
        );
        let context = patch
            .edit_context
            .as_ref()
            .context("CAPABILITY_PREVIEW_EXPIRED")?;
        anyhow::ensure!(
            context.expected_config_revision == share.config_revision,
            "CAPABILITY_STATE_CHANGED"
        );
        let consent = patch
            .capability_consent
            .as_ref()
            .context("CAPABILITY_PREVIEW_EXPIRED")?;
        let verified = verify(&config, &consent.credential)?;
        anyhow::ensure!(
            verified.share_id == proposal.ticket.share_id
                && verified.revision == proposal.ticket.revision
                && verified.target == proposal.ticket.target
                && verified.conditions == proposal.ticket.conditions,
            "CAPABILITY_STATE_CHANGED"
        );
        let accepted = consent.accept_uncovered.iter().collect::<BTreeSet<_>>();
        anyhow::ensure!(
            accepted.len() == proposal.required.len()
                && proposal
                    .required
                    .iter()
                    .all(|a| accepted.contains(&a.condition)),
            "QUOTA_COVERAGE_REQUIRED"
        );
        for mut ack in proposal.required {
            ack.actor = context
                .actor
                .as_deref()
                .filter(|actor| !actor.is_empty())
                .unwrap_or("server-admin")
                .into();
            ack.accepted_at_ms = now_ms() as i64;
            let scope = ack.scope.clone();
            proposal
                .permissions
                .get_mut(&scope)
                .expect("coverage belongs to an enabled grant")
                .coverage_acknowledgements
                .push(ack);
        }
        patch.validated_grant_capabilities = Some(proposal.permissions);
        Ok(patch)
    }

    pub(crate) async fn grant_capability_router_negotiated(&self) -> bool {
        let config = self.config_snapshot().await;
        self.router_quota_capability
            .read()
            .await
            .as_ref()
            .is_some_and(|(base, id, at, c)| {
                c.grant_capabilities
                    && at.elapsed() < std::time::Duration::from_secs(300)
                    && config.router_api_base() == Some(base.as_str())
                    && config
                        .registered_router_identity()
                        .is_some_and(|i| i.installation_id == *id)
            })
    }
}

fn proposal_pricing(
    dir: &std::path::Path,
    config: &ServerConfig,
    draft: &Share,
    edits: &BTreeMap<String, GrantCapabilityEdit>,
) -> anyhow::Result<Option<String>> {
    let has_usd = edits.keys().any(|email| {
        draft.user_grants.get(email).is_some_and(|g| {
            g.policy.value_quota.is_some()
                || g.pending_value_quota
                    .as_ref()
                    .is_some_and(|p| p.policy.value_quota.is_some())
        })
    });
    if has_usd {
        Ok(tariff(dir, config, draft)?.map(|(revision, _)| revision))
    } else {
        Ok(None)
    }
}

/// Called again under the settings transaction gate. Previously approved but
/// unavailable bindings remain dormant; only new authority must match now.
pub(super) fn validate_grant_transition(
    before: Option<&Share>,
    share: &Share,
    permissions: &GrantCapabilities,
    providers: &ProviderStore,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        share.capability_policy.is_none(),
        "CAPABILITY_UPGRADE_REQUIRED"
    );
    let previous = before.and_then(|s| s.grant_capabilities.as_ref());
    for (email, policy) in permissions {
        let grant = share
            .user_grants
            .get(email)
            .filter(|g| g.active)
            .context("CAPABILITY_STATE_CHANGED")?;
        policy.validate().map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            policy
                .coverage_acknowledgements
                .iter()
                .all(|a| a.scope == *email),
            "CAPABILITY_STATE_CHANGED"
        );
        let allowed_before = previous.and_then(|map| map.get(email));
        for (operation, bindings) in &policy.operation_bindings {
            for binding in bindings {
                if allowed_before
                    .and_then(|p| p.operation_bindings.get(operation))
                    .is_some_and(|list| list.contains(binding))
                {
                    continue;
                }
                anyhow::ensure!(
                    grant.manager != ShareGrantManager::RouterShareMarket,
                    "MARKET_CAPABILITY_CONFLICT"
                );
                anyhow::ensure!(
                    !bindings_unavailable(share, &grant.policy, providers)
                        && eligible_bindings(share, &grant.policy).any(|b| app_name(b.app)
                            == binding.app
                            && providers
                                .runtime_plan(b.app, &b.provider_id)
                                .is_some_and(|p| implemented(&p, *operation)
                                    && grant_binding_fingerprint(share, b.app, &p)
                                        == binding.fingerprint)),
                    "CAPABILITY_STATE_CHANGED"
                );
            }
        }
    }
    anyhow::ensure!(
        token_conditions(share, permissions).is_empty(),
        "QUOTA_COVERAGE_REQUIRED"
    );
    Ok(())
}

/// One-time materialization of previously effective named grants. This reads
/// legacy state only during migration; public access never enters the map.
pub(crate) fn migrate_grant_capabilities(
    store: &mut ShareStore,
    providers: &ProviderStore,
) -> usize {
    let mut migrated = 0;
    for share in &mut store.shares {
        if share.grant_capabilities.is_some() {
            continue;
        }
        let mut permissions = GrantCapabilities::new();
        if let Some(legacy) = share
            .capability_policy
            .as_ref()
            .filter(|p| p.validate().is_ok())
        {
            for (email, grant) in share.user_grants.iter().filter(|(_, g)| g.active) {
                let mut policy = ShareCapabilityPolicy::default();
                let uses_tokens = std::iter::once(&grant.policy)
                    .chain(grant.pending_value_quota.as_ref().map(|p| &p.policy))
                    .any(|terms| unit(share, Some(terms)) == "tokens");
                for operation in &legacy.allowed_operations {
                    if grant
                        .policy
                        .allowed_operations
                        .as_ref()
                        .is_some_and(|ops| !ops.contains(operation))
                        || (grant.manager == ShareGrantManager::RouterShareMarket
                            && grant.policy.allowed_operations.is_none())
                        || grant
                            .pending_value_quota
                            .as_ref()
                            .and_then(|p| p.policy.allowed_operations.as_ref())
                            .is_some_and(|ops| !ops.contains(operation))
                    {
                        continue;
                    }
                    let mut approved = Vec::new();
                    for binding in legacy
                        .operation_bindings
                        .get(operation)
                        .into_iter()
                        .flatten()
                    {
                        let active = eligible_bindings(share, &grant.policy).find_map(|b| {
                            if app_name(b.app) != binding.app {
                                return None;
                            }
                            let runtime = providers.runtime_plan(b.app, &b.provider_id)?;
                            (provider_enabled(&runtime, *operation)
                                && binding_fingerprint(&runtime) == binding.fingerprint)
                                .then(|| grant_binding_fingerprint(share, b.app, &runtime))
                        });
                        let Some(new_fingerprint) = active else {
                            continue;
                        };
                        let id =
                            condition(email, "tokens", *operation, &binding.fingerprint, TOKEN_GAP);
                        let ack = legacy.coverage_acknowledgements.iter().find(|a| {
                            a.condition == id
                                && a.scope == *email
                                && a.accepted_at_ms > 0
                                && !a.actor.is_empty()
                        });
                        if uses_tokens
                            && ack.is_none()
                            && grant.manager != ShareGrantManager::RouterShareMarket
                        {
                            continue;
                        }
                        if let Some(ack) = ack {
                            let mut moved = ack.clone();
                            moved.binding_fingerprint = new_fingerprint.clone();
                            moved.condition =
                                condition(email, "tokens", *operation, &new_fingerprint, TOKEN_GAP);
                            policy.coverage_acknowledgements.push(moved);
                        }
                        approved.push(OperationBinding {
                            app: binding.app.clone(),
                            fingerprint: new_fingerprint,
                        });
                    }
                    if !approved.is_empty() {
                        policy.allowed_operations.push(*operation);
                        policy.operation_bindings.insert(*operation, approved);
                    }
                }
                if !policy.allowed_operations.is_empty() {
                    permissions.insert(email.clone(), policy);
                }
            }
        }
        share.capability_policy = None;
        share.grant_capabilities = Some(permissions);
        crate::domain::sharing::shares::normalize_grant_capability_terms(share);
        share.config_revision = share.config_revision.saturating_add(1).max(1);
        share.router_last_sync_error = None;
        migrated += 1;
    }
    migrated
}
