use std::sync::{Arc, Mutex};

use super::*;
use crate::domain::usage::history::{UsageHistory, UsageSelection};
use rusqlite::{params_from_iter, types::Value as SqlValue, OptionalExtension};

#[derive(Debug)]
struct SqliteUsageHistory {
    config_dir: PathBuf,
    connection: Option<Arc<Mutex<Connection>>>,
}

pub(super) fn attach(config_dir: &Path, store: &mut UsageStore) -> anyhow::Result<()> {
    store.history = Some(Arc::new(SqliteUsageHistory {
        config_dir: config_dir.into(),
        connection: None,
    }));
    let selection = UsageSelection {
        limit: Some(2048),
        ..Default::default()
    };
    let history = store.history.clone().expect("attached durable history");
    history.visit(&selection, &mut |log| {
        store.cache_log(log.clone());
        Ok(())
    })?;
    Ok(())
}

fn decode(payload: &str, digest: &str) -> anyhow::Result<UsageLog> {
    ensure!(
        sha256_hex(payload.as_bytes()) == digest,
        "Usage SQLite payload digest mismatch"
    );
    serde_json::from_str(payload).context("decode durable Usage record")
}

impl SqliteUsageHistory {
    fn with_connection<T>(
        &self,
        read: impl FnOnce(&Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if let Some(connection) = &self.connection {
            let connection = connection
                .lock()
                .map_err(|_| anyhow::anyhow!("Usage read snapshot poisoned"))?;
            read(&connection)
        } else {
            let connection = open_database(&database_path(&self.config_dir), true)?;
            read(&connection)
        }
    }
}

impl UsageHistory for SqliteUsageHistory {
    fn snapshot(&self) -> anyhow::Result<Arc<dyn UsageHistory>> {
        let connection = open_database(&database_path(&self.config_dir), true)?;
        connection.execute_batch("BEGIN DEFERRED;")?;
        connection
            .query_row(
                "SELECT value FROM meta WHERE key='repository_generation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        Ok(Arc::new(Self {
            config_dir: self.config_dir.clone(),
            connection: Some(Arc::new(Mutex::new(connection))),
        }))
    }
    fn visit(
        &self,
        selection: &UsageSelection,
        visitor: &mut dyn FnMut(&UsageLog) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.with_connection(|connection| {
        let mut conditions = Vec::new();
        let mut parameters = Vec::<SqlValue>::new();
        let mut add = |sql: &str, value: SqlValue| {
            conditions.push(sql.to_owned());
            parameters.push(value);
        };
        if !selection.include_expired && !selection.pending_router_sync {
            let cutoff = crate::infra::time::now_ms()
                .saturating_sub(crate::domain::usage::store::USAGE_DETAIL_RETENTION_MS);
            add(
                "created_at_ms >= ?",
                SqlValue::Integer(i64::try_from(cutoff)?),
            );
        }
        if let Some(from) = selection.from_ms {
            add(
                "created_at_ms >= ?",
                SqlValue::Integer(i64::try_from(from)?),
            );
        }
        if let Some(to) = selection.to_ms {
            add("created_at_ms < ?", SqlValue::Integer(i64::try_from(to)?));
        }
        if let Some(app) = selection.app {
            add(
                "json_extract(payload_json,'$.app') = ?",
                SqlValue::Text(app.as_str().into()),
            );
        }
        if let Some(id) = &selection.provider_id {
            add("provider_id = ?", SqlValue::Text(id.clone()));
        }
        if let Some(id) = &selection.share_id {
            add("share_id = ?", SqlValue::Text(id.clone()));
        }
        if let Some(email) = &selection.user_email {
            add(
                "lower(json_extract(payload_json,'$.userEmail')) = lower(?)",
                SqlValue::Text(email.clone()),
            );
        }
        if let Some(health) = selection.is_health_check {
            add(
                "coalesce(json_extract(payload_json,'$.isHealthCheck'),0) = ?",
                SqlValue::Integer(i64::from(health)),
            );
        }
        if let Some(source) = &selection.data_source {
            add(
                "json_extract(payload_json,'$.dataSource') = ?",
                SqlValue::Text(source.clone()),
            );
        }
        if let Some(fingerprint) = &selection.health_fingerprint {
            add(
                "json_extract(payload_json,'$.providerHealthFingerprint') = ?",
                SqlValue::Text(fingerprint.clone()),
            );
        }
        if let Some(model) = &selection.requested_model {
            add(
                "json_extract(payload_json,'$.requestedModel') = ?",
                SqlValue::Text(model.clone()),
            );
        }
        if let Some(prefix) = &selection.data_source_prefix {
            add(
                "substr(json_extract(payload_json,'$.dataSource'),1,length(?)) = ?",
                SqlValue::Text(prefix.clone()),
            );
            parameters.push(SqlValue::Text(prefix.clone()));
        }
        if selection.exact_probe_identity {
            for (column, value) in [
                ("share_id", &selection.share_id),
                (
                    "json_extract(payload_json,'$.dataSource')",
                    &selection.data_source,
                ),
                (
                    "json_extract(payload_json,'$.providerHealthFingerprint')",
                    &selection.health_fingerprint,
                ),
                (
                    "json_extract(payload_json,'$.requestedModel')",
                    &selection.requested_model,
                ),
            ] {
                if value.is_none() {
                    conditions.push(format!("{column} IS NULL"));
                }
            }
        }
        if selection.pending_router_sync {
            conditions.push("json_extract(payload_json,'$.routerLastSyncedAtMs') IS NULL AND share_id IS NOT NULL AND coalesce(json_extract(payload_json,'$.isHealthCheck'),0)=0 AND coalesce(json_extract(payload_json,'$.recordKind'),'user_inference')='user_inference' AND json_extract(payload_json,'$.dataSource')='router_share' AND ((length(request_id) BETWEEN 8 AND 80 AND substr(request_id,1,4)='req_' AND request_id NOT GLOB '*[^A-Za-z0-9_-]*') OR (length(request_id)=36 AND substr(request_id,9,1)='-' AND substr(request_id,14,1)='-' AND substr(request_id,19,1)='-' AND substr(request_id,24,1)='-' AND length(replace(request_id,'-',''))=32 AND request_id NOT GLOB '*[^0-9a-fA-F-]*'))".into());
        }
        if let Some(now) = selection.retry_before_ms {
            conditions.push(
                "coalesce(json_extract(payload_json,'$.routerNextSyncAttemptAtMs'),0) <= ?".into(),
            );
            parameters.push(SqlValue::Integer(i64::try_from(now)?));
        }
        let predicate = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        let order = if selection.limit.is_some() {
            "DESC"
        } else {
            "ASC"
        };
        let ordering = if selection.pending_router_sync {
            "coalesce(json_extract(payload_json,'$.routerExportSequence'),0),created_at_ms,request_id".to_owned()
        } else {
            format!("created_at_ms {order},request_id {order}")
        };
        let mut sql = format!(
            "SELECT payload_json,payload_sha256 FROM usage_records{predicate} ORDER BY {ordering}"
        );
        if let Some(limit) = selection.limit {
            sql.push_str(" LIMIT ?");
            parameters.push(SqlValue::Integer(i64::try_from(limit)?));
        }
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(parameters))?;
        while let Some(row) = rows.next()? {
            let log = decode(row.get_ref(0)?.as_str()?, row.get_ref(1)?.as_str()?)?;
            visitor(&log)?;
        }
        Ok(())
        })
    }

    fn request(&self, id: &str) -> anyhow::Result<Option<UsageLog>> {
        self.with_connection(|connection| {
            let row: Option<(String, String)> = connection
                .query_row(
                    "SELECT payload_json,payload_sha256 FROM usage_records WHERE request_id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            row.map(|(payload, digest)| decode(&payload, &digest))
                .transpose()
        })
    }
}

