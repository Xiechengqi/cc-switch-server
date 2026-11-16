// Included in ledger tests. Owner-managed unpriced usage is an explicit $0
// with a reason; Market entitlement scopes keep the strict behaviour.
fn unpriced_admission<'a>(
    scope: &'a Scope,
    window: &'a Window,
    quota: &'a ValueQuota,
    revision: &'a str,
    request: &'a str,
    model: &'a str,
) -> Admission<'a> {
    Admission {
        attribution: None,
        resource_plan: None,
        enforce: false,
        scope,
        window,
        policy_epoch: "evidence",
        quota,
        pricing_revision: revision,
        request_id: request,
        attempt_id: "1",
        effective_model: model,
        effective_tier: "standard",
        started_at_ms: 1,
        estimate: RiskEstimate {
            input_tokens: 100,
            output_tokens: 100,
        },
        parallel_limit: 4,
    }
}
fn unpriced_log(
    scope: &Scope,
    request: &str,
    model: &str,
    tokens: Option<(u64, u64)>,
) -> crate::domain::usage::store::UsageLog {
    let mut log = log_for(scope, request);
    log.model = Some(model.into());
    log.attempt_count = 1;
    log.started_at_ms = 1;
    log.created_at_ms = 2;
    log.usage_revision = 1;
    if let Some((input, output)) = tokens {
        log.input_tokens = Some(input);
        log.output_tokens = Some(output);
        log.raw_input_tokens = Some(input);
        log.total_tokens = Some(input + output);
        log.cache_read_tokens = Some(0);
        log.cache_creation_tokens = Some(0);
        log.usage_state = crate::domain::usage::store::UsageState::Observed;
    }
    log
}
fn attempt_state(conn: &Connection, request: &str) -> (String, String, Option<String>) {
    conn.query_row(
        "SELECT usage_picos,metering_status,unpriced_reason FROM value_quota_attempts WHERE request_id=?1",
        [request],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}
fn pin_origin(conn: &Connection, scope: &Scope, through_ms: i64) {
    conn.execute(
        "INSERT INTO meta(key,value) VALUES('quota_evidence_legacy_origin_v6',?1)",
        [
            serde_json::json!({"routerUrl":scope.router_url,"installationId":scope.installation_id,
            "throughMs":through_ms,"excluded":[]})
            .to_string(),
        ],
    )
    .unwrap();
}

#[test]
fn materialize_records_owner_unpriced_history_at_zero_and_keeps_market_missing() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    let a = unpriced_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "owner",
        "unpriced-model",
    );
    evidence::record_admission(&tx, &a, true).unwrap();
    let log = unpriced_log(&scope, "owner", "unpriced-model", Some((100, 50)));
    evidence::capture_log(&tx, &log, None).unwrap();
    assert!(evidence::materialize(&tx, &scope, "owner", "1", &revision).unwrap());
    assert_eq!(
        attempt_state(&tx, "owner"),
        ("0".into(), "observed".into(), Some("model_unpriced".into()))
    );
    let summary = unpriced_summary_in(&tx, &scope, &window).unwrap();
    assert_eq!(summary.requests, 1);
    assert_eq!(summary.models, vec!["unpriced-model".to_string()]);
    assert_eq!(status_in_tx(&tx, &scope, &window).unwrap().used_picos, "0");

    let market = Scope {
        entitlement_id: Some("contract".into()),
        ..scope.clone()
    };
    let a = unpriced_admission(
        &market,
        &window,
        &quota,
        &revision,
        "market",
        "unpriced-model",
    );
    evidence::record_admission(&tx, &a, true).unwrap();
    let log = unpriced_log(&market, "market", "unpriced-model", Some((100, 50)));
    evidence::capture_log(&tx, &log, None).unwrap();
    assert!(!evidence::materialize(&tx, &market, "market", "1", &revision).unwrap());
    assert!(read_attempt_row(&tx, "market", "1").unwrap().is_none());
    assert_eq!(evidence::missing_in(&tx, &market, &window).unwrap(), 1);
}

