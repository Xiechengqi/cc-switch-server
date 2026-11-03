// Included in grok_catalog_provider_tests to reuse signed-ingress fixtures.
#[tokio::test]
async fn buddy_account_catalog_invoke_requires_auth_and_supports_unsaved_forms_without_secrets() {
    use crate::domain::codebuddy::CodeBuddySite;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let upstream = Router::new().route("/v3/config", get(|| async {
        Json(json!({"models":[{"id":"hy3","name":"HY3","supportsToolCall":true,"prompt":"private-prompt"},
            {"id":"fast-model","name":"Fast","supportsToolCall":true,"Uin":"private-identity"}]}))
    }));
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
        for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
            let state = catalog_test_state("buddy-account-preview");
            state.mutate_accounts_immediate(|accounts| accounts.upsert(serde_json::from_value(json!({
                "id":"preview-account","providerType":product,"accessToken":"private-access","expiresAt":i64::MAX/2,
                "profile":{"site":site,"domain":site.profile().domain,"uid":"private-uid"},
                "raw":{"testCodeBuddyBaseUrl":origin}
            })).unwrap())).await.unwrap();
            let accounts_before = serde_json::to_value(state.accounts_snapshot().await).unwrap();
            let router = Router::new()
                .route(
                    "/web-api/invoke/:command",
                    axum::routing::post(web_invoke_compat),
                )
                .with_state(state.clone());
            let valid = json!({"providerType":product,"accountId":"preview-account","authIdentityGeneration":1});
            let mut cases = vec![
                (valid.clone(), false, StatusCode::UNAUTHORIZED),
                (valid.clone(), true, StatusCode::OK),
            ];
            for (key, value) in [
                ("authIdentityGeneration", json!(2)),
                ("accountId", json!("missing")),
                ("baseUrl", json!("https://arbitrary.invalid")),
                ("providerType", json!("grok_oauth")),
            ] {
                let mut invalid = valid.clone();
                invalid[key] = value;
                cases.push((
                    invalid,
                    true,
                    if key == "authIdentityGeneration" {
                        StatusCode::CONFLICT
                    } else if key == "accountId" {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::BAD_REQUEST
                    },
                ));
            }
            for (payload, authenticated, expected_status) in cases {
                let mut builder = Request::builder()
                    .method("POST")
                    .uri("/web-api/invoke/fetch_buddy_account_models")
                    .header("content-type", "application/json");
                if authenticated {
                    builder = builder.header("x-cc-switch-web-user-email", "owner@example.com");
                }
                let response = router
                    .clone()
                    .oneshot(builder.body(Body::from(payload.to_string())).unwrap())
                    .await
                    .unwrap();
                let status = response.status();
                let body: Value = serde_json::from_slice(
                    &axum::body::to_bytes(response.into_body(), 65536)
                        .await
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(status, expected_status, "{product:?}/{site:?}: {body}");
                for secret in [
                    "private-access",
                    "private-uid",
                    "private-prompt",
                    "private-identity",
                    "testCodeBuddyBaseUrl",
                ] {
                    assert!(!body.to_string().contains(secret), "leaked {secret}");
                }
                if status == StatusCode::OK {
                    assert_eq!(body["providerType"], json!(product));
                    assert_eq!(
                        body["models"][0]["id"],
                        if site == CodeBuddySite::Cn {
                            "hy3"
                        } else {
                            "fast-model"
                        }
                    );
                    assert_eq!(body["models"][0]["raw"]["capabilities"]["tools"], true);
                }
            }
            assert!(state.providers.read().await.providers.is_empty());
            assert_eq!(
                serde_json::to_value(state.accounts_snapshot().await).unwrap(),
                accounts_before
            );
        }
    }
    server.abort();
}

async fn install_buddy_catalog_fixture(
    state: &ServerState,
    product: ProviderType,
    app: AppKind,
    site: crate::domain::codebuddy::CodeBuddySite,
    origin: &str,
) {
    let policy = crate::domain::codebuddy::buddy_product_policy(product, site).unwrap();
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": "buddy-account", "providerType": product.as_str(),
                    "accessToken": "fixture-access", "refreshToken": "fixture-refresh",
                    "expiresAt": i64::MAX / 2,
                    "profile": {"site": site.as_str(), "domain": site.profile().domain,
                        "uid": "fixture-uid", "productPlatform": policy.auth_platform},
                    "raw": {"observedAtMs": chrono::Utc::now().timestamp_millis(),
                        "testCodeBuddyBaseUrl": origin}
                }))
                .unwrap(),
            );
        })
        .await
        .unwrap();
    state.mutate_providers_immediate(move |providers| {
        providers.upsert(app, Provider {
            id: "buddy-provider".to_string(), name: "Buddy fixture".to_string(),
            settings_config: json!({"modelMapping":{"mode":"single","upstreamModel":"configured-decoy"}}),
            category: None,
            meta: Some(ProviderMeta {
                provider_type: Some(product.as_str().to_string()),
                auth_binding: Some(AuthBinding {
                    source: Some("account_store".to_string()),
                    auth_provider: Some(product.as_str().to_string()),
                    account_id: Some("buddy-account".to_string()),
                    auth_identity_generation: Some(1),
                }), ..Default::default()
            }), extra: Default::default(),
        })
    }).await.unwrap();
    configure_test_share_binding(
        state,
        "buddy-share",
        app,
        "buddy-provider".to_string(),
        product,
    )
    .await;
}

