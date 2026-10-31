//! Durable Share USD-equivalent metering, independent of report retention.
//! Callers enter through ServerStateInner. Every admission is an IMMEDIATE
//! transaction; an upstream request must never start before this commit.
//!
//! This is a request-level SOFT cap: each attempt reserves a bounded risk
//! (input estimate + declared output ceiling), and every admitted attempt is
//! settled by an outcome class (exact, free, interrupted, unknown). Failures
//! are scoped to one (scope, window) and cleared by an owner reconcile.
use super::*;
use crate::domain::usage::value_pricing::{
    canonical_service_tier, unsigned_decimal, ExecutionModelPrice, PricingSnapshot, RiskEstimate,
    ValueError, ValueQuota, ValueQuotaRejection, ValueQuotaRejectionKind, ValueTokens,
    MAX_OUTPUT_CEILING,
};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    pub router_url: String,
    pub installation_id: String,
    pub share_id: String,
    pub email: String,
}
impl Scope {
    pub fn key(&self) -> anyhow::Result<String> {
        ensure!(
            !self.router_url.is_empty()
                && !self.installation_id.is_empty()
                && !self.share_id.is_empty(),
            "invalid value quota scope"
        );
        ensure!(
            !self.email.is_empty()
                && self.email.len() <= 320
                && self.email == self.email.trim().to_ascii_lowercase(),
            "invalid value quota identity"
        );
        Ok(serde_json::to_string(self)?)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Window {
    pub id: String,
    pub starts_at_ms: i64,
    pub ends_at_ms: Option<i64>,
    pub metered_from_ms: i64,
}
impl Window {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.id.is_empty() && self.starts_at_ms >= 0 && self.metered_from_ms >= 0,
            "invalid value quota window"
        );
        ensure!(
            self.ends_at_ms.is_none_or(|end| end > self.starts_at_ms),
            "invalid value quota window end"
        );
        Ok(())
    }
    fn metered_start_ms(&self) -> i64 {
        self.starts_at_ms.max(self.metered_from_ms)
    }
}
#[derive(Debug, Clone)]
pub(crate) struct Admission<'a> {
    pub enforce: bool,
    pub scope: &'a Scope,
    pub window: &'a Window,
    pub policy_epoch: &'a str,
    pub quota: &'a ValueQuota,
    pub pricing_revision: &'a str,
    pub request_id: &'a str,
    pub attempt_id: &'a str,
    pub effective_model: &'a str,
    pub effective_tier: &'a str,
    pub started_at_ms: i64,
    /// Bounded reservation input; the risk is priced from the frozen tariff.
    pub estimate: RiskEstimate,
    /// min(grant parallel limit, 4); counted over this scope's pending attempts.
    pub parallel_limit: u32,
}
/// Result of a successful admission. `reserved_picos` is the bounded risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Admitted {
    pub reserved_picos: i128,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct QuotaStatus {
    pub observed_picos: String,
    pub manual_offset_picos: String,
    pub used_picos: String,
    pub pending_risk_picos: String,
    pub metering_status: String,
    pub usage_watermark: u64,
    pub usage_pricing_revisions_digest: String,
}
const DAY_MS: i64 = 86_400_000;
const RESTORE_FENCE: &str = "share-value-quota-restore-fence.json";
/// Pre-v2 global USD-only failure marker. It is no longer written or
/// enforced; an owner reconcile removes it.
const LEGACY_DEGRADED_META: &str = "value_quota_degraded_v1";
/// Block reason written when the Router acknowledged a higher watermark.
const ROUTER_AHEAD_REASON: &str = "router_watermark_ahead_of_authority";
/// An unchanged projection is re-sent at most this often; it stays below the
/// 60 +/- 6 s heartbeat so idle users sync on every heartbeat (the Router
/// treats projections older than 180 s as stale).
const PROJECTION_KEEPALIVE_MS: i64 = 45_000;
fn block_key(scope_key: &str, window_id: &str) -> String {
    format!("value_quota_blocked:{scope_key}:{window_id}")
}
fn legacy_scope_block_key(scope_key: &str) -> String {
    format!("value_quota_blocked:{scope_key}")
}
fn attempt_meta_key(request_id: &str, attempt_id: &str) -> anyhow::Result<String> {
    Ok(format!(
        "value_attempt:{}",
        serde_json::to_string(&(request_id, attempt_id))?
    ))
}
/// Admission-time reservation facts, kept in `meta` (no schema change) so
/// settlement can bound interrupted/unknown outcomes by the same ceiling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttemptMeta {
    input_tokens: u64,
    output_tokens: u64,
    reserved_picos: String,
}
impl AttemptMeta {
    fn reserved(&self) -> i128 {
        unsigned_decimal(&self.reserved_picos).unwrap_or(0)
    }
}
fn read_attempt_meta(
    conn: &Connection,
    request_id: &str,
    attempt_id: &str,
) -> anyhow::Result<Option<AttemptMeta>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [attempt_meta_key(request_id, attempt_id)?],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
}

/// SQLite busy/locked is a transient contention signal, not a quota fact.
pub(crate) fn is_storage_busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<rusqlite::Error>(),
            Some(rusqlite::Error::SqliteFailure(failure, _))
                if matches!(
                    failure.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        )
    })
}

type SnapshotKey = (String, String, String);
/// Parsed+validated snapshots keyed by (router, installation, revision).
/// The revision is the content hash and install refuses conflicting payloads,
/// so a cached entry can never disagree with a stored row of the same key.
fn snapshot_cache() -> &'static Mutex<std::collections::HashMap<SnapshotKey, Arc<PricingSnapshot>>>
{
    static CACHE: OnceLock<Mutex<std::collections::HashMap<SnapshotKey, Arc<PricingSnapshot>>>> =
        OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn remember_snapshot(key: SnapshotKey, snapshot: Arc<PricingSnapshot>) {
    let mut cache = snapshot_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.len() >= 32 {
        cache.clear();
    }
    cache.insert(key, snapshot);
}
fn cached_snapshot(
    conn: &Connection,
    router_url: &str,
    installation_id: &str,
    revision: &str,
) -> anyhow::Result<Arc<PricingSnapshot>> {
    let key = (
        router_url.to_owned(),
        installation_id.to_owned(),
        revision.to_owned(),
    );
    let hit = snapshot_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
        .cloned();
    if let Some(hit) = hit {
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3)",
            params![router_url, installation_id, revision],
            |r| r.get(0),
        )?;
        ensure!(exists, "VALUE_QUOTA_SNAPSHOT_UNAVAILABLE");
        return Ok(hit);
    }
    let json: String = conn
        .query_row(
            "SELECT payload_json FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3",
            params![router_url, installation_id, revision],
            |r| r.get(0),
        )
        .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
    let snapshot: PricingSnapshot = serde_json::from_str(&json)?;
    ensure!(
        snapshot.revision()? == revision,
        "VALUE_QUOTA_SNAPSHOT_CORRUPT"
    );
    let snapshot = Arc::new(snapshot);
    remember_snapshot(key, snapshot.clone());
    Ok(snapshot)
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreFence {
    schema_version: u16,
    scopes: BTreeMap<String, i64>,
}
fn read_restore_fence(dir: &Path) -> anyhow::Result<RestoreFence> {
    let path = dir.join(RESTORE_FENCE);
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RestoreFence {
                schema_version: 1,
                ..Default::default()
            })
        }
        Err(e) => return Err(e.into()),
    };
    let fence: RestoreFence = serde_json::from_slice(&bytes)?;
    ensure!(
        fence.schema_version == 1 && fence.scopes.values().all(|at| *at >= 0),
        "VALUE_QUOTA_INVALID_RESTORE_FENCE"
    );
    Ok(fence)
}
pub(crate) fn write_restore_fence(dir: &Path) -> anyhow::Result<()> {
    let mut fence = read_restore_fence(dir)?;
    if !is_committed(dir)? {
        return Ok(());
    }
    let conn = open_database(&database_path(dir), true)?;
    let mut query = conn.prepare("SELECT scope FROM value_quota_coverage")?;
    for key in query.query_map([], |r| r.get::<_, String>(0))? {
        let value = fence.scopes.entry(key?).or_default();
        *value = (*value).max(now_ms());
    }
    crate::infra::storage::write_json_pretty(&dir.join(RESTORE_FENCE), &fence)
}
/// The fence stays authoritative until an owner reconcile (`set`) for the
/// scope clears it; the reconcile is the explicit "history verified" act.
fn clear_restore_fence(dir: &Path, scope_key: &str) -> anyhow::Result<bool> {
    let mut fence = read_restore_fence(dir)?;
    if fence.scopes.remove(scope_key).is_none() {
        return Ok(false);
    }
    crate::infra::storage::write_json_pretty(&dir.join(RESTORE_FENCE), &fence)?;
    Ok(true)
}
fn restore_fence_blocks(dir: &Path, scope: &Scope, window: &Window) -> anyhow::Result<bool> {
    Ok(read_restore_fence(dir)?
        .scopes
        .get(&scope.key()?)
        .is_some_and(|at| window.metered_start_ms() < *at))
}
#[cfg(test)]
fn ensure_restore_coverage(dir: &Path, scope: &Scope, window: &Window) -> anyhow::Result<()> {
    ensure!(
        !restore_fence_blocks(dir, scope, window)?,
        "VALUE_QUOTA_RESTORE_RECONCILIATION_REQUIRED"
    );
    Ok(())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    amount: String,
    risk: String,
    states: BTreeMap<String, u64>,
    revisions: BTreeMap<String, u64>,
    watermark: u64,
}
impl Checkpoint {
    fn empty() -> Self {
        Self {
            amount: "0".into(),
            risk: "0".into(),
            ..Default::default()
        }
    }
    fn count(&self) -> anyhow::Result<u64> {
        self.states.values().try_fold(0u64, |sum, value| {
            sum.checked_add(*value)
                .context("VALUE_QUOTA_CHECKPOINT_CORRUPT")
        })
    }
    fn change(
        &mut self,
        amount: &str,
        risk: &str,
        state: &str,
        revision: Option<&str>,
        mark: u64,
        add: bool,
    ) -> anyhow::Result<()> {
        fn number(old: &str, change: &str, add: bool) -> anyhow::Result<String> {
            let (old, change) = (unsigned_decimal(old)?, unsigned_decimal(change)?);
            let result = if add {
                old.checked_add(change)
            } else {
                old.checked_sub(change)
            }
            .context("value checkpoint overflow")?;
            ensure!(result >= 0, "VALUE_QUOTA_CHECKPOINT_CORRUPT");
            Ok(result.to_string())
        }
        fn counter(map: &mut BTreeMap<String, u64>, key: &str, add: bool) -> anyhow::Result<()> {
            let old = map.get(key).copied().unwrap_or(0);
            let next = if add {
                old.checked_add(1)
            } else {
                old.checked_sub(1)
            }
            .context("VALUE_QUOTA_CHECKPOINT_CORRUPT")?;
            if next == 0 {
                map.remove(key);
            } else {
                map.insert(key.into(), next);
            }
            Ok(())
        }
        self.amount = number(&self.amount, amount, add)?;
        self.risk = number(&self.risk, risk, add)?;
        counter(&mut self.states, state, add)?;
        if let Some(revision) = revision {
            counter(&mut self.revisions, revision, add)?;
        }
        self.watermark = self.watermark.max(mark);
        Ok(())
    }
    fn merge(&mut self, other: &Self) -> anyhow::Result<()> {
        self.amount = unsigned_decimal(&self.amount)?
            .checked_add(unsigned_decimal(&other.amount)?)
            .context("value checkpoint overflow")?
            .to_string();
        self.risk = unsigned_decimal(&self.risk)?
            .checked_add(unsigned_decimal(&other.risk)?)
            .context("value checkpoint overflow")?
            .to_string();
        for (target, source) in [
            (&mut self.states, &other.states),
            (&mut self.revisions, &other.revisions),
        ] {
            for (key, value) in source {
                let old = target.get(key).copied().unwrap_or(0u64);
                target.insert(
                    key.clone(),
                    old.checked_add(*value)
                        .context("value checkpoint count overflow")?,
                );
            }
        }
        self.watermark = self.watermark.max(other.watermark);
        Ok(())
    }
    /// Window status is observed, conservative or pending. Legacy
    /// `unavailable` attempts and coverage gaps are reported as conservative:
    /// they never lock a window, an owner reconcile resolves them.
    fn metering(&self) -> &'static str {
        if self.states.contains_key("pending") {
            "pending"
        } else if ["conservative", "unavailable", "gap"]
            .iter()
            .any(|state| self.states.contains_key(*state))
        {
            "conservative"
        } else {
            "observed"
        }
    }
}
fn read_checkpoint(conn: &Connection, key: &str, day: i64) -> anyhow::Result<Checkpoint> {
    let row:Option<(String,String)>=conn.query_row("SELECT payload_json,payload_sha256 FROM value_quota_checkpoints WHERE scope=?1 AND day_start_ms=?2",params![key,day],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    match row {
        None => Ok(Checkpoint::empty()),
        Some((json, hash)) => {
            ensure!(
                sha256_hex(json.as_bytes()) == hash,
                "VALUE_QUOTA_CHECKPOINT_CORRUPT"
            );
            Ok(serde_json::from_str(&json)?)
        }
    }
}
fn write_checkpoint(
    tx: &Transaction<'_>,
    scope: &str,
    day: i64,
    checkpoint: &Checkpoint,
) -> anyhow::Result<()> {
    let json = serde_json::to_string(checkpoint)?;
    tx.execute("INSERT INTO value_quota_checkpoints VALUES(?1,?2,?3,?4) ON CONFLICT(scope,day_start_ms) DO UPDATE SET payload_json=excluded.payload_json,payload_sha256=excluded.payload_sha256",
        params![scope,day,json,sha256_hex(json.as_bytes())])?;
    Ok(())
}
struct Contribution<'a> {
    amount: &'a str,
    risk: &'a str,
    state: &'a str,
    revision: Option<&'a str>,
    watermark: u64,
}
fn update_checkpoint(
    tx: &Transaction<'_>,
    scope: &str,
    at: i64,
    old: Option<Contribution<'_>>,
    new: Option<Contribution<'_>>,
) -> anyhow::Result<()> {
    let day = at.div_euclid(DAY_MS) * DAY_MS;
    let mut checkpoint = read_checkpoint(tx, scope, day)?;
    if let Some(c) = old {
        checkpoint.change(c.amount, c.risk, c.state, c.revision, c.watermark, false)?;
    }
    if let Some(c) = new {
        checkpoint.change(c.amount, c.risk, c.state, c.revision, c.watermark, true)?;
    }
    write_checkpoint(tx, scope, day, &checkpoint)
}
/// Recompute every daily checkpoint of one scope from its attempt and gap
/// rows. Used by the owner reconcile to repair corrupt/missing checkpoints.
fn rebuild_checkpoints_in_tx(tx: &Transaction<'_>, scope: &str) -> anyhow::Result<usize> {
    let mut days: BTreeMap<i64, Checkpoint> = BTreeMap::new();
    {
        let mut attempts = tx.prepare(
            "SELECT started_at_ms,usage_picos,pending_risk_picos,metering_status,watermark,pricing_revision
             FROM value_quota_attempts WHERE scope=?1",
        )?;
        let rows = attempts.query_map([scope], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (at, amount, risk, state, mark, revision) = row?;
            days.entry(at.div_euclid(DAY_MS) * DAY_MS)
                .or_insert_with(Checkpoint::empty)
                .change(
                    &amount,
                    &risk,
                    &state,
                    Some(&revision),
                    u64::try_from(mark)?,
                    true,
                )?;
        }
        let mut gaps =
            tx.prepare("SELECT started_at_ms,watermark FROM value_quota_gaps WHERE scope=?1")?;
        let rows = gaps.query_map([scope], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (at, mark) = row?;
            days.entry(at.div_euclid(DAY_MS) * DAY_MS)
                .or_insert_with(Checkpoint::empty)
                .change("0", "0", "gap", None, u64::try_from(mark)?, true)?;
        }
    }
    tx.execute(
        "DELETE FROM value_quota_checkpoints WHERE scope=?1",
        [scope],
    )?;
    for (day, checkpoint) in &days {
        write_checkpoint(tx, scope, *day, checkpoint)?;
    }
    Ok(days.len())
}

