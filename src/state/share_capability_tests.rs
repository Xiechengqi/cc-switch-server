// Included in state's integration tests: real persistence and runtime compilation, no upstream requests.
async fn capability_test_state() -> ServerState {
    let state = test_state();
    let mut config = state.config_snapshot().await;
    config.owner.email = Some("owner@example.com".into());
    state.replace_config(config).await.unwrap();
    state
        .configure_value_quota_router_for_test("capability-install")
        .await;
    let config = state.config_snapshot().await;
    state
        .record_router_quota_capability(
            &config,
            client::RouterShareCapabilities {
                share_contract_version: 13,
                capabilities: vec![
                    "usdEquivalentQuotaV1".into(),
                    "shareCapabilityPolicyV1".into(),
                ],
            },
        )
        .await;
    crate::repository::server_sqlite::activate_authority(&state.config_dir).unwrap();
    state
        .upsert_provider_command(
            AppKind::Codex,
            test_provider("cap-provider", "Capability provider"),
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
                .upsert(router_sync_share_input("cap-share", "cap-provider"))
                .unwrap();
            let share = store
                .shares
                .iter_mut()
                .find(|s| s.id == "cap-share")
                .unwrap();
            share.token_limit = Some(1_000_000);
            share.tokens_used = 42;
        })
        .await
        .unwrap();
    state
}
async fn capability_enable_search(
    state: &ServerState,
    operation_id: &str,
) -> crate::domain::sharing::router_contract::ShareSettingsPatch {
    use crate::domain::sharing::capabilities::*;
    let share = state
        .shares
        .read()
        .await
        .clone()
        .get("cap-share")
        .unwrap()
        .clone();
    let policy = ShareCapabilityPolicy {
        allowed_operations: vec![ShareOperation::WebSearch],
        ..Default::default()
    };
    let preview = state
        .preview_share_capabilities(
            &share.id,
            crate::state::share_capability::CapabilityPreviewInput {
                expected_config_revision: share.config_revision,
                share_token_limit: None,
                policy: Some(policy),
                policies: BTreeMap::new(),
                replace_policies: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(preview["ready"], true);
    assert!(preview["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["operation"] == "tool.web_search" && r["controllable"] == true));
    crate::domain::sharing::router_contract::ShareSettingsPatch {
        capability_policy: Some(serde_json::from_value(preview["policy"].clone()).unwrap()),
        capability_consent: Some(CapabilityConsent {
            credential: preview["previewCredential"].as_str().unwrap().into(),
            accept_uncovered: preview["requiredAcknowledgements"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a["condition"].as_str().unwrap().into())
                .collect(),
        }),
        edit_context: Some(
            crate::domain::sharing::router_contract::ShareSettingsEditContext {
                operation_id: operation_id.into(),
                expected_config_revision: share.config_revision,
                actor: Some("owner@example.com".into()),
            },
        ),
        ..Default::default()
    }
}
#[tokio::test]
async fn capability_policy_is_atomic_replayable_and_preserves_history() {
    let state = capability_test_state().await;
    let patch = capability_enable_search(&state, "cap-save").await;
    for credential in ["not-a-ticket", "gg.00", "00.gg"] {
        let mut malformed = patch.clone();
        malformed.capability_consent.as_mut().unwrap().credential = credential.into();
        assert!(state
            .apply_share_settings_patch_immediate("cap-share", malformed)
            .await
            .unwrap_err()
            .to_string()
            .contains("CAPABILITY_PREVIEW_EXPIRED"));
    }
    let mut missing = patch.clone();
    missing
        .capability_consent
        .as_mut()
        .unwrap()
        .accept_uncovered
        .clear();
    assert!(state
        .apply_share_settings_patch_immediate("cap-share", missing)
        .await
        .unwrap_err()
        .to_string()
        .contains("QUOTA_COVERAGE_REQUIRED"));
    assert!(state
        .shares
        .read()
        .await
        .clone()
        .get("cap-share")
        .unwrap()
        .capability_policy
        .as_ref()
        .unwrap()
        .allowed_operations
        .is_empty());
    let saved = state
        .apply_share_settings_patch_immediate("cap-share", patch.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.tokens_used, 42);
    assert_eq!(saved.token_limit, Some(1_000_000));
    assert!(saved
        .capability_policy
        .as_ref()
        .unwrap()
        .coverage_acknowledgements
        .iter()
        .all(|a| a.actor == "owner@example.com" && a.accepted_at_ms > 0));
    // Simulate the short in-document receipt window being pruned. A durable
    // receipt must precede revision/credential checks and survive restart.
    state
        .mutate_shares_immediate(|store| store.applied_settings_operations.clear())
        .await
        .unwrap();
    let replay = state
        .apply_share_settings_patch_immediate("cap-share", patch.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.config_revision, saved.config_revision);
    let mut conflicting = patch.clone();
    conflicting.description = Some(Some("different".into()));
    assert!(state
        .apply_share_settings_patch_immediate("cap-share", conflicting)
        .await
        .is_err());
    let persisted = crate::repository::server_sqlite::load_authoritative(&state.config_dir)
        .unwrap()
        .shares;
    assert_eq!(
        persisted.get("cap-share").unwrap().capability_policy,
        saved.capability_policy
    );
    let directory = state.config_dir.clone();
    drop(state);
    let restored = test_state_at(directory);
    let before_replay = restored
        .shares
        .read()
        .await
        .get("cap-share")
        .unwrap()
        .clone();
    assert_eq!(before_replay.capability_policy, saved.capability_policy);
    assert_eq!(before_replay.tokens_used, 42);
    assert_eq!(
        restored
            .apply_share_settings_patch_immediate("cap-share", patch.clone())
            .await
            .unwrap()
            .unwrap()
            .config_revision,
        before_replay.config_revision
    );
    restored
        .delete_share_immediate("cap-share")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        restored
            .apply_share_settings_patch_immediate("cap-share", patch)
            .await
            .unwrap(),
        Err(SharePatchError::NotFound)
    ));
}
#[tokio::test]
async fn capability_gate_checks_owner_token_grants_and_changed_runtime_before_prices() {
    use crate::domain::sharing::capabilities::*;
    let state = capability_test_state().await;
    let runtime = state
        .providers
        .read()
        .await
        .runtime_plan(AppKind::Codex, "cap-provider")
        .unwrap();
    let mut plan = crate::domain::usage::value_pricing::resource_request_plan(
        &json!({"tools":[{"type":"web_search"}]}),
        crate::domain::usage::store::InferenceOperation::Responses,
    );
    plan.binding_fingerprint =
        Some(crate::domain::sharing::capability_intent::binding_fingerprint(&runtime));
    let context = crate::domain::usage::store::UsageLogContext {
        share_id: Some("cap-share".into()),
        request_id: Some("cap-request".into()),
        quota_app: Some(AppKind::Codex),
        operation: crate::domain::usage::store::InferenceOperation::Responses,
        ..Default::default()
    };
    let attempt = state
        .admit_share_value_request(
            &context,
            "unpriced-model",
            Some("standard"),
            Default::default(),
            true,
            Some(plan.clone()),
        )
        .await;
    assert!(attempt
        .err()
        .expect("disabled operation must be rejected")
        .to_string()
        .contains("SHARE_CAPABILITY_DISABLED"));
    let patch = capability_enable_search(&state, "cap-enable").await;
    state
        .apply_share_settings_patch_immediate("cap-share", patch)
        .await
        .unwrap()
        .unwrap();
    assert!(state
        .admit_share_value_request(
            &context,
            "unpriced-model",
            Some("standard"),
            Default::default(),
            true,
            Some(plan.clone())
        )
        .await
        .unwrap()
        .is_none());
    let config = state.config_snapshot().await;
    let providers = state.providers.read().await;
    let mut share = state
        .shares
        .read()
        .await
        .clone()
        .get("cap-share")
        .unwrap()
        .clone();
    share
        .user_grants
        .get_mut("owner@example.com")
        .unwrap()
        .policy
        .allowed_operations = Some(vec![]);
    assert!(state
        .check_share_capabilities(
            &share,
            &UsageLogContext {
                user_email: Some("owner@example.com".into()),
                ..context.clone()
            },
            "unpriced-model",
            Some(&plan),
            &providers,
            &config
        )
        .unwrap_err()
        .to_string()
        .contains("SHARE_CAPABILITY_DISABLED"));
    plan.binding_fingerprint = Some("f".repeat(64));
    assert!(state
        .check_share_capabilities(&share, &context, "model", Some(&plan), &providers, &config)
        .unwrap_err()
        .to_string()
        .contains("CAPABILITY_STATE_CHANGED"));
    let mut next = share.clone();
    next.capability_policy = None;
    let previous = crate::domain::sharing::shares::ShareStore {
        shares: vec![share],
        ..Default::default()
    };
    let candidate = crate::domain::sharing::shares::ShareStore {
        shares: vec![next],
        ..Default::default()
    };
    assert!(state
        .validate_capability_transition(&previous, &candidate, &providers, &config)
        .is_err());
    let _ = ShareOperation::WebSearch;
}

#[tokio::test]
async fn capability_new_user_requires_fresh_scope_consent_and_market_never_inherits() {
    let state = capability_test_state().await;
    let patch = capability_enable_search(&state, "cap-original").await;
    state
        .apply_share_settings_patch_immediate("cap-share", patch)
        .await
        .unwrap()
        .unwrap();
    let before = state.shares.read().await.get("cap-share").unwrap().clone();
    let make_grant = || crate::domain::sharing::router_contract::ShareUserGrant {
        email: "new@example.test".into(),
        active: true,
        policy: crate::domain::sharing::router_contract::ShareUserPolicy {
            token_limit: Some(100),
            ..Default::default()
        },
        ..Default::default()
    };
    let failure = state
        .mutate_shares_immediate(|store| {
            store
                .shares
                .iter_mut()
                .find(|s| s.id == "cap-share")
                .unwrap()
                .user_grants
                .insert("new@example.test".into(), make_grant());
        })
        .await
        .unwrap_err();
    assert!(
        failure.to_string().contains("QUOTA_COVERAGE_REQUIRED"),
        "{failure}"
    );
    assert_eq!(
        state
            .shares
            .read()
            .await
            .get("cap-share")
            .unwrap()
            .config_revision,
        before.config_revision
    );
    assert!(!state
        .shares
        .read()
        .await
        .get("cap-share")
        .unwrap()
        .user_grants
        .contains_key("new@example.test"));
    let error = state
        .mutate_shares_immediate(|store| {
            let mut grant = make_grant();
            grant.manager =
                crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket;
            grant.entitlement_id = Some("market-explicit".into());
            store
                .shares
                .iter_mut()
                .find(|s| s.id == "cap-share")
                .unwrap()
                .user_grants
                .insert("new@example.test".into(), grant);
        })
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("MARKET_CAPABILITY_CONFLICT"),
        "{error}"
    );
}

#[tokio::test]
async fn capability_continuation_inherits_hosted_controls_without_recharging_old_understanding_inputs(
) {
    use crate::domain::sharing::capabilities::ShareOperation;
    use crate::domain::usage::value_pricing::{resource_request_plan, ResourcePlan};
    use crate::repository::server_sqlite;
    let state = capability_test_state().await;
    let runtime = state
        .providers
        .read()
        .await
        .runtime_plan(AppKind::Codex, "cap-provider")
        .unwrap();
    let fingerprint = crate::domain::sharing::capability_intent::binding_fingerprint(&runtime);
    let context = UsageLogContext {
        share_id: Some("cap-share".into()),
        request_id: Some("continuation".into()),
        quota_app: Some(AppKind::Codex),
        operation: crate::domain::usage::store::InferenceOperation::Responses,
        ..Default::default()
    };
    let scope =
        server_sqlite::capability_context::scope(&state.config_snapshot().await, &context).unwrap();
    let conn = rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir)).unwrap();
    for (response, operations, resource) in [
        ("understanding", vec![], "audio.input_token"),
        ("hosted", vec![ShareOperation::WebSearch], "tool.web_search"),
    ] {
        let parent = ResourcePlan {
            operations,
            binding_fingerprint: Some(fingerprint.clone()),
            requires_v2: true,
            text_tokens: true,
            quantities: BTreeMap::from([(resource.into(), 128_000)]),
            ..Default::default()
        };
        conn.execute(
            "INSERT INTO share_capability_contexts VALUES(?1,?2,?3,?4,?5)",
            rusqlite::params![
                scope,
                fingerprint,
                response,
                serde_json::to_string(&parent).unwrap(),
                (crate::infra::time::now_ms() as i64) + 86_400_000
            ],
        )
        .unwrap();
    }
    let mut plan = resource_request_plan(
        &json!({"previous_response_id":"understanding"}),
        context.operation,
    );
    plan.binding_fingerprint = Some(fingerprint.clone());
    assert!(state
        .admit_share_value_request(
            &context,
            "text-model",
            Some("standard"),
            Default::default(),
            true,
            Some(plan)
        )
        .await
        .unwrap()
        .is_none());
    let saved: String = conn.query_row("SELECT plan_json FROM share_capability_admissions WHERE request_id='continuation' AND attempt=1", [], |row| row.get(0)).unwrap();
    let saved: ResourcePlan = serde_json::from_str(&saved).unwrap();
    assert!(!saved.requires_v2);
    assert!(saved.quantities.is_empty());
    let mut plan =
        resource_request_plan(&json!({"previous_response_id":"hosted"}), context.operation);
    plan.binding_fingerprint = Some(fingerprint);
    let error = state
        .admit_share_value_request(
            &UsageLogContext {
                request_id: Some("hosted-continuation".into()),
                ..context
            },
            "text-model",
            Some("standard"),
            Default::default(),
            true,
            Some(plan),
        )
        .await
        .err()
        .unwrap();
    assert!(
        error.to_string().contains("SHARE_CAPABILITY_DISABLED"),
        "{error}"
    );
}

