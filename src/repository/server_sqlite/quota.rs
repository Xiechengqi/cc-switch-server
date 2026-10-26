//! Exact revisions share the detail transaction but not its retention policy.
use super::*;
use crate::domain::usage::quota_ledger::{QuotaUsageEntry, QuotaUsageLedger};

const META_KEY: &str = "quota_usage_metadata_v1";
const META_DIGEST_KEY: &str = "quota_usage_metadata_sha256_v1";

fn metadata(connection: &Connection) -> anyhow::Result<QuotaUsageLedger> {
    let payload: Option<String> = connection
        .query_row("SELECT value FROM meta WHERE key=?1", [META_KEY], |row| {
            row.get(0)
        })
        .optional()?;
    let ledger: QuotaUsageLedger = if let Some(payload) = payload {
        let digest: Option<String> = connection
            .query_row(
                "SELECT value FROM meta WHERE key=?1",
                [META_DIGEST_KEY],
                |row| row.get(0),
            )
            .optional()?;
        ensure!(
            digest.as_deref() == Some(sha256_hex(payload.as_bytes()).as_str()),
            "quota metadata digest mismatch"
        );
        serde_json::from_str(&payload)?
    } else {
        QuotaUsageLedger::default()
    };
    ledger.validate_metadata().map_err(anyhow::Error::msg)?;
    ensure!(
        ledger.entries.is_empty(),
        "quota metadata contains unexpected rows"
    );
    Ok(ledger)
}

fn save_metadata(transaction: &Transaction<'_>, ledger: &QuotaUsageLedger) -> anyhow::Result<()> {
    let payload = serde_json::to_string(&ledger.metadata())?;
    for (key, value) in [
        (META_KEY, payload.clone()),
        (META_DIGEST_KEY, sha256_hex(payload.as_bytes())),
    ] {
        transaction.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
    }
    Ok(())
}

pub(super) fn load(connection: &Connection) -> anyhow::Result<QuotaUsageLedger> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version == 1 {
        return Ok(QuotaUsageLedger::default());
    }
    let mut ledger = metadata(connection)?;
    let mut statement = connection.prepare(
        "SELECT payload_json,payload_sha256,request_id,usage_revision,watermark,source_ref,source_epoch,
         bucket_id,share_id,user_email,started_at_ms FROM quota_usage_entries ORDER BY watermark")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, i64>(10)?,
        ))
    })?;
    for row in rows {
        let (
            payload,
            digest,
            request,
            revision,
            watermark,
            source,
            epoch,
            bucket,
            share,
            user,
            started,
        ) = row?;
        ensure!(
            sha256_hex(payload.as_bytes()) == digest,
            "quota usage payload digest mismatch"
        );
        let entry: QuotaUsageEntry = serde_json::from_str(&payload)?;
        ensure!(
            entry.request_id == request
                && to_sql_u64(entry.usage_revision)? == revision
                && to_sql_u64(entry.watermark)? == watermark
                && entry.attribution.source_ref == source
                && to_sql_u64(entry.attribution.source_epoch)? == epoch
                && entry.attribution.bucket_id == bucket
                && entry.share_id == share
                && entry.user_email == user
                && entry.attribution.started_at_ms == started,
            "quota normalized columns disagree with immutable payload"
        );
        ledger.insert_persisted(entry).map_err(anyhow::Error::msg)?;
    }
    ledger.validate().map_err(anyhow::Error::msg)?;
    let maximum = ledger
        .entries
        .values()
        .flat_map(|records| records.values())
        .flat_map(|versions| versions.iter())
        .map(|entry| entry.watermark)
        .max()
        .unwrap_or(0);
    ensure!(
        maximum == ledger.watermark,
        "quota usage watermark has missing revisions"
    );
    Ok(ledger)
}

fn insert(transaction: &Transaction<'_>, entry: &QuotaUsageEntry) -> anyhow::Result<()> {
    let payload = serde_json::to_string(entry)?;
    // A revision is immutable, including its watermark. Never silently replace
    // a conflicting row during restore/compaction.
    let existing: Option<String> = transaction.query_row(
        "SELECT payload_sha256 FROM quota_usage_entries WHERE request_id=?1 AND usage_revision=?2",
        params![entry.request_id, to_sql_u64(entry.usage_revision)?], |row| row.get(0)).optional()?;
    let digest = sha256_hex(payload.as_bytes());
    if let Some(existing) = existing {
        ensure!(
            existing == digest,
            "quota usage immutable revision conflict"
        );
        return Ok(());
    }
    transaction.execute(
        "INSERT INTO quota_usage_entries(request_id,usage_revision,watermark,source_ref,source_epoch,bucket_id,
         share_id,user_email,started_at_ms,payload_json,payload_sha256) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![entry.request_id, to_sql_u64(entry.usage_revision)?, to_sql_u64(entry.watermark)?,
            entry.attribution.source_ref, to_sql_u64(entry.attribution.source_epoch)?, entry.attribution.bucket_id,
            entry.share_id, entry.user_email, entry.attribution.started_at_ms, payload, digest])?;
    Ok(())
}

pub(super) fn upsert_log(transaction: &Transaction<'_>, log: &UsageLog) -> anyhow::Result<()> {
    let mut ledger = metadata(transaction)?;
    let previous: Option<(String, String)> = transaction.query_row(
        "SELECT payload_json,payload_sha256 FROM quota_usage_entries WHERE request_id=?1 ORDER BY usage_revision DESC LIMIT 1",
        [&log.request_id], |row| Ok((row.get(0)?, row.get(1)?))).optional()?;
    if let Some((payload, digest)) = previous {
        ensure!(
            sha256_hex(payload.as_bytes()) == digest,
            "quota usage payload digest mismatch"
        );
        ledger
            .insert_persisted(serde_json::from_str(&payload)?)
            .map_err(anyhow::Error::msg)?;
    }
    if ledger.record(log).map_err(anyhow::Error::msg)? {
        let entry = ledger
            .entries
            .values()
            .flat_map(|records| records.values())
            .filter_map(|versions| versions.last())
            .find(|entry| entry.watermark == ledger.watermark)
            .context("new quota usage revision missing")?;
        insert(transaction, entry)?;
        save_metadata(transaction, &ledger)?;
    }
    Ok(())
}

