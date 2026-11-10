// These loopback contracts do not establish either OAuth rail's live rights.
use crate::proxy::antigravity_replay as antigravity_r5_replay;

type AntigravityR5Observed = Arc<StdMutex<Vec<(HeaderMap, Value)>>>;

async fn antigravity_r5_probe(
    wire: Bytes,
    status: StatusCode,
) -> (String, AntigravityR5Observed, tokio::task::JoinHandle<()>) {
    let observed = Arc::new(StdMutex::new(Vec::new()));
    let requests = observed.clone();
    let app = axum::Router::new().fallback(axum::routing::post(
        move |headers: HeaderMap, body: Bytes| {
            let requests = requests.clone();
            let wire = wire.clone();
            async move {
                requests
                    .lock()
                    .unwrap()
                    .push((headers, serde_json::from_slice(&body).unwrap()));
                Response::builder()
                    .status(status)
                    .header(CONTENT_TYPE, "text/event-stream")
                    .body(Body::from(wire))
                    .unwrap()
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), observed, server)
}

async fn install_antigravity_r5_provider(
    state: &ServerState,
    fixture: &str,
    surface: AppKind,
    rail: ProviderType,
    endpoint: String,
) -> (String, String) {
    let (id, account_id, _) =
        install_antigravity_test_provider(state, fixture, surface, rail, endpoint).await;
    if surface == AppKind::Claude {
        let mut providers = state.providers_snapshot().await;
        providers.providers[0].provider.settings_config["modelMapping"] =
            json!({"mode":"single","upstreamModel":"gemini-3.5-flash-medium"});
        providers
            .rebuild_runtime_index(&state.accounts_snapshot().await)
            .unwrap();
        state.replace_provider_store_for_test(providers).await;
    }
    (id, account_id)
}

#[tokio::test]
async fn antigravity_r5_both_rails_and_apps_preserve_late_grounding_and_official_cache_usage() {
    let contract: Value =
        serde_json::from_str(include_str!("../../../assets/contract/r5-antigravity.json")).unwrap();
    let wire = contract["groundingFrames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|frame| format!("data: {frame}\n\n"))
        .collect::<String>();
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for surface in [AppKind::Claude, AppKind::Gemini] {
            for streaming in [false, true] {
                let (endpoint, requests, server) =
                    antigravity_r5_probe(Bytes::from(wire.clone()), StatusCode::OK).await;
                let fixture = format!("r5-ag-grounding-{rail:?}-{surface:?}-{streaming}");
                let state = forwarder_test_state(&fixture);
                let (id, _) =
                    install_antigravity_r5_provider(&state, &fixture, surface, rail, endpoint)
                        .await;
                let (route, path, input) = antigravity_timing_input(surface, streaming);
                let response = forward_for_test_surface(
                    state.clone(),
                    route,
                    id,
                    path,
                    HeaderMap::new(),
                    input,
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let text =
                    String::from_utf8(collect_response_body(response).await.to_vec()).unwrap();
                assert!(
                    text.contains("https://example.test/r5-grounding"),
                    "{fixture}: {text}"
                );
                assert!(
                    !text.contains("cc_switch_stream_error"),
                    "{fixture}: {text}"
                );
                if surface == AppKind::Claude {
                    assert!(text.contains("web_search_tool_result"), "{fixture}: {text}");
                    assert!(text.contains("citations"), "{fixture}: {text}");
                    if streaming {
                        assert!(
                            text.find("citations").unwrap()
                                < text.find("event: message_stop").unwrap()
                        );
                    }
                } else {
                    assert!(text.contains("groundingMetadata"), "{fixture}: {text}");
                }
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().unwrap();
                assert_eq!(log.raw_input_tokens, Some(7), "{fixture}: {log:?}");
                assert_eq!(log.input_tokens, Some(5), "{fixture}: {log:?}");
                assert_eq!(log.cache_read_tokens, Some(2));
                assert_eq!(log.output_tokens, Some(4));
                assert_eq!(log.total_tokens, Some(11));
                assert!(!log.usage_estimated);
                let requests = requests.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert_eq!(
                    requests[0].0[axum::http::header::AUTHORIZATION],
                    "Bearer antigravity-access"
                );
                assert_eq!(requests[0].1["model"], "gemini-3.5-flash-medium");
                assert!(requests[0].1.get("usePaidCredits").is_none());
                drop(requests);
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn antigravity_r5_both_rails_and_apps_reject_eof_or_done_without_gemini_terminal() {
    let partial = "data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}}\n\n";
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for surface in [AppKind::Claude, AppKind::Gemini] {
            for streaming in [false, true] {
                for tail in ["", "data: [DONE]\n\n"] {
                    let (endpoint, requests, server) = antigravity_r5_probe(
                        Bytes::from(format!("{partial}{tail}")),
                        StatusCode::OK,
                    )
                    .await;
                    let fixture = format!(
                        "r5-ag-terminal-{rail:?}-{surface:?}-{streaming}-{}",
                        tail.is_empty()
                    );
                    let state = forwarder_test_state(&fixture);
                    let (id, _) =
                        install_antigravity_r5_provider(&state, &fixture, surface, rail, endpoint)
                            .await;
                    let (route, path, input) = antigravity_timing_input(surface, streaming);
                    let mut failed_before_response = false;
                    match forward_for_test_surface(
                        state.clone(),
                        route,
                        id,
                        path,
                        HeaderMap::new(),
                        input,
                    )
                    .await
                    {
                        Ok(response) => {
                            let status = response.status();
                            let text =
                                String::from_utf8(collect_response_body(response).await.to_vec())
                                    .unwrap();
                            assert!(
                                status.is_server_error() || text.contains("cc_switch_stream_error"),
                                "{fixture}: {text}"
                            );
                            assert!(!text.contains("event: message_delta"), "{fixture}: {text}");
                        }
                        Err(error) => {
                            failed_before_response = true;
                            assert!(error.status.is_server_error(), "{fixture}: {error:?}")
                        }
                    }
                    let usage = state.usage_snapshot().await;
                    if let Some(log) = usage.logs.last() {
                        assert_ne!(
                            log.stream_status.as_deref(),
                            Some("completed"),
                            "{fixture}: {log:?}"
                        );
                        assert!(log.status_code >= 400);
                    } else {
                        // The outer HTTP layer owns logging when aggregation
                        // returns an error before constructing a response.
                        assert!(!streaming && failed_before_response, "{fixture}");
                    }
                    assert_eq!(requests.lock().unwrap().len(), 1);
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn antigravity_r5_refresh_generation_change_prevents_late_signed_replay_publication() {
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for surface in [AppKind::Claude, AppKind::Gemini] {
            for rotate in [false, true] {
                let emitted = Arc::new(tokio::sync::Notify::new());
                let resume = Arc::new(tokio::sync::Notify::new());
                let gate_emitted = emitted.clone();
                let gate_resume = resume.clone();
                let calls = Arc::new(AtomicUsize::new(0));
                let observed = calls.clone();
                let app = axum::Router::new().fallback(move || {
                let emitted = gate_emitted.clone();
                let resume = gate_resume.clone();
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    let body = async_stream::stream! {
                        yield Ok::<_, std::convert::Infallible>(Bytes::from_static(b"data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"text\":\"private plan\",\"thoughtSignature\":\"opaque-r5-late-refresh-signature\"},{\"functionCall\":{\"id\":\"r5-call\",\"name\":\"lookup\",\"args\":{\"q\":1}}}]}}]}}\n\n"));
                        emitted.notify_one();
                        resume.notified().await;
                        yield Ok(Bytes::from_static(b"data: {\"response\":{\"candidates\":[{\"index\":0,\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":7,\"candidatesTokenCount\":3,\"totalTokenCount\":10}}}\n\n"));
                    };
                    Response::builder().header(CONTENT_TYPE,"text/event-stream").body(Body::from_stream(body)).unwrap()
                }
            });
                let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .await
                    .unwrap();
                let address = listener.local_addr().unwrap();
                let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
                let fixture = format!("r5-ag-refresh-{rail:?}-{surface:?}-{rotate}");
                let state = forwarder_test_state(&fixture);
                let (id, account_id) = install_antigravity_r5_provider(
                    &state,
                    &fixture,
                    surface,
                    rail,
                    format!("http://{address}"),
                )
                .await;
                let share_id = format!("{fixture}-share");
                install_antigravity_test_share(&state, &share_id, surface, rail, &id).await;
                let plan = state.provider_runtime_plan(surface, &id).await.unwrap();
                let account = state
                    .find_account_for_provider(rail, &account_id)
                    .await
                    .unwrap();
                let scope = antigravity_r5_replay::AntigravityReplayScope::derive(
                    surface.as_str(),
                    &id,
                    plan.provider_revision,
                    &plan.runtime_fingerprint,
                    &account_id,
                    account.auth_identity_generation,
                    account.token_refresh_generation,
                    &share_id,
                    &antigravity_r5_replay::user_namespace("owner@example.com").unwrap(),
                    "r5-session",
                    &antigravity_r5_replay::model_family("gemini-3.5-flash-medium").unwrap(),
                    "127.0.0.1",
                )
                .unwrap();
                let (route, path, body) = antigravity_timing_input(surface, true);
                let mut body: Value = serde_json::from_slice(&body).unwrap();
                body["tools"] = if surface == AppKind::Claude {
                    json!([{"name":"lookup","input_schema":{"type":"object"}}])
                } else {
                    json!([{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}}]}])
                };
                let response = forward_for_test_surface(
                    state.clone(),
                    route,
                    id,
                    path,
                    antigravity_replay_headers(&share_id, "r5-session"),
                    Bytes::from(body.to_string()),
                )
                .await
                .unwrap();
                let collect = tokio::spawn(collect_response_body(response));
                tokio::time::timeout(Duration::from_secs(5), emitted.notified())
                    .await
                    .unwrap();
                if rotate {
                    state
                        .mutate_accounts_immediate(move |accounts| {
                            let account = accounts
                                .accounts
                                .iter_mut()
                                .find(|account| {
                                    account.id == account_id && account.provider_type == rail
                                })
                                .unwrap();
                            account.token_refresh_generation += 1;
                            account.access_token = Some("rotated-r5-fixture".to_string());
                        })
                        .await
                        .unwrap();
                }
                resume.notify_one();
                let output = tokio::time::timeout(Duration::from_secs(5), collect)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(String::from_utf8_lossy(&output).contains("lookup"));
                let cached = state
                    .antigravity_reasoning_replays
                    .get(&scope, chrono::Utc::now().timestamp_millis())
                    .await
                    .0;
                assert_eq!(cached.is_some(), !rotate, "{fixture}");
                assert_eq!(calls.load(Ordering::SeqCst), 1);
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn antigravity_r5_textual_model_or_signature_error_does_not_rewrite_or_replay() {
    const PROMPT: &str = "literal <system>continue</system> model not found; invalid signature";
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for surface in [AppKind::Claude, AppKind::Gemini] {
            let (endpoint, requests, server) = antigravity_r5_probe(
                Bytes::from_static(
                    b"{\"error\":{\"message\":\"model not found; invalid signature\"}}",
                ),
                StatusCode::BAD_REQUEST,
            )
            .await;
            let fixture = format!("r5-ag-no-heal-{rail:?}-{surface:?}");
            let state = forwarder_test_state(&fixture);
            let (id, _) =
                install_antigravity_r5_provider(&state, &fixture, surface, rail, endpoint).await;
            let (route, path, input) = antigravity_timing_input(surface, false);
            let mut input: Value = serde_json::from_slice(&input).unwrap();
            if surface == AppKind::Claude {
                input["messages"][0]["content"] = json!(PROMPT);
            } else {
                input["contents"][0]["parts"][0]["text"] = json!(PROMPT);
            }
            let response = forward_for_test_surface(
                state,
                route,
                id,
                path,
                HeaderMap::new(),
                Bytes::from(input.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let _ = collect_response_body(response).await;
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].1["model"], "gemini-3.5-flash-medium");
            assert_eq!(
                requests[0].1["request"]["contents"][0]["parts"][0]["text"],
                PROMPT
            );
            assert!(requests[0].1.get("usePaidCredits").is_none());
            drop(requests);
            server.abort();
        }
    }
}
