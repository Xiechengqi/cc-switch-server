//! Ledger storage maintenance. Financial identities, amounts, checkpoints,
//! conversion facts and capability receipts are permanent. Old detail JSON is
//! compressed losslessly; late settlement and unit conversion use the same reader.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::BTreeSet;
use std::io::Read;

pub(crate) const RETENTION_DAYS: i64 = 90;
const DAY_MS: i64 = super::DAY_MS;
const ALIAS_TRIM_CURSOR_KEY: &str = "value_alias_trim_cursor";
const ALIAS_TRIM_DONE: &str = "done";
const ALIAS_PAGE: usize = 100;
const KEEP_NEWEST_SNAPSHOTS: usize = 3;
const META_ARCHIVE_PREFIX: &str = "zstd-v1:";
const MAX_META_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RetentionReport {
    pub archived_attempts: usize,
    pub deleted_snapshots: usize,
    pub trimmed_alias_rows: usize,
}

/// A bounded pass, invoked on the blocking pool without network activity.
pub(crate) fn maintain(dir: &Path, now: i64) -> anyhow::Result<RetentionReport> {
    assert_committed(dir)?;
    let mut conn = open_database(&database_path(dir), false)?;
    let trimmed_alias_rows = trim_alias_page(&mut conn)?;
    let cutoff = now.saturating_sub(RETENTION_DAYS * DAY_MS);
    let archived_attempts = archive_meta_page(&mut conn, cutoff)?;
    let last: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key='value_snapshot_gc_at'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let deleted_snapshots = if last
        .and_then(|s| s.parse::<i64>().ok())
        .is_none_or(|at| now.saturating_sub(at) >= DAY_MS)
    {
        let deleted = collect_snapshots(&mut conn, cutoff)?;
        conn.execute(
            "INSERT INTO meta(key,value) VALUES('value_snapshot_gc_at',?1)
            ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [now.to_string()],
        )?;
        deleted
    } else {
        0
    };
    Ok(RetentionReport {
        archived_attempts,
        deleted_snapshots,
        trimmed_alias_rows,
    })
}

pub(super) fn decode_meta(raw: &str) -> anyhow::Result<AttemptMeta> {
    let Some(archive) = raw.strip_prefix(META_ARCHIVE_PREFIX) else {
        return serde_json::from_str(raw).context("VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT");
    };
    let (hash, data) = archive
        .split_once(':')
        .context("VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT")?;
    ensure!(
        hash.len() == 64 && data.len() <= MAX_META_BYTES * 2,
        "VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT"
    );
    let compressed = STANDARD
        .decode(data)
        .context("VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT")?;
    let mut decoder = zstd::stream::read::Decoder::new(compressed.as_slice())?;
    decoder.window_log_max(22)?; // Bound decoder memory as well as output to 4 MiB.
    let mut json = Vec::new();
    decoder
        .take((MAX_META_BYTES + 1) as u64)
        .read_to_end(&mut json)?;
    ensure!(
        json.len() <= MAX_META_BYTES && sha256_hex(&json) == hash,
        "VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT"
    );
    serde_json::from_slice(&json).context("VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT")
}

fn encode_archive(raw: &str) -> anyhow::Result<String> {
    ensure!(
        raw.len() <= MAX_META_BYTES,
        "VALUE_QUOTA_ATTEMPT_METADATA_CORRUPT"
    );
    // Decode before archiving: never bury invalid JSON or amplify corruption.
    let _ = decode_meta(raw)?;
    let compressed = zstd::stream::encode_all(raw.as_bytes(), 3)?;
    Ok(format!(
        "{META_ARCHIVE_PREFIX}{}:{}",
        sha256_hex(raw.as_bytes()),
        STANDARD.encode(compressed)
    ))
}

pub(super) fn archive_meta_page(conn: &mut Connection, cutoff: i64) -> anyhow::Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let mut archived = 0;
    for status in ["observed", "conservative"] {
        let key = format!("value_archive_cursor:{status}");
        let cursor: Option<String> = tx
            .query_row("SELECT value FROM meta WHERE key=?1", [&key], |r| r.get(0))
            .optional()?;
        let cursor: (i64, String, String) = cursor
            .map(|v| serde_json::from_str(&v))
            .transpose()?
            .unwrap_or((-1, String::new(), String::new()));
        // Visit a bounded page even when every row is already archived. Filtering
        // on metadata content would rescan all archived history every heartbeat.
        let rows: Vec<(i64, String, String, String)> = {
            let mut query = tx.prepare("SELECT a.started_at_ms,a.request_id,a.attempt_id,m.value
                FROM value_quota_attempts a JOIN meta m ON m.key = 'value_attempt:' || json_array(a.request_id,a.attempt_id)
                WHERE a.metering_status=?1 AND a.started_at_ms<?2
                AND (a.started_at_ms,a.request_id,a.attempt_id)>(?3,?4,?5)
                ORDER BY a.started_at_ms,a.request_id,a.attempt_id LIMIT 100")?;
            let rows = query
                .query_map(params![status, cutoff, cursor.0, cursor.1, cursor.2], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (_, request, attempt, raw) in &rows {
            if raw.starts_with(META_ARCHIVE_PREFIX) || raw.len() > MAX_META_BYTES {
                continue;
            }
            let encoded = match encode_archive(raw) {
                Ok(encoded) => encoded,
                Err(error) => {
                    tracing::warn!(%error,request_id=%request,attempt_id=%attempt,"invalid quota metadata retained unchanged during archival");
                    continue;
                }
            };
            if encoded.len() >= raw.len() {
                continue;
            }
            tx.execute(
                "UPDATE meta SET value=?2 WHERE key=?1",
                params![attempt_meta_key(request, attempt)?, encoded],
            )?;
            archived += 1;
        }
        // Wrap on a following pass, so late observations that unarchive a row
        // and late terminal transitions are eventually visited again.
        let next = rows
            .last()
            .map(|(at, request, attempt, _)| (*at, request.clone(), attempt.clone()))
            .unwrap_or((-1, String::new(), String::new()));
        tx.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,serde_json::to_string(&next)?])?;
    }
    tx.commit()?;
    Ok(archived)
}