pub(super) fn merge(
    transaction: &Transaction<'_>,
    ledger: &QuotaUsageLedger,
) -> anyhow::Result<()> {
    ledger.validate().map_err(anyhow::Error::msg)?;
    let stored = metadata(transaction)?;
    ensure!(
        stored.watermark <= ledger.watermark,
        "refusing quota usage watermark rollback"
    );
    for records in ledger.entries.values() {
        for versions in records.values() {
            for entry in versions {
                insert(transaction, entry)?;
            }
        }
    }
    save_metadata(transaction, ledger)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        providers::model::{AppKind, ProviderType},
        usage::{
            quota_ledger::QuotaAttribution,
            store::{TokenUsage, UsageModelMetadata},
        },
    };

    fn log() -> UsageLog {
        let mut log = UsageLog::new(
            AppKind::Codex,
            "p".into(),
            "P".into(),
            ProviderType::CodexOAuth,
            200,
            0,
            UsageModelMetadata::default(),
            TokenUsage {
                total_tokens: Some(10),
                ..Default::default()
            },
        );
        log.request_id = "exact-request".into();
        log.share_id = Some("share".into());
        log.user_email = Some("u@example.com".into());
        log.started_at_ms = 4_123;
        log.created_at_ms = 4_123;
        log.quota_attribution = Some(QuotaAttribution {
            source_ref: "opaque".into(),
            source_epoch: 1,
            bucket_id: "weekly".into(),
            started_at_ms: 4_123,
        });
        log
    }

    #[test]
    fn exact_revisions_survive_detail_replacement_and_rollback() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let mut usage = UsageStore::default();
        usage.quota_ledger.begin_collection("opaque", 0);
        let transaction = connection.transaction().unwrap();
        merge(&transaction, &usage.quota_ledger).unwrap();
        transaction.commit().unwrap();
        let mut log = log();
        for tokens in [10, 20] {
            log.total_tokens = Some(tokens);
            let transaction = connection.transaction().unwrap();
            upsert_usage_row(&transaction, &log).unwrap();
            upsert_log(&transaction, &log).unwrap();
            transaction.commit().unwrap();
            usage.push(log.clone());
            log.usage_revision += 1;
        }
        let before = load(&connection).unwrap();
        assert_eq!(before.watermark, 2);
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&usage.quota_ledger).unwrap()
        );
        usage.logs.clear();
        replace_usage_records_in_connection(&connection, &usage).unwrap();
        assert_eq!(load_usage_logs(&connection).unwrap().len(), 0);
        assert_eq!(load(&connection).unwrap().watermark, 2);
        {
            let transaction = connection.transaction().unwrap();
            log.total_tokens = Some(99);
            upsert_usage_row(&transaction, &log).unwrap();
            upsert_log(&transaction, &log).unwrap();
            // Simulate a crash before COMMIT: both detail and exact revision roll back.
        }
        assert_eq!(load(&connection).unwrap().watermark, 2);
        assert!(load_usage_logs(&connection).unwrap().is_empty());
    }

    #[test]
    fn request_identity_cannot_move_and_corrupt_revisions_are_rejected() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let mut log = log();
        let transaction = connection.transaction().unwrap();
        upsert_log(&transaction, &log).unwrap();
        transaction.commit().unwrap();
        log.usage_revision += 1;
        log.quota_attribution.as_mut().unwrap().source_ref = "other".into();
        let transaction = connection.transaction().unwrap();
        assert!(upsert_log(&transaction, &log).is_err());
        drop(transaction);
        connection
            .execute("DELETE FROM quota_usage_entries", [])
            .unwrap();
        assert!(load(&connection).is_err());
    }

    #[test]
    fn quota_metadata_and_normalized_columns_detect_tampering() {
        for sql in [
            "UPDATE meta SET value=json_set(value,'$.watermark',0) WHERE key='quota_usage_metadata_v1'",
            "DELETE FROM meta WHERE key='quota_usage_metadata_sha256_v1'",
            "UPDATE quota_usage_entries SET started_at_ms=started_at_ms+1",
            "UPDATE quota_usage_entries SET source_epoch=2",
        ] {
            let mut connection = Connection::open_in_memory().unwrap();
            initialize_schema(&connection).unwrap();
            let transaction = connection.transaction().unwrap();
            upsert_log(&transaction, &log()).unwrap(); transaction.commit().unwrap();
            assert!(load(&connection).is_ok());
            connection.execute_batch(sql).unwrap();
            assert!(load(&connection).is_err(), "tampering not detected: {sql}");
        }
    }

    #[test]
    fn quota_missing_middle_revision_is_not_hidden_by_maximum_watermark() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_schema(&connection).unwrap();
        let mut log = log();
        for tokens in [10, 20, 30] {
            log.total_tokens = Some(tokens);
            let transaction = connection.transaction().unwrap();
            upsert_log(&transaction, &log).unwrap();
            transaction.commit().unwrap();
            log.usage_revision += 1;
        }
        assert_eq!(load(&connection).unwrap().watermark, 3);
        connection
            .execute("DELETE FROM quota_usage_entries WHERE watermark=2", [])
            .unwrap();
        assert!(load(&connection).is_err());
    }
}
