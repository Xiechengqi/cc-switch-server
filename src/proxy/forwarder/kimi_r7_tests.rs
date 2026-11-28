// Local production-forwarder fixtures; no vendor acceptance is implied.
const KIMI_R7_ACCESS: &str = "e30.eyJ1c2VyX2lkIjoia2ltaS1yZXBsYXktdXNlciJ9.rotated";

fn kimi_r7_business_literal() -> Value {
    json!({"reasoning_effort":42,"thinking":{"type":"disabled"},"max_tokens":0})
}

async fn kimi_r7_profiles(state: &ServerState, label: &str, endpoint: &str) -> String {
    let (id, account_id) = install_kimi_replay_test_provider(state, label, endpoint.into()).await;
    let base = state.providers_snapshot().await.providers[0].clone();
    state
        .mutate_accounts_immediate(|accounts| {
            let account = accounts
                .accounts
                .iter_mut()
                .find(|a| a.id == account_id)
                .unwrap();
            account.raw = Some(json!({"testOAuthTokenUrl":format!("{endpoint}/token")}));
        })
        .await
        .unwrap();
    let mut providers = ProviderStore::default();
    for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
        let mut stored = base.clone();
        stored.app = app;
        stored.resource.profile_id = Some(
            crate::domain::providers::registry::ProfileId::parse(format!(
                "{}.kimi_code",
                app.as_str()
            ))
            .unwrap(),
        );
        stored.provider.settings_config["modelMapping"] =
            json!({"mode":"single","upstreamModel":"k3"});
        providers.providers.push(stored);
    }
    providers
        .rebuild_runtime_index(&state.accounts_snapshot().await)
        .unwrap();
    state.replace_provider_store_for_test(providers).await;
    id
}

