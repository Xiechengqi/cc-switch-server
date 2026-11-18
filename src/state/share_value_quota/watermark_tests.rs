// Included in state::tests, exercising real SQLite authority and HTTP sync.
mod value_quota_watermark_regression_tests {
    use super::*;
    use crate::repository::server_sqlite::{self, value_quota as ledger};

    #[derive(Clone, Default)]
    struct MockRouter {
        batches: Arc<TokioMutex<Vec<Value>>>,
        fail_first_user: Arc<std::sync::atomic::AtomicBool>,
    }

    async fn batch(
        AxumState(mock): AxumState<MockRouter>,
        Json(request): Json<Value>,
    ) -> Json<Value> {
        assert!(request["signature"].as_str().is_some_and(|s| !s.is_empty()));
        let items = request["items"].as_array().unwrap();
        mock.batches.lock().await.push(request.clone());
        Json(json!({"items":items.iter().map(|item| {
            let email = &item["status"]["email"];
            if email == "user000@example.test" && mock.fail_first_user.load(AtomicOrdering::SeqCst) {
                json!({"shareId":item["shareId"],"email":email,"ok":false,"error":"descriptor_pending"})
            } else {
                json!({"shareId":item["shareId"],"email":email,"ok":true,"usageWatermark":item["status"]["usageWatermark"]})
            }
        }).collect::<Vec<_>>()}))
    }