#[tokio::test]
async fn codebuddy_workbuddy_signed_models_use_live_catalog_and_real_gemini_limits() {
    use crate::domain::codebuddy::CodeBuddySite;
    for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
        for site in [CodeBuddySite::Intl, CodeBuddySite::Cn] {
            for app_kind in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                for empty in [false, true] {
                    let model = if site == CodeBuddySite::Intl {
                        "default-model"
                    } else {
                        "default"
                    };
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let origin = format!("http://{}", listener.local_addr().unwrap());
                    let upstream = Router::new().route("/v3/config", get(move || async move {
                        Json(json!({"code":0,"data":{"models": if empty { vec![] } else { vec![json!({
                            "id":model,"supportsToolCall":true,"supportsReasoning":true,
                            "reasoning":{"supportedEfforts":["low","high"]},
                            "contextWindow":{"defaultLength":64000,"supportedLengths":[64000,128000]},
                            "maxInputTokens":128000,"maxOutputTokens":8192
                        })] }}}))
                    }));
                    let server = tokio::spawn(async move {
                        axum::serve(listener, upstream).await.unwrap();
                    });
                    let state = catalog_test_state("buddy-public-models");
                    configure_test_router(&state).await;
                    install_buddy_catalog_fixture(&state, product, app_kind, site, &origin).await;
                    let router = app_router(state);
                    let path = format!("/v1/models?app={}", app_kind.as_str());
                    let response = router
                        .clone()
                        .oneshot(
                            router_ingress_request(
                                Request::builder().uri(path).body(Body::empty()).unwrap(),
                                "buddy-models",
                                Some("buddy-share"),
                            )
                            .await,
                        )
                        .await
                        .unwrap();
                    assert_eq!(
                        response.status(),
                        StatusCode::OK,
                        "{product:?}/{site:?}/{app_kind:?}/{empty}"
                    );
                    let body: Value = serde_json::from_slice(
                        &axum::body::to_bytes(response.into_body(), 65536)
                            .await
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(
                        body["source"],
                        if product == ProviderType::WorkBuddyOAuth {
                            "workbuddy_live_model_catalog"
                        } else {
                            "codebuddy_live_model_catalog"
                        }
                    );
                    assert_eq!(body["stale"], false);
                    assert_eq!(body["data"].as_array().unwrap().len(), usize::from(!empty));
                    assert!(!body.to_string().contains("configured-decoy"));
                    if !empty {
                        assert_eq!(body["data"][0]["contextWindow"], 64000);
                        assert_eq!(body["data"][0]["maxOutputTokens"], 8192);
                    }
                    if app_kind == AppKind::Gemini {
                        let response = router
                            .oneshot(
                                router_ingress_request(
                                    Request::builder()
                                        .uri("/v1beta/models")
                                        .body(Body::empty())
                                        .unwrap(),
                                    "buddy-gemini-models",
                                    Some("buddy-share"),
                                )
                                .await,
                            )
                            .await
                            .unwrap();
                        assert_eq!(response.status(), StatusCode::OK);
                        let body: Value = serde_json::from_slice(
                            &axum::body::to_bytes(response.into_body(), 65536)
                                .await
                                .unwrap(),
                        )
                        .unwrap();
                        if !empty {
                            assert_eq!(body["models"][0]["inputTokenLimit"], 64000);
                            assert_eq!(body["models"][0]["outputTokenLimit"], 8192);
                        } else {
                            assert_eq!(body["models"], json!([]));
                        }
                    }
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn codebuddy_workbuddy_signed_models_fail_closed_on_protocol_errors() {
    use crate::domain::codebuddy::CodeBuddySite;
    for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let upstream = Router::new().route(
            "/v3/config",
            get(|| async { Json(json!({"code":0,"data":{}})) }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let state = catalog_test_state("buddy-public-models-invalid");
        configure_test_router(&state).await;
        install_buddy_catalog_fixture(
            &state,
            product,
            AppKind::Codex,
            CodeBuddySite::Intl,
            &origin,
        )
        .await;
        let response = app_router(state)
            .oneshot(
                router_ingress_request(
                    Request::builder()
                        .uri("/v1/models?app=codex")
                        .body(Body::empty())
                        .unwrap(),
                    "buddy-models-invalid",
                    Some("buddy-share"),
                )
                .await,
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert!(!String::from_utf8_lossy(&body).contains("configured-decoy"));
        server.abort();
    }
}
