// Included beside the signed-ingress catalog fixtures; no vendor network calls.
#[tokio::test]
async fn opencode_zen_catalog_invoke_requires_auth_and_never_creates_a_provider() {
    use crate::proxy::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    zen::clear_catalog();
    zen::store_catalog_payload(&json!({"data":[
        {"id":"big-pickle","name":"Big Pickle","cost":{"input":0,"output":0}},
        {"id":"paid-free","cost":{"input":1,"output":2}},
        {"id":"decision-free","type":"decision","cost":{"input":0,"output":0}}
    ]}));
    let state = catalog_test_state("zen-unsaved-preview");
    let before_files: Vec<_> = ["providers.json", "accounts.json"]
        .into_iter()
        .map(|name| std::fs::read(state.config_dir.join(name)).ok())
        .collect();
    let router = Router::new()
        .route(
            "/web-api/invoke/:command",
            axum::routing::post(web_invoke_compat),
        )
        .with_state(state.clone());
    for (args, auth, expected) in [
        (json!({}), false, StatusCode::UNAUTHORIZED),
        (json!({}), true, StatusCode::OK),
        (
            json!({"baseUrl":"https://arbitrary.invalid"}),
            true,
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"accountId":"unneeded"}),
            true,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let mut request = Request::builder()
            .method("POST")
            .uri("/web-api/invoke/fetch_opencode_zen_models")
            .header("content-type", "application/json");
        if auth {
            request = request.header("x-cc-switch-web-user-email", "owner@example.com");
        }
        let response = router
            .clone()
            .oneshot(request.body(Body::from(args.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body: Value = serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 65536)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(status, expected, "{body}");
        if status == StatusCode::OK {
            assert_eq!(body["source"], "opencode_zen_live_model_catalog");
            assert_eq!(body["stale"], false);
            let models = body["models"].as_array().unwrap();
            assert_eq!(models.len(), 3);
            let free = models.iter().find(|m| m["id"] == "big-pickle").unwrap();
            assert_eq!(free["pricing"], "free");
            assert_eq!(free["selectable"], true);
            assert!(models
                .iter()
                .filter(|m| m["id"] != "big-pickle")
                .all(|m| m["selectable"] == false));
        }
    }
    assert!(state.providers_snapshot().await.providers.is_empty());
    assert!(state.accounts_snapshot().await.accounts.is_empty());
    let after_files: Vec<_> = ["providers.json", "accounts.json"]
        .into_iter()
        .map(|name| std::fs::read(state.config_dir.join(name)).ok())
        .collect();
    assert_eq!(before_files, after_files);
    zen::clear_catalog();
}

#[tokio::test]
async fn opencode_zen_public_and_management_catalogs_project_one_stale_capability_snapshot() {
    use crate::proxy::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    zen::clear_catalog();
    zen::store_stale_catalog_payload(
        &json!({"data":[{"id":"test-free","name":"Test Free","type":"chat","cost":{"input":0,"output":0},"limit":{"context":123456,"output":8192},"capabilities":{"tools":{"supported":false},"reasoning":{"supported":true,"effort":["low","high"]},"structured_output":true}}]}),
    );
    let state = catalog_test_state("zen-stale-projection");
    let stored = state
        .upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app: AppKind::Codex,
            provider: crate::domain::providers::model::Provider {
                id: "zen-stale-provider".into(),
                name: "Zen".into(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            profile_id: Some(
                crate::domain::providers::registry::ProfileId::parse("codex.opencode_zen").unwrap(),
            ),
            custom_binding: None,
            expected_revision: None,
            client_request_id: None,
            credential_patches: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    let management =
        zen::refresh_model_catalog(&state.http_client().await, &state.config_dir, false)
            .await
            .unwrap();
    let public =
        proxy_models_for_selection(&state, Some(AppKind::Codex), Some(&stored.provider.id))
            .await
            .unwrap()
            .0;
    assert_eq!(public.stale, Some(true));
    assert_eq!(public.fetched_at_ms, Some(management.fetched_at_ms));
    assert_eq!(public.source.as_deref(), Some(management.source));
    assert_eq!(public.data.len(), 1);
    assert_eq!(public.data[0].id, management.models[0].id);
    assert_eq!(public.data[0].context_window, Some(123456));
    assert_eq!(public.data[0].max_output_tokens, Some(8192));
    assert_eq!(public.data[0].supports_tools, Some(false));
    assert!(public.data[0].reasoning_efforts.is_none());
    assert_eq!(
        management.models[0].effective_capabilities["structuredOutput"],
        false
    );
    zen::clear_catalog();
}
