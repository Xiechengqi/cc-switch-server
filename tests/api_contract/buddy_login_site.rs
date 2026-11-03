fn buddy_site_bundle_draft(product: ProviderType, id: &str) -> Value {
    let mut draft = grok_provider_bundle_draft(id, "buddy-site-account", "buddy-site-create");
    draft["familyId"] = json!(format!("family.{}", product.as_str()));
    draft["upstreamModel"] = json!("auto");
    for surface in draft["surfaces"].as_array_mut().unwrap() {
        surface["profileId"] = json!(format!(
            "{}.{}",
            surface["app"].as_str().unwrap(),
            product.as_str()
        ));
    }
    draft
}

#[tokio::test]
async fn buddy_login_site_round_trip_preserves_account_identity_runtime_and_legacy_writes() {
    use cc_switch_server::domain::codebuddy::{buddy_product_policy, CodeBuddySite};
    for product in [ProviderType::WorkBuddyOAuth, ProviderType::CodeBuddyOAuth] {
        for account_site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
            let state = test_state();
            let policy = buddy_product_policy(product, account_site).unwrap();
            state.mutate_accounts_immediate(|accounts| {
                accounts.upsert(serde_json::from_value(json!({
                    "id":"buddy-site-account", "providerType":product,
                    "accessToken":"buddy-site-private-access", "refreshToken":"buddy-site-private-refresh",
                    "expiresAt":i64::MAX / 2,
                    "profile":{"site":account_site,"domain":account_site.profile().domain,
                        "uid":"buddy-site-private-uid","productPlatform":policy.auth_platform}
                })).unwrap());
            }).await.unwrap();
            let accounts_before = serde_json::to_value(state.accounts_snapshot().await).unwrap();
            let app = app_router(state.clone());
            let token = setup_and_login(&app).await;
            let mut draft = buddy_site_bundle_draft(product, "buddy-site-provider");
            let created = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    "/web-api/invoke/upsert_provider_bundle",
                    json!({"bundle":draft}),
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(created.status(), StatusCode::OK);
            let initial = json_body(created).await;
            assert!(initial.get("buddyLoginSite").is_none());
            let mut revision = initial["revision"].as_u64().unwrap();
            draft.as_object_mut().unwrap().remove("clientRequestId");
            for preference in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                draft["expectedRevision"] = json!(revision);
                draft["buddyLoginSite"] = json!(preference);
                let saved = app
                    .clone()
                    .oneshot(json_request(
                        Method::POST,
                        "/web-api/invoke/upsert_provider_bundle",
                        json!({"bundle":draft}),
                        Some(&token),
                    ))
                    .await
                    .unwrap();
                assert_eq!(saved.status(), StatusCode::OK);
                let saved = json_body(saved).await;
                assert_eq!(saved["buddyLoginSite"], json!(preference));
                revision = saved["revision"].as_u64().unwrap();
                for app in ["claude", "codex", "gemini"] {
                    let before = &initial["surfaces"][app];
                    let after = &saved["surfaces"][app];
                    assert!(before["runtime"]["runtimeFingerprint"].is_string());
                    assert!(before["runtime"]["modelPolicy"].is_object());
                    assert_eq!(after["provider"]["buddyLoginSite"], json!(preference));
                    assert_eq!(
                        after["provider"]["meta"]["authBinding"],
                        before["provider"]["meta"]["authBinding"]
                    );
                    assert_eq!(
                        after["runtime"]["runtimeFingerprint"],
                        before["runtime"]["runtimeFingerprint"]
                    );
                    assert_eq!(
                        after["runtime"]["modelPolicy"],
                        before["runtime"]["modelPolicy"]
                    );
                }
            }
            // A client from before this optional field must preserve it, both
            // in read-only validation and when saving another field.
            draft.as_object_mut().unwrap().remove("buddyLoginSite");
            draft["expectedRevision"] = json!(revision);
            draft["notes"] = json!("legacy client edit");
            let validation = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    "/web-api/invoke/validate_provider_bundle_draft",
                    json!({"bundle":draft}),
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(validation.status(), StatusCode::OK);
            let saved = app
                .clone()
                .oneshot(json_request(
                    Method::PATCH,
                    "/api/provider-bundles/buddy-site-provider",
                    draft.clone(),
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(saved.status(), StatusCode::OK);
            let saved = json_body(saved).await["bundle"].clone();
            assert_eq!(saved["buddyLoginSite"], "intl");
            let stale = app
                .clone()
                .oneshot(json_request(
                    Method::PATCH,
                    "/api/provider-bundles/buddy-site-provider",
                    draft,
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(stale.status(), StatusCode::CONFLICT);
            let read_back = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    "/web-api/invoke/get_provider_bundle",
                    json!({"id":"buddy-site-provider"}),
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(json_body(read_back).await["buddyLoginSite"], "intl");

            let projected = app
                .clone()
                .oneshot(json_request(
                    Method::POST,
                    "/web-api/invoke/auth_list_accounts",
                    json!({"authProvider":product}),
                    Some(&token),
                ))
                .await
                .unwrap();
            assert_eq!(projected.status(), StatusCode::OK);
            let projected = json_body(projected).await;
            assert_eq!(projected[0]["buddy"], json!({"site":account_site}));
            for secret in [
                "buddy-site-private-access",
                "buddy-site-private-refresh",
                "buddy-site-private-uid",
                account_site.profile().domain,
            ] {
                assert!(!projected.to_string().contains(secret), "leaked {secret}");
            }
            assert_eq!(
                serde_json::to_value(state.accounts_snapshot().await).unwrap(),
                accounts_before
            );
            // Load the persisted Provider store independently of the live view.
            let persisted =
                cc_switch_server::domain::providers::store::ProviderStore::load_or_default(
                    &state.config_dir,
                )
                .unwrap();
            assert_eq!(persisted.providers.len(), 3);
            assert!(persisted
                .providers
                .iter()
                .all(|stored| stored.provider.extra["buddyLoginSite"] == "intl"));
            let mut inconsistent = persisted
                .providers
                .iter()
                .map(cc_switch_server::domain::providers::credentials::ProviderView::from_stored)
                .collect::<Vec<_>>();
            inconsistent[0]
                .provider
                .extra
                .insert("buddyLoginSite".to_string(), json!("cn"));
            assert!(
                cc_switch_server::domain::providers::bundle::ProviderBundleView::from_surface_views(
                    inconsistent
                )
                .unwrap_err()
                .to_string()
                .contains("metadata is inconsistent")
            );
        }
    }
}

#[tokio::test]
async fn buddy_login_site_projection_does_not_guess_from_invalid_or_unrelated_profiles() {
    let state = test_state();
    state
        .mutate_accounts_immediate(|accounts| {
            for product in [
                ProviderType::WorkBuddyOAuth,
                ProviderType::CodeBuddyOAuth,
                ProviderType::ClaudeOAuth,
            ] {
                let mut input = test_account_input(product.as_str(), product);
                input.profile =
                    Some(json!({"site":"cn", "domain":"unrelated.invalid", "uid":"private-uid"}));
                accounts.upsert(input);
            }
        })
        .await
        .unwrap();
    let app = app_router(state);
    let token = setup_and_login(&app).await;
    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/auth_list_accounts",
            json!({}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let projected = json_body(response).await;
    assert_eq!(projected.as_array().unwrap().len(), 3);
    assert!(projected
        .as_array()
        .unwrap()
        .iter()
        .all(|account| account["buddy"].is_null()));
    assert!(!projected.to_string().contains("private-uid"));
    assert!(!projected.to_string().contains("unrelated.invalid"));
}

#[tokio::test]
async fn buddy_login_site_rejects_invalid_values_wrong_families_and_unauthenticated_writes() {
    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut draft = buddy_site_bundle_draft(ProviderType::WorkBuddyOAuth, "invalid-site");
    draft["buddyLoginSite"] = json!("cn");
    let denied = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/upsert_provider_bundle",
            json!({"bundle":draft}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    for value in [
        json!("global"),
        json!("enterprise"),
        json!("CN"),
        json!(1),
        json!({}),
    ] {
        draft["buddyLoginSite"] = value;
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/upsert_provider_bundle",
                json!({"bundle":draft}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let mut wrong_family =
        grok_provider_bundle_draft("wrong-family", "irrelevant", "wrong-family-create");
    wrong_family["buddyLoginSite"] = json!("cn");
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/upsert_provider_bundle",
            json!({"bundle":wrong_family}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(providers_snapshot(&state).await.providers.is_empty());
}

#[tokio::test]
async fn buddy_login_site_survives_server_restart() {
    let state = test_state();
    for (product, site) in [
        (ProviderType::CodeBuddyOAuth, "cn"),
        (ProviderType::WorkBuddyOAuth, "intl"),
    ] {
        let account_id = format!("buddy-site-{}", product.as_str());
        state
            .mutate_accounts_immediate(|accounts| {
                accounts.upsert(serde_json::from_value(json!({
                "id":account_id, "providerType":product, "accessToken":"restart-fixture",
                "profile":{"site":"cn", "domain":"copilot.tencent.com", "uid":"restart-fixture"}
            })).unwrap());
            })
            .await
            .unwrap();
        let mut draft = buddy_site_bundle_draft(product, product.as_str());
        draft["clientRequestId"] = json!(format!("buddy-site-create-{}", product.as_str()));
        draft["managedAccount"]["accountId"] = json!(account_id);
        draft["buddyLoginSite"] = json!(site);
        state
            .upsert_provider_bundle_command(serde_json::from_value(draft).unwrap())
            .await
            .unwrap()
            .unwrap();
    }
    let config_dir = state.config_dir.clone();
    let stopped = Arc::downgrade(&state);
    drop(state);
    tokio::time::timeout(Duration::from_secs(5), async {
        while stopped.strong_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let reloaded = ServerStateInner::load(
        Cli {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(config_dir),
            web_dist_dir: None,
            log_level: "warn".to_string(),
            command: None,
        },
        Arc::new(cc_switch_server::logging::LogCapture::new(
            cc_switch_server::logging::RING_BUFFER_CAPACITY,
        )),
    )
    .unwrap();
    let app = app_router(reloaded);
    let token = setup_and_login(&app).await;
    for (product, site) in [("codebuddy_oauth", "cn"), ("workbuddy_oauth", "intl")] {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/web-api/invoke/get_provider_bundle",
                json!({"id":product}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["buddyLoginSite"], site);
    }
}
