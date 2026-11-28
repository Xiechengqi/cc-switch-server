// Production runtime preparation -> independent catalog operation -> native
// send boundary. No synthetic caller lease or timing-dependent pause hook.
async fn trae_r7_prepared_catalog_transition(change: &str) {
    let rails: &[ProviderType] = if change == "intl" {
        &[ProviderType::TraeIntl]
    } else {
        &[ProviderType::TraeSolo, ProviderType::TraeRaw]
    };
    for &kind in rails {
        for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
            let label = format!("trae-r7-epoch-{change}-{kind:?}-{app:?}");
            let state = forwarder_test_state(&label);
            let (origin, observed, server) = trae_r6_origin().await;
            let id = r5_timing_install_trae(&state, kind, app, &origin).await;
            let route = match app {
                AppKind::Claude => ProxyRoute::ClaudeMessages,
                AppKind::Codex => ProxyRoute::CodexResponses,
                AppKind::Gemini => ProxyRoute::Gemini,
            };
            let attempt = claude_timing_attempt(&state, route, &id).await;
            let execution = attempt.execution.as_ref().unwrap();
            let (account_type, account_id, generation) =
                execution.managed_account_identity_target().unwrap();
            assert_eq!(account_type, kind);
            let runtime = state
                .prepare_trae_runtime(
                    app,
                    &id,
                    execution.plan.provider_revision,
                    &execution.plan.runtime_fingerprint,
                    account_id,
                    generation,
                    Duration::from_secs(2),
                )
                .await
                .unwrap();
            let canonical =
                json!({"model":TRAE_R6_MODEL,"messages":[{"role":"user","content":"hello"}]});
            let intl;
            let capability = if kind == ProviderType::TraeIntl {
                intl =
                    super::super::trae_runtime::raw_passthrough_capability(TRAE_R6_MODEL).unwrap();
                &intl
            } else {
                &runtime.catalog.capabilities[TRAE_R6_MODEL]
            };
            let (payload, tool_contract) = if kind == ProviderType::TraeSolo {
                let payload =
                    super::super::trae::build_trae_payload(&canonical, TRAE_R6_MODEL, capability)
                        .unwrap();
                let tools = super::super::trae::TraeToolContract::from_payload(&payload).unwrap();
                (payload, tools)
            } else {
                super::super::trae::build_raw_payload(&canonical, TRAE_R6_MODEL, capability, false)
                    .unwrap()
            };
            let original = super::super::trae_runtime::FetchedTraeModelCatalog {
                enabled_models: runtime.catalog.enabled_models.clone(),
                raw_configs: runtime.catalog.raw_configs.clone(),
                capabilities: runtime.catalog.capabilities.clone(),
            };
            let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
            match change {
                "invalidate" | "intl" => state.invalidate_trae_runtime_scope(&runtime.scope).await,
                "empty" => {
                    state
                        .trae_runtime
                        .insert_catalog(
                            runtime.scope.clone(),
                            super::super::trae_runtime::FetchedTraeModelCatalog {
                                enabled_models: Vec::new(),
                                raw_configs: BTreeMap::new(),
                                capabilities: BTreeMap::new(),
                            },
                            now,
                        )
                        .await
                        .unwrap();
                }
                "capability" | "aba" => {
                    let mut changed = original.clone();
                    changed
                        .capabilities
                        .get_mut(TRAE_R6_MODEL)
                        .unwrap()
                        .max_output_tokens = Some(17);
                    state
                        .trae_runtime
                        .insert_catalog(runtime.scope.clone(), changed, now)
                        .await
                        .unwrap();
                    if change == "aba" {
                        state
                            .trae_runtime
                            .insert_catalog(runtime.scope.clone(), original.clone(), now + 1)
                            .await
                            .unwrap();
                    }
                }
                "identical" => {
                    state
                        .trae_runtime
                        .insert_catalog(runtime.scope.clone(), original, now + 1)
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let timing = TimingSlot::default();
            let result = Box::pin(send_trae_generation(TraeGenerationOptions {
                state: &state,
                execution,
                runtime: &runtime,
                payload: &payload,
                model_id: TRAE_R6_MODEL,
                tool_contract,
                simulate_tools: false,
                request_memory: None,
                value_attempt: None,
                timing: &timing,
            }))
            .await;
            if matches!(change, "identical" | "intl") {
                assert!(result.is_ok(), "valid prepared rail was rejected: {label}");
                assert_eq!(observed.lock().unwrap().len(), 1);
            } else {
                assert_eq!(
                    observed.lock().unwrap().len(),
                    0,
                    "revoked prepared catalog reached generation: {label}"
                );
                match result {
                    Err(TraeForwardAttemptError::Proxy(error)) => {
                        assert_eq!(error.status, StatusCode::CONFLICT)
                    }
                    _ => panic!("old prepared catalog must reject with conflict: {label}"),
                }
            }
            server.abort();
        }
    }
}

#[tokio::test]
async fn r7_trae_prepared_empty_catalog_is_revoked_before_send() {
    trae_r7_prepared_catalog_transition("empty").await;
}

#[tokio::test]
async fn r7_trae_prepared_capability_change_is_revoked_before_send() {
    trae_r7_prepared_catalog_transition("capability").await;
}

#[tokio::test]
async fn r7_trae_prepared_explicit_invalidation_is_revoked_before_send() {
    trae_r7_prepared_catalog_transition("invalidate").await;
}

#[tokio::test]
async fn r7_trae_prepared_aba_does_not_revive_old_catalog() {
    trae_r7_prepared_catalog_transition("aba").await;
}

#[tokio::test]
async fn r7_trae_equal_catalog_rediscovery_keeps_prepared_admission() {
    trae_r7_prepared_catalog_transition("identical").await;
}

#[tokio::test]
async fn r7_trae_intl_uncatalogued_explicit_model_keeps_prepared_admission() {
    trae_r7_prepared_catalog_transition("intl").await;
}
