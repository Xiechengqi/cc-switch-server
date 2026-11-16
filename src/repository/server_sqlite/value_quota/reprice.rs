//! Re-pricing of owner-managed `$0` attempts once a covering tariff revision
//! activates. Activation only records a durable job; the background loop
//! runs it in short pages with a persisted cursor (resumable after restart)
//! and no network I/O inside a transaction. A re-priced row has its
//! `unpriced_reason` cleared, so a repeated run is a no-op for it. Only rows
//! of the activating installation inside a window that has not ended (and,
//! for account-reset windows, of the same account bucket) are re-priced;
//! ended windows keep $0. Amounts come from observed evidence only, never
//! from an admission estimate ceiling.
use super::*;
use crate::domain::usage::value_pricing::price_type_unpriced_reason;

const REPRICE_PAGE: usize = 100;

fn reprice_key(scope: &Scope) -> String {
    format!(
        "value_reprice_due:{}:{}",
        scope.router_url, scope.installation_id
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RepriceJob {
    revision: String,
    /// Last visited `(scope, started_at_ms, request_id, attempt_id)`.
    #[serde(default)]
    after: Option<(String, i64, String, String)>,
}

/// Called in the activation transaction; a newer activation restarts the scan
/// because its prices may cover rows the previous run already passed.
pub(super) fn mark_due(conn: &Connection, scope: &Scope, revision: &str) -> anyhow::Result<()> {
    let job = RepriceJob {
        revision: revision.into(),
        after: None,
    };
    conn.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![reprice_key(scope), serde_json::to_string(&job)?],
    )?;
    Ok(())
}

pub(crate) fn reprice_due(dir: &Path, scope: &Scope) -> anyhow::Result<bool> {
    assert_committed(dir)?;
    let conn = open_database(&database_path(dir), true)?;
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)",
        [reprice_key(scope)],
        |r| r.get(0),
    )?)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RepricePage {
    pub repriced: usize,
    pub done: bool,
}

/// One bounded page in one IMMEDIATE transaction. `windows` maps a scope key
/// to the currently open window of that owner-managed grant; scopes without
/// an open window (ended, Market, revoked) are skipped.
pub(crate) fn reprice_page(
    dir: &Path,
    installation: &Scope,
    windows: &BTreeMap<String, Window>,
    now: i64,
) -> anyhow::Result<RepricePage> {
    assert_committed(dir)?;
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let page = reprice_page_in_tx(&tx, installation, windows, now)?;
    tx.commit()?;
    Ok(page)
}

