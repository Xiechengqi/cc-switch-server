async fn install_ollama_r5_provider(state: &ServerState, app: AppKind, endpoint: &str) -> String {
    let mut stored = stored_provider(
        app,
        ProviderType::OllamaCloud,
        json!({"modelMapping":{"mode":"single","upstreamModel":"gpt-oss:20b"},"env": if app == AppKind::Claude {
            json!({"ANTHROPIC_AUTH_TOKEN":"ollama-synthetic-key","ANTHROPIC_BASE_URL":"https://ollama.com"})
        } else {
            json!({"OPENAI_API_KEY":"ollama-synthetic-key","OPENAI_BASE_URL":"https://ollama.com"})
        }}),
        None,
    );
    let id = stored.provider.id.clone();
    stored.resource.profile_id = Some(
        crate::domain::providers::registry::ProfileId::parse(format!(
            "{}.ollama_cloud",
            app.as_str()
        ))
        .unwrap(),
    );
    stored.resource.profile_schema_revision = Some(1);
    let mut providers = ProviderStore {
        providers: vec![stored],
        ..ProviderStore::default()
    };
    providers
        .rebuild_runtime_index(&state.accounts_snapshot().await)
        .unwrap();
    let mut plan = providers.runtime_plan(app, &id).unwrap().as_ref().clone();
    plan.endpoint = endpoint.to_string();
    Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
    state.replace_provider_store_for_test(providers).await;
    id
}

fn ollama_r5_request(route: ProxyRoute, effort: Value) -> Value {
    match route {
        ProxyRoute::CodexResponses => {
            json!({"model":"gpt-oss:20b","input":"hello","reasoning":{"effort":effort},"stream":false})
        }
        ProxyRoute::CodexChatCompletions => {
            json!({"model":"gpt-oss:20b","messages":[{"role":"user","content":"hello"}],"reasoning_effort":effort,"stream":false})
        }
        _ => {
            json!({"model":"gpt-oss:20b","max_tokens":16,"messages":[{"role":"user","content":"hello"}],"output_config":{"effort":effort},"stream":false})
        }
    }
}

type OllamaR5ObservedRequests = Arc<std::sync::Mutex<Vec<(String, HeaderMap, Value)>>>;

async fn ollama_r5_probe() -> (
    String,
    OllamaR5ObservedRequests,
    tokio::task::JoinHandle<()>,
) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = seen.clone();
    let app = axum::Router::new().fallback(move |uri: axum::http::Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
        let observed = observed.clone();
        async move {
            observed.lock().unwrap().push((uri.path().to_string(), headers, body));
            axum::Json(json!({"id":"ollama_fixture","object":"chat.completion","model":"gpt-oss:20b","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}))
        }
    });
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), seen, server)
}

#[tokio::test]
async fn ollama_r5_responses_ultra_is_normalized_in_actual_forwarder() {
    let (endpoint, seen, server) = ollama_r5_probe().await;
    let state = forwarder_test_state("ollama-r5-ultra");
    let id = install_ollama_r5_provider(&state, AppKind::Codex, &endpoint).await;
    let response = forward_for_test_surface(
        state,
        ProxyRoute::CodexResponses,
        id,
        None,
        HeaderMap::new(),
        Bytes::from(ollama_r5_request(ProxyRoute::CodexResponses, json!("ultra")).to_string()),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    collect_response_body(response).await;
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "/v1/chat/completions");
    assert_eq!(seen[0].1["authorization"], "Bearer ollama-synthetic-key");
    assert_eq!(seen[0].2["reasoning_effort"], "max");
    assert_eq!(seen[0].2["model"], "gpt-oss:20b");
    server.abort();
}

#[tokio::test]
async fn ollama_r5_unknown_effort_is_rejected_before_any_actual_upstream_send() {
    for route in [
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::ClaudeMessages,
    ] {
        let (endpoint, seen, server) = ollama_r5_probe().await;
        let state = forwarder_test_state(&format!("ollama-r5-invalid-{route:?}"));
        let id = install_ollama_r5_provider(&state, route.app(), &endpoint).await;
        let result = forward_for_test_surface(
            state,
            route,
            id,
            None,
            HeaderMap::new(),
            Bytes::from(ollama_r5_request(route, json!("extreme")).to_string()),
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(response) => {
                collect_response_body(response).await;
                panic!(
                    "{route:?}: invalid effort reached upstream; sends={}",
                    seen.lock().unwrap().len()
                );
            }
        };
        assert_eq!(
            error.status,
            StatusCode::BAD_REQUEST,
            "{route:?}: {error:?}"
        );
        assert!(error.client_message().contains("Ollama"), "{error:?}");
        assert!(seen.lock().unwrap().is_empty());
        server.abort();
    }
}

