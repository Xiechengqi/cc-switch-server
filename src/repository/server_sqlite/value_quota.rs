//! Durable Share USD-equivalent metering, independent of report retention.
//! Callers enter through ServerStateInner. Every admission is an IMMEDIATE
//! transaction; an upstream request must never start before this commit.
use super::*;
use crate::domain::usage::value_pricing::{
    unsigned_decimal, ExecutionModelPrice, PricingSnapshot, ValueQuota, ValueTokens,
};

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
const DEGRADED_META: &str = "value_quota_degraded_v1";
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
fn ensure_restore_coverage(dir: &Path, scope: &Scope, window: &Window) -> anyhow::Result<()> {
    ensure!(
        read_restore_fence(dir)?
            .scopes
            .get(&scope.key()?)
            .is_none_or(|at| window.metered_start_ms() >= *at),
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
    fn metering(&self) -> &'static str {
        if self.states.contains_key("unavailable") || self.states.contains_key("gap") {
            "unavailable"
        } else if self.states.contains_key("pending") {
            "pending"
        } else if self.states.contains_key("conservative") {
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
    let json = serde_json::to_string(&checkpoint)?;
    tx.execute("INSERT INTO value_quota_checkpoints VALUES(?1,?2,?3,?4) ON CONFLICT(scope,day_start_ms) DO UPDATE SET payload_json=excluded.payload_json,payload_sha256=excluded.payload_sha256",
        params![scope,day,json,sha256_hex(json.as_bytes())])?;
    Ok(())
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
pub(crate) fn active_snapshot(
    config_dir: &Path,
    scope: &Scope,
) -> anyhow::Result<(String, PricingSnapshot)> {
    assert_committed(config_dir)?;
    let conn = open_database(&database_path(config_dir), true)?;
    let (revision,json): (String,String)=conn.query_row(
        "SELECT s.revision,s.payload_json FROM value_pricing_snapshots s JOIN meta m ON m.value=s.revision
         WHERE m.key=?1 AND s.router_url=?2 AND s.installation_id=?3",
        params![format!("value_tariff:{}:{}",scope.router_url,scope.installation_id),scope.router_url,scope.installation_id],
        |r|Ok((r.get(0)?,r.get(1)?))).context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
    let snapshot: PricingSnapshot = serde_json::from_str(&json)?;
    ensure!(
        snapshot.revision()? == revision,
        "VALUE_QUOTA_SNAPSHOT_CORRUPT"
    );
    Ok((revision, snapshot))
}
pub(crate) fn record_gap(
    config_dir: &Path,
    scope: &Scope,
    request_id: &str,
    started_at_ms: i64,
) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    let mut conn = open_database(&database_path(config_dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let watermark = next_watermark(&tx)?;
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO value_quota_gaps VALUES(?1,?2,?3,'unverified_surface_or_usage',?4)",
        params![scope.key()?, request_id, started_at_ms, watermark],
    )?;
    if inserted > 0 {
        update_checkpoint(
            &tx,
            &scope.key()?,
            started_at_ms,
            None,
            Some(Contribution {
                amount: "0",
                risk: "0",
                state: "gap",
                revision: None,
                watermark: u64::try_from(watermark)?,
            }),
        )?;
    }
    tx.commit()?;
    Ok(())
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
    // Re-authorizing the same identity must not create a new free lifetime.
    Ok(connection.query_row(
        "SELECT collected_from_ms FROM value_quota_coverage WHERE scope=?1",
        [scope.key()?],
        |r| r.get(0),
    )?)
}
/// Read-only counterpart used by activation previews. Preview must not start
/// collection, create an epoch, or repair missing history.
pub(crate) fn coverage_start(config_dir: &Path, scope: &Scope) -> anyhow::Result<Option<i64>> {
    assert_committed(config_dir)?;
    let connection = open_database(&database_path(config_dir), true)?;
    Ok(connection
        .query_row(
            "SELECT collected_from_ms FROM value_quota_coverage WHERE scope=?1",
            [scope.key()?],
            |r| r.get(0),
        )
        .optional()?)
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
                .is_none_or(|at| now.saturating_sub(at) >= 60000),
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
            tx.execute("INSERT INTO meta VALUES(?1,'router_watermark_ahead_of_authority') ON CONFLICT(key) DO NOTHING",[format!("value_quota_blocked:{}",scope.key()?)])?;
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
fn status_in_tx(
    connection: &Connection,
    scope: &Scope,
    window: &Window,
) -> anyhow::Result<QuotaStatus> {
    window.validate()?;
    let key = scope.key()?;
    let blocked: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM meta WHERE key IN (?1,?2))",
        params![format!("value_quota_blocked:{key}"), DEGRADED_META],
        |r| r.get(0),
    )?;
    ensure!(!blocked, "VALUE_QUOTA_RESTORE_RECONCILIATION_REQUIRED");
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
        metering_status: aggregate.metering().into(),
        usage_watermark: watermark,
        usage_pricing_revisions_digest: sha256_hex(&serde_json::to_vec(
            &aggregate
                .revisions
                .keys()
                .collect::<std::collections::BTreeSet<_>>(),
        )?),
    })
}
pub(crate) fn status(
    config_dir: &Path,
    scope: &Scope,
    window: &Window,
) -> anyhow::Result<QuotaStatus> {
    assert_committed(config_dir)?;
    ensure_restore_coverage(config_dir, scope, window)?;
    // Checkpoint payloads, indexed identity counts and manual offsets must be
    // read from one SQLite snapshot. Autocommit SELECTs could otherwise see
    // different concurrent revisions and report a spurious corrupt ledger.
    let mut connection = open_database(&database_path(config_dir), true)?;
    let tx = connection.transaction()?;
    let status = status_in_tx(&tx, scope, window)?;
    tx.commit()?;
    Ok(status)
}
pub(crate) fn admit(config_dir: &Path, input: &Admission<'_>) -> anyhow::Result<QuotaStatus> {
    assert_committed(config_dir)?;
    if input.enforce {
        ensure_restore_coverage(config_dir, input.scope, input.window)?;
    }
    let mut connection = open_database(&database_path(config_dir), false)?;
    let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let result = admit_in_tx(&tx, input)?;
    tx.commit()?;
    Ok(result)
}
fn admit_in_tx(tx: &Transaction<'_>, a: &Admission<'_>) -> anyhow::Result<QuotaStatus> {
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
    let snapshot: String = tx.query_row(
        "SELECT payload_json FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3",
        params![a.scope.router_url,a.scope.installation_id,a.pricing_revision], |r| r.get(0))
        .context("VALUE_QUOTA_SNAPSHOT_UNAVAILABLE")?;
    let snapshot: PricingSnapshot = serde_json::from_str(&snapshot)?;
    ensure!(
        snapshot.revision()?.as_str() == a.pricing_revision,
        "VALUE_QUOTA_SNAPSHOT_CORRUPT"
    );
    let model = snapshot.model(a.effective_model)?;
    ensure!(model.supports_tier(a.effective_tier), "price_unavailable");
    let frozen = serde_json::to_string(model)?;
    let existing: Option<(String,String,String,String,String,String,i64)> = tx.query_row(
        "SELECT scope,policy_epoch,pricing_revision,effective_model,effective_tier,window_id,started_at_ms
         FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
        params![a.request_id,a.attempt_id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).optional()?;
    if let Some(old) = existing {
        ensure!(
            old == (
                key,
                a.policy_epoch.into(),
                a.pricing_revision.into(),
                a.effective_model.into(),
                a.effective_tier.into(),
                a.window.id.clone(),
                a.started_at_ms
            ),
            "VALUE_QUOTA_REQUEST_IDENTITY_CONFLICT"
        );
        return status_in_tx(tx, a.scope, a.window);
    }
    let current = status_in_tx(tx, a.scope, a.window)?;
    ensure!(
        !a.enforce || current.metering_status != "unavailable",
        "VALUE_QUOTA_USAGE_UNAVAILABLE"
    );
    let spent = unsigned_decimal(&current.used_picos)?;
    let pending = unsigned_decimal(&current.pending_risk_picos)?;
    let limit = a.quota.limit_picos()?;
    let available = limit
        .checked_sub(spent)
        .and_then(|v| v.checked_sub(pending))
        .context("value quota arithmetic overflow")?;
    ensure!(!a.enforce || available > 0, "VALUE_QUOTA_EXHAUSTED");
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM value_quota_attempts WHERE scope=?1 AND metering_status IN ('pending','unavailable')
         AND started_at_ms>=?2 AND (?3 IS NULL OR started_at_ms<?3)",
        params![key,a.window.metered_start_ms(),a.window.ends_at_ms], |r| r.get(0))?;
    ensure!(!a.enforce || count < 4, "VALUE_QUOTA_PARALLEL_LIMIT");
    // No trusted output ceiling: reserve all remaining capacity. This is
    // released only with proved final usage, never by dropping an HTTP guard.
    let watermark = next_watermark(tx)?;
    tx.execute(
        "INSERT INTO value_quota_attempts(request_id,attempt_id,scope,policy_epoch,window_id,started_at_ms,
         pricing_revision,effective_model,effective_tier,frozen_tariff_json,usage_revision,usage_picos,
         pending_risk_picos,metering_status,watermark) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,0,'0',?11,'pending',?12)",
        params![a.request_id,a.attempt_id,key,a.policy_epoch,a.window.id,a.started_at_ms,a.pricing_revision,
            a.effective_model,a.effective_tier,frozen,if a.enforce {available.to_string()} else {"0".into()},watermark])?;
    let admission_risk = if a.enforce {
        available.to_string()
    } else {
        "0".into()
    };
    update_checkpoint(
        tx,
        &key,
        a.started_at_ms,
        None,
        Some(Contribution {
            amount: "0",
            risk: &admission_risk,
            state: "pending",
            revision: Some(a.pricing_revision),
            watermark: u64::try_from(watermark)?,
        }),
    )?;
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
    }
    tx.execute(
        "DELETE FROM value_quota_gaps WHERE scope=?1 AND request_id=?2",
        params![key, a.request_id],
    )?;
    status_in_tx(tx, a.scope, a.window)
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
    pub tokens: Option<ValueTokens>,
    pub final_observed: bool,
}
pub(crate) struct Adjustment<'a> {
    pub scope: &'a Scope,
    pub window: &'a Window,
    pub grant_revision: u64,
    pub edit: &'a crate::domain::usage::value_pricing::ValueUsageEdit,
}
pub(crate) fn apply_adjustments(
    config_dir: &Path,
    edits: &[Adjustment<'_>],
    actor: &str,
    now: i64,
) -> anyhow::Result<()> {
    assert_committed(config_dir)?;
    for a in edits {
        ensure_restore_coverage(config_dir, a.scope, a.window)?;
    }
    let mut conn = open_database(&database_path(config_dir), false)?;
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    apply_adjustments_in_tx(&tx, edits, actor, now)?;
    tx.commit()?;
    Ok(())
}
fn apply_adjustments_in_tx(
    tx: &Transaction<'_>,
    edits: &[Adjustment<'_>],
    actor: &str,
    now: i64,
) -> anyhow::Result<()> {
    use crate::domain::usage::value_pricing::ValueUsageEditAction;
    ensure!(
        !actor.is_empty() && !edits.is_empty() && edits.len() <= 500,
        "invalid adjustment batch"
    );
    let mut scopes = std::collections::BTreeSet::new();
    for a in edits {
        let key = a.scope.key()?;
        ensure!(scopes.insert(key.clone()), "duplicate adjustment scope");
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
        let current = status_in_tx(tx, a.scope, a.window)?;
        ensure!(
            current.usage_watermark == edit.expected_watermark,
            "VALUE_QUOTA_WATERMARK_CONFLICT"
        );
        let observed = unsigned_decimal(&current.observed_picos)?;
        let (offset, target) = match edit.action {
            ValueUsageEditAction::Clear => {
                ensure!(
                    edit.target_picos.is_none(),
                    "clear must not include targetPicos"
                );
                (0, None)
            }
            ValueUsageEditAction::Set => {
                let target = unsigned_decimal(
                    edit.target_picos
                        .as_deref()
                        .context("targetPicos is required")?,
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
            "beforePicos":current.used_picos,"afterPicos":(observed+offset).to_string()});
        tx.execute("INSERT INTO value_quota_adjustments(operation_id,scope,window_id,payload_json,target_picos,observed_watermark,actor,reason,created_at_ms,watermark)
            VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![edit.operation_id,key,a.window.id,payload.to_string(),target,
            i64::try_from(edit.expected_watermark)?,actor,edit.reason,now,watermark])?;
    }
    Ok(())
}
/// Usage and the executable-quota revision commit together. Incomplete or
/// contradictory evidence keeps risk occupied rather than becoming free.
/// If monetary storage is corrupt, retain the real usage evidence and the
/// original monetary risk. A durable USD-only fence commits with the usage;
/// even a crash before the state-layer marker cannot turn it into free quota.
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
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,'settlement_requires_recovery') ON CONFLICT(key) DO NOTHING",
                [DEGRADED_META],
            )?;
            tracing::error!(%error,"USD settlement isolated; usage evidence and pending risk are preserved");
            Ok(true)
        }
    }
}
pub(super) fn settle_usage_log(
    tx: &Transaction<'_>,
    log: &crate::domain::usage::store::UsageLog,
) -> anyhow::Result<()> {
    use crate::domain::usage::store::{RequestKind, UsageState};
    let rows = {
        let mut q=tx.prepare("SELECT attempt_id,scope,effective_model,effective_tier FROM value_quota_attempts WHERE request_id=?1")?;
        let mapped = q.query_map([&log.request_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        mapped.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (attempt, scope, model, tier) in rows {
        let scope: Scope = serde_json::from_str(&scope)?;
        ensure!(
            log.share_id.as_deref() == Some(&scope.share_id)
                && log
                    .user_email
                    .as_deref()
                    .is_some_and(|email| email.eq_ignore_ascii_case(&scope.email)),
            "VALUE_QUOTA_USAGE_SCOPE_CONFLICT"
        );
        let canonical_tier = match log.effective_service_tier.as_deref() {
            None | Some("default") | Some("standard") => Some("standard"),
            Some("priority") => Some("priority"),
            Some("flex") => Some("flex"),
            _ => None,
        };
        let actual = log.actual_model.as_deref().or(log.model.as_deref());
        let valid = log.attempt_count == 1
            && actual == Some(model.as_str())
            && canonical_tier == Some(tier.as_str())
            && !log.reported_model_mismatch.unwrap_or(false)
            && !log.reported_model_conflict.unwrap_or(false)
            && !log.usage_estimated
            && log.usage_state == UsageState::Observed
            && log.request_kind == RequestKind::Text
            && !log.is_health_check;
        let tokens = if valid { tokens_for_log(log) } else { None };
        let complete = !log.is_streaming || log.stream_status.as_deref() == Some("completed");
        // revision 0 is reserved for the pre-send admission marker.
        settle_in_tx(
            tx,
            &Observation {
                request_id: &log.request_id,
                attempt_id: &attempt,
                scope: &scope,
                usage_revision: log
                    .usage_revision
                    .checked_add(1)
                    .context("value usage revision overflow")?,
                effective_model: &model,
                effective_tier: &tier,
                reported_model: log.reported_model.as_deref(),
                final_observed: tokens.is_some() && complete,
                tokens,
            },
        )?;
    }
    Ok(())
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
fn settle_in_tx(tx: &Transaction<'_>, o: &Observation<'_>) -> anyhow::Result<()> {
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
    );
    let old: AttemptStateRow = tx.query_row(
        "SELECT scope,effective_model,effective_tier,frozen_tariff_json,usage_revision,usage_picos,observation_sha256,
         pending_risk_picos,metering_status,started_at_ms,compacted_at_ms
         FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2", params![o.request_id,o.attempt_id],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?,r.get(9)?,r.get(10)?)))?;
    ensure!(
        old.0 == o.scope.key()? && old.1 == o.effective_model && old.2 == o.effective_tier,
        "VALUE_QUOTA_OBSERVATION_SCOPE_CONFLICT"
    );
    let revision = i64::try_from(o.usage_revision)?;
    let digest = sha256_hex(&serde_json::to_vec(o)?);
    if revision < old.4 {
        return Ok(());
    }
    if revision == old.4 && old.6.is_some() {
        ensure!(
            old.6.as_deref() == Some(&digest),
            "VALUE_QUOTA_REVISION_CONFLICT"
        );
        return Ok(());
    }
    let scope: Scope = serde_json::from_str(&old.0)?;
    let (pricing_revision,snapshot_json): (String,String)=tx.query_row(
        "SELECT a.pricing_revision,s.payload_json FROM value_quota_attempts a JOIN value_pricing_snapshots s
         ON s.revision=a.pricing_revision AND s.router_url=?3 AND s.installation_id=?4
         WHERE a.request_id=?1 AND a.attempt_id=?2",params![o.request_id,o.attempt_id,scope.router_url,scope.installation_id],
        |r|Ok((r.get(0)?,r.get(1)?)))?;
    let snapshot: PricingSnapshot = serde_json::from_str(&snapshot_json)?;
    let tariff: ExecutionModelPrice = if old.10.is_some() {
        ensure!(old.3 == "null", "VALUE_QUOTA_COMPACTED_TARIFF_CORRUPT");
        snapshot.model(o.effective_model)?.clone()
    } else {
        serde_json::from_str(&old.3)?
    };
    ensure!(
        snapshot.revision()? == pricing_revision && snapshot.model(o.effective_model)? == &tariff,
        "VALUE_QUOTA_FROZEN_TARIFF_CORRUPT"
    );
    let reported_matches = o.reported_model.is_none_or(|reported| {
        snapshot
            .exact_aliases
            .get(reported)
            .map(String::as_str)
            .unwrap_or(reported)
            == snapshot
                .exact_aliases
                .get(o.effective_model)
                .map(String::as_str)
                .unwrap_or(o.effective_model)
    });
    let priced = o.tokens.as_ref().and_then(|tokens| {
        reported_matches
            .then(|| tariff.price(o.effective_tier, tokens).ok())
            .flatten()
    });
    let amount = priced
        .map(|p| p.usage_picos)
        .unwrap_or(unsigned_decimal(&old.5)?);
    let state = if o.final_observed && priced.is_some() {
        if priced.is_some_and(|p| p.conservative_cache_ttl) {
            "conservative"
        } else {
            "observed"
        }
    } else if priced.is_some() {
        "pending"
    } else {
        "unavailable"
    };
    let watermark = next_watermark(tx)?;
    let risk = if matches!(state, "observed" | "conservative") {
        0
    } else {
        unsigned_decimal(&old.7)?
            .checked_add(unsigned_decimal(&old.5)?)
            .and_then(|v| v.checked_sub(amount))
            .context("value quota risk overflow")?
            .max(0)
    }
    .to_string();
    tx.execute(
        "INSERT INTO value_quota_revisions(request_id,attempt_id,usage_revision,observation_sha256,usage_picos,metering_status,watermark,observation_json)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![o.request_id,o.attempt_id,revision,digest,amount.to_string(),state,watermark,serde_json::to_string(o)?])?;
    tx.execute(
        "UPDATE value_quota_attempts SET usage_revision=?3,usage_picos=?4,metering_status=?5,watermark=?6,
         observation_sha256=?7,pending_risk_picos=?8
         WHERE request_id=?1 AND attempt_id=?2",
        params![o.request_id,o.attempt_id,revision,amount.to_string(),state,watermark,digest,risk])?;
    update_checkpoint(
        tx,
        &old.0,
        old.9,
        Some(Contribution {
            amount: &old.5,
            risk: &old.7,
            state: &old.8,
            revision: Some(&pricing_revision),
            watermark: 0,
        }),
        Some(Contribution {
            amount: &amount.to_string(),
            risk: &risk,
            state,
            revision: Some(&pricing_revision),
            watermark: u64::try_from(watermark)?,
        }),
    )?;
    if old.10.is_some() && matches!(state, "observed" | "conservative") {
        tx.execute("DELETE FROM value_quota_revisions WHERE request_id=?1 AND attempt_id=?2 AND usage_revision<?3",params![o.request_id,o.attempt_id,revision])?;
    }
    Ok(())
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
    }
    Ok(rows.len())
}
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
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(
        serde_json::json!({"attempts":counts.0,"pendingAttempts":counts.1,"unavailableAttempts":counts.2,
        "coverageGaps":gaps,"oldestUnresolvedAtMs":counts.3,"dailyCheckpoints":checkpoints,
        "databaseBytes":pages.saturating_mul(page_size),"capacityWarning":counts.0>=1_000_000 || pages.saturating_mul(page_size)>=4*1024*1024*1024i64,
        "retention":"attempt identities and latest revisions are never deleted; terminal duplicate tariffs and superseded revisions compact after 30 days"}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;

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
                    final_observed: true,
                },
            )
            .unwrap();
            let status = status_in_tx(&tx, &scope, &window).unwrap();
            let verified = matches!(reported, "model" | "declared-alias");
            assert_eq!(
                status.metering_status,
                if verified { "observed" } else { "unavailable" },
                "{reported}"
            );
            assert_eq!(
                status.pending_risk_picos,
                if verified { "0" } else { "1000000000000" },
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
                    },
                );
                match result {
                    Ok(_) => {
                        tx.commit().unwrap();
                        true
                    }
                    Err(error) => {
                        assert!(error.to_string().contains("EXHAUSTED"));
                        false
                    }
                }
            }));
        }
        assert_eq!(
            tasks
                .into_iter()
                .map(|task| task.join().unwrap())
                .filter(|ok| *ok)
                .count(),
            1
        );
        let reopened = Connection::open(&path).unwrap();
        assert_eq!(
            status_in_tx(&reopened, &scope, &window)
                .unwrap()
                .pending_risk_picos,
            "1000000000000"
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
                    final_observed: true,
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
                    final_observed: true,
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
            final_observed: true,
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
        assert!(status_in_tx(&conn, &scope, &window)
            .unwrap_err()
            .to_string()
            .contains("RESTORE_RECONCILIATION_REQUIRED"));
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
        assert!(settle_usage_log_isolated(&tx, &log).unwrap());
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
                [DEGRADED_META],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "settlement_requires_recovery"
        );
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
            12
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
                final_observed: true,
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
            2
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
            final_observed: true,
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
    fn missing_usage_never_releases_risk_and_cross_scope_reuse_fails() {
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
                final_observed: false,
            },
        )
        .unwrap();
        tx.commit().unwrap();
        let view = status_in_tx(&conn, &scope, &window).unwrap();
        assert_eq!(view.metering_status, "unavailable");
        assert_eq!(view.pending_risk_picos, "1000000000000");
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
}
