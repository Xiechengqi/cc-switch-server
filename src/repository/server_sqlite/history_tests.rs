use super::*;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::usage::store::{TokenUsage, UsageModelMetadata};

fn record(id: &str, sequence: u64, created: u128) -> UsageLog {
    let mut log = UsageLog::new(
        AppKind::Codex,
        "provider".into(),
        "Provider".into(),
        ProviderType::Codex,
        200,
        17,
        UsageModelMetadata::default(),
        TokenUsage::default(),
    );
    log.request_id = id.into();
    log.created_at_ms = created;
    log.share_id = Some("share".into());
    log.data_source = Some("router_share".into());
    log.router_export_sequence = sequence;
    log
}

fn reader(connection: Arc<Mutex<Connection>>) -> SqliteUsageHistory {
    SqliteUsageHistory {
        config_dir: PathBuf::new(),
        connection: Some(connection),
        retention_cutoff_ms: 0,
        statistics: Mutex::new(BTreeMap::new()),
    }
}

#[test]
fn provider_statistics_use_covering_projection_and_revision_replacement() {
    let mut connection = Connection::open_in_memory().unwrap();
    schema::initialize_schema(&connection).unwrap();
    let mut log = record("req_statistics", 1, 10);
    // The projection must preserve the integer text, including u128 latency.
    log.duration_ms = u128::from(u64::MAX) + 19;
    let transaction = connection.transaction().unwrap();
    upsert_usage_row(&transaction, &log).unwrap();
    transaction.commit().unwrap();
    let plan = connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT created_at_ms,status_code,duration_ms_text
        FROM usage_records INDEXED BY usage_provider_statistics_idx
        WHERE provider_id='provider' AND app='codex' AND is_health_check=0 AND created_at_ms>=0",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("usage_provider_statistics_idx"));
    let shared = Arc::new(Mutex::new(connection));
    let view = reader(shared.clone());
    let stats = view
        .provider_statistics(AppKind::Codex, "provider")
        .unwrap();
    assert_eq!(stats.requests, 1);
    assert_eq!(stats.latency_total_ms, log.duration_ms);
    log.usage_revision += 1;
    log.status_code = 503;
    log.duration_ms = 23;
    {
        let mut connection = shared.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        upsert_usage_row(&transaction, &log).unwrap();
        transaction.commit().unwrap();
    }
    // A repeated projection in one view reuses its statistics; a fresh view
    // counts the revised request once, not as a second request.
    assert_eq!(
        view.provider_statistics(AppKind::Codex, "provider")
            .unwrap()
            .successes,
        1
    );
    let stats = reader(shared)
        .provider_statistics(AppKind::Codex, "provider")
        .unwrap();
    assert_eq!((stats.requests, stats.successes, stats.failures), (1, 0, 1));
    assert_eq!(stats.latency_total_ms, 23);
}

#[test]
fn export_cursor_is_applied_before_limit_and_includes_old_revisions() {
    let mut connection = Connection::open_in_memory().unwrap();
    schema::initialize_schema(&connection).unwrap();
    let transaction = connection.transaction().unwrap();
    for log in [
        record("req_before", 1, 1000),
        record("req_first", 2, 3000),
        record("req_revised", 3, 1),
        record("req_after", 4, 2000),
    ] {
        upsert_usage_row(&transaction, &log).unwrap();
    }
    transaction.commit().unwrap();
    let history = reader(Arc::new(Mutex::new(connection)));
    let mut ids = Vec::new();
    history
        .visit(
            &UsageSelection {
                share_id: Some("share".into()),
                data_source: Some("router_share".into()),
                is_health_check: Some(false),
                include_expired: true,
                after_export_sequence: Some(1),
                limit: Some(2),
                ..Default::default()
            },
            &mut |log| {
                ids.push(log.request_id.clone());
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(ids, ["req_first", "req_revised"]);
}

#[test]
fn compatible_extension_tracks_legacy_v10_payload_writes() {
    let connection = Connection::open_in_memory().unwrap();
    schema::initialize_schema(&connection).unwrap();
    let mut log = record("req_legacy", 1, 10);
    let payload = serde_json::to_string(&log).unwrap();
    connection.execute("INSERT INTO usage_records(request_id,created_at_ms,share_id,provider_id,usage_revision,payload_json,payload_sha256)
        VALUES(?1,10,'share','provider',0,?2,?3)", params![log.request_id,payload,sha256_hex(payload.as_bytes())]).unwrap();
    log.status_code = 503;
    log.usage_revision = 1;
    let payload = serde_json::to_string(&log).unwrap();
    connection.execute("UPDATE usage_records SET usage_revision=1,payload_json=?1,payload_sha256=?2 WHERE request_id=?3",
        params![payload,sha256_hex(payload.as_bytes()),log.request_id]).unwrap();
    schema::migrate_to_current(&connection).unwrap();
    schema::migrate_to_current(&connection).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        10
    );
    let stats = reader(Arc::new(Mutex::new(connection)))
        .provider_statistics(AppKind::Codex, "provider")
        .unwrap();
    assert_eq!((stats.requests, stats.successes, stats.failures), (1, 0, 1));
}

#[test]
fn control_read_snapshot_preserves_health_and_quota_without_recent_details() {
    use crate::domain::health::{ProviderHealthObservation, ProviderHealthStatus};
    let connection = Connection::open_in_memory().unwrap();
    schema::initialize_schema(&connection).unwrap();
    let mut usage = UsageStore {
        history: Some(Arc::new(reader(Arc::new(Mutex::new(connection))))),
        ..Default::default()
    };
    usage.logs.push(record("req_recent", 1, 10));
    usage.quota_ledger.watermark = 7;
    let health = usage.provider_health.record(ProviderHealthObservation {
        app: AppKind::Codex,
        provider_id: "provider".into(),
        provider_revision: 3,
        runtime_fingerprint: "fingerprint".into(),
        status: ProviderHealthStatus::Healthy,
        checked_at_ms: 10,
        source: "scheduled".into(),
        status_code: Some(200),
        latency_ms: Some(17),
        model: None,
        error_category: None,
        error_message: None,
        transient_failure: false,
    });
    let snapshot = usage.read_snapshot();
    assert!(snapshot.logs.is_empty());
    assert!(snapshot.history.is_some());
    assert_eq!(snapshot.quota_ledger.watermark, 7);
    assert_eq!(
        snapshot.provider_health.get(AppKind::Codex, "provider"),
        Some(&health)
    );
}