#[tokio::test]
async fn ollama_r5_all_aliases_reach_the_exact_model_on_all_declared_routes() {
    let evidence: Value = serde_json::from_str(include_str!(
        "../../../assets/contract/r5-ollama-cloud.json"
    ))
    .unwrap();
    let aliases = evidence["cases"][0]["inputToCanonical"]
        .as_object()
        .unwrap();
    for route in [
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::ClaudeMessages,
    ] {
        let (endpoint, seen, server) = ollama_r5_probe().await;
        let state = forwarder_test_state(&format!("ollama-r5-aliases-{route:?}"));
        let id = install_ollama_r5_provider(&state, route.app(), &endpoint).await;
        for (alias, expected) in aliases {
            let request =
                ollama_r5_request(route, json!(format!(" {} ", alias.to_ascii_uppercase())));
            let response = forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                None,
                HeaderMap::new(),
                Bytes::from(request.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{route:?} {alias}");
            collect_response_body(response).await;
            let seen = seen.lock().unwrap();
            let (_, headers, body) = seen.last().unwrap();
            assert_eq!(headers["authorization"], "Bearer ollama-synthetic-key");
            assert_eq!(
                &body["reasoning_effort"], expected,
                "{route:?} {alias}: {body}"
            );
            assert_eq!(body["model"], "gpt-oss:20b");
            assert!(body.get("reasoning").is_none());
        }
        assert_eq!(seen.lock().unwrap().len(), aliases.len());
        assert!(state.accounts_snapshot().await.accounts.is_empty());
        server.abort();
    }
}

#[tokio::test]
async fn ollama_r5_invalid_types_containers_and_conflicts_have_zero_send_or_health_penalty() {
    for route in [
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::ClaudeMessages,
    ] {
        let (endpoint, seen, server) = ollama_r5_probe().await;
        let state = forwarder_test_state(&format!("ollama-r5-types-{route:?}"));
        let id = install_ollama_r5_provider(&state, route.app(), &endpoint).await;
        let mut requests: Vec<Value> = [Value::Null, json!(true), json!(12), json!({}), json!([])]
            .into_iter()
            .map(|effort| ollama_r5_request(route, effort))
            .collect();
        let mut conflict = ollama_r5_request(route, json!("low"));
        if route == ProxyRoute::ClaudeMessages {
            conflict["thinking"] = json!({"effort":"high"});
        } else if route == ProxyRoute::CodexResponses {
            conflict["reasoning_effort"] = json!("high");
        } else {
            conflict["reasoning"] = json!({"effort":"high"});
        }
        requests.push(conflict);
        for value in [Value::Null, json!(false), json!(3), json!([])] {
            let mut request = ollama_r5_request(route, json!("low"));
            request[if route == ProxyRoute::ClaudeMessages {
                "output_config"
            } else {
                "reasoning"
            }] = value;
            requests.push(request);
        }
        for request in requests {
            let error = forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                None,
                HeaderMap::new(),
                Bytes::from(request.to_string()),
            )
            .await
            .unwrap_err();
            assert_eq!(
                error.status,
                StatusCode::BAD_REQUEST,
                "{route:?}: {error:?}"
            );
            assert!(error.client_message().contains("Ollama"), "{error:?}");
        }
        assert!(seen.lock().unwrap().is_empty());
        assert!(state
            .usage_snapshot()
            .await
            .provider_health
            .get(route.app(), &id)
            .is_none_or(|health| health.consecutive_failures == 0));
        server.abort();
    }
}

