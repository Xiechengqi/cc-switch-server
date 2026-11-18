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