    #[tokio::test]
    async fn value_quota_watermark_batch_syncs_130_grants_and_retries_only_failed_items() {
        let mock = MockRouter::default();
        mock.fail_first_user.store(true, AtomicOrdering::SeqCst);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/v1/shares/value-quota-watermarks", post(batch))
            .with_state(mock.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        server_sqlite::activate_authority(&state.config_dir).unwrap();
        configure_registered_test_router(&state, &format!("http://{address}"), "watermark-batch")
            .await;
        let config = state.config_snapshot().await;
        state
            .record_router_quota_capability(
                &config,
                client::RouterShareCapabilities {
                    share_contract_version: 13,
                    capabilities: vec![
                        "usdEquivalentQuotaV1".into(),
                        "valueQuotaWatermarkBatchV1".into(),
                        "valueQuotaUnpricedV1".into(),
                    ],
                },
            )
            .await;
        state
            .upsert_provider_command(
                AppKind::Codex,
                test_provider("batch-provider", "Provider"),
                Some(ProfileId::parse("codex.openai_api_key").unwrap()),
                None,
                None,
                test_api_key_credential(),
            )
            .await
            .unwrap()
            .unwrap();
        let scope = ledger::Scope {
            router_url: config.router_api_base().unwrap().into(),
            installation_id: "watermark-batch".into(),
            share_id: "batch-share".into(),
            email: "owner@example.test".into(),
            entitlement_id: None,
        };
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/share-value-pricing-v1.json"
        ))
        .unwrap();
        let snapshot: crate::domain::usage::value_pricing::PricingSnapshot =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        let revision = snapshot.revision().unwrap();
        ledger::install_snapshot(
            &state.config_dir,
            &scope,
            &revision,
            &snapshot,
            now_ms_i64(),
        )
        .unwrap();
        ledger::activate_snapshot(&state.config_dir, &scope, &revision).unwrap();
        state.mutate_shares_immediate(|store| {
            let mut input=router_sync_share_input("batch-share","batch-provider");
            for n in 0..130 {
                let email=format!("user{n:03}@example.test");
                let grant=serde_json::from_value(json!({"email":email,"active":true,"policy":{"tokenPeriod":"lifetime","valueQuota":{"limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1"}}})).unwrap();
                input.user_grants.insert(email,grant);
            }
            store.upsert(input).unwrap();
        }).await.unwrap();
        state
            .sync_share_value_watermarks(&config, false)
            .await
            .unwrap();
        let batches = mock.batches.lock().await.clone();
        assert_eq!(batches.len(), 3);
        assert_eq!(
            batches
                .iter()
                .map(|r| r["items"].as_array().unwrap().len())
                .sum::<usize>(),
            130
        );
        for batch in batches {
            assert!(serde_json::to_vec(&batch).unwrap().len() < 256 * 1024);
        }
        mock.fail_first_user.store(false, AtomicOrdering::SeqCst);
        state
            .sync_share_value_watermarks(&config, false)
            .await
            .unwrap();
        let batches = mock.batches.lock().await.clone();
        assert_eq!(batches.len(), 4);
        assert_eq!(batches[3]["items"].as_array().unwrap().len(), 1);
        assert_eq!(
            batches[3]["items"][0]["status"]["email"],
            "user000@example.test"
        );
        state
            .sync_share_value_watermarks(&config, false)
            .await
            .unwrap();
        assert_eq!(
            mock.batches.lock().await.len(),
            4,
            "fresh ACKs suppress unchanged sends"
        );
        server.abort();
    }

    #[tokio::test]
    async fn value_quota_watermark_capabilities_do_not_enable_share_permissions_or_weaken_unpriced_policy(
    ) {
        let state = test_state();
        configure_registered_test_router(&state, "http://127.0.0.1:9", "capability-flags").await;
        let config = state.config_snapshot().await;
        state
            .record_router_quota_capability(
                &config,
                client::RouterShareCapabilities {
                    share_contract_version: 13,
                    capabilities: vec!["shareCapabilityPolicyV1".into()],
                },
            )
            .await;
        assert!(state.capability_policy_router_negotiated().await);
        assert!(!state.value_watermark_batch_router_negotiated().await);
        state
            .record_router_quota_capability(
                &config,
                client::RouterShareCapabilities {
                    share_contract_version: 13,
                    capabilities: vec![
                        "valueQuotaWatermarkBatchV1".into(),
                        "valueQuotaUnpricedPolicyV1".into(),
                    ],
                },
            )
            .await;
        assert!(!state.capability_policy_router_negotiated().await);
        assert!(state.value_watermark_batch_router_negotiated().await);
        assert!(state.value_unpriced_policy_router_negotiated().await);
        let mut descriptor = ShareDescriptor::default();
        let grant=serde_json::from_value(json!({"email":"user@example.test","active":true,
            "policy":{"valueQuota":{"limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1","unpricedUsage":"reject"}}})).unwrap();
        descriptor
            .user_grants
            .insert("user@example.test".into(), grant);
        let before = serde_json::to_value(&descriptor).unwrap();
        assert!(
            apply_value_descriptor_projection(&mut descriptor, true, true, false)
                .unwrap_err()
                .to_string()
                .contains("UNPRICED_POLICY_UNSUPPORTED")
        );
        assert_eq!(serde_json::to_value(&descriptor).unwrap(), before);
        apply_value_descriptor_projection(&mut descriptor, true, true, true).unwrap();
        assert_eq!(
            serde_json::to_value(&descriptor).unwrap()["userGrants"]["user@example.test"]["policy"]
                ["valueQuota"]["unpricedUsage"],
            "reject"
        );
    }
    #[test]
    fn value_quota_watermark_batches_bound_escaped_model_diagnostics_by_bytes() {
        use crate::domain::usage::value_pricing::ValueQuotaStatus;
        let status: ValueQuotaStatus = serde_json::from_value(json!({
            "email":"user@example.test","policyEpoch":"epoch","windowId":"Lifetime:0:0",
            "windowStartsAtMs":0,"resetsAtMs":null,"meteredFromMs":0,
            "activePricingRevision":"a".repeat(64),"usagePricingRevisionsDigest":"b".repeat(64),
            "usageWatermark":10,"limitUsdMicros":"1000000","usedPicos":"100",
            "observedPicos":"100","manualOffsetPicos":"0","pendingRiskPicos":"0",
            "meteringStatus":"observed","syncedAtMs":0,"unpricedModels":vec!["\\".repeat(256);20]
        }))
        .unwrap();
        let scope = ledger::Scope {
            router_url: "https://router.test".into(),
            installation_id: "batch".into(),
            share_id: "share".into(),
            email: status.email.clone(),
            entitlement_id: None,
        };
        let pending: Vec<_> = (0..64)
            .map(|n| {
                (
                    format!("share{n}"),
                    status.email.clone(),
                    scope.clone(),
                    status.clone(),
                )
            })
            .collect();
        let chunks = crate::state::share_value_quota::watermark_chunks(&pending).unwrap();
        assert!(
            chunks.len() > 1,
            "item count alone would exceed the ingress limit"
        );
        assert_eq!(chunks.iter().map(|chunk| chunk.len()).sum::<usize>(), 64);
        for chunk in chunks {
            let items: Vec<_> = chunk
                .iter()
                .map(|(id, _, _, status)| json!({"shareId":id,"status":status}))
                .collect();
            assert!(serde_json::to_vec(&json!({"items":items})).unwrap().len() <= 192 * 1024);
        }
    }
}