#[tokio::test]
async fn ollama_r5_missing_empty_disabled_and_equivalent_controls_preserve_intent() {
    for route in [
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::ClaudeMessages,
    ] {
        let (endpoint, seen, server) = ollama_r5_probe().await;
        let state = forwarder_test_state(&format!("ollama-r5-default-{route:?}"));
        let id = install_ollama_r5_provider(&state, route.app(), &endpoint).await;
        let empty = ollama_r5_request(route, json!("  "));
        let mut missing = empty.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove(if route == ProxyRoute::ClaudeMessages {
                "output_config"
            } else if route == ProxyRoute::CodexResponses {
                "reasoning"
            } else {
                "reasoning_effort"
            });
        let mut equivalent = ollama_r5_request(route, json!("xhigh"));
        if route == ProxyRoute::ClaudeMessages {
            equivalent["thinking"] = json!({"effort":"ultra"});
        } else if route == ProxyRoute::CodexResponses {
            equivalent["reasoning_effort"] = json!("ultra");
        } else {
            equivalent["reasoning"] = json!({"effort":"ultra","summary":"auto"});
        }
        let mut cases = vec![(missing, None), (empty, None), (equivalent, Some("max"))];
        if route == ProxyRoute::ClaudeMessages {
            let mut empty_thinking = ollama_r5_request(route, json!("  "));
            empty_thinking["thinking"] = json!({"effort":"  "});
            cases.push((empty_thinking, None));
            let mut disabled = ollama_r5_request(route, json!("none"));
            disabled.as_object_mut().unwrap().remove("output_config");
            disabled["thinking"] = json!({"type":"disabled"});
            cases.push((disabled, Some("none")));
        }
        for (request, expected) in cases {
            let response = forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                None,
                HeaderMap::new(),
                Bytes::from(request.to_string()),
            )
            .await
            .unwrap();
            collect_response_body(response).await;
            let seen = seen.lock().unwrap();
            let body = &seen.last().unwrap().2;
            assert_eq!(
                body.get("reasoning_effort").and_then(Value::as_str),
                expected,
                "{route:?}: {body}"
            );
            if let Some(nested) = body.pointer("/reasoning/effort") {
                assert_eq!(nested.as_str(), expected);
                assert_eq!(body.pointer("/reasoning/summary"), Some(&json!("auto")));
            }
        }
        server.abort();
    }
}
async fn ollama_r5_terminal_probe(
    finish: &'static str,
    truncated: bool,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = axum::Router::new().fallback(move |axum::Json(body): axum::Json<Value>| {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            let message = if finish == "tool_calls" {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"call_fixture","type":"function","function":{"name":"lookup","arguments":"{}"}}]})
            } else { json!({"role":"assistant","content":"hello"}) };
            let usage = json!({"prompt_tokens":7,"completion_tokens":3,"total_tokens":10,"prompt_tokens_details":{"cached_tokens":2}});
            if body["stream"] == true {
                let delta = if finish == "tool_calls" {
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":"call_fixture","type":"function","function":{"name":"lookup","arguments":"{}"}}]})
                } else { json!({"role":"assistant","content":"hello"}) };
                let mut wire = format!("data: {}\n\n", json!({"id":"ollama_fixture","object":"chat.completion.chunk","model":"gpt-oss:20b","choices":[{"index":0,"delta":delta,"finish_reason":null}]}));
                if !truncated {
                    for frame in [json!({"id":"ollama_fixture","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":finish}]}), json!({"choices":[],"usage":usage})] {
                        wire.push_str(&format!("data: {frame}\n\n"));
                    }
                    wire.push_str("data: [DONE]\n\n");
                }
                Response::builder().header(CONTENT_TYPE, "text/event-stream").body(Body::from(wire)).unwrap()
            } else {
                axum::Json(json!({"id":"ollama_fixture","object":"chat.completion","model":"gpt-oss:20b","choices":[{"index":0,"message":message,"finish_reason":finish}],"usage":usage})).into_response()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), calls, server)
}

