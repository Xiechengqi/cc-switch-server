use super::*;
use crate::clients::router::client::RouterShareCapabilities;
use crate::domain::usage::value_pricing::PricingSnapshot;
use crate::repository::server_sqlite::{self, value_quota as ledger};

async fn fixture() -> (ServerState, Value) {
    let fixture_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_dir = std::env::temp_dir().join(format!("cc-switch-initial-value-{fixture_id}"));
    let state = crate::state::ServerStateInner::load(
        crate::cli::Cli {
            host: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
            config_dir: Some(config_dir),
            web_dist_dir: None,
            log_level: "warn".into(),
            command: None,
        },
        std::sync::Arc::new(crate::logging::LogCapture::new(
            crate::logging::RING_BUFFER_CAPACITY,
        )),
    )
    .unwrap();
    let mut config = state.config_snapshot().await;
    config.owner.email = Some("owner@example.test".into());
    state.replace_config(config).await.unwrap();
    state
        .configure_value_quota_router_for_test("opening-bundle-install")
        .await;
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    state.upsert_provider_bundle_command(serde_json::from_value(json!({
        "id":"opening-bundle","familyId":"family.custom_http","name":"Opening bundle","modelPolicy":"passthrough",
        "surfaces":[{"app":"claude","enabled":false,"profileId":"claude.custom_http",
          "customBinding":{"upstreamProtocol":"anthropic_messages","authScheme":"bearer"}},
          {"app":"gemini","enabled":false,"profileId":"gemini.custom_http",
          "customBinding":{"upstreamProtocol":"gemini_native","authScheme":"api_key"}},
          {"app":"codex","enabled":true,"profileId":"codex.custom_http","endpoint":"http://127.0.0.1:9/v1",
          "customBinding":{"upstreamProtocol":"open_ai_responses","authScheme":"bearer"},
          "credentialPatches":{"/settingsConfig/apiKey":{"action":"replace","value":"fixture-only-key"}}}]
    })).unwrap()).await.unwrap().unwrap();
    let config = state.config_snapshot().await;
    state
        .record_router_quota_capability(
            &config,
            RouterShareCapabilities {
                share_contract_version: 12,
                capabilities: vec![
                    "usdEquivalentQuotaV1".into(),
                    "quotaUnitConversionV1".into(),
                ],
            },
        )
        .await;
    let scope = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "opening-bundle-install".into(),
        share_id: "unused".into(),
        email: "owner@example.test".into(),
        entitlement_id: None,
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    let pricing: PricingSnapshot = serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    let revision = pricing.revision().unwrap();
    ledger::install_snapshot(
        &state.config_dir,
        &scope,
        &revision,
        &pricing,
        crate::infra::time::now_ms() as i64,
    )
    .unwrap();
    ledger::activate_snapshot(&state.config_dir, &scope, &revision).unwrap();
    let mut command = json!({"bundleId":"opening-bundle","enabled":true,"subdomain":"opening-bundle",
        "tokenLimit":-1,"parallelLimit":-1,"expiresAt":"2099-01-01T00:00:00Z"});
    let share = web_save_provider_bundle_share(&state, &command, None)
        .await
        .unwrap()
        .unwrap();
    crate::state::stop_share_tunnel(&state, &share.id).await;
    command["shareId"] = json!(share.id);
    command["expectedConfigRevision"] = json!(share.config_revision);
    command["enabled"] = json!(false);
    let policy = json!({"tokenPeriod":"lifetime","valueQuota":{"limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1"}});
    let preview = state
        .preview_share_value_quota(
            &share.id,
            share.config_revision,
            BTreeMap::from([(
                "new@example.test".into(),
                serde_json::from_value(policy.clone()).unwrap(),
            )]),
        )
        .await
        .unwrap();
    assert_eq!(preview["ready"], true, "{preview}");
    let row = &preview["rows"][0];
    assert_eq!(row["canInitializeUsage"], true, "{preview}");
    command["userGrantEdits"] = json!({"new@example.test":{"expectedRevision":0,"policy":policy}});
    command["valueUsageOperationId"] = json!("fixture-operation");
    command["userQuotaConversions"] =
        json!({"new@example.test":{"credential":row["conversionCredential"]}});
    command["userValueUsageInitializations"] = json!({"new@example.test":{"targetPicos":"123456789123",
        "windowId":row["windowId"],"reason":"Verified opening usage"}});
    (state, command)
}

