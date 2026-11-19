// Included in ledger tests: bounded growth (archive, snapshot collection,
// alias trimming), no-output failures and the owner reject policy.
fn retention_admission<'a>(
    scope: &'a Scope,
    window: &'a Window,
    quota: &'a ValueQuota,
    revision: &'a str,
    request: &'a str,
    started_at_ms: i64,
) -> Admission<'a> {
    Admission {
        attribution: None,
        resource_plan: None,
        enforce: true,
        scope,
        window,
        policy_epoch: "epoch",
        quota,
        pricing_revision: revision,
        request_id: request,
        attempt_id: "1",
        effective_model: "model",
        effective_tier: "standard",
        started_at_ms,
        estimate: RiskEstimate {
            input_tokens: 10,
            output_tokens: 10,
        },
        parallel_limit: 4,
    }
}

#[test]
fn value_quota_archive_preserves_identity_usage_and_late_settlement() {
    let (mut conn, scope, window, quota, revision) = setup();
    {
        let tx = conn.transaction().unwrap();
        for request in ["archived", "pending"] {
            admit_in_tx(
                &tx,
                &retention_admission(&scope, &window, &quota, &revision, request, 1),
            )
            .unwrap();
        }
        settle_outcome_in_tx(&tx, "archived", "1", AttemptOutcome::NoOutput, true).unwrap();
        let mut meta = read_attempt_meta(&tx, "archived", "1").unwrap().unwrap();
        meta.exact_aliases = (0..1000)
            .map(|i| (format!("model-{i:04}"), "model".into()))
            .collect();
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![
                attempt_meta_key("archived", "1").unwrap(),
                serde_json::to_string(&meta).unwrap()
            ],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let before = status_in_tx(&conn, &scope, &window).unwrap();
    assert_eq!(retention::archive_meta_page(&mut conn, DAY_MS).unwrap(), 1);
    let raw: String = conn
        .query_row(
            "SELECT value FROM meta WHERE key=?1",
            [attempt_meta_key("archived", "1").unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert!(raw.starts_with("zstd-v1:"));
    assert!(
        raw.len() < 10_000,
        "archive must reduce the retained detail size"
    );
    assert_eq!(
        read_attempt_meta(&conn, "archived", "1")
            .unwrap()
            .unwrap()
            .exact_aliases
            .len(),
        1000
    );
    let after = status_in_tx(&conn, &scope, &window).unwrap();
    assert_eq!(
        (
            after.used_picos,
            after.pending_risk_picos,
            after.usage_watermark
        ),
        (
            before.used_picos,
            before.pending_risk_picos,
            before.usage_watermark
        )
    );
    assert_eq!(retention::archive_meta_page(&mut conn, DAY_MS).unwrap(), 0);
    assert_eq!(
        conn.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    // Historical attempts can still be corrected from real usage after archival.
    let tx = conn.transaction().unwrap();
    let observation = Observation {
        request_id: "archived",
        attempt_id: "1",
        usage_revision: 1,
        effective_model: "model",
        effective_tier: "standard",
        reported_model: Some("model-0001"),
        observed_tier: None,
        tokens: Some(ValueTokens {
            fresh_input: 100,
            output: 20,
            ..Default::default()
        }),
        resources: None,
        outcome: ObservedOutcome::Final,
        scope: &scope,
        synthetic: false,
    };
    assert!(settle_in_tx(&tx, &observation).unwrap());
    assert!(!settle_in_tx(&tx, &observation).unwrap());
    assert_eq!(
        read_attempt_row(&tx, "archived", "1").unwrap().unwrap().5,
        "120000000"
    );
    // Replaying a generation keeps the old identity and cannot create a new debit.
    admit_in_tx(
        &tx,
        &retention_admission(&scope, &window, &quota, &revision, "archived", 1),
    )
    .unwrap();
    assert_eq!(
        tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    tx.commit().unwrap();
    let mut corrupted = raw.into_bytes();
    corrupted[8] = if corrupted[8] == b'0' { b'1' } else { b'0' };
    assert!(retention::decode_meta(std::str::from_utf8(&corrupted).unwrap()).is_err());
}

#[test]
fn value_quota_snapshot_collection_pins_conversion_preparations() {
    let (mut conn, scope, _, _, revision) = setup();
    let mut revisions = Vec::new();
    {
        let tx = conn.transaction().unwrap();
        let base: PricingSnapshot = serde_json::from_str(
            &tx.query_row(
                "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
                [revision],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        for n in 1..=6i64 {
            let mut snapshot = base.clone();
            snapshot
                .models
                .get_mut("model")
                .unwrap()
                .rates
                .get_mut("standard/base")
                .unwrap()
                .input = Some((1_000_000 + n).to_string());
            let revision = snapshot.revision().unwrap();
            install_in_tx(&tx, &scope, &revision, &snapshot, n * DAY_MS).unwrap();
            revisions.push(revision);
        }
        tx.execute("INSERT INTO quota_history_preparations(scope,cursor,pricing_revision,state,processed,updated_at_ms) VALUES(?1,'',?2,'preparing',0,0)",params![scope.key().unwrap(),revisions[0]]).unwrap();
        tx.execute(
            "INSERT INTO meta(key,value) VALUES('value_tariff_pending:test',?1)",
            [revisions[1].clone()],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    retention::collect_snapshots(&mut conn, 10 * DAY_MS).unwrap();
    for (index, revision) in revisions.iter().enumerate() {
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM value_pricing_snapshots WHERE revision=?1)",
                [revision],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, index != 2, "revision {index}");
    }
}

#[test]
fn value_quota_attempt_aliases_keep_only_the_effective_models_family() {
    let aliases = BTreeMap::from([
        ("model-latest".to_string(), "model".to_string()),
        ("model-2026".to_string(), "model".to_string()),
        ("other-latest".to_string(), "other".to_string()),
    ]);
    let subset = attempt_aliases(&aliases, "model-latest");
    assert_eq!(
        subset.keys().map(String::as_str).collect::<Vec<_>>(),
        ["model-2026", "model-latest"]
    );
    // A canonical key keeps its aliases; an unknown model keeps nothing.
    assert_eq!(attempt_aliases(&aliases, "model").len(), 2);
    assert!(attempt_aliases(&aliases, "unknown").is_empty());
    // Same answer as the full table for every reported model.
    let canonical = |table: &BTreeMap<String, String>, model: &str| {
        table
            .get(model)
            .cloned()
            .unwrap_or_else(|| model.to_owned())
    };
    for reported in [
        "model",
        "model-latest",
        "model-2026",
        "other",
        "other-latest",
        "x",
    ] {
        assert_eq!(
            canonical(&aliases, reported) == canonical(&aliases, "model-latest"),
            canonical(&subset, reported) == canonical(&subset, "model-latest"),
            "{reported}"
        );
    }
}

#[test]
fn value_quota_no_output_failures_charge_the_input_estimate_only() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    admit_in_tx(
        &tx,
        &retention_admission(&scope, &window, &quota, &revision, "no-output", 1),
    )
    .unwrap();
    assert!(settle_outcome_in_tx(&tx, "no-output", "1", AttemptOutcome::NoOutput, true).unwrap());
    let view = status_in_tx(&tx, &scope, &window).unwrap();
    // 10 input tokens at $1/M; the 10-token output ceiling is not charged.
    assert_eq!(view.used_picos, "10000000");
    assert_eq!(view.pending_risk_picos, "0");
    assert_eq!(view.metering_status, "conservative");
}

#[test]
fn value_quota_owner_reject_policy_refuses_unpriced_models_before_send() {
    let (mut conn, scope, window, mut quota, revision) = setup();
    quota.unpriced_usage = UnpricedUsage::Reject;
    let tx = conn.transaction().unwrap();
    let error = admit_in_tx(
        &tx,
        &Admission {
            effective_model: "unpriced-model",
            ..retention_admission(&scope, &window, &quota, &revision, "rejected", 1)
        },
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<PriceRejection>().unwrap().reason_code,
        "model_unpriced"
    );
    let rows: i64 = tx
        .query_row("SELECT count(*) FROM value_quota_attempts", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 0);
    // The default policy still serves and meters it at $0.
    quota.unpriced_usage = UnpricedUsage::Zero;
    admit_in_tx(
        &tx,
        &Admission {
            effective_model: "unpriced-model",
            ..retention_admission(&scope, &window, &quota, &revision, "served", 1)
        },
    )
    .unwrap();
}

#[test]
fn value_quota_alias_trim_rewrites_legacy_meta_once() {
    let (mut conn, scope, window, quota, revision) = setup();
    {
        let tx = conn.transaction().unwrap();
        admit_in_tx(
            &tx,
            &retention_admission(&scope, &window, &quota, &revision, "legacy", 1),
        )
        .unwrap();
        let key = attempt_meta_key("legacy", "1").unwrap();
        let mut meta = read_attempt_meta(&tx, "legacy", "1").unwrap().unwrap();
        meta.exact_aliases = (0..50)
            .map(|n| (format!("other-{n}"), "other".to_string()))
            .chain([("model-latest".to_string(), "model".to_string())])
            .collect();
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![key, serde_json::to_string(&meta).unwrap()],
        )
        .unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(retention::trim_alias_page(&mut conn).unwrap(), 1);
    let meta = read_attempt_meta(&conn, "legacy", "1").unwrap().unwrap();
    assert_eq!(
        meta.exact_aliases,
        BTreeMap::from([("model-latest".to_string(), "model".to_string())])
    );
    // Done: later runs are no-ops.
    assert_eq!(retention::trim_alias_page(&mut conn).unwrap(), 0);
}

#[test]
fn value_quota_snapshot_collection_keeps_newest_referenced_and_recent() {
    let (mut conn, scope, window, quota, revision) = setup();
    let mut revisions = Vec::new();
    {
        let tx = conn.transaction().unwrap();
        let base: PricingSnapshot = serde_json::from_str(
            &tx.query_row(
                "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
                [&revision],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
        )
        .unwrap();
        for n in 1..=4i64 {
            let mut snapshot = base.clone();
            snapshot
                .models
                .get_mut("model")
                .unwrap()
                .rates
                .get_mut("standard/base")
                .unwrap()
                .input = Some((1_000_000 + n).to_string());
            let r = snapshot.revision().unwrap();
            install_in_tx(&tx, &scope, &r, &snapshot, n * DAY_MS).unwrap();
            revisions.push(r);
        }
        // The setup revision is referenced by a retained attempt.
        admit_in_tx(
            &tx,
            &retention_admission(&scope, &window, &quota, &revision, "kept", 1),
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let deleted = retention::collect_snapshots(&mut conn, 10 * DAY_MS).unwrap();
    assert_eq!(deleted, 1);
    let exists = |r: &str| -> bool {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM value_pricing_snapshots WHERE revision=?1)",
            [r],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert!(exists(&revision));
    assert!(!exists(&revisions[0]));
    assert!(revisions[1..].iter().all(|r| exists(r)));
    // Nothing is collected while it is still recent.
    assert_eq!(retention::collect_snapshots(&mut conn, 0).unwrap(), 0);
}

#[test]
fn value_quota_natural_boundary_normalizes_live_and_shadow_without_reassigning_account_cycles() {
    let (conn, scope, mut window, quota, revision) = setup();
    let snapshot: PricingSnapshot = serde_json::from_str(
        &conn
            .query_row(
                "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
                [&revision],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
    )
    .unwrap();
    let dir = std::env::temp_dir().join(format!("cc-quota-boundary-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&dir).unwrap();
    refresh_shadow(
        &dir,
        ShadowImportInput {
            providers: &Default::default(),
            accounts: &Default::default(),
            shares: &Default::default(),
            usage: &Default::default(),
        },
    )
    .unwrap();
    let marker = activate_authority(&dir).unwrap();
    install_snapshot(&dir, &scope, &revision, &snapshot, 0).unwrap();
    activate_snapshot(&dir, &scope, &revision).unwrap();
    cover_from(&dir, &scope, 0).unwrap();
    window.starts_at_ms = DAY_MS;
    window.metered_from_ms = DAY_MS;
    window.ends_at_ms = Some(2 * DAY_MS);
    window.id = "day:new".into();
    let request_start = DAY_MS - 10;
    let admission = retention_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "boundary-live",
        request_start,
    );
    admit(&dir, &admission).unwrap();
    admit(&dir, &admission).unwrap();
    let mut shadow = retention_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "boundary-shadow",
        request_start,
    );
    shadow.enforce = false;
    evidence::admit_shadow(&dir, &shadow, true).unwrap();
    let mut db = open_database(&database_path(&dir), false).unwrap();
    for request in ["boundary-live", "boundary-shadow"] {
        let (attempt_at,fact_at):(i64,i64)=db.query_row("SELECT a.started_at_ms,f.started_at_ms FROM value_quota_attempts a JOIN quota_metering_facts f USING(request_id,attempt_id) WHERE a.request_id=?1",[request],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!((attempt_at, fact_at), (DAY_MS, DAY_MS));
    }
    // A late log retains the normalized USD clock and the original Token clock.
    let tx = db.transaction().unwrap();
    let mut log = log_for(&scope, "boundary-live");
    log.started_at_ms = request_start as u128;
    log.created_at_ms = (DAY_MS + 10) as u128;
    log.usage_revision = 1;
    evidence::capture_log(&tx, &log, None).unwrap();
    let clocks:(i64,i64)=tx.query_row("SELECT started_at_ms,accounted_at_ms FROM quota_metering_facts WHERE request_id='boundary-live'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(clocks, (DAY_MS, DAY_MS + 10));
    tx.commit().unwrap();
    drop(db);
    window.account = Some(crate::domain::sharing::quota_cycle::AccountQuotaCycle {
        stream_incarnation: "stream".into(),
        source_ref: "source".into(),
        source_epoch: 1,
        bucket_id: "bucket".into(),
        cycle_id: "cycle".into(),
        cycle_sequence: 1,
        cycle_revision: 1,
        starts_at_ms: DAY_MS,
        ends_at_ms: Some(2 * DAY_MS),
        basis: crate::domain::sharing::quota_cycle::QuotaTimeBasis::UpstreamWindow,
        observed_at_ms: DAY_MS,
        event_id: "event".into(),
    });
    let mut account = retention_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "boundary-account",
        request_start,
    );
    account.attribution = Some(crate::domain::usage::quota_ledger::QuotaAttribution {
        source_ref: "source".into(),
        source_epoch: 1,
        bucket_id: "bucket".into(),
        started_at_ms: request_start,
    });
    assert!(admit(&dir, &account)
        .unwrap_err()
        .to_string()
        .contains("WINDOW_CONFLICT"));
    account.request_id = "boundary-account-shadow";
    account.enforce = false;
    evidence::admit_shadow(&dir, &account, true).unwrap();
    let db = open_database(&database_path(&dir), false).unwrap();
    let original:i64=db.query_row("SELECT started_at_ms FROM quota_metering_facts WHERE request_id='boundary-account-shadow'",[],|r|r.get(0)).unwrap();
    assert_eq!(
        original, request_start,
        "account attribution must never move into another source window"
    );
    drop(db);
    // Even this empty fixture creates a readonly migration backup. Restore
    // owner access to its directory so non-root runners can remove the fixture.
    let backup = dir.join(marker.legacy_backup_dir.unwrap());
    assert!(std::fs::metadata(&backup).unwrap().permissions().readonly());
    set_private_directory_permissions(&backup).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn value_quota_calendar_admission_keeps_source_clock_for_conversion_pending_and_repricing() {
    use crate::domain::sharing::quota_cycle::{AccountQuotaCycle, QuotaTimeBasis};
    let (mut conn, scope, mut calendar, quota, revision) = setup();
    calendar.starts_at_ms = DAY_MS;
    calendar.metered_from_ms = DAY_MS;
    calendar.ends_at_ms = Some(2 * DAY_MS);
    calendar.id = "calendar:new".into();
    let cycle = AccountQuotaCycle {
        stream_incarnation: "stream".into(),
        source_ref: "source".into(),
        source_epoch: 1,
        bucket_id: "bucket".into(),
        cycle_id: "old".into(),
        cycle_sequence: 1,
        cycle_revision: 1,
        starts_at_ms: 0,
        ends_at_ms: Some(DAY_MS),
        basis: QuotaTimeBasis::UpstreamWindow,
        observed_at_ms: 0,
        event_id: "event".into(),
    };
    let old = Window {
        account: Some(cycle.clone()),
        id: "account:old".into(),
        starts_at_ms: 0,
        ends_at_ms: Some(DAY_MS),
        metered_from_ms: 0,
    };
    let new = Window {
        account: Some(AccountQuotaCycle {
            starts_at_ms: DAY_MS,
            ends_at_ms: Some(2 * DAY_MS),
            cycle_id: "new".into(),
            cycle_sequence: 2,
            ..cycle
        }),
        id: "account:new".into(),
        ..calendar.clone()
    };
    let original_start = DAY_MS - 10;
    let attributed = |request, model| Admission {
        attribution: Some(crate::domain::usage::quota_ledger::QuotaAttribution {
            source_ref: "source".into(),
            source_epoch: 1,
            bucket_id: "bucket".into(),
            started_at_ms: original_start,
        }),
        enforce: false,
        effective_model: model,
        // This is the frozen admission after crossing the calendar boundary.
        ..retention_admission(&scope, &calendar, &quota, &revision, request, DAY_MS)
    };
    let final_log = |request, model| {
        let mut log = unpriced_log(&scope, request, model, Some((100, 0)));
        log.started_at_ms = original_start as u128;
        log.created_at_ms = (DAY_MS + 10) as u128;
        log.quota_attribution = attributed(request, model).attribution;
        log
    };
    let tx = conn.transaction().unwrap();
    for request in ["clock-settled", "clock-pending"] {
        let a = attributed(request, "model");
        evidence::record_admission(&tx, &a, true).unwrap();
        admit_in_tx(&tx, &a).unwrap();
    }
    let log = final_log("clock-settled", "model");
    evidence::capture_log(&tx, &log, None).unwrap();
    settle_log_in_tx(&tx, &log, false).unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &calendar).unwrap().used_picos,
        "100000000"
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &old).unwrap().used_picos,
        "100000000"
    );
    assert_eq!(status_in_tx(&tx, &scope, &new).unwrap().used_picos, "0");
    let old_proof = conversion::CoverageProof {
        from_ms: 0,
        until_ms: DAY_MS,
        tokens: 100,
        requests: 1,
        minimum_tokens: 100,
    };
    let new_proof = conversion::CoverageProof {
        from_ms: DAY_MS,
        until_ms: 2 * DAY_MS,
        tokens: 0,
        requests: 0,
        minimum_tokens: 0,
    };
    let preview = conversion::preview_in_tx(&tx, &scope, &old, &old_proof).unwrap();
    assert!(preview.complete && preview.pending_complete);
    assert_eq!(preview.pending_tokens, 20);
    assert_eq!(preview.status.pending_risk_picos, "20000000");
    let preview = conversion::preview_in_tx(&tx, &scope, &new, &new_proof).unwrap();
    assert!(preview.complete && preview.pending_complete);
    assert_eq!(preview.pending_tokens, 0);
    assert_eq!(preview.status.pending_risk_picos, "0");
    let mut change = conversion_change(&scope, &old, "clock-conversion", 100, 1, "usdEquivalent");
    change.proof = old_proof;
    conversion::apply_in_tx(&tx, &[change]).unwrap();
    let risk: String = tx
        .query_row(
            "SELECT pending_risk_picos FROM value_quota_attempts WHERE request_id='clock-pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        risk, "20000000",
        "conversion reserves the old source cycle's live attempt"
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &new).unwrap().pending_risk_picos,
        "0"
    );

    // An unmaterialized fact belongs to the same original source cycle.
    evidence::record_admission(&tx, &attributed("clock-missing", "model"), true).unwrap();
    assert_eq!(evidence::missing_in(&tx, &scope, &old).unwrap(), 1);
    assert_eq!(evidence::missing_in(&tx, &scope, &new).unwrap(), 0);
    assert!(
        conversion::preview_in_tx(&tx, &scope, &new, &new_proof)
            .unwrap()
            .complete
    );
    assert!(evidence::materialize(&tx, &scope, "clock-missing", "1", &revision).unwrap());

    let a = attributed("clock-unpriced", "late-model");
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_in_tx(&tx, &a).unwrap();
    let log = final_log("clock-unpriced", "late-model");
    evidence::capture_log(&tx, &log, None).unwrap();
    settle_log_in_tx(&tx, &log, false).unwrap();
    assert_eq!(unpriced_summary_in(&tx, &scope, &old).unwrap().requests, 1);
    assert_eq!(unpriced_summary_in(&tx, &scope, &new).unwrap().requests, 0);
    let covering = activate_covering_tariff(&tx, &scope, &revision);
    let windows = BTreeMap::from([(scope.key().unwrap(), new)]);
    assert_eq!(
        reprice::reprice_page_in_tx(&tx, &scope, &windows, 100)
            .unwrap()
            .repriced,
        0
    );
    reprice::mark_due(&tx, &scope, &covering).unwrap();
    let windows = BTreeMap::from([(scope.key().unwrap(), old)]);
    assert_eq!(
        reprice::reprice_page_in_tx(&tx, &scope, &windows, 100)
            .unwrap()
            .repriced,
        1
    );
    let mut query = tx.prepare("EXPLAIN QUERY PLAN SELECT sum(quota_tokens) FROM quota_metering_facts WHERE scope=?1 AND account_key=?2 AND account_started_at_ms>=?3").unwrap();
    let plan = query
        .query_map(
            params![scope.key().unwrap(), calendar.account_key().unwrap(), 0],
            |r| r.get::<_, String>(3),
        )
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("quota_facts_account_idx"), "{plan}");
}

#[test]
fn value_quota_v8_upgrade_preserves_evidence_and_derives_the_original_account_clock() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    let mut a = retention_admission(&scope, &window, &quota, &revision, "v8-clock", 100);
    a.attribution = Some(crate::domain::usage::quota_ledger::QuotaAttribution {
        source_ref: "source".into(),
        source_epoch: 1,
        bucket_id: "bucket".into(),
        started_at_ms: 90,
    });
    evidence::record_admission(&tx, &a, true).unwrap();
    let before: (String, String) = tx
        .query_row(
            "SELECT fact_json,fact_sha256 FROM quota_metering_facts WHERE request_id='v8-clock'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    tx.commit().unwrap();
    // Recreate the published v8 layout without touching its migration/checksum.
    conn.execute_batch("DROP INDEX value_quota_status_time_idx;
        DROP INDEX value_quota_uncompacted_time_idx;
        DROP INDEX quota_facts_account_idx;
        ALTER TABLE quota_metering_facts DROP COLUMN account_started_at_ms;
        CREATE INDEX quota_facts_account_idx ON quota_metering_facts(scope,account_key,started_at_ms,request_id,attempt_id);
        ALTER TABLE value_quota_attempts DROP COLUMN quota_enforced;
        DELETE FROM schema_migrations WHERE version>=9; PRAGMA user_version=8;").unwrap();
    super::super::schema::migrate_to_current(&conn).unwrap();
    super::super::schema::migrate_to_current(&conn).unwrap();
    let after: (String, String, i64, i64) = conn.query_row("SELECT fact_json,fact_sha256,started_at_ms,account_started_at_ms FROM quota_metering_facts WHERE request_id='v8-clock'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!((after.0, after.1), before);
    assert_eq!((after.2, after.3), (100, 90));
}

#[test]
fn value_quota_no_output_guard_log_and_unpriced_history_keep_identical_input_only_usage() {
    for path in ["guard", "log", "history"] {
        let (mut conn, scope, window, quota, revision) = resource_fixture();
        let tx = conn.transaction().unwrap();
        let a = resource_admission(
            &scope,
            &window,
            &quota,
            &revision,
            path,
            ("image.output", 2),
            true,
        );
        evidence::record_admission(&tx, &a, true).unwrap();
        if path != "history" {
            admit_in_tx(&tx, &a).unwrap();
        }
        if path == "log" {
            let mut log = log_for(&scope, path);
            log.status_code = 503;
            log.failure_kind = Some("upstream_error_no_output".into());
            log.usage_revision = 1;
            evidence::capture_log(&tx, &log, None).unwrap();
            settle_log_in_tx(&tx, &log, false).unwrap();
        } else {
            settle_attempt_outcome_in_tx(&tx, path, "1", AttemptOutcome::NoOutput).unwrap();
            if path == "history" {
                assert!(evidence::materialize(&tx, &scope, path, "1", &revision).unwrap());
            }
        }
        let row = read_attempt_row(&tx, path, "1").unwrap().unwrap();
        assert_eq!(
            row.5, "100000000",
            "{path}: only 100 estimated input Tokens, no image or output bound"
        );
        assert_eq!(row.7, "0");
        assert_eq!(row.8, "conservative");
        let fact = evidence::read_fact(&tx, path, "1").unwrap().unwrap();
        assert!(matches!(fact.outcome, ObservedOutcome::NoOutput));
        assert!(fact.resources.is_none());
        // The real result, when later available, replaces conservative evidence.
        settle_in_tx(
            &tx,
            &Observation {
                request_id: path,
                attempt_id: "1",
                scope: &scope,
                usage_revision: 10,
                effective_model: "model",
                effective_tier: "standard",
                reported_model: None,
                observed_tier: None,
                tokens: Some(ValueTokens {
                    fresh_input: 10,
                    output: 5,
                    ..Default::default()
                }),
                resources: Some(crate::domain::usage::value_pricing::ResourceUsage {
                    response_id: None,
                    quantities: BTreeMap::from([("image.output".into(), 1)]),
                    source: "upstream.protocol".into(),
                    estimated: false,
                    version: crate::domain::usage::value_pricing::RESOURCE_ESTIMATE_VERSION.into(),
                }),
                outcome: ObservedOutcome::Final,
                synthetic: false,
            },
        )
        .unwrap();
        assert_eq!(
            read_attempt_row(&tx, path, "1").unwrap().unwrap().5,
            "1015000000"
        );
    }
}

#[test]
fn value_quota_archive_pages_resume_and_keep_the_maintenance_queries_indexed() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    for n in 0..205 {
        let request = format!("page-{n:03}");
        let mut a = retention_admission(&scope, &window, &quota, &revision, &request, 1);
        a.enforce = false;
        admit_in_tx(&tx, &a).unwrap();
        settle_outcome_in_tx(&tx, &request, "1", AttemptOutcome::NoOutput, true).unwrap();
        let mut meta = read_attempt_meta(&tx, &request, "1").unwrap().unwrap();
        meta.exact_aliases = (0..100)
            .map(|n| (format!("alias-{n:03}"), "model".into()))
            .collect();
        tx.execute(
            "UPDATE meta SET value=?2 WHERE key=?1",
            params![
                attempt_meta_key(&request, "1").unwrap(),
                serde_json::to_string(&meta).unwrap()
            ],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    for expected in [100, 100, 5, 0, 0] {
        assert_eq!(
            retention::archive_meta_page(&mut conn, DAY_MS).unwrap(),
            expected
        );
    }
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM meta WHERE key LIKE 'value_attempt:%' AND value LIKE 'zstd-v1:%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 205);
    for (query,index) in [
        ("EXPLAIN QUERY PLAN SELECT request_id FROM value_quota_attempts WHERE metering_status='observed' AND started_at_ms<10 AND (started_at_ms,request_id,attempt_id)>(0,'','') ORDER BY started_at_ms,request_id,attempt_id LIMIT 100","value_quota_status_time_idx"),
        ("EXPLAIN QUERY PLAN SELECT request_id FROM value_quota_attempts WHERE metering_status IN ('observed','conservative') AND started_at_ms<10 AND compacted_at_ms IS NULL ORDER BY started_at_ms,request_id,attempt_id LIMIT 100","value_quota_uncompacted_time_idx")
    ] {
        let plan:Vec<String>=conn.prepare(query).unwrap().query_map([],|r|r.get(3)).unwrap().map(Result::unwrap).collect();
        assert!(plan.iter().any(|line|line.contains(index)),"{plan:?}");
    }
}
