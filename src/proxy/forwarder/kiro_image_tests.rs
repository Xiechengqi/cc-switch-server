#[tokio::test]
async fn kiro_r5_final_image_fields_are_bounded_on_both_products_and_all_surfaces() {
    use base64::Engine;
    let mut seed = 0x1234_5678_u32;
    let image = image::RgbImage::from_fn(768, 768, |_, _| {
        let mut rgb = [0; 3];
        for value in &mut rgb {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            *value = seed as u8;
        }
        image::Rgb(rgb)
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes.into_inner());
    assert!(encoded.len() > 400_000);
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = seen.clone();
    let upstream = axum::Router::new()
        .fallback(axum::routing::post(
            move |headers: HeaderMap, bytes: Bytes| {
                let observed = observed.clone();
                async move {
                    if let Some(catalog) = kiro_core03_catalog(&headers) {
                        return catalog;
                    }
                    let body: Value = serde_json::from_slice(&bytes).unwrap();
                    observed.lock().unwrap().push((headers, body));
                    (
                        [(CONTENT_TYPE, "application/vnd.amazon.eventstream")],
                        kiro_text_eventstream("hello"),
                    )
                        .into_response()
                }
            },
        ))
        .layer(axum::extract::DefaultBodyLimit::max(8 * 1024 * 1024));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    for kind in [ProviderType::KiroOAuth, ProviderType::AmazonQOAuth] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
        ] {
            let state = forwarder_test_state(&format!("kiro-r5-image-{}-{route:?}", kind.as_str()));
            let id = kiro_core03_provider(
                &state,
                kind,
                route,
                &format!("http://{address}"),
                "http://127.0.0.1:9",
            )
            .await;
            let model = kiro_core03_model(kind);
            let input = match route {
                ProxyRoute::ClaudeMessages => {
                    json!({"model":model,"max_tokens":32,"messages":[{"role":"user","content":[
                    {"type":"image","source":{"type":"base64","media_type":"image/png","data":encoded}}]}]})
                }
                ProxyRoute::CodexResponses => {
                    json!({"model":model,"input":[{"role":"user","content":[
                    {"type":"input_image","image_url":format!("data:image/png;base64,{encoded}")}]}]})
                }
                _ => json!({"model":model,"messages":[{"role":"user","content":[
                    {"type":"image_url","image_url":{"url":format!("data:image/png;base64,{encoded}")}}]}]}),
            };
            let response = forward_for_test_surface(
                state,
                route,
                id,
                None,
                HeaderMap::new(),
                Bytes::from(input.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(
                String::from_utf8_lossy(&collect_response_body(response).await).contains("hello")
            );
            let (headers, body) = seen.lock().unwrap().last().unwrap().clone();
            let message = &body["conversationState"]["currentMessage"]["userInputMessage"];
            let field = message["images"][0]["source"]["bytes"].as_str().unwrap();
            assert!(
                field.len() <= 400_000,
                "{} {route:?}: {} chars",
                kind.as_str(),
                field.len()
            );
            assert_eq!(message["images"].as_array().unwrap().len(), 1);
            assert_eq!(message["images"][0]["format"], "jpeg");
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(field)
                .unwrap();
            image::load_from_memory(&decoded).unwrap();
            assert_eq!(
                message["origin"],
                if kind == ProviderType::AmazonQOAuth {
                    "CLI"
                } else {
                    "AI_EDITOR"
                }
            );
            assert_eq!(
                headers["authorization"],
                if kind == ProviderType::AmazonQOAuth {
                    "Bearer amazon-q-old-access"
                } else {
                    "Bearer kiro-old-access"
                }
            );
        }
    }
    assert_eq!(seen.lock().unwrap().len(), 6);
    server.abort();
}

#[tokio::test]
async fn kiro_r5_local_image_rejection_does_not_generate_refresh_or_change_account_health() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let upstream = axum::Router::new().fallback(move |headers: HeaderMap| {
        let observed = observed.clone();
        async move {
            if let Some(catalog) = kiro_core03_catalog(&headers) {
                return catalog;
            }
            observed.fetch_add(1, Ordering::SeqCst);
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    for kind in [ProviderType::KiroOAuth, ProviderType::AmazonQOAuth] {
        let state =
            forwarder_test_state(&format!("kiro-r5-image-local-rejection-{}", kind.as_str()));
        let id = kiro_core03_provider(
            &state,
            kind,
            ProxyRoute::ClaudeMessages,
            &format!("http://{address}"),
            &format!("http://{address}/token"),
        )
        .await;
        let before = serde_json::to_value(state.accounts_snapshot().await).unwrap();
        let body = json!({"model":kiro_core03_model(kind),"max_tokens":32,"messages":[{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":"invalid base64!"}}
        ]}]});
        let error = forward_for_test_surface(
            state.clone(),
            ProxyRoute::ClaudeMessages,
            id,
            None,
            HeaderMap::new(),
            Bytes::from(body.to_string()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            serde_json::to_value(state.accounts_snapshot().await).unwrap(),
            before
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
    server.abort();
}

#[tokio::test]
async fn kiro_r5_image_result_rejects_stale_account_and_provider_snapshots() {
    for account_changed in [false, true] {
        let state = forwarder_test_state(&format!("kiro-r5-image-stale-binding-{account_changed}"));
        let id = kiro_core03_provider(
            &state,
            ProviderType::KiroOAuth,
            ProxyRoute::ClaudeMessages,
            "http://127.0.0.1:9",
            "http://127.0.0.1:9/token",
        )
        .await;
        let providers = state.providers_snapshot().await;
        let stored = providers
            .providers
            .iter()
            .find(|p| p.provider.id == id)
            .unwrap()
            .clone();
        let execution = ProviderExecution::from_store(&providers, stored.clone()).unwrap();
        let accounts = state.accounts_snapshot().await;
        if account_changed {
            state
                .mutate_accounts_immediate(|store| {
                    let account = store
                        .find_for_provider(ProviderType::KiroOAuth, Some("kiro-core03-account"))
                        .unwrap()
                        .clone();
                    store.mark_refresh_success(
                        &account.id,
                        crate::domain::accounts::store::AccountRefreshUpdate {
                            access_token: Some("changed-after-snapshot".into()),
                            ..Default::default()
                        },
                    );
                })
                .await
                .unwrap();
        } else {
            state
                .replace_provider_store_for_test(ProviderStore::default())
                .await;
        }
        let context = UsageLogContext {
            session_id: Some("kiro-r5-image-binding".into()),
            ..Default::default()
        };
        let body = json!({"model":"claude-sonnet-4-8","max_tokens":32,"messages":[{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png",
                "data":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="}}
        ]}]});
        let result = kiro_provider::prepare_bound_request(
            &state,
            &execution,
            &stored,
            &accounts,
            ProxyRoute::ClaudeMessages,
            &context,
            &body,
            "claude-sonnet-4-8",
            "1.0.0",
            false,
            None,
        )
        .await;
        let error = result.err().expect("stale image results must be rejected");
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert!(
            error.message.contains(if account_changed {
                "account binding changed"
            } else {
                "Provider binding changed"
            }),
            "{error:?}"
        );
    }
}
