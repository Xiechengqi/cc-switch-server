#[derive(Clone)]
struct CopilotNativeProbe {
    endpoint: crate::domain::accounts::copilot::CopilotEndpoint,
    stream: bool,
    reject_first: bool,
    revoke_on_refresh: bool,
    reject_first_model: bool,
    stream_fault: Arc<std::sync::Mutex<Option<&'static str>>>,
    stream_dropped: Arc<std::sync::atomic::AtomicBool>,
    model_requests: Arc<std::sync::atomic::AtomicUsize>,
    token_requests: Arc<std::sync::atomic::AtomicUsize>,
    requests: Arc<std::sync::atomic::AtomicUsize>,
    observations: Arc<std::sync::Mutex<Vec<(String, HeaderMap, Value)>>>,
}

fn copilot_native_response(endpoint: crate::domain::accounts::copilot::CopilotEndpoint) -> Value {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    match endpoint {
        CopilotEndpoint::Messages => json!({
            "id":"msg_native", "type":"message", "role":"assistant", "model":"opaque-model",
            "content":[{"type":"thinking","thinking":"thought","signature":"opaque-signature"},
                {"type":"text","text":"answer"},{"type":"tool_use","id":"call_native","name":"Read","input":{"path":"a"}}], "stop_reason":"tool_use", "stop_sequence":null,
            "usage":{"input_tokens":4,"output_tokens":2}
        }),
        CopilotEndpoint::Responses => json!({
            "id":"resp_native", "object":"response", "status":"completed", "model":"opaque-model",
            "output":[{"id":"rs_native","type":"reasoning","summary":[],"encrypted_content":"opaque-encrypted"},
                {"id":"msg_native","type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"answer","annotations":[]}]},
                {"type":"function_call","id":"fc_native","call_id":"call_native","name":"Read","arguments":"{\"path\":\"a\"}","status":"completed"}],
            "usage":{"input_tokens":4,"output_tokens":2,"total_tokens":6}
        }),
        CopilotEndpoint::Chat => unreachable!(),
    }
}

async fn spawn_copilot_native_upstream(
    endpoint: crate::domain::accounts::copilot::CopilotEndpoint,
    stream: bool,
    reject_first: bool,
    revoke_on_refresh: bool,
    reject_first_model: bool,
) -> (
    std::net::SocketAddr,
    CopilotNativeProbe,
    tokio::task::JoinHandle<()>,
) {
    async fn models(
        axum::extract::State(probe): axum::extract::State<CopilotNativeProbe>,
    ) -> Response {
        let n = probe
            .model_requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if probe.reject_first_model && n == 0 {
            return Response::builder()
                .status(401)
                .body(Body::from("expired catalog token"))
                .unwrap();
        }
        let denied =
            probe.revoke_on_refresh && probe.requests.load(std::sync::atomic::Ordering::SeqCst) > 0;
        axum::Json(
            json!({"data":[{"id":"opaque-model", "policy":{"state":if denied {"disabled"} else {"enabled"}},
            "supported_endpoints":[probe.endpoint.path()]}]}),
        ).into_response()
    }
    async fn inference(
        axum::extract::State(probe): axum::extract::State<CopilotNativeProbe>,
        axum::extract::OriginalUri(uri): axum::extract::OriginalUri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let n = probe
            .requests
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        probe.observations.lock().unwrap().push((
            uri.path().into(),
            headers,
            serde_json::from_slice(&body).unwrap(),
        ));
        if n == 0 && probe.reject_first {
            return Response::builder()
                .status(401)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"error":{"message":"expired token"}}"#))
                .unwrap();
        }
        let output = copilot_native_response(probe.endpoint);
        if !probe.stream {
            return Response::builder()
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(output.to_string()))
                .unwrap();
        }
        use crate::domain::accounts::copilot::CopilotEndpoint;
        let mut frames: Vec<(&str, Value)> = match probe.endpoint {
            CopilotEndpoint::Responses => vec![
                (
                    "response.output_text.delta",
                    json!({"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"answer"}),
                ),
                (
                    "response.output_item.added",
                    json!({"type":"response.output_item.added","output_index":2,"item":{"id":"fc_native","type":"function_call","call_id":"call_native","name":"Read","arguments":"","status":"in_progress"}}),
                ),
                (
                    "response.function_call_arguments.delta",
                    json!({"type":"response.function_call_arguments.delta","item_id":"fc_native","output_index":2,"delta":"{\"path\":\"a\"}"}),
                ),
                (
                    "response.function_call_arguments.done",
                    json!({"type":"response.function_call_arguments.done","item_id":"fc_native","output_index":2,"arguments":"{\"path\":\"a\"}"}),
                ),
                (
                    "response.output_item.done",
                    json!({"type":"response.output_item.done","output_index":2,"item":output["output"][2]}),
                ),
                (
                    "response.completed",
                    json!({"type":"response.completed","response":output}),
                ),
            ],
            CopilotEndpoint::Messages => vec![
                (
                    "message_start",
                    json!({"type":"message_start","message":{"id":"msg_native","type":"message","role":"assistant","model":"opaque-model","content":[],"usage":{"input_tokens":4,"output_tokens":0}}}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"thought"}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"opaque-signature"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":0}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":1}),
                ),
                (
                    "content_block_start",
                    json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_native","name":"Read","input":{}}}),
                ),
                (
                    "content_block_delta",
                    json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}),
                ),
                (
                    "content_block_stop",
                    json!({"type":"content_block_stop","index":2}),
                ),
                (
                    "message_delta",
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":2}}),
                ),
                ("message_stop", json!({"type":"message_stop"})),
            ],
            CopilotEndpoint::Chat => unreachable!(),
        };
        let fault = *probe.stream_fault.lock().unwrap();
        match fault {
            Some("truncated" | "pending") => {
                frames.retain(|(event, _)| {
                    !matches!(
                        *event,
                        "message_delta" | "message_stop" | "response.completed"
                    )
                });
            }
            Some("error") => {
                frames.truncate(1);
                frames.push(("error", json!({"type":"error","error":{"type":"api_error","message":"synthetic native failure"}})));
            }
            _ => {}
        }
        let chunks = async_stream::stream! {
            struct Dropped(Arc<std::sync::atomic::AtomicBool>);
            impl Drop for Dropped {
                fn drop(&mut self) {
                    self.0.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _guard = Dropped(probe.stream_dropped);
            for (event, data) in frames {
                yield Ok::<_, std::convert::Infallible>(Bytes::from(format!(
                    "event: {event}\ndata: {data}\n\n"
                )));
            }
            if fault == Some("pending") {
                std::future::pending::<()>().await;
            }
        };
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap()
    }
    let probe = CopilotNativeProbe {
        endpoint,
        stream,
        reject_first,
        revoke_on_refresh,
        reject_first_model,
        stream_fault: Default::default(),
        stream_dropped: Default::default(),
        model_requests: Default::default(),
        token_requests: Default::default(),
        requests: Default::default(),
        observations: Default::default(),
    };
    let app = axum::Router::new()
        .route("/models", axum::routing::get(models))
        .route(endpoint.path(), axum::routing::post(inference))
        .route(
            "/token",
            axum::routing::get(
                |axum::extract::State(probe): axum::extract::State<CopilotNativeProbe>| async move {
                    probe
                        .token_requests
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    axum::Json(json!({
                        "token":"copilot-refreshed-sub-token","expires_at":i64::MAX/2,
                        "endpoints":{"api":"https://api.githubcopilot.com"}
                    }))
                },
            ),
        )
        .with_state(probe.clone());
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (addr, probe, server)
}

