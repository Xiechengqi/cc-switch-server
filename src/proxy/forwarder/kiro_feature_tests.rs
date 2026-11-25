#[tokio::test]
async fn kiro_enhancement_stop_metadata_survives_every_public_surface() {
    for (reason, expected) in [
        ("MAX_TOKENS", "max_tokens"),
        ("CONTENT_FILTERED", "refusal"),
    ] {
        let mut frames =
            kiro::fixture_event_frame("assistantResponseEvent", &json!({"content":"answer"}));
        frames.extend(kiro::fixture_event_frame(
            "metadataEvent",
            &json!({"stopReason":reason}),
        ));
        frames.extend(kiro::fixture_event_frame("endEvent", &json!({})));
        let (address, server) = spawn_kiro_eventstream_upstream(frames).await;
        for (index, route) in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
        ]
        .into_iter()
        .enumerate()
        {
            let state = forwarder_test_state(&format!("kiro-stop-{reason}-{index}"));
            let provider = install_kiro_test_provider_for_app(
                &state,
                route.app(),
                "kiro-stop",
                format!("http://{address}"),
                "http://127.0.0.1:9/token".into(),
            )
            .await;
            for streaming in [false, true] {
                let body = match route {
                    ProxyRoute::CodexResponses => {
                        json!({"model":"claude-sonnet-4-8","input":"hi","stream":streaming})
                    }
                    _ => {
                        json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"hi"}],"stream":streaming})
                    }
                };
                let response = forward_for_test_surface(
                    state.clone(),
                    route,
                    provider.clone(),
                    None,
                    HeaderMap::new(),
                    Bytes::from(body.to_string()),
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = collect_response_body(response).await;
                let text = String::from_utf8(bytes.clone()).unwrap();
                let marker = match route {
                    ProxyRoute::ClaudeMessages => expected,
                    ProxyRoute::CodexChatCompletions => {
                        if reason == "MAX_TOKENS" {
                            "length"
                        } else {
                            "content_filter"
                        }
                    }
                    _ => {
                        if reason == "MAX_TOKENS" {
                            "incomplete"
                        } else {
                            "content_filter"
                        }
                    }
                };
                assert!(
                    text.contains(marker),
                    "{route:?} stream={streaming}: {text}"
                );
            }
        }
        server.abort();
    }
}

#[tokio::test]
async fn kiro_enhancement_hosted_search_completes_on_bound_account_for_all_surfaces() {
    let observed = Arc::new(std::sync::Mutex::new(Vec::<(String, Value, String)>::new()));
    let handler_observed = observed.clone();
    let app=axum::Router::new().fallback(axum::routing::post(move |uri:axum::http::Uri, headers:HeaderMap, axum::Json(body):axum::Json<Value>| {
        let observed=handler_observed.clone();
        async move {
            let path=uri.path().to_string();
            observed.lock().unwrap().push((path.clone(),body.clone(),headers.get("authorization").unwrap().to_str().unwrap().to_string()));
            if path.ends_with("/mcp") {
                assert!(headers.contains_key("x-amzn-kiro-profile-arn"));
                return axum::Json(json!({"jsonrpc":"2.0","id":body["id"],"result":{"isError":false,"content":[{"type":"text","text":json!({"results":[{"url":"https://example.com/news","title":"News","snippet":"Current information"}]}).to_string()}]}})).into_response();
            }
            let searched=body.pointer("/conversationState/currentMessage/userInputMessage/userInputMessageContext/toolResults").and_then(Value::as_array).is_some_and(|results|!results.is_empty());
            let mut frames=if searched {kiro::fixture_event_frame("assistantResponseEvent",&json!({"content":"searched answer"}))} else {kiro::fixture_event_frame("toolUseEvent",&json!({"toolUseId":"search-call","name":kiro::client_features::SEARCH_TOOL,"input":"{\"query\":\"current news\"}","stop":true}))};
            frames.extend(kiro::fixture_event_frame("metadataEvent",&json!({"tokenUsage":{"inputTokens":10,"outputTokens":5}})));
            frames.extend(kiro::fixture_event_frame("endEvent",&json!({})));
            Response::new(Body::from(frames))
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    for (index, route) in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
    ]
    .into_iter()
    .enumerate()
    {
        let state = forwarder_test_state(&format!("kiro-hosted-{index}"));
        let provider = install_kiro_test_provider_for_app(
            &state,
            route.app(),
            "kiro-hosted",
            format!("http://{address}"),
            "http://127.0.0.1:9/token".into(),
        )
        .await;
        for streaming in [false, true] {
            let mut body = match route {
                ProxyRoute::CodexResponses => {
                    json!({"model":"claude-sonnet-4-8","input":"latest news","tools":[{"type":"web_search"}]})
                }
                ProxyRoute::CodexChatCompletions => {
                    json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"latest news"}],"web_search_options":{}})
                }
                _ => {
                    json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"latest news"}],"tools":[{"type":"web_search_20250305","name":"web_search"}]})
                }
            };
            body["stream"] = json!(streaming);
            let response = forward_for_test_surface(
                state.clone(),
                route,
                provider.clone(),
                None,
                HeaderMap::new(),
                Bytes::from(body.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let text = String::from_utf8(collect_response_body(response).await).unwrap();
            assert!(
                text.contains("searched answer"),
                "{route:?} stream={streaming}: {text}"
            );
            if route != ProxyRoute::CodexChatCompletions {
                assert!(text.contains("web_search"), "{text}");
            }
            assert!(
                !text.contains(kiro::client_features::SEARCH_TOOL),
                "internal tool escaped: {text}"
            );
            assert_eq!(
                state
                    .account_in_flight
                    .snapshot()
                    .current(ProviderType::KiroOAuth, "kiro-hosted-account"),
                0
            );
        }
    }
    let observations = observed.lock().unwrap();
    assert_eq!(
        observations
            .iter()
            .filter(|(path, _, _)| path.ends_with("/mcp"))
            .count(),
        6
    );
    assert!(observations
        .iter()
        .all(|(_, _, authorization)| authorization == "Bearer kiro-old-access"));
    server.abort();
}

