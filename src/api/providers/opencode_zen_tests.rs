// Repository-owned formal-driver receipts, not live vendor acceptance.
async fn install_zen_test_execution(
    state: &ServerState,
    app: AppKind,
    origin: &str,
    fixed: bool,
) -> proxy::provider_ops::ProviderExecution {
    let settings = if fixed {
        json!({"modelMapping":{"mode":"single","upstreamModel":"chat-free"},"testModel":"wrong-global-model"})
    } else {
        json!({})
    };
    let stored = state
        .upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app,
            provider: Provider {
                id: format!("zen-{}", app.as_str()),
                name: "Zen test".into(),
                settings_config: settings,
                category: None,
                meta: None,
                extra: Default::default(),
            },
            profile_id: Some(
                crate::domain::providers::registry::ProfileId::parse(format!(
                    "{}.opencode_zen",
                    app.as_str()
                ))
                .unwrap(),
            ),
            custom_binding: None,
            expected_revision: None,
            client_request_id: None,
            credential_patches: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    let store = state.providers_snapshot().await;
    let mut execution = proxy::provider_ops::ProviderExecution::from_store(&store, stored).unwrap();
    std::sync::Arc::make_mut(&mut execution.plan)
        .driver_options
        .insert("testZenOrigin".into(), json!(origin));
    execution
}

#[test]
fn opencode_zen_formal_test_uses_fixed_policy_and_complete_json_and_sse() {
    // Match the debug server's worker stack, independently of the larger stack
    // the full libtest fixture suite requests via RUST_MIN_STACK.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(8 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::spawn(Box::pin(zen_formal_probe_regression()))
            .await
            .unwrap();
    });
}

async fn zen_formal_probe_regression() {
    use crate::proxy::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    zen::store_catalog(
        vec!["chat-free".into()],
        chrono::Utc::now().timestamp_millis(),
    );
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let mode = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    let upstream_mode = mode.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |headers:HeaderMap, body:Bytes| {
        count.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        let mode = upstream_mode.load(std::sync::atomic::Ordering::SeqCst);
        async move {
            assert_eq!(headers["authorization"], "Bearer public");
            let sent:Value=serde_json::from_slice(&body).unwrap();
            assert_eq!(sent["model"],"chat-free");
            assert_eq!(sent["max_completion_tokens"],256);
            if sent["stream"] == true {
                let first = "data: {\"id\":\"c\",\"model\":\"chat-free\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"pong\"},\"finish_reason\":null}]}\n\n";
                let last = "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":256}}\n\ndata: [DONE]\n\n";
                let body = Body::from_stream(async_stream::stream! {
                    yield Ok::<Bytes,std::io::Error>(Bytes::from_static(first.as_bytes()));
                    if mode == 2 { std::future::pending::<()>().await; }
                    if mode == 0 { yield Ok(Bytes::from_static(last.as_bytes())); }
                });
                Response::builder().header("content-type","text/event-stream").body(body).unwrap()
            } else {
                let payload=if mode == 0 {json!({"id":"c","model":"chat-free","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1}})} else {json!({"id":"c","model":"chat-free","choices":[{"message":null,"finish_reason":"stop"}]})};
                Response::builder().header("content-type","application/json").body(Body::from(payload.to_string())).unwrap()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = provider_api_test_state("zen-formal");
    for app in [AppKind::Claude, AppKind::Codex] {
        let execution = install_zen_test_execution(&state, app, &origin, true).await;
        assert_eq!(execution.plan.test_model.as_deref(), Some("chat-free"));
        let mut query = TestProviderQuery {
            app,
            model: Some("wrong-requested-model".into()),
            network: Some(false),
            timeout_ms: Some(2000),
            test_prompt: Some("ping".into()),
            stream: Some(false),
        };
        let before = calls.load(std::sync::atomic::Ordering::SeqCst);
        let dry = test_provider_inner(&state, execution.clone(), &query)
            .await
            .unwrap();
        assert!(dry.ok);
        assert!(!dry.network_checked);
        assert_eq!(dry.model, "chat-free");
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), before);
        query.network = Some(true);
        for stream in [false, true] {
            query.stream = Some(stream);
            let result = test_provider_inner(&state, execution.clone(), &query)
                .await
                .unwrap();
            assert!(result.ok, "{app:?}/{stream}: {}", result.message);
            assert!(result.network_checked);
            assert_eq!(result.model, "chat-free");
            assert_eq!(result.network_stream_completed, stream.then_some(true));
        }
        mode.store(1, std::sync::atomic::Ordering::SeqCst);
        for stream in [false, true] {
            query.stream = Some(stream);
            let result = test_provider_inner(&state, execution.clone(), &query)
                .await
                .unwrap();
            assert!(!result.ok, "malformed probe accepted");
        }
        mode.store(2, std::sync::atomic::Ordering::SeqCst);
        query.stream = Some(true);
        query.timeout_ms = Some(80);
        let result = test_provider_inner(&state, execution, &query)
            .await
            .unwrap();
        assert!(!result.ok);
        assert_eq!(result.outcome, ProviderOperationOutcome::Timeout);
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let usage = state.usage_snapshot().await;
                if usage
                    .logs
                    .iter()
                    .all(|log| log.stream_status.as_deref() != Some("pending"))
                {
                    assert!(usage
                        .logs
                        .iter()
                        .any(|log| { log.stream_status.as_deref() == Some("client_cancelled") }));
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("probe timeout must abort body draining and settle cancellation");
        mode.store(0, std::sync::atomic::Ordering::SeqCst);
    }
    server.abort();
    zen::clear_catalog();
}

#[tokio::test]
async fn opencode_zen_test_without_model_is_configuration_error_without_network() {
    let state = provider_api_test_state("zen-no-test-model");
    let execution =
        install_zen_test_execution(&state, AppKind::Codex, "http://127.0.0.1:9", false).await;
    assert!(execution.plan.test_model.is_none());
    let result = test_provider_inner(
        &state,
        execution,
        &TestProviderQuery {
            app: AppKind::Codex,
            model: None,
            network: Some(true),
            timeout_ms: Some(10),
            test_prompt: None,
            stream: Some(true),
        },
    )
    .await
    .unwrap();
    assert!(!result.ok);
    assert!(!result.network_checked);
    assert_eq!(result.outcome, ProviderOperationOutcome::InvalidConfig);
}
