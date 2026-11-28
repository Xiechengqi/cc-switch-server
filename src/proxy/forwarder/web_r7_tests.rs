// Full native Web Session paths against a synthetic loopback upstream.
// The parent is isolated from concurrent tests; no real Cookie is used.
fn web_r7_body(route: ProxyRoute, stream: bool, text: &str) -> Bytes {
    let value = if route == ProxyRoute::CodexResponses {
        json!({"model":"client-alias","stream":stream,"input":text})
    } else {
        json!({"model":"client-alias","stream":stream,"messages":[{"role":"user","content":text}]})
    };
    Bytes::from(value.to_string())
}

fn web_r7_routes() -> [(AppKind, ProxyRoute, bool); 6] {
    [
        (AppKind::Claude, ProxyRoute::ClaudeMessages, false),
        (AppKind::Claude, ProxyRoute::ClaudeMessages, true),
        (AppKind::Codex, ProxyRoute::CodexChatCompletions, false),
        (AppKind::Codex, ProxyRoute::CodexChatCompletions, true),
        (AppKind::Codex, ProxyRoute::CodexResponses, false),
        (AppKind::Codex, ProxyRoute::CodexResponses, true),
    ]
}

async fn web_r7_origin(
    text: String,
    malformed: bool,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let seen = Arc::new(AtomicUsize::new(0));
    let observed = seen.clone();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let (text, observed) = (text.clone(), observed.clone());
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            let (content_type, wire) = if uri.path().contains("app-chat") {
                ("application/json", if malformed { "{broken}\n".into() } else {
                    format!("{}\n", json!({"result":{"response":{"responseId":"r7-grok","modelResponse":{"message":text}}}}))
                })
            } else {
                ("text/event-stream", if malformed { "data: {broken}\n\n".into() } else {
                    format!("data: {}\n\nevent: end_of_stream\n\n", json!({"status":"COMPLETED","backend_uuid":"r7-pplx","blocks":[{"intended_usage":"ask_text","markdown_block":{"chunks":[text]}}]}))
                })
            };
            ([(CONTENT_TYPE, content_type)], wire).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), seen, server)
}

async fn web_r7_install(
    state: &ServerState,
    app: AppKind,
    driver: &str,
    origin: &str,
) -> ProviderExecution {
    let path = if driver == "grok" {
        "/rest/app-chat/conversations/new"
    } else {
        "/rest/sse/perplexity_ask"
    };
    install_web_session_forwarder_test_provider(state, app, driver, format!("{origin}{path}"), 1)
        .await
}

async fn web_r7_forward(
    state: ServerState,
    execution: ProviderExecution,
    route: ProxyRoute,
    body: Bytes,
    budget: RequestMemoryBudget,
) -> Result<Response, ProxyError> {
    let attempt = ForwardAttemptContext {
        execution: Some(execution),
        provider_binding_pinned: true,
        request_memory: Some(budget),
        ..Default::default()
    };
    Box::pin(forward_with_attempt(
        state,
        route,
        None,
        HeaderMap::new(),
        body,
        attempt,
    ))
    .await
}

