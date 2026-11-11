// Compiled locally; executed by CI. No upstream/network/model requests.
fn conversion_fixture() -> (Connection, Scope, Window, ValueQuota, String) {
    let (mut conn, mut scope, w, mut quota, revision) = setup();
    scope.installation_id = format!(
        "conversion-{}",
        crate::domain::sharing::quota_cycle::opaque_id()
    );
    conn.execute(
        "INSERT INTO value_quota_coverage VALUES(?1,0)",
        [scope.key().unwrap()],
    )
    .unwrap();
    let json: String = conn
        .query_row(
            "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
            [revision],
            |r| r.get(0),
        )
        .unwrap();
    let mut snapshot: PricingSnapshot = serde_json::from_str(&json).unwrap();
    snapshot
        .models
        .get_mut("model")
        .unwrap()
        .rates
        .get_mut("standard/base")
        .unwrap()
        .input = Some("1400000".into());
    let revision = snapshot.revision().unwrap();
    let tx = conn.transaction().unwrap();
    install_in_tx(&tx, &scope, &revision, &snapshot, 0).unwrap();
    tx.commit().unwrap();
    quota.limit_usd_micros = "100000000".into();
    (conn, scope, w, quota, revision)
}
fn conversion_admission<'a>(
    scope: &'a Scope,
    w: &'a Window,
    quota: &'a ValueQuota,
    revision: &'a str,
    request: &'a str,
) -> Admission<'a> {
    Admission {
        attribution: w.account.as_ref().map(|c| {
            crate::domain::usage::quota_ledger::QuotaAttribution {
                source_ref: c.source_ref.clone(),
                source_epoch: c.source_epoch,
                bucket_id: c.bucket_id.clone(),
                started_at_ms: 1,
            }
        }),
        enforce: false,
        scope,
        window: w,
        policy_epoch: "shadow",
        quota,
        pricing_revision: revision,
        request_id: request,
        attempt_id: "1",
        effective_model: "model",
        effective_tier: "standard",
        started_at_ms: 1,
        estimate: ESTIMATE,
        resource_plan: None,
        parallel_limit: 4,
    }
}
fn conversion_log(tx: &Transaction<'_>, a: &Admission<'_>, tokens: u64) {
    evidence::record_admission(tx, a, true).unwrap();
    admit_with_trial_in_tx(tx, a, None).unwrap();
    let mut log = log_for(a.scope, a.request_id);
    log.attempt_count = 1;
    log.started_at_ms = 1;
    log.created_at_ms = 2;
    log.input_tokens = Some(tokens);
    log.output_tokens = Some(0);
    log.raw_input_tokens = Some(tokens);
    log.total_tokens = Some(tokens);
    log.usage_state = crate::domain::usage::store::UsageState::Observed;
    log.cache_read_tokens = Some(0);
    log.cache_creation_tokens = Some(0);
    log.usage_revision = 1;
    log.quota_attribution = a.attribution.clone();
    evidence::capture_log(tx, &log, None).unwrap();
    settle_log_in_tx(tx, &log, false).unwrap();
}
fn conversion_change(
    scope: &Scope,
    w: &Window,
    operation: &str,
    tokens: u64,
    requests: u64,
    target: &str,
) -> conversion::Conversion {
    conversion::Conversion {
        scope: scope.clone(),
        window: w.clone(),
        proof: conversion::CoverageProof {
            from_ms: 0,
            until_ms: 10,
            tokens,
            requests,
            minimum_tokens: tokens,
        },
        require_history: target == "usdEquivalent",
        non_token_basis: None,
        operation_id: operation.into(),
        payload_sha256: operation.into(),
        source_unit: if target == "tokens" {
            "usdEquivalent"
        } else {
            "tokens"
        }
        .into(),
        target_unit: target.into(),
        target_limit: Some("100000000".into()),
        target_offset: None,
        source_adjustment: None,
        target_adjustment: None,
        source_offset: "0".into(),
        actor: "owner@example.test".into(),
        reason: "Audited conversion".into(),
        now: 3,
    }
}
#[test]
fn conversion_carries_30m_as_42_dollars_and_keeps_intervening_token_consumption() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    conversion_log(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "first"),
        30_000_000,
    );
    let c = conversion_change(&scope, &w, "to-usd", 30_000_000, 1, "usdEquivalent");
    let preview = conversion::preview_in_tx(&tx, &scope, &w, &c.proof).unwrap();
    assert!(preview.complete);
    assert_eq!(preview.status.used_picos, "42000000000000");
    conversion::apply_in_tx(&tx, std::slice::from_ref(&c)).unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        "42000000000000"
    );
    assert_eq!(
        quota.limit_picos().unwrap() - 42_000_000_000_000,
        58_000_000_000_000
    );
    conversion::apply_in_tx(
        &tx,
        &[conversion_change(
            &scope, &w, "to-token", 30_000_000, 1, "tokens",
        )],
    )
    .unwrap();
    conversion_log(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "during-token"),
        10_000_000,
    );
    conversion::apply_in_tx(
        &tx,
        &[conversion_change(
            &scope,
            &w,
            "again-usd",
            40_000_000,
            2,
            "usdEquivalent",
        )],
    )
    .unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        "56000000000000"
    );
    // Replaying never adds usage and cannot accept a different payload.
    conversion::apply_in_tx(&tx, std::slice::from_ref(&c)).unwrap();
    let mut different = c;
    different.payload_sha256 = "changed".into();
    assert!(conversion::apply_in_tx(&tx, &[different]).is_err());
    assert_eq!(
        tx.query_row("SELECT count(*) FROM quota_conversion_receipts", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        3
    );
}
#[test]
fn conversion_allows_exhausted_target_and_new_admission_rejects_immediately() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    conversion_log(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "spent"),
        100_000_000,
    );
    conversion::apply_in_tx(
        &tx,
        &[conversion_change(
            &scope,
            &w,
            "exhausted",
            100_000_000,
            1,
            "usdEquivalent",
        )],
    )
    .unwrap();
    let a = conversion_admission(&scope, &w, &quota, &revision, "next");
    let error = admit_with_trial_in_tx(&tx, &Admission { enforce: true, ..a }, None).unwrap_err();
    assert_eq!(
        error.downcast_ref::<ValueQuotaRejection>().unwrap().kind,
        ValueQuotaRejectionKind::Exhausted
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        "140000000000000"
    );
}
#[test]
fn conversion_preview_promotes_the_same_live_risk_as_commit() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    let a = conversion_admission(&scope, &w, &quota, &revision, "live");
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().pending_risk_picos,
        "0"
    );
    let c = conversion_change(&scope, &w, "pending", 0, 0, "usdEquivalent");
    let p = conversion::preview_in_tx(&tx, &scope, &w, &c.proof).unwrap();
    assert!(p.complete && p.pending_complete);
    assert_ne!(p.status.pending_risk_picos, "0");
    assert!(p.pending_tokens > 0);
    conversion::apply_in_tx(&tx, &[c]).unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().pending_risk_picos,
        p.status.pending_risk_picos
    );
}
#[test]
fn legacy_pending_without_token_facts_cannot_release_risk_on_unit_change() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    // A v5 attempt has a frozen monetary reservation but no v6 Token fact.
    let a = conversion_admission(&scope, &w, &quota, &revision, "legacy-live");
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    let c = conversion_change(&scope, &w, "legacy-to-token", 0, 0, "tokens");
    let p = conversion::preview_in_tx(&tx, &scope, &w, &c.proof).unwrap();
    assert!(!p.pending_complete && !p.complete);
    assert!(p.missing_facts > 0);
    assert!(conversion::apply_in_tx(&tx, std::slice::from_ref(&c)).is_err());
    assert_eq!(
        tx.query_row("SELECT count(*) FROM quota_conversion_receipts", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        0
    );
    // Capturing the independently verified bound makes the same change safe.
    evidence::record_admission(&tx, &a, true).unwrap();
    let p = conversion::preview_in_tx(&tx, &scope, &w, &c.proof).unwrap();
    assert!(p.pending_complete && p.pending_tokens > 0);
    conversion::apply_in_tx(&tx, &[c]).unwrap();
}
#[test]
fn generation_outcome_fact_survives_a_monetary_settlement_failure() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    let a = conversion_admission(&scope, &w, &quota, &revision, "outcome-fault");
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    tx.execute_batch("CREATE TRIGGER reject_outcome_price BEFORE UPDATE ON value_quota_attempts BEGIN SELECT RAISE(ABORT,'fixture settlement unavailable'); END;").unwrap();
    assert!(!settle_attempt_outcome_in_tx(
        &tx,
        a.request_id,
        a.attempt_id,
        AttemptOutcome::NotSent
    )
    .unwrap());
    tx.commit().unwrap();
    let outcome: String = conn.query_row(
        "SELECT json_extract(fact_json,'$.outcome') FROM quota_metering_facts WHERE request_id='outcome-fault'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(outcome, "free");
    assert!(window_blocked(&conn, &scope.key().unwrap(), &w.id).unwrap());
}
#[test]
fn token_conversion_rejects_rollups_below_retained_token_facts_or_grant_totals() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    conversion_log(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "retained-tokens"),
        30_000_000,
    );
    let mut c = conversion_change(&scope, &w, "lost-token-rollup", 0, 0, "tokens");
    assert!(
        !conversion::preview_in_tx(&tx, &scope, &w, &c.proof)
            .unwrap()
            .token_history_complete
    );
    assert!(conversion::apply_in_tx(&tx, std::slice::from_ref(&c)).is_err());
    c.proof.tokens = 30_000_000;
    c.proof.requests = 1;
    c.proof.minimum_tokens = 40_000_000;
    assert!(conversion::apply_in_tx(&tx, std::slice::from_ref(&c)).is_err());
    c.proof.minimum_tokens = 30_000_000;
    conversion::apply_in_tx(&tx, &[c]).unwrap();
}
#[test]
fn token_conversion_rechecks_calibration_evidence_in_the_commit_transaction() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    let a = Admission {
        enforce: true,
        ..conversion_admission(&scope, &w, &quota, &revision, "late-conservative")
    };
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    let c = conversion_change(&scope, &w, "late-token", 0, 0, "tokens");
    assert!(
        conversion::preview_in_tx(&tx, &scope, &w, &c.proof)
            .unwrap()
            .pending_complete
    );
    settle_attempt_outcome_in_tx(&tx, a.request_id, a.attempt_id, AttemptOutcome::Unknown).unwrap();
    let error = conversion::apply_in_tx(&tx, &[c]).unwrap_err();
    assert!(
        error.to_string().contains("QUOTA_CONVERSION_PREVIEW_STALE"),
        "{error}"
    );
}
#[test]
fn conversion_missing_prices_gaps_and_lost_rollups_cannot_become_zero() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    let a = conversion_admission(&scope, &w, &quota, &revision, "unpriced");
    evidence::record_admission(&tx, &a, false).unwrap();
    let c = conversion_change(&scope, &w, "missing", 0, 0, "usdEquivalent");
    assert!(
        !conversion::preview_in_tx(&tx, &scope, &w, &c.proof)
            .unwrap()
            .complete
    );
    assert!(conversion::apply_in_tx(&tx, std::slice::from_ref(&c)).is_err());
    evidence::outcome(&tx, "unpriced", "1", AttemptOutcome::NotSent).unwrap();
    let mut lost = c;
    lost.proof.minimum_tokens = 1;
    assert!(
        !conversion::preview_in_tx(&tx, &scope, &w, &lost.proof)
            .unwrap()
            .complete
    );
    assert_eq!(
        tx.query_row("SELECT count(*) FROM quota_conversion_receipts", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        0
    );
}
#[test]
fn conversion_failure_rolls_back_the_entire_batch_and_adjustment_mapping() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    {
        let tx = conn.transaction().unwrap();
        conversion_log(
            &tx,
            &conversion_admission(&scope, &w, &quota, &revision, "first"),
            30_000_000,
        );
        tx.commit().unwrap();
    }
    {
        let tx = conn.transaction().unwrap();
        let mut first = conversion_change(&scope, &w, "batch", 30_000_000, 1, "usdEquivalent");
        first.target_offset = Some("1000000000000".into());
        first.source_adjustment = Some("token:manual".into());
        first.source_offset = "100".into();
        let mut other = scope.clone();
        other.email = "other@example.test".into();
        let invalid = conversion_change(&other, &w, "batch", 99, 1, "usdEquivalent");
        assert!(conversion::apply_in_tx(&tx, &[first, invalid]).is_err());
        // This is the Share commit transaction, so dropping it rolls back all users.
    }
    assert_eq!(
        conn.query_row("SELECT count(*) FROM quota_conversion_receipts", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT count(*) FROM quota_adjustment_mappings", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        status_in_tx(&conn, &scope, &w).unwrap().used_picos,
        "42000000000000"
    );
}
#[test]
fn frozen_minimum_survives_compaction_without_the_catalog_and_allows_late_revision() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    conversion_log(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "late"),
        30_000_000,
    );
    compact_in_tx(&tx, 3, 4).unwrap();
    // Clear the immutable snapshot cache too: the durable minimal tariff is sufficient.
    tx.execute("DELETE FROM value_pricing_snapshots", [])
        .unwrap();
    snapshot_cache().lock().unwrap().remove(&(
        scope.router_url.clone(),
        scope.installation_id.clone(),
        revision.clone(),
    ));
    let mut log = log_for(&scope, "late");
    log.attempt_count = 1;
    log.usage_revision = 2;
    log.usage_state = crate::domain::usage::store::UsageState::Observed;
    log.input_tokens = Some(40_000_000);
    log.output_tokens = Some(0);
    log.raw_input_tokens = Some(40_000_000);
    log.total_tokens = Some(40_000_000);
    settle_log_in_tx(&tx, &log, false).unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        "56000000000000"
    );
}
#[test]
fn account_conversion_isolates_source_epoch_and_bucket_with_overlapping_time() {
    use crate::domain::sharing::quota_cycle::{AccountQuotaCycle, QuotaTimeBasis};
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    let cycle = AccountQuotaCycle {
        stream_incarnation: "stream".into(),
        source_ref: "source".into(),
        source_epoch: 1,
        bucket_id: "bucket".into(),
        cycle_id: "cycle".into(),
        cycle_sequence: 1,
        cycle_revision: 1,
        starts_at_ms: 0,
        ends_at_ms: w.ends_at_ms,
        basis: QuotaTimeBasis::UpstreamWindow,
        observed_at_ms: 0,
        event_id: "event".into(),
    };
    let account = Window {
        account: Some(cycle.clone()),
        id: "account:source:1:bucket:cycle".into(),
        ..w.clone()
    };
    conversion_log(
        &tx,
        &conversion_admission(&scope, &account, &quota, &revision, "account-a"),
        30_000_000,
    );
    let other = Window {
        account: Some(AccountQuotaCycle {
            source_epoch: 2,
            ..cycle
        }),
        id: "account:source:2:bucket:cycle".into(),
        ..w
    };
    conversion_log(
        &tx,
        &conversion_admission(&scope, &other, &quota, &revision, "account-b"),
        10_000_000,
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &account).unwrap().used_picos,
        "42000000000000"
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &other).unwrap().used_picos,
        "14000000000000"
    );
    let c = conversion_change(&scope, &account, "account", 30_000_000, 1, "usdEquivalent");
    assert!(
        conversion::preview_in_tx(&tx, &scope, &account, &c.proof)
            .unwrap()
            .complete
    );
    conversion::apply_in_tx(&tx, &[c]).unwrap();
}
#[test]
fn conversion_mapping_is_reciprocal_and_pending_without_bound_is_blocked() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    conversion_log(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "first"),
        30_000_000,
    );
    let mut c = conversion_change(&scope, &w, "mapping", 30_000_000, 1, "usdEquivalent");
    c.source_adjustment = Some("token:source".into());
    c.target_offset = Some("2000000000000".into());
    c.source_offset = "500".into();
    conversion::apply_in_tx(&tx, &[c]).unwrap();
    let id = conversion::latest_adjustment(&tx, &scope, &w)
        .unwrap()
        .unwrap();
    let reciprocal:String=tx.query_row("SELECT target_offset FROM quota_adjustment_mappings WHERE source_adjustment=?1 AND target_unit='tokens'",[id],|r|r.get(0)).unwrap();
    assert_eq!(reciprocal, "500");
    let a = Admission {
        estimate: RiskEstimate::default(),
        ..conversion_admission(&scope, &w, &quota, &revision, "unbounded")
    };
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    assert!(
        !conversion::preview_in_tx(
            &tx,
            &scope,
            &w,
            &conversion_change(&scope, &w, "bound", 30_000_000, 1, "usdEquivalent").proof
        )
        .unwrap()
        .complete
    );
}