#[test]
fn zero_quantity_audio_dimensions_price_through_the_normal_text_path() {
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    let tx = conn.transaction().unwrap();
    // The fixture prices audio input but not audio output: zero counts of
    // either must neither require a rate nor block the attempt.
    let zero = BTreeMap::from([
        ("audio.input_token".to_string(), 0),
        ("audio.output_token".to_string(), 0),
    ]);
    let a = Admission {
        resource_plan: Some(crate::domain::usage::value_pricing::ResourcePlan {
            quantities: zero.clone(),
            text_tokens: true,
            requires_v2: true,
        }),
        ..unpriced_admission(&scope, &window, &quota, &revision, "audio-zero", "model")
    };
    evidence::record_admission(&tx, &a, true).unwrap();
    let mut log = unpriced_log(&scope, "audio-zero", "model", Some((100, 50)));
    log.value_resources = Some(crate::domain::usage::value_pricing::ResourceUsage {
        quantities: zero,
        source: "upstream.protocol".into(),
        estimated: false,
        version: crate::domain::usage::value_pricing::RESOURCE_ESTIMATE_VERSION.into(),
    });
    evidence::capture_log(&tx, &log, None).unwrap();
    assert!(evidence::materialize(&tx, &scope, "audio-zero", "1", &revision).unwrap());
    assert_eq!(
        attempt_state(&tx, "audio-zero"),
        ("150000000".into(), "observed".into(), None)
    );
}

#[test]
fn unknown_history_without_usage_is_usage_unobserved_and_tokens_are_priced() {
    let (mut conn, scope, window, _, revision) = setup();
    let tx = conn.transaction().unwrap();
    pin_origin(&tx, &scope, 10);
    // A legacy local 429 rejection: no estimate, no usage.
    let mut rejected = unpriced_log(&scope, "rejected", "model", None);
    rejected.status_code = 429;
    evidence::capture_log(&tx, &rejected, Some(&scope)).unwrap();
    assert!(evidence::materialize(&tx, &scope, "rejected", "1", &revision).unwrap());
    assert_eq!(
        attempt_state(&tx, "rejected"),
        (
            "0".into(),
            "conservative".into(),
            Some("usage_unobserved".into())
        )
    );
    // An outcome-unknown legacy log with real tokens and no estimate is
    // priced conservatively from those tokens.
    let mut unknown = unpriced_log(&scope, "unknown", "model", Some((100, 50)));
    unknown.failure_kind = Some("upstream_outcome_unknown".into());
    evidence::capture_log(&tx, &unknown, Some(&scope)).unwrap();
    assert!(evidence::materialize(&tx, &scope, "unknown", "1", &revision).unwrap());
    assert_eq!(
        attempt_state(&tx, "unknown"),
        ("150000000".into(), "conservative".into(), None)
    );
    // Neither is unpriced: usage_unobserved is empty, not a missing price.
    assert_eq!(
        unpriced_summary_in(&tx, &scope, &window).unwrap().requests,
        0
    );
    assert_eq!(evidence::missing_in(&tx, &scope, &window).unwrap(), 0);
    assert_eq!(evidence::unresolved_in(&tx, &scope, &window).unwrap(), 0);
}

