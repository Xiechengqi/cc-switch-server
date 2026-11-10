// CORE-01 exercises real loopback forwarding, including drivers that bypass
// the generic HTTP path. Fixtures cannot prove vendor acceptance or latency.
async fn r5_timing_forward(
    state: ServerState,
    route: ProxyRoute,
    id: String,
    path: Option<String>,
    headers: HeaderMap,
    body: Bytes,
) -> (Response, TimingSlot) {
    let attempt = claude_timing_attempt(&state, route, &id).await;
    let timing = attempt.timing.clone();
    let response = forward_with_attempt(state, route, path, headers, body, attempt)
        .await
        .unwrap();
    (response, timing)
}

async fn r5_timing_assert_delivery(slot: &TimingSlot, label: &str, result: &str) {
    let timing = slot.get().expect("actual driver must initialize timing");
    tokio::time::timeout(Duration::from_secs(2), async {
        while timing.request().snapshot()["milestonesMs"]["owner_released"].is_null() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "last owner did not release: {}",
            timing.request().snapshot()
        )
    });
    let snapshot = timing.request().snapshot();
    assert_eq!(snapshot["providerType"], label, "{snapshot}");
    assert_eq!(snapshot["result"], result, "{snapshot}");
    let mut previous = 0.0;
    for phase in [
        "admission",
        "send_started",
        "headers",
        "first_semantic",
        "terminal",
    ] {
        let at = snapshot["milestonesMs"][phase]
            .as_f64()
            .unwrap_or_else(|| panic!("missing {phase}: {snapshot}"));
        assert!(at >= previous, "out-of-order {phase}: {snapshot}");
        previous = at;
    }
    assert!(
        snapshot["milestonesMs"]["http_body_handoff"].is_number(),
        "{snapshot}"
    );
    for phase in ["upstream_write_completed", "downstream_write_completed"] {
        assert!(
            snapshot["milestonesMs"][phase].is_null(),
            "HTTP handoff is not socket completion: {snapshot}"
        );
    }
}

fn r5_timing_input(route: ProxyRoute, model: &str, stream: bool) -> (Option<String>, Bytes) {
    let value = match route {
        ProxyRoute::ClaudeMessages => {
            json!({"model":model,"max_tokens":64,"stream":stream,"messages":[{"role":"user","content":"hello"}]})
        }
        ProxyRoute::CodexResponses => json!({"model":model,"stream":stream,"input":"hello"}),
        ProxyRoute::CodexChatCompletions => {
            json!({"model":model,"stream":stream,"messages":[{"role":"user","content":"hello"}]})
        }
        ProxyRoute::Gemini => json!({"contents":[{"role":"user","parts":[{"text":"hello"}]}]}),
        _ => unreachable!(),
    };
    let path = (route == ProxyRoute::Gemini).then(|| {
        format!(
            "models/{model}:{}",
            if stream {
                "streamGenerateContent"
            } else {
                "generateContent"
            }
        )
    });
    (path, Bytes::from(value.to_string()))
}

