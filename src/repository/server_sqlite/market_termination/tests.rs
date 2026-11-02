use super::*;
use crate::domain::{
    providers::model::{AppKind, ProviderType},
    sharing::{
        market_trials::MarketRequestReceipt, router_contract::ShareMarketPhase, shares::Share,
    },
    usage::store::{TokenUsage, UsageModelMetadata, UsageState},
};
use serde_json::json;

pub(super) fn fixture() -> (Connection, ShareStore, ShareManagedGrantOperation) {
    let c = Connection::open_in_memory().unwrap();
    super::super::schema::initialize_schema(&c).unwrap();
    c.execute(
        "UPDATE meta SET value='0' WHERE key='market_settlement_since_ms'",
        [],
    )
    .unwrap();
    let share: Share = serde_json::from_value(json!({
        "id":"share", "app":"codex", "providerId":"provider", "providerType":"codex",
        "userGrants": {"user@example.test":{"email":"user@example.test","active":false,"entitlementId":"entitlement"}}
    })).unwrap();
    c.execute(
        "INSERT INTO providers VALUES('codex','provider','codex',1,1,'{}','fixture')",
        [],
    )
    .unwrap();
    c.execute(
        "INSERT INTO shares VALUES('share','codex','provider','codex',1,?1,'fixture')",
        [serde_json::to_string(&share).unwrap()],
    )
    .unwrap();
    let store = ShareStore {
        shares: vec![share],
        ..Default::default()
    };
    let op = serde_json::from_value(json!({
        "operationId":"operation", "entitlementId":"entitlement", "shareSequence":1,
        "expectedConfigRevision":1,"action":"revoke","email":"user@example.test",
        "termination":{"terminationId":"termination","cutoffAtMs":500,
            "periodStartMs":100,"periodEndMs":1000,"policy":{"tokenLimit":100,"tokenPeriod":"lifetime"}}
    })).unwrap();
    (c, store, op)
}
fn admission(store: &mut ShareStore, id: &str, phase: ShareMarketPhase, at: i64, tokens: u64) {
    store.shares[0].market_requests.insert(
        id.into(),
        MarketRequestReceipt {
            entitlement_id: "entitlement".into(),
            email: "user@example.test".into(),
            phase,
            started_at_ms: at,
            tokens,
            revision: 1,
            count_request: true,
            parent_request_id: None,
            value_picos: "0".into(),
        },
    );
}
fn log(id: &str, revision: u64, state: UsageState) -> UsageLog {
    let mut log = UsageLog::new(
        AppKind::Codex,
        "provider".into(),
        "Provider".into(),
        ProviderType::Codex,
        200,
        1,
        UsageModelMetadata::default(),
        TokenUsage {
            input_tokens: Some(10),
            ..Default::default()
        },
    );
    log.request_id = id.into();
    log.usage_revision = revision;
    log.usage_state = state;
    log.share_id = Some("share".into());
    log.user_email = Some("user@example.test".into());
    log.input_tokens = Some(match id {
        "opening" => 20,
        "trial" | "race" => 99,
        _ if revision == 2 => 11,
        _ => 10,
    });
    log
}

#[test]
fn drains_trial_and_racing_admissions_but_only_pre_cutoff_paid_use_earns_rent() {
    let (mut c, mut store, op) = fixture();
    for (id, phase, at, tokens) in [
        ("opening", ShareMarketPhase::Paid, 50, 20),
        ("paid", ShareMarketPhase::Paid, 200, 10),
        ("trial", ShareMarketPhase::Trial, 300, 99),
        ("race", ShareMarketPhase::Paid, 600, 99),
    ] {
        admission(&mut store, id, phase, at, tokens);
    }
    archive_in(&c, &store).unwrap();
    let tx = c.transaction().unwrap();
    for id in ["opening", "paid", "race"] {
        finalize_log_in(&tx, &log(id, 1, UsageState::Observed)).unwrap();
    }
    assert!(receipt_in(&tx, "share", &op, 700)
        .unwrap_err()
        .to_string()
        .contains("PENDING"));
    finalize_log_in(&tx, &log("trial", 1, UsageState::Interrupted)).unwrap();
    assert!(
        receipt_in(&tx, "share", &op, 700).is_err(),
        "uncertain trial is not zero final usage"
    );
    finalize_log_in(&tx, &log("trial", 1, UsageState::Observed)).unwrap();
    let r = receipt_in(&tx, "share", &op, 700).unwrap();
    assert_eq!(r.quota_used_before_period, "20");
    assert_eq!(
        r.windows,
        vec![UsageWindow {
            starts_at_ms: 100,
            ends_at_ms: 1000,
            used: "10".into()
        }]
    );
    assert_eq!(
        receipt_in(&tx, "share", &op, 900).unwrap(),
        r,
        "receipt replay is immutable, including revoke time"
    );
    let mut other = op.clone();
    other.termination.as_mut().unwrap().cutoff_at_ms = 501;
    assert!(receipt_in(&tx, "share", &other, 900).is_err());
    assert!(tx
        .execute("DELETE FROM market_settlement_receipts", [])
        .is_err());
    tx.commit().unwrap();
}