#[test]
fn capture_never_downgrades_a_not_sent_fact_and_counts_post_cutoff_rejections() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    // Without a pinned legacy origin nothing proves the log was never
    // admitted: it stays a hole.
    let mut unpinned = unpriced_log(&scope, "unpinned-400", "model", None);
    unpinned.status_code = 400;
    unpinned.created_at_ms = 5;
    evidence::capture_log(&tx, &unpinned, Some(&scope)).unwrap();
    assert!(evidence::read_fact(&tx, "unpinned-400", "1")
        .unwrap()
        .is_none());
    pin_origin(&tx, &scope, 1);
    let a = unpriced_admission(&scope, &window, &quota, &revision, "not-sent", "model");
    evidence::record_admission(&tx, &a, true).unwrap();
    evidence::outcome(&tx, "not-sent", "1", AttemptOutcome::NotSent).unwrap();
    let mut log = unpriced_log(&scope, "not-sent", "model", None);
    log.status_code = 499;
    evidence::capture_log(&tx, &log, None).unwrap();
    let outcome: String = tx
        .query_row(
            "SELECT json_extract(fact_json,'$.outcome') FROM quota_metering_facts WHERE request_id='not-sent'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "free");
    // Post-cutoff local rejection logged before admission: a free fact keeps
    // the evidence request count equal to the Token proof's.
    let mut rejected = unpriced_log(&scope, "local-400", "model", None);
    rejected.status_code = 400;
    rejected.created_at_ms = 5;
    evidence::capture_log(&tx, &rejected, Some(&scope)).unwrap();
    // Anything that may have executed stays a genuine hole: Token usage, a
    // success without usage, or resource-only (image / resource) evidence.
    let mut holes = Vec::new();
    let mut tokens = unpriced_log(&scope, "hole-tokens", "model", Some((10, 10)));
    tokens.status_code = 400;
    holes.push(tokens);
    holes.push(unpriced_log(&scope, "hole-success", "model", None));
    let mut image = unpriced_log(&scope, "hole-image", "model", None);
    image.status_code = 400;
    image.image_count = Some(1);
    holes.push(image);
    let mut resource = unpriced_log(&scope, "hole-resource", "model", None);
    resource.status_code = 429;
    resource.value_resources = Some(crate::domain::usage::value_pricing::ResourceUsage {
        quantities: BTreeMap::from([("tool.web_search".into(), 1)]),
        source: "upstream.protocol".into(),
        estimated: false,
        version: crate::domain::usage::value_pricing::RESOURCE_ESTIMATE_VERSION.into(),
    });
    holes.push(resource);
    for mut hole in holes {
        hole.created_at_ms = 5;
        evidence::capture_log(&tx, &hole, Some(&scope)).unwrap();
        assert!(
            evidence::read_fact(&tx, &hole.request_id, "1")
                .unwrap()
                .is_none(),
            "{}",
            hole.request_id
        );
    }
    let (tokens, requests) = evidence::totals_in(&tx, &scope, &window, 0, Some(10)).unwrap();
    assert_eq!((tokens, requests), (0, 2));
    let proof = conversion::CoverageProof {
        from_ms: 0,
        until_ms: 10,
        tokens: 0,
        requests: 2,
        minimum_tokens: 0,
    };
    assert!(
        conversion::preview_in_tx(&tx, &scope, &window, &proof)
            .unwrap()
            .complete
    );
}

#[test]
fn conversion_is_complete_with_unpriced_evidence_and_reports_it() {
    let (mut conn, scope, w, quota, revision) = conversion_fixture();
    let tx = conn.transaction().unwrap();
    let a = unpriced_admission(&scope, &w, &quota, &revision, "unpriced", "unpriced-model");
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    let log = unpriced_log(&scope, "unpriced", "unpriced-model", Some((1000, 0)));
    evidence::capture_log(&tx, &log, None).unwrap();
    settle_log_in_tx(&tx, &log, false).unwrap();
    let c = conversion_change(&scope, &w, "unpriced-to-usd", 1000, 1, "usdEquivalent");
    let preview = conversion::preview_in_tx(&tx, &scope, &w, &c.proof).unwrap();
    assert!(preview.complete, "{preview:?}");
    assert_eq!(preview.unpriced_facts, 1);
    assert_eq!(preview.unpriced_tokens, 1000);
    assert_eq!(preview.unpriced_models, vec!["unpriced-model".to_string()]);
    assert_eq!(preview.status.used_picos, "0");
    conversion::apply_in_tx(&tx, &[c]).unwrap();
    // A genuine Token mismatch still blocks.
    let mismatch = conversion_change(&scope, &w, "mismatch", 999, 1, "usdEquivalent");
    assert!(
        !conversion::preview_in_tx(&tx, &scope, &w, &mismatch.proof)
            .unwrap()
            .complete
    );
}

fn evidence_window() -> Window {
    Window {
        account: None,
        id: "evidence".into(),
        starts_at_ms: 0,
        ends_at_ms: None,
        metered_from_ms: 0,
    }
}
fn set_active(conn: &Connection, scope: &Scope, revision: &str) {
    conn.execute(
        "INSERT INTO meta VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![
            format!(
                "value_tariff:{}:{}",
                scope.router_url, scope.installation_id
            ),
            revision
        ],
    )
    .unwrap();
}
/// The owner prices `late-model`: a new revision is installed and activates,
/// which records the durable re-pricing job.
fn activate_covering_tariff(tx: &Transaction<'_>, scope: &Scope, old: &str) -> String {
    let raw: String = tx
        .query_row(
            "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
            [old],
            |r| r.get(0),
        )
        .unwrap();
    let mut snapshot: PricingSnapshot = serde_json::from_str(&raw).unwrap();
    snapshot
        .models
        .insert("late-model".into(), snapshot.models["model"].clone());
    let new = snapshot.revision().unwrap();
    install_in_tx(tx, scope, &new, &snapshot, 10).unwrap();
    tx.execute(
        "INSERT INTO meta VALUES(?1,?2)",
        params![
            pending_tariff_key(scope),
            serde_json::to_string(&PendingTariff {
                revision: new.clone(),
                effective_from_ms: 100
            })
            .unwrap()
        ],
    )
    .unwrap();
    assert_eq!(
        tariff::activate_due_in_tx(tx, scope, 100).unwrap(),
        Some(new.clone())
    );
    new
}
/// Shadow-admits an unpriced `late-model` attempt and, unless `log` is None,
/// settles it from that log.
fn unpriced_attempt(
    tx: &Transaction<'_>,
    a: &Admission<'_>,
    log: Option<crate::domain::usage::store::UsageLog>,
) {
    evidence::record_admission(tx, a, true).unwrap();
    admit_with_trial_in_tx(tx, a, None).unwrap();
    if let Some(log) = log {
        evidence::capture_log(tx, &log, None).unwrap();
        settle_log_in_tx(tx, &log, false).unwrap();
    }
}