pub(crate) fn install_snapshot(
    config_dir: &Path,
    scope: &Scope,
    revision: &str,
    snapshot: &PricingSnapshot,
    now: i64,
) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    let mut connection = open_database(&database_path(config_dir), false)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    install_in_tx(&tx, scope, revision, snapshot, now)?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn activate_snapshot(
    config_dir: &Path,
    scope: &Scope,
    revision: &str,
) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    let conn = open_database(&database_path(config_dir), false)?;
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3)",
        params![scope.router_url,scope.installation_id,revision],|r|r.get(0))?;
    ensure!(exists, "VALUE_QUOTA_SNAPSHOT_UNAVAILABLE");
    conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![format!("value_tariff:{}:{}",scope.router_url,scope.installation_id),revision])?;
    Ok(())
}
/// The acknowledged active revision only (no payload parse); used for the
/// `knownRevision` fetch contract and cheap readiness checks.
pub(crate) fn active_revision(config_dir: &Path, scope: &Scope) -> anyhow::Result<Option<String>> {
    assert_committed(config_dir)?;
    let conn = open_database(&database_path(config_dir), true)?;
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!(
                "value_tariff:{}:{}",
                scope.router_url, scope.installation_id
            )],
            |r| r.get(0),
        )
        .optional()?)
}
fn pending_tariff_key(scope: &Scope) -> String {
    format!(
        "value_tariff_pending:{}:{}",
        scope.router_url, scope.installation_id
    )
}
/// A validated, stored and Router-acknowledged revision whose
/// `effectiveFrom` has not passed yet. It is activated by the heartbeat once
/// due; until then the previous active revision keeps pricing admissions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PendingTariff {
    pub revision: String,
    pub effective_from_ms: i64,
}
pub(crate) fn pending_tariff(
    config_dir: &Path,
    scope: &Scope,
) -> anyhow::Result<Option<PendingTariff>> {
    assert_committed(config_dir)?;
    let conn = open_database(&database_path(config_dir), true)?;
    let raw: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [pending_tariff_key(scope)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|raw| serde_json::from_str(&raw).ok()))
}
pub(crate) fn set_pending_tariff(
    config_dir: &Path,
    scope: &Scope,
    pending: Option<&PendingTariff>,
) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    let conn = open_database(&database_path(config_dir), false)?;
    match pending {
        Some(pending) => {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3)",
                params![scope.router_url, scope.installation_id, pending.revision],
                |r| r.get(0),
            )?;
            ensure!(exists, "VALUE_QUOTA_SNAPSHOT_UNAVAILABLE");
            conn.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![pending_tariff_key(scope), serde_json::to_string(pending)?],
            )?;
        }
        None => {
            conn.execute("DELETE FROM meta WHERE key=?1", [pending_tariff_key(scope)])?;
        }
    }
    Ok(())
}
/// Whether this (router, installation) stored `revision` locally.
pub(crate) fn snapshot_installed(
    config_dir: &Path,
    scope: &Scope,
    revision: &str,
) -> anyhow::Result<bool> {
    assert_committed(config_dir)?;
    let conn = open_database(&database_path(config_dir), true)?;
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3)",
        params![scope.router_url, scope.installation_id, revision],
        |r| r.get(0),
    )?)
}
fn install_in_tx(
    tx: &Transaction<'_>,
    scope: &Scope,
    revision: &str,
    snapshot: &PricingSnapshot,
    now: i64,
) -> anyhow::Result<()> {
    scope.key()?;
    ensure!(
        snapshot.revision()?.as_str() == revision,
        "pricing snapshot hash mismatch"
    );
    let payload = serde_json::to_string(snapshot)?;
    let previous: Option<String> = tx.query_row(
        "SELECT payload_json FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3",
        params![scope.router_url, scope.installation_id, revision], |r| r.get(0)).optional()?;
    ensure!(
        previous.as_ref().is_none_or(|p| p == &payload),
        "immutable pricing revision conflict"
    );
    tx.execute("INSERT OR IGNORE INTO value_pricing_snapshots(router_url,installation_id,revision,payload_json,installed_at_ms) VALUES(?1,?2,?3,?4,?5)",
        params![scope.router_url,scope.installation_id,revision,payload,now])?;
    remember_snapshot(
        (
            scope.router_url.clone(),
            scope.installation_id.clone(),
            revision.to_owned(),
        ),
        Arc::new(snapshot.clone()),
    );
    Ok(())
}
fn next_watermark(tx: &Transaction<'_>) -> anyhow::Result<i64> {
    let raw: String = tx.query_row(
        "SELECT value FROM meta WHERE key='value_quota_watermark_v1'",
        [],
        |r| r.get(0),
    )?;
    let next = raw
        .parse::<i64>()?
        .checked_add(1)
        .context("value quota watermark overflow")?;
    tx.execute(
        "UPDATE meta SET value=?1 WHERE key='value_quota_watermark_v1'",
        [next.to_string()],
    )?;
    Ok(next)
}
pub(crate) fn begin_coverage(config_dir: &Path, scope: &Scope, now: i64) -> anyhow::Result<i64> {
    assert_committed(config_dir)?;
    let connection = open_database(&database_path(config_dir), false)?;
    ensure!(now >= 0, "invalid coverage timestamp");
    connection.execute(
        "INSERT OR IGNORE INTO value_quota_coverage(scope,collected_from_ms) VALUES(?1,?2)",
        params![scope.key()?, now],
    )?;
    Ok(connection.query_row(
        "SELECT collected_from_ms FROM value_quota_coverage WHERE scope=?1",
        [scope.key()?],
        |r| r.get(0),
    )?)
}
/// Coverage that starts at `from_ms` (inserted, or lowered to it). Used only
/// when the Server proved the user has no recorded Share request since
/// `from_ms`, so the empty ledger is the exact history of that span.
pub(crate) fn cover_from(config_dir: &Path, scope: &Scope, from_ms: i64) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    ensure!(from_ms >= 0, "invalid coverage timestamp");
    let connection = open_database(&database_path(config_dir), false)?;
    connection.execute(
        "INSERT INTO value_quota_coverage(scope,collected_from_ms) VALUES(?1,?2)
         ON CONFLICT(scope) DO UPDATE SET collected_from_ms=min(collected_from_ms,excluded.collected_from_ms)",
        params![scope.key()?, from_ms],
    )?;
    Ok(())
}
fn projection_digest(
    status: &crate::domain::usage::value_pricing::ValueQuotaStatus,
) -> anyhow::Result<String> {
    let mut status = status.clone();
    status.synced_at_ms = 0;
    Ok(sha256_hex(&serde_json::to_vec(&status)?))
}
pub(crate) fn value_sync_cursor(dir: &Path, scope: &Scope) -> anyhow::Result<String> {
    let conn = open_database(&database_path(dir), true)?;
    Ok(conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!(
                "value_sync_cursor:{}:{}",
                scope.router_url, scope.installation_id
            )],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default())
}
pub(crate) fn set_value_sync_cursor(dir: &Path, scope: &Scope, id: &str) -> anyhow::Result<()> {
    let conn = open_database(&database_path(dir), false)?;
    conn.execute(
        "INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![
            format!(
                "value_sync_cursor:{}:{}",
                scope.router_url, scope.installation_id
            ),
            id
        ],
    )?;
    Ok(())
}
pub(crate) fn projection_needs_sync(
    dir: &Path,
    scope: &Scope,
    status: &crate::domain::usage::value_pricing::ValueQuotaStatus,
    now: i64,
) -> anyhow::Result<bool> {
    let conn = open_database(&database_path(dir), true)?;
    let previous: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [format!("value_projection_ack:{}", scope.key()?)],
            |r| r.get(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Ok(true);
    };
    let previous: serde_json::Value = serde_json::from_str(&previous)?;
    Ok(
        previous.get("digest").and_then(|v| v.as_str())
            != Some(projection_digest(status)?.as_str())
            || previous
                .get("at")
                .and_then(|v| v.as_i64())
                .is_none_or(|at| now.saturating_sub(at) >= PROJECTION_KEEPALIVE_MS),
    )
}
pub(crate) fn ack_projection(
    dir: &Path,
    scope: &Scope,
    status: &crate::domain::usage::value_pricing::ValueQuotaStatus,
    ack: u64,
    now: i64,
) -> anyhow::Result<()> {
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let blocked = ack_in_tx(&tx, scope, status, ack, now)?;
    tx.commit()?;
    ensure!(!blocked, "VALUE_QUOTA_RESTORE_RECONCILIATION_REQUIRED");
    Ok(())
}
/// A Router ACK ahead of local authority means local history was lost
/// (e.g. an old backup). Only that (scope, window) is blocked; later windows
/// and other users keep running, and an owner reconcile clears it.
fn ack_in_tx(
    tx: &Transaction<'_>,
    scope: &Scope,
    status: &crate::domain::usage::value_pricing::ValueQuotaStatus,
    ack: u64,
    now: i64,
) -> anyhow::Result<bool> {
    if ack > status.usage_watermark {
        let window = Window {
            id: status.window_id.clone(),
            starts_at_ms: status.window_starts_at_ms,
            ends_at_ms: status.resets_at_ms,
            metered_from_ms: status.metered_from_ms,
        };
        if status_in_tx(tx, scope, &window)?.usage_watermark < ack {
            // Keep the highest Router watermark in the block record: the
            // reconcile (`set`) must move local authority past it, or the
            // next heartbeat would block the window again.
            let key = block_key(&scope.key()?, &window.id);
            let router_watermark = blocked_router_watermark(tx, &key)?.unwrap_or(0).max(ack);
            tx.execute(
                "INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![
                    key,
                    serde_json::json!({
                        "reason": ROUTER_AHEAD_REASON,
                        "routerWatermark": router_watermark,
                    })
                    .to_string()
                ],
            )?;
            return Ok(true);
        }
        return Ok(false);
    }
    ensure!(ack == status.usage_watermark, "VALUE_QUOTA_ACK_MISMATCH");
    let value = serde_json::json!({"digest":projection_digest(status)?,"at":now});
    tx.execute(
        "INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![
            format!("value_projection_ack:{}", scope.key()?),
            value.to_string()
        ],
    )?;
    Ok(false)
}
/// Router watermark recorded by a "Router ACK ahead of authority" block
/// (`None` for other block reasons or legacy plain-string records).
fn blocked_router_watermark(connection: &Connection, block: &str) -> anyhow::Result<Option<u64>> {
    let raw: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key=?1", [block], |r| r.get(0))
        .optional()?;
    Ok(raw
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| value.get("routerWatermark").and_then(|mark| mark.as_u64())))
}
/// Raises the global watermark counter so the next `next_watermark` value is
/// strictly above `floor`.
fn raise_watermark_floor(tx: &Transaction<'_>, floor: u64) -> anyhow::Result<()> {
    let floor = i64::try_from(floor).context("value quota watermark overflow")?;
    let raw: String = tx.query_row(
        "SELECT value FROM meta WHERE key='value_quota_watermark_v1'",
        [],
        |r| r.get(0),
    )?;
    if raw.parse::<i64>()? < floor {
        tx.execute(
            "UPDATE meta SET value=?1 WHERE key='value_quota_watermark_v1'",
            [floor.to_string()],
        )?;
    }
    Ok(())
}
fn window_blocked(connection: &Connection, key: &str, window_id: &str) -> anyhow::Result<bool> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key IN (?1,?2))",
        params![block_key(key, window_id), legacy_scope_block_key(key)],
        |r| r.get(0),
    )?)
}
fn status_in_tx(
    connection: &Connection,
    scope: &Scope,
    window: &Window,
) -> anyhow::Result<QuotaStatus> {
    window.validate()?;
    let key = scope.key()?;
    let blocked = window_blocked(connection, &key, &window.id)?;
    let coverage: Option<i64> = connection
        .query_row(
            "SELECT collected_from_ms FROM value_quota_coverage WHERE scope=?1",
            [&key],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        coverage.is_some_and(|at| at <= window.metered_start_ms()),
        "VALUE_QUOTA_COVERAGE_INCOMPLETE"
    );
    let from = window.metered_start_ms();
    let full_from = from
        .checked_add(DAY_MS - 1)
        .context("invalid value window")?
        .div_euclid(DAY_MS)
        * DAY_MS;
    let full_until = window.ends_at_ms.unwrap_or(i64::MAX).div_euclid(DAY_MS) * DAY_MS;
    let mut aggregate = Checkpoint::empty();
    let mut buckets=connection.prepare("SELECT day_start_ms FROM value_quota_checkpoints WHERE scope=?1 AND day_start_ms>=?2 AND day_start_ms<?3")?;
    for day in buckets.query_map(params![key, full_from, full_until], |r| r.get::<_, i64>(0))? {
        aggregate.merge(&read_checkpoint(connection, &key, day?)?)?;
    }
    // Missing materialized evidence cannot masquerade as an empty window.
    let actual_count:i64=connection.query_row("SELECT
        (SELECT count(*) FROM value_quota_attempts WHERE scope=?1 AND started_at_ms>=?2 AND started_at_ms<?3)+
        (SELECT count(*) FROM value_quota_gaps WHERE scope=?1 AND started_at_ms>=?2 AND started_at_ms<?3)",params![key,full_from,full_until],|r|r.get(0))?;
    ensure!(
        u64::try_from(actual_count)? == aggregate.count()?,
        "VALUE_QUOTA_CHECKPOINT_CORRUPT"
    );
    let mut query = connection.prepare(
        "SELECT usage_picos,pending_risk_picos,metering_status,watermark,pricing_revision FROM value_quota_attempts
         WHERE scope=?1 AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3)
         AND NOT(started_at_ms>=?4 AND started_at_ms<?5)")?;
    let rows = query.query_map(
        params![key, from, window.ends_at_ms, full_from, full_until],
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
        let (amount, pending, state, mark, revision) = row?;
        aggregate.change(
            &amount,
            &pending,
            &state,
            Some(&revision),
            u64::try_from(mark)?,
            true,
        )?;
    }
    let gaps: (i64, i64) = connection.query_row(
        "SELECT count(*),COALESCE(MAX(watermark),0) FROM value_quota_gaps
         WHERE scope=?1 AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3)
         AND NOT(started_at_ms>=?4 AND started_at_ms<?5)",
        params![key, from, window.ends_at_ms, full_from, full_until],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if gaps.0 > 0 {
        aggregate
            .states
            .insert("gap".into(), u64::try_from(gaps.0)?);
        aggregate.watermark = aggregate.watermark.max(u64::try_from(gaps.1)?);
    }
    let mut used = unsigned_decimal(&aggregate.amount)?;
    let mut watermark = aggregate.watermark;
    let last_adjustment:Option<(String,i64)>=connection.query_row(
        "SELECT payload_json,watermark FROM value_quota_adjustments WHERE scope=?1 AND window_id=?2 ORDER BY watermark DESC LIMIT 1",
        params![key,window.id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let observed = used;
    let mut offset = 0i128;
    if let Some((json, mark)) = last_adjustment {
        let payload: serde_json::Value = serde_json::from_str(&json)?;
        offset = unsigned_decimal(
            payload
                .get("offsetPicos")
                .and_then(|v| v.as_str())
                .context("invalid value adjustment")?,
        )?;
        used = used
            .checked_add(offset)
            .context("value adjusted quota overflow")?;
        watermark = watermark.max(u64::try_from(mark)?);
    }
    // Policy epochs are deliberately absent from this query: editing a grant
    // or changing units cannot erase requests in the same real window.
    Ok(QuotaStatus {
        observed_picos: observed.to_string(),
        manual_offset_picos: offset.to_string(),
        used_picos: used.to_string(),
        pending_risk_picos: aggregate.risk.clone(),
        metering_status: if blocked {
            "blocked".into()
        } else {
            aggregate.metering().into()
        },
        usage_watermark: watermark,
        usage_pricing_revisions_digest: sha256_hex(&serde_json::to_vec(
            &aggregate
                .revisions
                .keys()
                .collect::<std::collections::BTreeSet<_>>(),
        )?),
    })
}
/// Read-only status. A restore fence or a scoped block reports `blocked`
/// instead of failing, so one user's recovery state never hides others.
pub(crate) fn status(
    config_dir: &Path,
    scope: &Scope,
    window: &Window,
) -> anyhow::Result<QuotaStatus> {
    assert_committed(config_dir)?;
    let fenced = restore_fence_blocks(config_dir, scope, window)?;
    // Checkpoint payloads, indexed identity counts and manual offsets must be
    // read from one SQLite snapshot. Autocommit SELECTs could otherwise see
    // different concurrent revisions and report a spurious corrupt ledger.
    let mut connection = open_database(&database_path(config_dir), true)?;
    let tx = connection.transaction()?;
    let mut status = status_in_tx(&tx, scope, window)?;
    tx.commit()?;
    if fenced {
        status.metering_status = "blocked".into();
    }
    Ok(status)
}
pub(crate) fn admit(config_dir: &Path, input: &Admission<'_>) -> anyhow::Result<Admitted> {
    assert_committed(config_dir)?;
    if input.enforce && restore_fence_blocks(config_dir, input.scope, input.window)? {
        return Err(ValueQuotaRejection::new(ValueQuotaRejectionKind::Blocked).into());
    }
    let mut connection = open_database(&database_path(config_dir), false)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result = admit_in_tx(&tx, input)?;
    tx.commit()?;
    Ok(result)
}
fn admit_in_tx(tx: &Transaction<'_>, a: &Admission<'_>) -> anyhow::Result<Admitted> {
    a.window.validate()?;
    ensure!(
        !a.policy_epoch.is_empty() && !a.request_id.is_empty() && !a.attempt_id.is_empty(),
        "invalid admission identity"
    );
    ensure!(
        a.started_at_ms >= a.window.metered_start_ms()
            && a.window.ends_at_ms.is_none_or(|end| a.started_at_ms < end),
        "VALUE_QUOTA_WINDOW_CONFLICT"
    );
    let key = a.scope.key()?;
    let snapshot = cached_snapshot(
        tx,
        &a.scope.router_url,
        &a.scope.installation_id,
        a.pricing_revision,
    )?;
    let model = snapshot.model(a.effective_model)?;
    if !model.supports_tier(a.effective_tier) {
        return Err(ValueError::PriceUnavailable.into());
    }
    // policy_epoch is deliberately not part of the identity: a retry after a
    // policy edit is the same request in the same real window.
    let existing: Option<(String, String, String, String, String, i64)> = tx
        .query_row(
            "SELECT scope,pricing_revision,effective_model,effective_tier,window_id,started_at_ms
             FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
            params![a.request_id, a.attempt_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .optional()?;
    if let Some(old) = existing {
        ensure!(
            old == (
                key,
                a.pricing_revision.into(),
                a.effective_model.into(),
                a.effective_tier.into(),
                a.window.id.clone(),
                a.started_at_ms
            ),
            "VALUE_QUOTA_REQUEST_IDENTITY_CONFLICT"
        );
        return Ok(Admitted {
            reserved_picos: read_attempt_meta(tx, a.request_id, a.attempt_id)?
                .map(|meta| meta.reserved())
                .unwrap_or(0),
        });
    }
    let reserved = if a.enforce {
        let current = status_in_tx(tx, a.scope, a.window)?;
        if current.metering_status == "blocked" {
            return Err(ValueQuotaRejection::new(ValueQuotaRejectionKind::Blocked).into());
        }
        let spent = unsigned_decimal(&current.used_picos)?;
        let pending = unsigned_decimal(&current.pending_risk_picos)?;
        let limit = a.quota.limit_picos()?;
        if spent >= limit {
            return Err(ValueQuotaRejection {
                kind: ValueQuotaRejectionKind::Exhausted,
                resets_at_ms: a.window.ends_at_ms,
            }
            .into());
        }
        // Settled usage is below the limit; only in-flight reservations fill
        // it. That is temporary, so answer "busy" (retryable), never a
        // lock-out until the window resets.
        let available = limit
            .checked_sub(spent)
            .and_then(|v| v.checked_sub(pending))
            .context("value quota arithmetic overflow")?;
        if available <= 0 {
            return Err(ValueQuotaRejection::new(ValueQuotaRejectionKind::Busy).into());
        }
        let in_flight: i64 = tx.query_row(
            "SELECT count(*) FROM value_quota_attempts WHERE scope=?1 AND metering_status='pending'
             AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3)",
            params![key, a.window.metered_start_ms(), a.window.ends_at_ms],
            |r| r.get(0),
        )?;
        if u64::try_from(in_flight)? >= u64::from(a.parallel_limit.clamp(1, 4)) {
            return Err(ValueQuotaRejection::new(ValueQuotaRejectionKind::Busy).into());
        }
        // Bounded reservation: price(input estimate + output ceiling) at the
        // context tier implied by the estimate, never more than remains.
        model
            .price(a.effective_tier, &a.estimate.tokens())
            .map(|price| price.usage_picos)
            .unwrap_or(available)
            .clamp(0, available)
    } else {
        0
    };
    let watermark = next_watermark(tx)?;
    let frozen = serde_json::to_string(model)?;
    tx.execute(
        "INSERT INTO value_quota_attempts(request_id,attempt_id,scope,policy_epoch,window_id,started_at_ms,
         pricing_revision,effective_model,effective_tier,frozen_tariff_json,usage_revision,usage_picos,
         pending_risk_picos,metering_status,watermark) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,'0',?11,'pending',?12)",
        params![a.request_id,a.attempt_id,key,a.policy_epoch,a.window.id,a.started_at_ms,a.pricing_revision,
            a.effective_model,a.effective_tier,frozen,reserved.to_string(),watermark])?;
    tx.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![
            attempt_meta_key(a.request_id, a.attempt_id)?,
            serde_json::to_string(&AttemptMeta {
                input_tokens: a.estimate.input_tokens,
                output_tokens: a.estimate.output_tokens,
                reserved_picos: reserved.to_string(),
            })?
        ],
    )?;
    update_checkpoint(
        tx,
        &key,
        a.started_at_ms,
        None,
        Some(Contribution {
            amount: "0",
            risk: &reserved.to_string(),
            state: "pending",
            revision: Some(a.pricing_revision),
            watermark: u64::try_from(watermark)?,
        }),
    )?;
    // Legacy coverage gaps for the same request are superseded by the
    // admitted attempt in the same transaction.
    let gap: Option<(i64, i64)> = tx
        .query_row(
            "SELECT started_at_ms,watermark FROM value_quota_gaps WHERE scope=?1 AND request_id=?2",
            params![key, a.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((at, mark)) = gap {
        update_checkpoint(
            tx,
            &key,
            at,
            Some(Contribution {
                amount: "0",
                risk: "0",
                state: "gap",
                revision: None,
                watermark: u64::try_from(mark)?,
            }),
            None,
        )?;
        tx.execute(
            "DELETE FROM value_quota_gaps WHERE scope=?1 AND request_id=?2",
            params![key, a.request_id],
        )?;
    }
    Ok(Admitted {
        reserved_picos: reserved,
    })
}

/// How one usage observation settles an attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ObservedOutcome {
    /// Partial usage of a still-running attempt; risk stays reserved.
    InProgress,
    /// Complete single-attempt usage with matching model/tier evidence.
    Final,
    /// Complete usage priced exactly but with weaker evidence (multi-attempt,
    /// tier upgrade, merged cache TTL): `conservative`.
    FinalConservative,
    /// Not sent upstream, or upstream error status without usage: `$0`.
    Free,
    /// Interrupted/truncated stream: partial usage plus streamed-text output
    /// estimate, capped by the output ceiling; `conservative`.
    Interrupted,
    /// Outcome unknown: max(priced observed usage, reserved risk).
    Unknown,
}
impl ObservedOutcome {
    fn terminal(self) -> bool {
        self != Self::InProgress
    }
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Observation<'a> {
    pub request_id: &'a str,
    pub attempt_id: &'a str,
    pub scope: &'a Scope,
    pub usage_revision: u64,
    pub effective_model: &'a str,
    pub effective_tier: &'a str,
    pub reported_model: Option<&'a str>,
    /// Canonical tier observed for the request when it differs from the
    /// admitted tier; settlement charges the higher price conservatively.
    pub observed_tier: Option<&'a str>,
    pub tokens: Option<ValueTokens>,
    pub outcome: ObservedOutcome,
    /// Sweeper-produced observation: any later genuine terminal usage log may
    /// replace it even at the same revision.
    #[serde(skip)]
    pub synthetic: bool,
}
pub(crate) struct Adjustment<'a> {
    pub scope: &'a Scope,
    pub window: &'a Window,
    pub grant_revision: u64,
    pub edit: &'a crate::domain::usage::value_pricing::ValueUsageEdit,
}
/// Owner calibration. `set` is also the reconcile operation for its window:
/// it resolves stale pending attempts as outcome-unknown, rebuilds the
/// scope's checkpoints, clears the scope/window block, the legacy global
/// marker and the scope's restore fence, all audited in one record.
/// `stale_before_ms` is the sweeper's staleness bound (max provider request
/// timeout + margin): only pending attempts started before it are treated as
/// stuck; younger ones may still be live and keep their reservation.
pub(crate) fn apply_adjustments(
    config_dir: &Path,
    edits: &[Adjustment<'_>],
    actor: &str,
    now: i64,
    stale_before_ms: i64,
) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    let mut fenced = Vec::new();
    for a in edits {
        if a.edit.action == crate::domain::usage::value_pricing::ValueUsageEditAction::Set
            && read_restore_fence(config_dir)?
                .scopes
                .contains_key(&a.scope.key()?)
        {
            fenced.push(a.scope.key()?);
        }
    }
    let mut conn = open_database(&database_path(config_dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    apply_adjustments_in_tx(&tx, edits, actor, now, stale_before_ms, &fenced)?;
    tx.commit()?;
    // The audit record already notes the fence clear; the file write is
    // idempotent and repeated by a retried reconcile if it fails here.
    for key in fenced {
        clear_restore_fence(config_dir, &key)?;
    }
    Ok(())
}
fn apply_adjustments_in_tx(
    tx: &Transaction<'_>,
    edits: &[Adjustment<'_>],
    actor: &str,
    now: i64,
    stale_before_ms: i64,
    fenced: &[String],
) -> anyhow::Result<()> {
    use crate::domain::usage::value_pricing::ValueUsageEditAction;
    ensure!(
        !actor.is_empty() && !edits.is_empty() && edits.len() <= 500,
        "VALUE_QUOTA_TARGET_INVALID: invalid adjustment batch"
    );
    let mut scopes = std::collections::BTreeSet::new();
    for a in edits {
        let key = a.scope.key()?;
        ensure!(
            scopes.insert(key.clone()),
            "VALUE_QUOTA_TARGET_INVALID: duplicate adjustment scope"
        );
        let edit = a.edit;
        ensure!(
            !edit.operation_id.is_empty()
                && edit.operation_id.len() <= 128
                && !edit.reason.trim().is_empty()
                && edit.reason.len() <= 1000
                && edit.expected_grant_revision == a.grant_revision
                && edit.window_id == a.window.id,
            "VALUE_QUOTA_REVISION_CONFLICT"
        );
        let old: Option<(String, String)> = tx
            .query_row(
                "SELECT scope,payload_json FROM value_quota_adjustments WHERE operation_id=?1",
                [&edit.operation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_scope, json)) = old {
            let json: serde_json::Value = serde_json::from_str(&json)?;
            ensure!(
                old_scope == key
                    && json.get("edit") == Some(&serde_json::to_value(edit)?)
                    && json.get("actor").and_then(|v| v.as_str()) == Some(actor),
                "VALUE_QUOTA_OPERATION_CONFLICT"
            );
            continue;
        }
        let mut reconcile = serde_json::Value::Null;
        if edit.action == ValueUsageEditAction::Set {
            let checkpoint_days = rebuild_checkpoints_in_tx(tx, &key)?;
            let resolved = resolve_window_pending_in_tx(tx, &key, a.window, stale_before_ms)?;
            // A Router ACK ahead of local authority blocked this window; the
            // adjustment written below must carry a watermark above the
            // Router's so the next projection sync is accepted, not
            // re-blocked.
            let router_watermark = [block_key(&key, &a.window.id), legacy_scope_block_key(&key)]
                .iter()
                .map(|block| blocked_router_watermark(tx, block))
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .flatten()
                .max();
            if let Some(mark) = router_watermark {
                raise_watermark_floor(tx, mark)?;
            }
            let cleared = tx.execute(
                "DELETE FROM meta WHERE key IN (?1,?2,?3)",
                params![
                    block_key(&key, &a.window.id),
                    legacy_scope_block_key(&key),
                    LEGACY_DEGRADED_META
                ],
            )?;
            reconcile = serde_json::json!({"resolvedPendingAttempts":resolved,"rebuiltCheckpointDays":checkpoint_days,
                "clearedBlocks":cleared,"restoreFenceCleared":fenced.contains(&key),
                "routerWatermark":router_watermark});
        }
        // Any request moves the watermark, so an exact CAS would make owner
        // saves impossible under load. The idempotent operationId plus grant
        // revision/window checks guard the edit; the target is validated
        // against the observed usage at apply time.
        let current = status_in_tx(tx, a.scope, a.window)?;
        let observed = unsigned_decimal(&current.observed_picos)?;
        let (offset, target) = match edit.action {
            ValueUsageEditAction::Clear => {
                ensure!(
                    edit.target_picos.is_none(),
                    "VALUE_QUOTA_TARGET_INVALID: clear must not include targetPicos"
                );
                (0, None)
            }
            ValueUsageEditAction::Set => {
                let target = unsigned_decimal(
                    edit.target_picos
                        .as_deref()
                        .context("VALUE_QUOTA_TARGET_INVALID: targetPicos is required")?,
                )?;
                ensure!(
                    target >= observed
                        && target
                            <= crate::domain::usage::value_pricing::MAX_LIMIT_MICROS * 1_000_000,
                    "VALUE_QUOTA_TARGET_BELOW_OBSERVED_OR_OUT_OF_RANGE"
                );
                (target - observed, Some(target.to_string()))
            }
        };
        let watermark = next_watermark(tx)?;
        let payload = serde_json::json!({"edit":edit,"actor":actor,"offsetPicos":offset.to_string(),"observedPicos":current.observed_picos,
            "beforePicos":current.used_picos,"afterPicos":(observed+offset).to_string(),
            "observedWatermark":current.usage_watermark,"reconcile":reconcile});
        tx.execute("INSERT INTO value_quota_adjustments(operation_id,scope,window_id,payload_json,target_picos,observed_watermark,actor,reason,created_at_ms,watermark)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![edit.operation_id,key,a.window.id,payload.to_string(),target,
            i64::try_from(current.usage_watermark)?,actor,edit.reason,now,watermark])?;
    }
    Ok(())
}
/// Usage and the executable-quota revision commit together. A settlement
/// that cannot be applied rolls back only its USD savepoint (the usage/Token
/// evidence still commits) and blocks just the affected (scope, window)
/// until an owner reconcile; every other user keeps running.
pub(super) fn settle_usage_log_isolated(
    tx: &Transaction<'_>,
    log: &crate::domain::usage::store::UsageLog,
) -> anyhow::Result<bool> {
    tx.execute_batch("SAVEPOINT share_value_settlement")?;
    match settle_usage_log(tx, log) {
        Ok(()) => {
            tx.execute_batch("RELEASE share_value_settlement")?;
            Ok(false)
        }
        Err(error) => {
            tx.execute_batch("ROLLBACK TO share_value_settlement; RELEASE share_value_settlement")?;
            let targets = {
                let mut query = tx.prepare(
                    "SELECT DISTINCT scope,window_id FROM value_quota_attempts WHERE request_id=?1",
                )?;
                let rows = query.query_map([&log.request_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (scope, window) in &targets {
                tx.execute(
                    "INSERT INTO meta(key,value) VALUES(?1,'settlement_requires_reconcile') ON CONFLICT(key) DO NOTHING",
                    [block_key(scope, window)],
                )?;
            }
            tracing::error!(%error, request_id = %log.request_id, windows = targets.len(),
                "USD settlement isolated; usage evidence is kept and only this user's window awaits reconcile");
            Ok(false)
        }
    }
}
fn in_progress_log(log: &crate::domain::usage::store::UsageLog) -> bool {
    use crate::domain::usage::store::UsageState;
    log.usage_state == UsageState::Pending
        || (log.is_streaming
            && matches!(log.stream_status.as_deref(), Some("pending" | "streaming")))
}
/// Lenient partial usage (an interrupted stream may lack output or input).
fn partial_tokens(
    log: &crate::domain::usage::store::UsageLog,
    meta: Option<&AttemptMeta>,
) -> ValueTokens {
    let cache = log.cache_creation_tokens.unwrap_or(0);
    let (cache_write5m, cache_write1h, cache_write_unknown) = match log.cache_creation_details {
        Some(d) if d.five_minutes.checked_add(d.one_hour) == Some(cache) => {
            (d.five_minutes, d.one_hour, 0)
        }
        _ => (0, 0, cache),
    };
    let ceiling = meta.map_or(MAX_OUTPUT_CEILING, |meta| meta.output_tokens.max(1));
    let streamed = log.value_streamed_text_bytes.unwrap_or(0).div_ceil(3);
    ValueTokens {
        fresh_input: log
            .input_tokens
            .or_else(|| meta.map(|meta| meta.input_tokens))
            .unwrap_or(0),
        output: log.output_tokens.unwrap_or(0).max(streamed).min(ceiling),
        cache_read: log.cache_read_tokens.unwrap_or(0),
        cache_write5m,
        cache_write1h,
        cache_write_unknown,
    }
}
/// Outcome classification of one usage log for one admitted attempt.
fn classify_log(
    log: &crate::domain::usage::store::UsageLog,
    model: &str,
    admitted_tier: &str,
    meta: Option<&AttemptMeta>,
    force_terminal: bool,
) -> (ObservedOutcome, Option<ValueTokens>, Option<&'static str>) {
    use crate::domain::usage::store::RequestKind;
    let actual = log.actual_model.as_deref().or(log.model.as_deref());
    let evidence_ok = actual == Some(model)
        && !log.reported_model_mismatch.unwrap_or(false)
        && !log.reported_model_conflict.unwrap_or(false)
        && log.request_kind == RequestKind::Text
        && !log.is_health_check;
    let tokens = (!log.usage_estimated)
        .then(|| tokens_for_log(log))
        .flatten();
    let has_usage = tokens.is_some_and(|t| t != ValueTokens::default());
    let tier = canonical_service_tier(log.effective_service_tier.as_deref());
    let observed_tier = tier.filter(|tier| *tier != admitted_tier);
    if in_progress_log(log) && !force_terminal {
        return (
            ObservedOutcome::InProgress,
            tokens.filter(|_| evidence_ok),
            None,
        );
    }
    let streamed = log.value_streamed_text_bytes.unwrap_or(0);
    if log.status_code >= 400 && !has_usage && streamed == 0 {
        return (ObservedOutcome::Free, None, None);
    }
    if !evidence_ok || tier.is_none() {
        return (ObservedOutcome::Unknown, tokens, None);
    }
    let completed = if log.is_streaming {
        log.stream_status.as_deref() == Some("completed")
    } else {
        !in_progress_log(log)
    };
    if completed && ((200..300).contains(&log.status_code) || has_usage) {
        return match tokens {
            Some(tokens) if log.attempt_count > 1 || observed_tier.is_some() => (
                ObservedOutcome::FinalConservative,
                Some(tokens),
                observed_tier,
            ),
            Some(tokens) => (ObservedOutcome::Final, Some(tokens), None),
            None => (ObservedOutcome::Unknown, None, None),
        };
    }
    if log.is_streaming {
        return (
            ObservedOutcome::Interrupted,
            Some(partial_tokens(log, meta)),
            observed_tier,
        );
    }
    (ObservedOutcome::Unknown, tokens, None)
}
/// The usage log of one request settles the admitted attempt it belongs to
/// (`attempt_id == attempt_count`) and nothing else.
pub(super) fn settle_usage_log(
    tx: &Transaction<'_>,
    log: &crate::domain::usage::store::UsageLog,
) -> anyhow::Result<()> {
    settle_log_in_tx(tx, log, false).map(|_| ())
}
fn settle_log_in_tx(
    tx: &Transaction<'_>,
    log: &crate::domain::usage::store::UsageLog,
    force_terminal: bool,
) -> anyhow::Result<bool> {
    let rows = {
        let mut q=tx.prepare("SELECT attempt_id,scope,effective_model,effective_tier,usage_revision FROM value_quota_attempts WHERE request_id=?1")?;
        let mapped = q.query_map([&log.request_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    // Only the attempt this log describes is settled. Any other admitted
    // attempt of the request (e.g. an earlier one that was sent) is left to
    // its terminal guard or the sweeper; a fallback here could settle a sent
    // attempt with this log's (possibly free) outcome.
    let wanted = log.attempt_count.max(1).to_string();
    let Some((attempt, scope, model, tier, old_revision)) =
        rows.iter().find(|row| row.0 == wanted).cloned()
    else {
        if !rows.is_empty() {
            tracing::debug!(request_id = %log.request_id, attempt = %wanted,
                "usage log matches no admitted USD attempt; left to the guard or sweeper");
        }
        return Ok(false);
    };
    let scope: Scope = serde_json::from_str(&scope)?;
    ensure!(
        log.share_id.as_deref() == Some(&scope.share_id)
            && log
                .user_email
                .as_deref()
                .is_some_and(|email| email.eq_ignore_ascii_case(&scope.email)),
        "VALUE_QUOTA_USAGE_SCOPE_CONFLICT"
    );
    let meta = read_attempt_meta(tx, &log.request_id, &attempt)?;
    let (outcome, tokens, observed_tier) =
        classify_log(log, &model, &tier, meta.as_ref(), force_terminal);
    // revision 0 is reserved for the pre-send admission marker.
    let mut usage_revision = log
        .usage_revision
        .checked_add(1)
        .context("value usage revision overflow")?;
    if force_terminal {
        usage_revision = usage_revision.max(u64::try_from(old_revision)?);
    }
    settle_in_tx(
        tx,
        &Observation {
            request_id: &log.request_id,
            attempt_id: &attempt,
            scope: &scope,
            usage_revision,
            effective_model: &model,
            effective_tier: &tier,
            reported_model: log.reported_model.as_deref(),
            observed_tier,
            tokens,
            outcome,
            synthetic: force_terminal,
        },
    )
}
fn tokens_for_log(log: &crate::domain::usage::store::UsageLog) -> Option<ValueTokens> {
    let fresh_input = log.input_tokens?;
    let output = log.output_tokens?;
    let cache_read = log.cache_read_tokens.unwrap_or(0);
    let cache = log.cache_creation_tokens.unwrap_or(0);
    let (cache_write5m, cache_write1h, cache_write_unknown) = match log.cache_creation_details {
        Some(d) if d.five_minutes.checked_add(d.one_hour)? == cache => {
            (d.five_minutes, d.one_hour, 0)
        }
        Some(_) => return None,
        None => (0, 0, cache),
    };
    let tokens = ValueTokens {
        fresh_input,
        output,
        cache_read,
        cache_write5m,
        cache_write1h,
        cache_write_unknown,
    };
    if log
        .raw_input_tokens
        .is_some_and(|raw| tokens.raw_input().ok() != Some(raw))
    {
        return None;
    }
    Some(tokens)
}
type AttemptStateRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
    String,
    i64,
    Option<i64>,
    String,
);
fn read_attempt_row(
    tx: &Transaction<'_>,
    request_id: &str,
    attempt_id: &str,
) -> anyhow::Result<Option<AttemptStateRow>> {
    Ok(tx
        .query_row(
            "SELECT scope,effective_model,effective_tier,frozen_tariff_json,usage_revision,usage_picos,observation_sha256,
             pending_risk_picos,metering_status,started_at_ms,compacted_at_ms,pricing_revision
             FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
            params![request_id, attempt_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                    r.get(10)?,
                    r.get(11)?,
                ))
            },
        )
        .optional()?)
}
#[allow(clippy::too_many_arguments)]
fn write_settlement(
    tx: &Transaction<'_>,
    request_id: &str,
    attempt_id: &str,
    old: &AttemptStateRow,
    revision: i64,
    digest: &str,
    observation_json: &str,
    amount: i128,
    state: &str,
    risk: i128,
) -> anyhow::Result<()> {
    let watermark = next_watermark(tx)?;
    let (amount, risk) = (amount.to_string(), risk.to_string());
    tx.execute(
        "INSERT OR REPLACE INTO value_quota_revisions(request_id,attempt_id,usage_revision,observation_sha256,usage_picos,metering_status,watermark,observation_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![request_id,attempt_id,revision,digest,amount,state,watermark,observation_json])?;
    tx.execute(
        "UPDATE value_quota_attempts SET usage_revision=?3,usage_picos=?4,metering_status=?5,watermark=?6,
         observation_sha256=?7,pending_risk_picos=?8
         WHERE request_id=?1 AND attempt_id=?2",
        params![request_id, attempt_id, revision, amount, state, watermark, digest, risk],
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
            amount: &amount,
            risk: &risk,
            state,
            revision: Some(&old.11),
            watermark: u64::try_from(watermark)?,
        }),
    )?;
    if old.10.is_some() && matches!(state, "observed" | "conservative") {
        tx.execute("DELETE FROM value_quota_revisions WHERE request_id=?1 AND attempt_id=?2 AND usage_revision<?3",params![request_id,attempt_id,revision])?;
    }
    Ok(())
}
fn settle_in_tx(tx: &Transaction<'_>, o: &Observation<'_>) -> anyhow::Result<bool> {
    let old =
        read_attempt_row(tx, o.request_id, o.attempt_id)?.context("VALUE_QUOTA_ATTEMPT_UNKNOWN")?;
    ensure!(
        old.0 == o.scope.key()? && old.1 == o.effective_model && old.2 == o.effective_tier,
        "VALUE_QUOTA_OBSERVATION_SCOPE_CONFLICT"
    );
    let revision = i64::try_from(o.usage_revision)?;
    let observation_json = serde_json::to_string(o)?;
    let digest = if o.synthetic {
        format!("synthetic:{}", sha256_hex(observation_json.as_bytes()))
    } else {
        sha256_hex(observation_json.as_bytes())
    };
    if revision < old.4 {
        return Ok(false);
    }
    if revision == old.4 {
        if let Some(previous) = old.6.as_deref() {
            if previous == digest {
                return Ok(false);
            }
            // Same revision, different evidence: a synthetic (outcome or
            // sweeper) settlement yields to genuine usage; a terminal
            // observation completes a pending one; otherwise first wins.
            let previous_synthetic = previous.starts_with("synthetic:");
            let replace = if o.synthetic {
                old.8 == "pending" || old.8 == "unavailable"
            } else {
                previous_synthetic || (old.8 == "pending" && o.outcome.terminal())
            };
            if !replace {
                tracing::debug!(request_id = %o.request_id, "same-revision USD observation ignored");
                return Ok(false);
            }
        }
    }
    let scope: Scope = serde_json::from_str(&old.0)?;
    let snapshot = cached_snapshot(tx, &scope.router_url, &scope.installation_id, &old.11)?;
    let tariff: ExecutionModelPrice = if old.10.is_some() {
        ensure!(old.3 == "null", "VALUE_QUOTA_COMPACTED_TARIFF_CORRUPT");
        snapshot.model(o.effective_model)?.clone()
    } else {
        serde_json::from_str(&old.3)?
    };
    ensure!(
        snapshot.model(o.effective_model)? == &tariff,
        "VALUE_QUOTA_FROZEN_TARIFF_CORRUPT"
    );
    let canonical = |model: &str| {
        snapshot
            .exact_aliases
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_owned())
    };
    let reported_matches = o
        .reported_model
        .is_none_or(|reported| canonical(reported) == canonical(o.effective_model));
    let reserved = read_attempt_meta(tx, o.request_id, o.attempt_id)?
        .map(|meta| meta.reserved())
        .unwrap_or(0);
    let old_amount = unsigned_decimal(&old.5)?;
    let price = |tier: &str| {
        o.tokens
            .as_ref()
            .and_then(|tokens| tariff.price(tier, tokens).ok())
    };
    let unknown = || {
        let priced = price(o.effective_tier).map_or(0, |p| p.usage_picos);
        let floor = if old.8 == "pending" { old_amount } else { 0 };
        (priced.max(reserved).max(floor), "conservative")
    };
    let (amount, state) = match o.outcome {
        ObservedOutcome::InProgress => (
            price(o.effective_tier).map_or(old_amount, |p| p.usage_picos),
            "pending",
        ),
        ObservedOutcome::Free => (0, "observed"),
        ObservedOutcome::Final
        | ObservedOutcome::FinalConservative
        | ObservedOutcome::Interrupted
            if reported_matches =>
        {
            match price(o.effective_tier) {
                Some(priced) => {
                    let mut amount = priced.usage_picos;
                    let mut conservative =
                        priced.conservative_cache_ttl || o.outcome != ObservedOutcome::Final;
                    if let Some(observed) = o.observed_tier.filter(|t| *t != o.effective_tier) {
                        if let Some(higher) = price(observed) {
                            amount = amount.max(higher.usage_picos);
                        }
                        conservative = true;
                    }
                    (
                        amount,
                        if conservative {
                            "conservative"
                        } else {
                            "observed"
                        },
                    )
                }
                None => unknown(),
            }
        }
        _ => unknown(),
    };
    let risk = if state == "pending" {
        unsigned_decimal(&old.7)?
            .checked_add(old_amount)
            .and_then(|v| v.checked_sub(amount))
            .context("value quota risk overflow")?
            .max(0)
    } else {
        0
    };
    write_settlement(
        tx,
        o.request_id,
        o.attempt_id,
        &old,
        revision,
        &digest,
        &observation_json,
        amount,
        state,
        risk,
    )?;
    Ok(true)
}
/// Outcome of an admitted attempt that produced no usage log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum AttemptOutcome {
    /// Never written upstream (connect/DNS/TLS failure, early error, client
    /// gone before send): `$0`, observed.
    NotSent,
    /// Upstream answered an error HTTP status and no usage was observed.
    UpstreamError,
    /// Sent but the outcome is unknown: the reserved risk is charged.
    Unknown,
}
/// Synthetic settlement of a still-unobserved (or, for the sweeper/reconcile,
/// still-pending) attempt. It writes the current revision with a
/// `synthetic:` digest so any genuine usage log at that or a later revision
/// wins; a second outcome for the same attempt is a no-op (never double
/// counts).
fn settle_outcome_in_tx(
    tx: &Transaction<'_>,
    request_id: &str,
    attempt_id: &str,
    outcome: AttemptOutcome,
    only_unobserved: bool,
) -> anyhow::Result<bool> {
    let Some(old) = read_attempt_row(tx, request_id, attempt_id)? else {
        return Ok(false);
    };
    if !matches!(old.8.as_str(), "pending" | "unavailable")
        || (only_unobserved && (old.4 != 0 || old.6.is_some()))
    {
        return Ok(false);
    }
    let reserved = read_attempt_meta(tx, request_id, attempt_id)?
        .map(|meta| meta.reserved())
        .unwrap_or(0);
    let (amount, state) = match outcome {
        AttemptOutcome::NotSent | AttemptOutcome::UpstreamError => (0, "observed"),
        AttemptOutcome::Unknown => (reserved.max(unsigned_decimal(&old.5)?), "conservative"),
    };
    let observation_json = serde_json::json!({"requestId":request_id,"attemptId":attempt_id,
        "usageRevision":old.4,"outcome":outcome,"amountPicos":amount.to_string()})
    .to_string();
    let digest = format!("synthetic:{}", sha256_hex(observation_json.as_bytes()));
    write_settlement(
        tx,
        request_id,
        attempt_id,
        &old,
        old.4,
        &digest,
        &observation_json,
        amount,
        state,
        0,
    )?;
    Ok(true)
}
/// Settles an admitted attempt that will not (or did not) produce a usage
/// log. Idempotent with normal settlement: an attempt already observed by a
/// usage log is left untouched, and a later usage log replaces this result.
pub(crate) fn settle_attempt_outcome(
    config_dir: &Path,
    request_id: &str,
    attempt_id: &str,
    outcome: AttemptOutcome,
) -> anyhow::Result<bool> {
    assert_committed(config_dir)?;
    let mut conn = open_database(&database_path(config_dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let settled = settle_outcome_in_tx(&tx, request_id, attempt_id, outcome, true)?;
    tx.commit()?;
    Ok(settled)
}
fn resolve_window_pending_in_tx(
    tx: &Transaction<'_>,
    key: &str,
    window: &Window,
    started_before: i64,
) -> anyhow::Result<usize> {
    let rows = {
        let mut query = tx.prepare(
            "SELECT request_id,attempt_id FROM value_quota_attempts WHERE scope=?1
             AND metering_status IN ('pending','unavailable') AND started_at_ms>=?2
             AND (?3 IS NULL OR started_at_ms<?3) AND started_at_ms<?4",
        )?;
        let rows = query.query_map(
            params![
                key,
                window.metered_start_ms(),
                window.ends_at_ms,
                started_before
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut resolved = 0;
    for (request, attempt) in rows {
        resolved += usize::from(settle_outcome_in_tx(
            tx,
            &request,
            &attempt,
            AttemptOutcome::Unknown,
            false,
        )?);
    }
    Ok(resolved)
}
/// Candidates per sweeper run and per short write transaction.
const SWEEP_CANDIDATE_LIMIT: usize = 400;
const SWEEP_WRITE_BATCH: usize = 25;
/// Stale attempts, oldest `pending` first, then legacy `unavailable` rows
/// (legacy Token-shadow rows may be numerous; they hold no reservation and
/// are not counted by admission's concurrency check, which only counts
/// `pending` rows of the admitted scope's current window).
fn sweep_candidates(
    connection: &Connection,
    started_before_ms: i64,
    limit: usize,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut found = Vec::new();
    for state in ["pending", "unavailable"] {
        let remaining = limit.saturating_sub(found.len());
        if remaining == 0 {
            break;
        }
        let mut query = connection.prepare(
            "SELECT request_id,attempt_id FROM value_quota_attempts
             WHERE metering_status=?1 AND started_at_ms<?2
             ORDER BY started_at_ms LIMIT ?3",
        )?;
        let rows = query.query_map(
            params![state, started_before_ms, i64::try_from(remaining)?],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?;
        found.extend(rows.collect::<rusqlite::Result<Vec<_>>>()?);
    }
    Ok(found)
}
/// One stale attempt: classified from its persisted usage log when present
/// (an unfinished stream counts as interrupted), else outcome-unknown.
fn sweep_one_in_tx(
    tx: &Transaction<'_>,
    request: &str,
    attempt: &str,
    report: &mut SweepReport,
) -> anyhow::Result<()> {
    use crate::domain::usage::store::UsageLog;
    let still_stale: Option<bool> = tx
        .query_row(
            "SELECT metering_status IN ('pending','unavailable') FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
            params![request, attempt],
            |r| r.get(0),
        )
        .optional()?;
    if still_stale != Some(true) {
        // Settled concurrently between candidate selection and this batch.
        return Ok(());
    }
    let log: Option<UsageLog> = tx
        .query_row(
            "SELECT payload_json FROM usage_records WHERE request_id=?1",
            [request],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|json| serde_json::from_str(&json).ok());
    if let Some(log) = log.filter(|log| log.attempt_count.max(1).to_string() == attempt) {
        // A log that cannot be classified falls back to outcome-unknown.
        tx.execute_batch("SAVEPOINT value_sweep_log")?;
        match settle_log_in_tx(tx, &log, true) {
            Ok(_) => tx.execute_batch("RELEASE value_sweep_log")?,
            Err(error) => {
                tx.execute_batch("ROLLBACK TO value_sweep_log; RELEASE value_sweep_log")?;
                tracing::warn!(%error, request_id = %request, "stale USD attempt log could not be classified");
            }
        }
        let resolved: bool = tx.query_row(
            "SELECT metering_status NOT IN ('pending','unavailable') FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
            params![request, attempt],
            |r| r.get(0),
        )?;
        if resolved {
            report.from_usage_log += 1;
            return Ok(());
        }
    }
    if settle_outcome_in_tx(tx, request, attempt, AttemptOutcome::Unknown, false)? {
        report.outcome_unknown += 1;
    }
    Ok(())
}
/// Settles a batch, each attempt in its own savepoint. An attempt that
/// cannot be settled (e.g. a corrupt checkpoint) blocks only its own
/// (scope, window) for owner reconcile; the rest of the batch still commits.
fn sweep_batch_in_tx(
    tx: &Transaction<'_>,
    rows: &[(String, String)],
    report: &mut SweepReport,
) -> anyhow::Result<()> {
    for (request, attempt) in rows {
        tx.execute_batch("SAVEPOINT value_sweep")?;
        match sweep_one_in_tx(tx, request, attempt, report) {
            Ok(()) => tx.execute_batch("RELEASE value_sweep")?,
            Err(error) => {
                tx.execute_batch("ROLLBACK TO value_sweep; RELEASE value_sweep")?;
                let target: Option<(String, String)> = tx
                    .query_row(
                        "SELECT scope,window_id FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
                        params![request, attempt],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                if let Some((scope, window)) = target {
                    tx.execute(
                        "INSERT INTO meta(key,value) VALUES(?1,'settlement_requires_reconcile') ON CONFLICT(key) DO NOTHING",
                        [block_key(&scope, &window)],
                    )?;
                }
                report.blocked += 1;
                tracing::warn!(%error, request_id = %request,
                    "stale USD attempt could not be settled; only its window awaits reconcile");
            }
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SweepReport {
    pub from_usage_log: usize,
    pub outcome_unknown: usize,
    /// Attempts whose settlement failed; their window is blocked.
    pub blocked: usize,
}
/// Settles attempts left `pending` (or legacy `unavailable`) by a crash,
/// a lost usage write, or a stream that outlived every timeout. Attempts
/// started before `started_before_ms` (the later of process start and
/// now - (max request timeout + 5 min)) are selected in a read transaction
/// (WAL readers never block usage writes; the status scan is unindexed and
/// the schema is frozen), then settled in short IMMEDIATE write batches.
pub(crate) fn sweep_stale_attempts(
    config_dir: &Path,
    started_before_ms: i64,
) -> anyhow::Result<SweepReport> {
    assert_committed(config_dir)?;
    let candidates = {
        let mut reader = open_database(&database_path(config_dir), true)?;
        let tx = reader.transaction()?;
        let rows = sweep_candidates(&tx, started_before_ms, SWEEP_CANDIDATE_LIMIT)?;
        tx.commit()?;
        rows
    };
    let mut report = SweepReport::default();
    if candidates.is_empty() {
        return Ok(report);
    }
    let mut conn = open_database(&database_path(config_dir), false)?;
    for batch in candidates.chunks(SWEEP_WRITE_BATCH) {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        sweep_batch_in_tx(&tx, batch, &mut report)?;
        tx.commit()?;
    }
    Ok(report)
}
/// Bounded maintenance never deletes attempt identities, pending risk, frozen
/// snapshot references, highest revisions, or the latest observation digest.
/// Older revisions are redundant once their exact replacement is committed.
pub(crate) fn compact_terminal_history(dir: &Path, now: i64) -> anyhow::Result<usize> {
    assert_committed(dir)?;
    let mut conn = open_database(&database_path(dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let last: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='value_compaction_at'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if last
        .and_then(|s| s.parse::<i64>().ok())
        .is_some_and(|at| now.saturating_sub(at) < 60_000)
    {
        return Ok(0);
    }
    let count = compact_in_tx(&tx, now.saturating_sub(30 * DAY_MS), now)?;
    tx.execute("INSERT INTO meta VALUES('value_compaction_at',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[now.to_string()])?;
    tx.commit()?;
    Ok(count)
}
fn compact_in_tx(tx: &Transaction<'_>, cutoff: i64, now: i64) -> anyhow::Result<usize> {
    let rows = {
        let mut q=tx.prepare("SELECT request_id,attempt_id,scope,started_at_ms FROM value_quota_attempts
        WHERE started_at_ms<?1 AND metering_status IN ('observed','conservative') AND compacted_at_ms IS NULL ORDER BY started_at_ms,request_id,attempt_id LIMIT 100")?;
        let rows = q.query_map([cutoff], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (request, attempt, key, at) in &rows {
        let checkpoint = read_checkpoint(tx, key, at.div_euclid(DAY_MS) * DAY_MS)?;
        ensure!(checkpoint.count()? > 0, "VALUE_QUOTA_CHECKPOINT_CORRUPT");
        tx.execute("DELETE FROM value_quota_revisions WHERE request_id=?1 AND attempt_id=?2 AND usage_revision<
            (SELECT usage_revision FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2)",params![request,attempt])?;
        tx.execute("UPDATE value_quota_attempts SET frozen_tariff_json='null',compacted_at_ms=?3 WHERE request_id=?1 AND attempt_id=?2",params![request,attempt,now])?;
        tx.execute(
            "DELETE FROM meta WHERE key=?1",
            [attempt_meta_key(request, attempt)?],
        )?;
    }
    Ok(rows.len())
}
/// Diagnostics scan the share's rows via json_extract (unindexed); this is an
/// admin-only read and intentionally not on the request path.
pub(crate) fn diagnostics(dir: &Path, share_id: &str) -> anyhow::Result<serde_json::Value> {
    assert_committed(dir)?;
    let conn = open_database(&database_path(dir), true)?;
    let counts:(i64,i64,i64,Option<i64>)=conn.query_row("SELECT count(*),COALESCE(sum(metering_status='pending'),0),
        COALESCE(sum(metering_status='unavailable'),0),min(CASE WHEN metering_status IN ('pending','unavailable') THEN started_at_ms END)
        FROM value_quota_attempts WHERE json_extract(scope,'$.shareId')=?1",[share_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
    let gaps: i64 = conn.query_row(
        "SELECT count(*) FROM value_quota_gaps WHERE json_extract(scope,'$.shareId')=?1",
        [share_id],
        |r| r.get(0),
    )?;
    let checkpoints: i64 = conn.query_row(
        "SELECT count(*) FROM value_quota_checkpoints WHERE json_extract(scope,'$.shareId')=?1",
        [share_id],
        |r| r.get(0),
    )?;
    let blocked: i64 = conn.query_row(
        "SELECT count(*) FROM meta WHERE key LIKE 'value_quota_blocked:%' AND instr(key,?1)>0",
        [format!("\"shareId\":{}", serde_json::to_string(share_id)?)],
        |r| r.get(0),
    )?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(
        serde_json::json!({"attempts":counts.0,"pendingAttempts":counts.1,"unavailableAttempts":counts.2,
        "coverageGaps":gaps,"oldestUnresolvedAtMs":counts.3,"dailyCheckpoints":checkpoints,"blockedWindows":blocked,
        "databaseBytes":pages.saturating_mul(page_size),"capacityWarning":counts.0>=1_000_000 || pages.saturating_mul(page_size)>=4*1024*1024*1024i64,
        "retention":"attempt identities and latest revisions are never deleted; terminal duplicate tariffs and superseded revisions compact after 30 days"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    /// 2M tokens at $1/M = $2: larger than the $1 fixture limit, so the
    /// bounded reservation equals the remaining capacity.
    const ESTIMATE: RiskEstimate = RiskEstimate {
        input_tokens: 1_000_000,
        output_tokens: 1_000_000,
    };

    #[test]
    fn value_quota_reported_model_requires_the_same_identity_or_a_declared_exact_alias() {
        for reported in ["model", "declared-alias", "model-20261030", "other-model"] {
            let (mut conn, scope, window, quota, revision) = setup();
            let json: String = conn
                .query_row(
                    "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
                    [&revision],
                    |row| row.get(0),
                )
                .unwrap();
            let mut snapshot: PricingSnapshot = serde_json::from_str(&json).unwrap();
            snapshot
                .exact_aliases
                .insert("declared-alias".into(), "model".into());
            let revision = snapshot.revision().unwrap();
            let tx = conn.transaction().unwrap();
            install_in_tx(&tx, &scope, &revision, &snapshot, 0).unwrap();
            admit_in_tx(
                &tx,
                &Admission {
                    enforce: true,
                    scope: &scope,
                    window: &window,
                    policy_epoch: "epoch",
                    quota: &quota,
                    pricing_revision: &revision,
                    request_id: "reported-model",
                    attempt_id: "1",
                    effective_model: "model",
                    effective_tier: "standard",
                    started_at_ms: 1,
                    estimate: ESTIMATE,
                    parallel_limit: 4,
                },
            )
            .unwrap();
            settle_in_tx(
                &tx,
                &Observation {
                    request_id: "reported-model",
                    attempt_id: "1",
                    scope: &scope,
                    usage_revision: 1,
                    effective_model: "model",
                    effective_tier: "standard",
                    reported_model: Some(reported),
                    tokens: Some(ValueTokens {
                        fresh_input: 1,
                        ..Default::default()
                    }),
                    observed_tier: None,
                    outcome: ObservedOutcome::Final,
                    synthetic: false,
                },
            )
            .unwrap();
            let status = status_in_tx(&tx, &scope, &window).unwrap();
            let verified = matches!(reported, "model" | "declared-alias");
            // An unverified reported model is outcome-unknown: the reserved
            // risk is charged conservatively instead of locking the window.
            assert_eq!(
                status.metering_status,
                if verified { "observed" } else { "conservative" },
                "{reported}"
            );
            assert_eq!(status.pending_risk_picos, "0", "{reported}");
            assert_eq!(
                status.used_picos,
                if verified { "1000000" } else { "1000000000000" },
                "{reported}"
            );
        }
    }
    use crate::domain::usage::value_pricing::*;
    #[test]
    fn value_quota_durable_admission_is_atomic_across_connections_and_survives_reopen() {
        let (conn, scope, window, quota, revision) = setup();
        let dir = std::env::temp_dir().join(format!(
            "server-value-concurrency-{}",
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite3");
        conn.backup(rusqlite::DatabaseName::Main, &path, None)
            .unwrap();
        drop(conn);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut tasks = Vec::new();
        for i in 0..2 {
            let (path, scope, window, quota, revision, barrier) = (
                path.clone(),
                scope.clone(),
                window.clone(),
                quota.clone(),
                revision.clone(),
                barrier.clone(),
            );
            tasks.push(std::thread::spawn(move || {
                let mut conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                barrier.wait();
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .unwrap();
                let request = format!("parallel-{i}");
                let result = admit_in_tx(
                    &tx,
                    &Admission {
                        enforce: true,
                        scope: &scope,
                        window: &window,
                        policy_epoch: "epoch",
                        quota: &quota,
                        pricing_revision: &revision,
                        request_id: &request,
                        attempt_id: "1",
                        effective_model: "model",
                        effective_tier: "standard",
                        started_at_ms: 1,
                        // 2,000 tokens at $1/M: a bounded reservation leaves
                        // room for a second concurrent request.
                        estimate: RiskEstimate {
                            input_tokens: 1_000,
                            output_tokens: 1_000,
                        },
                        parallel_limit: 4,
                    },
                );
                match result {
                    Ok(_) => {
                        tx.commit().unwrap();
                        true
                    }
                    Err(error) => panic!("bounded admission rejected: {error}"),
                }
            }));
        }
        assert_eq!(
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            2
        );
        let reopened = Connection::open(&path).unwrap();
        assert_eq!(
            status_in_tx(&reopened, &scope, &window)
                .unwrap()
                .pending_risk_picos,
            "4000000000"
        );
        drop(reopened);
        // Independent restore evidence is outside the DB and cannot be undone
        // by replaying a backup. Only a complete later window is safe again.
        crate::infra::storage::write_json_pretty(
            &dir.join(RESTORE_FENCE),
            &RestoreFence {
                schema_version: 1,
                scopes: BTreeMap::from([(scope.key().unwrap(), 100)]),
            },
        )
        .unwrap();
        assert!(ensure_restore_coverage(&dir, &scope, &window).is_err());
        assert!(ensure_restore_coverage(
            &dir,
            &scope,
            &Window {
                starts_at_ms: DAY_MS,
                ends_at_ms: Some(2 * DAY_MS),
                ..window.clone()
            }
        )
        .is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn value_quota_checkpoints_cover_lifetime_and_partial_days_and_reject_corruption() {
        let (mut conn, scope, window, quota, revision) = setup();
        let lifetime = Window {
            id: "lifetime".into(),
            ends_at_ms: None,
            ..window.clone()
        };
        for (i, at) in [1, DAY_MS, 2 * DAY_MS + 2].into_iter().enumerate() {
            let tx = conn.transaction().unwrap();
            let request = format!("request-{i}");
            admit_in_tx(
                &tx,
                &Admission {
                    enforce: false,
                    scope: &scope,
                    window: &lifetime,
                    policy_epoch: "epoch",
                    quota: &quota,
                    pricing_revision: &revision,
                    request_id: &request,
                    attempt_id: "1",
                    effective_model: "model",
                    effective_tier: "standard",
                    started_at_ms: at,
                    estimate: ESTIMATE,
                    parallel_limit: 4,
                },
            )
            .unwrap();
            settle_in_tx(
                &tx,
                &Observation {
                    scope: &scope,
                    request_id: &request,
                    reported_model: None,
                    attempt_id: "1",
                    usage_revision: 1,
                    effective_model: "model",
                    effective_tier: "standard",
                    tokens: Some(ValueTokens {
                        fresh_input: 10,
                        ..Default::default()
                    }),
                    observed_tier: None,
                    outcome: ObservedOutcome::Final,
                    synthetic: false,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            status_in_tx(&conn, &scope, &lifetime).unwrap().used_picos,
            "30000000"
        );
        let partial = Window {
            id: "anchored".into(),
            starts_at_ms: 2,
            ends_at_ms: Some(2 * DAY_MS + 2),
            metered_from_ms: 0,
        };
        assert_eq!(
            status_in_tx(&conn, &scope, &partial).unwrap().used_picos,
            "10000000"
        );
        conn.execute(
            "UPDATE value_quota_checkpoints SET payload_json='{}' WHERE day_start_ms=0",
            [],
        )
        .unwrap();
        assert!(status_in_tx(&conn, &scope, &lifetime).is_err());
    }
    /// Deterministic capacity fixture, not a throughput/SLO assertion. Run
    /// explicitly; ordinary CI keeps the correctness fixtures small.
    #[test]
    #[ignore = "opt-in 100,000-attempt exact checkpoint read capacity check"]
    fn value_quota_capacity_100000_preserves_exact_amounts_and_uses_scope_index() {
        let (mut conn, scope, window, _, revision) = setup();
        let lifetime = Window {
            id: "lifetime".into(),
            ends_at_ms: None,
            ..window
        };
        let key = scope.key().unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute("WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<100000)
            INSERT INTO value_quota_attempts(request_id,attempt_id,scope,policy_epoch,window_id,started_at_ms,pricing_revision,
            effective_model,effective_tier,frozen_tariff_json,usage_revision,usage_picos,pending_risk_picos,metering_status,watermark,compacted_at_ms)
            SELECT 'capacity-'||v,'1',?1,'epoch','lifetime',v*1000,?2,'model','standard','null',1,'1','0','observed',v,1 FROM n",params![key,revision]).unwrap();
        let days=tx.prepare("SELECT (started_at_ms/86400000)*86400000,count(*),max(watermark) FROM value_quota_attempts WHERE scope=?1 GROUP BY 1").unwrap()
            .query_map([&key],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,u64>(1)?,r.get::<_,u64>(2)?))).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap();
        for (day, count, watermark) in days {
            let checkpoint = Checkpoint {
                amount: count.to_string(),
                risk: "0".into(),
                states: BTreeMap::from([("observed".into(), count)]),
                revisions: BTreeMap::from([(revision.clone(), count)]),
                watermark,
            };
            let json = serde_json::to_string(&checkpoint).unwrap();
            tx.execute(
                "INSERT INTO value_quota_checkpoints VALUES(?1,?2,?3,?4)",
                params![key, day, json, sha256_hex(json.as_bytes())],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        let began = std::time::Instant::now();
        let status = status_in_tx(&conn, &scope, &lifetime).unwrap();
        eprintln!(
            "100,000 attempts / exact lifetime read: {:?}",
            began.elapsed()
        );
        assert_eq!(status.used_picos, "100000");
        assert_eq!(status.usage_watermark, 100000);
        let plan=conn.prepare("EXPLAIN QUERY PLAN SELECT count(*) FROM value_quota_attempts WHERE scope=?1 AND started_at_ms>=?2 AND started_at_ms<?3").unwrap()
            .query_map(params![key,0,i64::MAX],|r|r.get::<_,String>(3)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap().join(" ");
        assert!(plan.contains("INDEX"), "{plan}");
    }
    #[test]
    fn value_quota_compaction_keeps_deduplication_and_accepts_exact_late_revisions() {
        let (mut conn, scope, window, quota, revision) = setup();
        let a = Admission {
            enforce: true,
            scope: &scope,
            window: &window,
            policy_epoch: "epoch",
            quota: &quota,
            pricing_revision: &revision,
            request_id: "compact",
            attempt_id: "1",
            effective_model: "model",
            effective_tier: "standard",
            started_at_ms: 1,
            estimate: ESTIMATE,
            parallel_limit: 4,
        };
        let tx = conn.transaction().unwrap();
        admit_in_tx(&tx, &a).unwrap();
        for usage_revision in 1..=3 {
            settle_in_tx(
                &tx,
                &Observation {
                    scope: &scope,
                    request_id: a.request_id,
                    reported_model: None,
                    attempt_id: "1",
                    usage_revision,
                    effective_model: "model",
                    effective_tier: "standard",
                    tokens: Some(ValueTokens {
                        fresh_input: usage_revision,
                        ..Default::default()
                    }),
                    observed_tier: None,
                    outcome: ObservedOutcome::Final,
                    synthetic: false,
                },
            )
            .unwrap();
        }
        let before = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(compact_in_tx(&tx, DAY_MS, 40 * DAY_MS).unwrap(), 1);
        assert_eq!(
            status_in_tx(&tx, &scope, &window).unwrap().used_picos,
            before.used_picos
        );
        assert_eq!(
            tx.query_row("SELECT count(*) FROM value_quota_revisions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        admit_in_tx(&tx, &a).unwrap();
        let o = Observation {
            scope: &scope,
            request_id: a.request_id,
            reported_model: None,
            attempt_id: "1",
            usage_revision: 4,
            effective_model: "model",
            effective_tier: "standard",
            tokens: Some(ValueTokens {
                fresh_input: 1,
                ..Default::default()
            }),
            observed_tier: None,
            outcome: ObservedOutcome::Final,
            synthetic: false,
        };
        settle_in_tx(&tx, &o).unwrap();
        settle_in_tx(&tx, &o).unwrap();
        assert_eq!(
            status_in_tx(&tx, &scope, &window).unwrap().used_picos,
            "1000000"
        );
        assert!(
            status_in_tx(&tx, &scope, &window).unwrap().usage_watermark > before.usage_watermark
        );
        tx.commit().unwrap();
    }
    #[test]
    fn value_quota_router_ahead_ack_freezes_restored_authority_instead_of_resetting_usage() {
        let (mut conn, scope, window, _, revision) = setup();
        let status = ValueQuotaStatus {
            email: scope.email.clone(),
            policy_epoch: "epoch".into(),
            window_id: window.id.clone(),
            window_starts_at_ms: 0,
            resets_at_ms: window.ends_at_ms,
            metered_from_ms: 0,
            active_pricing_revision: revision,
            usage_pricing_revisions_digest: "digest".into(),
            usage_watermark: 0,
            limit_usd_micros: Some("1000000".into()),
            used_picos: "0".into(),
            observed_picos: "0".into(),
            manual_offset_picos: "0".into(),
            pending_risk_picos: "0".into(),
            metering_status: "observed".into(),
            synced_at_ms: 1,
        };
        let tx = conn.transaction().unwrap();
        assert!(!ack_in_tx(&tx, &scope, &status, 0, 10).unwrap());
        assert!(ack_in_tx(&tx, &scope, &status, 1, 11).unwrap());
        tx.commit().unwrap();
        // Only this (scope, window) is blocked; the next window runs.
        assert_eq!(
            status_in_tx(&conn, &scope, &window)
                .unwrap()
                .metering_status,
            "blocked"
        );
        let next = Window {
            id: "day:1".into(),
            starts_at_ms: DAY_MS,
            ends_at_ms: Some(2 * DAY_MS),
            metered_from_ms: 0,
        };
        assert_eq!(
            status_in_tx(&conn, &scope, &next).unwrap().metering_status,
            "observed"
        );
        // The block remembers the Router's watermark; a later, higher ACK
        // raises it.
        let tx = conn.transaction().unwrap();
        assert!(ack_in_tx(&tx, &scope, &status, 5, 12).unwrap());
        assert_eq!(
            blocked_router_watermark(&tx, &block_key(&scope.key().unwrap(), &window.id)).unwrap(),
            Some(5)
        );
        // `set` reconciles past the Router: the adjustment watermark is
        // above the Router's, so the next projection is accepted instead of
        // re-blocking the (lifetime) window forever.
        let edit = ValueUsageEdit {
            action: ValueUsageEditAction::Set,
            target_picos: Some("0".into()),
            expected_grant_revision: 1,
            window_id: window.id.clone(),
            expected_watermark: 0,
            operation_id: "reconcile-router-ahead".into(),
            reason: "restored database reviewed".into(),
        };
        apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 1,
                edit: &edit,
            }],
            "owner",
            13,
            0,
            &[],
        )
        .unwrap();
        let reconciled = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(reconciled.metering_status, "observed");
        assert!(reconciled.usage_watermark > 5);
        let refreshed = ValueQuotaStatus {
            usage_watermark: reconciled.usage_watermark,
            ..status.clone()
        };
        assert!(!ack_in_tx(&tx, &scope, &refreshed, reconciled.usage_watermark, 14).unwrap());
        tx.commit().unwrap();
    }
    #[test]
    fn value_quota_streaming_revision_zero_and_final_revision_one_never_collide_or_release_early() {
        use crate::domain::providers::model::{AppKind, ProviderType};
        use crate::domain::usage::store::{
            TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata,
        };
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        admit_in_tx(
            &tx,
            &Admission {
                enforce: true,
                scope: &scope,
                window: &window,
                policy_epoch: "epoch",
                quota: &quota,
                pricing_revision: &revision,
                request_id: "stream",
                attempt_id: "1",
                effective_model: "model",
                effective_tier: "standard",
                started_at_ms: 1,
                estimate: ESTIMATE,
                parallel_limit: 4,
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let mut log = UsageLog::new(
            AppKind::Codex,
            "provider".into(),
            "Provider".into(),
            ProviderType::Codex,
            200,
            1,
            UsageModelMetadata {
                model: Some("model".into()),
                ..Default::default()
            },
            TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(1),
                ..Default::default()
            },
        );
        log.apply_context(UsageLogContext {
            request_id: Some("stream".into()),
            share_id: Some(scope.share_id.clone()),
            user_email: Some(scope.email.clone()),
            ..Default::default()
        });
        log.is_streaming = true;
        log.stream_status = Some("streaming".into());
        log.usage_revision = 0;
        let tx = conn.transaction().unwrap();
        settle_usage_log(&tx, &log).unwrap();
        tx.commit().unwrap();
        let state = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(state.used_picos, "11000000");
        assert_eq!(state.metering_status, "pending");
        assert_eq!(state.pending_risk_picos, "999989000000");
        log.stream_status = Some("completed".into());
        log.usage_revision = 1;
        let tx = conn.transaction().unwrap();
        settle_usage_log(&tx, &log).unwrap();
        settle_usage_log(&tx, &log).unwrap();
        tx.commit().unwrap();
        let state = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(state.metering_status, "observed");
        assert_eq!(state.pending_risk_picos, "0");
        log.usage_revision = 2;
        log.output_tokens = Some(0);
        let tx = conn.transaction().unwrap();
        settle_usage_log(&tx, &log).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            status_in_tx(&conn, &scope, &window).unwrap().used_picos,
            "10000000"
        );
        // A broken USD checkpoint must not discard real usage/Token evidence
        // or leak partially applied monetary revisions out of its savepoint.
        conn.execute(
            "UPDATE value_quota_checkpoints SET payload_sha256='corrupt'",
            [],
        )
        .unwrap();
        log.usage_revision = 3;
        log.output_tokens = Some(2);
        let tx = conn.transaction().unwrap();
        super::super::upsert_usage_row(&tx, &log).unwrap();
        super::super::quota::upsert_log(&tx, &log).unwrap();
        assert!(!settle_usage_log_isolated(&tx, &log).unwrap());
        tx.commit().unwrap();
        let kept: (i64, String) = conn.query_row(
            "SELECT usage_revision,usage_picos FROM value_quota_attempts WHERE request_id='stream'",
            [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(kept, (3, "10000000".into()));
        assert_eq!(
            conn.query_row(
                "SELECT usage_revision FROM usage_records WHERE request_id='stream'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        assert!(status_in_tx(&conn, &scope, &window).is_err());
        assert_eq!(
            conn.query_row(
                "SELECT value FROM meta WHERE key=?1",
                [block_key(&scope.key().unwrap(), &window.id)],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "settlement_requires_reconcile"
        );
        // The owner reconcile (`set`) rebuilds the scope's checkpoints and
        // clears only this window's block, in one audited transaction.
        let tx = conn.transaction().unwrap();
        apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 1,
                edit: &ValueUsageEdit {
                    action: ValueUsageEditAction::Set,
                    target_picos: Some("10000000".into()),
                    expected_grant_revision: 1,
                    window_id: window.id.clone(),
                    expected_watermark: 0,
                    operation_id: "reconcile-1".into(),
                    reason: "reviewed".into(),
                },
            }],
            "owner",
            10,
            0,
            &[],
        )
        .unwrap();
        tx.commit().unwrap();
        let state = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(state.metering_status, "observed");
        assert_eq!(state.used_picos, "10000000");
    }
    #[test]
    fn value_quota_manual_offsets_are_atomic_idempotent_and_never_delete_observed_usage() {
        let (mut conn, scope, window, _, _) = setup();
        let edit = ValueUsageEdit {
            action: ValueUsageEditAction::Set,
            target_picos: Some("123456789".into()),
            expected_grant_revision: 7,
            window_id: window.id.clone(),
            expected_watermark: 0,
            operation_id: "manual-1".into(),
            reason: "Reviewed adjustment".into(),
        };
        let tx = conn.transaction().unwrap();
        apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 7,
                edit: &edit,
            }],
            "owner",
            10,
            0,
            &[],
        )
        .unwrap();
        tx.commit().unwrap();
        let current = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(current.used_picos, "123456789");
        let tx = conn.transaction().unwrap();
        apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 7,
                edit: &edit,
            }],
            "owner",
            11,
            0,
            &[],
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            status_in_tx(&conn, &scope, &window)
                .unwrap()
                .usage_watermark,
            current.usage_watermark
        );
        let clear = ValueUsageEdit {
            action: ValueUsageEditAction::Clear,
            target_picos: None,
            expected_watermark: current.usage_watermark,
            operation_id: "manual-2".into(),
            ..edit.clone()
        };
        let other = Scope {
            email: "another@example.test".into(),
            ..scope.clone()
        };
        let tx = conn.transaction().unwrap();
        assert!(apply_adjustments_in_tx(
            &tx,
            &[
                Adjustment {
                    scope: &scope,
                    window: &window,
                    grant_revision: 7,
                    edit: &clear
                },
                Adjustment {
                    scope: &other,
                    window: &window,
                    grant_revision: 7,
                    edit: &edit
                }
            ],
            "owner",
            12,
            0,
            &[],
        )
        .is_err());
        drop(tx);
        assert_eq!(
            status_in_tx(&conn, &scope, &window).unwrap().used_picos,
            "123456789"
        );
        let tx = conn.transaction().unwrap();
        apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 7,
                edit: &clear,
            }],
            "owner",
            13,
            0,
            &[],
        )
        .unwrap();
        tx.commit().unwrap();
        assert_eq!(
            status_in_tx(&conn, &scope, &window).unwrap().used_picos,
            "0"
        );
    }
    #[test]
    fn value_quota_adjustments_cannot_lower_observed_consumption_or_panic_on_corrupt_counts() {
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        admit_in_tx(
            &tx,
            &Admission {
                enforce: true,
                scope: &scope,
                window: &window,
                policy_epoch: "epoch",
                quota: &quota,
                pricing_revision: &revision,
                request_id: "used",
                attempt_id: "1",
                effective_model: "model",
                effective_tier: "standard",
                started_at_ms: 1,
                estimate: ESTIMATE,
                parallel_limit: 4,
            },
        )
        .unwrap();
        settle_in_tx(
            &tx,
            &Observation {
                request_id: "used",
                reported_model: None,
                attempt_id: "1",
                scope: &scope,
                usage_revision: 1,
                effective_model: "model",
                effective_tier: "standard",
                tokens: Some(ValueTokens {
                    fresh_input: 1,
                    ..Default::default()
                }),
                observed_tier: None,
                outcome: ObservedOutcome::Final,
                synthetic: false,
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let before = status_in_tx(&conn, &scope, &window).unwrap();
        let edit = ValueUsageEdit {
            action: ValueUsageEditAction::Set,
            target_picos: Some("0".into()),
            expected_grant_revision: 1,
            window_id: window.id.clone(),
            expected_watermark: before.usage_watermark,
            operation_id: "cannot-erase".into(),
            reason: "reviewed".into(),
        };
        let tx = conn.transaction().unwrap();
        assert!(apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 1,
                edit: &edit
            }],
            "owner",
            2,
            0,
            &[],
        )
        .is_err());
        drop(tx);
        assert_eq!(
            status_in_tx(&conn, &scope, &window).unwrap().used_picos,
            before.used_picos
        );
        let checkpoint = Checkpoint {
            states: BTreeMap::from([("observed".into(), u64::MAX), ("gap".into(), 1)]),
            ..Checkpoint::empty()
        };
        assert!(checkpoint.count().is_err());
    }
    fn setup() -> (Connection, Scope, Window, ValueQuota, String) {
        let mut conn = Connection::open_in_memory().unwrap();
        schema::initialize_schema(&conn).unwrap();
        let scope = Scope {
            router_url: "https://router.test".into(),
            installation_id: "install".into(),
            share_id: "share".into(),
            email: "user@example.com".into(),
        };
        let window = Window {
            id: "day:0".into(),
            starts_at_ms: 0,
            ends_at_ms: Some(86400000),
            metered_from_ms: 0,
        };
        let rates = ExecutionRates {
            input: Some("1000000".into()),
            output: Some("1000000".into()),
            cache_read: None,
            cache_write5m: None,
            cache_write1h: None,
            cache_write_policy: CacheWritePolicy::Unavailable,
        };
        let snapshot = PricingSnapshot {
            schema_version: 1,
            tariff_policy_id: TARIFF_POLICY.into(),
            currency: "USD".into(),
            exact_aliases: BTreeMap::new(),
            models: BTreeMap::from([(
                "model".into(),
                ExecutionModelPrice {
                    context_input_basis: ContextInputBasis::RawInput,
                    long_context_threshold: None,
                    long_context_inclusive: false,
                    rates: BTreeMap::from([("standard/base".into(), rates)]),
                },
            )]),
        };
        let revision = snapshot.revision().unwrap();
        let tx = conn.transaction().unwrap();
        install_in_tx(&tx, &scope, &revision, &snapshot, 0).unwrap();
        tx.commit().unwrap();
        conn.execute(
            "INSERT INTO value_quota_coverage VALUES(?1,0)",
            [scope.key().unwrap()],
        )
        .unwrap();
        let quota = ValueQuota {
            limit_usd_micros: "1000000".into(),
            tariff_policy_id: TARIFF_POLICY.into(),
        };
        (conn, scope, window, quota, revision)
    }
    #[test]
    fn durable_risk_and_revision_idempotency_survive_policy_changes() {
        let (mut conn, scope, window, quota, revision) = setup();
        let a = Admission {
            enforce: true,
            scope: &scope,
            window: &window,
            policy_epoch: "epoch-a",
            quota: &quota,
            pricing_revision: &revision,
            request_id: "r1",
            attempt_id: "0",
            effective_model: "model",
            effective_tier: "standard",
            started_at_ms: 1,
            estimate: ESTIMATE,
            parallel_limit: 4,
        };
        {
            let tx = conn.transaction().unwrap();
            admit_in_tx(&tx, &a).unwrap();
            tx.commit().unwrap();
        }
        {
            let tx = conn.transaction().unwrap();
            assert!(admit_in_tx(
                &tx,
                &Admission {
                    request_id: "r2",
                    policy_epoch: "epoch-b",
                    ..a.clone()
                }
            )
            .is_err());
        }
        let o = Observation {
            request_id: "r1",
            reported_model: None,
            attempt_id: "0",
            scope: &scope,
            usage_revision: 1,
            effective_model: "model",
            effective_tier: "standard",
            tokens: Some(ValueTokens {
                fresh_input: 1_100_000,
                ..Default::default()
            }),
            observed_tier: None,
            outcome: ObservedOutcome::Final,
            synthetic: false,
        };
        {
            let tx = conn.transaction().unwrap();
            settle_in_tx(&tx, &o).unwrap();
            settle_in_tx(&tx, &o).unwrap();
            tx.commit().unwrap();
        }
        let view = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(view.used_picos, "1100000000000");
        assert_eq!(view.pending_risk_picos, "0");
        {
            // A retry after a policy edit is the same attempt identity.
            let tx = conn.transaction().unwrap();
            admit_in_tx(
                &tx,
                &Admission {
                    policy_epoch: "epoch-b",
                    ..a.clone()
                },
            )
            .unwrap();
            let exhausted = admit_in_tx(
                &tx,
                &Admission {
                    request_id: "r3",
                    ..a.clone()
                },
            )
            .unwrap_err();
            assert_eq!(
                exhausted.downcast_ref::<ValueQuotaRejection>(),
                Some(&ValueQuotaRejection {
                    kind: ValueQuotaRejectionKind::Exhausted,
                    resets_at_ms: window.ends_at_ms,
                })
            );
        }
        {
            let tx = conn.transaction().unwrap();
            assert!(admit_in_tx(
                &tx,
                &Admission {
                    request_id: "r2",
                    policy_epoch: "epoch-b",
                    ..a.clone()
                }
            )
            .is_err());
        }
        {
            let tx = conn.transaction().unwrap();
            settle_in_tx(
                &tx,
                &Observation {
                    usage_revision: 2,
                    tokens: Some(ValueTokens {
                        fresh_input: 10,
                        ..Default::default()
                    }),
                    ..o
                },
            )
            .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            status_in_tx(&conn, &scope, &window).unwrap().used_picos,
            "10000000"
        );
    }
    #[test]
    fn missing_usage_charges_reserved_risk_conservatively_and_cross_scope_reuse_fails() {
        let (mut conn, scope, window, quota, revision) = setup();
        let a = Admission {
            enforce: true,
            scope: &scope,
            window: &window,
            policy_epoch: "epoch",
            quota: &quota,
            pricing_revision: &revision,
            request_id: "r1",
            attempt_id: "0",
            effective_model: "model",
            effective_tier: "standard",
            started_at_ms: 1,
            estimate: ESTIMATE,
            parallel_limit: 4,
        };
        let tx = conn.transaction().unwrap();
        admit_in_tx(&tx, &a).unwrap();
        settle_in_tx(
            &tx,
            &Observation {
                request_id: "r1",
                reported_model: None,
                attempt_id: "0",
                scope: &scope,
                usage_revision: 1,
                effective_model: "model",
                effective_tier: "standard",
                tokens: None,
                observed_tier: None,
                outcome: ObservedOutcome::Unknown,
                synthetic: false,
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let view = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(view.metering_status, "conservative");
        assert_eq!(view.pending_risk_picos, "0");
        assert_eq!(view.used_picos, "1000000000000");
        let other = Scope {
            email: "other@example.com".into(),
            ..scope.clone()
        };
        let tx = conn.transaction().unwrap();
        assert!(admit_in_tx(&tx, &Admission { scope: &other, ..a }).is_err());
    }
    #[test]
    fn report_cleanup_and_old_window_revisions_do_not_reset_current_usage() {
        let (mut conn, scope, window, quota, revision) = setup();
        let a = Admission {
            enforce: true,
            scope: &scope,
            window: &window,
            policy_epoch: "epoch",
            quota: &quota,
            pricing_revision: &revision,
            request_id: "old",
            attempt_id: "0",
            effective_model: "model",
            effective_tier: "standard",
            started_at_ms: 1,
            estimate: ESTIMATE,
            parallel_limit: 4,
        };
        let tx = conn.transaction().unwrap();
        admit_in_tx(&tx, &a).unwrap();
        tx.commit().unwrap();
        conn.execute("DELETE FROM usage_records", []).unwrap();
        let next = Window {
            id: "day:1".into(),
            starts_at_ms: 86400000,
            ends_at_ms: Some(172800000),
            ..window.clone()
        };
        assert_eq!(status_in_tx(&conn, &scope, &next).unwrap().used_picos, "0");
        assert_ne!(
            status_in_tx(&conn, &scope, &window)
                .unwrap()
                .pending_risk_picos,
            "0"
        );
        conn.execute("UPDATE value_quota_coverage SET collected_from_ms=2", [])
            .unwrap();
        assert!(status_in_tx(&conn, &scope, &window).is_err());
    }

    #[test]
    fn value_quota_bounded_reservation_enforces_parallel_cap_with_a_distinct_busy_code() {
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        let base = Admission {
            enforce: true,
            scope: &scope,
            window: &window,
            policy_epoch: "epoch",
            quota: &quota,
            pricing_revision: &revision,
            request_id: "p0",
            attempt_id: "1",
            effective_model: "model",
            effective_tier: "standard",
            started_at_ms: 1,
            estimate: ESTIMATE,
            parallel_limit: 2,
        };
        let estimate = RiskEstimate {
            input_tokens: 1_000,
            output_tokens: 1_000,
        };
        for id in ["p0", "p1"] {
            let admitted = admit_in_tx(
                &tx,
                &Admission {
                    request_id: id,
                    estimate,
                    ..base.clone()
                },
            )
            .unwrap();
            assert_eq!(admitted.reserved_picos, 2_000_000_000);
        }
        let busy = admit_in_tx(
            &tx,
            &Admission {
                request_id: "p2",
                estimate,
                ..base.clone()
            },
        )
        .unwrap_err();
        assert_eq!(
            busy.downcast_ref::<ValueQuotaRejection>().unwrap().kind,
            ValueQuotaRejectionKind::Busy
        );
        // Reservations alone filling the limit is temporary: busy, not exhausted.
        assert!(settle_outcome_in_tx(&tx, "p1", "1", AttemptOutcome::NotSent, true).unwrap());
        admit_in_tx(
            &tx,
            &Admission {
                request_id: "big",
                parallel_limit: 4,
                ..base.clone()
            },
        )
        .unwrap();
        let full = admit_in_tx(
            &tx,
            &Admission {
                request_id: "p3",
                estimate,
                parallel_limit: 4,
                ..base.clone()
            },
        )
        .unwrap_err();
        assert_eq!(
            full.downcast_ref::<ValueQuotaRejection>().unwrap().kind,
            ValueQuotaRejectionKind::Busy
        );
    }
    #[test]
    fn value_quota_outcomes_release_or_charge_once_and_yield_to_genuine_usage() {
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        let a = Admission {
            enforce: true,
            scope: &scope,
            window: &window,
            policy_epoch: "epoch",
            quota: &quota,
            pricing_revision: &revision,
            request_id: "not-sent",
            attempt_id: "1",
            effective_model: "model",
            effective_tier: "standard",
            started_at_ms: 1,
            estimate: RiskEstimate {
                input_tokens: 10,
                output_tokens: 10,
            },
            parallel_limit: 4,
        };
        admit_in_tx(&tx, &a).unwrap();
        assert!(settle_outcome_in_tx(&tx, "not-sent", "1", AttemptOutcome::NotSent, true).unwrap());
        assert!(
            !settle_outcome_in_tx(&tx, "not-sent", "1", AttemptOutcome::Unknown, true).unwrap()
        );
        let view = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(
            (view.used_picos.as_str(), view.pending_risk_picos.as_str()),
            ("0", "0")
        );
        assert_eq!(view.metering_status, "observed");
        admit_in_tx(
            &tx,
            &Admission {
                request_id: "unknown",
                ..a.clone()
            },
        )
        .unwrap();
        assert!(settle_outcome_in_tx(&tx, "unknown", "1", AttemptOutcome::Unknown, true).unwrap());
        let view = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(view.used_picos, "20000000");
        assert_eq!(view.metering_status, "conservative");
        // A late genuine usage log replaces the synthetic charge exactly.
        assert!(settle_in_tx(
            &tx,
            &Observation {
                request_id: "unknown",
                attempt_id: "1",
                scope: &scope,
                usage_revision: 1,
                effective_model: "model",
                effective_tier: "standard",
                reported_model: None,
                observed_tier: None,
                tokens: Some(ValueTokens {
                    fresh_input: 3,
                    output: 2,
                    ..Default::default()
                }),
                outcome: ObservedOutcome::Final,
                synthetic: false,
            },
        )
        .unwrap());
        let view = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(view.used_picos, "5000000");
        assert_eq!(view.metering_status, "observed");
    }
    fn log_for(scope: &Scope, request: &str) -> crate::domain::usage::store::UsageLog {
        use crate::domain::providers::model::{AppKind, ProviderType};
        use crate::domain::usage::store::{
            TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata,
        };
        let mut log = UsageLog::new(
            AppKind::Claude,
            "provider".into(),
            "Provider".into(),
            ProviderType::Claude,
            200,
            1,
            UsageModelMetadata {
                model: Some("model".into()),
                ..Default::default()
            },
            TokenUsage::default(),
        );
        log.apply_context(UsageLogContext {
            request_id: Some(request.into()),
            share_id: Some(scope.share_id.clone()),
            user_email: Some(scope.email.clone()),
            ..Default::default()
        });
        log
    }
    #[test]
    fn value_quota_log_outcomes_classify_errors_interruptions_and_retries() {
        let (_, scope, _, _, _) = setup();
        let meta = AttemptMeta {
            input_tokens: 100,
            output_tokens: 50,
            reserved_picos: "1".into(),
        };
        let mut error = log_for(&scope, "e");
        error.status_code = 529;
        assert_eq!(
            classify_log(&error, "model", "standard", Some(&meta), false).0,
            ObservedOutcome::Free
        );
        let mut cut = log_for(&scope, "i");
        cut.is_streaming = true;
        cut.stream_status = Some("client_cancelled".into());
        cut.input_tokens = Some(40);
        cut.value_streamed_text_bytes = Some(3_000);
        let (outcome, tokens, _) = classify_log(&cut, "model", "standard", Some(&meta), false);
        assert_eq!(outcome, ObservedOutcome::Interrupted);
        assert_eq!(
            tokens.map(|t| (t.fresh_input, t.output)),
            Some((40, 50)),
            "output estimate is capped by the request's output ceiling"
        );
        cut.input_tokens = None;
        cut.value_streamed_text_bytes = Some(30);
        let (_, tokens, _) = classify_log(&cut, "model", "standard", Some(&meta), false);
        assert_eq!(tokens.map(|t| (t.fresh_input, t.output)), Some((100, 10)));
        let mut retried = log_for(&scope, "r");
        retried.input_tokens = Some(1);
        retried.output_tokens = Some(1);
        retried.attempt_count = 2;
        retried.usage_state = crate::domain::usage::store::UsageState::Observed;
        assert_eq!(
            classify_log(&retried, "model", "standard", None, false).0,
            ObservedOutcome::FinalConservative
        );
        retried.attempt_count = 1;
        retried.actual_model = Some("other".into());
        assert_eq!(
            classify_log(&retried, "model", "standard", None, false).0,
            ObservedOutcome::Unknown
        );
        let mut pending = log_for(&scope, "p");
        pending.is_streaming = true;
        pending.stream_status = Some("pending".into());
        assert_eq!(
            classify_log(&pending, "model", "standard", Some(&meta), false).0,
            ObservedOutcome::InProgress
        );
        assert_eq!(
            classify_log(&pending, "model", "standard", Some(&meta), true).0,
            ObservedOutcome::Interrupted
        );
    }
    #[test]
    fn value_quota_sweeper_settles_stale_attempts_from_logs_or_reserved_risk() {
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        for request in ["orphan", "stream"] {
            admit_in_tx(
                &tx,
                &Admission {
                    enforce: true,
                    scope: &scope,
                    window: &window,
                    policy_epoch: "epoch",
                    quota: &quota,
                    pricing_revision: &revision,
                    request_id: request,
                    attempt_id: "1",
                    effective_model: "model",
                    effective_tier: "standard",
                    started_at_ms: 1,
                    estimate: RiskEstimate {
                        input_tokens: 10,
                        output_tokens: 10,
                    },
                    parallel_limit: 4,
                },
            )
            .unwrap();
        }
        let mut log = log_for(&scope, "stream");
        log.is_streaming = true;
        log.stream_status = Some("pending".into());
        log.input_tokens = Some(4);
        log.value_streamed_text_bytes = Some(9);
        super::super::upsert_usage_row(&tx, &log).unwrap();
        settle_usage_log(&tx, &log).unwrap();
        tx.commit().unwrap();
        // The sweeper entry point needs a committed authority directory;
        // exercise its per-attempt classification on the fixture connection.
        let tx = conn.transaction().unwrap();
        let mut from_log = 0;
        let mut unknown = 0;
        for request in ["orphan", "stream"] {
            let stored: Option<crate::domain::usage::store::UsageLog> = tx
                .query_row(
                    "SELECT payload_json FROM usage_records WHERE request_id=?1",
                    [request],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .unwrap()
                .and_then(|json| serde_json::from_str(&json).ok());
            match stored {
                Some(stored) => {
                    settle_log_in_tx(&tx, &stored, true).unwrap();
                    from_log += 1;
                }
                None => {
                    assert!(settle_outcome_in_tx(
                        &tx,
                        request,
                        "1",
                        AttemptOutcome::Unknown,
                        false
                    )
                    .unwrap());
                    unknown += 1;
                }
            }
        }
        assert_eq!((from_log, unknown), (1, 1));
        let view = status_in_tx(&tx, &scope, &window).unwrap();
        // orphan: reserved 20 tokens; stream: 4 input + ceil(9/3) output.
        assert_eq!(view.used_picos, "27000000");
        assert_eq!(view.pending_risk_picos, "0");
        assert_eq!(view.metering_status, "conservative");
    }
    #[test]
    fn value_quota_usage_log_settles_only_its_own_attempt() {
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        admit_in_tx(
            &tx,
            &Admission {
                enforce: true,
                scope: &scope,
                window: &window,
                policy_epoch: "epoch",
                quota: &quota,
                pricing_revision: &revision,
                request_id: "only-first",
                attempt_id: "1",
                effective_model: "model",
                effective_tier: "standard",
                started_at_ms: 1,
                estimate: RiskEstimate {
                    input_tokens: 10,
                    output_tokens: 10,
                },
                parallel_limit: 4,
            },
        )
        .unwrap();
        // A free (error) log of attempt 2 must not settle sent attempt 1.
        let mut log = log_for(&scope, "only-first");
        log.attempt_count = 2;
        log.status_code = 529;
        assert!(!settle_log_in_tx(&tx, &log, false).unwrap());
        let view = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(view.metering_status, "pending");
        assert_eq!(view.used_picos, "0");
        assert_eq!(view.pending_risk_picos, "20000000");
        tx.commit().unwrap();
    }
    #[test]
    fn value_quota_reconcile_leaves_young_pending_attempts_reserved() {
        let (mut conn, scope, window, quota, revision) = setup();
        let tx = conn.transaction().unwrap();
        for (request, started) in [("stuck", 1), ("live", 50_000)] {
            admit_in_tx(
                &tx,
                &Admission {
                    enforce: true,
                    scope: &scope,
                    window: &window,
                    policy_epoch: "epoch",
                    quota: &quota,
                    pricing_revision: &revision,
                    request_id: request,
                    attempt_id: "1",
                    effective_model: "model",
                    effective_tier: "standard",
                    started_at_ms: started,
                    estimate: RiskEstimate {
                        input_tokens: 10,
                        output_tokens: 10,
                    },
                    parallel_limit: 4,
                },
            )
            .unwrap();
        }
        let edit = ValueUsageEdit {
            action: ValueUsageEditAction::Set,
            target_picos: Some("20000000".into()),
            expected_grant_revision: 1,
            window_id: window.id.clone(),
            expected_watermark: 0,
            operation_id: "reconcile-young".into(),
            reason: "reviewed".into(),
        };
        // Stale bound (max request timeout + margin) between the two starts.
        apply_adjustments_in_tx(
            &tx,
            &[Adjustment {
                scope: &scope,
                window: &window,
                grant_revision: 1,
                edit: &edit,
            }],
            "owner",
            60_000,
            10_000,
            &[],
        )
        .unwrap();
        let state = |request: &str| -> String {
            tx.query_row(
                "SELECT metering_status FROM value_quota_attempts WHERE request_id=?1",
                [request],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(state("stuck"), "conservative");
        assert_eq!(state("live"), "pending");
        let view = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(view.pending_risk_picos, "20000000");
        tx.commit().unwrap();
    }
    #[test]
    fn value_quota_sweeper_isolates_failing_rows_and_prefers_pending_over_legacy() {
        let (mut conn, scope, window, quota, revision) = setup();
        let next = Window {
            id: "day:1".into(),
            starts_at_ms: DAY_MS,
            ends_at_ms: Some(2 * DAY_MS),
            metered_from_ms: 0,
        };
        let tx = conn.transaction().unwrap();
        for (request, w, started) in [
            ("healthy", &window, 1),
            ("corrupt", &next, DAY_MS + 1),
            ("legacy", &window, 0),
        ] {
            admit_in_tx(
                &tx,
                &Admission {
                    enforce: true,
                    scope: &scope,
                    window: w,
                    policy_epoch: "epoch",
                    quota: &quota,
                    pricing_revision: &revision,
                    request_id: request,
                    attempt_id: "1",
                    effective_model: "model",
                    effective_tier: "standard",
                    started_at_ms: started,
                    estimate: RiskEstimate {
                        input_tokens: 10,
                        output_tokens: 10,
                    },
                    parallel_limit: 4,
                },
            )
            .unwrap();
        }
        // A legacy shadow row ('unavailable', no reservation).
        tx.execute(
            "UPDATE value_quota_attempts SET metering_status='unavailable' WHERE request_id='legacy'",
            [],
        )
        .unwrap();
        rebuild_checkpoints_in_tx(&tx, &scope.key().unwrap()).unwrap();
        tx.commit().unwrap();
        // Pending rows come first even though the legacy row is older.
        let candidates = sweep_candidates(&conn, 10 * DAY_MS, 2).unwrap();
        assert_eq!(
            candidates,
            vec![
                ("healthy".to_string(), "1".to_string()),
                ("corrupt".to_string(), "1".to_string())
            ]
        );
        // Corrupt only the second day's checkpoint.
        conn.execute(
            "UPDATE value_quota_checkpoints SET payload_sha256='bad' WHERE day_start_ms=?1",
            [DAY_MS],
        )
        .unwrap();
        let candidates = sweep_candidates(&conn, 10 * DAY_MS, 10).unwrap();
        assert_eq!(candidates.len(), 3);
        let tx = conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .unwrap();
        let mut report = SweepReport::default();
        sweep_batch_in_tx(&tx, &candidates, &mut report).unwrap();
        tx.commit().unwrap();
        assert_eq!(report.outcome_unknown, 2);
        assert_eq!(report.blocked, 1);
        let state = |request: &str| -> String {
            conn.query_row(
                "SELECT metering_status FROM value_quota_attempts WHERE request_id=?1",
                [request],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(state("healthy"), "conservative");
        assert_eq!(state("legacy"), "conservative");
        assert_eq!(state("corrupt"), "pending");
        assert!(window_blocked(&conn, &scope.key().unwrap(), &next.id).unwrap());
        assert!(!window_blocked(&conn, &scope.key().unwrap(), &window.id).unwrap());
    }
}