/// Deletes unreferenced tariff snapshots installed before `cutoff`.
pub(super) fn collect_snapshots(conn: &mut Connection, cutoff: i64) -> anyhow::Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let referenced: BTreeSet<String> = {
        // Attempts settle and reprice against their own revision; a history
        // preparation (unit conversion) materializes facts at its revision.
        let mut q = tx.prepare(
            "SELECT pricing_revision FROM value_quota_attempts
             UNION SELECT pricing_revision FROM quota_history_preparations",
        )?;
        let rows = q.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    // Active/pending tariffs and re-pricing jobs name their revision in meta.
    let pinned: Vec<String> = {
        let mut q = tx.prepare(
            "SELECT value FROM meta WHERE key LIKE 'value_tariff:%' OR key LIKE 'value_tariff_pending:%' OR key LIKE 'value_reprice_due:%'",
        )?;
        let rows = q.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let candidates: Vec<(String, String, String, i64)> = {
        let mut q = tx.prepare(
            "SELECT router_url,installation_id,revision,installed_at_ms FROM value_pricing_snapshots
             ORDER BY router_url,installation_id,installed_at_ms DESC",
        )?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut newest: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut deleted = 0;
    for (router, installation, revision, installed_at) in candidates {
        let rank = newest
            .entry((router.clone(), installation.clone()))
            .or_default();
        *rank += 1;
        if *rank <= KEEP_NEWEST_SNAPSHOTS
            || installed_at >= cutoff
            || referenced.contains(&revision)
            || pinned.iter().any(|value| value.contains(revision.as_str()))
        {
            continue;
        }
        deleted += tx.execute(
            "DELETE FROM value_pricing_snapshots WHERE router_url=?1 AND installation_id=?2 AND revision=?3",
            params![router, installation, revision],
        )?;
    }
    tx.commit()?;
    Ok(deleted)
}

/// Rewrites one page of attempt meta rows that still carry the whole alias
/// catalogue. Resumable through a meta cursor; a no-op once done.
pub(super) fn trim_alias_page(conn: &mut Connection) -> anyhow::Result<usize> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let cursor: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [ALIAS_TRIM_CURSOR_KEY],
            |r| r.get(0),
        )
        .optional()?;
    if cursor.as_deref() == Some(ALIAS_TRIM_DONE) {
        return Ok(0);
    }
    let rows: Vec<(String, String)> = {
        let mut q = tx.prepare(
            "SELECT key,value FROM meta WHERE key>?1 AND key>='value_attempt:' AND key<'value_attempt;'
             ORDER BY key LIMIT ?2",
        )?;
        let rows = q.query_map(
            params![cursor.unwrap_or_default(), ALIAS_PAGE as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut trimmed = 0;
    for (key, value) in &rows {
        let Ok(mut meta) = decode_meta(value) else {
            continue;
        };
        if meta.exact_aliases.len() <= 1 {
            continue;
        }
        let Ok((request, attempt)) = serde_json::from_str::<(String, String)>(
            key.strip_prefix("value_attempt:").unwrap_or_default(),
        ) else {
            continue;
        };
        let model: Option<String> = tx
            .query_row(
                "SELECT effective_model FROM value_quota_attempts WHERE request_id=?1 AND attempt_id=?2",
                params![request, attempt],
                |r| r.get(0),
            )
            .optional()?;
        let Some(model) = model else {
            continue;
        };
        let subset = attempt_aliases(&meta.exact_aliases, &model);
        if subset.len() == meta.exact_aliases.len() {
            continue;
        }
        meta.exact_aliases = subset;
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, serde_json::to_string(&meta)?],
        )?;
        trimmed += 1;
    }
    let next = if rows.len() < ALIAS_PAGE {
        ALIAS_TRIM_DONE.to_owned()
    } else {
        rows.last().map(|(key, _)| key.clone()).unwrap_or_default()
    };
    tx.execute(
        "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![ALIAS_TRIM_CURSOR_KEY, next],
    )?;
    tx.commit()?;
    Ok(trimmed)
}