#[test]
fn activation_reprices_open_window_from_observed_usage_and_keeps_ended_windows() {
    let (mut conn, scope, _, quota, old) = setup();
    let window = evidence_window();
    let tx = conn.transaction().unwrap();
    set_active(&tx, &scope, &old);
    let base = |request: &'static str, started: i64| Admission {
        started_at_ms: started,
        ..unpriced_admission(&scope, &window, &quota, &old, request, "late-model")
    };
    let log = |request: &str, started: i64| {
        let mut log = unpriced_log(&scope, request, "late-model", Some((100, 50)));
        log.started_at_ms = started as u128;
        log
    };
    unpriced_attempt(&tx, &base("ended", 10), Some(log("ended", 10)));
    unpriced_attempt(&tx, &base("open", 2_000), Some(log("open", 2_000)));
    // Outcome unknown with observed tokens: the estimate ceiling (100+100
    // Tokens = $0.0002) must never be charged, only the observed $0.00015.
    let mut unknown = log("unknown", 2_001);
    unknown.failure_kind = Some("upstream_outcome_unknown".into());
    unpriced_attempt(&tx, &base("unknown", 2_001), Some(unknown));
    // Unknown execution tier: priced at the highest available tier.
    unpriced_attempt(
        &tx,
        &Admission {
            effective_tier: "unknown",
            ..base("unknown-tier", 2_002)
        },
        Some(log("unknown-tier", 2_002)),
    );
    // Still running: re-frozen only; its reserve and pending risk agree.
    unpriced_attempt(&tx, &base("pending", 2_003), None);
    for request in ["ended", "open"] {
        assert_eq!(
            attempt_state(&tx, request),
            ("0".into(), "observed".into(), Some("model_unpriced".into()))
        );
    }
    let new = activate_covering_tariff(&tx, &scope, &old);
    let before = status_in_tx(&tx, &scope, &window).unwrap().usage_watermark;
    // Only the open window (starting at 1000) is re-priced.
    let windows = BTreeMap::from([(
        scope.key().unwrap(),
        Window {
            account: None,
            id: "Day:1000:0".into(),
            starts_at_ms: 1_000,
            ends_at_ms: None,
            metered_from_ms: 1_000,
        },
    )]);
    let page = reprice::reprice_page_in_tx(&tx, &scope, &windows, 100).unwrap();
    assert_eq!(
        page,
        reprice::RepricePage {
            repriced: 4,
            done: true
        }
    );
    assert_eq!(
        attempt_state(&tx, "open"),
        ("150000000".into(), "observed".into(), None)
    );
    assert_eq!(
        attempt_state(&tx, "unknown"),
        ("150000000".into(), "conservative".into(), None)
    );
    assert_eq!(
        attempt_state(&tx, "unknown-tier"),
        ("150000000".into(), "conservative".into(), None)
    );
    assert_eq!(
        attempt_state(&tx, "pending"),
        ("0".into(), "pending".into(), None)
    );
    let pending_risk: String = tx
        .query_row(
            "SELECT pending_risk_picos FROM value_quota_attempts WHERE request_id='pending'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        read_attempt_meta(&tx, "pending", "1")
            .unwrap()
            .unwrap()
            .reserved()
            .to_string(),
        pending_risk
    );
    assert_eq!(
        tx.query_row(
            "SELECT pricing_revision FROM value_quota_attempts WHERE request_id='open'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        new
    );
    assert_eq!(
        attempt_state(&tx, "ended"),
        ("0".into(), "observed".into(), Some("model_unpriced".into()))
    );
    let status = status_in_tx(&tx, &scope, &window).unwrap();
    assert_eq!(status.used_picos, "450000000");
    assert_eq!(status.pending_risk_picos, pending_risk);
    assert!(status.usage_watermark > before);
    // Idempotent: a repeated job finds nothing left to re-price.
    reprice::mark_due(&tx, &scope, &new).unwrap();
    let page = reprice::reprice_page_in_tx(&tx, &scope, &windows, 100).unwrap();
    assert_eq!(
        page,
        reprice::RepricePage {
            repriced: 0,
            done: true
        }
    );
    assert_eq!(
        status_in_tx(&tx, &scope, &window).unwrap().used_picos,
        "450000000"
    );
    assert!(!tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key LIKE 'value_reprice_due:%')",
            [],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
}