#[tokio::test]
async fn r7_web_session_source_working_set_rejects_before_network_on_every_surface() {
    let (origin, seen, server) = web_r7_origin("ok".into(), false).await;
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let body = web_r7_body(route, stream, "body fits, parser does not");
            let state = crate::state::ServerStateInner::configure_test_state(
                forwarder_test_state("web-r7-source"),
                |s| s.request_body_limits.memory_budget_bytes = body.len() + 1,
            );
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            let result = Box::pin(forward_for_test_surface(
                state.clone(),
                route,
                execution.stored.provider.id.clone(),
                None,
                HeaderMap::new(),
                body,
            ))
            .await;
            assert!(
                matches!(result, Err(ref e) if e.is_request_memory_exhausted()),
                "{driver}/{route:?}/{stream}: {result:?}"
            );
            assert_eq!(seen.load(Ordering::SeqCst), 0);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_response_working_set_rejects_once_without_auth_invalidation() {
    let (origin, seen, server) = web_r7_origin("r".repeat(50_000), false).await;
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state("web-r7-response");
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(24_000));
            let budget = RequestMemoryBudget::with_parent(24_000, parent.clone());
            let before = seen.load(Ordering::SeqCst);
            let result = web_r7_forward(
                state.clone(),
                execution.clone(),
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            )
            .await;
            assert!(
                matches!(result, Err(ref e) if e.is_request_memory_exhausted()),
                "{driver}/{route:?}/{stream}: {result:?}"
            );
            assert_eq!(seen.load(Ordering::SeqCst), before + 1);
            assert_eq!(budget.snapshot().used_bytes, 0);
            assert_eq!(parent.snapshot().used_bytes, 0);
            let recovered = web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                RequestMemoryBudget::new(16 * 1024 * 1024),
            )
            .await
            .unwrap();
            assert_eq!(recovered.status(), StatusCode::OK);
            drop(recovered);
            assert_eq!(
                seen.load(Ordering::SeqCst),
                before + 2,
                "capacity must not poison Cookie authentication or cause retries"
            );
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_output_bytes_keep_charge_until_last_clone_and_slice_drop() {
    let (origin, seen, server) =
        web_r7_origin("quote \" slash \\ newline\n".repeat(100), false).await;
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state("web-r7-output");
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            let parent =
                crate::infra::resident_memory::ResidentMemoryBudget::new(Some(2 * 1024 * 1024));
            let budget = RequestMemoryBudget::with_parent(2 * 1024 * 1024, parent.clone());
            let before = seen.load(Ordering::SeqCst);
            let response = web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            )
            .await
            .unwrap();
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert!(
                budget.snapshot().used_bytes >= body.len(),
                "output released its charge before downstream body: {:?}",
                budget.snapshot()
            );
            let charged = budget.snapshot().used_bytes;
            assert_eq!(parent.snapshot().used_bytes, charged);
            let clone = body.clone();
            let slice = body.slice(1..3);
            drop(body);
            drop(clone);
            assert_eq!(budget.snapshot().used_bytes, charged);
            drop(slice);
            assert_eq!(budget.snapshot().used_bytes, 0);
            assert_eq!(parent.snapshot().used_bytes, 0);
            assert_eq!(seen.load(Ordering::SeqCst), before + 1);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_share_and_pinned_compressed_input_are_budgeted_before_decode() {
    let (origin, seen, server) = web_r7_origin("ok".into(), false).await;
    for driver in ["grok", "perplexity"] {
        for app in [AppKind::Claude, AppKind::Codex] {
            let route = if app == AppKind::Claude {
                ProxyRoute::ClaudeMessages
            } else {
                ProxyRoute::CodexResponses
            };
            let body = Bytes::from(gzip_bytes(&web_r7_body(
                route,
                false,
                &"large decoded text ".repeat(2000),
            )));
            let state = crate::state::ServerStateInner::configure_test_state(
                forwarder_test_state("web-r7-decode"),
                |s| s.request_body_limits.memory_budget_bytes = body.len() + 1,
            );
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            install_antigravity_test_share(
                &state,
                "web-r7-share",
                app,
                execution.stored.provider_type,
                &execution.stored.provider.id,
            )
            .await;
            let mut headers = HeaderMap::new();
            headers.insert("content-encoding", HeaderValue::from_static("gzip"));
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let error = Box::pin(forward_for_test_surface(
                state.clone(),
                route,
                execution.stored.provider.id.clone(),
                None,
                headers.clone(),
                body.clone(),
            ))
            .await
            .unwrap_err();
            assert!(
                error.is_request_memory_exhausted(),
                "{driver}/{app:?} pinned: {error:?}"
            );
            headers.insert(
                "x-cc-switch-share-id",
                HeaderValue::from_static("web-r7-share"),
            );
            headers.insert(
                "x-cc-switch-user-email",
                HeaderValue::from_static("owner@example.com"),
            );
            let error = Box::pin(forward(state.clone(), route, None, headers, body))
                .await
                .unwrap_err();
            assert!(
                error.is_request_memory_exhausted(),
                "{driver}/{app:?} share: {error:?}"
            );
            assert_eq!(seen.load(Ordering::SeqCst), 0);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_shared_parent_keeps_first_output_and_sheds_second_request() {
    let (origin, seen, server) = web_r7_origin("parent-owned output".repeat(100), false).await;
    for driver in ["grok", "perplexity"] {
        let state = forwarder_test_state("web-r7-parent");
        let config_dir = state.config_dir.clone();
        let execution = web_r7_install(&state, AppKind::Claude, driver, &origin).await;
        let parent =
            crate::infra::resident_memory::ResidentMemoryBudget::new(Some(2 * 1024 * 1024));
        let first = RequestMemoryBudget::with_parent(2 * 1024 * 1024, parent.clone());
        let second = RequestMemoryBudget::with_parent(2 * 1024 * 1024, parent.clone());
        let route = ProxyRoute::ClaudeMessages;
        let before = seen.load(Ordering::SeqCst);
        let response = web_r7_forward(
            state.clone(),
            execution.clone(),
            route,
            web_r7_body(route, true, "hi"),
            first.clone(),
        )
        .await
        .unwrap();
        let output = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let charge = parent.snapshot().used_bytes;
        assert!(charge >= output.len());
        parent.configure_limit(Some(charge + 200));
        let error = web_r7_forward(
            state.clone(),
            execution.clone(),
            route,
            web_r7_body(route, true, "hi"),
            second.clone(),
        )
        .await
        .unwrap_err();
        assert!(error.is_request_memory_exhausted());
        assert_eq!(second.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().used_bytes, charge);
        assert_eq!(seen.load(Ordering::SeqCst), before + 1);
        drop(output);
        assert_eq!(parent.snapshot().used_bytes, 0);
        parent.configure_limit(Some(2 * 1024 * 1024));
        let response = web_r7_forward(
            state.clone(),
            execution,
            route,
            web_r7_body(route, false, "hi"),
            RequestMemoryBudget::with_parent(2 * 1024 * 1024, parent.clone()),
        )
        .await
        .unwrap();
        drop(response);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(seen.load(Ordering::SeqCst), before + 2);
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_malformed_response_releases_all_request_owners_without_retry() {
    let (origin, seen, server) = web_r7_origin("unused".into(), true).await;
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state("web-r7-malformed");
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            let parent =
                crate::infra::resident_memory::ResidentMemoryBudget::new(Some(1024 * 1024));
            let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
            let before = seen.load(Ordering::SeqCst);
            let error = web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_GATEWAY);
            assert!(!budget.is_exhausted());
            assert_eq!(seen.load(Ordering::SeqCst), before + 1);
            assert_eq!(budget.snapshot().used_bytes, 0);
            assert_eq!(parent.snapshot().used_bytes, 0);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_cancelled_buffer_releases_parent_and_never_replays() {
    let seen = Arc::new(AtomicUsize::new(0));
    let observed = seen.clone();
    let (partial_tx, mut partial_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri| {
        let (seen, partial_tx) = (observed.clone(), partial_tx.clone());
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            let (content_type, wire) = if uri.path().contains("app-chat") {
                (
                    "application/json",
                    format!(
                        "{}\n",
                        json!({"result":{"response":{"token":"p".repeat(4096)}}})
                    ),
                )
            } else {
                (
                    "text/event-stream",
                    format!(
                        "data: {}\n\n",
                        json!({"blocks":[{"markdown_block":{"answer":"p".repeat(4096)}}]})
                    ),
                )
            };
            let stream = futures_util::stream::once(async move {
                let _ = partial_tx.send(());
                Ok::<_, std::convert::Infallible>(Bytes::from(wire))
            })
            .chain(futures_util::stream::pending());
            Response::builder()
                .header(CONTENT_TYPE, content_type)
                .body(Body::from_stream(stream))
                .unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state("web-r7-cancel");
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &format!("http://{address}")).await;
            let parent =
                crate::infra::resident_memory::ResidentMemoryBudget::new(Some(1024 * 1024));
            let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
            let request = tokio::spawn(web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            ));
            tokio::time::timeout(Duration::from_secs(2), partial_rx.recv())
                .await
                .unwrap()
                .unwrap();
            // Tiny canonical/wire requests are below 4 KiB. The larger live
            // charge proves the client has retained the partial response.
            tokio::time::timeout(Duration::from_secs(2), async {
                while budget.snapshot().used_bytes < 4096 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(parent.snapshot().used_bytes > 0);
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            assert_eq!(budget.snapshot().used_bytes, 0);
            assert_eq!(parent.snapshot().used_bytes, 0);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    assert_eq!(seen.load(Ordering::SeqCst), 12);
    server.abort();
}

#[tokio::test]
async fn r7_web_session_translated_request_budget_and_local_count_tokens() {
    let (origin, seen, server) = web_r7_origin("ok".into(), false).await;
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state("web-r7-translation");
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(12_000));
            let budget = RequestMemoryBudget::with_parent(12_000, parent.clone());
            let error = web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            )
            .await
            .unwrap_err();
            assert!(error.is_request_memory_exhausted());
            assert_eq!(seen.load(Ordering::SeqCst), 0);
            assert_eq!(parent.snapshot().used_bytes, 0);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
        let state = forwarder_test_state("web-r7-count");
        let config_dir = state.config_dir.clone();
        let execution = web_r7_install(&state, AppKind::Claude, driver, &origin).await;
        let budget = RequestMemoryBudget::new(64_000);
        let response = web_r7_forward(
            state.clone(),
            execution,
            ProxyRoute::ClaudeCountTokens,
            web_r7_body(ProxyRoute::ClaudeCountTokens, false, "hello"),
            budget.clone(),
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert!(budget.snapshot().used_bytes >= body.len());
        assert!(
            serde_json::from_slice::<Value>(&body).unwrap()["input_tokens"]
                .as_u64()
                .unwrap()
                > 0
        );
        drop(body);
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(seen.load(Ordering::SeqCst), 0);
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_output_working_set_rejects_after_validated_terminal_once() {
    let (origin, seen, server) = web_r7_origin("x".repeat(12_000), false).await;
    for driver in ["grok", "perplexity"] {
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state("web-r7-output-capacity");
            let config_dir = state.config_dir.clone();
            let execution = web_r7_install(&state, app, driver, &origin).await;
            let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(185_000));
            let budget = RequestMemoryBudget::with_parent(185_000, parent.clone());
            let before = seen.load(Ordering::SeqCst);
            let error = web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            )
            .await
            .unwrap_err();
            assert!(
                error.is_request_memory_exhausted(),
                "{driver}/{route:?}/{stream}: {error:?}"
            );
            let timing = budget.timing().unwrap().attempt_snapshot();
            assert!(
                timing["milestonesMs"]["first_semantic"].is_number(),
                "terminal was not parsed before output allocation: {timing}"
            );
            assert_eq!(seen.load(Ordering::SeqCst), before + 1);
            assert_eq!(budget.snapshot().used_bytes, 0);
            assert_eq!(parent.snapshot().used_bytes, 0);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_web_session_perplexity_sparse_patch_and_nested_json_are_admitted_before_expansion() {
    // A valid FINAL blob with a second encoded JSON object. The extra array
    // is ignored semantically, but its parser allocations must be admitted.
    let inner = json!({"answer":"ok","ignored":vec![0;10_000]}).to_string();
    let nested = json!([{"step_type":"FINAL","answer":inner}]).to_string();
    for (label, event, limit) in [
        (
            "sparse",
            json!({"status":"COMPLETED","blocks":[{"intended_usage":"ask_text","diff_block":{"field":"markdown_block","patches":[{"op":"replace","path":"/chunks/4096","value":"ok"}]}}]}),
            100_000,
        ),
        (
            "nested",
            json!({"status":"COMPLETED","text":nested}),
            512_000,
        ),
    ] {
        let seen = Arc::new(AtomicUsize::new(0));
        let observed = seen.clone();
        let wire = format!("data: {event}\n\nevent: end_of_stream\n\n");
        let app = axum::Router::new().fallback(move || {
            let (wire, seen) = (wire.clone(), observed.clone());
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                ([(CONTENT_TYPE, "text/event-stream")], wire).into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        for (app, route, stream) in web_r7_routes() {
            let state = forwarder_test_state(&format!("web-r7-{label}"));
            let config_dir = state.config_dir.clone();
            let execution =
                web_r7_install(&state, app, "perplexity", &format!("http://{address}")).await;
            let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(limit));
            let budget = RequestMemoryBudget::with_parent(limit, parent.clone());
            let before = seen.load(Ordering::SeqCst);
            let error = web_r7_forward(
                state.clone(),
                execution.clone(),
                route,
                web_r7_body(route, stream, "hi"),
                budget.clone(),
            )
            .await
            .unwrap_err();
            assert!(
                error.is_request_memory_exhausted(),
                "{label}/{route:?}/{stream}: {error:?}"
            );
            assert_eq!(seen.load(Ordering::SeqCst), before + 1);
            assert_eq!(parent.snapshot().used_bytes, 0);
            // The same strict protocol is valid with sufficient local space;
            // capacity rejection must not be confused with malformed data.
            let response = web_r7_forward(
                state.clone(),
                execution,
                route,
                web_r7_body(route, stream, "hi"),
                RequestMemoryBudget::new(16 * 1024 * 1024),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            drop(response);
            assert_eq!(seen.load(Ordering::SeqCst), before + 2);
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
        server.abort();
    }
}
