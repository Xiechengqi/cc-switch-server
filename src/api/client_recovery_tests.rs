// Included in api::grok_catalog_provider_tests to reuse the isolated API fixture.

#[tokio::test]
async fn identity_recovery_keeps_local_login_available_and_bootstrap_closed() {
    let state = catalog_test_state("identity-recovery-login");
    let mut config = crate::domain::settings::config::ServerConfig::from_setup(
        crate::domain::settings::config::SetupInput {
            password: "existing-password-123".into(),
            owner_email: "owner@example.com".into(),
            router_url: "http://127.0.0.1:9".into(),
            client_tunnel_subdomain: Some("original-client".into()),
            options: None,
        },
    )
    .unwrap();
    config.client.tunnel_status = Some("claim_conflict".into());
    state.replace_config(config).await.unwrap();
    let before = state.config_snapshot().await;
    let app = app_router(state.clone());
    let status = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status: Value = serde_json::from_slice(
        &axum::body::to_bytes(status.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["needsSetup"], false);
    assert_eq!(status["initialized"], true);

    for path in ["/api/setup", "/api/setup/bootstrap", "/api/setup/validate"] {
        let result = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({
                            "password":"replacement-password-123", "ownerEmail":"other@example.com",
                            "routerUrl":"http://127.0.0.1:9", "clientTunnelSubdomain":"replacement"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::CONFLICT, "{path}");
    }
    let login = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"password":"existing-password-123"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let after = state.config_snapshot().await;
    assert_eq!(before.auth.password_hash, after.auth.password_hash);
    assert_eq!(before.owner.email, after.owner.email);
    assert_eq!(
        before.client.tunnel_subdomain,
        after.client.tunnel_subdomain
    );
    assert_eq!(before.initialized_at_ms, after.initialized_at_ms);
}

#[test]
fn identity_recovery_error_is_visible_without_a_tunnel_url() {
    let status =
        web_client_tunnel_share_status(Some(crate::clients::router::tunnel::TunnelRuntimeStatus {
            status: "blocked".into(),
            blocked_reason_code: Some("client_tunnel_subdomain_conflict".into()),
            last_error: Some("reserved Client domain belongs to the original identity".into()),
            ..Default::default()
        }));
    assert_eq!(status["info"], Value::Null);
    assert_eq!(status["recoveryRequired"], true);
    assert_eq!(
        status["blockedReasonCode"],
        "client_tunnel_subdomain_conflict"
    );
    assert!(status["lastError"]
        .as_str()
        .unwrap()
        .contains("original identity"));
}

#[tokio::test]
async fn identity_recovery_diagnostics_distinguish_registration_and_claim() {
    let state = catalog_test_state("identity-recovery-diagnostics");
    let mut config = crate::domain::settings::config::ServerConfig::from_setup(
        crate::domain::settings::config::SetupInput {
            password: "existing-password-123".into(),
            owner_email: "owner@example.com".into(),
            router_url: "http://127.0.0.1:9".into(),
            client_tunnel_subdomain: Some("original-client".into()),
            options: None,
        },
    )
    .unwrap();
    config.router.identity = Some(RouterIdentity {
        installation_id: TEST_INSTALLATION_ID.into(),
        public_key: "public-key".into(),
        private_key: "private-key".into(),
        control_secret: Some(TEST_CONTROL_SECRET.into()),
    });
    config.client.tunnel_status = Some("claim_conflict".into());
    state.replace_config(config).await.unwrap();
    state
        .push_session(crate::state::Session {
            token: "recovery-admin".into(),
        })
        .await;
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        "Bearer recovery-admin".parse().unwrap(),
    );
    let diagnostic = router_status(State(state.clone()), headers.clone())
        .await
        .unwrap()
        .0;
    assert!(diagnostic.connectivity.initialized);
    assert!(diagnostic.connectivity.registered);
    assert!(!diagnostic.connectivity.client_tunnel_online);
    assert!(diagnostic.connectivity.recovery_required);
    assert_eq!(diagnostic.connectivity.phase, "claim_conflict");
    let mut config = state.config_snapshot().await;
    config.client.tunnel_status = Some("pending".into());
    config.router.last_register_error =
        Some("client_identity_recovery_required: mismatched ID".into());
    state.replace_config(config).await.unwrap();
    let diagnostic = router_status(State(state.clone()), headers)
        .await
        .unwrap()
        .0;
    assert_eq!(diagnostic.connectivity.phase, "identity_recovery_required");
    assert!(diagnostic.connectivity.local_ready);
}
