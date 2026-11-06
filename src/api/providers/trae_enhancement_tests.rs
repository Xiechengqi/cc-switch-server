fn trae_test_tool_request(
    route: proxy::ProxyRoute,
    stream: bool,
    history: &[(String, usize)],
    previous: Option<&str>,
) -> Value {
    let model = "DeepSeek-V4-Pro-Official";
    let schema = json!({"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]});
    let mut chat_messages = vec![
        json!({"role":"system","content":"system one"}),
        json!({"role":"system","content":"system two"}),
        json!({"role":"user","content":"look twice"}),
    ];
    for (id, n) in history {
        chat_messages.push(json!({"role":"assistant","content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":"lookup","arguments":format!("{{\"n\":{n}}}")}}]}));
        chat_messages.push(json!({"role":"tool","tool_call_id":id,"content":if *n == 1 { "" } else { "second result" }}));
    }
    match route {
        proxy::ProxyRoute::CodexChatCompletions => {
            json!({"model":model,"stream":stream,"stream_options":{"include_usage":true},"messages":chat_messages,"tools":[{"type":"function","function":{"name":"lookup","parameters":schema}}],"max_completion_tokens":128,"reasoning_effort":"high"})
        }
        proxy::ProxyRoute::CodexResponses => {
            let mut input =
                vec![json!({"role":"user","content":[{"type":"input_text","text":"look twice"}]})];
            for (id, n) in history {
                input.push(json!({"type":"function_call","call_id":id,"name":"lookup","arguments":format!("{{\"n\":{n}}}")}));
                input.push(json!({"type":"function_call_output","call_id":id,"output":if *n==1 { "" } else { "second result" }}));
            }
            let mut request = json!({"model":model,"stream":stream,"instructions":"system one\nsystem two","input":input,"tools":[{"type":"function","name":"lookup","parameters":schema}]});
            request["reasoning"] = json!({"effort":"high"});
            request["max_output_tokens"] = json!(128);
            if let Some(previous) = previous {
                request["previous_response_id"] = json!(previous);
                let (id, n) = history.last().unwrap();
                request["input"] = json!([{"type":"function_call_output","call_id":id,"output":if *n==1 { "" } else { "second result" }}]);
            }
            request
        }
        proxy::ProxyRoute::ClaudeMessages => {
            let mut messages = vec![json!({"role":"user","content":"look twice"})];
            for (id, n) in history {
                messages.push(json!({"role":"assistant","content":[{"type":"tool_use","id":id,"name":"lookup","input":{"n":n}}]}));
                messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":if *n==1 { "" } else { "second result" }}]}));
            }
            json!({"model":model,"stream":stream,"max_tokens":128,"system":[{"type":"text","text":"system one"},{"type":"text","text":"system two"}],"messages":messages,"tools":[{"name":"lookup","input_schema":schema}],"thinking":{"type":"adaptive","effort":"high"}})
        }
        proxy::ProxyRoute::Gemini => {
            let mut contents = vec![json!({"role":"user","parts":[{"text":"look twice"}]})];
            for (id, n) in history {
                contents.push(json!({"role":"model","parts":[{"functionCall":{"id":id,"name":"lookup","args":{"n":n}}}]}));
                contents.push(json!({"role":"user","parts":[{"functionResponse":{"id":id,"name":"lookup","response":{"result":if *n==1 { "" } else { "second result" }}}}]}));
            }
            json!({"contents":contents,"systemInstruction":{"parts":[{"text":"system one"},{"text":"system two"}]},"tools":[{"functionDeclarations":[{"name":"lookup","parameters":schema}]}],"generationConfig":{"maxOutputTokens":128,"thinkingConfig":{"thinkingLevel":"HIGH"}}})
        }
        _ => unreachable!(),
    }
}

fn trae_find_id(value: &Value, prefix: &str) -> Option<String> {
    match value {
        Value::String(value) if value.starts_with(prefix) => Some(value.clone()),
        Value::Object(values) => values
            .values()
            .find_map(|value| trae_find_id(value, prefix)),
        Value::Array(values) => values.iter().find_map(|value| trae_find_id(value, prefix)),
        _ => None,
    }
}
fn trae_downstream_id(bytes: &[u8], stream: bool, prefix: &str) -> Option<String> {
    if !stream {
        return trae_find_id(&serde_json::from_slice::<Value>(bytes).unwrap(), prefix);
    }
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|value| trae_find_id(&value, prefix))
}