#[test]
fn preparation_resumes_pages_and_revisits_completed_unpriced_evidence() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    {
        let tx = conn.transaction().unwrap();
        for n in 0..501 {
            let request = format!("history-{n:04}");
            let a = conversion_admission(&scope, &w, &quota, &revision, &request);
            evidence::record_admission(&tx, &a, true).unwrap();
            let mut log = log_for(&scope, &request);
            log.started_at_ms = 1;
            log.created_at_ms = 2;
            log.attempt_count = 1;
            log.input_tokens = Some(1);
            log.output_tokens = Some(0);
            log.raw_input_tokens = Some(1);
            log.total_tokens = Some(1);
            log.cache_read_tokens = Some(0);
            log.cache_creation_tokens = Some(0);
            log.usage_revision = 1;
            log.usage_state = crate::domain::usage::store::UsageState::Observed;
            evidence::capture_log(&tx, &log, None).unwrap();
        }
        // The first bounded page only traverses the empty retained-log set.
        assert_eq!(
            evidence::prepare_page_in_tx(&tx, &scope, false, 3, revision.clone())
                .unwrap()
                .state,
            "preparing"
        );
        assert_eq!(
            evidence::prepare_page_in_tx(&tx, &scope, false, 3, revision.clone())
                .unwrap()
                .state,
            "preparing"
        );
        assert_eq!(
            tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
                .get::<_, u64>(0))
                .unwrap(),
            500
        );
        tx.commit().unwrap();
    }
    // A new transaction resumes the durable cursor instead of pricing twice.
    let tx = conn.transaction().unwrap();
    assert_eq!(
        evidence::prepare_page_in_tx(&tx, &scope, false, 4, revision.clone())
            .unwrap()
            .state,
        "complete"
    );
    let proof = conversion_change(&scope, &w, "prepared", 501, 501, "usdEquivalent").proof;
    let preview = conversion::preview_in_tx(&tx, &scope, &w, &proof).unwrap();
    assert!(preview.complete);
    assert_eq!(
        preview.status.used_picos,
        (501_i128 * 1_400_000).to_string()
    );
    // Previously complete preparation can be explicitly retried, including
    // facts lexically before the old cursor (e.g. late persisted usage).
    evidence::record_admission(
        &tx,
        &conversion_admission(&scope, &w, &quota, &revision, "000-late"),
        true,
    )
    .unwrap();
    evidence::outcome(&tx, "000-late", "1", AttemptOutcome::NotSent).unwrap();
    assert_eq!(
        evidence::prepare_page_in_tx(&tx, &scope, false, 5, revision.clone())
            .unwrap()
            .state,
        "preparing"
    );
    assert_eq!(
        evidence::prepare_page_in_tx(&tx, &scope, false, 5, revision.clone())
            .unwrap()
            .state,
        "preparing"
    );
    assert_eq!(
        evidence::prepare_page_in_tx(&tx, &scope, false, 5, revision)
            .unwrap()
            .state,
        "complete"
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        (501_i128 * 1_400_000).to_string()
    );
}

