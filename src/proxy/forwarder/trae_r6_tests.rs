const TRAE_R6_MODEL: &str = "DeepSeek-V4-Pro-Official";
// Exact synthetic dialect from Hub f7b7578 test_issue_solo_groups.py; this is
// independent of our decoder implementation, not independent vendor evidence.
const TRAE_R6_REFERENCE_DIALECT: &str = "event:progress_notice\ndata:{\"response\":\"fixture \"}\ndata:{\"response\":\"reply\",\"finish_reason\":\"stop\",\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}\ndata:[DONE]\n";

async fn trae_r6_origin() -> (
    String,
    Arc<StdMutex<Vec<Value>>>,
    tokio::task::JoinHandle<()>,
) {
    let (origin, observed, _, task) = trae_r6_origin_mode("normal").await;
    (origin, observed, task)
}

async fn trae_r6_origin_mode(
    mode: &'static str,
) -> (
    String,
    Arc<StdMutex<Vec<Value>>>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use axum::response::IntoResponse;
    let observed = Arc::new(StdMutex::new(Vec::new()));
    let captured = observed.clone();
    let refreshes = Arc::new(AtomicUsize::new(0));
    let refresh_count = refreshes.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |uri: http::Uri, headers: HeaderMap, body: Bytes| {
        let captured = captured.clone();
        let refresh_count = refresh_count.clone();
        async move {
            let efforts = if mode == "limited" { json!(["low"]) } else { json!(["low","medium","high","xhigh"]) };
            let model = json!({"config_name":TRAE_R6_MODEL,"model_name":TRAE_R6_MODEL,
                "display_config":{"display_name":"R6 fixture","model_capability":"reasoning_model"},
                "reasoning_effort_config":{"support_thinking":true,"options":efforts,"default_level":if mode == "limited" {"low"} else {"high"}}});
            if uri.path() == crate::domain::trae::TRAE_MODEL_DETAIL_PATH {
                return axum::Json(json!({"config_info_list":[model]})).into_response();
            }
            if uri.path() == "/api/ide/v1/model_list" {
                return axum::Json(json!({"model_configs":[model]})).into_response();
            }
            if uri.path() == crate::domain::trae::TRAE_EXCHANGE_TOKEN_PATH {
                assert_eq!(mode,"401");
                refresh_count.fetch_add(1,Ordering::SeqCst);
                return axum::Json(json!({"Code":0,"Result":{"Token":"synthetic-access-refreshed","RefreshToken":"synthetic-refresh-rotated","TokenExpireAt":4102444800000_i64,"RefreshExpireAt":4133980800000_i64}})).into_response();
            }
            if uri.path() == crate::domain::trae::TRAE_USER_INFO_PATH {
                assert_eq!(mode,"401");
                return axum::Json(json!({"Code":0,"Result":{"UserID":"fixture-uid","ScreenName":"Fixture","EnterpriseID":""}})).into_response();
            }
            assert!(uri.path() == crate::domain::trae::TRAE_CHAT_PATH || uri.path() == "/api/ide/v1/llm_raw_chat", "unexpected loopback operation: {uri}");
            let index = {
                let mut sent = captured.lock().unwrap();
                let index = sent.len();
                sent.push(serde_json::from_slice::<Value>(&body).unwrap());
                index
            };
            if mode == "401" {
                let expected = if index == 0 {"synthetic-access"} else {"synthetic-access-refreshed"};
                assert!(headers.get("authorization").and_then(|v| v.to_str().ok()).is_some_and(|v| v.ends_with(expected)),"fixed synthetic account token must match the refresh phase");
                if index == 0 {
                    return (StatusCode::UNAUTHORIZED,axum::Json(json!({"code":401,"message":"synthetic token expired"}))).into_response();
                }
            }
            if mode == "dialect" {
                assert_eq!(headers.get("x-ide-version").and_then(|v|v.to_str().ok()),Some(crate::domain::trae::TRAE_IDE_VERSION));
                return ([(CONTENT_TYPE,"text/event-stream")],TRAE_R6_REFERENCE_DIALECT).into_response();
            }
            ([(CONTENT_TYPE, "text/event-stream")], "event: output\ndata: {\"response\":\"R6_OK\"}\n\nevent: token_usage\ndata: {\"prompt_tokens\":2,\"completion_tokens\":3}\n\nevent: done\ndata: {\"finish_reason\":\"stop\"}\n\n").into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (origin, observed, refreshes, task)
}

fn trae_r6_input(route: ProxyRoute, stream: bool, control: &str) -> (Option<String>, Bytes) {
    let (path, bytes) = r5_timing_input(route, TRAE_R6_MODEL, stream);
    let mut value: Value = serde_json::from_slice(&bytes).unwrap();
    match control {
        "off" => match route {
            ProxyRoute::ClaudeMessages => value["thinking"] = json!({"type":"disabled"}),
            ProxyRoute::Gemini => {
                value["generationConfig"] = json!({"thinkingConfig":{"thinkingBudget":0}})
            }
            ProxyRoute::CodexResponses => value["reasoning"] = json!({"effort":"none"}),
            _ => value["reasoning_effort"] = json!("none"),
        },
        "off_alias" => value["reasoning_effort"] = json!("off"),
        "off_bool" => value["enable_thinking"] = json!(false),
        "high" => match route {
            ProxyRoute::ClaudeMessages => {
                value["thinking"] = json!({"type":"enabled","effort":"high"})
            }
            ProxyRoute::Gemini => {
                value["generationConfig"] = json!({"thinkingConfig":{"thinkingLevel":"high"}})
            }
            ProxyRoute::CodexResponses => value["reasoning"] = json!({"effort":"high"}),
            _ => value["reasoning_effort"] = json!("high"),
        },
        "on_bool" => value["enable_thinking"] = json!(true),
        "bad" => value["enable_thinking"] = json!("fixture-private-marker"),
        "conflict" => {
            value["enable_thinking"] = json!(false);
            value["reasoning_effort"] = json!("high");
        }
        "omitted" => {}
        _ => unreachable!(),
    }
    (path, Bytes::from(value.to_string()))
}

async fn trae_r6_forward(
    state: ServerState,
    route: ProxyRoute,
    id: &str,
    path: Option<String>,
    body: Bytes,
) -> Result<Response, ProxyError> {
    let attempt = claude_timing_attempt(&state, route, id).await;
    forward_with_attempt(state, route, path, HeaderMap::new(), body, attempt).await
}

#[tokio::test]
async fn trae_r6_explicit_off_is_rejected_before_generation_on_all_rails_and_surfaces() {
    for kind in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
            ProxyRoute::Gemini,
        ] {
            for stream in [false, true] {
                for control in ["off", "off_alias", "off_bool"] {
                    let (origin, captured, server) = trae_r6_origin().await;
                    let state = forwarder_test_state("trae-r6-off");
                    let id = r5_timing_install_trae(&state, kind, route.app(), &origin).await;
                    let (path, body) = trae_r6_input(route, stream, control);
                    match trae_r6_forward(state.clone(), route, &id, path, body).await {
                        Err(error) => {
                            assert_eq!(error.status, StatusCode::BAD_REQUEST);
                            assert!(
                                error.message.contains("explicit reasoning disable"),
                                "{}",
                                error.message
                            );
                        }
                        Ok(response) => {
                            drop(collect_response_body(response).await);
                            panic!("{kind:?} {route:?} stream={stream} {control}: explicit off generated with payload {:?}", captured.lock().unwrap());
                        }
                    }
                    assert!(captured.lock().unwrap().is_empty());
                    let usage = state.usage_snapshot().await;
                    let log = usage
                        .logs
                        .last()
                        .expect("unsupported prepared control must be visible");
                    assert_eq!(log.requested_reasoning_effort.as_deref(), Some("none"));
                    assert_eq!(log.effective_reasoning_effort, None);
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn trae_r6_requested_and_effective_diagnostics_match_final_wire_controls() {
    for kind in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
            ProxyRoute::Gemini,
        ] {
            for stream in [false, true] {
                for (control, expected) in [
                    ("omitted", None),
                    ("high", Some("high")),
                    ("on_bool", Some("medium")),
                    ("bad", None),
                    ("conflict", None),
                ] {
                    let (origin, captured, server) = trae_r6_origin().await;
                    let state = forwarder_test_state("trae-r6-diagnostics");
                    let id = r5_timing_install_trae(&state, kind, route.app(), &origin).await;
                    let (path, body) = trae_r6_input(route, stream, control);
                    let result = trae_r6_forward(state.clone(), route, &id, path, body).await;
                    if matches!(control, "bad" | "conflict") {
                        let error = result.unwrap_err();
                        assert_eq!(error.status, StatusCode::BAD_REQUEST);
                        assert!(!error.message.contains("fixture-private-marker"));
                        assert!(captured.lock().unwrap().is_empty());
                    } else {
                        let response = result.unwrap();
                        assert_eq!(response.status(), StatusCode::OK);
                        let bytes = collect_response_body(response).await;
                        assert!(String::from_utf8_lossy(&bytes).contains("R6_OK"));
                        {
                            let sent = captured.lock().unwrap();
                            assert_eq!(sent.len(), 1);
                            let wire = if kind == ProviderType::TraeSolo {
                                sent[0].get("reasoning_effort_level")
                            } else {
                                sent[0].pointer("/parameters/reasoning_effort_level")
                            };
                            assert_eq!(wire.and_then(Value::as_str), expected);
                        }
                        let usage = state.usage_snapshot().await;
                        let log = usage.logs.last().unwrap();
                        assert_eq!(
                            log.requested_reasoning_effort.as_deref(),
                            expected,
                            "{kind:?} {route:?} {control}"
                        );
                        assert_eq!(
                            log.effective_reasoning_effort.as_deref(),
                            expected,
                            "{kind:?} {route:?} {control}"
                        );
                    }
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn trae_r6_diagnostics_survive_one_same_identity_auth_refresh() {
    for kind in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
            ProxyRoute::Gemini,
        ] {
            for stream in [false, true] {
                let (origin, captured, refreshes, server) = trae_r6_origin_mode("401").await;
                let state = forwarder_test_state("trae-r6-401");
                let id = r5_timing_install_trae(&state, kind, route.app(), &origin).await;
                let (before, _) = state
                    .find_account_for_provider_with_incarnation(kind, "r5-trae-account")
                    .await
                    .unwrap();
                let (path, body) = trae_r6_input(route, stream, "high");
                let response = trae_r6_forward(state.clone(), route, &id, path, body)
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                assert!(
                    String::from_utf8_lossy(&collect_response_body(response).await)
                        .contains("R6_OK")
                );
                assert_eq!(refreshes.load(Ordering::SeqCst), 1);
                {
                    let sent = captured.lock().unwrap();
                    assert_eq!(sent.len(), 2);
                    assert_eq!(
                        sent[0], sent[1],
                        "authentication recovery must retain the same prepared controls"
                    );
                }
                let (account, _) = state
                    .find_account_for_provider_with_incarnation(kind, "r5-trae-account")
                    .await
                    .unwrap();
                assert_eq!(
                    account.auth_identity_generation,
                    before.auth_identity_generation
                );
                assert_eq!(
                    account.token_refresh_generation,
                    before.token_refresh_generation + 1
                );
                assert_eq!(account.raw.as_ref().unwrap()["testTraeAgentOrigin"], origin);
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().unwrap();
                assert_eq!(log.requested_reasoning_effort.as_deref(), Some("high"));
                assert_eq!(log.effective_reasoning_effort.as_deref(), Some("high"));
                assert_eq!(log.attempt_count, 2);
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn trae_r6_unsent_unsupported_level_has_no_invented_effective_value() {
    // Only SOLO's reviewed catalog constrains effort levels. The raw catalog
    // confirms exact model names while its effort record is syntax-only.
    for kind in [ProviderType::TraeSolo] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
            ProxyRoute::Gemini,
        ] {
            for stream in [false, true] {
                let (origin, captured, _, server) = trae_r6_origin_mode("limited").await;
                let state = forwarder_test_state("trae-r6-unsupported-level");
                let id = r5_timing_install_trae(&state, kind, route.app(), &origin).await;
                let (path, body) = trae_r6_input(route, stream, "high");
                let error = trae_r6_forward(state.clone(), route, &id, path, body)
                    .await
                    .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_REQUEST);
                assert!(error.message.contains("requested reasoning level"));
                assert!(captured.lock().unwrap().is_empty());
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().unwrap();
                assert_eq!(log.requested_reasoning_effort.as_deref(), Some("high"));
                assert_eq!(log.effective_reasoning_effort, None);
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn trae_r6_reference_solo_dialect_does_not_become_unproven_success() {
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for stream in [false, true] {
            let (origin, captured, _, server) = trae_r6_origin_mode("dialect").await;
            let state = forwarder_test_state("trae-r6-reference-dialect");
            let id =
                r5_timing_install_trae(&state, ProviderType::TraeSolo, route.app(), &origin).await;
            let (path, body) = trae_r6_input(route, stream, "omitted");
            let error = trae_r6_forward(state, route, &id, path, body)
                .await
                .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_GATEWAY);
            assert_eq!(
                captured.lock().unwrap().len(),
                1,
                "no generation replay or alternate function"
            );
            server.abort();
        }
    }
}

#[tokio::test]
async fn trae_r6_solo_function_entitlement_and_image_boundaries_stay_closed() {
    const PNG: &str="iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=";
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for stream in [false, true] {
            for case in ["fixed_function", "unknown_group_model", "image"] {
                let (origin, captured, server) = trae_r6_origin().await;
                let state = forwarder_test_state("trae-r6-capability-boundaries");
                let id =
                    r5_timing_install_trae(&state, ProviderType::TraeSolo, route.app(), &origin)
                        .await;
                let (mut path, body) = trae_r6_input(route, stream, "omitted");
                let mut value: Value = serde_json::from_slice(&body).unwrap();
                match case {
                    "fixed_function" => value["function"] = json!("solo_agent_remote"),
                    "unknown_group_model" => {
                        value["model"] = json!("qwen3.8-flash");
                        path = path.map(|s| s.replace(TRAE_R6_MODEL, "qwen3.8-flash"));
                    }
                    "image" => match route {
                        ProxyRoute::ClaudeMessages => {
                            value["messages"][0]["content"] = json!([{"type":"image","source":{"type":"base64","media_type":"image/png","data":PNG}}])
                        }
                        ProxyRoute::Gemini => {
                            value["contents"][0]["parts"] =
                                json!([{"inlineData":{"mimeType":"image/png","data":PNG}}])
                        }
                        ProxyRoute::CodexResponses => {
                            value["input"] = json!([{"role":"user","content":[{"type":"input_image","image_url":format!("data:image/png;base64,{PNG}")}]}])
                        }
                        _ => {
                            value["messages"][0]["content"] = json!([{"type":"image_url","image_url":{"url":format!("data:image/png;base64,{PNG}")}}])
                        }
                    },
                    _ => unreachable!(),
                }
                let result =
                    trae_r6_forward(state, route, &id, path, Bytes::from(value.to_string())).await;
                if case == "fixed_function" {
                    let response = result.unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    drop(collect_response_body(response).await);
                    let sent = captured.lock().unwrap();
                    assert_eq!(sent.len(), 1);
                    assert_eq!(sent[0]["function"], crate::domain::trae::TRAE_FUNCTION);
                } else {
                    let error = result.unwrap_err();
                    assert!(error.status.is_client_error());
                    if case == "image" {
                        assert!(error.message.to_ascii_lowercase().contains("image"));
                    }
                    assert!(captured.lock().unwrap().is_empty());
                }
                server.abort();
            }
        }
    }
}
