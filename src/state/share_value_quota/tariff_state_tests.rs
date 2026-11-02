#[tokio::test]
async fn fixed_execution_model_coverage_gates_usd_edits_but_not_ordinary_background_saves() {
    use crate::domain::usage::value_pricing::PricingSnapshot;
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    let state = test_state();
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    state
        .configure_value_quota_router_for_test("coverage-server")
        .await;
    let mut provider = test_provider("provider", "Provider");
    provider.settings_config =
        json!({"modelMapping":{"mode":"single","upstreamModel":"qiniu/conatus-free"}});
    state
        .upsert_provider_command(
            AppKind::Codex,
            provider,
            Some(ProfileId::parse("codex.openai_api_key").unwrap()),
            None,
            None,
            test_api_key_credential(),
        )
        .await
        .unwrap()
        .unwrap();
    state
        .mutate_shares_immediate(|store| {
            store
                .upsert(router_sync_share_input("coverage-share", "provider"))
                .unwrap();
        })
        .await
        .unwrap();
    let config = state.config_snapshot().await;
    let scope = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "coverage-server".into(),
        share_id: "coverage-share".into(),
        email: "user@example.test".into(),
        entitlement_id: None,
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    let mut snapshot: PricingSnapshot =
        serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    let old = snapshot.revision().unwrap();
    let now = now_ms_i64();
    ledger::install_snapshot(&state.config_dir, &scope, &old, &snapshot, now).unwrap();
    ledger::schedule_tariff(
        &state.config_dir,
        &scope,
        &ledger::PendingTariff {
            revision: old.clone(),
            effective_from_ms: now - 1,
        },
        now,
    )
    .unwrap();
    let revision = state
        .shares
        .read()
        .await
        .get("coverage-share")
        .unwrap()
        .config_revision;
    let coverage = |preview: &Value| {
        preview["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["code"] == "model_price_coverage")
            .unwrap()
            .clone()
    };
    let preview = state
        .preview_share_value_quota("coverage-share", revision, BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(coverage(&preview)["reasonCode"], "model_unpriced");
    assert_eq!(preview["capabilityReady"], false);
    snapshot.models.insert(
        "qiniu/conatus-free".into(),
        snapshot.models.values().next().unwrap().clone(),
    );
    let new = snapshot.revision().unwrap();
    ledger::install_snapshot(&state.config_dir, &scope, &new, &snapshot, now).unwrap();
    ledger::schedule_tariff(
        &state.config_dir,
        &scope,
        &ledger::PendingTariff {
            revision: new.clone(),
            effective_from_ms: now + 60_000,
        },
        now,
    )
    .unwrap();
    let pending = state
        .preview_share_value_quota("coverage-share", revision, BTreeMap::new())
        .await
        .unwrap();
    assert_eq!(coverage(&pending)["reasonCode"], "tariff_not_effective");
    let grant:crate::domain::sharing::router_contract::ShareUserGrant=serde_json::from_value(json!({"email":"user@example.test","active":true,"policy":{"tokenPeriod":"lifetime","valueQuota":{"limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1"}}})).unwrap();
    let insert_grant = |store: &mut ShareStore| {
        store
            .shares
            .iter_mut()
            .find(|s| s.id == "coverage-share")
            .unwrap()
            .user_grants
            .insert("user@example.test".into(), grant.clone());
    };
    let previous_grants = state
        .shares
        .read()
        .await
        .get("coverage-share")
        .unwrap()
        .user_grants
        .clone();
    assert!(state
        .mutate_shares_immediate(insert_grant)
        .await
        .unwrap_err()
        .to_string()
        .starts_with("VALUE_QUOTA_PRICE_COVERAGE:"));
    assert_eq!(
        state
            .shares
            .read()
            .await
            .get("coverage-share")
            .unwrap()
            .user_grants,
        previous_grants
    );
    // Activation is local, with no Router call or heartbeat.
    ledger::schedule_tariff(
        &state.config_dir,
        &scope,
        &ledger::PendingTariff {
            revision: new.clone(),
            effective_from_ms: now - 1,
        },
        now,
    )
    .unwrap();
    assert_eq!(
        state
            .preview_share_value_quota("coverage-share", revision, BTreeMap::new())
            .await
            .unwrap()["capabilityReady"],
        true
    );
    state.mutate_shares_immediate(insert_grant).await.unwrap();
    let bound = state
        .shares
        .read()
        .await
        .get("coverage-share")
        .unwrap()
        .user_grants["user@example.test"]
        .clone();
    assert!(bound.value_metering.is_some());
    ledger::schedule_tariff(
        &state.config_dir,
        &scope,
        &ledger::PendingTariff {
            revision: old,
            effective_from_ms: now - 1,
        },
        now,
    )
    .unwrap();
    state.mutate_shares_immediate(|_| {}).await.unwrap();
    let after = state
        .shares
        .read()
        .await
        .get("coverage-share")
        .unwrap()
        .user_grants["user@example.test"]
        .clone();
    assert_eq!(
        serde_json::to_value(bound).unwrap(),
        serde_json::to_value(after).unwrap(),
        "a later missing price never resets stored quota bindings"
    );
}

#[tokio::test]
async fn restored_router_ack_and_withdrawn_pending_tariff_converge_without_rewriting_grants() {
    use crate::domain::usage::value_pricing::PricingSnapshot;
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = test_state();
    configure_registered_test_router(
        &state,
        &format!("http://{address}"),
        "restored-tariff-server",
    )
    .await;
    let config = state.config_snapshot().await;
    state
        .record_router_quota_capability(
            &config,
            client::RouterShareCapabilities {
                share_contract_version: 10,
                capabilities: vec![
                    "usdEquivalentQuotaV1".into(),
                    "valuePricingActivationV1".into(),
                ],
            },
        )
        .await;
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    let scope = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "restored-tariff-server".into(),
        share_id: "pricing".into(),
        email: "pricing@local.invalid".into(),
        entitlement_id: None,
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    let mut snapshot: PricingSnapshot =
        serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    let revision = snapshot.revision().unwrap();
    let now = now_ms_i64();
    ledger::install_snapshot(&state.config_dir, &scope, &revision, &snapshot, now).unwrap();
    ledger::schedule_tariff(
        &state.config_dir,
        &scope,
        &ledger::PendingTariff {
            revision: revision.clone(),
            effective_from_ms: now - 1,
        },
        now,
    )
    .unwrap();
    let manifest = json!({"installationId":"restored-tariff-server","revision":revision,"effectiveFromMs":now-1,"snapshot":snapshot});
    snapshot.models.insert(
        "qiniu/conatus-free".into(),
        snapshot.models.values().next().unwrap().clone(),
    );
    let withdrawn = snapshot.revision().unwrap();
    ledger::install_snapshot(&state.config_dir, &scope, &withdrawn, &snapshot, now).unwrap();
    ledger::schedule_tariff(
        &state.config_dir,
        &scope,
        &ledger::PendingTariff {
            revision: withdrawn,
            effective_from_ms: now + 60_000,
        },
        now,
    )
    .unwrap();
    let acknowledgements = Arc::new(AtomicUsize::new(0));
    let reports = Arc::new(AtomicUsize::new(0));
    let ack_count = acknowledgements.clone();
    let report_count = reports.clone();
    let app = Router::new()
        .route(
            "/v1/shares/value-pricing/fetch",
            post(move || {
                let manifest = manifest.clone();
                async move { Json(manifest) }
            }),
        )
        .route(
            "/v1/shares/value-pricing/ack",
            post(move |Json(body): Json<Value>| {
                let count = ack_count.clone();
                async move {
                    count.fetch_add(1, AtomicOrdering::SeqCst);
                    assert_eq!(body["storageSchema"], 3);
                    Json(json!({"ok":true,"revision":body["revision"]}))
                }
            }),
        )
        .route(
            "/v1/shares/value-pricing/active",
            post(move |Json(body): Json<Value>| {
                let count = report_count.clone();
                async move {
                    count.fetch_add(1, AtomicOrdering::SeqCst);
                    Json(json!({"ok":true,"revision":body["revision"]}))
                }
            }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    state.sync_share_value_pricing(&config).await.unwrap();
    assert_eq!(
        acknowledgements.load(AtomicOrdering::SeqCst),
        1,
        "a full manifest must restore missing ACK even when already active"
    );
    assert_eq!(
        ledger::active_revision(&state.config_dir, &scope).unwrap(),
        Some(revision)
    );
    assert!(
        ledger::pending_tariff(&state.config_dir, &scope)
            .unwrap()
            .is_none(),
        "the Router withdrew the other pending revision"
    );
    state.report_active_value_tariff(&config).await.unwrap();
    assert_eq!(reports.load(AtomicOrdering::SeqCst), 1);
    assert!(
        state.shares.read().await.shares.is_empty(),
        "delivery never creates a grant or changes a policy"
    );
    server.abort();
}
