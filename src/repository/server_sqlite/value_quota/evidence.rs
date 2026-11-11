//! Unit-independent, body-free evidence. A missing price is a recorded gap,
//! never a reason to reject a Token request or invent a zero dollar amount.
use super::*;
use crate::domain::usage::store::{UsageLog, UsageRecordKind};
use crate::domain::usage::value_pricing::{ResourcePlan, ResourceUsage};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Fact {
    #[serde(default)]
    attribution: Option<crate::domain::usage::quota_ledger::QuotaAttribution>,
    model: String,
    tier: String,
    estimate: Option<RiskEstimate>,
    resource_plan: Option<ResourcePlan>,
    tokens: Option<ValueTokens>,
    resources: Option<ResourceUsage>,
    reported_model: Option<String>,
    observed_tier: Option<String>,
    outcome: ObservedOutcome,
    priced_at_admission: bool,
    /// A historical price is frozen once, independently of later catalog edits.
    migration_revision: Option<String>,
    verified: bool,
}

pub(super) fn read_fact(
    conn: &Connection,
    request: &str,
    attempt: &str,
) -> anyhow::Result<Option<Fact>> {
    let row: Option<(String, String)> = conn.query_row(
        "SELECT fact_json,fact_sha256 FROM quota_metering_facts WHERE request_id=?1 AND attempt_id=?2",
        params![request,attempt], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(json, hash)| {
        ensure!(
            sha256_hex(json.as_bytes()) == hash,
            "QUOTA_CONVERSION_EVIDENCE_CORRUPT"
        );
        Ok(serde_json::from_str(&json)?)
    })
    .transpose()
}

fn write_fact(conn: &Connection, request: &str, attempt: &str, fact: &Fact) -> anyhow::Result<()> {
    let json = serde_json::to_string(fact)?;
    conn.execute("UPDATE quota_metering_facts SET fact_json=?3,fact_sha256=?4 WHERE request_id=?1 AND attempt_id=?2",
        params![request,attempt,json,sha256_hex(json.as_bytes())])?;
    Ok(())
}

pub(super) fn record_admission(
    tx: &Transaction<'_>,
    a: &Admission<'_>,
    verified: bool,
) -> anyhow::Result<()> {
    let key = a.scope.key()?;
    if let Some(cycle) = &a.window.account {
        ensure!(
            a.attribution
                .as_ref()
                .is_some_and(|at| at.source_ref == cycle.source_ref
                    && at.source_epoch == cycle.source_epoch
                    && at.bucket_id == cycle.bucket_id
                    && at.started_at_ms == a.started_at_ms),
            "QUOTA_CONVERSION_ATTRIBUTION_CONFLICT"
        );
    }
    let fact = Fact {
        attribution: a.attribution.clone(),
        model: a.effective_model.into(),
        tier: a.effective_tier.into(),
        estimate: Some(a.estimate),
        resource_plan: a.resource_plan.clone(),
        tokens: None,
        resources: None,
        reported_model: None,
        observed_tier: None,
        outcome: ObservedOutcome::InProgress,
        priced_at_admission: false,
        migration_revision: None,
        verified,
    };
    let json = serde_json::to_string(&fact)?;
    tx.execute("INSERT INTO quota_metering_facts(scope,request_id,attempt_id,started_at_ms,accounted_at_ms,usage_revision,quota_tokens,request_count,fact_json,fact_sha256,account_key)
        VALUES(?1,?2,?3,?4,NULL,0,0,0,?5,?6,?7) ON CONFLICT(request_id,attempt_id) DO NOTHING",
        params![key,a.request_id,a.attempt_id,a.started_at_ms,json,sha256_hex(json.as_bytes()),attribution_key(a.attribution.as_ref())?])?;
    let stored: (String,i64)=tx.query_row("SELECT scope,started_at_ms FROM quota_metering_facts WHERE request_id=?1 AND attempt_id=?2",
        params![a.request_id,a.attempt_id],|r|Ok((r.get(0)?,r.get(1)?)))?;
    ensure!(
        stored == (key, a.started_at_ms),
        "VALUE_QUOTA_REQUEST_IDENTITY_CONFLICT"
    );
    Ok(())
}

