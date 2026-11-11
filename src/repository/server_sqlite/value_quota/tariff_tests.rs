// Included inside the existing ledger tests; no live network or wall-clock waits.
use crate::domain::usage::value_pricing::PriceRejection;
fn prepared_price_update() -> (Connection, Scope, Window, ValueQuota, String, String) {
    let (mut conn, scope, window, quota, old) = setup();
    let raw: String = conn
        .query_row(
            "SELECT payload_json FROM value_pricing_snapshots WHERE revision=?1",
            [&old],
            |r| r.get(0),
        )
        .unwrap();
    let mut snapshot: PricingSnapshot = serde_json::from_str(&raw).unwrap();
    snapshot.models.insert(
        "qiniu/conatus-free".into(),
        snapshot.models["model"].clone(),
    );
    let new = snapshot.revision().unwrap();
    let tx = conn.transaction().unwrap();
    install_in_tx(&tx, &scope, &new, &snapshot, 10).unwrap();
    tx.execute(
        "INSERT INTO meta VALUES(?1,?2)",
        params![
            format!(
                "value_tariff:{}:{}",
                scope.router_url, scope.installation_id
            ),
            old
        ],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO meta VALUES(?1,?2)",
        params![
            pending_tariff_key(&scope),
            serde_json::to_string(&PendingTariff {
                revision: new.clone(),
                effective_from_ms: 100
            })
            .unwrap()
        ],
    )
    .unwrap();
    tx.commit().unwrap();
    (conn, scope, window, quota, old, new)
}

#[test]
fn installed_model_price_becomes_usable_at_boundary_without_a_heartbeat() {
    let (mut conn, scope, window, quota, old, new) = prepared_price_update();
    let tx = conn.transaction().unwrap();
    assert_eq!(
        tariff::activate_due_in_tx(&tx, &scope, 99).unwrap(),
        Some(old.clone())
    );
    let admission = Admission {
        attribution: None,
        resource_plan: None,
        enforce: true,
        scope: &scope,
        window: &window,
        policy_epoch: "epoch",
        quota: &quota,
        pricing_revision: &old,
        request_id: "new-model",
        attempt_id: "1",
        effective_model: "qiniu/conatus-free",
        effective_tier: "standard",
        started_at_ms: 99,
        estimate: RiskEstimate {
            input_tokens: 1,
            output_tokens: 1,
        },
        parallel_limit: 4,
    };
    let error = admit_in_tx(&tx, &admission).unwrap_err();
    let diagnostic = error.downcast_ref::<PriceRejection>().unwrap();
    assert_eq!(diagnostic.reason_code, "tariff_not_effective");
    assert_eq!(diagnostic.effective_from_ms, Some(100));
    assert_eq!(
        tariff::activate_due_in_tx(&tx, &scope, 100).unwrap(),
        Some(new.clone())
    );
    admit_in_tx(
        &tx,
        &Admission {
            pricing_revision: &new,
            ..admission
        },
    )
    .unwrap();
    assert_eq!(
        tx.query_row("SELECT count(*) FROM value_quota_attempts", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(!tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM meta WHERE key=?1)",
            [pending_tariff_key(&scope)],
            |r| r.get::<_, bool>(0)
        )
        .unwrap());
    tx.commit().unwrap();
}

#[test]
fn tariff_activation_keeps_admitted_attempts_frozen_and_replays_idempotent() {
    let (mut conn, scope, window, quota, old, new) = prepared_price_update();
    let tx = conn.transaction().unwrap();
    let admission = Admission {
        attribution: None,
        resource_plan: None,
        enforce: true,
        scope: &scope,
        window: &window,
        policy_epoch: "epoch",
        quota: &quota,
        pricing_revision: &old,
        request_id: "frozen",
        attempt_id: "1",
        effective_model: "model",
        effective_tier: "standard",
        started_at_ms: 1,
        estimate: RiskEstimate {
            input_tokens: 1,
            output_tokens: 1,
        },
        parallel_limit: 4,
    };
    admit_in_tx(&tx, &admission).unwrap();
    let before:(String,String,String)=tx.query_row("SELECT pricing_revision,frozen_tariff_json,pending_risk_picos FROM value_quota_attempts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(
        tariff::admission_revision(&tx, &admission, 100).unwrap(),
        old
    );
    admit_in_tx(&tx, &admission).unwrap();
    assert_eq!(
        tariff::admission_revision(
            &tx,
            &Admission {
                request_id: "next",
                ..admission.clone()
            },
            100
        )
        .unwrap(),
        new
    );
    let after:(String,String,String)=tx.query_row("SELECT pricing_revision,frozen_tariff_json,pending_risk_picos FROM value_quota_attempts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(before, after);
    tx.commit().unwrap();
}

#[test]
fn pending_activation_is_durable_and_atomic_across_reopened_connections() {
    let (conn, scope, _, _, _, new) = prepared_price_update();
    let dir = std::env::temp_dir().join(format!(
        "server-tariff-activation-{}",
        rand::random::<u64>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.sqlite3");
    conn.backup(rusqlite::DatabaseName::Main, &path, None)
        .unwrap();
    drop(conn);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads = (0..2)
        .map(|_| {
            let (path, scope, new, barrier) =
                (path.clone(), scope.clone(), new.clone(), barrier.clone());
            std::thread::spawn(move || {
                let mut conn = Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                barrier.wait();
                let tx = conn
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .unwrap();
                assert_eq!(
                    tariff::activate_due_in_tx(&tx, &scope, 100).unwrap(),
                    Some(new)
                );
                tx.commit().unwrap();
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    let mut reopened = Connection::open(&path).unwrap();
    let tx = reopened.transaction().unwrap();
    assert_eq!(
        tariff::activate_due_in_tx(&tx, &scope, 101).unwrap(),
        Some(new)
    );
    tx.commit().unwrap();
    drop(reopened);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn corrupt_pending_state_is_not_silently_ignored() {
    let (mut conn, scope, _, _, _, _) = prepared_price_update();
    conn.execute(
        "UPDATE meta SET value='bad-json' WHERE key=?1",
        [pending_tariff_key(&scope)],
    )
    .unwrap();
    let tx = conn.transaction().unwrap();
    assert!(tariff::activate_due_in_tx(&tx, &scope, 100)
        .unwrap_err()
        .to_string()
        .contains("PENDING_TARIFF_CORRUPT"));
}