async fn r5_timing_protocol_fixture() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |uri: axum::http::Uri, axum::Json(body): axum::Json<Value>| {
        let calls = observed.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if uri.path().ends_with("countTokens") { return axum::Json(json!({"totalTokens":0})).into_response(); }
            if uri.path().ends_with("count_tokens") { return axum::Json(json!({"input_tokens":0})).into_response(); }
            let streaming = body["stream"] == true || uri.path().contains("streamGenerateContent");
            let (json, sse) = if uri.path().ends_with("messages") {
                (json!({"id":"msg_timing","type":"message","role":"assistant","model":"fixture-model","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),format!("{CLAUDE_TIMING_CONTENT}{CLAUDE_TIMING_TERMINAL}"))
            } else if uri.path().ends_with("responses") {
                let terminal = json!({"id":"resp_timing","object":"response","status":"completed","model":"fixture-model","output":[{"id":"msg_timing","type":"message","role":"assistant","content":[{"type":"output_text","text":"hello","annotations":[]}]}],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}});
                let sse = format!("data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"resp_timing\",\"status\":\"in_progress\"}}}}\n\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"hello\",\"output_index\":0,\"content_index\":0}}\n\ndata: {{\"type\":\"response.completed\",\"response\":{terminal}}}\n\n");
                (terminal, sse)
            } else if uri.path().contains("GenerateContent") || uri.path().contains("generateContent") {
                let terminal = json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":1,"totalTokenCount":2}});
                let wire = if uri.path().starts_with("/v1internal:") { json!({"response":terminal}) } else { terminal.clone() };
                (wire.clone(), format!("data: {wire}\n\n"))
            } else {
                assert!(uri.path().ends_with("chat/completions"), "unexpected protocol fixture path: {uri}");
                (json!({"id":"chat_timing","object":"chat.completion","model":"fixture-model","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}), concat!(
                    "data: {\"id\":\"chat_timing\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
                    "data: {\"id\":\"chat_timing\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n",
                    "data: {\"id\":\"chat_timing\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
                    "data: [DONE]\n\n"
                ).to_string())
            };
            Response::builder().header(CONTENT_TYPE, if streaming {"text/event-stream"} else {"application/json"})
                .body(Body::from(if streaming {sse} else {json.to_string()})).unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, calls, server)
}

#[tokio::test]
async fn r5_timing_static_provider_types_observe_all_four_protocols_in_real_forwarding() {
    let (endpoint, calls, server) = r5_timing_protocol_fixture().await;
    let cases = [
        (
            ProviderType::Claude,
            AppKind::Claude,
            "claude.anthropic_api_key",
        ),
        (
            ProviderType::ClaudeAuth,
            AppKind::Claude,
            "claude.custom_http",
        ),
        (ProviderType::Codex, AppKind::Codex, "codex.openai_api_key"),
        (
            ProviderType::Gemini,
            AppKind::Gemini,
            "gemini.google_api_key",
        ),
        (ProviderType::Nvidia, AppKind::Claude, "claude.nvidia"),
        (ProviderType::Nvidia, AppKind::Codex, "codex.nvidia"),
        (
            ProviderType::DeepSeekApi,
            AppKind::Claude,
            "claude.deepseek_api",
        ),
        (
            ProviderType::DeepSeekApi,
            AppKind::Codex,
            "codex.deepseek_api",
        ),
        (
            ProviderType::OpenRouter,
            AppKind::Claude,
            "claude.openrouter",
        ),
        (ProviderType::OpenRouter, AppKind::Codex, "codex.openrouter"),
        (
            ProviderType::OpenRouter,
            AppKind::Gemini,
            "gemini.openrouter",
        ),
    ];
    let mut expected = 0;
    for (kind, app, profile) in cases {
        let state = forwarder_test_state(&format!("r5-timing-{profile}"));
        let mut stored = stored_provider(
            app,
            kind,
            json!({"apiKey":"synthetic-key","env":{"ANTHROPIC_AUTH_TOKEN":"synthetic-key","ANTHROPIC_API_KEY":"synthetic-key","OPENAI_API_KEY":"synthetic-key","GEMINI_API_KEY":"synthetic-key"},"modelMapping":{"mode":if kind == ProviderType::Gemini { "passthrough" } else { "single" },"upstreamModel":"fixture-model"},"testRuntimeEndpoint":endpoint}),
            None,
        );
        stored.resource.profile_id =
            Some(crate::domain::providers::registry::ProfileId::parse(profile).unwrap());
        stored.resource.profile_schema_revision = Some(1);
        if kind == ProviderType::ClaudeAuth {
            stored.resource.custom_binding =
                Some(crate::domain::providers::registry::CustomBindingInput {
                    upstream_protocol:
                        crate::domain::providers::registry::UpstreamProtocol::AnthropicMessages,
                    auth_scheme: crate::domain::providers::registry::AuthScheme::Bearer,
                });
            stored.provider.settings_config["env"]["ANTHROPIC_BASE_URL"] = json!(endpoint);
            stored.provider.settings_config["modelMapping"]["mode"] = json!("passthrough");
        }
        let id = stored.provider.id.clone();
        let mut providers = ProviderStore {
            providers: vec![stored],
            ..Default::default()
        };
        providers
            .rebuild_runtime_index(&state.accounts_snapshot().await)
            .unwrap();
        state.replace_provider_store_for_test(providers).await;
        state
            .override_provider_runtime_endpoint_for_test(app, &id, endpoint.clone())
            .await
            .unwrap();
        let routes: &[ProxyRoute] = match app {
            AppKind::Claude => &[ProxyRoute::ClaudeMessages],
            AppKind::Codex => &[ProxyRoute::CodexChatCompletions, ProxyRoute::CodexResponses],
            AppKind::Gemini => &[ProxyRoute::Gemini],
        };
        for &route in routes {
            for streaming in [false, true] {
                let (path, body) = r5_timing_input(route, "fixture-model", streaming);
                let (response, timing) = r5_timing_forward(
                    state.clone(),
                    route,
                    id.clone(),
                    path,
                    HeaderMap::new(),
                    body,
                )
                .await;
                assert_eq!(
                    response.status(),
                    StatusCode::OK,
                    "{profile} {route:?} {streaming}"
                );
                let bytes = collect_response_body(response).await;
                assert!(
                    String::from_utf8_lossy(&bytes).contains("hello"),
                    "{profile} {route:?} {streaming}"
                );
                drop(bytes);
                r5_timing_assert_delivery(&timing, kind.as_str(), "complete").await;
                expected += 1;
            }
        }
        if kind == ProviderType::Gemini
            || kind == ProviderType::Claude
            || kind == ProviderType::ClaudeAuth
        {
            let (route, path) = if kind == ProviderType::Gemini {
                (
                    ProxyRoute::Gemini,
                    Some("models/fixture-model:countTokens".into()),
                )
            } else {
                (ProxyRoute::ClaudeCountTokens, None)
            };
            let (_, body) = r5_timing_input(
                if kind == ProviderType::Gemini {
                    ProxyRoute::Gemini
                } else {
                    ProxyRoute::ClaudeMessages
                },
                "fixture-model",
                false,
            );
            let (response, timing) = r5_timing_forward(
                state.clone(),
                route,
                id.clone(),
                path,
                HeaderMap::new(),
                body,
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = collect_response_body(response).await;
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                value[if kind == ProviderType::Gemini {
                    "totalTokens"
                } else {
                    "input_tokens"
                }],
                0
            );
            drop(bytes);
            r5_timing_assert_delivery(&timing, kind.as_str(), "complete").await;
            expected += 1;
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), expected);
    server.abort();
}

#[tokio::test]
async fn r5_timing_kimi_and_gemini_cli_cover_all_declared_app_surfaces() {
    let (endpoint, calls, server) = r5_timing_protocol_fixture().await;
    let mut expected = 0;
    for kind in [ProviderType::KimiCode, ProviderType::GeminiCli] {
        let state = forwarder_test_state(&format!("r5-timing-{}", kind.as_str()));
        let id = if kind == ProviderType::KimiCode {
            install_kimi_replay_test_provider(&state, "r5-timing-kimi", endpoint.clone())
                .await
                .0
        } else {
            install_gemini_v1internal_test_provider(
                &state,
                "r5-timing",
                endpoint.clone(),
                "http://127.0.0.1:9/token".into(),
                true,
            )
            .await
            .0
        };
        let base = state.providers.read().await.providers[0].clone();
        let apps: &[AppKind] = if kind == ProviderType::KimiCode {
            &[AppKind::Claude, AppKind::Codex, AppKind::Gemini]
        } else {
            &[AppKind::Claude, AppKind::Gemini]
        };
        let profiles: Vec<_> = apps
            .iter()
            .map(|&app| {
                let mut stored = base.clone();
                stored.app = app;
                stored.resource.profile_id = Some(
                    crate::domain::providers::registry::ProfileId::parse(format!(
                        "{}.{}",
                        app.as_str(),
                        if kind == ProviderType::KimiCode {
                            "kimi_code"
                        } else {
                            "google_oauth"
                        }
                    ))
                    .unwrap(),
                );
                stored
            })
            .collect();
        let mut providers = ProviderStore {
            providers: profiles,
            ..Default::default()
        };
        providers
            .rebuild_runtime_index(&state.accounts_snapshot().await)
            .unwrap();
        state.replace_provider_store_for_test(providers).await;
        for &app in apps {
            state
                .override_provider_runtime_endpoint_for_test(app, &id, endpoint.clone())
                .await
                .unwrap();
            let routes: &[ProxyRoute] = match app {
                AppKind::Claude => &[ProxyRoute::ClaudeMessages],
                AppKind::Codex => &[ProxyRoute::CodexChatCompletions, ProxyRoute::CodexResponses],
                AppKind::Gemini => &[ProxyRoute::Gemini],
            };
            for &route in routes {
                for streaming in [false, true] {
                    let (path, body) = r5_timing_input(
                        route,
                        if kind == ProviderType::KimiCode {
                            "kimi-for-coding"
                        } else {
                            "gemini-2.5-pro"
                        },
                        streaming,
                    );
                    let (response, timing) = r5_timing_forward(
                        state.clone(),
                        route,
                        id.clone(),
                        path,
                        HeaderMap::new(),
                        body,
                    )
                    .await;
                    assert_eq!(
                        response.status(),
                        StatusCode::OK,
                        "{} {route:?} {streaming}",
                        kind.as_str()
                    );
                    drop(collect_response_body(response).await);
                    r5_timing_assert_delivery(&timing, kind.as_str(), "complete").await;
                    expected += 1;
                }
            }
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), expected);
    server.abort();
}

async fn r5_timing_install_trae(
    state: &ServerState,
    kind: ProviderType,
    app: AppKind,
    endpoint: &str,
) -> String {
    let endpoint = endpoint.to_string();
    state.mutate_accounts_immediate(move |accounts| {
        accounts.upsert(serde_json::from_value(json!({"id":"r5-trae-account","providerType":kind.as_str(),"authIdentityGeneration":1,"tokenRefreshGeneration":1,"accessToken":"synthetic-access","refreshToken":"synthetic-refresh","tokenType":crate::domain::trae::TraeProtocol::for_provider(kind).unwrap().token_type(),"expiresAt":i64::MAX/2,"profile":{"uid":"fixture-uid","enterpriseId":"","machineId":"fixture-machine","deviceId":"fixture-device"},"raw":{"testTraeAgentOrigin":endpoint,"testTraeOAuthOrigin":endpoint,"testTraeBillingOrigin":"http://127.0.0.1:9"}})).unwrap());
    }).await.unwrap();
    let mut stored = stored_provider(
        app,
        kind,
        json!({"modelMapping":{"mode":"passthrough"}}),
        Some("r5-trae-account"),
    );
    let id = stored.provider.id.clone();
    stored.resource.profile_id = Some(
        crate::domain::providers::registry::ProfileId::parse(format!(
            "{}.{}",
            app.as_str(),
            kind.as_str()
        ))
        .unwrap(),
    );
    stored.resource.profile_schema_revision = Some(1);
    stored.resource.revision = 1;
    let mut providers = ProviderStore {
        providers: vec![stored],
        ..Default::default()
    };
    providers
        .rebuild_runtime_index(&state.accounts_snapshot().await)
        .unwrap();
    state.replace_provider_store_for_test(providers).await;
    id
}

#[tokio::test]
async fn r5_timing_trae_three_rails_cover_four_routes_without_counting_queue_frames() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app=axum::Router::new().fallback(axum::routing::post(move |uri:axum::http::Uri| {
        let calls=observed.clone();async move {
            if uri.path()==crate::domain::trae::TRAE_MODEL_DETAIL_PATH {
                return axum::Json(json!({"config_info_list":[{"config_name":"DeepSeek-V4-Pro-Official","display_config":{"display_name":"DeepSeek V4 Pro"}}]})).into_response();
            }
            if uri.path()=="/api/ide/v1/model_list" { return axum::Json(json!({"model_configs":[{"model_name":"DeepSeek-V4-Pro-Official"}]})).into_response(); }
            calls.fetch_add(1,Ordering::SeqCst);
            ([(CONTENT_TYPE,"text/event-stream")],"event: progress_notice\ndata: \"Processing_InQueue\"\n\nevent: output\ndata: {\"response\":\"hello\"}\n\nevent: token_usage\ndata: {\"prompt_tokens\":1,\"completion_tokens\":1}\n\nevent: done\ndata: {\"finish_reason\":\"stop\"}\n\n").into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for kind in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexResponses,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::Gemini,
        ] {
            let state = forwarder_test_state(&format!("r5-timing-{}-{route:?}", kind.as_str()));
            let id = r5_timing_install_trae(&state, kind, route.app(), &endpoint).await;
            for streaming in [false, true] {
                let (path, body) = r5_timing_input(route, "DeepSeek-V4-Pro-Official", streaming);
                let (response, timing) = r5_timing_forward(
                    state.clone(),
                    route,
                    id.clone(),
                    path,
                    HeaderMap::new(),
                    body,
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = collect_response_body(response).await;
                assert!(String::from_utf8_lossy(&bytes).contains("hello"));
                drop(bytes);
                r5_timing_assert_delivery(&timing, kind.as_str(), "complete").await;
            }
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 24);
    server.abort();
}

#[tokio::test]
async fn r5_timing_deepseek_completion_observes_json_and_sse_with_existing_pow_and_scope() {
    for streaming in [false, true] {
        let (address, observed, server) =
            spawn_deepseek_session_fixture(DeepSeekSessionFixtureMode::RebuildCachedSession).await;
        let state = forwarder_test_state(&format!("r5-timing-deepseek-{streaming}"));
        let (id, _) = install_deepseek_forwarder_test_provider(
            &state,
            "r5-timing",
            format!("http://{address}"),
        )
        .await;
        let share = "r5-timing-deepseek-share";
        install_antigravity_test_share(
            &state,
            share,
            AppKind::Claude,
            ProviderType::DeepSeekAccount,
            &id,
        )
        .await;
        for repaired in [false, true] {
            let (response, timing) = r5_timing_forward(
                state.clone(),
                ProxyRoute::ClaudeMessages,
                id.clone(),
                None,
                deepseek_forwarder_headers(share, "r5-timing-client-session"),
                deepseek_forwarder_request(streaming),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            drop(collect_response_body(response).await);
            r5_timing_assert_delivery(&timing, "deepseek_account", "complete").await;
            let current = timing.get().unwrap();
            let request = current.request().snapshot();
            let attempt = current.attempt_snapshot();
            if repaired {
                assert!(request["waitsMs"]["retry"].as_f64().unwrap() > 0.0);
                // The request includes the rejected completion's headers;
                // the final attempt contains only the replacement completion.
                assert!(
                    request["waitsMs"]["headers"].as_f64().unwrap()
                        > attempt["waitsMs"]["headers"].as_f64().unwrap(),
                    "request={request} attempt={attempt}"
                );
            } else {
                assert_eq!(request["waitsMs"]["retry"], 0.0);
            }
        }
        assert_eq!(observed.session_requests.load(Ordering::SeqCst), 2);
        assert_eq!(observed.pow_requests.load(Ordering::SeqCst), 3);
        assert_eq!(observed.completion_requests.load(Ordering::SeqCst), 3);
        server.abort();
    }
}

#[tokio::test]
async fn r5_timing_kimi_real_header_cancellation_and_http_failure_release_owners() {
    for cancel in [false, true] {
        let ready = Arc::new(tokio::sync::Notify::new());
        let observed = ready.clone();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed_calls = calls.clone();
        let app = axum::Router::new().fallback(axum::routing::post(move || {
            let ready = observed.clone();
            let calls = observed_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                ready.notify_one();
                if cancel {
                    std::future::pending::<()>().await;
                }
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(
                        json!({"error":{"type":"server_error","message":"synthetic failure"}}),
                    ),
                )
            }
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("r5-timing-kimi-failure-{cancel}"));
        let (id, _) =
            install_kimi_replay_test_provider(&state, "r5-timing-failure", endpoint).await;
        let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
        let slot = attempt.timing.clone();
        let task = tokio::spawn(forward_with_attempt(
            state,
            ProxyRoute::ClaudeMessages,
            None,
            HeaderMap::new(),
            kimi_seed_request(false),
            attempt,
        ));
        tokio::time::timeout(Duration::from_secs(2), ready.notified())
            .await
            .unwrap();
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            match task.await.unwrap() {
                Ok(response) => {
                    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
                    drop(collect_response_body(response).await);
                }
                Err(error) => assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        let timing = slot.get().unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while timing.request().snapshot()["milestonesMs"]["owner_released"].is_null() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let snapshot = timing.request().snapshot();
        assert_eq!(snapshot["providerType"], "kimi_code");
        assert_eq!(
            snapshot["result"],
            if cancel { "cancelled" } else { "failed" }
        );
        assert!(snapshot["milestonesMs"]["first_semantic"].is_null());
        if cancel {
            assert!(snapshot["milestonesMs"]["headers"].is_null());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[test]
fn r5_resource_runtime_capacity_is_503_and_never_authentication_recovery() {
    let error = TraeRuntimeError::Capacity;
    assert!(!error.is_authentication_failure());
    assert_eq!(
        trae_runtime_error_to_proxy_error(error).status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        deepseek_upstream_error_to_proxy_error(DeepSeekUpstreamError::RuntimeCapacity).status,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn r5_timing_gemini_cli_native_count_tokens_is_semantic_without_discovery() {
    let (address, _, loads, server) = spawn_gemini_count_tokens_upstream(0, false).await;
    let state = forwarder_test_state("r5-timing-gemini-cli-count");
    let (id, _) = install_gemini_v1internal_test_provider(
        &state,
        "r5-timing-count",
        format!("http://{address}"),
        "http://127.0.0.1:9/token".into(),
        false,
    )
    .await;
    let (response, timing) = r5_timing_forward(
        state,
        ProxyRoute::Gemini,
        id,
        Some("models/gemini-2.5-pro:countTokens".into()),
        HeaderMap::new(),
        Bytes::from_static(br#"{"contents":[{"role":"user","parts":[{"text":"hello"}]}]}"#),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    drop(collect_response_body(response).await);
    r5_timing_assert_delivery(&timing, "gemini_cli", "complete").await;
    assert_eq!(loads.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn r5_timing_copilot_native_chat_endpoint_covers_four_downstream_routes() {
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for streaming in [false, true] {
            let fixture = if streaming {
                CopilotInferenceFixture::StreamTool
            } else {
                CopilotInferenceFixture::NonStreamTool
            };
            let (address, probe, server) = spawn_copilot_upstream(fixture, 0).await;
            let state =
                forwarder_test_state(&format!("r5-timing-copilot-chat-{route:?}-{streaming}"));
            let (id, _) = install_copilot_test_provider(
                &state,
                "r5-timing-copilot-chat",
                route.app(),
                format!("http://{address}"),
                format!("http://{address}/token"),
            )
            .await;
            let (path, bytes) = r5_timing_input(route, "gpt-5.5", streaming);
            let mut input: Value = serde_json::from_slice(&bytes).unwrap();
            input["tools"] = match route {
                ProxyRoute::ClaudeMessages => {
                    json!([{"name":"lookup_weather","input_schema":{"type":"object","properties":{"city":{"type":"string"}}}}])
                }
                ProxyRoute::CodexResponses => {
                    json!([{"type":"function","name":"lookup_weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}])
                }
                ProxyRoute::Gemini => {
                    json!([{"functionDeclarations":[{"name":"lookup_weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}]}])
                }
                _ => {
                    json!([{"type":"function","function":{"name":"lookup_weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}])
                }
            };
            let (response, timing) = r5_timing_forward(
                state,
                route,
                id,
                path,
                HeaderMap::new(),
                Bytes::from(input.to_string()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let output = collect_response_body(response).await;
            let text = String::from_utf8_lossy(&output);
            assert!(
                text.contains("lookup_weather"),
                "{route:?} {streaming}: {text}"
            );
            assert!(
                !text.contains("\"error\":"),
                "{route:?} {streaming}: {text}"
            );
            drop(output);
            r5_timing_assert_delivery(&timing, "github_copilot", "complete").await;
            assert_eq!(probe.inference_requests.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }
}

#[tokio::test]
async fn r5_timing_web_local_count_has_semantic_delivery_and_no_upstream_send() {
    for driver in ["grok", "perplexity"] {
        let state = forwarder_test_state(&format!("r5-timing-web-count-{driver}"));
        let execution = install_web_session_forwarder_test_provider(
            &state,
            AppKind::Claude,
            driver,
            "http://127.0.0.1:9".into(),
            1,
        )
        .await;
        let (response, slot) = r5_timing_forward(
            state,
            ProxyRoute::ClaudeCountTokens,
            execution.stored.provider.id.clone(),
            None,
            HeaderMap::new(),
            Bytes::from_static(
                br#"{"model":"client-alias","messages":[{"role":"user","content":"hello"}]}"#,
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        drop(collect_response_body(response).await);
        let timing = slot.get().unwrap();
        let snapshot = timing.request().snapshot();
        assert_eq!(
            snapshot["providerType"],
            if driver == "grok" {
                "grok_web_session"
            } else {
                "perplexity_web_session"
            }
        );
        assert_eq!(snapshot["result"], "complete");
        for phase in [
            "admission",
            "first_semantic",
            "terminal",
            "http_body_handoff",
            "owner_released",
        ] {
            assert!(snapshot["milestonesMs"][phase].is_number(), "{snapshot}");
        }
        for phase in [
            "send_started",
            "headers",
            "upstream_write_completed",
            "downstream_write_completed",
        ] {
            assert!(snapshot["milestonesMs"][phase].is_null(), "{snapshot}");
        }
    }
}
