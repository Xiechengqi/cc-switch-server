mod value_usage_initialization_tests {
    use super::*;
    use crate::domain::sharing::router_contract::{ShareSettingsEditContext, ShareSettingsPatch};
    use crate::domain::usage::value_pricing::PricingSnapshot;
    use crate::repository::server_sqlite::{self, value_quota as ledger};

    async fn fixture() -> ServerState {
        let state = test_state();
        let mut config = state.config_snapshot().await;
        config.owner.email = Some("owner@example.com".into());
        state.replace_config(config).await.unwrap();
        configure_registered_test_router(&state, "http://127.0.0.1:9", "initial-value-install")
            .await;
        server_sqlite::activate_authority(&state.config_dir).unwrap();
        state
            .upsert_provider_command(
                AppKind::Codex,
                test_provider("provider", "Provider"),
                Some(ProfileId::parse("codex.openai_api_key").unwrap()),
                None,
                None,
                test_api_key_credential(),
            )
            .await
            .unwrap()
            .unwrap();
        let config = state.config_snapshot().await;
        state
            .record_router_quota_capability(
                &config,
                client::RouterShareCapabilities {
                    share_contract_version: 12,
                    capabilities: vec![
                        "usdEquivalentQuotaV1".into(),
                        "quotaUnitConversionV1".into(),
                    ],
                },
            )
            .await;
        state
            .mutate_shares_immediate(|store| {
                let mut input = router_sync_share_input("initial-value-share", "provider");
                input.enabled = Some(true);
                input.status = Some("active".into());
                store.upsert(input).unwrap();
            })
            .await
            .unwrap();
        let scope = ledger::Scope {
            router_url: config.router_api_base().unwrap().into(),
            installation_id: "initial-value-install".into(),
            share_id: "initial-value-share".into(),
            email: "owner@example.com".into(),
            entitlement_id: None,
        };
        let pricing: PricingSnapshot = serde_json::from_value(json!({"schemaVersion":1,"tariffPolicyId":"api-equivalent-v1","currency":"USD","exactAliases":{},
            "models":{"model":{"contextInputBasis":"rawInput","longContextThreshold":null,"longContextInclusive":false,
            "rates":{"standard/base":{"input":"1400000","output":"1000000","cacheRead":null,"cacheWrite5m":null,"cacheWrite1h":null,"cacheWritePolicy":"unavailable"}}}}})).unwrap();
        let revision = pricing.revision().unwrap();
        ledger::install_snapshot(&state.config_dir, &scope, &revision, &pricing, now_ms_i64())
            .unwrap();
        ledger::activate_snapshot(&state.config_dir, &scope, &revision).unwrap();
        state
    }

    async fn patch(state: &ServerState, names: &[&str]) -> ShareSettingsPatch {
        let share = state
            .shares
            .read()
            .await
            .get("initial-value-share")
            .unwrap()
            .clone();
        let policy = json!({"tokenPeriod":"day","valueQuota":{"limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1"}});
        let policies = names
            .iter()
            .map(|email| {
                (
                    (*email).into(),
                    serde_json::from_value(policy.clone()).unwrap(),
                )
            })
            .collect();
        let preview = state
            .preview_share_value_quota(&share.id, share.config_revision, policies)
            .await
            .unwrap();
        assert_eq!(preview["ready"], true, "{preview}");
        assert!(preview["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["code"] == "value_usage_initialization" && c["ready"] == true));
        let mut edits = serde_json::Map::new();
        let mut initializations = serde_json::Map::new();
        let mut conversions = serde_json::Map::new();
        for row in preview["rows"].as_array().unwrap() {
            let email = row["email"].as_str().unwrap();
            assert_eq!(row["canInitializeUsage"], true);
            edits.insert(email.into(), json!({"expectedRevision":0,"policy":policy}));
            conversions.insert(
                email.into(),
                json!({"credential":row["conversionCredential"]}),
            );
            initializations.insert(email.into(), json!({"targetPicos":"123456789123","windowId":row["windowId"],"reason":"Reviewed opening usage"}));
        }
        serde_json::from_value(json!({"editContext":{"operationId":"initial-operation","expectedConfigRevision":share.config_revision,"actor":"owner@example.com"},
            "userGrantEdits":edits,"userQuotaConversions":conversions,"userValueUsageInitializations":initializations})).unwrap()
    }

    fn adjustment_count(state: &ServerState) -> i64 {
        rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir))
            .unwrap()
            .query_row("SELECT count(*) FROM value_quota_adjustments", [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[tokio::test]
    async fn opening_usd_usage_commits_grants_audit_and_receipt_and_replays_once() {
        let state = fixture().await;
        let patch = patch(&state, &["new@example.test"]).await;
        let mut patch = patch;
        patch.edit_context.as_mut().unwrap().actor = Some("forged@example.test".into());
        let saved = state
            .apply_share_settings_patch_immediate("initial-value-share", patch.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(saved.user_grants["new@example.test"]
            .pending_value_quota
            .is_none());
        let statuses = state.value_quota_edit_statuses(&saved.id).await.unwrap();
        let row = statuses
            .iter()
            .find(|row| row["email"] == "new@example.test")
            .unwrap();
        assert_eq!(row["usedPicos"], "123456789123");
        assert_eq!(row["observedPicos"], "0");
        assert_eq!(row["manualOffsetPicos"], "123456789123");
        assert_eq!(adjustment_count(&state), 1);
        let actor: String =
            rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir))
                .unwrap()
                .query_row("SELECT actor FROM value_quota_adjustments", [], |row| {
                    row.get(0)
                })
                .unwrap();
        assert_eq!(
            actor, "owner@example.com",
            "the local session cannot forge an audit actor"
        );
        let again = state
            .apply_share_settings_patch_immediate(&saved.id, patch)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(again.config_revision, saved.config_revision);
        assert_eq!(adjustment_count(&state), 1);
        let durable = server_sqlite::load_authoritative(&state.config_dir)
            .unwrap()
            .shares;
        assert!(durable
            .get(&saved.id)
            .unwrap()
            .user_grants
            .contains_key("new@example.test"));
        assert!(durable
            .applied_settings_operations
            .contains_key("initial-operation"));
    }

    #[tokio::test]
    async fn invalid_opening_usd_batch_rolls_back_all_users_offsets_and_receipts() {
        let state = fixture().await;
        let mut patch = patch(&state, &["a@example.test", "z@example.test"]).await;
        patch
            .user_value_usage_initializations
            .as_mut()
            .unwrap()
            .get_mut("z@example.test")
            .unwrap()
            .target_picos = "-1".into();
        assert!(state
            .apply_share_settings_patch_immediate("initial-value-share", patch)
            .await
            .is_err());
        assert_eq!(adjustment_count(&state), 0);
        let durable = server_sqlite::load_authoritative(&state.config_dir)
            .unwrap()
            .shares;
        for email in ["a@example.test", "z@example.test"] {
            assert!(!durable
                .get("initial-value-share")
                .unwrap()
                .user_grants
                .contains_key(email));
            assert!(!state
                .shares
                .read()
                .await
                .get("initial-value-share")
                .unwrap()
                .user_grants
                .contains_key(email));
        }
        assert!(!durable
            .applied_settings_operations
            .contains_key("initial-operation"));
    }

    #[tokio::test]
    async fn share_write_failure_cannot_leave_an_opening_usd_offset_or_receipt() {
        let state = fixture().await;
        let patch = patch(&state, &["new@example.test"]).await;
        let db =
            rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir)).unwrap();
        db.execute_batch("CREATE TRIGGER fail_share_initialization BEFORE INSERT ON shares BEGIN SELECT RAISE(ABORT,'fixture Share write failure'); END;").unwrap();
        assert!(state
            .apply_share_settings_patch_immediate("initial-value-share", patch)
            .await
            .is_err());
        assert_eq!(adjustment_count(&state), 0);
        assert!(!state
            .shares
            .read()
            .await
            .applied_settings_operations
            .contains_key("initial-operation"));
        assert!(!server_sqlite::load_authoritative(&state.config_dir)
            .unwrap()
            .shares
            .get("initial-value-share")
            .unwrap()
            .user_grants
            .contains_key("new@example.test"));
    }

    #[tokio::test]
    async fn opening_usd_usage_rejects_stale_windows_existing_users_and_changed_retries() {
        let state = fixture().await;
        let good = patch(&state, &["new@example.test"]).await;
        let mut stale = good.clone();
        stale
            .user_value_usage_initializations
            .as_mut()
            .unwrap()
            .get_mut("new@example.test")
            .unwrap()
            .window_id = "Day:0:86400000".into();
        let error = state
            .apply_share_settings_patch_immediate("initial-value-share", stale)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("REVISION_CONFLICT"));
        assert_eq!(adjustment_count(&state), 0);
        let saved = state
            .apply_share_settings_patch_immediate("initial-value-share", good.clone())
            .await
            .unwrap()
            .unwrap();
        let mut changed = good.clone();
        changed
            .user_value_usage_initializations
            .as_mut()
            .unwrap()
            .get_mut("new@example.test")
            .unwrap()
            .target_picos = "1".into();
        assert!(state
            .apply_share_settings_patch_immediate(&saved.id, changed)
            .await
            .is_err());
        let mut existing = good;
        existing.edit_context = Some(ShareSettingsEditContext {
            operation_id: "another-operation".into(),
            expected_config_revision: saved.config_revision,
            actor: Some("owner@example.com".into()),
        });
        existing
            .user_grant_edits
            .as_mut()
            .unwrap()
            .get_mut("new@example.test")
            .unwrap()
            .expected_revision = saved.user_grants["new@example.test"].revision;
        assert!(state
            .apply_share_settings_patch_immediate(&saved.id, existing)
            .await
            .is_err());
        assert_eq!(adjustment_count(&state), 1);
    }

    #[tokio::test]
    async fn opening_usd_usage_rejects_historical_tokens_even_after_details_are_trimmed() {
        let state = fixture().await;
        let patch = patch(&state, &["old@example.test"]).await;
        let mut log = crate::domain::usage::store::UsageLog::new(
            AppKind::Codex,
            "provider".into(),
            "Provider".into(),
            ProviderType::Codex,
            200,
            1,
            crate::domain::usage::store::UsageModelMetadata::default(),
            crate::domain::usage::store::TokenUsage {
                total_tokens: Some(7),
                ..Default::default()
            },
        );
        log.share_id = Some("initial-value-share".into());
        log.user_email = Some("Old@Example.Test".into());
        log.created_at_ms = 1;
        state.push_usage_log(log).await.unwrap();
        assert!(
            state.usage_snapshot().await.logs.is_empty(),
            "the old detail is outside retention"
        );
        // Model an already-compacted SQLite detail set too. Persistent rollups
        // still prove this is not a genuinely new user's opening balance.
        rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir))
            .unwrap()
            .execute("DELETE FROM usage_records", [])
            .unwrap();
        let error = state
            .apply_share_settings_patch_immediate("initial-value-share", patch.clone())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("genuinely new")
                || error.to_string().contains("HISTORY_INCOMPLETE")
                || error.to_string().contains("PREVIEW_STALE"),
            "{error}"
        );
        assert_eq!(adjustment_count(&state), 0);
        assert!(!state
            .shares
            .read()
            .await
            .get("initial-value-share")
            .unwrap()
            .user_grants
            .contains_key("old@example.test"));
    }

    #[tokio::test]
    async fn opening_usd_usage_cannot_clear_a_restore_fence_or_initialize_revoked_users() {
        let state = fixture().await;
        let initial_patch = patch(&state, &["new@example.test"]).await;
        let mut revoked_patch = patch(&state, &["revoked@example.test"]).await;
        let config = state.config_snapshot().await;
        let scope = ledger::Scope {
            router_url: config.router_api_base().unwrap().into(),
            installation_id: "initial-value-install".into(),
            share_id: "initial-value-share".into(),
            email: "new@example.test".into(),
            entitlement_id: None,
        };
        ledger::begin_coverage(&state.config_dir, &scope, now_ms_i64()).unwrap();
        ledger::write_restore_fence(&state.config_dir).unwrap();
        assert!(!ledger::can_initialize_usage(&state.config_dir, &scope).unwrap());
        assert!(state
            .apply_share_settings_patch_immediate("initial-value-share", initial_patch)
            .await
            .is_err());
        assert!(!ledger::can_initialize_usage(&state.config_dir, &scope).unwrap());
        assert_eq!(adjustment_count(&state), 0);

        let patch = &mut revoked_patch;
        state.mutate_shares_immediate(|store| {
            store.shares.iter_mut().find(|share| share.id == "initial-value-share").unwrap().user_grants.insert("revoked@example.test".into(),
                serde_json::from_value(json!({"email":"revoked@example.test","active":false,"policy":{"tokenPeriod":"lifetime"}})).unwrap());
        }).await.unwrap();
        patch
            .edit_context
            .as_mut()
            .unwrap()
            .expected_config_revision = state
            .shares
            .read()
            .await
            .get("initial-value-share")
            .unwrap()
            .config_revision;
        let error = state
            .apply_share_settings_patch_immediate("initial-value-share", patch.clone())
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("genuinely new")
                || error.to_string().contains("HISTORY_INCOMPLETE")
                || error.to_string().contains("RESTORE_FENCED")
                || error.to_string().contains("PREVIEW_STALE"),
            "{error}"
        );
        assert_eq!(adjustment_count(&state), 0);
    }
    async fn consume_tokens(state: &ServerState, request: &str, count: u64) {
        use crate::domain::usage::store::{TokenUsage, UsageLog, UsageModelMetadata};
        let context = UsageLogContext {
            share_id: Some("initial-value-share".into()),
            user_email: Some("owner@example.com".into()),
            request_id: Some(request.into()),
            started_at_ms: Some(now_ms_i64() as u128),
            attempt_count: Some(1),
            quota_app: Some(AppKind::Codex),
            ..Default::default()
        };
        let guard = state
            .admit_share_value_text(
                &context,
                "model",
                Some("standard"),
                crate::domain::usage::value_pricing::RiskEstimate {
                    input_tokens: count,
                    output_tokens: 1,
                },
                true,
            )
            .await
            .unwrap()
            .unwrap();
        guard.mark_sent();
        let tokens = TokenUsage {
            input_tokens: Some(count),
            raw_input_tokens: Some(count),
            output_tokens: Some(0),
            cache_read_tokens: Some(0),
            cache_creation_tokens: Some(0),
            total_tokens: Some(count),
            ..Default::default()
        };
        guard.observe_tokens(tokens, false).await.unwrap();
        let mut log = UsageLog::new(
            AppKind::Codex,
            "provider".into(),
            "Provider".into(),
            ProviderType::Codex,
            200,
            1,
            UsageModelMetadata {
                model: Some("model".into()),
                ..Default::default()
            },
            tokens,
        );
        log.apply_context(context);
        log.usage_revision = 1;
        log.effective_service_tier = Some("standard".into());
        state.push_usage_log(log).await.unwrap();
        guard.settle_now().await;
    }

    #[tokio::test]
    async fn signed_conversion_commits_live_usage_and_calibrates_against_preview_then_replays_durably(
    ) {
        let state = fixture().await;
        consume_tokens(&state, "before-preview", 30_000_000).await;
        let share = state
            .shares
            .read()
            .await
            .get("initial-value-share")
            .unwrap()
            .clone();
        let policy:crate::domain::sharing::router_contract::ShareUserPolicy=serde_json::from_value(json!({"tokenPeriod":"lifetime","valueQuota":{"limitUsdMicros":"100000000","tariffPolicyId":"api-equivalent-v1"}})).unwrap();
        let preview = state
            .preview_share_value_quota(
                &share.id,
                share.config_revision,
                BTreeMap::from([("owner@example.com".into(), policy.clone())]),
            )
            .await
            .unwrap();
        assert_eq!(
            preview["rows"][0]["usedPicos"], "42000000000000",
            "{preview}"
        );
        assert_eq!(preview["rows"][0]["remainingPicos"], "58000000000000");
        let row = &preview["rows"][0];
        let edit:ShareSettingsPatch=serde_json::from_value(json!({
            "editContext":{"operationId":"live-conversion","expectedConfigRevision":share.config_revision,"actor":"forged@example.test"},
            "userGrantEdits":{"owner@example.com":{"expectedRevision":share.user_grants["owner@example.com"].revision,"policy":policy}},
            "userQuotaConversions":{"owner@example.com":{"credential":row["conversionCredential"],"targetUsedPicos":"50000000000000","reason":"Audited historical adjustment"}}
        })).unwrap();
        consume_tokens(&state, "after-preview", 10_000_000).await;
        let saved = state
            .apply_share_settings_patch_immediate(&share.id, edit.clone())
            .await
            .unwrap()
            .unwrap();
        let rows = state.value_quota_edit_statuses(&share.id).await.unwrap();
        let status = rows
            .iter()
            .find(|r| r["email"] == "owner@example.com")
            .unwrap();
        assert_eq!(status["observedPicos"], "56000000000000");
        assert_eq!(
            status["usedPicos"], "64000000000000",
            "new consumption is added to the preview-based offset"
        );
        assert_eq!(status["manualOffsetPicos"], "8000000000000");
        assert!(saved.user_grants["owner@example.com"]
            .pending_value_quota
            .is_none());
        // Even a pruned in-memory operation cache cannot reapply a conversion.
        state
            .mutate_shares_immediate(|store| {
                store.applied_settings_operations.clear();
            })
            .await
            .unwrap();
        let replay = state
            .apply_share_settings_patch_immediate(&share.id, edit.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay.config_revision, saved.config_revision);
        let result = state
            .share_quota_conversion_result(&share.id, "live-conversion")
            .await
            .unwrap();
        assert_eq!(result["committed"], true);
        assert_eq!(result["quotaConversions"].as_array().unwrap().len(), 1);
        let mut tampered = edit.clone();
        tampered.edit_context.as_mut().unwrap().operation_id = "tampered".into();
        tampered
            .user_quota_conversions
            .as_mut()
            .unwrap()
            .get_mut("owner@example.com")
            .unwrap()
            .credential
            .push('0');
        assert!(state
            .apply_share_settings_patch_immediate(&share.id, tampered)
            .await
            .is_err());
        let mut changed = edit;
        changed
            .user_quota_conversions
            .as_mut()
            .unwrap()
            .get_mut("owner@example.com")
            .unwrap()
            .target_used_picos = Some("1".into());
        assert!(state
            .apply_share_settings_patch_immediate(&share.id, changed)
            .await
            .is_err());
        // The same operation cannot be discovered after rebinding the Router.
        let mut config = state.config_snapshot().await;
        config.router.identity.as_mut().unwrap().installation_id = "other-installation".into();
        state.replace_config(config).await.unwrap();
        assert_eq!(
            state
                .share_quota_conversion_result(&share.id, "live-conversion")
                .await
                .unwrap()["committed"],
            false
        );
    }
}