/// The savepoint only isolates optional pricing. Durable evidence must commit
/// before sending upstream, even when a catalog, tier or resource is unknown.
pub(crate) fn admit_shadow(
    dir: &Path,
    a: &Admission<'_>,
    verified: bool,
) -> anyhow::Result<Admitted> {
    assert_committed(dir)?;
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    record_admission(&tx, a, verified)?;
    let mut priced = false;
    tx.execute_batch("SAVEPOINT optional_shadow_price;")?;
    let result = (|| {
        ensure!(verified, "VALUE_QUOTA_UNVERIFIED_SURFACE");
        let revision = tariff::admission_revision(&tx, a, crate::infra::time::now_ms() as i64)?;
        admit_with_trial_in_tx(
            &tx,
            &Admission {
                pricing_revision: &revision,
                ..a.clone()
            },
            None,
        )
    })();
    if result.is_ok() {
        tx.execute_batch("RELEASE optional_shadow_price;")?;
        priced = true;
    } else {
        tx.execute_batch("ROLLBACK TO optional_shadow_price; RELEASE optional_shadow_price;")?;
    }
    if priced {
        if let Some(mut fact) = read_fact(&tx, a.request_id, a.attempt_id)? {
            fact.priced_at_admission = true;
            write_fact(&tx, a.request_id, a.attempt_id, &fact)?;
        }
    }
    tx.commit()?;
    Ok(Admitted { reserved_picos: 0 })
}

pub(super) fn observe_tokens(
    tx: &Transaction<'_>,
    request: &str,
    attempt: &str,
    tokens: ValueTokens,
    estimated: bool,
) -> anyhow::Result<()> {
    if let Some(mut fact) = read_fact(tx, request, attempt)? {
        fact.tokens = Some(tokens);
        fact.outcome = if estimated {
            ObservedOutcome::FinalConservative
        } else {
            ObservedOutcome::Final
        };
        write_fact(tx, request, attempt, &fact)?;
    }
    Ok(())
}

pub(super) fn outcome(
    tx: &Transaction<'_>,
    request: &str,
    attempt: &str,
    outcome: AttemptOutcome,
) -> anyhow::Result<()> {
    if let Some(mut fact) = read_fact(tx, request, attempt)? {
        if fact.outcome == ObservedOutcome::InProgress {
            fact.outcome = match outcome {
                AttemptOutcome::NotSent | AttemptOutcome::UpstreamError => ObservedOutcome::Free,
                AttemptOutcome::Unknown => ObservedOutcome::Unknown,
            };
            write_fact(tx, request, attempt, &fact)?;
        }
    }
    Ok(())
}

