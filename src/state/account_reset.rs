//! Account-reset orchestration. All durable publication stays behind the
//! existing state transactions; network calls never hold a storage lock.
use super::*;
use crate::clients::oauth::codex_reset_credits::ConsumeResetCreditResult;
use crate::clients::public_resets::{self, Fetch};
use crate::domain::accounts::store::verified_openai_subject;
use crate::domain::sharing::{quota_cycle::*, router_contract::ShareTokenPeriod};
use serde_json::json;

pub(crate) struct PreparedResetCredit {
    pub operation_id: String,
    pub request_id: String,
    pub receipt: Option<ConsumeResetCreditResult>,
    pub dispatch: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("QUOTA_CYCLE_CONFLICT")]
pub(crate) struct QuotaPreviewConflict(pub Value);

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreQuotaFence {
    stream_incarnation: String,
    restored_at_ms: i64,
}

pub(super) fn write_restore_quota_fence(config_dir: &Path) -> anyhow::Result<()> {
    crate::infra::storage::write_json_pretty(
        &config_dir.join("account-reset-restore-fence.json"),
        &RestoreQuotaFence {
            stream_incarnation: opaque_id(),
            restored_at_ms: now_ms_i64(),
        },
    )
}

fn apply_public_status_result(
    ledger: &mut QuotaCycleLedger,
    fetched: &anyhow::Result<public_resets::StatusFetch>,
    now: i64,
) {
    let cursor = &mut ledger.public_status_cursor;
    match fetched {
        Ok(public_resets::StatusFetch::Observed {
            latest,
            etag,
            cache_evidence,
        }) => {
            if ledger.public_status_latest.as_ref() != Some(latest) {
                ledger.public_status_latest = Some(latest.clone());
                // Never clear a history Retry-After or failed-page backoff.
                if ledger.public_cursor.failures == 0 && ledger.public_cursor.last_error.is_none() {
                    ledger.public_cursor.retry_at_ms = None;
                }
            }
            cursor.etag.clone_from(etag);
            cursor.cache_evidence = Some(cache_evidence.clone());
            cursor.checked_at_ms = Some(now);
            cursor.retry_at_ms = Some(now + public_resets::POLL_MS);
            cursor.failures = 0;
            cursor.last_error = None;
        }
        Ok(public_resets::StatusFetch::NotModified) => {
            cursor.checked_at_ms = Some(now);
            cursor.retry_at_ms = Some(now + public_resets::POLL_MS);
            cursor.failures = 0;
            cursor.last_error = None;
        }
        Ok(public_resets::StatusFetch::Limited(delay)) => {
            cursor.last_error = Some("public_status_rate_limited".into());
            cursor.retry_at_ms = Some(now + delay);
        }
        Err(_) => {
            cursor.failures = cursor.failures.saturating_add(1);
            cursor.last_error = Some("public_status_unavailable".into());
            cursor.retry_at_ms =
                Some(now + public_resets::backoff_ms(cursor.failures, rand::random()));
        }
    }
}

fn ingest_public_quota_page(
    ledger: &mut QuotaCycleLedger,
    page: &public_resets::ResetPage,
    from: i64,
    query: &str,
    etag: &Option<String>,
    cache_evidence: &str,
    now: i64,
) -> Result<(), &'static str> {
    // Validate every row before changing either evidence or the continuation.
    let validated = page
        .data
        .iter()
        .map(|event| event.effective_at_ms(now).map(|time| (event, time)))
        .collect::<anyhow::Result<Vec<_>>>()
        .map_err(|_| "public_event_quarantined")?;
    for (event, time) in validated {
        if event.reset_type == "regular" {
            ledger.public_events.insert(event.id.clone(), time);
        }
        ledger.public_cursor.high_watermark_ms = Some(
            ledger
                .public_cursor
                .high_watermark_ms
                .unwrap_or(0)
                .max(time),
        );
    }
    let cursor = &mut ledger.public_cursor;
    cursor.from_at_ms = Some(from);
    cursor.next_cursor.clone_from(&page.pagination.next_cursor);
    cursor.etag.clone_from(etag);
    cursor.cache_query = Some(query.into());
    cursor.cache_evidence = Some(cache_evidence.into());
    cursor.failures = 0;
    cursor.last_error = None;
    cursor.checked_at_ms = Some(now);
    cursor.retry_at_ms = Some(
        now + if page.pagination.has_more {
            1_000
        } else {
            public_resets::HISTORY_MS
        },
    );
    if !page.pagination.has_more {
        cursor.from_at_ms = Some(
            from.max(
                cursor
                    .high_watermark_ms
                    .unwrap_or(from)
                    .saturating_sub(public_resets::OVERLAP_MS),
            ),
        );
    }
    Ok(())
}

