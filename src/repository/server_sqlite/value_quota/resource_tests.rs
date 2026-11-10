// Included in ledger tests. CI only; no live model or network access.
fn resource_fixture() -> (Connection, Scope, Window, ValueQuota, String) {
    let (mut conn, scope, window, mut quota, old) = setup();
    let json: String = conn
        .query_row(
            "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
            [&old],
            |r| r.get(0),
        )
        .unwrap();
    let mut snapshot: PricingSnapshot = serde_json::from_str(&json).unwrap();
    snapshot.schema_version = 2;
    snapshot.tariff_policy_id = crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY.into();
    snapshot.models.get_mut("model").unwrap().resources = BTreeMap::from([
        ("image.output".into(), "1000000000".into()),
        ("video.output_millisecond".into(), "1000000".into()),
        ("audio.input_token".into(), "2000000".into()),
    ]);
    let revision = snapshot.revision().unwrap();
    let tx = conn.transaction().unwrap();
    install_in_tx(&tx, &scope, &revision, &snapshot, 1).unwrap();
    tx.commit().unwrap();
    quota.tariff_policy_id = crate::domain::usage::value_pricing::RESOURCE_TARIFF_POLICY.into();
    (conn, scope, window, quota, revision)
}
fn resource_admission<'a>(
    scope: &'a Scope,
    window: &'a Window,
    quota: &'a ValueQuota,
    revision: &'a str,
    request: &'a str,
    resource: (&str, u64),
    text: bool,
) -> Admission<'a> {
    Admission {
        policy_epoch: "epoch",
        resource_plan: Some(crate::domain::usage::value_pricing::ResourcePlan {
            quantities: BTreeMap::from([(resource.0.into(), resource.1)]),
            requires_v2: true,
            text_tokens: text,
        }),
        enforce: true,
        scope,
        window,
        quota,
        pricing_revision: revision,
        request_id: request,
        attempt_id: "1",
        effective_model: "model",
        effective_tier: "standard",
        started_at_ms: 1,
        estimate: RiskEstimate {
            input_tokens: 100,
            output_tokens: 100,
        },
        parallel_limit: 4,
    }
}
#[test]
fn resource_admission_refuses_missing_prices_and_v1_before_any_attempt() {
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    let tx = conn.transaction().unwrap();
    let bad = resource_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "bad",
        ("tool.web_search", 1),
        true,
    );
    let error = admit_in_tx(&tx, &bad).unwrap_err();
    assert_eq!(
        error.downcast_ref::<PriceRejection>().unwrap().reason_code,
        "resource_unpriced"
    );
    let old = ValueQuota {
        tariff_policy_id: "api-equivalent-v1".into(),
        ..quota.clone()
    };
    let valid = resource_admission(
        &scope,
        &window,
        &old,
        &revision,
        "old",
        ("image.output", 1),
        false,
    );
    assert!(admit_in_tx(&tx, &valid)
        .unwrap_err()
        .to_string()
        .contains("RESOURCE_POLICY_REQUIRED"));
    assert_eq!(
        tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
#[test]
fn media_observation_revises_a_bound_once_and_retains_facts_after_compaction() {
    use crate::domain::usage::store::{InferenceOperation, RequestKind};
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    let tx = conn.transaction().unwrap();
    let a = resource_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "image",
        ("image.output/1024x1024/high", 3),
        false,
    );
    admit_in_tx(&tx, &a).unwrap();
    settle_outcome_in_tx(&tx, "image", "1", AttemptOutcome::Unknown, true).unwrap();
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "3000000000"
    );
    let mut log = log_for(&scope, "image");
    log.operation = InferenceOperation::ImageGeneration;
    log.request_kind = RequestKind::Image;
    log.image_count = Some(2);
    log.usage_revision = 1;
    log.attempt_count = 1;
    assert!(settle_log_in_tx(&tx, &log, false).unwrap());
    assert!(!settle_log_in_tx(&tx, &log, false).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "2000000000"
    );
    compact_in_tx(&tx, 2, 3).unwrap();
    assert!(read_attempt_meta(&tx, "image", "1").unwrap().is_some());
    log.image_count = Some(1);
    log.usage_revision += 1;
    assert!(settle_log_in_tx(&tx, &log, false).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "1000000000"
    );
}
#[test]
fn estimated_resources_keep_the_bound_until_trusted_evidence_arrives() {
    use crate::domain::usage::{
        store::{InferenceOperation, RequestKind},
        value_pricing::{ResourceUsage, RESOURCE_ESTIMATE_VERSION},
    };
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    let tx = conn.transaction().unwrap();
    admit_in_tx(
        &tx,
        &resource_admission(
            &scope,
            &window,
            &quota,
            &revision,
            "estimated-image",
            ("image.output", 3),
            false,
        ),
    )
    .unwrap();
    let mut log = log_for(&scope, "estimated-image");
    log.operation = InferenceOperation::ImageGeneration;
    log.request_kind = RequestKind::Image;
    log.attempt_count = 1;
    for (revision, count, estimated, expected) in [
        (1, 1, true, "3000000000"),
        (2, 5, true, "5000000000"),
        (3, 1, false, "1000000000"),
    ] {
        log.usage_revision = revision;
        log.value_resources = Some(ResourceUsage {
            quantities: BTreeMap::from([("image.output".into(), count)]),
            source: "upstream.protocol".into(),
            estimated,
            version: RESOURCE_ESTIMATE_VERSION.into(),
        });
        assert!(settle_log_in_tx(&tx, &log, false).unwrap());
        let status = status_in_tx(&tx, &scope, &window).unwrap();
        assert_eq!(status.used_picos, expected);
        assert_eq!(
            status.metering_status,
            if estimated {
                "conservative"
            } else {
                "observed"
            }
        );
    }
}
#[test]
fn unexpected_unpriced_resources_retain_risk_and_fence_only_their_window() {
    use crate::domain::usage::{
        store::{InferenceOperation, RequestKind},
        value_pricing::{ResourceUsage, RESOURCE_ESTIMATE_VERSION},
    };
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    let tx = conn.transaction().unwrap();
    admit_in_tx(
        &tx,
        &resource_admission(
            &scope,
            &window,
            &quota,
            &revision,
            "unexpected-resource",
            ("image.output", 1),
            false,
        ),
    )
    .unwrap();
    let mut log = log_for(&scope, "unexpected-resource");
    log.operation = InferenceOperation::ImageGeneration;
    log.request_kind = RequestKind::Image;
    log.attempt_count = 1;
    log.value_resources = Some(ResourceUsage {
        quantities: BTreeMap::from([("tool.web_search".into(), 1)]),
        source: "upstream.protocol".into(),
        estimated: false,
        version: RESOURCE_ESTIMATE_VERSION.into(),
    });
    super::super::upsert_usage_row(&tx, &log).unwrap();
    settle_usage_log_isolated(&tx, &log).unwrap();
    assert_eq!(
        tx.query_row(
            "SELECT value FROM meta WHERE key=?1",
            [block_key(&scope.key().unwrap(), &window.id)],
            |r| r.get::<_, String>(0),
        )
        .unwrap(),
        "settlement_requires_reconcile"
    );
    let attempt = read_attempt_row(&tx, "unexpected-resource", "1")
        .unwrap()
        .unwrap();
    assert_eq!(attempt.7, "1000000000");
    assert_eq!(attempt.8, "pending");
    assert!(admit_in_tx(
        &tx,
        &resource_admission(
            &scope,
            &window,
            &quota,
            &revision,
            "blocked",
            ("image.output", 1),
            false,
        ),
    )
    .is_err());
    let other = Scope {
        email: "other@example.com".into(),
        ..scope.clone()
    };
    tx.execute(
        "INSERT INTO value_quota_coverage(scope,collected_from_ms) VALUES(?1,0)",
        [other.key().unwrap()],
    )
    .unwrap();
    admit_in_tx(
        &tx,
        &resource_admission(
            &other,
            &window,
            &quota,
            &revision,
            "other-resource",
            ("image.output", 1),
            false,
        ),
    )
    .unwrap();
}
#[test]
fn pending_log_does_not_disarm_body_terminal_settlement() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    let mut a = resource_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "cancel",
        ("image.output", 1),
        true,
    );
    a.resource_plan = None;
    admit_in_tx(&tx, &a).unwrap();
    let mut log = log_for(&scope, "cancel");
    log.attempt_count = 1;
    log.is_streaming = true;
    log.stream_status = Some("pending".into());
    log.input_tokens = Some(10);
    log.output_tokens = Some(0);
    settle_log_in_tx(&tx, &log, false).unwrap();
    assert!(settle_outcome_in_tx(&tx, "cancel", "1", AttemptOutcome::Unknown, true).unwrap());
    let status = status_in_tx(&tx, &scope, &window).unwrap();
    assert_eq!(status.pending_risk_picos, "0");
    assert_eq!(status.metering_status, "conservative");
    assert_eq!(status.used_picos, "200000000");
}
#[test]
fn async_task_polling_survives_reopen_deduplicates_and_preserves_original_owner() {
    use crate::domain::usage::store::{InferenceOperation, RequestKind};
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    {
        let tx = conn.transaction().unwrap();
        admit_in_tx(
            &tx,
            &resource_admission(
                &scope,
                &window,
                &quota,
                &revision,
                "generation",
                ("video.output_millisecond", 10000),
                false,
            ),
        )
        .unwrap();
        tx.execute("INSERT INTO value_async_tasks VALUES(?1,'task','generation','1','pending',9223372036854775807,NULL)",[scope.key().unwrap()]).unwrap();
        tx.commit().unwrap();
    }
    let path = std::env::temp_dir().join(format!(
        "value-task-{}-{}.sqlite",
        std::process::id(),
        crate::domain::sharing::quota_cycle::opaque_id()
    ));
    conn.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    drop(conn);
    let mut conn = Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    assert!(
        sweep_candidates(&tx, 100, 10).unwrap().is_empty(),
        "live async work remains reserved after restart"
    );
    let mut poll = log_for(&scope, "poll-1");
    poll.operation = InferenceOperation::VideoStatus;
    poll.request_kind = RequestKind::Video;
    poll.media_task_id = Some("task".into());
    poll.parent_request_id = Some("generation".into());
    poll.media_status = Some("completed".into());
    poll.video_duration_seconds = Some(2);
    let mut foreign = poll.clone();
    foreign.user_email = Some("foreign@example.test".into());
    assert!(resource_tasks::observe_task_in_tx(&tx, &foreign).is_err());
    // A failed task with partial duration is still an unknown outcome. Its
    // request bound remains until a trustworthy terminal success revises it.
    let mut failed = poll.clone();
    failed.media_status = Some("failed".into());
    assert!(resource_tasks::observe_task_in_tx(&tx, &failed).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "10000000000"
    );
    assert!(resource_tasks::observe_task_in_tx(&tx, &poll).unwrap());
    assert!(!resource_tasks::observe_task_in_tx(&tx, &poll).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "2000000000"
    );
    assert_eq!(
        tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let mut stale_failure = poll.clone();
    stale_failure.media_status = Some("failed".into());
    assert!(!resource_tasks::observe_task_in_tx(&tx, &stale_failure).unwrap());
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "2000000000"
    );
    tx.commit().unwrap();
    drop(conn);
    let _ = std::fs::remove_file(path);
}