#[tokio::test]
async fn copilot_native_stream_errors_and_truncation_never_become_success_or_replay() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    for (endpoint, app, route) in [
        (
            CopilotEndpoint::Messages,
            AppKind::Claude,
            ProxyRoute::ClaudeMessages,
        ),
        (
            CopilotEndpoint::Responses,
            AppKind::Codex,
            ProxyRoute::CodexResponses,
        ),
    ] {
        for fault in ["truncated", "error"] {
            let (addr, probe, server) =
                spawn_copilot_native_upstream(endpoint, true, false, false, false).await;
            *probe.stream_fault.lock().unwrap() = Some(fault);
            let state = forwarder_test_state(&format!("copilot-native-{endpoint:?}-{fault}"));
            let (id, _) = install_copilot_test_provider(
                &state,
                "copilot-native-fault",
                app,
                format!("http://{addr}"),
                format!("http://{addr}/token"),
            )
            .await;
            let input = if endpoint == CopilotEndpoint::Messages {
                json!({"model":"opaque-model","stream":true,"max_tokens":64,"messages":[{"role":"user","content":"hello"}]})
            } else {
                json!({"model":"opaque-model","stream":true,"input":"hello"})
            };
            let result = forward_for_test_surface(
                state.clone(),
                route,
                id,
                None,
                HeaderMap::new(),
                Bytes::from(serde_json::to_vec(&input).unwrap()),
            )
            .await;
            match result {
                Err(error) => assert!(error.status.is_server_error(), "{error:?}"),
                Ok(response) => {
                    let status = response.status();
                    let body = collect_response_body(response).await;
                    let text = String::from_utf8_lossy(&body);
                    assert!(
                        !text.contains("event: response.completed"),
                        "failed stream became successful: {endpoint:?} {fault}: {text}"
                    );
                    assert!(
                        status.is_server_error()
                            || text.contains("error")
                            || text.contains("response.failed"),
                        "failure was not exposed: {endpoint:?} {fault}: {status}: {text}"
                    );
                }
            }
            assert!(state
                .usage_snapshot()
                .await
                .logs
                .iter()
                .all(|log| log.stream_status.as_deref() != Some("completed")));
            assert_eq!(probe.requests.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(
                probe
                    .token_requests
                    .load(std::sync::atomic::Ordering::SeqCst),
                0
            );
            server.abort();
        }
    }
}