pub(super) fn apply_restore_quota_fence(
    config_dir: &Path,
    shares: &mut ShareStore,
    usage: &mut UsageStore,
) -> anyhow::Result<bool> {
    let path = config_dir.join("account-reset-restore-fence.json");
    let payload = match std::fs::read(&path) {
        Ok(payload) => payload,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let fence: RestoreQuotaFence = serde_json::from_slice(&payload)?;
    anyhow::ensure!(
        !fence.stream_incarnation.is_empty() && fence.restored_at_ms >= 0,
        "invalid restore quota fence"
    );
    if shares.quota_cycle_ledger.stream_incarnation == fence.stream_incarnation {
        return Ok(false);
    }
    let ledger = &mut shares.quota_cycle_ledger;
    ledger.stream_incarnation = fence.stream_incarnation;
    // The old backup cannot know which credits were spent afterwards. This
    // fence survives outside backup targets, including a crash during restore.
    ledger.redeem_recovery_required = true;
    for operation in ledger.operations.values_mut().filter(|operation| {
        matches!(
            operation.status,
            ResetCreditStatus::IntentDurable | ResetCreditStatus::Dispatched
        )
    }) {
        operation.status = ResetCreditStatus::Unknown;
    }
    for source in ledger.sources.values_mut() {
        if let Some(cycle) = &mut source.cycle {
            cycle
                .stream_incarnation
                .clone_from(&ledger.stream_incarnation);
        }
        usage
            .quota_ledger
            .coverage_from_ms
            .entry(source.source_ref.clone())
            .and_modify(|start| *start = (*start).max(fence.restored_at_ms))
            .or_insert(fence.restored_at_ms);
    }
    ledger.revision = ledger.revision.saturating_add(1);
    let bindings: Vec<_> = shares
        .quota_cycle_sources
        .iter()
        .map(|(id, source)| (id.clone(), source.clone()))
        .collect();
    for (id, source) in bindings {
        shares
            .project_account_reset(&id, &source, usage, fence.restored_at_ms)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
    }
    Ok(true)
}

impl ServerStateInner {
    pub(crate) async fn record_passive_quota_window(
        self: &Arc<Self>,
        key: &ProviderKey,
        provider_revision: u64,
        account_id: &str,
        fence: &QuotaSourceFence,
        observation: &QuotaWindowObservation,
    ) -> anyhow::Result<bool> {
        let _workspace = self.codex_workspace_rebind_transactions.lock().await;
        let _quota = self.share_quota_mutations.lock().await;
        let providers = self.providers.read().await;
        let accounts = self.accounts.read().await;
        let Some(stored) = providers.providers.iter().find(|stored| {
            stored.app == key.app
                && stored.provider.id == key.provider_id
                && stored.resource.revision == provider_revision
        }) else {
            return Ok(false);
        };
        let Some(account) =
            crate::domain::providers::runtime::authoritative_managed_account(stored, &accounts)
                .filter(|account| {
                    account.id == account_id
                        && account.auth_identity_generation == fence.identity_generation
                        && accounts.account_incarnation(account_id)
                            == Some(fence.account_incarnation)
                })
        else {
            return Ok(false);
        };
        let Some(identity) =
            crate::domain::sharing::subscription_identity::resolve_provider_subscription_identity(
                stored, &accounts,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?
        else {
            return Ok(false);
        };
        let source_identity = format!(
            "{}:{}:{}",
            account.provider_type.as_str(),
            identity.fingerprint(),
            observation.bucket_id
        );
        let _commit = self.usage_persistence.gate.lock().await;
        let usage = self.usage.read().await;
        self.try_mutate_shares_in_workspace_transaction(|shares| {
            let Some(source) = shares
                .quota_cycle_ledger
                .sources
                .get(&source_identity)
                .filter(|source| &source.fence == fence)
                .cloned()
            else {
                return Ok::<_, &'static str>(false);
            };
            let changed =
                shares
                    .quota_cycle_ledger
                    .observe(&source.source_ref, fence, observation)?;
            if changed {
                let ids: Vec<_> = shares
                    .quota_cycle_sources
                    .iter()
                    .filter(|(_, current)| **current == source.source_ref)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ids {
                    shares
                        .project_account_reset(
                            &id,
                            &source.source_ref,
                            &usage,
                            observation.observed_at_ms,
                        )
                        .map_err(|_| "quota_projection_failed")?;
                }
            }
            Ok(changed)
        })
        .await?
        .map_err(anyhow::Error::msg)
    }

    pub async fn preview_share_account_reset(
        self: &Arc<Self>,
        share_id: &str,
        expected_revision: u64,
        policies: BTreeMap<String, crate::domain::sharing::router_contract::ShareUserPolicy>,
    ) -> anyhow::Result<Value> {
        self.refresh_share_account_quota(share_id).await?;
        let negotiated = self.router_supports_account_reset().await;
        let _quota = self.share_quota_mutations.lock().await;
        let _commit = self.usage_persistence.gate.lock().await;
        let usage = self.usage.read().await;
        let shares = self.shares.read().await;
        let share = shares.get(share_id).context("share not found")?;
        let source = shares
            .quota_cycle_sources
            .get(share_id)
            .and_then(|source| shares.quota_cycle_ledger.source(source));
        let cycle = source.and_then(|source| source.cycle.as_ref());
        if share.config_revision != expected_revision {
            return Err(QuotaPreviewConflict(
                json!({"currentConfigRevision":share.config_revision,
                "accountQuotaCycle":cycle,"previewRequired":true}),
            )
            .into());
        }
        let complete = cycle.is_some_and(|cycle| usage.quota_ledger.complete_for(cycle));
        let fresh = source.is_some_and(|source| {
            now_ms_i64().saturating_sub(source.last_observed_at_ms) <= 300_000
                && cycle.is_some_and(|cycle| {
                    source.last_observed_at_ms >= cycle.starts_at_ms && cycle.ends_at_ms.is_some()
                })
        });
        let reason = if !negotiated {
            Some("ACCOUNT_RESET_ROUTER_UPGRADE_REQUIRED")
        } else if source.is_none() {
            Some("ACCOUNT_RESET_UNSUPPORTED")
        } else if cycle.is_none() {
            Some("ACCOUNT_RESET_NOT_READY")
        } else if usage.quota_ledger.capacity_limited() {
            Some("QUOTA_LEDGER_CAPACITY_LIMIT")
        } else if !complete {
            Some("QUOTA_HISTORY_INCOMPLETE")
        } else if !fresh {
            Some("ACCOUNT_RESET_NOT_READY")
        } else {
            None
        };
        let capability = crate::domain::sharing::quota_cycle_source::AccountResetCapability {
            support: if source.is_some() {
                "supported".into()
            } else {
                share
                    .user_grants
                    .values()
                    .find_map(|grant| grant.account_reset_capability.as_ref())
                    .map(|capability| capability.support.clone())
                    .unwrap_or_else(|| "conditional".into())
            },
            sync_status: if reason.is_none() {
                "ready"
            } else if complete && !fresh {
                "stale"
            } else {
                "initializing"
            }
            .into(),
            reason: if source.is_none() && negotiated {
                share
                    .user_grants
                    .values()
                    .find_map(|grant| grant.account_reset_capability.as_ref())
                    .and_then(|capability| capability.reason.clone())
                    .or_else(|| reason.map(str::to_string))
            } else {
                reason.map(str::to_string)
            },
            minimum_contract_version: 8,
        };
        let policies = if policies.is_empty() {
            share
                .user_grants
                .iter()
                .filter(|(_, grant)| grant.active)
                .map(|(email, grant)| (email.clone(), grant.policy.clone()))
                .collect()
        } else {
            policies
        };
        let rows: Vec<_> = policies.into_iter().map(|(email, policy)| {
            let email = email.trim().to_ascii_lowercase();
            let current = share.user_grants.get(&email);
            let observed = cycle.filter(|_| complete).and_then(|cycle| usage.quota_ledger.query(cycle, share_id, &email, usage.quota_ledger.watermark).ok()).map(|value| value.0);
            let baseline = current.and_then(|grant| grant.account_usage_baseline.as_ref()).filter(|baseline|
                cycle.is_some_and(|cycle| baseline.source_ref == cycle.source_ref && baseline.cycle_id == cycle.cycle_id));
            let effective = observed.map(|observed| baseline.and_then(|baseline| cycle.and_then(|cycle|
                usage.quota_ledger.query(cycle, share_id, &email, baseline.usage_watermark).ok())
                .map(|at_edit| crate::domain::usage::quota_ledger::QuotaUsageLedger::effective_with_baseline(observed, baseline.target_tokens, at_edit.0))).unwrap_or(observed));
            let locked = current.is_some_and(|grant| grant.manager == crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket);
            json!({ "email": email, "currentTokens": current.and_then(|grant| grant.usage_quota.as_ref()).map(|quota| quota.effective_tokens_used),
                "expectedTokens": effective, "tokenLimit": policy.token_limit,
                "wouldExceed": effective.zip(policy.token_limit).map(|(used, limit)| used >= limit),
                "reason": if locked { Some("market_contract_read_only") } else { reason } })
        }).collect();
        Ok(
            json!({"shareId":share_id,"configRevision":share.config_revision,"accountQuotaCycle":cycle,
            "accountResetCapability":capability,
            "usageWatermark":usage.quota_ledger.watermark,"complete":complete,"rows":rows,
            "ledgerHealth":{"revisionCount":usage.quota_ledger.watermark,"capacityWarning":usage.quota_ledger.capacity_warning(),
                "admissionLimit":crate::domain::usage::quota_ledger::QUOTA_USAGE_ADMISSION_LIMIT,
                "retentionPolicy":crate::domain::usage::quota_ledger::QUOTA_USAGE_RETENTION_POLICY},
            "defaultTokenPeriod":if reason.is_none(){"accountReset"}else{"lifetime"}}),
        )
    }

    /// The caller owns both existing account consume locks. Persisting intent
    /// happens before HTTP. Uncertain previous attempts are never assigned a
    /// new upstream request ID or silently re-consumed.
    pub(crate) async fn prepare_codex_reset_credit(
        self: &Arc<Self>,
        account: &Account,
        credit_id: &str,
        operation_id: &str,
    ) -> anyhow::Result<PreparedResetCredit> {
        use crate::domain::sharing::subscription_identity::SubscriptionIdentityKey;
        anyhow::ensure!(
            !operation_id.is_empty()
                && operation_id.len() <= 128
                && operation_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
            "invalid reset operationId"
        );
        let _workspace = self.codex_workspace_rebind_transactions.lock().await;
        self.recover_pending_codex_workspace_rebind_transaction("Reset credit intent")?;
        let _quota = self.share_quota_mutations.lock().await;
        let accounts = self.accounts.read().await;
        let current = accounts
            .find_for_provider(ProviderType::CodexOAuth, Some(&account.id))
            .context("Codex account disappeared")?;
        anyhow::ensure!(
            current.auth_identity_generation == account.auth_identity_generation,
            "reset identity changed"
        );
        let identity = SubscriptionIdentityKey::CodexOAuth {
            subject: verified_openai_subject(current).context("unverified Codex principal")?,
            workspace_id: effective_codex_workspace_id(current)
                .context("unverified Codex workspace")?,
        };
        let source_identity = format!("codex_oauth:{}:shared_weekly", identity.fingerprint());
        let fingerprint = hex::encode(Sha256::digest(
            format!("{source_identity}\0{}", credit_id.trim()).as_bytes(),
        ));
        let fence = QuotaSourceFence {
            account_incarnation: accounts.account_incarnation(&account.id).unwrap_or(0),
            identity_generation: account.auth_identity_generation,
            ..Default::default()
        };
        // A restore may have stopped after writing its independent fence but
        // before publishing the reloaded ShareStore. Such an in-process retry
        // must be as conservative as the next startup.
        let restore_path = self.config_dir.join("account-reset-restore-fence.json");
        if restore_path.exists() {
            let restore: RestoreQuotaFence = serde_json::from_slice(&std::fs::read(restore_path)?)?;
            anyhow::ensure!(
                restore.stream_incarnation
                    == self
                        .shares
                        .read()
                        .await
                        .quota_cycle_ledger
                        .stream_incarnation,
                "RESET_CREDIT_NEEDS_RECONCILE"
            );
        }
        self.try_mutate_shares_in_workspace_transaction(|shares| {
            let ledger = &mut shares.quota_cycle_ledger;
            if ledger.redeem_recovery_required {
                return Err("RESET_CREDIT_NEEDS_RECONCILE");
            }
            let source_ref = ledger.register_source(
                source_identity,
                "codex_oauth".into(),
                "shared_weekly".into(),
                fence.clone(),
            )?;
            if let Some(previous) = ledger.operations.get(operation_id) {
                if previous.source_ref != source_ref
                    || previous.fence != fence
                    || previous.credit_fingerprint != fingerprint
                {
                    return Err("reset_operation_conflict");
                }
            }
            // Repeated UI operations for a concrete credit must resolve to
            // its existing operation, including unknown results after crash.
            let previous = ledger
                .operations
                .get(operation_id)
                .or_else(|| {
                    ledger.operations.values().find(|operation| {
                        operation.source_ref == source_ref
                            && operation.credit_fingerprint == fingerprint
                            && (!credit_id.trim().is_empty()
                                || !matches!(operation.status, ResetCreditStatus::Rejected))
                    })
                })
                .cloned();
            if let Some(previous) = previous {
                return Ok(PreparedResetCredit {
                    operation_id: previous.operation_id,
                    request_id: previous.request_id,
                    receipt: previous
                        .result
                        .as_deref()
                        .and_then(|result| serde_json::from_str(result).ok()),
                    dispatch: false,
                });
            }
            // A result of unknown provenance may already have consumed the
            // next credit. It requires reconciliation before another consume.
            if ledger.operations.values().any(|operation| {
                operation.source_ref == source_ref
                    && matches!(
                        operation.status,
                        ResetCreditStatus::IntentDurable
                            | ResetCreditStatus::Dispatched
                            | ResetCreditStatus::Unknown
                    )
            }) {
                return Err("RESET_CREDIT_NEEDS_RECONCILE");
            }
            let request_id =
                crate::clients::oauth::codex_reset_credits::generate_redeem_request_id();
            ledger.begin_redeem(ResetCreditOperation {
                operation_id: operation_id.into(),
                request_id: request_id.clone(),
                source_ref,
                fence,
                status: ResetCreditStatus::IntentDurable,
                created_at_ms: now_ms_i64(),
                confirmed_at_ms: None,
                result: None,
                credit_fingerprint: fingerprint,
            })?;
            // Durable permission to dispatch; after a crash this state is
            // deliberately indistinguishable from a request with unknown result.
            ledger
                .operations
                .get_mut(operation_id)
                .expect("intent inserted")
                .status = ResetCreditStatus::Dispatched;
            Ok(PreparedResetCredit {
                operation_id: operation_id.into(),
                request_id,
                receipt: None,
                dispatch: true,
            })
        })
        .await?
        .map_err(anyhow::Error::msg)
    }

    pub(crate) async fn finish_codex_reset_credit(
        self: &Arc<Self>,
        operation_id: &str,
        result: Option<&ConsumeResetCreditResult>,
    ) -> anyhow::Result<()> {
        let result_json = result.map(serde_json::to_string).transpose()?;
        let now = now_ms_i64();
        self.try_mutate_share_quota_immediate(|shares, usage, _| {
            let ledger = &mut shares.quota_cycle_ledger;
            let operation = ledger
                .operations
                .get(operation_id)
                .cloned()
                .ok_or("reset_operation_missing")?;
            if matches!(
                operation.status,
                ResetCreditStatus::Confirmed | ResetCreditStatus::Rejected
            ) {
                return Ok::<_, &'static str>(());
            }
            let confirmed = result.is_some_and(|receipt| receipt.shared_weekly_confirmed);
            if confirmed {
                ledger.apply_reset(
                    &operation.source_ref,
                    &operation.fence,
                    &format!("redeem:{}", operation.request_id),
                    now,
                    now,
                    QuotaTimeBasis::RedeemConfirmation,
                )?;
            }
            let operation = ledger
                .operations
                .get_mut(operation_id)
                .expect("checked above");
            operation.status = if confirmed {
                ResetCreditStatus::Confirmed
            } else if result
                .and_then(|receipt| receipt.code.as_deref())
                .is_some_and(|code| {
                    matches!(
                        code,
                        "alreadyRedeemed"
                            | "nothingToReset"
                            | "noCredit"
                            | "already_redeemed"
                            | "nothing_to_reset"
                            | "no_credit"
                    )
                })
            {
                ResetCreditStatus::Rejected
            } else {
                ResetCreditStatus::Unknown
            };
            operation.confirmed_at_ms = confirmed.then_some(now);
            operation.result = result_json;
            ledger.revision = ledger.revision.saturating_add(1);
            let source = operation.source_ref.clone();
            let ids: Vec<_> = shares
                .quota_cycle_sources
                .iter()
                .filter(|(_, current)| **current == source)
                .map(|(id, _)| id.clone())
                .collect();
            for id in ids {
                shares
                    .project_account_reset(&id, &source, usage, now)
                    .map_err(|_| "quota_projection_failed")?;
            }
            Ok(())
        })
        .await?
        .map_err(anyhow::Error::msg)
    }

    pub(super) async fn collect_public_quota_resets(self: &Arc<Self>) -> anyhow::Result<()> {
        let now = now_ms_i64();
        let initial_from = {
            let shares = self.shares.read().await;
            let selected = shares
                .quota_cycle_sources
                .iter()
                .filter(|(id, _)| {
                    shares.get(id).is_some_and(|share| {
                        share.enabled
                            && share.user_grants.values().any(|grant| {
                                grant.active
                                    && grant.policy.token_period == ShareTokenPeriod::AccountReset
                            })
                    })
                })
                .filter_map(|(_, source)| shares.quota_cycle_ledger.source(source))
                .filter(|source| source.provider_type == "codex_oauth")
                .collect::<Vec<_>>();
            if selected.is_empty() {
                return Ok(());
            }
            selected
                .iter()
                .filter_map(|source| source.cycle.as_ref().map(|cycle| cycle.starts_at_ms))
                .min()
                .unwrap_or_else(|| now.saturating_sub(7 * 86_400_000))
        };
        let http = public_resets::client()?;
        let status_cursor = self
            .shares
            .read()
            .await
            .quota_cycle_ledger
            .public_status_cursor
            .clone();
        if status_cursor.retry_at_ms.is_none_or(|due| due <= now) {
            let fetched = public_resets::status(&http, status_cursor.etag.as_deref()).await;
            self.try_mutate_share_quota_immediate(|shares, _, _| {
                apply_public_status_result(&mut shares.quota_cycle_ledger, &fetched, now);
                Ok::<_, &'static str>(())
            })
            .await?
            .map_err(anyhow::Error::msg)?;
        }
        if self
            .shares
            .read()
            .await
            .quota_cycle_ledger
            .public_cursor
            .retry_at_ms
            .is_some_and(|due| due > now)
        {
            return Ok(());
        }
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..public_resets::MAX_PAGES {
            if self.is_shutting_down() {
                return Ok(());
            }
            let cursor = self
                .shares
                .read()
                .await
                .quota_cycle_ledger
                .public_cursor
                .clone();
            let from = cursor.from_at_ms.unwrap_or(initial_from);
            let query = serde_json::to_string(&(from, &cursor.next_cursor))?;
            anyhow::ensure!(seen.insert(query.clone()), "public pagination loop");
            let etag = (cursor.cache_query.as_ref() == Some(&query))
                .then_some(cursor.etag.as_deref())
                .flatten();
            let fetched =
                public_resets::history(&http, from, cursor.next_cursor.as_deref(), etag).await;
            // A page and its cursor share one durable commit. Failure leaves
            // that exact query replayable, including after process restart.
            let result = self
                .try_mutate_share_quota_immediate(|shares, usage, _| {
                    let ledger = &mut shares.quota_cycle_ledger;
                    if ledger.public_cursor.next_cursor != cursor.next_cursor
                        || ledger.public_cursor.from_at_ms != cursor.from_at_ms
                    {
                        return Err("public_cursor_conflict");
                    }
                    match &fetched {
                        Ok(Fetch::Page {
                            page,
                            etag,
                            cache_evidence,
                        }) => {
                            ingest_public_quota_page(
                                ledger,
                                page,
                                from,
                                &query,
                                etag,
                                cache_evidence,
                                now,
                            )?;
                        }
                        Ok(Fetch::NotModified) => {
                            // This is NOT an account quota observation.
                            ledger.public_cursor.checked_at_ms = Some(now);
                            ledger.public_cursor.retry_at_ms =
                                Some(now + public_resets::HISTORY_MS);
                            ledger.public_cursor.failures = 0;
                            ledger.public_cursor.last_error = None;
                        }
                        Ok(Fetch::Limited(delay)) => {
                            ledger.public_cursor.last_error =
                                Some("public_history_rate_limited".into());
                            ledger.public_cursor.retry_at_ms = Some(now + delay);
                        }
                        Ok(Fetch::InvalidCursor) => {
                            ledger.public_cursor.next_cursor = None;
                            ledger.public_cursor.etag = None;
                            ledger.public_cursor.cache_query = None;
                            ledger.public_cursor.retry_at_ms = Some(now + public_resets::POLL_MS);
                        }
                        Err(_) => {
                            ledger.public_cursor.failures =
                                ledger.public_cursor.failures.saturating_add(1);
                            ledger.public_cursor.last_error =
                                Some("public_history_unavailable".into());
                            ledger.public_cursor.retry_at_ms = Some(
                                now + public_resets::backoff_ms(
                                    ledger.public_cursor.failures,
                                    rand::random(),
                                ),
                            );
                        }
                    }
                    let selected: std::collections::BTreeSet<_> = shares
                        .quota_cycle_sources
                        .iter()
                        .filter(|(id, _)| {
                            shares.get(id).is_some_and(|share| {
                                share.enabled
                                    && share.user_grants.values().any(|grant| {
                                        grant.active
                                            && grant.policy.token_period
                                                == ShareTokenPeriod::AccountReset
                                    })
                            })
                        })
                        .map(|(_, source)| source.clone())
                        .collect();
                    let mut events: Vec<_> = shares
                        .quota_cycle_ledger
                        .public_events
                        .iter()
                        .map(|(id, time)| (id.clone(), *time))
                        .collect();
                    events.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
                    for source_ref in selected {
                        let Some(source) = shares
                            .quota_cycle_ledger
                            .source(&source_ref)
                            .filter(|source| source.provider_type == "codex_oauth")
                            .cloned()
                        else {
                            continue;
                        };
                        for (id, time) in &events {
                            shares.quota_cycle_ledger.apply_reset(
                                &source_ref,
                                &source.fence,
                                &format!("public:{id}"),
                                *time,
                                now,
                                QuotaTimeBasis::PublicAnnouncement,
                            )?;
                        }
                        let ids: Vec<_> = shares
                            .quota_cycle_sources
                            .iter()
                            .filter(|(_, source)| **source == source_ref)
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in ids {
                            shares
                                .project_account_reset(&id, &source_ref, usage, now)
                                .map_err(|_| "quota_projection_failed")?;
                        }
                    }
                    shares.quota_cycle_ledger.revision =
                        shares.quota_cycle_ledger.revision.saturating_add(1);
                    Ok(())
                })
                .await?;
            if let Err(reason) = result {
                self.try_mutate_share_quota_immediate(|shares, _, _| {
                    let cursor = &mut shares.quota_cycle_ledger.public_cursor;
                    cursor.last_error = Some(reason.into());
                    cursor.failures = cursor.failures.saturating_add(1);
                    cursor.retry_at_ms =
                        Some(now + public_resets::backoff_ms(cursor.failures, rand::random()));
                    Ok::<_, &'static str>(())
                })
                .await?
                .map_err(anyhow::Error::msg)?;
                anyhow::bail!("{reason}");
            }
            match fetched {
                Ok(Fetch::Page { page, .. }) if page.pagination.has_more => continue,
                Err(error) => return Err(error),
                _ => return Ok(()),
            }
        }
        Ok(()) // A durable continuation remains queued, not skipped to latest.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);
    impl TestDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn test_dir() -> TestDir {
        let path = std::env::temp_dir().join(format!("cc-switch-quota-fence-{}", opaque_id()));
        std::fs::create_dir(&path).unwrap();
        TestDir(path)
    }

    #[test]
    fn quota_restore_fence_survives_old_backup_replay_and_is_idempotent() {
        let dir = test_dir();
        let mut shares = ShareStore::default();
        let fence = QuotaSourceFence::default();
        let source = shares
            .quota_cycle_ledger
            .register_source(
                "identity".into(),
                "codex_oauth".into(),
                "weekly".into(),
                fence.clone(),
            )
            .unwrap();
        shares
            .quota_cycle_ledger
            .apply_reset(
                &source,
                &fence,
                "event",
                1_000,
                2_000,
                QuotaTimeBasis::PublicAnnouncement,
            )
            .unwrap();
        shares
            .quota_cycle_ledger
            .begin_redeem(ResetCreditOperation {
                operation_id: "operation".into(),
                request_id: "stable-request".into(),
                source_ref: source.clone(),
                fence,
                status: ResetCreditStatus::Dispatched,
                created_at_ms: 2_000,
                confirmed_at_ms: None,
                result: None,
                credit_fingerprint: "opaque-credit".into(),
            })
            .unwrap();
        let backup = shares.clone();
        let mut usage = UsageStore::default();
        usage.quota_ledger.begin_collection(&source, 0);
        write_restore_quota_fence(dir.path()).unwrap();
        assert!(apply_restore_quota_fence(dir.path(), &mut shares, &mut usage).unwrap());
        let stream = shares.quota_cycle_ledger.stream_incarnation.clone();
        assert_ne!(stream, backup.quota_cycle_ledger.stream_incarnation);
        assert!(shares.quota_cycle_ledger.redeem_recovery_required);
        assert_eq!(
            shares.quota_cycle_ledger.operations["operation"].status,
            ResetCreditStatus::Unknown
        );
        assert_eq!(
            shares.quota_cycle_ledger.operations["operation"].request_id,
            "stable-request"
        );
        let cycle = shares
            .quota_cycle_ledger
            .source(&source)
            .unwrap()
            .cycle
            .as_ref()
            .unwrap();
        assert!(!usage.quota_ledger.complete_for(cycle));
        assert!(!apply_restore_quota_fence(dir.path(), &mut shares, &mut usage).unwrap());
        let mut old_again = backup;
        assert!(apply_restore_quota_fence(dir.path(), &mut old_again, &mut usage).unwrap());
        assert_eq!(old_again.quota_cycle_ledger.stream_incarnation, stream);
        assert!(!crate::state::backup_targets(dir.path())
            .contains(&dir.path().join("account-reset-restore-fence.json")));
    }

    #[test]
    fn quota_invalid_restore_fence_fails_closed_without_mutation() {
        let dir = test_dir();
        crate::infra::storage::write_json_pretty(
            &dir.path().join("account-reset-restore-fence.json"),
            &serde_json::json!({"streamIncarnation":"", "restoredAtMs":-1}),
        )
        .unwrap();
        let mut shares = ShareStore::default();
        let stream = shares.quota_cycle_ledger.stream_incarnation.clone();
        assert!(
            apply_restore_quota_fence(dir.path(), &mut shares, &mut UsageStore::default()).is_err()
        );
        assert_eq!(shares.quota_cycle_ledger.stream_incarnation, stream);
    }

    fn page(events: Value, next: Option<&str>) -> public_resets::ResetPage {
        serde_json::from_value(
            json!({"data":events,"pagination":{"has_more":next.is_some(),"next_cursor":next},
            "meta":{"api_version":"v1","generated_at":"2026-10-01T00:00:00Z"}}),
        )
        .unwrap()
    }

    fn public_event(id: &str, time: &str, kind: &str) -> Value {
        json!({"id":id,"reset_type":kind,"announced_at":time,"text":"untrusted https://invalid.test", "source":{"type":"observed"}})
    }

    #[test]
    fn public_quota_catchup_resumes_after_restart_and_bad_page_keeps_cursor() {
        let mut ledger = QuotaCycleLedger::default();
        let now = 1_800_000_000_000;
        let first = page(
            json!([public_event("one", "2026-10-01T00:00:00Z", "regular")]),
            Some("next_page"),
        );
        ingest_public_quota_page(
            &mut ledger,
            &first,
            0,
            "first-query",
            &Some("etag-one".into()),
            "age=0",
            now,
        )
        .unwrap();
        let mut restarted: QuotaCycleLedger =
            serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        assert_eq!(
            restarted.public_cursor.next_cursor.as_deref(),
            Some("next_page")
        );
        let before = serde_json::to_value(&restarted).unwrap();
        let bad = page(
            json!([
                public_event("valid", "2026-10-02T00:00:00Z", "regular"),
                public_event("future", "2099-01-01T00:00:00Z", "regular")
            ]),
            None,
        );
        assert!(ingest_public_quota_page(
            &mut restarted,
            &bad,
            0,
            "next-query",
            &None,
            "age=0",
            now
        )
        .is_err());
        assert_eq!(serde_json::to_value(&restarted).unwrap(), before);
        let second = page(
            json!([
                public_event("one", "2026-10-01T00:00:00Z", "regular"),
                public_event("late", "2026-09-30T23:59:00Z", "regular"),
                public_event("banked", "2026-10-02T00:00:00Z", "banked")
            ]),
            None,
        );
        ingest_public_quota_page(
            &mut restarted,
            &second,
            0,
            "next-query",
            &None,
            "age=100",
            now,
        )
        .unwrap();
        assert_eq!(restarted.public_events.len(), 2);
        assert!(!restarted.public_events.contains_key("banked"));
        assert!(restarted.public_cursor.next_cursor.is_none());
        assert!(restarted.public_cursor.from_at_ms.unwrap() < restarted.public_events["late"]);
        // A stale CDN replay remains idempotent evidence, never a new cycle.
        let events = restarted.public_events.clone();
        ingest_public_quota_page(
            &mut restarted,
            &second,
            0,
            "next-query",
            &None,
            "age=100",
            now + 1_000,
        )
        .unwrap();
        assert_eq!(restarted.public_events, events);
    }

    #[test]
    fn public_quota_status_signal_respects_history_backoff_and_never_refreshes_account() {
        let mut ledger = QuotaCycleLedger::default();
        let now = 1_800_000_000_000;
        ledger.public_cursor.retry_at_ms = Some(now + 600_000);
        ledger.public_cursor.last_error = Some("public_history_rate_limited".into());
        apply_public_status_result(
            &mut ledger,
            &Ok(public_resets::StatusFetch::Observed {
                latest: "changed".into(),
                etag: Some("status-etag".into()),
                cache_evidence: "age=42".into(),
            }),
            now,
        );
        assert_eq!(ledger.public_cursor.retry_at_ms, Some(now + 600_000));
        assert!(ledger.events.is_empty());
        assert!(ledger.sources.is_empty());
        assert!(ledger.public_events.is_empty());
        apply_public_status_result(
            &mut ledger,
            &Ok(public_resets::StatusFetch::Limited(300_000)),
            now,
        );
        assert_eq!(ledger.public_status_cursor.retry_at_ms, Some(now + 300_000));
        apply_public_status_result(&mut ledger, &Err(anyhow::anyhow!("offline")), now);
        let delay = ledger.public_status_cursor.retry_at_ms.unwrap() - now;
        assert!((120_000..132_000).contains(&delay));
        apply_public_status_result(
            &mut ledger,
            &Ok(public_resets::StatusFetch::NotModified),
            now,
        );
        assert_eq!(ledger.public_status_cursor.failures, 0);
        assert_eq!(ledger.public_cursor.retry_at_ms, Some(now + 600_000));
        for failures in [0, 1, 4, 16, u16::MAX] {
            let delay = public_resets::backoff_ms(failures, u16::MAX);
            assert!((public_resets::POLL_MS..=public_resets::MAX_BACKOFF_MS).contains(&delay));
        }
        ledger.validate().unwrap();
    }

    #[tokio::test]
    async fn public_quota_mock_page_commit_failure_replays_before_cursor_publication() {
        let dir = test_dir();
        let state = ServerStateInner::load(
            Cli {
                host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                port: 0,
                config_dir: Some(dir.path().into()),
                web_dist_dir: None,
                log_level: "warn".into(),
                command: None,
            },
            Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap();
        state.mutate_shares_immediate(|_| ()).await.unwrap();
        let response_body =
            json!({"data":[public_event("durable", "2026-10-01T00:00:00Z", "regular")],
            "pagination":{"has_more":true,"next_cursor":"next_page"},
            "meta":{"api_version":"v1","generated_at":"2026-10-01T00:00:00Z"}})
            .to_string();
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let captured = requests.clone();
        let app = axum::Router::new().fallback(axum::routing::get(
            move |request: axum::extract::Request| {
                let body = response_body.clone();
                captured.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert!(!request.headers().contains_key("authorization"));
                async move {
                    axum::http::Response::builder()
                        .header("etag", "page-v1")
                        .body(axum::body::Body::from(body))
                        .unwrap()
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let http = public_resets::client().unwrap();
        let shares_path = dir.path().join("shares.json");
        let saved_path = dir.path().join("shares.saved.json");
        std::fs::rename(&shares_path, &saved_path).unwrap();
        std::fs::create_dir(&shares_path).unwrap();
        let before = serde_json::to_value(&state.shares.read().await.quota_cycle_ledger).unwrap();
        for should_succeed in [false, true] {
            let Fetch::Page {
                page,
                etag,
                cache_evidence,
            } = public_resets::history_from_test_origin(&http, &origin, 0, None, None)
                .await
                .unwrap()
            else {
                panic!("expected page");
            };
            let result = state
                .try_mutate_share_quota_immediate(|shares, _, _| {
                    ingest_public_quota_page(
                        &mut shares.quota_cycle_ledger,
                        &page,
                        0,
                        "same-query",
                        &etag,
                        &cache_evidence,
                        1_800_000_000_000,
                    )
                })
                .await;
            if should_succeed {
                result.unwrap().unwrap();
            } else {
                assert!(result.is_err());
                assert_eq!(
                    serde_json::to_value(&state.shares.read().await.quota_cycle_ledger).unwrap(),
                    before
                );
                std::fs::remove_dir(&shares_path).unwrap();
                std::fs::rename(&saved_path, &shares_path).unwrap();
            }
        }
        let reloaded = ShareStore::load_or_default(dir.path()).unwrap();
        reloaded.quota_cycle_ledger.validate().unwrap();
        assert_eq!(reloaded.quota_cycle_ledger.public_events.len(), 1);
        assert_eq!(
            reloaded
                .quota_cycle_ledger
                .public_cursor
                .next_cursor
                .as_deref(),
            Some("next_page")
        );
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.abort();
    }
}
