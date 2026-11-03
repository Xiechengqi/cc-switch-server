async fn buddy_catalog_account(
    state: &ServerState,
    product: ProviderType,
    site: &str,
    endpoint: &str,
) -> Account {
    let domain = if site == "cn" {
        "www.codebuddy.cn"
    } else {
        "www.codebuddy.ai"
    };
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": format!("{product:?}-{site}"), "providerType": product,
                    "accessToken": "fixture-only-access", "expiresAt": i64::MAX,
                    "profile": {"site":site,"domain":domain,"uid":"fixture-user"},
                    "raw": {"testCodeBuddyBaseUrl":endpoint}
                }))
                .unwrap(),
            )
        })
        .await
        .unwrap()
}

fn buddy_preview_scope(
    account: &Account,
    incarnation: u64,
) -> crate::proxy::codebuddy_runtime::CodeBuddyRuntimeScope {
    let profile = crate::domain::codebuddy::CodeBuddyAccountProfile::parse_for(
        account.provider_type,
        account.profile.as_ref(),
    )
    .unwrap();
    crate::proxy::codebuddy_runtime::CodeBuddyRuntimeScope::account_preview(
        account.provider_type,
        &account.id,
        profile.site,
        &profile.domain,
        account.auth_identity_generation,
        account.token_refresh_generation,
        incarnation,
        &codebuddy_request_identity_fingerprint(account),
    )
    .unwrap()
}