#[tokio::test]
async fn kiro_enhancement_compaction_executes_one_summary_and_restores_next_turn() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let handler_seen = seen.clone();
    let app = axum::Router::new().fallback(axum::routing::post(
        move |axum::Json(body): axum::Json<Value>| {
            let seen = handler_seen.clone();
            async move {
                seen.lock().unwrap().push(body);
                let mut frames = kiro::fixture_event_frame(
                    "assistantResponseEvent",
                    &json!({"content":"summary marker"}),
                );
                frames.extend(kiro::fixture_event_frame("endEvent", &json!({})));
                Response::new(Body::from(frames))
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = forwarder_test_state("kiro-summary");
    let provider = install_kiro_test_provider_for_app(
        &state,
        AppKind::Codex,
        "kiro-summary",
        format!("http://{address}"),
        "http://127.0.0.1:9/token".into(),
    )
    .await;
    let response=forward_for_test_surface(state.clone(),ProxyRoute::CodexResponses,provider.clone(),None,HeaderMap::new(),Bytes::from(json!({"model":"claude-sonnet-4-8","input":[{"role":"user","content":"remember this"},{"type":"compaction_trigger"}]}).to_string())).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let compact: Value = serde_json::from_slice(&collect_response_body(response).await).unwrap();
    assert_eq!(compact["output"][0]["type"], "compaction");
    assert_eq!(seen.lock().unwrap().len(), 1);
    let response=forward_for_test_surface(state,ProxyRoute::CodexResponses,provider,None,HeaderMap::new(),Bytes::from(json!({"model":"claude-sonnet-4-8","input":[compact["output"][0].clone(),{"role":"user","content":"continue"}]}).to_string())).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = collect_response_body(response).await;
    let seen = seen.lock().unwrap();
    assert!(seen[1].to_string().contains("summary marker"));
    assert!(!seen[0].to_string().contains("compaction_trigger"));
    server.abort();
}

#[tokio::test]
async fn kiro_enhancement_raw_arguments_arrive_before_upstream_completion() {
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_release = release.clone();
    let app=axum::Router::new().fallback(axum::routing::post(move || {let release=handler_release.clone();async move {
        let stream=async_stream::stream! {
            yield Ok::<Bytes,std::io::Error>(Bytes::from(kiro::fixture_event_frame("toolUseEvent",&json!({"toolUseId":"call","name":"lookup","input":"{\"value\":","stop":false}))));
            release.notified().await;
            yield Ok(Bytes::from(kiro::fixture_event_frame("toolUseEvent",&json!({"toolUseId":"call","name":"lookup","input":"7}","stop":true}))));
            yield Ok(Bytes::from(kiro::fixture_event_frame("endEvent",&json!({}))));
        };
        Response::new(Body::from_stream(stream))
    }}));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = forwarder_test_state("kiro-incremental");
    let provider = install_kiro_test_provider_for_app(
        &state,
        AppKind::Claude,
        "kiro-incremental",
        format!("http://{address}"),
        "http://127.0.0.1:9/token".into(),
    )
    .await;
    let response=forward_for_test_surface(state,ProxyRoute::ClaudeMessages,provider,None,HeaderMap::new(),Bytes::from(json!({"model":"claude-sonnet-4-8","stream":true,"messages":[{"role":"user","content":"lookup"}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]}).to_string())).await.unwrap();
    let mut stream = response.into_body().into_data_stream();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(chunk) = stream.next().await {
            text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
            if text.contains("input_json_delta") {
                break;
            }
        }
    })
    .await
    .expect("arguments must arrive while upstream is waiting");
    assert!(
        text.contains("input_json_delta") && !text.contains("content_block_stop"),
        "{text}"
    );
    release.notify_one();
    while let Some(chunk) = stream.next().await {
        text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    assert!(text.contains("message_stop"));
    assert_eq!(
        text.matches("\"content_block\":{\"type\":\"tool_use\"")
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn kiro_enhancement_cancelled_hosted_request_releases_account_guard() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let handler_entered = entered.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move || {
        let entered = handler_entered.clone();
        async move {
            entered.notify_one();
            std::future::pending::<Response>().await
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = forwarder_test_state("kiro-cancel-hosted");
    let provider = install_kiro_test_provider_for_app(
        &state,
        AppKind::Claude,
        "kiro-cancel-hosted",
        format!("http://{address}"),
        "http://127.0.0.1:9/token".into(),
    )
    .await;
    let response=forward_for_test_surface(state.clone(),ProxyRoute::ClaudeMessages,provider,None,HeaderMap::new(),Bytes::from(json!({"model":"claude-sonnet-4-8","stream":true,"messages":[{"role":"user","content":"search"}],"tools":[{"type":"web_search_20250305","name":"web_search"}]}).to_string())).await.unwrap();
    let reader = tokio::spawn(async move { collect_response_body(response).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        state
            .account_in_flight
            .snapshot()
            .current(ProviderType::KiroOAuth, "kiro-cancel-hosted-account"),
        1
    );
    reader.abort();
    let _ = reader.await;
    assert_eq!(
        state
            .account_in_flight
            .snapshot()
            .current(ProviderType::KiroOAuth, "kiro-cancel-hosted-account"),
        0
    );
    server.abort();
}

#[tokio::test]
async fn kiro_enhancement_structured_output_uses_requested_schema_on_all_surfaces() {
    let (address, server) = spawn_kiro_eventstream_upstream(kiro_text_eventstream(
        "preface {\"ok\":false}\nfinal {\"ok\":true}",
    ))
    .await;
    for (index, route) in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
    ]
    .into_iter()
    .enumerate()
    {
        let state = forwarder_test_state(&format!("kiro-structured-{index}"));
        let provider = install_kiro_test_provider_for_app(
            &state,
            route.app(),
            "kiro-structured",
            format!("http://{address}"),
            "http://127.0.0.1:9/token".into(),
        )
        .await;
        for streaming in [false, true] {
            let format = json!({"type":"json_schema","name":"answer","schema":{"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false}});
            let mut body = match route {
                ProxyRoute::CodexResponses => {
                    json!({"model":"claude-sonnet-4-8","input":"answer","text":{"format":format}})
                }
                ProxyRoute::CodexChatCompletions => {
                    json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"answer"}],"response_format":{"type":"json_schema","json_schema":format}})
                }
                _ => {
                    json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"answer"}],"output_config":{"format":format}})
                }
            };
            body["stream"] = json!(streaming);
            let response = forward_for_test_surface(
                state.clone(),
                route,
                provider.clone(),
                None,
                HeaderMap::new(),
                Bytes::from(body.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let text = String::from_utf8(collect_response_body(response).await).unwrap();
            assert!(
                !text.contains("preface") && !text.contains("false"),
                "{route:?} stream={streaming}: {text}"
            );
            assert!(text.contains("true"), "{text}");
        }
    }
    server.abort();
}

#[tokio::test]
async fn kiro_enhancement_model_and_mcp_share_one_forced_refresh() {
    let counts = Arc::new(std::sync::Mutex::new([0_usize; 3]));
    let handler_counts = counts.clone();
    let app=axum::Router::new().fallback(axum::routing::post(move |uri:axum::http::Uri,headers:HeaderMap|{let counts=handler_counts.clone();async move {
        if uri.path()=="/token" {counts.lock().unwrap()[0]+=1;return axum::Json(json!({"accessToken":"kiro-refreshed","refreshToken":"kiro-rotated","expiresIn":3600})).into_response();}
        if uri.path()=="/mcp" {counts.lock().unwrap()[2]+=1;assert_eq!(headers.get("authorization").unwrap(),"Bearer kiro-refreshed");return StatusCode::UNAUTHORIZED.into_response();}
        counts.lock().unwrap()[1]+=1;if headers.get("authorization").unwrap()=="Bearer kiro-old-access" {return StatusCode::UNAUTHORIZED.into_response();}
        let mut frames=kiro::fixture_event_frame("toolUseEvent",&json!({"toolUseId":"search","name":kiro::client_features::SEARCH_TOOL,"input":"{\"query\":\"news\"}","stop":true}));frames.extend(kiro::fixture_event_frame("endEvent",&json!({})));Response::new(Body::from(frames))
    }}));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = forwarder_test_state("kiro-shared-refresh");
    let provider = install_kiro_test_provider_for_app(
        &state,
        AppKind::Claude,
        "kiro-shared-refresh",
        format!("http://{address}"),
        format!("http://{address}/token"),
    )
    .await;
    let error=forward_for_test_surface(state.clone(),ProxyRoute::ClaudeMessages,provider,None,HeaderMap::new(),Bytes::from(json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"search"}],"tools":[{"type":"web_search_20250305","name":"web_search"}]}).to_string())).await.unwrap_err();
    assert_eq!(error.status, StatusCode::UNAUTHORIZED);
    assert_eq!(*counts.lock().unwrap(), [1, 2, 1]);
    assert_eq!(
        state
            .account_in_flight
            .snapshot()
            .current(ProviderType::KiroOAuth, "kiro-shared-refresh-account"),
        0
    );
    server.abort();
}
