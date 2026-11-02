// Included in accounts::tests to exercise the actual JSON handler and invoke bridge.
async fn buddy_import_request(
    state: &ServerState,
    body: Value,
    authenticated: bool,
) -> (StatusCode, Value) {
    use tower::ServiceExt;
    let router = axum::Router::new()
        .route(
            "/api/accounts/buddy/import",
            axum::routing::post(import_buddy_credentials),
        )
        .with_state(state.clone());
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/accounts/buddy/import")
        .header("content-type", "application/json");
    if authenticated {
        request = request.header("x-cc-switch-web-user-email", "owner@example.com");
    }
    let response = router
        .oneshot(
            request
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

fn buddy_import_payload(product: ProviderType, uid: &str, token: &str) -> Value {
    json!({"providerType":product.as_str(), "site":"intl", "preview":false,
        "payload":{"uid":uid, "accessToken":"fixture-access-secret", "refreshToken":token, "domain":"www.workbuddy.ai"}})
}

#[tokio::test]
async fn codebuddy_workbuddy_import_preview_atomicity_and_independent_identity() {
    let state = account_api_test_state("buddy-import-atomic");
    let mut input = buddy_import_payload(
        ProviderType::WorkBuddyOAuth,
        "same-user",
        "workbuddy-refresh-secret",
    );
    let (status, _) = buddy_import_request(&state, input.clone(), false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    input["preview"] = json!(true);
    let (status, preview) = buddy_import_request(&state, input.clone(), true).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert!(state.accounts_snapshot().await.accounts.is_empty());
    assert!(!preview.to_string().contains("fixture-access-secret"));
    assert!(!preview.to_string().contains("workbuddy-refresh-secret"));
    input["preview"] = json!(false);
    let (status, result) = buddy_import_request(&state, input.clone(), true).await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let workbuddy_id = result["accounts"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(result["accounts"][0]["id"], preview["accounts"][0]["id"]);
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-cc-switch-web-user-email",
        HeaderValue::from_static("owner@example.com"),
    );
    let Json(invoked) = crate::api::invoke::dispatch::web_invoke_compat(
        State(state.clone()),
        headers,
        Path("auth_import_buddy".to_string()),
        axum::body::Bytes::from(
            buddy_import_payload(
                ProviderType::CodeBuddyOAuth,
                "same-user",
                "codebuddy-refresh-secret",
            )
            .to_string(),
        ),
    )
    .await
    .unwrap();
    let codebuddy_id = invoked["accounts"][0]["id"].as_str().unwrap();
    assert_ne!(codebuddy_id, workbuddy_id);
    assert_eq!(state.accounts_snapshot().await.accounts.len(), 2);
    let (status, _) = buddy_import_request(&state, input, true).await;
    assert_eq!(status, StatusCode::OK); // same-product stable identity updates the owner
    assert_eq!(state.accounts_snapshot().await.accounts.len(), 2);
    let mut batch = buddy_import_payload(ProviderType::WorkBuddyOAuth, "unused", "unused");
    batch["payload"] = json!([
        {"uid":"new-user","accessToken":"new-access", "refreshToken":"new-unique-refresh", "domain":"www.workbuddy.ai"},
        {"uid":"another-user","accessToken":"other-access", "refreshToken":"codebuddy-refresh-secret", "domain":"www.workbuddy.ai"}
    ]);
    let (status, result) = buddy_import_request(&state, batch, true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{result}");
    assert!(!result.to_string().contains("codebuddy-refresh-secret"));
    assert_eq!(state.accounts_snapshot().await.accounts.len(), 2); // no first-row partial commit
    let disk =
        crate::domain::accounts::store::AccountStore::load_or_default(&state.config_dir).unwrap();
    assert_eq!(disk.accounts.len(), 2);
    let _ = std::fs::remove_dir_all(&state.config_dir);
}

#[tokio::test]
async fn workbuddy_legacy_duplicate_rotating_token_is_rejected_before_outbound() {
    let state = account_api_test_state("buddy-legacy-duplicate");
    state
        .mutate_accounts_immediate(|accounts| {
            for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
                let inputs = crate::domain::accounts::buddy_import::normalize_buddy_import(
                    product,
                    crate::domain::codebuddy::CodeBuddySite::Intl,
                    &buddy_import_payload(product, "same-user", "legacy-shared-refresh")["payload"],
                )
                .unwrap();
                accounts.upsert(inputs.into_iter().next().unwrap());
            }
        })
        .await
        .unwrap();
    for account in state.accounts_snapshot().await.accounts {
        let error = state
            .refresh_managed_account_now_for_generation(
                account.provider_type,
                &account.id,
                account.auth_identity_generation,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            crate::state::ManagedAccountRefreshError::Refresh {
                status_code: 409,
                ..
            }
        ));
    }
    let _ = std::fs::remove_dir_all(&state.config_dir);
}