#[tokio::test]
async fn capability_token_usage_shows_native_counts_without_inventing_usd_or_reusing_retry_facts() {
    use crate::domain::sharing::capability_intent::binding_fingerprint;
    use crate::domain::usage::store::InferenceOperation;
    use crate::domain::usage::value_pricing::{
        resource_request_plan, ResourceUsage, RESOURCE_ESTIMATE_VERSION,
    };
    let state = capability_test_state().await;
    let patch = capability_enable_search(&state, "token-native-facts").await;
    state
        .apply_share_settings_patch_immediate("cap-share", patch)
        .await
        .unwrap()
        .unwrap();
    let config = state.config_snapshot().await;
    let runtime = state
        .providers_snapshot()
        .await
        .runtime_plan(AppKind::Codex, "cap-provider")
        .unwrap();
    let context = UsageLogContext {
        share_id: Some("cap-share".into()),
        user_email: Some("owner@example.com".into()),
        quota_app: Some(AppKind::Codex),
        operation: InferenceOperation::Responses,
        request_id: Some("token-resource-request".into()),
        ..Default::default()
    };
    let mut plan =
        resource_request_plan(&json!({"tools":[{"type":"web_search"}]}), context.operation);
    plan.binding_fingerprint = Some(binding_fingerprint(&runtime));
    assert!(state
        .admit_share_value_request(
            &context,
            "text-model",
            Some("standard"),
            Default::default(),
            true,
            Some(plan.clone())
        )
        .await
        .unwrap()
        .is_none());
    let mut log = UsageLog::new(
        AppKind::Codex,
        "cap-provider".into(),
        "Capability provider".into(),
        ProviderType::Codex,
        200,
        1,
        UsageModelMetadata::default(),
        TokenUsage::default(),
    );
    log.apply_context(context.clone());
    log.value_resources = Some(ResourceUsage {
        response_id: Some("token-resource-response".into()),
        quantities: BTreeMap::from([("tool.web_search".into(), 2)]),
        source: "upstream.protocol".into(),
        estimated: false,
        version: RESOURCE_ESTIMATE_VERSION.into(),
    });
    state.push_usage_log(log).await.unwrap();
    // The next attempt has no observation yet. The old attempt's usage must
    // remain attached to attempt 1; nothing fabricates actual counts for 2.
    let retry = UsageLogContext {
        attempt_count: Some(2),
        ..context
    };
    state
        .admit_share_value_request(
            &retry,
            "text-model",
            Some("standard"),
            Default::default(),
            true,
            Some(plan),
        )
        .await
        .unwrap();
    let recent = crate::repository::server_sqlite::capability_context::recent(
        &state.config_dir,
        config.router_api_base().unwrap(),
        &config.registered_router_identity().unwrap().installation_id,
        "cap-share",
    )
    .unwrap();
    let observed = recent.iter().find(|row| row["attempt"] == 1).unwrap();
    assert_eq!(observed["dimensions"][0]["quantity"], 2);
    assert_eq!(observed["dimensions"][0]["pricing"], "not_in_usd_ledger");
    assert!(observed["dimensions"][0]["amountPicos"].is_null());
    assert!(observed["settledPicos"].is_null());
    assert_eq!(observed["estimated"], false);
    assert!(
        recent.iter().find(|row| row["attempt"] == 2).unwrap()["dimensions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