/// Recover only affected rows. Each batch commits its Usage revision and exact
/// accounting evidence together and can be resumed after an interrupted boot.
pub(super) fn recover_interrupted(config_dir: &Path) -> anyhow::Result<()> {
    use crate::domain::usage::store::{UsageOutcome, UsageRecordKind, UsageState};
    let mut connection = open_database(&database_path(config_dir), false)?;
    quota::reconcile_usage(&mut connection)?;
    let mut after = String::new();
    let now = crate::infra::time::now_ms();
    let mut sequence = next_export_sequence(&connection)?.saturating_sub(1);
    loop {
        let transaction = connection.transaction()?;
        let batch = {
            let mut statement = transaction.prepare(
                "SELECT request_id,payload_json,payload_sha256 FROM usage_records
                 WHERE request_id>?1 AND coalesce(json_extract(payload_json,'$.recordKind'),'user_inference')='user_inference'
                 AND (json_extract(payload_json,'$.usageState')='pending' OR json_extract(payload_json,'$.outcome')='pending')
                 ORDER BY request_id LIMIT 256")?;
            let rows = statement.query_map([&after], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        if batch.is_empty() {
            break;
        }
        for (id, payload, digest) in batch {
            let mut log = decode(&payload, &digest)?;
            ensure!(
                log.record_kind == UsageRecordKind::UserInference,
                "invalid pending Usage kind"
            );
            log.usage_state = UsageState::Interrupted;
            log.outcome = UsageOutcome::Interrupted;
            log.failure_kind = Some("server_restarted".into());
            log.completed_at_ms = now;
            log.end_to_end_duration_ms = now.saturating_sub(log.started_at_ms);
            log.usage_revision = log
                .usage_revision
                .checked_add(1)
                .context("Usage revision overflow")?;
            log.reset_router_sync_state();
            sequence = sequence
                .checked_add(1)
                .context("Usage export sequence overflow")?;
            log.router_export_sequence = sequence;
            upsert_usage_row(&transaction, &log)?;
            quota::upsert_log(&transaction, &log)?;
            value_quota::evidence::capture_log(&transaction, &log, None)?;
            capability_context::observe(&transaction, &log)?;
            value_quota::settle_usage_log_isolated(&transaction, &log)?;
            market_termination::finalize_log_in(&transaction, &log)?;
            after = id;
        }
        bump_repository_generation(&transaction)?;
        transaction.commit()?;
        crate::infra::startup::advance("usage_recovery", 256);
    }
    quota::recover_pending(&mut connection)?;
    Ok(())
}

pub(super) fn next_export_sequence(connection: &Connection) -> anyhow::Result<u64> {
    let next: i64 = connection.query_row(
        "SELECT coalesce(max(coalesce(json_extract(payload_json,'$.routerExportSequence'),0)),0)+1 FROM usage_records",
        [], |row| row.get(0))?;
    Ok(u64::try_from(next)?)
}