/// Preserve original admission identity and pricing. User-invocation Token
/// totals appear only on the log's own attempt, never on earlier retries.
pub(crate) fn capture_log(
    tx: &Transaction<'_>,
    log: &UsageLog,
    identity: Option<&Scope>,
) -> anyhow::Result<()> {
    if log.is_health_check || log.record_kind == UsageRecordKind::HealthProbe {
        return Ok(());
    }
    resource_tasks::capture_async_fact(tx, log)?;
    let attempt = log.attempt_count.max(1).to_string();
    let existing = read_fact(tx, &log.request_id, &attempt)?;
    let identity = match identity {
        Some(scope) => scope.clone(),
        None => {
            let key: Option<String> = tx
                .query_row(
                    "SELECT scope FROM quota_metering_facts WHERE request_id=?1 AND attempt_id=?2",
                    params![log.request_id, attempt],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(key) = key else {
                return Ok(());
            };
            serde_json::from_str(&key)?
        }
    };
    ensure!(
        log.share_id.as_deref() == Some(&identity.share_id)
            && log
                .user_email
                .as_ref()
                .is_some_and(|e| e.eq_ignore_ascii_case(&identity.email)),
        "VALUE_QUOTA_USAGE_SCOPE_CONFLICT"
    );
    let stored_scope: Option<String> = tx
        .query_row(
            "SELECT scope FROM quota_metering_facts WHERE request_id=?1 AND attempt_id=?2",
            params![log.request_id, attempt],
            |r| r.get(0),
        )
        .optional()?;
    // Historical imports cannot reassign another installation or entitlement.
    if stored_scope
        .as_ref()
        .is_some_and(|key| identity.key().as_ref().ok() != Some(key))
    {
        return Ok(());
    }
    if existing.is_none() && !legacy_scope_matches(tx, &identity, log)? {
        return Ok(());
    }
    let mut fact = existing.unwrap_or(Fact {
        attribution: log.quota_attribution.clone(),
        model: log
            .actual_model
            .as_ref()
            .or(log.model.as_ref())
            .cloned()
            .unwrap_or_default(),
        tier: canonical_service_tier(log.effective_service_tier.as_deref())
            .unwrap_or("unknown")
            .into(),
        estimate: None,
        resource_plan: log.value_resources.as_ref().map(|r| ResourcePlan {
            quantities: r.quantities.clone(),
            text_tokens: log.request_kind == crate::domain::usage::store::RequestKind::Text,
            requires_v2: true,
        }),
        tokens: None,
        resources: None,
        reported_model: None,
        observed_tier: None,
        outcome: ObservedOutcome::Unknown,
        priced_at_admission: false,
        migration_revision: None,
        verified: true,
    });
    let meta = fact.estimate.map(|estimate| AttemptMeta {
        input_tokens: estimate.input_tokens,
        output_tokens: estimate.output_tokens,
        reserved_picos: "0".into(),
        resource_plan: fact.resource_plan.clone(),
        ..Default::default()
    });
    let (outcome, tokens, tier) = classify_log(log, &fact.model, &fact.tier, meta.as_ref(), false);
    if fact.attribution.is_none() {
        fact.attribution = log.quota_attribution.clone();
    }
    ensure!(
        log.quota_attribution
            .as_ref()
            .is_none_or(|a| fact.attribution.as_ref() == Some(a)),
        "QUOTA_CONVERSION_ATTRIBUTION_CONFLICT"
    );
    fact.outcome = outcome;
    fact.tokens = tokens;
    fact.observed_tier = tier.map(str::to_owned);
    fact.reported_model = log.reported_model.clone();
    fact.resources =
        resource_usage_for_log(log, meta.as_ref()).or_else(|| log.value_resources.clone());
    let json = serde_json::to_string(&fact)?;
    // Accounted-at intentionally retains the existing Token aggregation clock.
    // USD amounts retain original admission/start attribution across late logs.
    let quota_record = matches!(
        log.record_kind,
        UsageRecordKind::UserInference | UsageRecordKind::InternalSupplemental
    );
    tx.execute("INSERT INTO quota_metering_facts(scope,request_id,attempt_id,started_at_ms,accounted_at_ms,usage_revision,quota_tokens,request_count,fact_json,fact_sha256,account_key)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11) ON CONFLICT(request_id,attempt_id) DO UPDATE SET
        accounted_at_ms=excluded.accounted_at_ms,usage_revision=excluded.usage_revision,quota_tokens=excluded.quota_tokens,
        request_count=excluded.request_count,account_key=excluded.account_key,fact_json=excluded.fact_json,fact_sha256=excluded.fact_sha256
        WHERE excluded.usage_revision>=quota_metering_facts.usage_revision AND scope=excluded.scope",
        params![identity.key()?,log.request_id,attempt,to_sql_u128(log.started_at_ms)?,to_sql_u128(log.created_at_ms)?,
            to_sql_u64(log.usage_revision)?,to_sql_u64(if quota_record {log.quota_tokens()} else {0})?,
            i64::from(log.record_kind==UsageRecordKind::UserInference),json,sha256_hex(json.as_bytes()),attribution_key(fact.attribution.as_ref())?])?;
    Ok(())
}

/// Rebuild only unpriced attempts. An existing amount always wins, including
/// its original tariff and its revisions; switching never re-prices it.
pub(super) fn materialize(
    tx: &Transaction<'_>,
    identity: &Scope,
    request: &str,
    attempt: &str,
    revision: &str,
) -> anyhow::Result<bool> {
    if read_attempt_row(tx, request, attempt)?.is_some() {
        return Ok(true);
    }
    let Some(mut fact) = read_fact(tx, request, attempt)? else {
        return Ok(false);
    };
    if fact.outcome == ObservedOutcome::Free {
        return Ok(true);
    }
    if !fact.verified {
        return Ok(false);
    }
    if matches!(
        fact.outcome,
        ObservedOutcome::Unknown | ObservedOutcome::InProgress
    ) && fact
        .estimate
        .is_none_or(|e| e.input_tokens.saturating_add(e.output_tokens) == 0)
        && !fact
            .resource_plan
            .as_ref()
            .is_some_and(|p| p.quantities.values().any(|q| *q > 0))
    {
        return Ok(false);
    }
    if matches!(
        fact.outcome,
        ObservedOutcome::Final | ObservedOutcome::FinalConservative | ObservedOutcome::Interrupted
    ) && fact.tokens.is_none()
        && fact.resources.is_none()
    {
        return Ok(false);
    }
    // A legacy log for N retries describes only its last attempt. Earlier
    // attempts need their own evidence; the preparation check rejects holes.
    let started: i64 = tx.query_row(
        "SELECT started_at_ms FROM quota_metering_facts WHERE request_id=?1 AND attempt_id=?2",
        params![request, attempt],
        |r| r.get(0),
    )?;
    let w = Window {
        account: None,
        id: "evidence".into(),
        starts_at_ms: 0,
        ends_at_ms: None,
        metered_from_ms: 0,
    };
    let quota = ValueQuota {
        limit_usd_micros: "1".into(),
        tariff_policy_id: if fact.resource_plan.as_ref().is_some_and(|p| p.requires_v2) {
            crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY
        } else {
            crate::domain::usage::value_pricing::TARIFF_POLICY
        }
        .into(),
    };
    tx.execute_batch("SAVEPOINT materialize_fact;")?;
    let result = (|| -> anyhow::Result<()> {
        admit_with_trial_in_tx(
            tx,
            &Admission {
                attribution: fact.attribution.clone(),
                enforce: false,
                scope: identity,
                window: &w,
                policy_epoch: "evidence",
                quota: &quota,
                pricing_revision: revision,
                request_id: request,
                attempt_id: attempt,
                effective_model: &fact.model,
                effective_tier: &fact.tier,
                started_at_ms: started,
                estimate: fact.estimate.unwrap_or_default(),
                resource_plan: fact.resource_plan.clone(),
                parallel_limit: 4,
            },
            None,
        )?;
        settle_in_tx(
            tx,
            &Observation {
                request_id: request,
                attempt_id: attempt,
                scope: identity,
                usage_revision: 1,
                effective_model: &fact.model,
                effective_tier: &fact.tier,
                reported_model: fact.reported_model.as_deref(),
                observed_tier: fact.observed_tier.as_deref(),
                tokens: fact.tokens,
                resources: fact.resources.clone(),
                outcome: fact.outcome,
                synthetic: false,
            },
        )?;
        Ok(())
    })();
    if result.is_err() {
        tx.execute_batch("ROLLBACK TO materialize_fact; RELEASE materialize_fact;")?;
        return Ok(false);
    }
    tx.execute_batch("RELEASE materialize_fact;")?;
    fact.migration_revision = Some(revision.into());
    write_fact(tx, request, attempt, &fact)?;
    Ok(true)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preparation {
    pub state: String,
    pub processed: u64,
    pub pricing_revision: String,
}

/// One indexed, resumable page. No network, no full-history transaction.
pub(crate) fn prepare_page(
    dir: &Path,
    identity: &Scope,
    restart: bool,
    now: i64,
) -> anyhow::Result<Preparation> {
    assert_committed(dir)?;
    let revision = active_revision(dir, identity)?.context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result = prepare_page_in_tx(&tx, identity, restart, now, revision)?;
    tx.commit()?;
    Ok(result)
}

pub(super) fn prepare_page_in_tx(
    tx: &Transaction<'_>,
    identity: &Scope,
    restart: bool,
    now: i64,
    revision: String,
) -> anyhow::Result<Preparation> {
    let key = identity.key()?;
    let old:Option<(String,String,String,i64)>=tx.query_row("SELECT cursor,pricing_revision,state,processed FROM quota_history_preparations WHERE scope=?1",
        [&key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (cursor, basis, mut state, mut processed) = match old {
        Some(old) if !restart && old.2 != "complete" => old,
        _ => (String::new(), revision, "preparing".into(), 0),
    };
    let (phase, last): (&str, String) = if let Some(last) = cursor.strip_prefix("facts:") {
        ("facts", last.into())
    } else {
        ("logs", cursor.strip_prefix("logs:").unwrap_or("").into())
    };
    let mut next_cursor = cursor;
    if phase == "logs" {
        let rows = {
            let mut q=tx.prepare("SELECT request_id,payload_json,payload_sha256 FROM usage_records WHERE share_id=?1 AND lower(json_extract(payload_json,'$.userEmail'))=?3 AND request_id>?2 ORDER BY request_id LIMIT 500")?;
            let mapped = q.query_map(params![identity.share_id, last, identity.email], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, json, hash) in &rows {
            ensure!(
                sha256_hex(json.as_bytes()) == *hash,
                "QUOTA_CONVERSION_EVIDENCE_CORRUPT"
            );
            let log: UsageLog = serde_json::from_str(json)?;
            if log
                .user_email
                .as_ref()
                .is_some_and(|e| e.eq_ignore_ascii_case(&identity.email))
            {
                capture_log(tx, &log, Some(identity))?;
                processed += 1;
            }
            next_cursor = format!("logs:{id}");
        }
        if rows.len() < 500 {
            next_cursor = "facts:".into();
        }
    } else {
        let after: (String, String) = if last.is_empty() {
            (String::new(), String::new())
        } else {
            serde_json::from_str(&last)?
        };
        let rows = {
            let mut q=tx.prepare("SELECT request_id,attempt_id FROM quota_metering_facts WHERE scope=?1 AND (request_id,attempt_id)>(?2,?3) ORDER BY request_id,attempt_id LIMIT 500")?;
            let mapped = q.query_map(params![key, after.0, after.1], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            mapped.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (request, attempt) in &rows {
            materialize(tx, identity, request, attempt, &basis)?;
            processed = processed
                .checked_add(1)
                .context("history progress overflow")?;
            next_cursor = format!("facts:{}", serde_json::to_string(&(request, attempt))?);
        }
        if rows.len() < 500 {
            state = "complete".into();
        }
    }
    tx.execute("INSERT INTO quota_history_preparations VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(scope) DO UPDATE SET cursor=excluded.cursor,pricing_revision=excluded.pricing_revision,state=excluded.state,processed=excluded.processed,updated_at_ms=excluded.updated_at_ms",
        params![key,next_cursor,basis,state,processed,now])?;
    Ok(Preparation {
        state,
        processed: u64::try_from(processed)?,
        pricing_revision: basis,
    })
}

pub(super) fn missing_in(conn: &Connection, scope: &Scope, w: &Window) -> anyhow::Result<u64> {
    let missing:i64=conn.query_row("SELECT count(*) FROM quota_metering_facts f WHERE f.scope=?1 AND f.started_at_ms>=?2
        AND (?3 IS NULL OR f.started_at_ms<?3) AND (?4 IS NULL OR f.account_key=?4) AND json_extract(f.fact_json,'$.outcome')!='free' AND NOT EXISTS(SELECT 1 FROM value_quota_attempts a WHERE a.request_id=f.request_id AND a.attempt_id=f.attempt_id)",
        params![scope.key()?,w.metered_start_ms(),w.ends_at_ms,w.account_key()?],|r|r.get(0))?;
    Ok(u64::try_from(missing)?)
}

pub(super) fn totals_in(
    conn: &Connection,
    scope: &Scope,
    w: &Window,
    from: i64,
    until: Option<i64>,
) -> anyhow::Result<(u64, u64)> {
    let totals: (i64, i64) = if w.account.is_some() {
        conn.query_row("SELECT COALESCE(sum(quota_tokens),0),COALESCE(sum(request_count),0) FROM quota_metering_facts WHERE scope=?1 AND account_key=?2 AND started_at_ms>=?3 AND (?4 IS NULL OR started_at_ms<?4)",params![scope.key()?,w.account_key()?,from,until],|r|Ok((r.get(0)?,r.get(1)?)))?
    } else {
        conn.query_row("SELECT COALESCE(sum(quota_tokens),0),COALESCE(sum(request_count),0) FROM quota_metering_facts WHERE scope=?1 AND accounted_at_ms>=?2 AND (?3 IS NULL OR accounted_at_ms<?3)",params![scope.key()?,from,until],|r|Ok((r.get(0)?,r.get(1)?)))?
    };
    Ok((u64::try_from(totals.0)?, u64::try_from(totals.1)?))
}

pub(super) fn attribution_key(
    a: Option<&crate::domain::usage::quota_ledger::QuotaAttribution>,
) -> anyhow::Result<Option<String>> {
    a.map(|a| {
        serde_json::to_string(&(&a.source_ref, a.source_epoch, &a.bucket_id)).map_err(Into::into)
    })
    .transpose()
}

/// Account windows cannot use calendar checkpoints containing other sources.
/// The covering attribution index restricts this scan to the exact account,
/// incarnation and bucket; the cycle's boundaries restrict its generation.
pub(super) fn account_aggregate(
    conn: &Connection,
    scope: &Scope,
    w: &Window,
) -> anyhow::Result<Checkpoint> {
    let mut aggregate = Checkpoint::empty();
    let mut q=conn.prepare("SELECT a.usage_picos,a.pending_risk_picos,a.metering_status,a.watermark,a.pricing_revision
        FROM quota_metering_facts f JOIN value_quota_attempts a USING(request_id,attempt_id)
        WHERE f.scope=?1 AND f.account_key=?2 AND f.started_at_ms>=?3 AND (?4 IS NULL OR f.started_at_ms<?4)")?;
    let rows = q.query_map(
        params![
            scope.key()?,
            w.account_key()?,
            w.metered_start_ms(),
            w.ends_at_ms
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        },
    )?;
    for row in rows {
        let (amount, risk, state, mark, revision) = row?;
        aggregate.change(
            &amount,
            &risk,
            &state,
            Some(&revision),
            u64::try_from(mark)?,
            true,
        )?;
    }
    // Unattributed legacy amounts and gaps are uncertainty, never $0. They
    // require retained logs/evidence or an explicit reconciliation first.
    let unknown:i64=conn.query_row("SELECT
        (SELECT count(*) FROM value_quota_attempts a LEFT JOIN quota_metering_facts f USING(request_id,attempt_id)
         WHERE a.scope=?1 AND a.started_at_ms>=?2 AND (?3 IS NULL OR a.started_at_ms<?3) AND f.account_key IS NULL)+
        (SELECT count(*) FROM value_quota_gaps WHERE scope=?1 AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3))",
        params![scope.key()?,w.metered_start_ms(),w.ends_at_ms],|r|r.get(0))?;
    if unknown > 0 {
        aggregate
            .states
            .insert("gap".into(), u64::try_from(unknown)?);
    }
    Ok(aggregate)
}

/// Price-independent Token reservations. Retries retain the invocation Token
/// contract: one request contributes its largest live ceiling, not N copies.
pub(crate) fn pending_tokens(
    dir: &Path,
    scope: &Scope,
    w: &Window,
    exclude_request: Option<&str>,
) -> anyhow::Result<(u64, bool)> {
    let conn = open_database(&database_path(dir), true)?;
    pending_tokens_in(&conn, scope, w, exclude_request)
}
pub(super) fn pending_tokens_in(
    conn: &Connection,
    scope: &Scope,
    w: &Window,
    exclude: Option<&str>,
) -> anyhow::Result<(u64, bool)> {
    let mut q=conn.prepare("SELECT request_id,fact_json,quota_tokens FROM quota_metering_facts WHERE scope=?1 AND started_at_ms>=?2
        AND (?3 IS NULL OR started_at_ms<?3) AND (?4 IS NULL OR account_key=?4) AND (?5 IS NULL OR request_id!=?5)
        AND (json_extract(fact_json,'$.outcome')='inProgress' OR (accounted_at_ms IS NULL AND json_extract(fact_json,'$.outcome') IN ('final','finalConservative')))")?;
    let rows = q.query_map(
        params![
            scope.key()?,
            w.metered_start_ms(),
            w.ends_at_ms,
            w.account_key()?,
            exclude
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u64>(2)?,
            ))
        },
    )?;
    let mut pending = BTreeMap::<String, u64>::new();
    // Pre-v6 monetary reservations may not have an independent Token fact.
    // An empty fact query must not release that live risk on USD -> Token.
    // Missing account attribution is also uncertainty for every account cycle.
    let missing: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM value_quota_attempts a
         LEFT JOIN quota_metering_facts f USING(request_id,attempt_id)
         WHERE a.scope=?1 AND a.started_at_ms>=?2 AND (?3 IS NULL OR a.started_at_ms<?3)
         AND (?5 IS NULL OR a.request_id!=?5) AND a.metering_status='pending'
         AND (f.request_id IS NULL OR (?4 IS NOT NULL AND f.account_key IS NULL)))",
        params![
            scope.key()?,
            w.metered_start_ms(),
            w.ends_at_ms,
            w.account_key()?,
            exclude
        ],
        |r| r.get(0),
    )?;
    let mut complete = !missing;
    for row in rows {
        let (request, json, accounted) = row?;
        let f: Fact = serde_json::from_str(&json)?;

        if f.resource_plan.as_ref().is_some_and(|p| !p.text_tokens) {
            complete = false;
            continue;
        }
        let bound = if f.outcome == ObservedOutcome::InProgress {
            f.estimate
                .and_then(|e| e.input_tokens.checked_add(e.output_tokens))
        } else {
            f.tokens.and_then(|t| {
                t.raw_input()
                    .ok()
                    .and_then(|input| input.checked_add(t.output))
            })
        };
        match bound.filter(|n| *n > 0) {
            Some(bound) => {
                let entry = pending.entry(request).or_default();
                *entry = (*entry).max(bound.saturating_sub(accounted));
            }
            None => complete = false,
        }
    }
    Ok((
        pending.values().try_fold(0u64, |total, n| {
            total.checked_add(*n).context("Token arithmetic overflow")
        })?,
        complete,
    ))
}

pub(super) fn pending_value_in(
    conn: &Connection,
    scope: &Scope,
    w: &Window,
) -> anyhow::Result<(i128, bool)> {
    let mut risk = 0i128;
    let mut complete = true;
    let mut q=conn.prepare("SELECT a.request_id,a.attempt_id,a.usage_picos FROM value_quota_attempts a
        LEFT JOIN quota_metering_facts f USING(request_id,attempt_id) WHERE a.scope=?1 AND a.started_at_ms>=?2
        AND (?3 IS NULL OR a.started_at_ms<?3) AND (?4 IS NULL OR f.account_key=?4) AND a.metering_status='pending'")?;
    let rows = q.query_map(
        params![
            scope.key()?,
            w.metered_start_ms(),
            w.ends_at_ms,
            w.account_key()?
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        },
    )?;
    for row in rows {
        let (request, attempt, amount) = row?;
        let meta = read_attempt_meta(conn, &request, &attempt)?;
        match meta {
            Some(m)
                if m.input_tokens.saturating_add(m.output_tokens) > 0
                    || m.resource_plan
                        .as_ref()
                        .is_some_and(|p| p.quantities.values().any(|n| *n > 0)) =>
            {
                risk = risk
                    .checked_add(
                        m.reserved()
                            .saturating_sub(unsigned_decimal(&amount)?)
                            .max(0),
                    )
                    .context("value quota arithmetic overflow")?;
            }
            _ => complete = false,
        }
    }
    Ok((risk, complete))
}

/// A positive resource/conservative charge has no exact Token counterpart.
/// Its audit identity is stable so a round trip reuses the calibration once.
pub(super) fn non_token_basis(
    conn: &Connection,
    scope: &Scope,
    w: &Window,
) -> anyhow::Result<Option<String>> {
    let mut q=conn.prepare("SELECT a.request_id,a.attempt_id,a.usage_picos,a.metering_status,f.fact_json FROM value_quota_attempts a
        LEFT JOIN quota_metering_facts f USING(request_id,attempt_id) WHERE a.scope=?1 AND a.started_at_ms>=?2
        AND (?3 IS NULL OR a.started_at_ms<?3) AND (?4 IS NULL OR f.account_key=?4) AND a.usage_picos!='0' ORDER BY a.request_id,a.attempt_id")?;
    let rows = q.query_map(
        params![
            scope.key()?,
            w.metered_start_ms(),
            w.ends_at_ms,
            w.account_key()?
        ],
        |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        },
    )?;
    let mut basis = Vec::new();
    for row in rows {
        let (request, attempt, amount, state, json) = row?;
        let f = json.map(|j| serde_json::from_str::<Fact>(&j)).transpose()?;
        if state == "conservative"
            || f.as_ref().is_none_or(|f| {
                f.tokens.is_none()
                    || f.resource_plan.as_ref().is_some_and(|p| !p.text_tokens)
                    || f.resources
                        .as_ref()
                        .is_some_and(|r| r.quantities.values().any(|n| *n > 0))
            })
        {
            basis.push((request, attempt, amount));
        }
    }
    Ok((!basis.is_empty()).then(|| {
        format!(
            "non-token:{}",
            sha256_hex(serde_json::to_string(&basis).unwrap().as_bytes())
        )
    }))
}

/// Persist completion even for a Token generation whose tariff was unknown.
pub(super) fn observe_task_fact(
    tx: &Transaction<'_>,
    request: &str,
    attempt: &str,
    log: &UsageLog,
    completed: bool,
) -> anyhow::Result<()> {
    if let Some(mut f) = read_fact(tx, request, attempt)? {
        let meta = f.estimate.map(|e| AttemptMeta {
            input_tokens: e.input_tokens,
            output_tokens: e.output_tokens,
            resource_plan: f.resource_plan.clone(),
            ..Default::default()
        });
        f.resources =
            resource_usage_for_log(log, meta.as_ref()).or_else(|| log.value_resources.clone());
        f.tokens = Some(ValueTokens::default());
        f.outcome = if completed {
            ObservedOutcome::Final
        } else {
            ObservedOutcome::Unknown
        };
        write_fact(tx, request, attempt, &f)?;
    }
    Ok(())
}

pub(super) fn unresolved_in(conn: &Connection, scope: &Scope, w: &Window) -> anyhow::Result<u64> {
    let count:i64=conn.query_row("SELECT
        (SELECT count(*) FROM value_quota_gaps WHERE scope=?1 AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3))+
        (SELECT count(*) FROM value_quota_attempts a LEFT JOIN quota_metering_facts f USING(request_id,attempt_id)
         WHERE a.scope=?1 AND a.started_at_ms>=?2 AND (?3 IS NULL OR a.started_at_ms<?3)
         AND (a.metering_status='unavailable' OR (a.metering_status='pending' AND COALESCE(json_extract(f.fact_json,'$.outcome'),'unknown') NOT IN ('inProgress','free')) OR (?4 IS NOT NULL AND f.account_key IS NULL)))",
        params![scope.key()?,w.metered_start_ms(),w.ends_at_ms,w.account_key()?],|r|r.get(0))?;
    Ok(u64::try_from(count)?)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyOrigin {
    router_url: String,
    installation_id: String,
    through_ms: i64,
    excluded: std::collections::BTreeSet<(String, String)>,
}

/// Pin the upgrade's identity before traffic/config edits. Legacy logs have
/// no Router identity and must never be relabelled after a Router switch.
pub(crate) fn pin_legacy_origin(
    dir: &Path,
    config: &crate::domain::settings::config::ServerConfig,
    shares: &ShareStore,
) -> anyhow::Result<()> {
    if !is_committed(dir)? {
        return Ok(());
    }
    let conn = open_database(&database_path(dir), false)?;
    let mut excluded = std::collections::BTreeSet::new();
    for share in &shares.shares {
        for grant in share.user_grants.values().filter(|g| {
            g.manager
                == crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket
        }) {
            excluded.insert((share.id.clone(), grant.email.to_ascii_lowercase()));
        }
        for entitlement in share.market_entitlements.values() {
            excluded.insert((share.id.clone(), entitlement.email.to_ascii_lowercase()));
        }
    }
    let origin = LegacyOrigin {
        router_url: config.router_api_base().unwrap_or("").into(),
        installation_id: config
            .registered_router_identity()
            .map_or("", |i| i.installation_id.as_str())
            .into(),
        through_ms: crate::infra::time::now_ms() as i64,
        excluded,
    };
    conn.execute("INSERT INTO meta(key,value) VALUES('quota_evidence_legacy_origin_v6',?1) ON CONFLICT(key) DO NOTHING",[serde_json::to_string(&origin)?])?;
    Ok(())
}
fn legacy_scope_matches(
    conn: &Connection,
    identity: &Scope,
    log: &UsageLog,
) -> anyhow::Result<bool> {
    if let Some(row) =
        read_attempt_row(conn, &log.request_id, &log.attempt_count.max(1).to_string())?
    {
        return Ok(row.0 == identity.key()?);
    }
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='quota_evidence_legacy_origin_v6'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(false);
    };
    let origin: LegacyOrigin = serde_json::from_str(&raw)?;
    Ok(identity.entitlement_id.is_none()
        && identity.router_url == origin.router_url
        && identity.installation_id == origin.installation_id
        && i64::try_from(log.created_at_ms).is_ok_and(|at| at <= origin.through_ms)
        && !origin
            .excluded
            .contains(&(identity.share_id.clone(), identity.email.clone())))
}