#[tokio::test]
async fn ollama_r5_tool_and_length_terminals_preserve_official_usage_on_all_routes() {
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
    ] {
        for stream in [false, true] {
            for finish in ["tool_calls", "length"] {
                let (endpoint, calls, server) = ollama_r5_terminal_probe(finish, false).await;
                let state = forwarder_test_state(&format!(
                    "ollama-r5-terminal-{route:?}-{stream}-{finish}"
                ));
                let id = install_ollama_r5_provider(&state, route.app(), &endpoint).await;
                let mut request = ollama_r5_request(route, json!("minimal"));
                request["stream"] = json!(stream);
                request["tools"] = match route {
                    ProxyRoute::ClaudeMessages => {
                        json!([{"name":"lookup","input_schema":{"type":"object"}}])
                    }
                    ProxyRoute::CodexResponses => {
                        json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}])
                    }
                    _ => {
                        json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}])
                    }
                };
                let (response, timing) = r5_timing_forward(
                    state.clone(),
                    route,
                    id,
                    None,
                    HeaderMap::new(),
                    Bytes::from(request.to_string()),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = collect_response_body(response).await;
                let text = String::from_utf8(bytes.to_vec()).unwrap();
                assert!(
                    !text.contains("event: error"),
                    "{route:?} {finish} {stream}: {text}"
                );
                match route {
                    ProxyRoute::ClaudeMessages => assert!(
                        text.contains(if finish == "tool_calls" {
                            "\"stop_reason\":\"tool_use\""
                        } else {
                            "\"stop_reason\":\"max_tokens\""
                        }),
                        "{text}"
                    ),
                    ProxyRoute::CodexResponses => {
                        if stream {
                            assert!(
                                text.contains(if finish == "tool_calls" {
                                    "response.completed"
                                } else {
                                    "response.incomplete"
                                }),
                                "{text}"
                            );
                        } else {
                            let value: Value = serde_json::from_slice(&bytes).unwrap();
                            assert_eq!(
                                value["status"],
                                if finish == "tool_calls" {
                                    "completed"
                                } else {
                                    "incomplete"
                                }
                            );
                        }
                    }
                    _ => assert!(
                        text.contains(&format!("\"finish_reason\":\"{finish}\"")),
                        "{text}"
                    ),
                }
                if finish == "tool_calls" {
                    assert!(
                        text.contains("call_fixture") && text.contains("lookup"),
                        "{text}"
                    );
                }
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().unwrap();
                assert_eq!(log.raw_input_tokens, Some(7), "{route:?} {stream}: {log:?}");
                assert_eq!(log.input_tokens, Some(5));
                assert_eq!(log.output_tokens, Some(3));
                assert_eq!(log.cache_read_tokens, Some(2));
                assert_eq!(log.total_tokens, Some(10));
                assert!(!log.usage_estimated);
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                drop(bytes);
                r5_timing_assert_delivery(
                    &timing,
                    "ollama_cloud",
                    if finish == "length" {
                        "incomplete"
                    } else {
                        "complete"
                    },
                )
                .await;
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn ollama_r5_model_unsupported_is_returned_once_without_effort_or_model_downgrade() {
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = axum::Router::new().fallback(move |axum::Json(body): axum::Json<Value>| {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, Ordering::SeqCst);
                assert_eq!(body["model"], "gpt-oss:20b");
                assert_eq!(body["reasoning_effort"], "max");
                (StatusCode::BAD_REQUEST, axum::Json(json!({"error":{"message":"fixture model does not support max","type":"invalid_request_error"}})))
            }
        });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("ollama-r5-unsupported-{route:?}"));
        let id =
            install_ollama_r5_provider(&state, route.app(), &format!("http://{address}")).await;
        let result = forward_for_test_surface(
            state,
            route,
            id,
            None,
            HeaderMap::new(),
            Bytes::from(ollama_r5_request(route, json!("ultra")).to_string()),
        )
        .await;
        match result {
            Ok(response) => {
                assert_eq!(response.status(), StatusCode::BAD_REQUEST);
                collect_response_body(response).await;
            }
            Err(error) => assert_eq!(error.status, StatusCode::BAD_REQUEST, "{error:?}"),
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn ollama_r5_truncated_streams_fail_without_replay_on_all_routes() {
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
    ] {
        let (endpoint, calls, server) = ollama_r5_terminal_probe("length", true).await;
        let state = forwarder_test_state(&format!("ollama-r5-truncated-{route:?}"));
        let id = install_ollama_r5_provider(&state, route.app(), &endpoint).await;
        let mut request = ollama_r5_request(route, json!("low"));
        request["stream"] = json!(true);
        let response = forward_for_test_surface(
            state.clone(),
            route,
            id,
            None,
            HeaderMap::new(),
            Bytes::from(request.to_string()),
        )
        .await
        .unwrap();
        let text = String::from_utf8(collect_response_body(response).await.to_vec()).unwrap();
        assert!(
            !text.contains("response.completed") && !text.contains("\"stop_reason\":\"end_turn\""),
            "{text}"
        );
        let usage = state.usage_snapshot().await;
        let log = usage.logs.last().unwrap();
        assert_ne!(
            log.stream_status.as_deref(),
            Some("completed"),
            "{route:?}: {log:?}"
        );
        assert!(log.status_code >= 400, "{log:?}");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn ollama_r5_client_cancel_drops_upstream_body_and_records_cancellation() {
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
    ] {
        let released = Arc::new(AtomicBool::new(false));
        let observed = released.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let requests = calls.clone();
        let app = axum::Router::new().fallback(move || {
            let observed = observed.clone();
            let requests = requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                struct Owner(Arc<AtomicBool>);
                impl Drop for Owner { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
                let wire = async_stream::stream! {
                    let _owner = Owner(observed);
                    yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"data: {\"id\":\"ollama_fixture\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hello\"},\"finish_reason\":null}]}\n\n"));
                    std::future::pending::<()>().await;
                };
                Response::builder().header(CONTENT_TYPE, "text/event-stream").body(Body::from_stream(wire)).unwrap()
            }
        });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("ollama-r5-cancel-{route:?}"));
        let id =
            install_ollama_r5_provider(&state, route.app(), &format!("http://{address}")).await;
        let mut request = ollama_r5_request(route, json!("low"));
        request["stream"] = json!(true);
        let response = forward_for_test_surface(
            state.clone(),
            route,
            id.clone(),
            None,
            HeaderMap::new(),
            Bytes::from(request.to_string()),
        )
        .await
        .unwrap();
        let mut body = response.into_body().into_data_stream();
        assert!(tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .is_ok());
        drop(body);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let usage = state.usage_snapshot().await;
                if released.load(Ordering::SeqCst)
                    && usage
                        .logs
                        .last()
                        .is_some_and(|log| log.stream_status.as_deref() == Some("client_cancelled"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled request and upstream owner must finish");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(state
            .usage_snapshot()
            .await
            .provider_health
            .get(route.app(), &id)
            .is_none_or(|health| health.consecutive_failures == 0));
        server.abort();
    }
}