#[test]
fn admission_replay_stays_idempotent_after_re_pricing() {
    let (mut conn, scope, _, quota, old) = setup();
    let window = evidence_window();
    let tx = conn.transaction().unwrap();
    set_active(&tx, &scope, &old);
    let a = unpriced_admission(&scope, &window, &quota, &old, "replay", "late-model");
    unpriced_attempt(
        &tx,
        &a,
        Some(unpriced_log(
            &scope,
            "replay",
            "late-model",
            Some((100, 50)),
        )),
    );
    let new = activate_covering_tariff(&tx, &scope, &old);
    let windows = BTreeMap::from([(scope.key().unwrap(), window.clone())]);
    reprice::reprice_page_in_tx(&tx, &scope, &windows, 100).unwrap();
    assert_eq!(attempt_state(&tx, "replay").0, "150000000");
    // Replaying the original admission (old revision) or the stored one is
    // the same request; the amount and row count are unchanged.
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    admit_with_trial_in_tx(
        &tx,
        &Admission {
            pricing_revision: &new,
            ..a.clone()
        },
        None,
    )
    .unwrap();
    assert_eq!(attempt_state(&tx, "replay").0, "150000000");
    assert_eq!(
        tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    // A genuinely different identity still conflicts.
    let error = admit_with_trial_in_tx(
        &tx,
        &Admission {
            started_at_ms: 2,
            ..a.clone()
        },
        None,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("REQUEST_IDENTITY_CONFLICT"),
        "{error}"
    );
}

#[test]
fn re_pricing_scan_is_confined_to_the_installation_and_account_bucket() {
    use crate::domain::sharing::quota_cycle::{AccountQuotaCycle, QuotaTimeBasis};
    use crate::domain::usage::quota_ledger::QuotaAttribution;
    let (mut conn, scope, _, quota, old) = setup();
    let window = evidence_window();
    let tx = conn.transaction().unwrap();
    set_active(&tx, &scope, &old);
    for (request, bucket) in [("bucket-a", "a"), ("bucket-b", "b")] {
        let a = Admission {
            attribution: Some(QuotaAttribution {
                source_ref: "source".into(),
                source_epoch: 1,
                bucket_id: bucket.into(),
                started_at_ms: 1,
            }),
            ..unpriced_admission(&scope, &window, &quota, &old, request, "late-model")
        };
        unpriced_attempt(
            &tx,
            &a,
            Some(unpriced_log(&scope, request, "late-model", Some((100, 50)))),
        );
    }
    // A row of another installation, even when its scope is offered.
    let foreign = Scope {
        installation_id: "other-installation".into(),
        ..scope.clone()
    };
    tx.execute(
        "INSERT INTO value_quota_attempts(request_id,attempt_id,scope,policy_epoch,window_id,started_at_ms,
         pricing_revision,effective_model,effective_tier,frozen_tariff_json,usage_revision,usage_picos,
         pending_risk_picos,metering_status,watermark,unpriced_reason)
         VALUES('foreign','1',?1,'evidence','evidence',1,?2,'late-model','standard',
         '{\"unpricedReason\":\"model_unpriced\"}',1,'0','0','observed',999999,'model_unpriced')",
        params![foreign.key().unwrap(), old],
    )
    .unwrap();
    activate_covering_tariff(&tx, &scope, &old);
    let cycle = AccountQuotaCycle {
        stream_incarnation: "stream".into(),
        source_ref: "source".into(),
        source_epoch: 1,
        bucket_id: "a".into(),
        cycle_id: "cycle".into(),
        cycle_sequence: 1,
        cycle_revision: 1,
        starts_at_ms: 0,
        ends_at_ms: None,
        basis: QuotaTimeBasis::UpstreamWindow,
        observed_at_ms: 0,
        event_id: "event".into(),
    };
    let windows = BTreeMap::from([
        (
            scope.key().unwrap(),
            Window {
                account: Some(cycle),
                id: "account:source:1:a:cycle".into(),
                ..window.clone()
            },
        ),
        (foreign.key().unwrap(), window.clone()),
    ]);
    let page = reprice::reprice_page_in_tx(&tx, &scope, &windows, 100).unwrap();
    assert_eq!(
        page,
        reprice::RepricePage {
            repriced: 1,
            done: true
        }
    );
    assert_eq!(
        attempt_state(&tx, "bucket-a"),
        ("150000000".into(), "observed".into(), None)
    );
    assert_eq!(
        attempt_state(&tx, "bucket-b").2.as_deref(),
        Some("model_unpriced")
    );
    assert_eq!(
        attempt_state(&tx, "foreign").2.as_deref(),
        Some("model_unpriced")
    );
}

