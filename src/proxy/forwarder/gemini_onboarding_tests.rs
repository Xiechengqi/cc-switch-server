#[tokio::test]
async fn gemini_r5_missing_project_never_onboards_polls_refreshes_or_generates_on_either_app() {
    for app in [AppKind::Gemini, AppKind::Claude] {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = seen.clone();
        let upstream =
            axum::Router::new().fallback(move |uri: axum::http::Uri, headers: HeaderMap| {
                let observed = observed.clone();
                async move {
                    observed
                        .lock()
                        .unwrap()
                        .push((uri.path().to_string(), headers));
                    axum::Json(
                        json!({"allowedTiers":[{"id":"synthetic-paid","isDefault":true}],
                    "name":"operations/synthetic","done":false}),
                    )
                    .into_response()
                }
            });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let endpoint = format!("http://{address}");
        let state = forwarder_test_state(&format!("gemini-r5-missing-project-{app:?}"));
        let (id, account_id) = install_gemini_v1internal_test_provider(
            &state,
            "r5-no-project",
            endpoint.clone(),
            format!("{endpoint}/token"),
            false,
        )
        .await;
        if app == AppKind::Claude {
            let mut providers = state.providers_snapshot().await;
            let stored = &mut providers.providers[0];
            stored.app = AppKind::Claude;
            stored.resource.profile_id = Some(
                crate::domain::providers::registry::ProfileId::parse("claude.google_oauth")
                    .unwrap(),
            );
            let accounts = state.accounts_snapshot().await;
            providers.rebuild_runtime_index(&accounts).unwrap();
            let mut plan = providers.runtime_plan(app, &id).unwrap().as_ref().clone();
            plan.endpoint = endpoint;
            Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
            state.replace_provider_store_for_test(providers).await;
        }
        let (route, path, body) = if app == AppKind::Gemini {
            (
                ProxyRoute::Gemini,
                Some("models/gemini-3.1-pro-preview:generateContent".into()),
                json!({"contents":[{"parts":[{"text":"hello"}]}]}),
            )
        } else {
            (
                ProxyRoute::ClaudeMessages,
                None,
                json!({"model":"gemini-3.1-pro-preview","max_tokens":32,"messages":[{"role":"user","content":"hello"}]}),
            )
        };
        let error = forward_for_test_surface(
            state.clone(),
            route,
            id,
            path,
            gemini_v1internal_downstream_headers(),
            Bytes::from(body.to_string()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{app:?}: {error:?}");
        {
            let seen = seen.lock().unwrap();
            assert_eq!(
                seen.len(),
                1,
                "{app:?}: no generation, refresh, onboard or poll"
            );
            assert_eq!(seen[0].0, "/v1internal:loadCodeAssist");
            assert_eq!(seen[0].1["authorization"], "Bearer gemini-initial-access");
        }
        let account = state.find_account_by_id(&account_id).await.unwrap();
        assert!(account
            .last_refresh_error
            .as_deref()
            .is_some_and(|error| error.contains("no usable projectId")));
        assert!(!account.needs_relogin);
        assert_eq!(account.refresh_consecutive_failures, 0);
        assert!(crate::domain::accounts::store::gemini_v1internal_project_id(&account).is_none());
        assert_eq!(
            account.access_token.as_deref(),
            Some("gemini-initial-access")
        );
        server.abort();
    }
}