async fn kimi_r7_fixture(
    recover: bool,
) -> (
    String,
    Arc<StdMutex<Vec<(HeaderMap, Value)>>>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let seen = Arc::new(StdMutex::new(Vec::new()));
    let tokens = Arc::new(AtomicUsize::new(0));
    let (captured, refreshed) = (seen.clone(), tokens.clone());
    let app = axum::Router::new().fallback(axum::routing::post(move |uri:axum::http::Uri, headers:HeaderMap, body:Bytes| {
        let (captured, refreshed) = (captured.clone(), refreshed.clone());
        async move {
            if uri.path()=="/token" {
                refreshed.fetch_add(1,Ordering::SeqCst);
                return axum::Json(json!({"access_token":KIMI_R7_ACCESS,"refresh_token":"kimi-r7-rotated","expires_in":3600,"token_type":"Bearer"})).into_response();
            }
            let body:Value=serde_json::from_slice(&body).unwrap();
            assert!(uri.path().ends_with("messages") || uri.path().ends_with("chat/completions"), "unexpected Kimi path: {uri}");
            captured.lock().unwrap().push((headers.clone(),body.clone()));
            if recover && headers.get("authorization").and_then(|v|v.to_str().ok())!=Some(concat!("Bearer ","e30.eyJ1c2VyX2lkIjoia2ltaS1yZXBsYXktdXNlciJ9.rotated")) {
                return (StatusCode::UNAUTHORIZED,axum::Json(json!({"error":{"message":"expired fixture token"}}))).into_response();
            }
            let streaming=body["stream"]==true;
            let messages=uri.path().ends_with("messages");
            let payload=if streaming {
                if messages { format!("{CLAUDE_TIMING_CONTENT}{CLAUDE_TIMING_TERMINAL}") }
                else { concat!(
                    "data: {\"id\":\"kimi_r7\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"}}]}\n\n",
                    "data: {\"id\":\"kimi_r7\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":1,\"total_tokens\":2}}\n\n",
                    "data: [DONE]\n\n"
                ).into() }
            } else if messages {
                json!({"id":"kimi_r7","type":"message","role":"assistant","model":"k3","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}).to_string()
            } else {
                json!({"id":"kimi_r7","object":"chat.completion","model":"k3","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}).to_string()
            };
            Response::builder().header(CONTENT_TYPE,if streaming {"text/event-stream"} else {"application/json"}).body(Body::from(payload)).unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, seen, tokens, server)
}

fn kimi_r7_input(route: ProxyRoute, streaming: bool, controls: &Value) -> (Option<String>, Bytes) {
    let (path, body) = r5_timing_input(route, "k3", streaming);
    let mut body: Value = serde_json::from_slice(&body).unwrap();
    for (key, value) in controls.as_object().unwrap() {
        body[key] = value.clone();
    }
    let schema = json!({"type":"object","properties":{"text":{"$ref":"#/$defs/Text"}},"$defs":{"Text":{"type":"string"}},"default":kimi_r7_business_literal()});
    body["tools"] = match route {
        ProxyRoute::ClaudeMessages => json!([{"name":"inspect","input_schema":schema}]),
        ProxyRoute::CodexResponses => {
            json!([{"type":"function","name":"inspect","parameters":schema}])
        }
        ProxyRoute::CodexChatCompletions => {
            json!([{"type":"function","function":{"name":"inspect","parameters":schema}}])
        }
        ProxyRoute::Gemini => {
            json!([{"functionDeclarations":[{"name":"inspect","parameters":schema}]}])
        }
        _ => unreachable!(),
    };
    (path, Bytes::from(body.to_string()))
}

async fn kimi_r7_invalid(cases: Vec<Value>) {
    let (endpoint, seen, tokens, server) = kimi_r7_fixture(false).await;
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for streaming in [false, true] {
            let label = format!("kimi-r7-invalid-{route:?}-{streaming}");
            let state = forwarder_test_state(&label);
            let id = kimi_r7_profiles(&state, &label, &endpoint).await;
            for controls in &cases {
                let (path, body) = kimi_r7_input(route, streaming, controls);
                let result = Box::pin(forward_for_test_surface(
                    state.clone(),
                    route,
                    id.clone(),
                    path,
                    HeaderMap::new(),
                    body,
                ))
                .await;
                let error = result.expect_err(&format!(
                    "accepted invalid source controls: {route:?} {controls}"
                ));
                assert_eq!(
                    error.status,
                    StatusCode::BAD_REQUEST,
                    "{route:?} {controls}: {error:?}"
                );
                assert!(
                    seen.lock().unwrap().is_empty(),
                    "invalid controls reached the network"
                );
                assert_eq!(tokens.load(Ordering::SeqCst), 0);
            }
        }
    }
    server.abort();
}

#[tokio::test]
async fn r7_kimi_conflicting_source_controls_fail_before_network() {
    kimi_r7_invalid(vec![
        json!({"thinking":{"type":"disabled"},"output_config":{"effort":"high"}}),
        json!({"reasoning_effort":"low","reasoning":{"effort":"high"}}),
        json!({"thinking":{"type":"disabled","effort":"max"}}),
        json!({"max_tokens":64,"max_completion_tokens":128}),
        json!({"max_output_tokens":128,"max_completion_tokens":256}),
    ])
    .await;
}

#[tokio::test]
async fn r7_kimi_malformed_source_controls_fail_before_network() {
    kimi_r7_invalid(vec![
        json!({"reasoning_effort":42}),
        json!({"reasoning":{"effort":[]}}),
        json!({"thinking":false}),
        json!({"output_config":[]}),
        json!({"reasoning":null}),
        json!({"thinking":{"type":17}}),
        json!({"thinking":{"effort":"unknown-private-label"}}),
        json!({"reasoning_effort":""}),
        json!({"max_output_tokens":0}),
        json!({"max_completion_tokens":"128"}),
    ])
    .await;
}

#[tokio::test]
async fn r7_kimi_exact_positive_budgets_are_not_approximated() {
    kimi_r7_invalid(vec![
        json!({"thinking":{"type":"enabled","budget_tokens":1024}}),
    ])
    .await;
}

async fn kimi_r7_valid(recover: bool, diagnostics: bool) {
    let (endpoint, seen, tokens, server) = kimi_r7_fixture(recover).await;
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for streaming in [false, true] {
            for (n,(controls,requested,effective)) in [
                (json!({}),None,Some("max")),
                (json!({"thinking":{"type":"disabled"}}),Some("none"),Some("none")),
                (json!({"reasoning_effort":"minimal"}),Some("minimal"),Some("low")),
                (json!({"reasoning":{"effort":"medium"}}),Some("medium"),Some("high")),
                (json!({"output_config":{"effort":"xhigh"}}),Some("xhigh"),Some("max")),
                (json!({"reasoning_effort":"high","thinking":{"type":"enabled"},"output_config":{"effort":"medium"}}),Some("high"),Some("high")),
                (json!({"thinking":{"budget_tokens":0}}),Some("none"),Some("none")),
                (json!({"max_tokens":64,"max_completion_tokens":64,"max_output_tokens":64,"reasoning_effort":"high"}),Some("high"),Some("high")),
            ].into_iter().enumerate() {
                let label=format!("kimi-r7-valid-{route:?}-{streaming}-{recover}-{diagnostics}-{n}");
                let state=forwarder_test_state(&label);
                let id=kimi_r7_profiles(&state,&label,&endpoint).await;
                let (path,body)=kimi_r7_input(route,streaming,&controls);
                let response=Box::pin(forward_for_test_surface(state.clone(),route,id,path,HeaderMap::new(),body)).await.unwrap();
                assert_eq!(response.status(),StatusCode::OK);
                assert!(String::from_utf8_lossy(&collect_response_body(response).await).contains("hello"));
                let observed=std::mem::take(&mut *seen.lock().unwrap());
                assert_eq!(observed.len(),if recover {2} else {1});
                for (headers,wire) in &observed {
                    assert_eq!(headers.get("x-msh-device-id").unwrap(),"kimi-replay-device");
                    assert_eq!(wire["thinking"]["type"],if effective==Some("none") {"disabled"} else {"enabled"},"{route:?} {controls}: {wire}");
                    let effort=if route.app()==AppKind::Claude {wire.pointer("/output_config/effort")} else {wire.pointer("/thinking/effort")};
                    assert_eq!(effort.and_then(Value::as_str),effective.filter(|v|*v!="none"),"{route:?} {controls}: {wire}");
                    assert!(wire.pointer("/thinking/budget_tokens").is_none(),"{route:?} {controls}: {wire}");
                    let default=if route.app()==AppKind::Claude {wire.pointer("/tools/0/input_schema/default")} else {wire.pointer("/tools/0/function/parameters/default")};
                    assert_eq!(default,Some(&kimi_r7_business_literal()),"business controls in schema changed: {wire}");
                }
                if diagnostics {
                    let usage=state.usage_snapshot().await;
                    let log=usage.logs.last().unwrap();
                    assert_eq!(log.requested_reasoning_effort.as_deref(),requested,"{route:?} {controls}");
                    assert_eq!(log.effective_reasoning_effort.as_deref(),effective,"{route:?} {controls}");
                    assert_eq!(log.attempt_count,if recover {2} else {1});
                }
            }
        }
    }
    assert_eq!(tokens.load(Ordering::SeqCst), if recover { 64 } else { 0 });
    server.abort();
}

#[tokio::test]
async fn r7_kimi_valid_controls_survive_all_source_bridges() {
    kimi_r7_valid(false, false).await;
}

#[tokio::test]
async fn r7_kimi_prepared_diagnostics_distinguish_request_from_effective() {
    kimi_r7_valid(false, true).await;
}

#[tokio::test]
async fn r7_kimi_same_account_recovery_keeps_controls_device_and_diagnostics() {
    kimi_r7_valid(true, true).await;
}

#[tokio::test]
async fn r7_kimi_gemini_native_control_aliases_and_dynamic_budget() {
    let (endpoint, seen, tokens, server) = kimi_r7_fixture(false).await;
    let route = ProxyRoute::Gemini;
    for streaming in [false, true] {
        let label = format!("kimi-r7-gemini-native-{streaming}");
        let state = forwarder_test_state(&label);
        let id = kimi_r7_profiles(&state, &label, &endpoint).await;
        for (controls, effort) in [
            (
                json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":-1},"maxOutputTokens":128}}),
                Some("max"),
            ),
            (
                json!({"generation_config":{"thinking_config":{"thinking_budget":0},"max_output_tokens":128}}),
                None,
            ),
            (
                json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"LOW"},"maxOutputTokens":128}}),
                Some("low"),
            ),
            (
                json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"MEDIUM"}},"generation_config":{"thinking_config":{"thinking_level":"high"},"max_output_tokens":128}}),
                Some("high"),
            ),
        ] {
            let (path, body) = kimi_r7_input(route, streaming, &controls);
            let response = Box::pin(forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                path,
                HeaderMap::new(),
                body,
            ))
            .await
            .unwrap();
            assert!(
                String::from_utf8_lossy(&collect_response_body(response).await).contains("hello")
            );
            let observations = std::mem::take(&mut *seen.lock().unwrap());
            assert_eq!(observations.len(), 1);
            let wire = &observations[0].1;
            assert_eq!(
                wire["thinking"]["type"],
                if effort.is_some() {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            assert_eq!(
                wire.pointer("/thinking/effort").and_then(Value::as_str),
                effort
            );
            assert_eq!(wire["max_completion_tokens"], 128);
        }
        for controls in [
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":1024}}}),
            json!({"generation_config":{"thinking_config":{"thinking_budget":-2}}}),
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":"0"}}}),
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":0,"thinkingLevel":"HIGH"}}}),
            json!({"generationConfig":{"thinkingConfig":[]}}),
            json!({"generationConfig":false}),
            json!({"generationConfig":{"maxOutputTokens":128},"generation_config":{"max_output_tokens":256}}),
        ] {
            let (path, body) = kimi_r7_input(route, streaming, &controls);
            let error = Box::pin(forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                path,
                HeaderMap::new(),
                body,
            ))
            .await
            .unwrap_err();
            assert_eq!(
                error.status,
                StatusCode::BAD_REQUEST,
                "{controls}: {error:?}"
            );
            assert!(seen.lock().unwrap().is_empty());
        }
    }
    assert_eq!(tokens.load(Ordering::SeqCst), 0);
    server.abort();
}

#[tokio::test]
async fn r7_kimi_non_k3_efforts_do_not_borrow_k3_alias_equivalence() {
    let (endpoint, seen, tokens, server) = kimi_r7_fixture(false).await;
    let state = forwarder_test_state("kimi-r7-non-k3-efforts");
    let id = kimi_r7_profiles(&state, "kimi-r7-non-k3-efforts", &endpoint).await;
    let mut providers = state.providers_snapshot().await;
    for stored in &mut providers.providers {
        stored.provider.settings_config["modelMapping"]["upstreamModel"] = json!("kimi-for-coding");
    }
    providers
        .rebuild_runtime_index(&state.accounts_snapshot().await)
        .unwrap();
    state.replace_provider_store_for_test(providers).await;
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for streaming in [false, true] {
            let (path, body) = kimi_r7_input(
                route,
                streaming,
                &json!({"reasoning_effort":"high","output_config":{"effort":"medium"}}),
            );
            let error = Box::pin(forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                path,
                HeaderMap::new(),
                body,
            ))
            .await
            .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert!(seen.lock().unwrap().is_empty());
        }
    }
    assert_eq!(tokens.load(Ordering::SeqCst), 0);
    server.abort();
}
