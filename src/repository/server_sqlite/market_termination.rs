//! Durable Market admissions/final usage. Never reads retained display logs.
use super::*;
use crate::domain::sharing::{
    market_termination_contract::*, router_contract::ShareManagedGrantOperation,
};

mod ack;
pub(crate) use ack::{delete_ack, enqueue_ack, pending_acks, touch_ack};

pub(crate) fn archive_since(dir: &Path) -> anyhow::Result<Option<i64>> {
    if !super::is_committed(dir)? {
        return Ok(None);
    }
    super::database_meta_value(dir, "market_settlement_since_ms")?
        .map(|v| v.parse().context("invalid Market archive epoch"))
        .transpose()
}

pub(super) fn archive_in(conn: &Connection, store: &ShareStore) -> anyhow::Result<()> {
    let mut insert = conn.prepare("INSERT INTO market_settlement_requests
        (request_id,share_id,entitlement_id,email,phase,started_at_ms,revision,tokens,value_picos,payload_json)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
        ON CONFLICT(request_id) DO UPDATE SET revision=excluded.revision,tokens=excluded.tokens,
        value_picos=excluded.value_picos,payload_json=excluded.payload_json,
        final_state=CASE WHEN excluded.revision>market_settlement_requests.revision THEN 'pending' ELSE market_settlement_requests.final_state END,
        no_value_execution=CASE WHEN excluded.revision>market_settlement_requests.revision THEN 0 ELSE market_settlement_requests.no_value_execution END
        WHERE market_settlement_requests.share_id=excluded.share_id
          AND market_settlement_requests.entitlement_id=excluded.entitlement_id
          AND market_settlement_requests.email=excluded.email
          AND market_settlement_requests.phase=excluded.phase
          AND market_settlement_requests.started_at_ms=excluded.started_at_ms
          AND excluded.revision>=market_settlement_requests.revision")?;
    for share in &store.shares {
        for (id, r) in &share.market_requests {
            let previous:Option<(String,String,String,String,i64,i64,String)>=conn.query_row("SELECT share_id,entitlement_id,email,phase,started_at_ms,revision,tokens FROM market_settlement_requests WHERE request_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
            if let Some((s, e, email, phase, start, revision, tokens)) = previous {
                ensure!(
                    s == share.id
                        && e == r.entitlement_id
                        && email == r.email
                        && phase == serde_json::to_string(&r.phase)?
                        && start == r.started_at_ms,
                    "Market archive request identity conflict"
                );
                ensure!(
                    revision != r.revision as i64 || tokens == r.tokens.to_string(),
                    "Market archive revision payload conflict"
                );
            }
            insert.execute(params![
                id,
                share.id,
                r.entitlement_id,
                r.email,
                serde_json::to_string(&r.phase)?,
                r.started_at_ms,
                r.revision,
                r.tokens.to_string(),
                r.value_picos,
                serde_json::to_string(r)?
            ])?;
        }
    }
    Ok(())
}

pub(super) fn finalize_log_in(conn: &Connection, log: &UsageLog) -> anyhow::Result<()> {
    use crate::domain::usage::store::UsageState;
    let state = match log.usage_state {
        UsageState::Pending => "pending",
        UsageState::Interrupted | UsageState::ParseError => "uncertain",
        UsageState::Missing if (200..300).contains(&log.status_code) => "uncertain",
        _ => "final",
    };
    let saved: Option<(String, String, i64, String)> = conn.query_row(
        "SELECT share_id,email,revision,tokens FROM market_settlement_requests WHERE request_id=?1",
        [&log.request_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)),
    ).optional()?;
    if let Some((share, email, revision, tokens)) = saved {
        ensure!(
            log.share_id.as_deref() == Some(share.as_str())
                && log
                    .user_email
                    .as_deref()
                    .is_some_and(|e| e.eq_ignore_ascii_case(&email)),
            "Market final usage identity conflict"
        );
        if revision == log.usage_revision.max(1) as i64 && state != "pending" {
            ensure!(
                tokens == log.quota_tokens().to_string(),
                "Market final usage quantity conflict"
            );
        }
    }
    conn.execute(
        "UPDATE market_settlement_requests SET final_state=?2,no_value_execution=?4
        WHERE request_id=?1 AND revision=?3",
        params![
            log.request_id,
            state,
            log.usage_revision.max(1),
            i64::from(log.status_code >= 400 && log.quota_tokens() == 0)
        ],
    )?;
    Ok(())
}

/// Called only AFTER the original managed entitlement was durably revoked.
/// A pending request is never silently converted into zero consumption.
pub(crate) fn receipt(
    dir: &Path,
    share_id: &str,
    op: &ShareManagedGrantOperation,
) -> anyhow::Result<TerminationReceipt> {
    super::assert_committed(dir)?;
    let mut conn = super::open_database(&super::database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result = receipt_in(&tx, share_id, op, crate::infra::time::now_ms() as i64)?;
    tx.commit()?;
    Ok(result)
}

fn receipt_in(
    tx: &Transaction<'_>,
    share_id: &str,
    op: &ShareManagedGrantOperation,
    now: i64,
) -> anyhow::Result<TerminationReceipt> {
    let fence = op
        .termination
        .as_ref()
        .context("missing Market termination fence")?;
    ensure!(
        now >= fence.cutoff_at_ms,
        "MARKET_TERMINATION_PENDING: waiting for the cutoff clock"
    );
    ensure!(
        op.action == crate::domain::sharing::router_contract::ShareManagedGrantAction::Revoke,
        "Market receipt requires Revoke"
    );
    if let Some(saved) = tx
        .query_row(
            "SELECT payload_json FROM market_settlement_receipts WHERE operation_id=?1",
            [&op.operation_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        let receipt: TerminationReceipt = serde_json::from_str(&saved)?;
        ensure!(
            receipt.termination_id == fence.termination_id
                && receipt.entitlement_id == op.entitlement_id
                && receipt.share_id == share_id
                && receipt.email.eq_ignore_ascii_case(&op.email)
                && receipt.cutoff_at_ms == fence.cutoff_at_ms
                && receipt.period_start_ms == fence.period_start_ms
                && receipt.period_end_ms == fence.period_end_ms,
            "Market receipt identity conflict"
        );
        return Ok(receipt);
    }
    let since: i64 = tx.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key='market_settlement_since_ms'",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        fence.period_start_ms >= since
            && fence.period_start_ms <= fence.cutoff_at_ms
            && fence.cutoff_at_ms <= fence.period_end_ms,
        "Market settlement archive does not cover period"
    );
    let payload: String = tx
        .query_row(
            "SELECT payload_json FROM shares WHERE share_id=?1",
            [share_id],
            |r| r.get(0),
        )
        .context("MARKET_TERMINATION_PENDING: original Share unavailable")?;
    let share: crate::domain::sharing::shares::Share = serde_json::from_str(&payload)?;
    ensure!(
        !share
            .market_entitlements
            .get(&op.entitlement_id)
            .is_some_and(|e| e.active)
            && !share
                .user_grants
                .values()
                .any(|g| g.active && g.entitlement_id.as_deref() == Some(&op.entitlement_id)),
        "MARKET_TERMINATION_PENDING: original entitlement remains active"
    );
    // After an upgrade/crash the admission may have been imported before its
    // already-committed final Usage row. Recover only from that exact durable
    // revision, never from `revision > 0` or a missing/bounded display log.
    let logs = {
        let mut q = tx.prepare(
            "SELECT u.payload_json FROM market_settlement_requests r
            JOIN usage_records u ON u.request_id=r.request_id
            WHERE r.share_id=?1 AND r.entitlement_id=?2 AND r.final_state!='final'
              AND u.usage_revision=r.revision LIMIT 256",
        )?;
        let rows = q
            .query_map(params![share_id, op.entitlement_id], |r| {
                r.get::<_, String>(0)
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for raw in logs {
        finalize_log_in(tx, &serde_json::from_str::<UsageLog>(&raw)?)?;
    }
    let rows = {
        let mut q=tx.prepare("SELECT request_id,email,started_at_ms,tokens,final_state,phase,no_value_execution FROM market_settlement_requests
            WHERE share_id=?1 AND entitlement_id=?2 ORDER BY started_at_ms,request_id")?;
        let rows = q
            .query_map(params![share_id, op.entitlement_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, bool>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut windows: BTreeMap<(i64, i64), i128> = BTreeMap::new();
    let mut before = 0i128;
    for (id, email, started, tokens, state, phase, no_execution) in &rows {
        ensure!(
            email.eq_ignore_ascii_case(&op.email),
            "Market usage identity mismatch"
        );
        ensure!(
            state == "final",
            "MARKET_TERMINATION_PENDING: final usage is pending or uncertain"
        );
        // Drain ALL admissions, including trial and a direct-Server request
        // racing the Router fence. Only pre-cutoff paid work earns rent.
        if phase != "\"paid\"" || *started >= fence.cutoff_at_ms {
            continue;
        }
        let amount = if fence.policy.value_quota.is_some() {
            // Query the execution-price ledger, not the current price catalog.
            let requests = [super::value_quota::SettledRequest {
                request_id: id,
                share_id,
                email,
                entitlement_id: Some(&op.entitlement_id),
            }];
            let totals = super::value_quota::settled_request_picos_in(tx, &requests)?;
            let pending: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM value_quota_attempts WHERE request_id=?1
                AND metering_status NOT IN ('observed','conservative'))",
                [id],
                |r| r.get(0),
            )?;
            ensure!(
                !pending,
                "MARKET_TERMINATION_PENDING: USD attempts remain unresolved"
            );
            match totals.get(id) {
                Some(v) => *v,
                None if *no_execution => 0,
                None => {
                    anyhow::bail!("MARKET_TERMINATION_PENDING: missing execution-price evidence")
                }
            }
        } else {
            tokens.parse::<i128>()?
        };
        ensure!(amount >= 0, "negative Market usage");
        // The value ledger retains latest attempt identities and settled
        // totals indefinitely; archive the amount too, never display estimates.
        if fence.policy.value_quota.is_some() {
            tx.execute(
                "UPDATE market_settlement_requests SET value_picos=?2 WHERE request_id=?1",
                params![id, amount.to_string()],
            )?;
        }
        if *started < fence.period_start_ms {
            before = before
                .checked_add(amount)
                .context("opening quota overflow")?;
            continue;
        }
        if *started >= fence.period_end_ms {
            continue;
        }
        let w = crate::domain::sharing::token_period::token_period_window(&fence.policy, *started)
            .map_err(anyhow::Error::msg)?;
        let key = (
            w.starts_at_ms.unwrap_or(fence.period_start_ms),
            w.ends_at_ms.unwrap_or(fence.period_end_ms),
        );
        let entry = windows.entry(key).or_default();
        *entry = entry.checked_add(amount).context("Market usage overflow")?;
    }
    let receipt = TerminationReceipt {
        termination_id: fence.termination_id.clone(),
        entitlement_id: op.entitlement_id.clone(),
        share_id: share_id.into(),
        email: op.email.clone(),
        operation_id: op.operation_id.clone(),
        cutoff_at_ms: fence.cutoff_at_ms,
        period_start_ms: fence.period_start_ms,
        period_end_ms: fence.period_end_ms,
        revoked_at_ms: now,
        archive_since_ms: since,
        quota_used_before_period: before.to_string(),
        windows: windows
            .into_iter()
            .map(|((starts_at_ms, ends_at_ms), used)| UsageWindow {
                starts_at_ms,
                ends_at_ms,
                used: used.to_string(),
            })
            .collect(),
    };
    tx.execute(
        "INSERT INTO market_settlement_receipts VALUES(?1,?2)",
        params![op.operation_id, serde_json::to_string(&receipt)?],
    )?;
    Ok(receipt)
}

#[cfg(test)]
mod tests;