#[test]
fn historical_preparation_uses_scope_indexes_with_100k_unrelated_records() {
    let (conn, scope, _, _, _) = conversion_fixture();
    conn.execute_batch("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i<100000)
        INSERT INTO usage_records SELECT printf('other-%06d',i),1,'other-share','provider',1,'{\"userEmail\":\"other@example.test\"}','fixture' FROM n;").unwrap();
    let mut query=conn.prepare("EXPLAIN QUERY PLAN SELECT request_id FROM usage_records WHERE share_id=?1 AND lower(json_extract(payload_json,'$.userEmail'))=?2 AND request_id>?3 ORDER BY request_id LIMIT 500").unwrap();
    let plan = query
        .query_map(params![scope.share_id, scope.email, ""], |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("usage_records_quota_backfill_idx"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    let mut query=conn.prepare("EXPLAIN QUERY PLAN SELECT request_id,attempt_id FROM quota_metering_facts WHERE scope=?1 AND (request_id,attempt_id)>(?2,?3) ORDER BY request_id,attempt_id LIMIT 500").unwrap();
    let plan = query
        .query_map(params![scope.key().unwrap(), "", ""], |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
        .join(" ");
    assert!(plan.contains("quota_facts_scope_request_idx"), "{plan}");
}

#[test]
fn unpriced_async_generation_keeps_ownership_and_terminal_resource_evidence() {
    use crate::domain::usage::store::InferenceOperation;
    let (mut conn, scope, w, quota, revision) = resource_fixture();
    let tx = conn.transaction().unwrap();
    let admission = resource_admission(
        &scope,
        &w,
        &quota,
        &revision,
        "unpriced-async",
        ("video.output_millisecond", 5000),
        false,
    );
    evidence::record_admission(
        &tx,
        &Admission {
            enforce: false,
            ..admission
        },
        true,
    )
    .unwrap();
    resource_tasks::attach_task_in_tx(&tx, "unpriced-async", "1", "task", 99999).unwrap();
    let mut poll = log_for(&scope, "poll");
    poll.operation = InferenceOperation::VideoStatus;
    poll.media_task_id = Some("task".into());
    poll.parent_request_id = Some("unpriced-async".into());
    poll.media_status = Some("completed".into());
    poll.value_resources = Some(crate::domain::usage::value_pricing::ResourceUsage {
        quantities: BTreeMap::from([("video.output_millisecond".into(), 3000)]),
        source: "fixture".into(),
        estimated: false,
        version: crate::domain::usage::value_pricing::RESOURCE_ESTIMATE_VERSION.into(),
    });
    let mut wrong = poll.clone();
    wrong.user_email = Some("other@example.test".into());
    assert!(resource_tasks::capture_async_fact(&tx, &wrong).is_err());
    resource_tasks::capture_async_fact(&tx, &poll).unwrap();
    assert!(resource_tasks::observe_task_in_tx(&tx, &poll).unwrap());
    assert!(!resource_tasks::observe_task_in_tx(&tx, &poll).unwrap());
    assert!(evidence::materialize(&tx, &scope, "unpriced-async", "1", &revision).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        "3000000000"
    );
    poll.media_status = Some("failed".into());
    resource_tasks::capture_async_fact(&tx, &poll).unwrap();
    assert!(!resource_tasks::observe_task_in_tx(&tx, &poll).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &w).unwrap().used_picos,
        "3000000000"
    );
}
