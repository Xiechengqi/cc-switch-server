async fn seed_copilot_inference_catalog(
    state: &ServerState,
) -> crate::clients::oauth::copilot_models::CopilotModelCatalogKey {
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id":"copilot-inference-scope", "providerType":"github_copilot",
                    "accessToken":"synthetic-short-lived", "refreshToken":"synthetic-github-oauth",
                    "expiresAt":i64::MAX / 2,
                    "profile":{"githubDomain":"github.com","ghes":false},
                    "raw":{"githubDomain":"github.com", "githubToken":"synthetic-github-oauth",
                        "copilotToken":{"token":"synthetic-short-lived"},
                        "copilotApiBase":"https://api.githubcopilot.com"}
                }))
                .unwrap(),
            );
        })
        .await
        .unwrap();
    let auth = state
        .prepare_copilot_upstream_auth("copilot-inference-scope", 1)
        .await
        .unwrap();
    let key = crate::clients::oauth::copilot_models::CopilotModelCatalogKey {
        account_id: "copilot-inference-scope".into(),
        auth_identity_generation: 1,
        account_token_refresh_generation: auth.account_token_refresh_generation,
        copilot_token_generation: auth.token_generation,
        github_domain: "github.com".into(),
        api_origin: auth.api_endpoint,
    };
    let descriptors = crate::clients::oauth::copilot_models::parse_copilot_models(&json!({"data":[
        {"id":"opaque-model","supported_endpoints":["/responses"]},
        {"id":"denied-model","policy":{"state":"disabled"},"supported_endpoints":["/chat/completions"]}
    ]}));
    state
        .copilot_model_catalogs
        .insert(
            key.clone(),
            descriptors,
            crate::infra::time::now_ms() as i64,
        )
        .await
        .unwrap();
    key
}

#[tokio::test]
async fn copilot_inference_plan_revalidates_capabilities_and_all_token_scope_components() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    let state = test_state();
    let config_dir = state.config_dir.clone();
    let key = seed_copilot_inference_catalog(&state).await;
    let mut plan = state
        .prepare_copilot_inference_plan(
            "copilot-inference-scope",
            1,
            "opaque-model",
            CopilotEndpoint::Chat,
            Duration::from_secs(1),
            true,
            None,
        )
        .await
        .unwrap();
    assert_eq!(plan.endpoint, CopilotEndpoint::Responses);
    assert!(state
        .ensure_copilot_inference_plan_current(&plan)
        .await
        .is_ok());
    for model in ["denied-model", "unknown-model"] {
        assert!(matches!(
            state
                .prepare_copilot_inference_plan(
                    "copilot-inference-scope",
                    1,
                    model,
                    CopilotEndpoint::Chat,
                    Duration::from_secs(1),
                    true,
                    None
                )
                .await,
            Err(CopilotModelCatalogError::Discovery {
                status_code: 400,
                ..
            })
        ));
    }
    let old_expiry = plan.catalog_valid_until_ms;
    plan.catalog_valid_until_ms = 0;
    assert!(state
        .ensure_copilot_inference_plan_current(&plan)
        .await
        .is_err());
    plan.catalog_valid_until_ms = old_expiry;
    // A same-key catalog replacement may revoke this model without rotating auth.
    state
        .copilot_model_catalogs
        .insert(key.clone(), Vec::new(), crate::infra::time::now_ms() as i64)
        .await
        .unwrap();
    assert!(state
        .ensure_copilot_inference_plan_current(&plan)
        .await
        .is_err());
    state
        .copilot_model_catalogs
        .insert(
            key,
            vec![plan.descriptor().clone()],
            crate::infra::time::now_ms() as i64,
        )
        .await
        .unwrap();
    for change in 0..3 {
        let old = state.copilot_upstream_auth.read().await["copilot-inference-scope"].clone();
        {
            let mut auth = state.copilot_upstream_auth.write().await;
            let auth = auth.get_mut("copilot-inference-scope").unwrap();
            match change {
                0 => auth.token_generation += 1,
                1 => auth.api_endpoint = "https://api.individual.githubcopilot.com".into(),
                _ => auth.rejected = true,
            }
        }
        assert!(state
            .ensure_copilot_inference_plan_current(&plan)
            .await
            .is_err());
        state
            .copilot_upstream_auth
            .write()
            .await
            .insert("copilot-inference-scope".into(), old);
    }
    // Discovery may expose stale metadata after a transient failure; inference cannot.
    state
        .copilot_model_catalogs
        .insert(
            plan.key.clone(),
            vec![plan.descriptor().clone()],
            crate::infra::time::now_ms() as i64
                - crate::clients::oauth::copilot_models::MODEL_CACHE_TTL_MS
                - 1,
        )
        .await
        .unwrap();
    assert!(matches!(
        state
            .prepare_copilot_inference_plan(
                "copilot-inference-scope",
                1,
                "opaque-model",
                CopilotEndpoint::Responses,
                Duration::from_secs(1),
                true,
                Some("http://127.0.0.1:9/models")
            )
            .await,
        Err(CopilotModelCatalogError::Discovery {
            status_code: 409,
            ..
        })
    ));
    state
        .mutate_accounts_immediate(|accounts| {
            accounts
                .accounts
                .iter_mut()
                .find(|account| account.id == "copilot-inference-scope")
                .unwrap()
                .token_refresh_generation += 1;
        })
        .await
        .unwrap();
    assert!(state
        .ensure_copilot_inference_plan_current(&plan)
        .await
        .is_err());
    drop(state);
    fs::remove_dir_all(config_dir).unwrap();
}

#[tokio::test]
async fn r5_kimi_and_copilot_share_one_parent_and_capacity_is_not_auth_failure() {
    use crate::clients::oauth::copilot_models::{
        parse_copilot_models, CopilotModelCatalogCache, CopilotModelCatalogKey,
        CopilotModelDescriptor,
    };
    use crate::infra::resident_memory::ResidentMemoryBudget;
    fn key(suffix: &str) -> CopilotModelCatalogKey {
        CopilotModelCatalogKey {
            account_id: suffix.into(),
            auth_identity_generation: 1,
            account_token_refresh_generation: 1,
            copilot_token_generation: 1,
            github_domain: "github.com".into(),
            api_origin: "https://api.githubcopilot.com".into(),
        }
    }
    fn descriptors() -> Vec<CopilotModelDescriptor> {
        parse_copilot_models(
            &serde_json::json!({"data":[{"id":"synthetic","supported_endpoints":["/v1/messages"]}]}),
        )
    }

    use crate::proxy::kimi_runtime::{KimiModelCatalogCache, KimiModelCatalogScope};
    let parent = ResidentMemoryBudget::new(None);
    let kimi = KimiModelCatalogCache::with_parent(parent.clone());
    let scope = KimiModelCatalogScope::derive("claude", "p", 1, "r", "a", 1, 1);
    let held = kimi.insert(scope, vec!["k3".into()], 1).await;
    let bytes = parent.snapshot().used_bytes;
    parent.configure_limit(Some(bytes));
    let copilot = CopilotModelCatalogCache::with_parent(parent.clone());
    assert!(copilot
        .insert(key("first"), descriptors(), 1)
        .await
        .is_none());
    assert!(copilot.lock(&key("first")).await.is_none());
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(kimi);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
    parent.configure_limit(Some(0));
    assert!(copilot
        .insert(key("first"), descriptors(), 2)
        .await
        .is_none());
    parent.configure_limit(None);
    assert!(copilot
        .insert(key("first"), descriptors(), 3)
        .await
        .is_some());
    drop(copilot);
    assert_eq!(parent.snapshot().used_bytes, 0);
}