#[tokio::test]
async fn trae_four_entrypoints_reject_lost_or_conflicting_controls_before_network_io() {
    for provider_type in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for (app, route, controls) in [
            (
                AppKind::Codex,
                proxy::ProxyRoute::CodexChatCompletions,
                vec![
                    json!({"max_tokens":32,"max_completion_tokens":16}),
                    json!({"response_format":{"type":"json_object"}}),
                    json!({"logprobs":true}),
                    json!({"reasoning_effort":"high","reasoning":{"effort":"low"}}),
                ],
            ),
            (
                AppKind::Codex,
                proxy::ProxyRoute::CodexResponses,
                vec![
                    json!({"max_tokens":32,"max_output_tokens":16}),
                    json!({"response_format":{"type":"json_object"}}),
                    json!({"text":{"format":{"type":"json_schema","schema":{}}}}),
                    json!({"reasoning_effort":"low","reasoning":{"effort":"high"}}),
                ],
            ),
            (
                AppKind::Claude,
                proxy::ProxyRoute::ClaudeMessages,
                vec![
                    json!({"max_tokens":32,"max_output_tokens":16}),
                    json!({"output_config":{"format":{"type":"json_schema","schema":{}}}}),
                    json!({"top_k":20}),
                    json!({"thinking":{"type":"disabled"},"output_config":{"effort":"high"}}),
                    json!({"thinking":{"type":"enabled","budget_tokens":1024}}),
                    json!({"tool_choice":{"type":"unknown"}}),
                    json!({"tool_choice":{"type":"tool"}}),
                    json!({"tool_choice":{"type":"any","disable_parallel_tool_use":"true"}}),
                ],
            ),
            (
                AppKind::Gemini,
                proxy::ProxyRoute::Gemini,
                vec![
                    json!({"generationConfig":{"maxOutputTokens":0}}),
                    json!({"generationConfig":{"maxOutputTokens":32,"max_output_tokens":16}}),
                    json!({"generationConfig":{"responseMimeType":"application/json"}}),
                    json!({"generationConfig":{"responseSchema":{"type":"object"}}}),
                    json!({"generationConfig":{"candidateCount":2}}),
                    json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"HIGH","thinkingBudget":0}}}),
                    json!({"toolConfig":{"functionCallingConfig":{"mode":"INVALID"}}}),
                    json!({"toolConfig":{"functionCallingConfig":{"mode":"VALIDATED"}}}),
                    json!({"toolConfig":{"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["unknown"]}}}),
                    json!({"toolConfig":{"functionCallingConfig":{"mode":"AUTO","allowedFunctionNames":[]}}}),
                ],
            ),
        ] {
            let state = provider_api_test_state(&format!(
                "trae-control-rejection-{provider_type:?}-{route:?}"
            ));
            let dir = state.config_dir.clone();
            let execution = install_trae_channel_execution(
                &state,
                "http://127.0.0.1:9".into(),
                app,
                provider_type,
                true,
            )
            .await;
            for control in controls {
                let mut request = trae_test_tool_request(route, false, &[], None);
                request
                    .as_object_mut()
                    .unwrap()
                    .extend(control.as_object().unwrap().clone());
                let path = (route == proxy::ProxyRoute::Gemini)
                    .then(|| "models/DeepSeek-V4-Pro-Official:generateContent".into());
                let error = Box::pin(proxy::forward_provider_test(
                    state.clone(),
                    route,
                    execution.clone(),
                    path,
                    HeaderMap::new(),
                    Bytes::from(request.to_string()),
                ))
                .await
                .unwrap_err();
                assert_eq!(
                    error.status,
                    StatusCode::BAD_REQUEST,
                    "{provider_type:?} {route:?} {control}: {error:?}"
                );
            }
            drop(state);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[tokio::test]
async fn trae_responses_cache_rejects_unknown_references_and_recreated_accounts() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let captured = observed.clone();
    let router = axum::Router::new().fallback(axum::routing::any(move |request: axum::extract::Request| {
        let captured = captured.clone(); async move {
            let body:Value = serde_json::from_slice(&axum::body::to_bytes(request.into_body(),1024*1024).await.unwrap()).unwrap();
            captured.lock().unwrap().push(body);
            Response::builder().header("content-type","text/event-stream").body(Body::from("event: output\ndata: {\"response\":\"first reply\"}\n\nevent: done\ndata: {}\n\n")).unwrap()
        }
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let state = provider_api_test_state("trae-responses-cache-isolation");
    let dir = state.config_dir.clone();
    let execution = install_trae_channel_execution(
        &state,
        origin,
        AppKind::Codex,
        ProviderType::TraeIntl,
        false,
    )
    .await;
    let route = proxy::ProxyRoute::CodexResponses;
    let first = json!({"model":"exact-native-model","input":"original user input"});
    let response = Box::pin(proxy::forward_provider_test(
        state.clone(),
        route,
        execution.clone(),
        None,
        HeaderMap::new(),
        Bytes::from(first.to_string()),
    ))
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let previous = trae_downstream_id(&bytes, false, "resp_").unwrap();
    assert_eq!(state.previous_response_cache_stats().current_entries, 1);
    let mut follow = json!({"model":"exact-native-model","input":"current user input","previous_response_id":previous});
    let response = Box::pin(proxy::forward_provider_test(
        state.clone(),
        route,
        execution.clone(),
        None,
        HeaderMap::new(),
        Bytes::from(follow.to_string()),
    ))
    .await
    .unwrap();
    axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let flat = observed.lock().unwrap()[1]["messages"].to_string();
    for text in ["original user input", "first reply", "current user input"] {
        assert!(flat.contains(text), "{flat}");
    }
    follow["previous_response_id"] = json!("resp_unknown_fixture");
    let error = Box::pin(proxy::forward_provider_test(
        state.clone(),
        route,
        execution.clone(),
        None,
        HeaderMap::new(),
        Bytes::from(follow.to_string()),
    ))
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    follow["previous_response_id"] = json!(123);
    let error = Box::pin(proxy::forward_provider_test(
        state.clone(),
        route,
        execution.clone(),
        None,
        HeaderMap::new(),
        Bytes::from(follow.to_string()),
    ))
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    follow["previous_response_id"] = json!(previous);
    let account = state
        .find_account_for_provider(ProviderType::TraeIntl, "trae-discovery-account")
        .await
        .unwrap();
    let old_incarnation = state
        .find_account_for_provider_with_incarnation(ProviderType::TraeIntl, &account.id)
        .await
        .unwrap()
        .1;
    let replacement: crate::domain::accounts::store::UpsertAccountInput =
        serde_json::from_value(serde_json::to_value(&account).unwrap()).unwrap();
    state
        .mutate_accounts_immediate(move |accounts| {
            assert!(accounts.delete("trae-discovery-account"));
            accounts.upsert(replacement);
        })
        .await
        .unwrap();
    let new_incarnation = state
        .find_account_for_provider_with_incarnation(ProviderType::TraeIntl, &account.id)
        .await
        .unwrap()
        .1;
    assert_ne!(old_incarnation, new_incarnation);
    let error = Box::pin(proxy::forward_provider_test(
        state.clone(),
        route,
        execution,
        None,
        HeaderMap::new(),
        Bytes::from(follow.to_string()),
    ))
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(observed.lock().unwrap().len(), 2);
    server.abort();
    drop(state);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn trae_queue_timeout_heartbeat_idle_and_downstream_cancellation_are_bounded() {
    use futures_util::StreamExt;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct Dropped(Arc<AtomicBool>);
    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    for mode in [
        "headers",
        "error_body",
        "headers_queue",
        "queue",
        "idle",
        "cancel",
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let dropped = Arc::new(AtomicBool::new(false));
        let upstream_dropped = dropped.clone();
        let router = axum::Router::new().fallback(axum::routing::any(move |request: axum::extract::Request| {
            assert_eq!(request.uri().path(),"/api/ide/v1/llm_raw_chat");
            let guard = Dropped(upstream_dropped.clone());
            async move {
                // Consume the request, so disconnect detection belongs to the
                // response stream rather than unread request-body handling.
                axum::body::to_bytes(request.into_body(),1024*1024).await.unwrap();
                if mode == "headers" {
                    let _guard = guard;
                    std::future::pending::<()>().await;
                    unreachable!();
                }
                if mode == "headers_queue" {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                }
                let stream = async_stream::stream! {
                    let _guard = guard;
                    if mode == "error_body" {
                        yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"{"));
                        std::future::pending::<()>().await;
                    } else if mode == "queue" || mode == "headers_queue" {
                        loop {
                            yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"event: progress_notice\ndata: \"Processing_InQueue\"\n\n"));
                            tokio::time::sleep(Duration::from_millis(25)).await;
                        }
                    } else {
                        yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"event: output\ndata: {\"response\":\"first text\"}\n\n"));
                        std::future::pending::<()>().await;
                    }
                };
                Response::builder()
                    .status(if mode == "error_body" { StatusCode::BAD_GATEWAY } else { StatusCode::OK })
                    .header("content-type","text/event-stream").body(Body::from_stream(stream)).unwrap()
            }
        }));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let state = provider_api_test_state(&format!("trae-deadlines-{mode}"));
        let dir = state.config_dir.clone();
        let mut execution = install_trae_channel_execution(
            &state,
            origin,
            AppKind::Codex,
            ProviderType::TraeIntl,
            false,
        )
        .await;
        let plan = Arc::make_mut(&mut execution.plan);
        plan.transport_policy.timeout_ms = 500;
        plan.transport_policy.stream_first_byte_timeout_ms = Some(1000);
        plan.transport_policy.stream_idle_timeout_ms =
            Some(if mode == "idle" { 16_500 } else { 1000 });
        let started = std::time::Instant::now();
        let result = Box::pin(proxy::forward_provider_test(
            state.clone(),
            proxy::ProxyRoute::CodexResponses,
            execution,
            None,
            HeaderMap::new(),
            Bytes::from_static(
                b"{\"model\":\"exact-native-model\",\"input\":\"hello\",\"stream\":true}",
            ),
        ))
        .await;
        if matches!(mode, "headers" | "error_body" | "headers_queue" | "queue") {
            assert_eq!(result.unwrap_err().status, StatusCode::GATEWAY_TIMEOUT);
            assert!(
                started.elapsed() < Duration::from_millis(800),
                "{mode}: {:?}",
                started.elapsed()
            );
        } else {
            let response = result.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let mut stream = response.into_body().into_data_stream();
            let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(String::from_utf8_lossy(&first).contains("first text"));
            if mode == "idle" {
                let mut rest = Vec::new();
                tokio::time::timeout(Duration::from_secs(20), async {
                    while let Some(chunk) = stream.next().await {
                        rest.extend_from_slice(&chunk.unwrap());
                    }
                })
                .await
                .unwrap();
                let rest = String::from_utf8_lossy(&rest);
                assert!(
                    rest.contains("keep-alive") || rest.contains("keepalive"),
                    "{rest}"
                );
                assert!(rest.contains("response.failed"), "{rest}");
                assert!(!rest.contains("response.completed"), "{rest}");
                assert!(started.elapsed() < Duration::from_secs(19));
            }
            drop(stream);
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            while !dropped.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(state.previous_response_cache_stats().current_entries, 0);
        server.abort();
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[tokio::test]
async fn trae_raw_images_survive_all_four_client_bridges_and_solo_rejects_them() {
    for provider_type in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for (app, route, request, invalid_pointer) in [
            (
                AppKind::Codex,
                proxy::ProxyRoute::CodexChatCompletions,
                json!({"model":"DeepSeek-V4-Pro-Official","messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]}),
                "/messages/0/content/1/image_url/url",
            ),
            (
                AppKind::Codex,
                proxy::ProxyRoute::CodexResponses,
                json!({"model":"DeepSeek-V4-Pro-Official","input":[{"role":"user","content":[{"type":"input_text","text":"describe"},{"type":"input_image","image_url":"data:image/png;base64,AA=="}]}]}),
                "/input/0/content/1/image_url",
            ),
            (
                AppKind::Claude,
                proxy::ProxyRoute::ClaudeMessages,
                json!({"model":"DeepSeek-V4-Pro-Official","max_tokens":32,"messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}]}),
                "/messages/0/content/1/source/data",
            ),
            (
                AppKind::Gemini,
                proxy::ProxyRoute::Gemini,
                json!({"contents":[{"role":"user","parts":[{"text":"describe"},{"inlineData":{"mimeType":"image/png","data":"AA=="}}]}]}),
                "/contents/0/parts/1/inlineData/data",
            ),
            (
                AppKind::Codex,
                proxy::ProxyRoute::CodexChatCompletions,
                json!({"model":"DeepSeek-V4-Pro-Official","messages":[{"role":"assistant","content":null,"tool_calls":[{"id":"c","type":"function","function":{"name":"picture","arguments":"{}"}}]},{"role":"tool","tool_call_id":"c","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}]}),
                "/messages/1/content/0/image_url/url",
            ),
            (
                AppKind::Codex,
                proxy::ProxyRoute::CodexResponses,
                json!({"model":"DeepSeek-V4-Pro-Official","input":[{"type":"function_call","call_id":"c","name":"picture","arguments":"{}"},{"type":"function_call_output","call_id":"c","output":[{"type":"input_image","image_url":"data:image/png;base64,AA=="}]}]}),
                "/input/1/output/0/image_url",
            ),
            (
                AppKind::Claude,
                proxy::ProxyRoute::ClaudeMessages,
                json!({"model":"DeepSeek-V4-Pro-Official","max_tokens":32,"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"c","name":"picture","input":{}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"c","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}]}]}),
                "/messages/1/content/0/content/0/source/data",
            ),
            (
                AppKind::Gemini,
                proxy::ProxyRoute::Gemini,
                json!({"contents":[{"role":"model","parts":[{"functionCall":{"id":"c","name":"picture","args":{}}}]},{"role":"user","parts":[{"functionResponse":{"id":"c","name":"picture","response":{"ok":true},"parts":[{"inlineData":{"mimeType":"image/png","data":"AA=="}}]}}]}]}),
                "/contents/1/parts/0/functionResponse/parts/0/inlineData/data",
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let generated = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let captured = generated.clone();
            let router = axum::Router::new().fallback(axum::routing::any(move |request:axum::extract::Request| {
                let captured=captured.clone(); async move {
                    let path=request.uri().path().to_string();
                    let body:Value=serde_json::from_slice(&axum::body::to_bytes(request.into_body(),1024*1024).await.unwrap()).unwrap();
                    if path==crate::domain::trae::TRAE_MODEL_DETAIL_PATH { return Response::new(Body::from(json!({"config_info_list":[{"config_name":"DeepSeek-V4-Pro-Official","display_config":{"display_name":"DeepSeek V4 Pro"}}]}).to_string())); }
                    if path=="/api/ide/v1/model_list" { return Response::new(Body::from(json!({"model_configs":[{"model_name":"DeepSeek-V4-Pro-Official"}]}).to_string())); }
                    captured.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
                    assert!(body["messages"].as_array().unwrap().iter().any(|message| {
                        message["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part == &json!({"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}})))
                    }), "image was lost: {body}");
                    Response::builder().header("content-type","text/event-stream").body(Body::from("event: output\ndata: {\"response\":\"image reply\"}\n\nevent: done\ndata: {}\n\n")).unwrap()
                }
            }));
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let state = provider_api_test_state(&format!("trae-image-{provider_type:?}-{route:?}"));
            let dir = state.config_dir.clone();
            let execution =
                install_trae_channel_execution(&state, origin, app, provider_type, false).await;
            let path = (route == proxy::ProxyRoute::Gemini)
                .then(|| "models/DeepSeek-V4-Pro-Official:generateContent".to_string());
            let result = Box::pin(proxy::forward_provider_test(
                state.clone(),
                route,
                execution.clone(),
                path.clone(),
                HeaderMap::new(),
                Bytes::from(request.to_string()),
            ))
            .await;
            if provider_type == ProviderType::TraeSolo {
                assert_eq!(result.unwrap_err().status, StatusCode::BAD_REQUEST);
            } else {
                let response = result.unwrap_or_else(|error| {
                    panic!("{provider_type:?} {route:?} {invalid_pointer}: {error:?}")
                });
                let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                assert!(String::from_utf8_lossy(&bytes).contains("image reply"));
            }
            let mut invalid = request;
            *invalid.pointer_mut(invalid_pointer).unwrap() = json!("invalid image bytes or URL");
            let error = Box::pin(proxy::forward_provider_test(
                state.clone(),
                route,
                execution,
                path,
                HeaderMap::new(),
                Bytes::from(invalid.to_string()),
            ))
            .await
            .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert_eq!(
                generated.load(std::sync::atomic::Ordering::SeqCst),
                usize::from(provider_type != ProviderType::TraeSolo)
            );
            server.abort();
            drop(state);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}

#[tokio::test]
async fn trae_all_channels_four_protocols_stream_and_nonstream_two_tool_roundtrips() {
    for provider_type in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for (app, route) in [
            (AppKind::Codex, proxy::ProxyRoute::CodexChatCompletions),
            (AppKind::Codex, proxy::ProxyRoute::CodexResponses),
            (AppKind::Claude, proxy::ProxyRoute::ClaudeMessages),
            (AppKind::Gemini, proxy::ProxyRoute::Gemini),
        ] {
            for stream in [false, true] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let origin = format!("http://{}", listener.local_addr().unwrap());
                let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
                let captured = observations.clone();
                let router = axum::Router::new().fallback(axum::routing::any(move |headers: HeaderMap, request: axum::extract::Request| {
                    let captured = captured.clone(); async move {
                        let path = request.uri().path().to_string();
                        let body: Value = serde_json::from_slice(&axum::body::to_bytes(request.into_body(),2*1024*1024).await.unwrap()).unwrap();
                        if path == crate::domain::trae::TRAE_MODEL_DETAIL_PATH { return Response::builder().header("content-type","application/json").body(Body::from(json!({"config_info_list":[{"config_name":"DeepSeek-V4-Pro-Official","display_config":{"display_name":"DeepSeek V4 Pro"},"reasoning_effort_config":{"support_thinking":true,"options":["high"],"default_level":"high"}}]}).to_string())).unwrap(); }
                        if path == "/api/ide/v1/model_list" { return Response::builder().header("content-type","application/json").body(Body::from(json!({"model_configs":[{"model_name":"DeepSeek-V4-Pro-Official"}]}).to_string())).unwrap(); }
                        assert_eq!(path,crate::domain::trae::TraeProtocol::for_provider(provider_type).unwrap().chat_path);
                        let round = { let mut seen=captured.lock().unwrap(); let n=seen.len()+1; seen.push(json!({"body":body,"authorization":headers.get("authorization").unwrap().to_str().unwrap(),"version":headers.get("x-ide-version-code").unwrap().to_str().unwrap(),"ua":headers.get("user-agent").unwrap().to_str().unwrap()})); n };
                        let output = if round <= 2 {
                            if provider_type == ProviderType::TraeSolo { json!({"reasoning_content":"think","tool_calls":[{"index":0,"id":format!("call_fixture_{round}"),"function_call":{"name":"lookup","arguments":format!("{{\"n\":{round}}}")}}]}) }
                            else { json!({"reasoning_content":"think","response":format!("<|FunctionCallBegin|>[{{\"name\":\"lookup\",\"parameters\":{{\"n\":{round}}}}}]<|FunctionCallEnd|>")}) }
                        } else { json!({"response":"all done"}) };
                        let sse = format!("event: progress_notice\ndata: \"Processing_InQueue\"\n\nevent: output\ndata: {output}\n\nevent: token_usage\ndata: {{\"prompt_tokens\":9,\"completion_tokens\":3,\"cache_read_input_tokens\":2,\"reasoning_tokens\":1}}\n\nevent: done\ndata: {{\"finish_reason\":\"stop\"}}\n\n");
                        // Segmentation exercises JSON, UTF8 and marker boundaries through HTTP.
                        let chunks = sse.as_bytes().chunks(17).map(|chunk|Ok::<_,std::convert::Infallible>(Bytes::copy_from_slice(chunk))).collect::<Vec<_>>();
                        Response::builder().header("content-type","text/event-stream").body(Body::from_stream(futures_util::stream::iter(chunks))).unwrap()
                    }
                }));
                let server = tokio::spawn(async move {
                    axum::serve(listener, router).await.unwrap();
                });
                let state = provider_api_test_state(&format!(
                    "trae-tools-{}-{:?}-{stream}",
                    provider_type.as_str(),
                    route
                ));
                let dir = state.config_dir.clone();
                let execution =
                    install_trae_channel_execution(&state, origin, app, provider_type, true).await;
                let mut history = Vec::new();
                let mut previous = None;
                for round in 1..=3 {
                    let mut body =
                        trae_test_tool_request(route, stream, &history, previous.as_deref());
                    match route {
                        proxy::ProxyRoute::CodexChatCompletions
                        | proxy::ProxyRoute::CodexResponses => {
                            body["temperature"] = json!(0.5);
                            body["top_p"] = json!(0.9);
                            body["stop"] = json!(["halt"]);
                            body["tool_choice"] =
                                json!(if round == 3 { "none" } else { "required" });
                            body["parallel_tool_calls"] = json!(false);
                            if round == 3 {
                                if route == proxy::ProxyRoute::CodexResponses {
                                    body["reasoning"] = json!({"effort":"none"});
                                } else {
                                    body["reasoning_effort"] = json!("none");
                                }
                            }
                        }
                        proxy::ProxyRoute::ClaudeMessages => {
                            body["temperature"] = json!(0.5);
                            body["top_p"] = json!(0.9);
                            body["stop_sequences"] = json!(["halt"]);
                            body["tool_choice"] = json!({"type":if round == 3 {"none"} else {"any"},"disable_parallel_tool_use":true});
                            if round == 3 {
                                body["thinking"] = json!({"type":"disabled"});
                            }
                        }
                        proxy::ProxyRoute::Gemini => {
                            body["generationConfig"]["temperature"] = json!(0.5);
                            body["generationConfig"]["topP"] = json!(0.9);
                            body["generationConfig"]["stopSequences"] = json!(["halt"]);
                            body["toolConfig"] = json!({"functionCallingConfig":{"mode":if round == 3 {"NONE"} else {"ANY"}}});
                            if round == 3 {
                                body["generationConfig"]["thinkingConfig"] =
                                    json!({"thinkingBudget":0});
                            }
                        }
                        _ => unreachable!(),
                    }
                    let path = (route == proxy::ProxyRoute::Gemini).then(|| {
                        format!(
                            "/v1beta/models/DeepSeek-V4-Pro-Official:{}",
                            if stream {
                                "streamGenerateContent?alt=sse"
                            } else {
                                "generateContent"
                            }
                        )
                    });
                    let response = Box::pin(proxy::forward_provider_test(
                        state.clone(),
                        route,
                        execution.clone(),
                        path,
                        HeaderMap::new(),
                        Bytes::from(body.to_string()),
                    ))
                    .await
                    .unwrap_or_else(|error| {
                        panic!(
                            "{} {route:?} stream={stream} round={round}: {error:?}",
                            provider_type.as_str()
                        )
                    });
                    assert_eq!(response.status(), StatusCode::OK);
                    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
                        .await
                        .unwrap();
                    let text = String::from_utf8_lossy(&bytes);
                    assert!(!text.contains("event: error"), "{text}");
                    let values: Vec<Value> = if stream {
                        text.lines()
                            .filter_map(|line| line.strip_prefix("data: "))
                            .filter_map(|data| serde_json::from_str(data).ok())
                            .collect()
                    } else {
                        vec![serde_json::from_slice(&bytes).unwrap()]
                    };
                    let (cached_path, reasoning_path) = match route {
                        proxy::ProxyRoute::CodexChatCompletions => (
                            "/usage/prompt_tokens_details/cached_tokens",
                            "/usage/completion_tokens_details/reasoning_tokens",
                        ),
                        proxy::ProxyRoute::CodexResponses if stream => (
                            "/response/usage/input_tokens_details/cached_tokens",
                            "/response/usage/output_tokens_details/reasoning_tokens",
                        ),
                        proxy::ProxyRoute::CodexResponses => (
                            "/usage/input_tokens_details/cached_tokens",
                            "/usage/output_tokens_details/reasoning_tokens",
                        ),
                        proxy::ProxyRoute::ClaudeMessages => (
                            "/usage/cache_read_input_tokens",
                            "/usage/output_tokens_details/thinking_tokens",
                        ),
                        proxy::ProxyRoute::Gemini => (
                            "/usageMetadata/cachedContentTokenCount",
                            "/usageMetadata/thoughtsTokenCount",
                        ),
                        _ => unreachable!(),
                    };
                    assert!(
                        values
                            .iter()
                            .any(|value| value.pointer(cached_path) == Some(&json!(2))),
                        "{provider_type:?} {route:?} missing cache usage: {text}"
                    );
                    assert!(
                        values
                            .iter()
                            .any(|value| value.pointer(reasoning_path) == Some(&json!(1))),
                        "{provider_type:?} {route:?} missing reasoning usage: {text}"
                    );
                    if round <= 2 {
                        assert!(text.contains("think"), "{text}");
                        let id = trae_downstream_id(&bytes, stream, "call_")
                            .unwrap_or_else(|| panic!("missing call id: {text}"));
                        assert!(text.contains("lookup"), "{text}");
                        history.push((id, round));
                    } else {
                        assert!(text.contains("all done"), "{text}");
                    }
                    if route == proxy::ProxyRoute::CodexResponses {
                        previous = Some(
                            trae_downstream_id(&bytes, stream, "resp_")
                                .unwrap_or_else(|| panic!("missing response id: {text}")),
                        );
                    }
                }
                let seen = observations.lock().unwrap();
                assert_eq!(seen.len(), 3);
                for (index, request) in seen.iter().enumerate() {
                    assert_eq!(
                        request["authorization"],
                        format!(
                            "{} trae-discovery-access",
                            crate::domain::trae::TraeProtocol::for_provider(provider_type)
                                .unwrap()
                                .token_type()
                        )
                    );
                    let body = &request["body"];
                    let flat = body["messages"].to_string();
                    if route != proxy::ProxyRoute::CodexResponses || index == 0 {
                        assert!(
                            flat.contains("system one") && flat.contains("system two"),
                            "{flat}"
                        );
                    }
                    if index > 0 {
                        assert!(flat.contains(&history[index - 1].0), "{flat}");
                    }
                    let limit = if provider_type == ProviderType::TraeSolo {
                        &body["max_tokens"]
                    } else {
                        &body["parameters"]["max_tokens"]
                    };
                    assert_eq!(limit, 128);
                    let effort = if provider_type == ProviderType::TraeSolo {
                        &body["reasoning_effort_level"]
                    } else {
                        &body["parameters"]["reasoning_effort_level"]
                    };
                    let expected_effort = if index == 2 {
                        Value::Null
                    } else {
                        json!("high")
                    };
                    assert_eq!(
                        effort, &expected_effort,
                        "{provider_type:?} {route:?}: {body}"
                    );
                    let parameters = if provider_type == ProviderType::TraeSolo {
                        body
                    } else {
                        &body["parameters"]
                    };
                    assert_eq!(parameters["temperature"], 0.5);
                    assert_eq!(parameters["top_p"], 0.9);
                    assert_eq!(parameters["stop"], json!(["halt"]));
                    if provider_type != ProviderType::TraeSolo {
                        assert!(body.get("tools").is_none());
                    }
                    if provider_type == ProviderType::TraeIntl {
                        assert_eq!(request["version"], "20260924");
                        assert_eq!(request["ua"], "TraeClient/TTNet");
                    }
                }
                drop(seen);
                server.abort();
                drop(state);
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }
}

#[tokio::test]
async fn trae_raw_discovery_uses_cn_catalog_and_intl_stops_before_network_io() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let captured = observed.clone();
    let router = axum::Router::new().fallback(axum::routing::any(
        move |request: axum::extract::Request| {
            let captured = captured.clone();
            async move {
                let path = request.uri().path().to_string();
                assert_eq!(path, "/api/ide/v1/model_list");
                assert_eq!(
                    request.headers()["authorization"],
                    "Cloud-IDE-JWT trae-discovery-access"
                );
                assert_eq!(request.headers()["user-agent"], "Trae CN/3.4.3");
                let body: Value = serde_json::from_slice(
                    &axum::body::to_bytes(request.into_body(), 1024)
                        .await
                        .unwrap(),
                )
                .unwrap();
                assert_eq!(body, json!({"type":"chat"}));
                captured.lock().unwrap().push(path);
                axum::Json(json!({"model_configs":[{"model_name":"Exact-Native-Name"}]}))
            }
        },
    ));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for provider_type in [ProviderType::TraeRaw, ProviderType::TraeIntl] {
        let state = provider_api_test_state(&format!("trae-raw-discovery-{provider_type:?}"));
        let dir = state.config_dir.clone();
        let execution = install_trae_channel_execution(
            &state,
            origin.clone(),
            AppKind::Codex,
            provider_type,
            false,
        )
        .await;
        let result = fetch_provider_models_inner(&state, &execution, Some(1000)).await;
        if provider_type == ProviderType::TraeIntl {
            assert_eq!(result.unwrap_err().status, StatusCode::NOT_IMPLEMENTED);
        } else {
            let fetched = result.unwrap();
            assert!(fetched.url.ends_with("/api/ide/v1/model_list"));
            assert_eq!(fetched.models.len(), 1);
            assert_eq!(fetched.models[0].id, "Exact-Native-Name");
            assert_eq!(
                fetched.models[0].raw.pointer("/capabilities/tools"),
                Some(&json!(false))
            );
            assert_eq!(
                fetched.models[0].raw.pointer("/capabilities/images"),
                Some(&json!(true))
            );
        }
        assert_eq!(observed.lock().unwrap().len(), 1);
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }
    server.abort();
}

#[tokio::test]
async fn trae_raw_provider_tests_require_explicit_models_without_global_defaults() {
    for provider_type in [ProviderType::TraeRaw, ProviderType::TraeIntl] {
        for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
            let state =
                provider_api_test_state(&format!("trae-raw-probe-{provider_type:?}-{app:?}"));
            let dir = state.config_dir.clone();
            let execution = install_trae_channel_execution(
                &state,
                "http://127.0.0.1:9".into(),
                app,
                provider_type,
                false,
            )
            .await;
            assert!(execution.plan.test_model.is_none());
            assert_eq!(provider_test_model(app, &execution.stored, None, None), "");
            let mut query = TestProviderQuery {
                app,
                network: Some(true),
                timeout_ms: Some(100),
                model: None,
                test_prompt: None,
                stream: Some(false),
            };
            for model in [None, Some("auto")] {
                query.model = model.map(str::to_string);
                let result = Box::pin(test_provider_inner(&state, execution.clone(), &query))
                    .await
                    .unwrap();
                assert!(!result.ok);
                assert!(!result.network_checked);
                assert_eq!(result.outcome, ProviderOperationOutcome::InvalidConfig);
            }
            query.model = Some("Exact-Native-Name".into());
            query.network = Some(false);
            let result = Box::pin(test_provider_inner(&state, execution, &query))
                .await
                .unwrap();
            assert!(result.ok);
            assert!(!result.network_checked);
            assert_eq!(result.model, "Exact-Native-Name");
            drop(state);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