#[tokio::test]
async fn buddy_account_catalog_is_available_without_provider_and_force_refresh_preserves_empty() {
    let calls = Arc::new(AtomicUsize::new(0));
    let empty = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (handler_calls, handler_empty) = (calls.clone(), empty.clone());
    let app = Router::new().route("/v3/config", get(move || {
        let (calls, empty) = (handler_calls.clone(), handler_empty.clone());
        async move {
            calls.fetch_add(1, AtomicOrdering::SeqCst);
            Json(if empty.load(AtomicOrdering::SeqCst) { json!({"models":[]}) }
                else { json!({"models":[{"id":"hy3"},{"id":"fast-model"},{"id":"unreviewed-model"},{"id":"default-model","enable":false}]}) })
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
        for site in ["cn", "intl"] {
            let state = test_state();
            let account = buddy_catalog_account(&state, product, site, &endpoint).await;
            let before = serde_json::to_value(state.accounts_snapshot().await).unwrap();
            let count = calls.load(AtomicOrdering::SeqCst);
            for _ in 0..2 {
                let catalog = state
                    .prepare_buddy_account_catalog(
                        product,
                        &account.id,
                        account.auth_identity_generation,
                        Duration::from_secs(2),
                        false,
                    )
                    .await
                    .unwrap();
                let expected = if site == "cn" {
                    vec!["hy3"]
                } else {
                    vec!["fast-model", "hy3"]
                };
                assert_eq!(catalog.enabled_models, expected);
                assert!(!catalog.stale);
            }
            assert_eq!(calls.load(AtomicOrdering::SeqCst), count + 1);
            assert_eq!(
                serde_json::to_value(state.accounts_snapshot().await).unwrap(),
                before
            );
            assert!(state.providers.read().await.providers.is_empty());
            empty.store(true, AtomicOrdering::SeqCst);
            let catalog = state
                .prepare_buddy_account_catalog(
                    product,
                    &account.id,
                    account.auth_identity_generation,
                    Duration::from_secs(2),
                    true,
                )
                .await
                .unwrap();
            assert!(catalog.enabled_models.is_empty());
            assert_eq!(calls.load(AtomicOrdering::SeqCst), count + 2);
            empty.store(false, AtomicOrdering::SeqCst);
            let cached = state
                .prepare_buddy_account_catalog(
                    product,
                    &account.id,
                    account.auth_identity_generation,
                    Duration::from_secs(2),
                    false,
                )
                .await
                .unwrap();
            assert!(cached.enabled_models.is_empty());
            assert_eq!(calls.load(AtomicOrdering::SeqCst), count + 2);
            assert!(state
                .prepare_buddy_account_catalog(
                    product,
                    &account.id,
                    account.auth_identity_generation + 1,
                    Duration::from_secs(2),
                    false
                )
                .await
                .is_err());
        }
    }
    server.abort();
}

#[tokio::test]
async fn buddy_account_catalog_uses_stale_only_for_transient_errors() {
    let status = Arc::new(AtomicUsize::new(503));
    let handler_status = status.clone();
    let app = Router::new().route(
        "/v3/config",
        get(move || {
            let status = handler_status.clone();
            async move {
                (
                    StatusCode::from_u16(status.load(AtomicOrdering::SeqCst) as u16).unwrap(),
                    "invalid JSON",
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = test_state();
    let account =
        buddy_catalog_account(&state, ProviderType::WorkBuddyOAuth, "cn", &endpoint).await;
    let (_, incarnation) = state
        .find_account_for_provider_with_incarnation(account.provider_type, &account.id)
        .await
        .unwrap();
    let scope = buddy_preview_scope(&account, incarnation);
    let parsed = crate::proxy::codebuddy_runtime::parse_buddy_model_catalog(
        &json!({"models":[{"id":"hy3"}]}),
        account.provider_type,
        crate::domain::codebuddy::CodeBuddySite::Cn,
    )
    .unwrap();
    state
        .codebuddy_runtime
        .insert_catalog(
            scope.clone(),
            parsed,
            crate::infra::time::now_ms() as i64 - 9 * 60_000,
        )
        .await;
    let stale = state
        .prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            false,
        )
        .await
        .unwrap();
    assert!(stale.stale);
    let fresh = crate::proxy::codebuddy_runtime::parse_buddy_model_catalog(
        &json!({"models":[{"id":"hy3"}]}),
        account.provider_type,
        crate::domain::codebuddy::CodeBuddySite::Cn,
    )
    .unwrap();
    state
        .codebuddy_runtime
        .insert_catalog(scope.clone(), fresh, crate::infra::time::now_ms() as i64)
        .await;
    // Even a previously fresh entry becomes reference-only after a failed
    // manual refresh, including when the next form opens inside its old TTL.
    let fallback = state
        .prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            true,
        )
        .await
        .unwrap();
    assert!(fallback.stale);
    assert!(state
        .codebuddy_runtime
        .catalog(&scope, crate::infra::time::now_ms() as i64)
        .await
        .is_none());
    status.store(200, AtomicOrdering::SeqCst);
    assert!(state
        .prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            false
        )
        .await
        .is_err());
    assert!(state
        .codebuddy_runtime
        .stale_catalog(&scope, crate::infra::time::now_ms() as i64)
        .await
        .is_none());
    let fresh = crate::proxy::codebuddy_runtime::parse_buddy_model_catalog(
        &json!({"models":[{"id":"hy3"}]}),
        account.provider_type,
        crate::domain::codebuddy::CodeBuddySite::Cn,
    )
    .unwrap();
    state
        .codebuddy_runtime
        .insert_catalog(scope.clone(), fresh, crate::infra::time::now_ms() as i64)
        .await;
    status.store(401, AtomicOrdering::SeqCst);
    let error = state
        .prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            true,
        )
        .await
        .unwrap_err();
    assert!(error.is_authentication_failure());
    assert!(state
        .codebuddy_runtime
        .stale_catalog(&scope, crate::infra::time::now_ms() as i64)
        .await
        .is_none());
    server.abort();
}

#[tokio::test]
async fn buddy_account_catalog_discards_late_identity_and_recreated_account_results() {
    for recreate in [false, true] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let (handler_entered, handler_resume) = (entered.clone(), resume.clone());
        let app = Router::new().route(
            "/v3/config",
            get(move || {
                let (entered, resume) = (handler_entered.clone(), handler_resume.clone());
                async move {
                    entered.notify_one();
                    resume.notified().await;
                    Json(json!({"models":[{"id":"hy3"}]}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = test_state();
        let account =
            buddy_catalog_account(&state, ProviderType::WorkBuddyOAuth, "cn", &endpoint).await;
        let (_, incarnation) = state
            .find_account_for_provider_with_incarnation(account.provider_type, &account.id)
            .await
            .unwrap();
        let scope = buddy_preview_scope(&account, incarnation);
        let (request_state, request_account) = (state.clone(), account.clone());
        let request = tokio::spawn(async move {
            request_state
                .prepare_buddy_account_catalog(
                    request_account.provider_type,
                    &request_account.id,
                    request_account.auth_identity_generation,
                    Duration::from_secs(5),
                    false,
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), entered.notified())
            .await
            .unwrap();
        if recreate {
            state
                .mutate_accounts_immediate(|accounts| {
                    assert!(accounts.delete(&account.id));
                })
                .await
                .unwrap();
            let current =
                buddy_catalog_account(&state, account.provider_type, "cn", &endpoint).await;
            assert_eq!(
                current.auth_identity_generation,
                account.auth_identity_generation
            );
        } else {
            state
                .mutate_accounts_immediate(|accounts| {
                    accounts
                        .accounts
                        .iter_mut()
                        .find(|current| current.id == account.id)
                        .unwrap()
                        .access_token = Some("rotated".into());
                })
                .await
                .unwrap();
        }
        resume.notify_one();
        assert!(matches!(
            request.await.unwrap(),
            Err(CodeBuddyRuntimeError::IdentityChanged)
        ));
        assert!(state
            .codebuddy_runtime
            .catalog(&scope, crate::infra::time::now_ms() as i64)
            .await
            .is_none());
        server.abort();
    }
}

#[tokio::test]
async fn buddy_account_catalog_concurrent_manual_refreshes_share_one_fetch() {
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let (handler_calls, handler_entered, handler_resume) =
        (calls.clone(), entered.clone(), resume.clone());
    let app = Router::new().route(
        "/v3/config",
        get(move || {
            let (calls, entered, resume) = (
                handler_calls.clone(),
                handler_entered.clone(),
                handler_resume.clone(),
            );
            async move {
                let count = calls.fetch_add(1, AtomicOrdering::SeqCst);
                if count > 0 {
                    entered.notify_one();
                    resume.notified().await;
                }
                Json(json!({"models":[{"id":if count == 0 {"default"} else {"hy3"}}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = test_state();
    let account =
        buddy_catalog_account(&state, ProviderType::WorkBuddyOAuth, "cn", &endpoint).await;
    let initial = state
        .prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            false,
        )
        .await
        .unwrap();
    assert_eq!(initial.enabled_models, ["default"]);
    let (first, second, _) = tokio::join!(
        state.prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            true
        ),
        state.prepare_buddy_account_catalog(
            account.provider_type,
            &account.id,
            account.auth_identity_generation,
            Duration::from_secs(2),
            true
        ),
        async {
            tokio::time::timeout(Duration::from_secs(5), entered.notified())
                .await
                .unwrap();
            resume.notify_one();
        }
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.enabled_models, ["hy3"]);
    assert!(first.same_snapshot(&second));
    assert!(!first.same_snapshot(&initial));
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 2);
    server.abort();
}