#[tokio::test]
async fn copilot_native_downstream_cancellation_releases_the_original_upstream() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    for (endpoint, app, route) in [
        (
            CopilotEndpoint::Messages,
            AppKind::Claude,
            ProxyRoute::ClaudeMessages,
        ),
        (
            CopilotEndpoint::Responses,
            AppKind::Codex,
            ProxyRoute::CodexResponses,
        ),
    ] {
        let (addr, probe, server) =
            spawn_copilot_native_upstream(endpoint, true, false, false, false).await;
        *probe.stream_fault.lock().unwrap() = Some("pending");
        let state = forwarder_test_state(&format!("copilot-native-cancel-{endpoint:?}"));
        let (id, _) = install_copilot_test_provider(
            &state,
            "copilot-native-cancel",
            app,
            format!("http://{addr}"),
            format!("http://{addr}/token"),
        )
        .await;
        let input = if endpoint == CopilotEndpoint::Messages {
            json!({"model":"opaque-model","stream":true,"max_tokens":64,"messages":[{"role":"user","content":"hello"}]})
        } else {
            json!({"model":"opaque-model","stream":true,"input":"hello"})
        };
        let response = forward_for_test_surface(
            state.clone(),
            route,
            id,
            None,
            HeaderMap::new(),
            Bytes::from(serde_json::to_vec(&input).unwrap()),
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
        tokio::time::timeout(Duration::from_secs(2), async {
            while !probe
                .stream_dropped
                .load(std::sync::atomic::Ordering::SeqCst)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("downstream cancellation must drop the same native upstream");
        assert_eq!(probe.requests.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(
            probe
                .token_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert!(state
            .usage_snapshot()
            .await
            .logs
            .iter()
            .all(|log| log.stream_status.as_deref() != Some("completed")));
        server.abort();
    }
}

#[tokio::test]
async fn copilot_native_endpoints_preserve_messages_and_responses_without_chat_intermediate() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    for (endpoint, app, route) in [
        (
            CopilotEndpoint::Messages,
            AppKind::Claude,
            ProxyRoute::ClaudeMessages,
        ),
        (
            CopilotEndpoint::Responses,
            AppKind::Codex,
            ProxyRoute::CodexResponses,
        ),
    ] {
        for stream in [false, true] {
            let (addr, probe, server) =
                spawn_copilot_native_upstream(endpoint, stream, false, false, false).await;
            let state = forwarder_test_state(&format!("copilot-native-{endpoint:?}-{stream}"));
            let (id, _) = install_copilot_test_provider(
                &state,
                "copilot-native",
                app,
                format!("http://{addr}"),
                format!("http://{addr}/token"),
            )
            .await;
            let input = match endpoint {
                CopilotEndpoint::Messages => {
                    json!({"model":"opaque-model","max_tokens":64,"stream":stream,
                    "messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"previous","signature":"original-signature"},
                        {"type":"text","text":"previous answer"},{"type":"tool_use","id":"call_previous","name":"Read","input":{"path":"before"}}]},
                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_previous","content":"previous result"},{"type":"text","text":"continue"}]}],
                    "tools":[{"name":"Read","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]})
                }
                _ => {
                    json!({"model":"opaque-model","stream":stream,"previous_response_id":"resp_previous",
                    "input":[{"type":"reasoning","id":"rs_previous","encrypted_content":"original-encrypted","summary":[]},
                        {"type":"function_call","call_id":"call_previous","name":"Read","arguments":"{}"},
                        {"type":"function_call_output","call_id":"call_previous","output":"previous result"},
                        {"role":"user","content":"continue"}],
                    "tools":[{"type":"function","name":"Read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]})
                }
            };
            let mut headers = HeaderMap::new();
            headers.insert(
                "anthropic-beta",
                HeaderValue::from_static("context-1m-2025-08-07"),
            );
            let response = forward_for_test_surface(
                state,
                route,
                id,
                None,
                headers,
                Bytes::from(serde_json::to_vec(&input).unwrap()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = collect_response_body(response).await;
            if stream {
                let text = String::from_utf8(bytes.to_vec()).unwrap();
                assert!(text.contains("answer"));
                assert!(text.contains("call_native"));
                assert!(text.contains(if endpoint == CopilotEndpoint::Messages {
                    "opaque-signature"
                } else {
                    "opaque-encrypted"
                }));
                assert!(text.contains(if endpoint == CopilotEndpoint::Messages {
                    "message_stop"
                } else {
                    "response.completed"
                }));
            } else {
                let output: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(output, copilot_native_response(endpoint));
            }
            let observations = probe.observations.lock().unwrap();
            assert_eq!(observations.len(), 1);
            let (path, headers, body) = &observations[0];
            assert_eq!(path, endpoint.path());
            assert_eq!(headers["authorization"], "Bearer copilot-initial-sub-token");
            assert!(!format!("{headers:?}").contains("github-oauth-long-lived-secret"));
            if endpoint == CopilotEndpoint::Messages {
                assert_eq!(headers["anthropic-version"], "2023-06-01");
            }
            // Native opaque state must survive, including continuation tokens.
            // Stream usage options may be added, so compare the authoritative fields.
            assert_eq!(body["model"], input["model"]);
            assert_eq!(body["tools"], input["tools"]);
            if endpoint == CopilotEndpoint::Messages {
                assert_eq!(body["messages"], input["messages"]);
            } else {
                assert_eq!(body["input"], input["input"]);
                assert_eq!(body["previous_response_id"], input["previous_response_id"]);
            }
            server.abort();
        }
    }
}

#[tokio::test]
async fn copilot_native_refresh_rechecks_catalog_without_endpoint_trial_or_revoked_replay() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    for revoked in [false, true] {
        let (addr, probe, server) =
            spawn_copilot_native_upstream(CopilotEndpoint::Responses, false, true, revoked, false)
                .await;
        let state = forwarder_test_state(&format!("copilot-native-refresh-{revoked}"));
        let (id, _) = install_copilot_test_provider(
            &state,
            "copilot-native-refresh",
            AppKind::Codex,
            format!("http://{addr}"),
            format!("http://{addr}/token"),
        )
        .await;
        let result = forward_for_test_surface(
            state,
            ProxyRoute::CodexResponses,
            id,
            None,
            HeaderMap::new(),
            Bytes::from_static(br#"{"model":"opaque-model","input":"hello","stream":false}"#),
        )
        .await;
        if revoked {
            assert!(result.is_err());
        } else {
            let response = result.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            collect_response_body(response).await;
        }
        let observations = probe.observations.lock().unwrap();
        assert_eq!(observations.len(), if revoked { 1 } else { 2 });
        assert!(observations.iter().all(|(path, _, _)| path == "/responses"));
        if !revoked {
            assert_eq!(
                observations[1].1["authorization"],
                "Bearer copilot-refreshed-sub-token"
            );
        }
        server.abort();
    }
}

#[tokio::test]
async fn copilot_discovery_and_inference_share_one_authentication_recovery_budget() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    let (addr, probe, server) =
        spawn_copilot_native_upstream(CopilotEndpoint::Responses, false, true, false, true).await;
    let state = forwarder_test_state("copilot-shared-auth-budget");
    let (id, _) = install_copilot_test_provider(
        &state,
        "copilot-shared-auth-budget",
        AppKind::Codex,
        format!("http://{addr}"),
        format!("http://{addr}/token"),
    )
    .await;
    let response = forward_for_test_surface(
        state,
        ProxyRoute::CodexResponses,
        id,
        None,
        HeaderMap::new(),
        Bytes::from_static(br#"{"model":"opaque-model","input":"hello","stream":false}"#),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    collect_response_body(response).await;
    assert_eq!(
        probe
            .token_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(
        probe
            .model_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    assert_eq!(probe.requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn copilot_native_endpoints_bridge_all_declared_downstream_routes_and_tool_lifecycles() {
    use crate::domain::accounts::copilot::CopilotEndpoint;
    for endpoint in [CopilotEndpoint::Messages, CopilotEndpoint::Responses] {
        for (app, route) in [
            (AppKind::Claude, ProxyRoute::ClaudeMessages),
            (AppKind::Codex, ProxyRoute::CodexResponses),
            (AppKind::Codex, ProxyRoute::CodexChatCompletions),
            (AppKind::Gemini, ProxyRoute::Gemini),
        ] {
            for stream in [false, true] {
                let (addr, probe, server) =
                    spawn_copilot_native_upstream(endpoint, stream, false, false, false).await;
                let state = forwarder_test_state(&format!(
                    "copilot-native-matrix-{endpoint:?}-{route:?}-{stream}"
                ));
                let (id, _) = install_copilot_test_provider(
                    &state,
                    "copilot-native-matrix",
                    app,
                    format!("http://{addr}"),
                    format!("http://{addr}/token"),
                )
                .await;
                let input = match route {
                    ProxyRoute::ClaudeMessages => {
                        json!({"model":"opaque-model","max_tokens":64,"stream":stream,
                        "messages":[{"role":"user","content":"read a"}],
                        "tools":[{"name":"Read","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}]})
                    }
                    ProxyRoute::CodexResponses => {
                        json!({"model":"opaque-model","stream":stream,"input":"read a",
                        "tools":[{"type":"function","name":"Read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]})
                    }
                    ProxyRoute::CodexChatCompletions => {
                        json!({"model":"opaque-model","stream":stream,"messages":[{"role":"user","content":"read a"}],
                        "tools":[{"type":"function","function":{"name":"Read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}}]})
                    }
                    _ => json!({"contents":[{"role":"user","parts":[{"text":"read a"}]}],
                        "tools":[{"functionDeclarations":[{"name":"Read","parameters":{"type":"object","properties":{"path":{"type":"string"}}}}]}]}),
                };
                let path = (route == ProxyRoute::Gemini).then(|| {
                    format!(
                        "models/opaque-model:{}",
                        if stream {
                            "streamGenerateContent"
                        } else {
                            "generateContent"
                        }
                    )
                });
                let (response, timing) = r5_timing_forward(
                    state,
                    route,
                    id,
                    path,
                    HeaderMap::new(),
                    Bytes::from(serde_json::to_vec(&input).unwrap()),
                )
                .await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "{endpoint:?} {route:?} stream={stream}"
                );
                let bytes = collect_response_body(response).await;
                let text = String::from_utf8(bytes.to_vec()).unwrap();
                assert!(
                    text.contains("Read"),
                    "missing native tool: {endpoint:?} {route:?} stream={stream}: {text}"
                );
                assert!(
                    !text.contains("\"error\":"),
                    "unexpected stream error: {text}"
                );
                if !stream {
                    let v: Value = serde_json::from_slice(&bytes).unwrap();
                    match route {
                        ProxyRoute::ClaudeMessages => assert_eq!(v["stop_reason"], "tool_use"),
                        ProxyRoute::CodexResponses => assert_eq!(v["status"], "completed"),
                        ProxyRoute::CodexChatCompletions => {
                            assert_eq!(v["choices"][0]["finish_reason"], "tool_calls")
                        }
                        _ => assert!(v["candidates"][0]["content"]["parts"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|p| p.get("functionCall").is_some())),
                    }
                }
                assert_eq!(probe.requests.load(std::sync::atomic::Ordering::SeqCst), 1);
                assert_eq!(probe.observations.lock().unwrap()[0].0, endpoint.path());
                drop(bytes);
                r5_timing_assert_delivery(&timing, "github_copilot", "complete").await;
                server.abort();
            }
        }
    }
}
