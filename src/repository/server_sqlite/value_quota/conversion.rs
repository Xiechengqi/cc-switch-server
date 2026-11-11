//! The conversion's ledger changes and Share policy commit in one transaction.
//! Consumption watermarks are deliberately absent from the preview CAS: live
//! usage is incorporated here; only structural edits invalidate a credential.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CoverageProof {
    pub from_ms: i64,
    pub until_ms: i64,
    pub tokens: u64,
    pub requests: u64,
    pub minimum_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Preview {
    pub status: QuotaStatus,
    pub complete: bool,
    pub missing_facts: u64,
    pub evidence_tokens: u64,
    pub evidence_requests: u64,
    pub pricing_basis: Option<String>,
    pub preparation_state: Option<String>,
    pub processed: u64,
    pub source_adjustment: Option<String>,
    pub non_token_basis: Option<String>,
    pub pending_tokens: u64,
    pub pending_complete: bool,
    pub token_history_complete: bool,
}

pub(crate) fn proof_start(dir: &Path, scope: &Scope, w: &Window) -> anyhow::Result<i64> {
    if w.account.is_some() {
        return Ok(w.metered_start_ms());
    }
    let conn = open_database(&database_path(dir), true)?;
    let through:Option<(i64,i64)>=conn.query_row("SELECT value_history_through_ms,metered_from_ms FROM quota_shadow_intervals WHERE scope=?1",
        [scope.key()?],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    Ok(through
        .filter(|(_, origin)| *origin <= w.metered_start_ms())
        .map_or(w.metered_start_ms(), |(at, _)| at.max(w.metered_start_ms())))
}

pub(super) fn latest_adjustment(
    conn: &Connection,
    scope: &Scope,
    w: &Window,
) -> anyhow::Result<Option<String>> {
    Ok(conn.query_row("SELECT operation_id FROM value_quota_adjustments WHERE scope=?1 AND window_id=?2 ORDER BY watermark DESC LIMIT 1",
        params![scope.key()?,w.id],|r|r.get(0)).optional()?)
}

pub(crate) fn preview(
    dir: &Path,
    scope: &Scope,
    w: &Window,
    proof: &CoverageProof,
) -> anyhow::Result<Preview> {
    assert_committed(dir)?;
    let mut conn = open_database(&database_path(dir), true)?;
    let tx = conn.transaction()?;
    let mut result = preview_in_tx(&tx, scope, w, proof)?;
    if restore_fence_blocks(dir, scope, w)? {
        result.status.metering_status = "blocked".into();
        result.complete = false;
    }
    tx.commit()?;
    Ok(result)
}

pub(super) fn preview_in_tx(
    tx: &Connection,
    scope: &Scope,
    w: &Window,
    proof: &CoverageProof,
) -> anyhow::Result<Preview> {
    let mut status = status_with_coverage_in(tx, scope, w, false)?;
    let (tokens, requests) =
        evidence::totals_in(tx, scope, w, proof.from_ms, Some(proof.until_ms))?;
    let missing =
        evidence::missing_in(tx, scope, w)?.saturating_add(evidence::unresolved_in(tx, scope, w)?);
    let preparation:Option<(String,String,i64)>=tx.query_row("SELECT state,pricing_revision,processed FROM quota_history_preparations WHERE scope=?1",
        [scope.key()?],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let source_adjustment = latest_adjustment(tx, scope, w)?;
    // A final log for retry N alone cannot prove what earlier attempts cost.
    let holes:i64=tx.query_row("SELECT count(*) FROM quota_metering_facts f WHERE f.scope=?1 AND f.started_at_ms>=?2
        AND (?3 IS NULL OR f.started_at_ms<?3) AND CAST(f.attempt_id AS INTEGER)>
        (SELECT count(*) FROM (SELECT a.attempt_id FROM value_quota_attempts a WHERE a.request_id=f.request_id
        UNION SELECT b.attempt_id FROM quota_metering_facts b WHERE b.request_id=f.request_id))",
        params![scope.key()?,w.metered_start_ms(),w.ends_at_ms],|r|r.get(0))?;
    let (pending, pending_value_complete) = evidence::pending_value_in(tx, scope, w)?;
    let (pending_tokens, pending_token_complete) = evidence::pending_tokens_in(tx, scope, w, None)?;
    status.pending_risk_picos = pending.to_string();
    let non_token_basis = evidence::non_token_basis(tx, scope, w)?;
    let complete = missing == 0
        && holes == 0
        && pending_value_complete
        && tokens == proof.tokens
        && requests == proof.requests
        && proof.tokens >= proof.minimum_tokens
        && !matches!(
            status.metering_status.as_str(),
            "unavailable" | "gap" | "blocked"
        );
    Ok(Preview {
        status,
        complete,
        missing_facts: missing.saturating_add(u64::try_from(holes)?),
        evidence_tokens: tokens,
        evidence_requests: requests,
        pricing_basis: preparation.as_ref().map(|p| p.1.clone()),
        preparation_state: preparation.as_ref().map(|p| p.0.clone()),
        non_token_basis,
        pending_tokens,
        pending_complete: pending_value_complete && pending_token_complete,
        // Token conversion does not need a currently priceable catalog. It
        // still cannot accept a lost rollup that contradicts retained facts
        // or the grant's independent accumulated Token counters.
        token_history_complete: proof.tokens >= proof.minimum_tokens
            && tokens <= proof.tokens
            && requests <= proof.requests,
        processed: preparation.map_or(0, |p| p.2.max(0) as u64),
        source_adjustment,
    })
}

pub(crate) fn mapped_offset(
    dir: &Path,
    scope: &Scope,
    w: &Window,
    source: &str,
    unit: &str,
) -> anyhow::Result<Option<String>> {
    let conn = open_database(&database_path(dir), true)?;
    Ok(conn.query_row("SELECT target_offset FROM quota_adjustment_mappings WHERE scope=?1 AND window_id=?2 AND source_adjustment=?3 AND target_unit=?4",
        params![scope.key()?,w.id,source,unit],|r|r.get(0)).optional()?)
}

#[derive(Debug, Clone)]
pub(crate) struct Conversion {
    pub scope: Scope,
    pub window: Window,
    pub proof: CoverageProof,
    pub require_history: bool,
    pub non_token_basis: Option<String>,
    pub operation_id: String,
    pub payload_sha256: String,
    pub source_unit: String,
    pub target_unit: String,
    pub target_limit: Option<String>,
    pub target_offset: Option<String>,
    pub source_adjustment: Option<String>,
    pub target_adjustment: Option<String>,
    pub source_offset: String,
    pub actor: String,
    pub reason: String,
    pub now: i64,
}

/// Every promoted attempt retains the old request/window attribution and its
/// frozen model price. No network request is held behind this short gate.
fn reserve_live(tx: &Transaction<'_>, c: &Conversion) -> anyhow::Result<()> {
    let rows = {
        let mut q=tx.prepare("SELECT request_id,attempt_id,started_at_ms,usage_picos,pending_risk_picos,pricing_revision,watermark FROM value_quota_attempts
            WHERE scope=?1 AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3) AND metering_status='pending' AND (?4 IS NULL OR EXISTS(SELECT 1 FROM quota_metering_facts f WHERE f.request_id=value_quota_attempts.request_id AND f.attempt_id=value_quota_attempts.attempt_id AND f.account_key=?4))")?;
        let mapped = q.query_map(
            params![
                c.scope.key()?,
                c.window.metered_start_ms(),
                c.window.ends_at_ms,
                c.window.account_key()?
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ))
            },
        )?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (request, attempt, at, amount, old_risk, revision, old_mark) in rows {
        let meta = read_attempt_meta(tx, &request, &attempt)?
            .context("QUOTA_CONVERSION_PENDING_EVIDENCE_MISSING")?;
        let risk = meta
            .reserved()
            .checked_sub(unsigned_decimal(&amount)?)
            .context("value quota arithmetic overflow")?
            .max(0);
        if risk == unsigned_decimal(&old_risk)? {
            continue;
        }
        let mark = next_watermark(tx)?;
        tx.execute("UPDATE value_quota_attempts SET pending_risk_picos=?3,watermark=?4 WHERE request_id=?1 AND attempt_id=?2",
            params![request,attempt,risk.to_string(),mark])?;
        update_checkpoint(
            tx,
            &c.scope.key()?,
            at,
            Some(Contribution {
                amount: &amount,
                risk: &old_risk,
                state: "pending",
                revision: Some(&revision),
                watermark: u64::try_from(old_mark)?,
            }),
            Some(Contribution {
                amount: &amount,
                risk: &risk.to_string(),
                state: "pending",
                revision: Some(&revision),
                watermark: u64::try_from(mark)?,
            }),
        )?;
    }
    Ok(())
}

pub(crate) fn ensure_unfenced(dir: &Path, conversions: &[Conversion]) -> anyhow::Result<()> {
    for c in conversions {
        ensure!(
            !restore_fence_blocks(dir, &c.scope, &c.window)?,
            "QUOTA_CONVERSION_RESTORE_FENCED"
        );
    }
    Ok(())
}

pub(crate) fn apply_in_tx(tx: &Transaction<'_>, conversions: &[Conversion]) -> anyhow::Result<()> {
    for c in conversions {
        let key = c.scope.key()?;
        let replay:Option<String>=tx.query_row("SELECT payload_sha256 FROM quota_conversion_receipts WHERE operation_id=?1 AND scope=?2",
            params![c.operation_id,key],|r|r.get(0)).optional()?;
        if let Some(old) = replay {
            ensure!(
                old == c.payload_sha256,
                "QUOTA_CONVERSION_OPERATION_CONFLICT"
            );
            continue;
        }
        if c.require_history {
            ensure!(
                preview_in_tx(tx, &c.scope, &c.window, &c.proof)?.complete,
                "QUOTA_CONVERSION_HISTORY_INCOMPLETE"
            );
            tx.execute("INSERT INTO value_quota_coverage VALUES(?1,?2) ON CONFLICT(scope) DO UPDATE SET collected_from_ms=min(collected_from_ms,excluded.collected_from_ms)",
                params![key,c.window.metered_start_ms()])?;
        }
        if c.target_unit == "usdEquivalent" {
            ensure!(
                !window_blocked(tx, &key, &c.window.id)?,
                "QUOTA_CONVERSION_RESTORE_FENCED"
            );
            ensure!(
                evidence::pending_value_in(tx, &c.scope, &c.window)?.1,
                "QUOTA_CONVERSION_PENDING_EVIDENCE_MISSING"
            );
            reserve_live(tx, c)?;
        } else if c.source_unit == "usdEquivalent" {
            let preview = preview_in_tx(tx, &c.scope, &c.window, &c.proof)?;
            ensure!(
                preview.non_token_basis == c.non_token_basis,
                "QUOTA_CONVERSION_PREVIEW_STALE"
            );
            ensure!(
                preview.token_history_complete,
                "QUOTA_CONVERSION_HISTORY_INCOMPLETE"
            );
            ensure!(
                preview.pending_complete,
                "QUOTA_CONVERSION_PENDING_EVIDENCE_MISSING"
            );
            tx.execute("INSERT INTO quota_shadow_intervals VALUES(?1,?2,?3) ON CONFLICT(scope) DO UPDATE SET value_history_through_ms=excluded.value_history_through_ms,metered_from_ms=min(metered_from_ms,excluded.metered_from_ms)",
                params![key,c.now,c.window.metered_from_ms])?;
        }
        if let Some(offset) = &c.target_offset {
            ensure!(
                !c.reason.trim().is_empty(),
                "QUOTA_CONVERSION_ADJUSTMENT_REASON_REQUIRED"
            );
            let adjustment_id = format!(
                "conversion:{}:{}",
                c.operation_id,
                sha256_hex(key.as_bytes())
            );
            if c.target_unit == "usdEquivalent" {
                let before = status_in_tx(tx, &c.scope, &c.window)?;
                let mark = next_watermark(tx)?;
                let payload = serde_json::json!({"action":"set","offsetPicos":offset,"conversionOperationId":c.operation_id});
                tx.execute("INSERT INTO value_quota_adjustments(operation_id,scope,window_id,payload_json,target_picos,observed_watermark,actor,reason,created_at_ms,watermark)
                    VALUES(?1,?2,?3,?4,NULL,?5,?6,?7,?8,?9)",params![adjustment_id,key,c.window.id,payload.to_string(),to_sql_u64(before.usage_watermark)?,c.actor,c.reason,c.now,mark])?;
            }
            if let Some(source) = &c.source_adjustment {
                tx.execute("INSERT INTO quota_adjustment_mappings VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(scope,window_id,source_adjustment,target_unit) DO NOTHING",
                    params![key,c.window.id,source,c.target_unit,offset,c.reason,c.actor,c.operation_id])?;
                // Reciprocal mapping makes a round trip apply the same offset
                // once; consumption accrued in between remains independent.
                if c.target_unit == "usdEquivalent" || c.target_adjustment.is_some() {
                    let reciprocal = c.target_adjustment.as_ref().unwrap_or(&adjustment_id);
                    tx.execute("INSERT INTO quota_adjustment_mappings VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT(scope,window_id,source_adjustment,target_unit) DO NOTHING",
                        params![key,c.window.id,reciprocal,c.source_unit,c.source_offset,c.reason,c.actor,c.operation_id])?;
                }
            }
        }
        let mark = next_watermark(tx)?;
        tx.execute("INSERT INTO quota_conversion_receipts(operation_id,scope,window_id,watermark,payload_sha256,receipt_json,created_at_ms) VALUES(?1,?2,?3,?4,?5,'{}',?6)",
            params![c.operation_id,key,c.window.id,mark,c.payload_sha256,c.now])?;
        let status = if c.target_unit == "usdEquivalent" {
            Some(status_in_tx(tx, &c.scope, &c.window)?)
        } else {
            None
        };
        let receipt = serde_json::json!({"operationId":c.operation_id,"email":c.scope.email,"sourceUnit":c.source_unit,"targetUnit":c.target_unit,
            "windowId":c.window.id,"meteredFromMs":c.window.metered_from_ms,"effectiveFromMs":c.now,"targetLimit":c.target_limit,
            "status":status,"committed":true});
        tx.execute("UPDATE quota_conversion_receipts SET receipt_json=?3 WHERE operation_id=?1 AND scope=?2",params![c.operation_id,key,receipt.to_string()])?;
    }
    Ok(())
}

pub(crate) fn receipts(
    dir: &Path,
    operation: &str,
    share_id: &str,
    config: &crate::domain::settings::config::ServerConfig,
) -> anyhow::Result<Vec<Value>> {
    if !is_committed(dir)? {
        return Ok(Vec::new());
    }
    let Some(identity) = config.registered_router_identity() else {
        return Ok(Vec::new());
    };
    let conn = open_database(&database_path(dir), true)?;
    let mut q=conn.prepare("SELECT receipt_json FROM quota_conversion_receipts WHERE operation_id=?1 AND json_extract(scope,'$.shareId')=?2 AND json_extract(scope,'$.routerUrl')=?3 AND json_extract(scope,'$.installationId')=?4 ORDER BY scope")?;
    let rows = q.query_map(
        params![
            operation,
            share_id,
            config.router_api_base().unwrap_or(""),
            identity.installation_id
        ],
        |r| r.get::<_, String>(0),
    )?;
    rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
}

pub(crate) fn finalize_in_tx(
    tx: &Transaction<'_>,
    conversions: &[Conversion],
    shares: &ShareStore,
) -> anyhow::Result<()> {
    for c in conversions {
        let settings = shares
            .applied_settings_operations
            .get(&c.operation_id)
            .context("QUOTA_CONVERSION_RECEIPT_MISSING")?;
        let finalized = tx.execute("UPDATE quota_conversion_receipts SET settings_receipt_json=?3 WHERE operation_id=?1 AND scope=?2 AND settings_receipt_json IS NULL",params![c.operation_id,c.scope.key()?,serde_json::to_string(settings)?])?;
        if finalized == 0 {
            continue;
        }
        if c.target_unit != "usdEquivalent" {
            continue;
        }
        let key = c.scope.key()?;
        let raw: String = tx.query_row(
            "SELECT receipt_json FROM quota_conversion_receipts WHERE operation_id=?1 AND scope=?2",
            params![c.operation_id, key],
            |r| r.get(0),
        )?;
        let mut receipt: Value = serde_json::from_str(&raw)?;
        receipt["status"] = serde_json::to_value(status_in_tx(tx, &c.scope, &c.window)?)?;
        tx.execute("UPDATE quota_conversion_receipts SET receipt_json=?3 WHERE operation_id=?1 AND scope=?2",params![c.operation_id,key,receipt.to_string()])?;
    }
    Ok(())
}

pub(crate) fn settings_receipt(
    dir: &Path,
    operation: &str,
    share: &str,
    config: &crate::domain::settings::config::ServerConfig,
) -> anyhow::Result<Option<crate::domain::sharing::router_contract::ShareSettingsReceipt>> {
    if !is_committed(dir)? {
        return Ok(None);
    }
    let Some(identity) = config.registered_router_identity() else {
        return Ok(None);
    };
    let conn = open_database(&database_path(dir), true)?;
    let row:Option<String>=conn.query_row("SELECT settings_receipt_json FROM quota_conversion_receipts WHERE operation_id=?1 AND json_extract(scope,'$.shareId')=?2 AND json_extract(scope,'$.routerUrl')=?3 AND json_extract(scope,'$.installationId')=?4 AND settings_receipt_json IS NOT NULL LIMIT 1",params![operation,share,config.router_api_base().unwrap_or(""),identity.installation_id],|r|r.get(0)).optional()?;
    row.map(|j| serde_json::from_str(&j).map_err(Into::into))
        .transpose()
}