#[test]
fn unknown_tier_prices_owner_attempts_at_the_highest_tier_and_market_refuses() {
    let (mut conn, scope, window, quota, revision) = setup();
    let tx = conn.transaction().unwrap();
    let a = Admission {
        enforce: true,
        effective_tier: "unknown",
        ..unpriced_admission(&scope, &window, &quota, &revision, "tier", "model")
    };
    evidence::record_admission(&tx, &a, true).unwrap();
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    // The reservation is the highest-tier estimate (200 Tokens), not the
    // whole remaining limit, and no unpriced reason is recorded.
    assert_eq!(
        tx.query_row(
            "SELECT pending_risk_picos FROM value_quota_attempts WHERE request_id='tier'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "200000000"
    );
    let log = unpriced_log(&scope, "tier", "model", Some((100, 50)));
    evidence::capture_log(&tx, &log, None).unwrap();
    settle_log_in_tx(&tx, &log, false).unwrap();
    assert_eq!(
        attempt_state(&tx, "tier"),
        ("150000000".into(), "conservative".into(), None)
    );
    let market = Scope {
        entitlement_id: Some("contract".into()),
        ..scope.clone()
    };
    let error = admit_with_trial_in_tx(
        &tx,
        &Admission {
            scope: &market,
            request_id: "market-tier",
            ..a.clone()
        },
        None,
    )
    .unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::domain::usage::value_pricing::PriceRejection>()
            .unwrap()
            .reason_code,
        "tier_unpriced"
    );
}

#[test]
fn a_model_without_any_text_tier_is_tier_unpriced_and_zero_dimensions_are_stripped() {
    let (mut conn, scope, window, quota, revision) = resource_fixture();
    let raw: String = conn
        .query_row(
            "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
            [&revision],
            |r| r.get(0),
        )
        .unwrap();
    let mut snapshot: PricingSnapshot = serde_json::from_str(&raw).unwrap();
    let mut media = snapshot.models["model"].clone();
    media.rates.clear();
    media.long_context_threshold = None;
    snapshot.models.insert("media-only".into(), media);
    let revision = snapshot.revision().unwrap();
    let tx = conn.transaction().unwrap();
    install_in_tx(&tx, &scope, &revision, &snapshot, 2).unwrap();
    admit_with_trial_in_tx(
        &tx,
        &unpriced_admission(&scope, &window, &quota, &revision, "no-tier", "media-only"),
        None,
    )
    .unwrap();
    assert_eq!(
        attempt_state(&tx, "no-tier"),
        ("0".into(), "pending".into(), Some("tier_unpriced".into()))
    );
    // Live admission drops zero-quantity dimensions before pricing and the
    // v2 policy check; the unpriced audio output never needs a rate.
    let mut a = resource_admission(
        &scope,
        &window,
        &quota,
        &revision,
        "zero-stripped",
        ("image.output", 1),
        false,
    );
    a.resource_plan
        .as_mut()
        .unwrap()
        .quantities
        .insert("audio.output_token".into(), 0);
    admit_with_trial_in_tx(&tx, &a, None).unwrap();
    assert_eq!(attempt_state(&tx, "zero-stripped").2, None);
    assert_eq!(
        read_attempt_meta(&tx, "zero-stripped", "1")
            .unwrap()
            .unwrap()
            .resource_plan
            .unwrap()
            .quantities,
        BTreeMap::from([("image.output".to_string(), 1)])
    );
}