#[test]
fn higher_revisions_reopen_finality_until_the_corresponding_usage_commit() {
    let (mut c, mut store, op) = fixture();
    admission(&mut store, "paid", ShareMarketPhase::Paid, 200, 10);
    archive_in(&c, &store).unwrap();
    finalize_log_in(&c, &log("paid", 1, UsageState::Observed)).unwrap();
    let r = store.shares[0].market_requests.get_mut("paid").unwrap();
    r.tokens = 11;
    r.revision = 2;
    archive_in(&c, &store).unwrap();
    finalize_log_in(&c, &log("paid", 1, UsageState::Observed)).unwrap();
    let tx = c.transaction().unwrap();
    assert!(
        receipt_in(&tx, "share", &op, 700).is_err(),
        "a stale final log cannot close a newer revision"
    );
    finalize_log_in(&tx, &log("paid", 2, UsageState::Observed)).unwrap();
    assert_eq!(
        receipt_in(&tx, "share", &op, 700).unwrap().windows[0].used,
        "11"
    );
    tx.commit().unwrap();
    store.shares[0]
        .market_requests
        .get_mut("paid")
        .unwrap()
        .tokens = 12;
    assert!(
        archive_in(&c, &store).is_err(),
        "same revision cannot change quantity"
    );
    store.shares[0]
        .market_requests
        .get_mut("paid")
        .unwrap()
        .email = "another@example.test".into();
    assert!(
        archive_in(&c, &store).is_err(),
        "request identity cannot be reassigned"
    );
}

#[test]
fn dollar_receipt_requires_execution_evidence_not_a_display_or_missing_zero() {
    let (mut c, mut store, mut op) = fixture();
    admission(&mut store, "paid", ShareMarketPhase::Paid, 200, 10);
    archive_in(&c, &store).unwrap();
    finalize_log_in(&c, &log("paid", 1, UsageState::Observed)).unwrap();
    op.termination.as_mut().unwrap().policy.token_limit = None;
    op.termination.as_mut().unwrap().policy.value_quota = Some(
        serde_json::from_value(json!({
            "limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1"
        }))
        .unwrap(),
    );
    let tx = c.transaction().unwrap();
    assert!(receipt_in(&tx, "share", &op, 700)
        .unwrap_err()
        .to_string()
        .contains("execution-price"));
    store.shares[0]
        .market_requests
        .get_mut("paid")
        .unwrap()
        .tokens = 0;
    store.shares[0]
        .market_requests
        .get_mut("paid")
        .unwrap()
        .revision = 2;
    archive_in(&tx, &store).unwrap();
    let mut rejected = log("paid", 1, UsageState::NotApplicable);
    rejected.usage_revision = 2;
    rejected.status_code = 400;
    rejected.input_tokens = None;
    rejected.raw_input_tokens = None;
    rejected.output_tokens = None;
    rejected.total_tokens = None;
    finalize_log_in(&tx, &rejected).unwrap();
    assert_eq!(
        receipt_in(&tx, "share", &op, 700).unwrap().windows[0].used,
        "0"
    );
}

#[test]
fn active_original_grant_missing_share_and_incomplete_archive_never_prove_revocation() {
    let (mut c, mut store, op) = fixture();
    store.shares[0]
        .user_grants
        .get_mut("user@example.test")
        .unwrap()
        .active = true;
    c.execute(
        "UPDATE shares SET payload_json=?1",
        [serde_json::to_string(&store.shares[0]).unwrap()],
    )
    .unwrap();
    let tx = c.transaction().unwrap();
    assert!(receipt_in(&tx, "share", &op, 700).is_err());
    tx.execute("DELETE FROM shares", []).unwrap();
    assert!(receipt_in(&tx, "share", &op, 700).is_err());
    tx.execute(
        "UPDATE meta SET value='101' WHERE key='market_settlement_since_ms'",
        [],
    )
    .unwrap();
    assert!(receipt_in(&tx, "share", &op, 700)
        .unwrap_err()
        .to_string()
        .contains("cover"));
}
