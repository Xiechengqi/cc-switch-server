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
                share_contract_version: 14,
                capabilities: vec![
                    "usdEquivalentQuotaV1".into(),
                    "shareCapabilityPolicyV1".into(),
                    "shareGrantCapabilitiesV1".into(),
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
            // Explicit v13 fixture for the historical settings and migration tests.
            share.grant_capabilities = None;
            share.capability_policy = Some(Default::default());
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
    assert!(before_replay.capability_policy.is_none());
    let migrated = before_replay.grant_capabilities.as_ref().unwrap();
    assert_eq!(migrated.len(), 1);
    assert_eq!(
        migrated["owner@example.com"].allowed_operations,
        vec![crate::domain::sharing::capabilities::ShareOperation::WebSearch]
    );
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
            None,
            &config
        )
        .unwrap_err()
        .to_string()
        .contains("SHARE_CAPABILITY_DISABLED"));
    plan.binding_fingerprint = Some("f".repeat(64));
    assert!(state
        .check_share_capabilities(
            &share,
            &context,
            "model",
            Some(&plan),
            &providers,
            None,
            &config
        )
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

#[tokio::test]
async fn capability_display_separates_driver_configuration_account_and_price_evidence() {
    use crate::domain::providers::{registry::DriverId, share_operations::*};
    use crate::domain::sharing::capabilities::ShareOperation::*;
    let state = capability_test_state().await;
    let share = state.shares.read().await.get("cap-share").unwrap().clone();
    let preview = state
        .preview_share_capabilities(
            "cap-share",
            crate::state::share_capability::CapabilityPreviewInput {
                expected_config_revision: share.config_revision,
                share_token_limit: None,
                policy: None,
                policies: BTreeMap::new(),
                replace_policies: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(preview["displayVersion"], 2);
    let row = |op: &str| {
        preview["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["operation"] == op)
            .unwrap()
    };
    assert_eq!(row("tool.web_search")["implemented"], true);
    assert_eq!(row("tool.web_search")["accountSupport"], "unknown");
    assert_eq!(row("tool.web_search")["unavailableReason"], Value::Null);
    assert_eq!(row("tool.computer")["implemented"], false);
    assert_eq!(row("tool.computer")["accountSupport"], "unknown");
    assert_eq!(
        row("tool.computer")["unavailableReason"],
        "driver_not_implemented"
    );
    let mut plan = state
        .providers
        .read()
        .await
        .runtime_plan(AppKind::Codex, "cap-provider")
        .unwrap()
        .as_ref()
        .clone();
    plan.driver_id = DriverId::parse("oauth.openai_codex").unwrap();
    assert!(driver_implements(&plan, ImageGenerate));
    assert!(!provider_enabled(&plan, ImageGenerate));
    assert!(!implemented(&plan, ImageGenerate));
    plan.driver_options
        .insert("codexImageGenerationEnabled".into(), json!(true));
    assert!(implemented(&plan, ImageGenerate));
    plan.driver_id = DriverId::parse("oauth.grok_responses").unwrap();
    for op in [
        FileSearch,
        CodeExecution,
        AudioGenerate,
        Computer,
        RemoteMcp,
    ] {
        assert!(!driver_implements(&plan, op), "{op:?}");
    }
    assert!(driver_implements(&plan, WebSearch));
    assert!(!implemented(&plan, VideoGenerate));
    for driver in ["special.antigravity", "special.agy"] {
        plan.driver_id = DriverId::parse(driver).unwrap();
        assert!(implemented(&plan, WebSearch));
        assert!(!implemented(&plan, ImageGenerate));
    }
}

#[tokio::test]
async fn capability_only_edit_preserves_scheduled_financial_terms_usage_and_activation() {
    use crate::domain::sharing::{capabilities::*, router_contract::*};
    use crate::domain::usage::value_pricing::{PricingSnapshot, ValueMeteringBinding, ValueQuota};
    use crate::repository::server_sqlite::value_quota as ledger;
    let state = capability_test_state().await;
    let config = state.config_snapshot().await;
    let snapshot: PricingSnapshot = serde_json::from_value(json!({
        "schemaVersion":2,"tariffPolicyId":"api-equivalent-v2","currency":"USD","exactAliases":{},
        "models":{"model":{"contextInputBasis":"rawInput","longContextThreshold":null,"longContextInclusive":false,
            "rates":{"standard/base":{"input":"1000000","output":"1000000","cacheRead":null,"cacheWrite5m":null,"cacheWrite1h":null,"cacheWritePolicy":"unavailable"}},
            "resources":{"tool.web_search":"1000000000"}}}
    })).unwrap();
    let revision = snapshot.revision().unwrap();
    let identity = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "capability-install".into(),
        share_id: "cap-share".into(),
        email: "owner@example.com".into(),
        entitlement_id: None,
    };
    ledger::install_snapshot(
        &state.config_dir,
        &identity,
        &revision,
        &snapshot,
        now_ms_i64(),
    )
    .unwrap();
    ledger::activate_snapshot(&state.config_dir, &identity, &revision).unwrap();
    let boundary = (now_ms_i64() / 86_400_000 + 1) * 86_400_000;
    state
        .mutate_shares_immediate(|store| {
            let grant = store
                .shares
                .iter_mut()
                .find(|s| s.id == "cap-share")
                .unwrap()
                .user_grants
                .get_mut("owner@example.com")
                .unwrap();
            grant.policy.token_period = ShareTokenPeriod::Day;
            grant.policy.token_limit = Some(100);
        })
        .await
        .unwrap();
    let mut recorded = UsageLog::new(
        AppKind::Codex,
        "cap-provider".into(),
        "Capability provider".into(),
        ProviderType::Codex,
        200,
        1,
        UsageModelMetadata {
            model: Some("model".into()),
            ..Default::default()
        },
        TokenUsage {
            input_tokens: Some(17),
            output_tokens: Some(20),
            total_tokens: Some(37),
            ..Default::default()
        },
    );
    recorded.apply_context(UsageLogContext {
        request_id: Some("before-capability-edit".into()),
        share_id: Some("cap-share".into()),
        user_email: Some("owner@example.com".into()),
        ..Default::default()
    });
    state.push_usage_log(recorded).await.unwrap();
    state
        .record_share_invocation_result("cap-share", Some("owner@example.com"), 37, now_ms_i64())
        .await;
    state
        .mutate_shares_immediate(|store| {
            let share = store
                .shares
                .iter_mut()
                .find(|s| s.id == "cap-share")
                .unwrap();
            let grant = share.user_grants.get_mut("owner@example.com").unwrap();
            grant.policy.token_period = ShareTokenPeriod::Day;
            grant.policy.token_limit = Some(100);
            let mut future = grant.policy.clone();
            future.token_limit = None;
            future.value_quota = Some(ValueQuota {
                limit_usd_micros: "1000000".into(),
                tariff_policy_id: "api-equivalent-v1".into(),
                unpriced_usage: Default::default(),
            });
            grant.pending_value_quota = Some(PendingValueQuota {
                policy: future,
                effective_from_ms: boundary,
                retain_access: true,
            });
            grant.value_metering = Some(ValueMeteringBinding {
                quota_policy_epoch: "legacy".into(),
                value_metered_from_ms: boundary,
                effective_from_ms: boundary,
                pricing_revision: revision.clone(),
            });
        })
        .await
        .unwrap();
    let before = state.shares.read().await.get("cap-share").unwrap().clone();
    let old_grant = &before.user_grants["owner@example.com"];
    let mut requested = old_grant
        .pending_value_quota
        .as_ref()
        .unwrap()
        .policy
        .clone();
    requested.allowed_operations = Some(vec![ShareOperation::WebSearch]);
    requested.value_quota.as_mut().unwrap().tariff_policy_id = "api-equivalent-v2".into();
    let policies = BTreeMap::from([("owner@example.com".into(), requested.clone())]);
    let preview = state
        .preview_share_capabilities(
            "cap-share",
            crate::state::share_capability::CapabilityPreviewInput {
                expected_config_revision: before.config_revision,
                share_token_limit: None,
                policy: Some(ShareCapabilityPolicy {
                    allowed_operations: vec![ShareOperation::WebSearch],
                    ..Default::default()
                }),
                policies,
                replace_policies: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(preview["blockers"], json!([]));
    let units = preview["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["operation"] == "tool.web_search")
        .unwrap()["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["scope"] == "owner@example.com")
        .map(|s| s["quotaUnit"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(
        units.contains(&"tokens") && units.contains(&"usdEquivalent"),
        "{units:?}"
    );
    let patch = ShareSettingsPatch {
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
        user_grant_edits: Some(BTreeMap::from([(
            "owner@example.com".into(),
            ShareUserGrantEdit {
                expected_revision: old_grant.revision,
                policy: Some(requested),
            },
        )])),
        edit_context: Some(ShareSettingsEditContext {
            operation_id: "scheduled-capability-edit".into(),
            expected_config_revision: before.config_revision,
            actor: Some("owner@example.com".into()),
        }),
        ..Default::default()
    };
    let mut rejected = patch.clone();
    rejected
        .capability_consent
        .as_mut()
        .unwrap()
        .accept_uncovered
        .clear();
    assert!(state
        .apply_share_settings_patch_immediate("cap-share", rejected)
        .await
        .unwrap_err()
        .to_string()
        .contains("QUOTA_COVERAGE_REQUIRED"));
    let financial = |grant: &ShareUserGrant| {
        (
            grant.policy.clone(),
            grant.pending_value_quota.clone(),
            grant.value_metering.clone(),
            grant.usage.day.tokens_used,
            grant.usage.lifetime.tokens_used,
            grant.usage_rebase.clone(),
            grant.revision,
        )
    };
    let rejected_state = state.shares.read().await.get("cap-share").unwrap().clone();
    assert_eq!(
        financial(&rejected_state.user_grants["owner@example.com"]),
        financial(old_grant)
    );
    assert_eq!(rejected_state.config_revision, before.config_revision);
    assert_eq!(rejected_state.capability_policy, before.capability_policy);
    let saved = state
        .apply_share_settings_patch_immediate("cap-share", patch)
        .await
        .unwrap()
        .unwrap();
    let grant = &saved.user_grants["owner@example.com"];
    assert_eq!(grant.policy.token_limit, Some(100));
    assert!(grant.policy.value_quota.is_none());
    assert_eq!(
        grant.policy.allowed_operations,
        Some(vec![ShareOperation::WebSearch])
    );
    let pending = grant.pending_value_quota.as_ref().unwrap();
    assert_eq!(pending.effective_from_ms, boundary);
    assert!(pending.retain_access);
    assert_eq!(
        pending
            .policy
            .value_quota
            .as_ref()
            .unwrap()
            .limit_usd_micros,
        "1000000"
    );
    assert_eq!(
        pending
            .policy
            .value_quota
            .as_ref()
            .unwrap()
            .tariff_policy_id,
        "api-equivalent-v2"
    );
    assert_eq!(
        pending.policy.allowed_operations,
        grant.policy.allowed_operations
    );
    assert_eq!(grant.value_metering, old_grant.value_metering);
    assert_eq!(grant.usage.day.tokens_used, 37);
    assert_eq!(grant.usage.day.tokens_used, old_grant.usage.day.tokens_used);
    assert_eq!(
        grant.usage.lifetime.tokens_used,
        old_grant.usage.lifetime.tokens_used
    );
    let persisted = crate::repository::server_sqlite::load_authoritative(&state.config_dir)
        .unwrap()
        .shares;
    assert_eq!(
        financial(&persisted.get("cap-share").unwrap().user_grants["owner@example.com"]),
        financial(grant)
    );
    let before_store = state.shares.read().await.clone();
    let mut activated = before_store.clone();
    state
        .prepare_value_bindings_at(&mut activated, &before_store, boundary)
        .await
        .unwrap();
    let active = &activated.get("cap-share").unwrap().user_grants["owner@example.com"];
    assert!(active.pending_value_quota.is_none());
    assert_eq!(active.policy, pending.policy);
}

#[tokio::test]
async fn grant_capability_save_is_personal_signed_and_replayable() {
    use crate::domain::sharing::capabilities::{
        CapabilityConsent, GrantCapabilityEdit, ShareOperation,
    };
    use crate::domain::sharing::router_contract::{
        ShareSettingsEditContext, ShareSettingsPatch, ShareUserGrantEdit, ShareUserPolicy,
    };
    let state = capability_test_state().await;
    let providers = state.providers.read().await.clone();
    assert_eq!(
        state
            .mutate_shares_immediate(|store| {
                assert_eq!(
                    share_capability::migrate_grant_capabilities(store, &providers),
                    1
                );
                share_capability::migrate_grant_capabilities(store, &providers)
            })
            .await
            .unwrap(),
        0
    );

    let before = state.shares.read().await.get("cap-share").unwrap().clone();
    let email = "alice@example.com";
    let operation = ShareOperation::WebSearch;
    let quota_edits = BTreeMap::from([(
        email.into(),
        ShareUserGrantEdit {
            expected_revision: 0,
            policy: Some(ShareUserPolicy::default()),
        },
    )]);
    let capability_edits = BTreeMap::from([(
        email.into(),
        GrantCapabilityEdit {
            expected_revision: 0,
            operations: vec![operation],
            reconfirm: vec![],
        },
    )]);
    let preview = state
        .preview_share_permissions(
            "cap-share",
            json!({
                "expectedConfigRevision":before.config_revision,
                "grantCapabilityEdits":capability_edits,"userGrantEdits":quota_edits
            }),
        )
        .await
        .unwrap();
    assert!(preview.get("recentUsage").is_none());
    let required = preview["requiredAcknowledgements"].as_array().unwrap();
    assert_eq!(required.len(), 1);
    assert_eq!(required[0]["scope"], email);
    let mut patch = ShareSettingsPatch {
        user_grant_edits: Some(quota_edits),
        grant_capability_edits: Some(capability_edits),
        capability_consent: Some(CapabilityConsent {
            credential: preview["previewCredential"].as_str().unwrap().into(),
            accept_uncovered: vec![],
        }),
        edit_context: Some(ShareSettingsEditContext {
            operation_id: "personal-capability-1".into(),
            expected_config_revision: before.config_revision,
            actor: None,
        }),
        ..Default::default()
    };
    assert!(state
        .apply_share_settings_patch_immediate("cap-share", patch.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("QUOTA_COVERAGE_REQUIRED"));
    patch
        .capability_consent
        .as_mut()
        .unwrap()
        .accept_uncovered
        .push(required[0]["condition"].as_str().unwrap().into());
    let saved = state
        .apply_share_settings_patch_immediate("cap-share", patch.clone())
        .await
        .unwrap()
        .unwrap();
    let replay = state
        .apply_share_settings_patch_immediate("cap-share", patch.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.config_revision, saved.config_revision);
    let persisted =
        crate::repository::server_sqlite::load_authoritative(&state.config_dir).unwrap();
    assert_eq!(
        persisted
            .shares
            .get("cap-share")
            .unwrap()
            .grant_capabilities,
        saved.grant_capabilities
    );
    assert_eq!(saved.tokens_used, before.tokens_used);
    assert!(saved.capability_policy.is_none());
    let permissions = saved.grant_capabilities.as_ref().unwrap();
    assert_eq!(permissions.len(), 1);
    assert_eq!(permissions[email].allowed_operations, vec![operation]);
    assert_eq!(
        permissions[email].coverage_acknowledgements[0].actor,
        "owner@example.com"
    );

    let runtime = providers
        .runtime_plan(AppKind::Codex, "cap-provider")
        .unwrap();
    let mut plan = crate::domain::usage::value_pricing::resource_request_plan(
        &json!({"tools":[{"type":"web_search"}]}),
        crate::domain::usage::store::InferenceOperation::Responses,
    );
    plan.binding_fingerprint =
        Some(crate::domain::sharing::capability_intent::binding_fingerprint(&runtime));
    let mut context = crate::domain::usage::store::UsageLogContext {
        quota_app: Some(AppKind::Codex),
        operation: crate::domain::usage::store::InferenceOperation::Responses,
        user_email: Some(email.into()),
        ..Default::default()
    };
    let config = state.config_snapshot().await;
    assert!(state
        .check_share_capabilities(
            &saved,
            &context,
            "model",
            Some(&plan),
            &providers,
            None,
            &config
        )
        .is_ok());
    context.user_email = Some("public@example.com".into());
    assert!(state
        .check_share_capabilities(
            &saved,
            &context,
            "model",
            Some(&plan),
            &providers,
            None,
            &config
        )
        .is_err());
    context.user_email = Some("owner@example.com".into());
    assert!(state
        .check_share_capabilities(
            &saved,
            &context,
            "model",
            Some(&plan),
            &providers,
            None,
            &config
        )
        .is_err());

    let mut conflict = patch;
    conflict
        .grant_capability_edits
        .as_mut()
        .unwrap()
        .get_mut(email)
        .unwrap()
        .operations
        .clear();
    assert!(state
        .apply_share_settings_patch_immediate("cap-share", conflict)
        .await
        .is_err());

    let mut descriptor =
        descriptor_for_share_with_accounts_and_usage(&saved, &providers, None, None);
    apply_value_descriptor_projection(&mut descriptor, false, false, false).unwrap();
    assert_eq!(descriptor.contract_version, 14);

    let incomplete = state
        .mutate_providers_immediate(|providers| {
            providers.upsert(
                AppKind::Claude,
                test_provider("missing-runtime", "Incomplete provider"),
            )
        })
        .await
        .unwrap();
    state
        .mutate_shares_immediate(|store| {
            let share = &mut store.shares[0];
            let mut missing = share.bindings[0].clone();
            missing.app = AppKind::Claude;
            missing.provider_id = "missing-runtime".into();
            missing.provider_type = incomplete.provider_type;
            share.bindings.push(missing);
            share.enabled_apps = None;
        })
        .await
        .unwrap();
    // The stored binding remains valid while its runtime cannot be read.
    state.replace_provider_store_for_test(providers).await;
    let current = state.shares.read().await.get("cap-share").unwrap().clone();
    let revision = current.user_grants[email].revision;
    let candidates = state
        .preview_share_permissions(
            "cap-share",
            json!({"expectedConfigRevision":current.config_revision,"subject":email}),
        )
        .await
        .unwrap();
    assert_eq!(candidates["unavailable"], true);
    assert!(candidates["rows"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| { row["operation"] == "tool.web_search" && row["available"] == true }));
    let reconfirm = json!({
        "expectedConfigRevision":current.config_revision,
        "grantCapabilityEdits":{(email):{"operations":[operation],"reconfirm":[operation],"expectedRevision":revision}}
    });
    assert_eq!(
        state
            .preview_share_permissions("cap-share", reconfirm)
            .await
            .unwrap_err()
            .to_string(),
        "CAPABILITY_STATE_CHANGED"
    );
    let revoke = json!({
        "expectedConfigRevision":current.config_revision,
        "grantCapabilityEdits":{(email):{"operations":[],"expectedRevision":revision}}
    });
    assert!(state
        .preview_share_permissions("cap-share", revoke)
        .await
        .is_ok());
}

#[tokio::test]
async fn grant_capability_migration_keeps_only_confirmed_named_bindings() {
    use crate::domain::sharing::capabilities::ShareOperation;
    let state = capability_test_state().await;
    let patch = capability_enable_search(&state, "legacy-capability").await;
    let saved = state
        .apply_share_settings_patch_immediate("cap-share", patch)
        .await
        .unwrap()
        .unwrap();
    let mut store = state.shares.read().await.clone();
    let providers = state.providers.read().await.clone();
    assert_eq!(
        share_capability::migrate_grant_capabilities(&mut store, &providers),
        1
    );
    let migrated = store.get("cap-share").unwrap();
    let permissions = migrated.grant_capabilities.as_ref().unwrap();
    assert!(!permissions.contains_key("$default"));
    let owner = permissions.get("owner@example.com").unwrap();
    assert_eq!(owner.allowed_operations, vec![ShareOperation::WebSearch]);
    assert_eq!(
        owner.operation_bindings[&ShareOperation::WebSearch].len(),
        1
    );
    assert_eq!(owner.coverage_acknowledgements.len(), 1);
    assert_eq!(migrated.tokens_used, saved.tokens_used);
    assert!(migrated.capability_policy.is_none());
}
