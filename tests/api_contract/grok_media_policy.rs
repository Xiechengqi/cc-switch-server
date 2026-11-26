#[tokio::test]
async fn bundle_grok_media_writes_round_trip_only_for_shares_that_enforce_them() {
    let state = test_state();
    let account = "media-fixture";
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(test_account_input(account, ProviderType::GrokOAuth));
        })
        .await
        .unwrap();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let bundle = "media-bundle";
    let post = async |path: &str, input: Value| {
        custom_share_api(&app, &token, Method::POST, path, input).await
    };
    let (status, result) = post(
        "/api/provider-bundles",
        grok_provider_bundle_draft(bundle, account, "media-create"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    let save = "/web-api/invoke/save_provider_bundle_share";
    let single_save = "/web-api/invoke/save_provider_share";
    let mut command = custom_bundle_share_input(bundle);
    let bits = |image, edit, video| json!({"imageGenerationEnabled":image, "imageEditEnabled":edit, "videoGenerationEnabled":video});
    command["params"]["grokMediaPolicy"] = bits(true, false, false);
    for enabled in [false, true] {
        command["params"]["enabled"] = json!(enabled);
        let (status, result) = post(save, command.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{result}");
        assert_eq!(result["code"], "CAPABILITY_UPGRADE_REQUIRED");
    }

    // Older editors send all three defaults on create. The new Share still
    // grants no operations to either the owner or the invited user.
    command["params"]["grokMediaPolicy"] = bits(false, false, false);
    let (status, created) = post(save, command.clone()).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["grantCapabilityPolicy"], true);
    for grant in created["userGrants"].as_object().unwrap().values() {
        assert_eq!(grant["policy"]["allowedOperations"], json!([]));
    }
    let modern_id = created["id"].as_str().unwrap();
    command["params"]["shareId"] = json!(modern_id);
    command["params"]["enabled"] = json!(false);
    command["params"]["expectedConfigRevision"] = created["configRevision"].clone();
    command["params"]
        .as_object_mut()
        .unwrap()
        .remove("userGrants");
    command["params"]["grokMediaPolicy"] = bits(true, true, true);
    for path in [save, single_save] {
        let (status, error) = post(path, command.clone()).await;
        assert_eq!(status, StatusCode::CONFLICT, "{error}");
        assert_eq!(error["code"], "CAPABILITY_UPGRADE_REQUIRED");
    }
    let mut named = command.clone();
    named["params"]
        .as_object_mut()
        .unwrap()
        .remove("grokMediaPolicy");
    named["params"]["grantCapabilityEdits"] =
        json!({"friend@example.com":{"expectedRevision":1,"operations":["image.edit"]}});
    let (status, error) = post(single_save, named).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"], "CAPABILITY_UPGRADE_REQUIRED");

    // Seed a distinct historical record with the same Provider bindings.
    // An existing v14 Share cannot be downgraded through any state writer.
    cc_switch_server::state::stop_share_tunnel(&state, modern_id).await;
    let id = "legacy-media-share";
    state
        .mutate_shares_immediate(|store| {
            let share = store.shares.iter_mut().find(|s| s.id == modern_id).unwrap();
            share.id = id.into();
            share.grant_capabilities = None;
        })
        .await
        .unwrap();
    command["params"]["shareId"] = json!(id);
    for policy in [
        bits(true, false, false),
        bits(false, true, false),
        bits(false, false, true),
        bits(false, false, false),
        bits(true, false, true),
    ] {
        command["params"]["grokMediaPolicy"] = policy.clone();
        let (status, saved) = post(save, command.clone()).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        assert_eq!(saved["grokMediaPolicy"], policy);
        command["params"]["expectedConfigRevision"] = saved["configRevision"].clone();
        let (status, detail) =
            post("/web-api/invoke/get_share_detail", json!({"shareId":id})).await;
        assert_eq!(status, StatusCode::OK, "{detail}");
        assert_eq!(detail["grokMediaPolicy"], policy);
    }
    command["params"]
        .as_object_mut()
        .unwrap()
        .remove("grokMediaPolicy");
    command["params"]["description"] = json!("description-only edit");
    let (status, saved) = post(save, command.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["grokMediaPolicy"], bits(true, false, true));
    let dir = state.config_dir.clone();
    cc_switch_server::state::stop_client_tunnel(&state).await;
    let stopped = Arc::downgrade(&state);
    drop(app);
    drop(state);
    tokio::time::timeout(Duration::from_secs(5), async {
        while stopped.strong_count() != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("release the fixture before reopening");
    let state = load_test_state(Ipv4Addr::LOCALHOST.into(), dir);
    let app = app_router(state.clone());
    let token = login_test_admin(&app).await;
    let (status, reopened) = custom_share_api(
        &app,
        &token,
        Method::POST,
        "/web-api/invoke/get_share_detail",
        json!({"shareId":id}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reopened}");
    assert_eq!(reopened["grokMediaPolicy"], bits(true, false, true));

    // The single-Provider compatibility command also waits for Router sync.
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{}", listener.local_addr().unwrap()));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "media-fixture".into(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("fixture-control".into()),
    });
    state.replace_config(config).await.unwrap();
    let router = Router::new()
        .route(
            "/v1/shares/batch-sync",
            axum::routing::post(|| async { StatusCode::OK }),
        )
        .route(
            "/v1/shares/runtime-refresh",
            axum::routing::post(|| async { StatusCode::OK }),
        );
    let sync_task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    command["params"]["expectedConfigRevision"] = reopened["configRevision"].clone();
    command["params"]["grokMediaPolicy"] = bits(false, true, false);
    let mut single_command = command.clone();
    single_command["params"]["userGrants"] = reopened["userGrants"].clone();
    let (status, saved) =
        custom_share_api(&app, &token, Method::POST, single_save, single_command).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["grokMediaPolicy"], bits(false, true, false));
    assert_eq!(saved["userGrants"]["friend@example.com"]["active"], true);
    sync_task.abort();

    state
        .mutate_shares_immediate(|store| {
            store
                .shares
                .iter_mut()
                .find(|s| s.id == id)
                .unwrap()
                .capability_policy = Some(Default::default());
        })
        .await
        .unwrap();
    command["params"]["expectedConfigRevision"] = saved["configRevision"].clone();
    command["params"]["grokMediaPolicy"] = bits(false, false, false);
    let (status, error) = custom_share_api(&app, &token, Method::POST, save, command).await;
    assert_eq!(status, StatusCode::CONFLICT, "{error}");
    assert_eq!(error["code"], "CAPABILITY_UPGRADE_REQUIRED");
}
