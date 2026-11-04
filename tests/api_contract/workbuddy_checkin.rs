#[tokio::test]
async fn checkin_rest_and_web_share_admin_auth_identity_and_policy_revision() {
    let state = test_state();
    state.mutate_accounts_immediate(|store| store.upsert(serde_json::from_value(json!({
        "id":"checkin-control-account", "providerType":"workbuddy_oauth", "accessToken":"fixture-not-live",
        "profile":{"site":"cn","uid":"fixture-uid","domain":"www.codebuddy.cn","productPlatform":"workbuddy"}
    })).unwrap())).await.unwrap();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let rest = "/api/accounts/checkin-control-account/checkin?authIdentityGeneration=1";
    let response = app
        .clone()
        .oneshot(json_request(Method::GET, rest, json!(null), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(json_request(Method::GET, rest, json!(null), Some(&token)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let initial = json_body(response).await;
    assert_eq!(initial["supported"], true);
    assert_eq!(initial["enabled"], false);
    assert_eq!(initial["time"], "09:00");
    assert_eq!(initial["timezone"], "Asia/Shanghai");
    let policy = json!({"accountId":"checkin-control-account","authIdentityGeneration":1,"revision":0,"enabled":true,"time":"10:45"});
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/auth_set_checkin_policy",
            policy.clone(),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let saved = json_body(response).await;
    assert_eq!(saved["revision"], 1);
    assert_eq!(saved["enabled"], true);
    let response = app
        .clone()
        .oneshot(json_request(Method::GET, rest, json!(null), Some(&token)))
        .await
        .unwrap();
    assert_eq!(json_body(response).await, saved);
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/auth_set_checkin_policy",
            policy,
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    for (method, path, body, expected) in [
        (
            Method::PUT,
            "/api/accounts/checkin-control-account/checkin",
            json!({"authIdentityGeneration":1,"revision":1,"enabled":true,"time":"25:00"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            Method::POST,
            "/api/accounts/checkin-control-account/checkin",
            json!({"authIdentityGeneration":2}),
            StatusCode::CONFLICT,
        ),
        (
            Method::POST,
            "/web-api/invoke/auth_checkin",
            json!({"accountId":"checkin-control-account","authIdentityGeneration":2}),
            StatusCode::CONFLICT,
        ),
        (
            Method::POST,
            "/web-api/invoke/auth_get_checkin",
            json!({"accountId":"checkin-control-account","authIdentityGeneration":1,"origin":"https://untrusted.invalid"}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(json_request(method, path, body, Some(&token)))
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    // Reading or editing a policy must not contact upstream or create receipts.
    let snapshot = serde_json::to_value(state.accounts_snapshot().await).unwrap();
    assert_eq!(
        snapshot["buddyCheckins"]["checkin-control-account"]["attempts"],
        0
    );
    assert_eq!(
        snapshot["buddyCheckins"]["checkin-control-account"]["lastResult"],
        Value::Null
    );
}