pub(super) fn reprice_page_in_tx(
    tx: &Transaction<'_>,
    installation: &Scope,
    windows: &BTreeMap<String, Window>,
    now: i64,
) -> anyhow::Result<RepricePage> {
    let key = reprice_key(installation);
    let raw: Option<String> = tx
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
        .optional()?;
    let Some(raw) = raw else {
        return Ok(RepricePage {
            repriced: 0,
            done: true,
        });
    };
    let mut job: RepriceJob =
        serde_json::from_str(&raw).context("VALUE_QUOTA_REPRICE_JOB_CORRUPT")?;
    let Some(revision) = tariff::activate_due_in_tx(tx, installation, now)? else {
        tx.execute("DELETE FROM meta WHERE key=?1", [&key])?;
        return Ok(RepricePage {
            repriced: 0,
            done: true,
        });
    };
    // A due activation inside this transaction rewrote the job; reread it.
    if let Some(raw) = tx
        .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
    {
        job = serde_json::from_str(&raw).context("VALUE_QUOTA_REPRICE_JOB_CORRUPT")?;
    }
    let snapshot = cached_snapshot(
        tx,
        &installation.router_url,
        &installation.installation_id,
        &revision,
    )?;
    let after = job.after.clone();
    // Scopes are visited in key order; each query is confined in SQL to the
    // activating installation, the scope's open window and (account-reset)
    // its bucket, so ended windows and other installations are never read.
    let mut rows: Vec<(String, i64, String, String)> = Vec::new();
    let mut q = tx.prepare(
        "SELECT a.started_at_ms,a.request_id,a.attempt_id FROM value_quota_attempts a
         WHERE a.scope=?1 AND a.unpriced_reason IS NOT NULL AND a.unpriced_reason!='usage_unobserved'
         AND json_extract(a.scope,'$.routerUrl')=?2 AND json_extract(a.scope,'$.installationId')=?3
         AND a.started_at_ms>=?4 AND (?5 IS NULL OR a.started_at_ms<?5)
         AND (a.started_at_ms,a.request_id,a.attempt_id)>(?6,?7,?8)
         AND (?9 IS NULL OR EXISTS(SELECT 1 FROM quota_metering_facts f
           WHERE f.request_id=a.request_id AND f.attempt_id=a.attempt_id AND f.account_key=?9))
         ORDER BY a.started_at_ms,a.request_id,a.attempt_id LIMIT ?10",
    )?;
    let first = after.as_ref().map_or(String::new(), |a| a.0.clone());
    for (scope_key, w) in windows.range(first..) {
        let remaining = REPRICE_PAGE - rows.len();
        if remaining == 0 {
            break;
        }
        let Ok(scope) = serde_json::from_str::<Scope>(scope_key) else {
            continue;
        };
        if scope.router_url != installation.router_url
            || scope.installation_id != installation.installation_id
            || scope.entitlement_id.is_some()
        {
            continue;
        }
        let (at, request, attempt) = match after.as_ref().filter(|a| &a.0 == scope_key) {
            Some(a) => (a.1, a.2.clone(), a.3.clone()),
            None => (i64::MIN, String::new(), String::new()),
        };
        let mapped = q.query_map(
            params![
                scope_key,
                installation.router_url,
                installation.installation_id,
                w.starts_at_ms,
                w.ends_at_ms,
                at,
                request,
                attempt,
                w.account_key()?,
                remaining as i64
            ],
            |r| {
                Ok((
                    scope_key.clone(),
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?;
        rows.extend(mapped.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    drop(q);
    let mut repriced = 0;
    for (_, _, request, attempt) in &rows {
        tx.execute_batch("SAVEPOINT value_reprice")?;
        match reprice_row_in_tx(tx, request, attempt, &revision, &snapshot) {
            Ok(changed) => {
                tx.execute_batch("RELEASE value_reprice")?;
                repriced += usize::from(changed);
            }
            Err(error) => {
                tx.execute_batch("ROLLBACK TO value_reprice; RELEASE value_reprice")?;
                tracing::warn!(%error, request_id = %request, "unpriced USD attempt kept at $0; re-pricing skipped");
            }
        }
    }
    let done = rows.len() < REPRICE_PAGE;
    if done {
        tx.execute("DELETE FROM meta WHERE key=?1", [&key])?;
    } else {
        job.after = rows.last().cloned();
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, serde_json::to_string(&job)?],
        )?;
    }
    Ok(RepricePage { repriced, done })
}

/// Re-freezes one `$0` attempt with the covering model price and, when it is
/// already terminal, re-settles it from its stored fact (tokens, resources,
/// outcome) at the same usage revision. Pending attempts only receive the
/// tariff: their own settlement prices them. Returns false when the row is
/// still unpriced under this revision (it keeps its reason).
pub(super) fn reprice_row_in_tx(
    tx: &Transaction<'_>,
    request: &str,
    attempt: &str,
    revision: &str,
    snapshot: &PricingSnapshot,
) -> anyhow::Result<bool> {
    let Some(old) = read_attempt_row(tx, request, attempt)? else {
        return Ok(false);
    };
    let reason: Option<String> = tx.query_row(
        "SELECT unpriced_reason FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
        params![request, attempt],
        |r| r.get(0),
    )?;
    let scope: Scope = serde_json::from_str(&old.0)?;
    if !reason.as_deref().is_some_and(price_type_unpriced_reason)
        || scope.entitlement_id.is_some()
        || old.8 == "unavailable"
    {
        return Ok(false);
    }
    let (Some(fact), Some(mut meta)) = (
        evidence::read_fact(tx, request, attempt)?,
        read_attempt_meta(tx, request, attempt)?,
    ) else {
        return Ok(false);
    };
    let Ok(model) = snapshot.model(&old.1) else {
        return Ok(false);
    };
    let plan = meta.resource_plan.clone();
    let positive = |q: &BTreeMap<String, u64>| q.values().any(|n| *n > 0);
    let resources_priced = |q: &BTreeMap<String, u64>| {
        !positive(q)
            || (snapshot.schema_version == 2
                && crate::domain::usage::value_pricing::price_resources(&model.resources, q)
                    .is_ok())
    };
    if !plan
        .as_ref()
        .is_none_or(|p| resources_priced(&p.quantities))
        || !fact
            .resources
            .as_ref()
            .is_none_or(|r| resources_priced(&r.quantities))
        // An unknown/unsupported tier is priced at the highest tier during
        // settlement; only a model without any text tier stays unpriced.
        || (plan.as_ref().is_none_or(|p| p.text_tokens) && model.supported_tiers().next().is_none())
    {
        return Ok(false);
    }
    let pending = old.8 == "pending";
    let frozen = serde_json::to_string(model)?;
    meta.frozen_tariff_sha256 = Some(sha256_hex(frozen.as_bytes()));
    meta.exact_aliases = snapshot.exact_aliases.clone();
    meta.admission_revision
        .get_or_insert_with(|| old.11.clone());
    // Settled rows are re-priced from observed evidence alone: a zero
    // reserve keeps an outcome-unknown row from charging the estimate
    // ceiling. A pending row keeps its admission reserve, which is what its
    // `pending_risk_picos` (and thus admission and `pending_value_in`) holds.
    if !pending {
        meta.reserved_picos = "0".into();
    }
    tx.execute(
        "UPDATE meta SET value=?2 WHERE key=?1",
        params![
            attempt_meta_key(request, attempt)?,
            serde_json::to_string(&meta)?
        ],
    )?;
    let mark = next_watermark(tx)?;
    // Terminal rows are re-settled below at their own revision; clearing the
    // digest lets that settlement replace the $0 result exactly once.
    tx.execute(
        "UPDATE value_quota_attempts SET frozen_tariff_json=?3,pricing_revision=?4,unpriced_reason=NULL,
         watermark=?5,observation_sha256=CASE WHEN ?6 THEN observation_sha256 ELSE NULL END
         WHERE request_id=?1 AND attempt_id=?2",
        params![request, attempt, frozen, revision, mark, pending],
    )?;
    update_checkpoint(
        tx,
        &old.0,
        old.9,
        Some(Contribution {
            amount: &old.5,
            risk: &old.7,
            state: &old.8,
            revision: Some(&old.11),
            watermark: 0,
        }),
        Some(Contribution {
            amount: &old.5,
            risk: &old.7,
            state: &old.8,
            revision: Some(revision),
            watermark: u64::try_from(mark)?,
        }),
    )?;
    if pending {
        return Ok(true);
    }
    settle_in_tx(
        tx,
        &Observation {
            request_id: request,
            attempt_id: attempt,
            scope: &scope,
            usage_revision: u64::try_from(old.4)?,
            effective_model: &old.1,
            effective_tier: &old.2,
            reported_model: fact.reported_model.as_deref(),
            observed_tier: fact.observed_tier.as_deref(),
            tokens: fact.tokens,
            resources: fact.resources.clone(),
            // A terminal attempt whose fact never left in-progress ended
            // with an unknown outcome (sweeper / reconcile).
            outcome: match fact.outcome {
                ObservedOutcome::InProgress => ObservedOutcome::Unknown,
                outcome => outcome,
            },
            synthetic: old
                .6
                .as_deref()
                .is_some_and(|d| d.starts_with("synthetic:")),
        },
    )?;
    Ok(true)
}