#[tokio::test]
async fn native_bundle_opening_usd_usage_is_atomic_replayable_and_never_succeeds_for_a_deleted_share(
) {
    let (state, command) = fixture().await;
    let saved = web_save_provider_bundle_share(&state, &command, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.user_grants["new@example.test"]
            .quota_conversion_id
            .as_deref(),
        Some("bundle-value:fixture-operation")
    );
    assert!(saved.user_grants["new@example.test"]
        .value_metering
        .is_some());
    let statuses = state.value_quota_edit_statuses(&saved.id).await.unwrap();
    assert_eq!(
        statuses
            .iter()
            .find(|row| row["email"] == "new@example.test")
            .unwrap()["usedPicos"],
        "123456789123"
    );
    let again = web_save_provider_bundle_share(&state, &command, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.config_revision, saved.config_revision);
    let db = rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir)).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM value_quota_adjustments", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        1
    );
    let result = state
        .share_quota_conversion_result(&saved.id, "bundle-value:fixture-operation")
        .await
        .unwrap();
    assert_eq!(result["committed"], true, "{result}");
    assert_eq!(
        result["settingsReceipt"]["configRevision"],
        saved.config_revision
    );
    assert_eq!(result["quotaConversions"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["quotaConversions"][0]["status"]["usedPicos"],
        "123456789123"
    );
    let mut changed = command.clone();
    changed["userValueUsageInitializations"]["new@example.test"]["targetPicos"] = json!("1");
    assert_eq!(
        web_save_provider_bundle_share(&state, &changed, None)
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    state
        .delete_share_immediate(&saved.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        web_save_provider_bundle_share(&state, &command, None)
            .await
            .unwrap_err()
            .status,
        StatusCode::NOT_FOUND
    );
    crate::state::stop_client_tunnel(&state).await;
}

#[tokio::test]
async fn native_bundle_invalid_opening_usd_usage_keeps_grants_ledger_and_receipts_unchanged() {
    let (state, mut command) = fixture().await;
    command["userValueUsageInitializations"]["new@example.test"]["targetPicos"] = json!("-1");
    let error = web_save_provider_bundle_share(&state, &command, None)
        .await
        .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST, "{error:?}");
    assert_eq!(error.code, Some("quota_target_invalid"), "{error:?}");
    let durable = server_sqlite::load_authoritative(&state.config_dir)
        .unwrap()
        .shares;
    assert!(!durable
        .get(command["shareId"].as_str().unwrap())
        .unwrap()
        .user_grants
        .contains_key("new@example.test"));
    assert!(!durable
        .applied_settings_operations
        .contains_key("bundle-value:fixture-operation"));
    let db = rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir)).unwrap();
    assert_eq!(
        db.query_row("SELECT count(*) FROM value_quota_adjustments", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM quota_conversion_receipts",
            [],
            |row| { row.get::<_, i64>(0) }
        )
        .unwrap(),
        0
    );
    command["userValueUsageInitializations"]["new@example.test"]["targetPicos"] = json!("1");
    command["userUsageEdits"] = json!({"new@example.test":{"action":"set","targetTokens":1}});
    assert_eq!(
        web_save_provider_bundle_share(&state, &command, None)
            .await
            .unwrap_err()
            .status,
        StatusCode::BAD_REQUEST
    );
    crate::state::stop_client_tunnel(&state).await;
}
