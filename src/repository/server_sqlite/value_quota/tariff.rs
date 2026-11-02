//! Local, transactionally activated execution tariffs. No network calls.
use super::*;
use crate::domain::usage::value_pricing::{diagnostic_identity, PriceRejection};

fn active_key(scope: &Scope) -> String {
    format!(
        "value_tariff:{}:{}",
        scope.router_url, scope.installation_id
    )
}

pub(super) fn admission_revision(
    conn: &Connection,
    input: &Admission<'_>,
    now: i64,
) -> anyhow::Result<String> {
    let active = activate_due_in_tx(conn, input.scope, now)?;
    let existing:Option<String>=conn.query_row("SELECT pricing_revision FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
        params![input.request_id,input.attempt_id],|r|r.get(0)).optional()?;
    existing
        .or(active)
        .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")
}

pub(super) fn read_pending(
    conn: &Connection,
    scope: &Scope,
) -> anyhow::Result<Option<PendingTariff>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [pending_tariff_key(scope)],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).context("VALUE_QUOTA_PENDING_TARIFF_CORRUPT"))
        .transpose()
}

pub(super) fn activate_due_in_tx(
    conn: &Connection,
    scope: &Scope,
    now: i64,
) -> anyhow::Result<Option<String>> {
    if let Some(pending) = read_pending(conn, scope)?.filter(|p| p.effective_from_ms <= now) {
        // Only schedule_tariff writes this key, after installation and Router ACK.
        cached_snapshot(
            conn,
            &scope.router_url,
            &scope.installation_id,
            &pending.revision,
        )?;
        conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![active_key(scope), pending.revision])?;
        conn.execute("DELETE FROM meta WHERE key=?1", [pending_tariff_key(scope)])?;
    }
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [active_key(scope)],
            |r| r.get(0),
        )
        .optional()?)
}

/// Ordinary reads do not acquire a write lock; only a due revision needs one.
pub(crate) fn activate_due_tariff(
    dir: &Path,
    scope: &Scope,
    now: i64,
) -> anyhow::Result<Option<String>> {
    assert_committed(dir)?;
    let conn = open_database(&database_path(dir), true)?;
    let due = read_pending(&conn, scope)?.is_some_and(|p| p.effective_from_ms <= now);
    if !due {
        return Ok(conn
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                [active_key(scope)],
                |r| r.get(0),
            )
            .optional()?);
    }
    drop(conn);
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let active = activate_due_in_tx(&tx, scope, now)?;
    tx.commit()?;
    Ok(active)
}

/// Replaces pending state and activates an already due manifest atomically.
/// Call only after the Router acknowledged this installed revision.
pub(crate) fn schedule_tariff(
    dir: &Path,
    scope: &Scope,
    pending: &PendingTariff,
    now: i64,
) -> anyhow::Result<()> {
    assert_committed(dir)?;
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    cached_snapshot(
        &tx,
        &scope.router_url,
        &scope.installation_id,
        &pending.revision,
    )?;
    tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![pending_tariff_key(scope), serde_json::to_string(pending)?])?;
    activate_due_in_tx(&tx, scope, now)?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TariffCoverage {
    pub ready: bool,
    pub reason_code: &'static str,
    pub model: String,
    pub active_pricing_revision: Option<String>,
    pub pending_pricing_revision: Option<String>,
    pub effective_from_ms: Option<i64>,
}

pub(crate) fn model_coverage(
    dir: &Path,
    scope: &Scope,
    model: &str,
    tier: &str,
    now: i64,
) -> anyhow::Result<TariffCoverage> {
    activate_due_tariff(dir, scope, now)?;
    let mut conn = open_database(&database_path(dir), true)?;
    let tx = conn.transaction()?;
    let active: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [active_key(scope)],
            |r| r.get(0),
        )
        .optional()?;
    let pending = read_pending(&tx, scope)?;
    let ready = match active.as_deref() {
        Some(revision) => {
            cached_snapshot(&tx, &scope.router_url, &scope.installation_id, revision)?
                .model(model)
                .is_ok_and(|price| price.supports_tier(tier))
        }
        None => false,
    };
    let pending_covers = match pending.as_ref() {
        Some(p) => cached_snapshot(&tx, &scope.router_url, &scope.installation_id, &p.revision)?
            .model(model)
            .is_ok_and(|price| price.supports_tier(tier)),
        None => false,
    };
    let result = TariffCoverage {
        ready,
        reason_code: if ready {
            "ready"
        } else if pending_covers {
            "tariff_not_effective"
        } else {
            "model_unpriced"
        },
        model: diagnostic_identity(model),
        active_pricing_revision: active,
        pending_pricing_revision: pending.as_ref().map(|p| p.revision.clone()),
        effective_from_ms: pending.map(|p| p.effective_from_ms),
    };
    tx.commit()?;
    Ok(result)
}

pub(super) fn price_rejection(
    conn: &Connection,
    scope: &Scope,
    revision: &str,
    model: &str,
    tier: &str,
    reason: &'static str,
) -> PriceRejection {
    let pending = read_pending(conn, scope).ok().flatten();
    let pending_covers = pending.as_ref().is_some_and(|p| {
        cached_snapshot(conn, &scope.router_url, &scope.installation_id, &p.revision)
            .ok()
            .is_some_and(|s| s.model(model).is_ok_and(|price| price.supports_tier(tier)))
    });
    PriceRejection {
        reason_code: if pending_covers {
            "tariff_not_effective"
        } else {
            reason
        },
        model: diagnostic_identity(model),
        tier: diagnostic_identity(tier),
        active_pricing_revision: Some(revision.into()),
        pending_pricing_revision: pending.as_ref().map(|p| p.revision.clone()),
        effective_from_ms: pending.map(|p| p.effective_from_ms),
    }
}