#[tokio::test]
async fn ollama_r5_done_without_finish_or_finish_without_done_is_not_success() {
    let finish = json!({"id":"ollama_fixture","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexResponses,
        ProxyRoute::CodexChatCompletions,
    ] {
        for wire in [
            "data: [DONE]\n\n".to_string(),
            format!("data: {finish}\n\n"),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"  \"}]}\n\ndata: [DONE]\n\n"
                .to_string(),
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":7}]}\n\ndata: [DONE]\n\n"
                .to_string(),
            format!("data: {finish}\n\ndata: [DONE]\n\ndata: [DONE]\n\n"),
            format!("data: {finish}\n\ndata: [DONE]\n\ndata: {{\"choices\":[]}}\n\n"),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let app = axum::Router::new().fallback(move || {
                let observed = observed.clone();
                let wire = wire.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    Response::builder()
                        .header(CONTENT_TYPE, "text/event-stream")
                        .body(Body::from(wire))
                        .unwrap()
                }
            });
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let state = forwarder_test_state(&format!("ollama-r5-terminal-pair-{route:?}"));
            let id =
                install_ollama_r5_provider(&state, route.app(), &format!("http://{address}")).await;
            let mut request = ollama_r5_request(route, json!("low"));
            request["stream"] = json!(true);
            let response = forward_for_test_surface(
                state.clone(),
                route,
                id,
                None,
                HeaderMap::new(),
                Bytes::from(request.to_string()),
            )
            .await
            .unwrap();
            let text = String::from_utf8(collect_response_body(response).await.to_vec()).unwrap();
            assert!(
                !text.contains("event: response.completed")
                    && !text.contains("event: message_delta"),
                "{route:?}: {text}"
            );
            assert!(text.contains("cc_switch_stream_error"), "{route:?}: {text}");
            let usage = state.usage_snapshot().await;
            let log = usage.logs.last().unwrap();
            assert_ne!(
                log.stream_status.as_deref(),
                Some("completed"),
                "{route:?}: {log:?}"
            );
            assert!(log.status_code >= 400);
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }
}
