// Repository-owned loopback receipts, never a vendor live acceptance receipt.
async fn zen_test_options(state: &ServerState, body: Value) -> OpencodeZenForwardOptions {
    let stored = state
        .upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app: AppKind::Codex,
            provider: Provider {
                id: "zen-loopback".into(),
                name: "Zen loopback".into(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            profile_id: Some(
                crate::domain::providers::registry::ProfileId::parse("codex.opencode_zen").unwrap(),
            ),
            custom_binding: None,
            expected_revision: None,
            client_request_id: None,
            credential_patches: Default::default(),
        })
        .await
        .unwrap()
        .unwrap();
    let providers = state.providers_snapshot().await;
    let execution = ProviderExecution::from_store(&providers, stored.clone()).unwrap();
    super::super::opencode_zen::store_catalog(
        vec!["chat-free".to_string()],
        current_time_ms() as i64,
    );
    OpencodeZenForwardOptions {
        state: state.clone(),
        execution,
        stored,
        route: ProxyRoute::CodexChatCompletions,
        body: Bytes::from(body.to_string()),
        request_context: UsageLogContext {
            request_id: Some("zen-receipt".into()),
            ..Default::default()
        },
        account_in_flight_guard: None,
        share_invocation_guard: None,
        started: Instant::now(),
        timing: Default::default(),
        request_memory: None,
    }
}

fn zen_fixture_sse(payload: Value, wire: super::super::opencode_zen::ZenWire) -> Response {
    use super::super::opencode_zen::ZenWire;
    let frame = |v: Value| format!("data: {v}\n\n");
    let body = match wire {
        ZenWire::Chat => [frame(json!({"id":payload["id"],"model":payload["model"],"choices":[{"index":0,"delta":payload["choices"][0]["message"],"finish_reason":payload["choices"][0]["finish_reason"]}],"usage":payload["usage"]})), "data: [DONE]\n\n".into()].concat(),
        ZenWire::Responses => frame(json!({"type":"response.completed","response":payload})),
        ZenWire::Messages => {
            let mut start = payload.clone();
            start["content"] = json!([]);
            start["stop_reason"] = Value::Null;
            let mut events = frame(json!({"type":"message_start","message":start}));
            for (index, block) in payload["content"].as_array().unwrap().iter().enumerate() {
                events.push_str(&frame(json!({"type":"content_block_start","index":index,"content_block":block})));
                events.push_str(&frame(json!({"type":"content_block_stop","index":index})));
            }
            events.push_str(&frame(json!({"type":"message_delta","delta":{"stop_reason":payload["stop_reason"]},"usage":payload["usage"]})));
            events.push_str(&frame(json!({"type":"message_stop"})));
            events
        }
    };
    Response::builder()
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn opencode_zen_loopback_is_incremental_and_unread_bodies_settle_cancellation() {
    let _lock = super::super::opencode_zen::CATALOG_TEST_LOCK.lock().await;
    for unread in [false, true] {
        let state = forwarder_test_state("zen-stream-lifecycle");
        let release = Arc::new(tokio::sync::Notify::new());
        let upstream_release = release.clone();
        let router = axum::Router::new().fallback(axum::routing::post(move |headers: HeaderMap, body: Bytes| {
            let release = upstream_release.clone();
            async move {
                assert_eq!(headers["authorization"], "Bearer public");
                let body: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(body["model"], "chat-free");
                assert!(body["tools"].as_array().unwrap().len() >= 2);
                Response::builder().header(CONTENT_TYPE, "text/event-stream").body(Body::from_stream(async_stream::stream! {
                    yield Ok::<Bytes, std::io::Error>(Bytes::from_static(b"data: {\"id\":\"chat_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"first\"},\"finish_reason\":null}]}\n\n"));
                    release.notified().await;
                    yield Ok(Bytes::from_static(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":2,\"total_tokens\":12}}\n\ndata: [DONE]\n\n"));
                })).unwrap()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let mut options = zen_test_options(
            &state,
            json!({"model":"downstream-alias","stream":true,"messages":[{"role":"user","content":"hi"}]}),
        )
        .await;
        Arc::make_mut(&mut options.execution.plan).model_policy =
            crate::domain::providers::runtime::RuntimeModelPolicy::Single {
                upstream_model: "chat-free".into(),
            };
        let response = forward_opencode_zen_at(options, &origin).await.unwrap();
        if unread {
            drop(response);
        } else {
            let mut stream = response.into_body().into_data_stream();
            let first = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(String::from_utf8_lossy(&first).contains("first"));
            // EOF is still blocked in the mock: first content was delivered live.
            release.notify_one();
            while let Some(chunk) = stream.next().await {
                chunk.unwrap();
            }
            assert_eq!(
                state
                    .usage_snapshot()
                    .await
                    .request_detail("zen-receipt")
                    .unwrap()
                    .stream_status
                    .as_deref(),
                Some("completed")
            );
        }
        if unread {
            for _ in 0..100 {
                if state
                    .usage_snapshot()
                    .await
                    .request_detail("zen-receipt")
                    .unwrap()
                    .stream_status
                    .as_deref()
                    == Some("client_cancelled")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                state
                    .usage_snapshot()
                    .await
                    .request_detail("zen-receipt")
                    .unwrap()
                    .stream_status
                    .as_deref(),
                Some("client_cancelled")
            );
        }
        server.abort();
    }
}

#[tokio::test]
async fn opencode_zen_loopback_continuation_keeps_first_answer_and_sums_usage() {
    let _lock = super::super::opencode_zen::CATALOG_TEST_LOCK.lock().await;
    let state = forwarder_test_state("zen-continuation");
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    state
        .configure_value_quota_router_for_test("zen-installation")
        .await;
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    let scope = ledger::Scope {
        router_url: state
            .config_snapshot()
            .await
            .router_api_base()
            .unwrap()
            .into(),
        installation_id: "zen-installation".into(),
        share_id: "zen-share".into(),
        email: "owner@example.test".into(),
        entitlement_id: None,
    };
    let mut fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    fixture["snapshot"]["models"]["chat-free"] =
        fixture["snapshot"]["models"]["text-model"].clone();
    let snapshot: crate::domain::usage::value_pricing::PricingSnapshot =
        serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    let revision = snapshot.revision().unwrap();
    ledger::install_snapshot(
        &state.config_dir,
        &scope,
        &revision,
        &snapshot,
        current_time_ms() as i64,
    )
    .unwrap();
    ledger::activate_snapshot(&state.config_dir, &scope, &revision).unwrap();

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let router = axum::Router::new().fallback(axum::routing::post(move |uri: axum::http::Uri, body: Bytes| {
        let n = count.fetch_add(1, Ordering::SeqCst);
        async move {
            assert_eq!(uri.path(), "/inference/v1/chat/completions");
            if n == 0 {
                // A concurrent refresh changes the catalog's route; continuation
                // must retain the admitted protocol of the original generation.
                super::super::opencode_zen::store_catalog_payload(&json!({"data":[{
                    "id":"chat-free","type":"chat","cost":{"input":0,"output":0},
                    "api":{"messages":{"base_url":"https://opencode.ai/inference/anthropic/v1"}}
                }]}));
            }
            let sent: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(sent["model"], "chat-free");
            assert_eq!(sent["stream"], true);
            if n == 1 { assert!(sent["messages"].to_string().contains("partial")); }
            zen_fixture_sse(json!({"id":"chat_1","object":"chat.completion","model":"chat-free","choices":[{"index":0,"message":{"role":"assistant","content":if n==0 {"partial"} else {" rest"}},"finish_reason":if n==0 {"length"} else {"stop"}}],"usage":{"prompt_tokens":10,"completion_tokens":100,"total_tokens":110}}), super::super::opencode_zen::ZenWire::Chat)
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut options = zen_test_options(&state, json!({"model":"downstream-alias","stream":false,"max_tokens":2048,"messages":[{"role":"user","content":"hi"}]})).await;
    Arc::make_mut(&mut options.execution.plan).model_policy =
        crate::domain::providers::runtime::RuntimeModelPolicy::Single {
            upstream_model: "chat-free".into(),
        };
    state.mutate_shares_immediate(|store| {
        store.shares.push(serde_json::from_value(json!({
            "id":"zen-share","app":"codex","providerId":"zen-loopback","providerType":"opencode_zen","ownerEmail":"owner@example.test","enabled":true,"status":"active",
            "bindings":[{"app":"codex","providerId":"zen-loopback","providerType":"opencode_zen"}],
            "userGrants":{"owner@example.test":{"email":"owner@example.test","role":"owner","active":true,"policy":{"tokenPeriod":"lifetime","valueQuota":{"limitUsdMicros":"1000000","tariffPolicyId":"api-equivalent-v1"}}}}
        })).unwrap());
    }).await.unwrap();
    options.request_context.share_id = Some("zen-share".into());
    options.request_context.user_email = Some("owner@example.test".into());
    options.request_context.started_at_ms = Some(current_time_ms());
    options.request_context.quota_app = Some(AppKind::Codex);
    let response = forward_opencode_zen_at(options, &origin).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let response: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(response["choices"][0]["message"]["content"], "partial rest");
    assert_eq!(response["usage"]["prompt_tokens"], 20);
    assert_eq!(response["usage"]["completion_tokens"], 200);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let log = state
        .usage_snapshot()
        .await
        .request_detail("zen-receipt")
        .unwrap()
        .clone();
    assert_eq!(log.output_tokens, Some(200));
    assert_eq!(log.input_tokens, Some(20));
    assert_eq!(log.attempt_count, 2);
    assert_eq!(log.value_generation_usage.unwrap().output_tokens, Some(100));
    let db = rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir)).unwrap();
    let mut query = db.prepare("SELECT attempt_id,usage_picos FROM value_quota_attempts WHERE request_id='zen-receipt' ORDER BY attempt_id").unwrap();
    let rows: Vec<(String, String)> = query
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        rows,
        vec![
            ("1".into(), "1530000000".into()),
            ("2".into(), "1530000000".into())
        ]
    );
    assert_eq!(
        state
            .usage_snapshot()
            .await
            .logs
            .iter()
            .filter(|log| log.request_id == "zen-receipt")
            .count(),
        1
    );
    server.abort();
}

#[tokio::test]
async fn opencode_zen_loopback_rate_limit_is_not_replayed_or_leaked() {
    let _lock = super::super::opencode_zen::CATALOG_TEST_LOCK.lock().await;
    let state = forwarder_test_state("zen-quota");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = calls.clone();
    let router = axum::Router::new().fallback(axum::routing::post(move || {
        count.fetch_add(1, Ordering::SeqCst);
        async {
            Response::builder()
                .status(429)
                .header("retry-after", "3")
                .header("x-request-id", "safe-123")
                .body(Body::from("private-upstream-body"))
                .unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let options = zen_test_options(
        &state,
        json!({"model":"chat-free","stream":false,"messages":[]}),
    )
    .await;
    let error = forward_opencode_zen_at(options, &origin).await.unwrap_err();
    assert_eq!(error.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(error
        .client_message()
        .contains("upstream rate_limit: HTTP 429"));
    assert!(!error.client_message().contains("quota"));
    assert!(!error.message.contains("private-upstream-body"));
    assert!(error.message.contains("safe-123"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(error.retry_after_seconds(), Some(3));
    let mut second_request =
        zen_test_options(&state, json!({"model":"chat-free","messages":[]})).await;
    second_request.request_context.request_id = Some("zen-second-receipt".into());
    second_request.request_context.user_email = Some("other@example.test".into());
    assert_eq!(
        forward_opencode_zen_at(second_request, &origin)
            .await
            .unwrap_err()
            .status,
        429
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_ne!(
        state
            .usage_snapshot()
            .await
            .request_detail("zen-receipt")
            .unwrap()
            .stream_status
            .as_deref(),
        Some("completed")
    );
    server.abort();
}

#[test]
fn opencode_zen_continuation_uses_remaining_budget_and_rejects_tools() {
    let request = json!({"model":"chat-free","max_tokens":1000,"messages":[]});
    let first = json!({"choices":[{"message":{"content":"prefix"},"finish_reason":"length"}],"usage":{"completion_tokens":100}});
    let prepared = zen_continuation_request(
        &request,
        ProxyRoute::CodexChatCompletions,
        &super::super::opencode_zen::ZenAdmission {
            endpoint: super::super::opencode_zen::CatalogEndpoint::LegacyChat,
            capabilities: Default::default(),
            stale: false,
            context_window: None,
            max_output_tokens: None,
        },
        super::super::opencode_zen::ContinuationKind::MaxTokens,
        &first,
        tokio::time::Instant::now() + Duration::from_secs(10),
    )
    .unwrap()
    .unwrap()
    .1;
    let payload: Value = serde_json::from_slice(&prepared.body).unwrap();
    assert_eq!(payload["max_completion_tokens"], 900);
    let mut tool = json!({"choices":[{"message":{"content":"","tool_calls":[{"id":"bad"}]}}]});
    assert!(zen_merge_continuation(
        &first,
        &mut tool,
        super::super::opencode_zen::ContinuationKind::MaxTokens
    )
    .is_err());
}

#[tokio::test]
async fn opencode_zen_role_only_stream_cannot_extend_first_semantic_deadline() {
    let _lock = super::super::opencode_zen::CATALOG_TEST_LOCK.lock().await;
    let state = forwarder_test_state("zen-semantic-deadline");
    let router = axum::Router::new().fallback(axum::routing::post(|| async {
        Response::builder().header(CONTENT_TYPE,"text/event-stream").body(Body::from_stream(async_stream::stream! {
            for _ in 0..30 {
                yield Ok::<Bytes,std::io::Error>(Bytes::from_static(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n"));
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })).unwrap()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let mut options = zen_test_options(
        &state,
        json!({"model":"chat-free","stream":true,"messages":[]}),
    )
    .await;
    Arc::make_mut(&mut options.execution.plan)
        .transport_policy
        .stream_first_byte_timeout_ms = Some(100);
    options.started = Instant::now();
    let response = forward_opencode_zen_at(options, &origin).await.unwrap();
    let body = tokio::time::timeout(
        Duration::from_secs(2),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("deadline"));
    // Chat's shared failure encoder terminates its explicit error with [DONE].
    // That delimiter alone is not a successful completion.
    let text = String::from_utf8_lossy(&body);
    let error_at = text.find("\"error\"").expect("explicit stream error");
    assert!(error_at < text.find("[DONE]").unwrap());
    assert!(!text.contains("\"finish_reason\":\"stop\""));
    let log = state
        .usage_snapshot()
        .await
        .request_detail("zen-receipt")
        .unwrap()
        .clone();
    assert_eq!(log.first_token_ms, None);
    assert_ne!(log.stream_status.as_deref(), Some("completed"));
    server.abort();
}

#[tokio::test]
async fn opencode_zen_codex_cli_envelope_uses_formal_forwarder_on_all_wires_and_modes() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    for model in ["chat-free", "muse-spark-free", "union-alpha"] {
        for stream in [false, true] {
            let state = forwarder_test_state("zen-codex-cli-envelope");
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = calls.clone();
            let router = axum::Router::new().fallback(axum::routing::post(move |body: Bytes| {
                let count = count.clone();
                async move {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let sent: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(sent["model"], model);
                    assert_eq!(sent["stream"], true);
                    assert!(sent.get("client_metadata").is_none());
                    assert!(sent.get("prompt_cache_key").is_none());
                    assert!(sent.get("reasoning").is_none());
                    assert!(!sent.to_string().contains("synthetic-client-"));
                    let payload = match zen::wire_for(model) {
                        zen::ZenWire::Chat => json!({"id":"chat_codex","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":"ZEN_CODEX_OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
                        zen::ZenWire::Responses => json!({"id":"resp_codex","object":"response","model":model,"status":"completed","output":[{"id":"msg_codex","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"ZEN_CODEX_OK","annotations":[]}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}),
                        zen::ZenWire::Messages => json!({"id":"msg_codex","type":"message","model":model,"role":"assistant","content":[{"type":"text","text":"ZEN_CODEX_OK"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}),
                    };
                    zen_fixture_sse(payload, zen::wire_for(model))
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let mut request: Value = serde_json::from_str(zen::CODEX_CLI_FIXTURE).unwrap();
            request["stream"] = json!(stream);
            let mut options = zen_test_options(&state, request).await;
            options.route = ProxyRoute::CodexResponses;
            Arc::make_mut(&mut options.execution.plan).model_policy =
                crate::domain::providers::runtime::RuntimeModelPolicy::Single {
                    upstream_model: model.into(),
                };
            zen::store_catalog(vec![model.into()], current_time_ms() as i64);
            let response = forward_opencode_zen_at(options, &origin).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let text = String::from_utf8(bytes.to_vec()).unwrap();
            assert!(text.contains("ZEN_CODEX_OK"), "{model} {stream}: {text}");
            if stream {
                assert!(text.contains("response.completed"));
            } else {
                assert_eq!(
                    serde_json::from_str::<Value>(&text).unwrap()["status"],
                    "completed"
                );
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            let usage = state.usage_snapshot().await;
            let log = usage.request_detail("zen-receipt").unwrap();
            assert_eq!(log.requested_model.as_deref(), Some("codex-client-model"));
            assert_eq!(log.actual_model.as_deref(), Some(model));
            assert_eq!(log.stream_status.as_deref(), Some("completed"));
            server.abort();
        }
    }
}

#[tokio::test]
async fn opencode_zen_fixed_models_choose_upstream_wire_before_conversion() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    for (model, api) in [
        ("chat-free", None),
        ("muse-spark-free", None),
        ("union-alpha", None),
        ("big-pickle", Some("chat-completions")),
        ("new-messages", Some("messages")),
        ("new-responses", Some("responses")),
    ] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
        ] {
            let state = forwarder_test_state("zen-fixed-wire");
            let router = axum::Router::new().fallback(axum::routing::post(move |uri: axum::http::Uri, body: Bytes| async move {
                assert_eq!(uri.path(), zen::endpoint_for(model));
                let sent: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(sent["model"], model);
                assert_eq!(sent["stream"], true);
                let payload = match zen::wire_for(model) {
                    zen::ZenWire::Chat => json!({"id":"chat_test","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":"fixed"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
                    zen::ZenWire::Responses => json!({"id":"resp_test","object":"response","model":model,"status":"completed","output":[{"id":"msg_test","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"fixed","annotations":[]}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}),
                    zen::ZenWire::Messages => json!({"id":"msg_test","type":"message","model":model,"role":"assistant","content":[{"type":"text","text":"fixed"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}),
                };
                zen_fixture_sse(payload, zen::wire_for(model))
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, router).await.unwrap();
            });
            let request = match route {
                ProxyRoute::CodexResponses => {
                    json!({"model":"downstream-alias","input":"hi","stream":false})
                }
                _ => {
                    json!({"model":"downstream-alias","messages":[{"role":"user","content":"hi"}],"max_tokens":128,"stream":false})
                }
            };
            let mut options = zen_test_options(&state, request).await;
            options.route = route;
            Arc::make_mut(&mut options.execution.plan).model_policy =
                crate::domain::providers::runtime::RuntimeModelPolicy::Single {
                    upstream_model: model.into(),
                };
            if let Some(api) = api {
                let base = if api == "messages" {
                    "https://opencode.ai/inference/anthropic/v1"
                } else {
                    "https://opencode.ai/inference/openai/v1"
                };
                zen::store_catalog_payload(
                    &json!({"data":[{"id":model,"cost":{"input":0,"output":0},
                    "api":{api:{"base_url":base}}}]}),
                );
            } else {
                zen::store_catalog(vec![model.to_string()], current_time_ms() as i64);
            }
            let response = forward_opencode_zen_at(options, &origin).await.unwrap();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("fixed"));
            let log = state
                .usage_snapshot()
                .await
                .request_detail("zen-receipt")
                .unwrap()
                .clone();
            assert_eq!(log.requested_model.as_deref(), Some("downstream-alias"));
            assert_eq!(log.actual_model.as_deref(), Some(model));
            server.abort();
        }
    }
}

#[tokio::test]
async fn opencode_zen_catalog_wait_obeys_total_deadline_and_records_preflight_failure_once() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    let state = forwarder_test_state("zen-catalog-deadline");
    let mut options = zen_test_options(&state, json!({"model":"chat-free","messages":[]})).await;
    options.started =
        Instant::now() - options.execution.request_timeout() + Duration::from_millis(40);
    let catalog_flight = zen::lock_catalog_refresh_for_test().await;
    let error = tokio::time::timeout(
        Duration::from_secs(1),
        forward_opencode_zen_at(options, "http://127.0.0.1:9"),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.status, 504);
    assert!(error.message.contains("catalog deadline"));
    drop(catalog_flight);
    let usage = state.usage_snapshot().await;
    let logs: Vec<_> = usage
        .logs
        .iter()
        .filter(|log| log.request_id == "zen-receipt")
        .collect();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        logs[0].failure_kind.as_deref(),
        Some("zen_preflight_rejected")
    );
    assert_eq!(logs[0].usage_state, UsageState::NotApplicable);
}

#[test]
fn opencode_zen_retry_after_accepts_http_date_and_never_echoes_unsafe_identifiers() {
    let mut headers = HeaderMap::new();
    headers.insert(
        "retry-after",
        httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(30))
            .parse()
            .unwrap(),
    );
    headers.insert(
        "x-request-id",
        "unsafe-query?secret=private".parse().unwrap(),
    );
    let error = zen_status_error(StatusCode::TOO_MANY_REQUESTS, &headers);
    assert!(matches!(error.retry_after_seconds(), Some(28..=30)));
    assert!(!error.client_message().contains("private"));
}

#[tokio::test]
async fn opencode_zen_typed_free_errors_preserve_retry_without_private_body() {
    for (kind, category) in [
        ("FreeUsageLimitError", "free_usage_limit"),
        ("FreeTierError", "free_tier_gate"),
        ("RegionError", "region_rejected"),
    ] {
        let raw = http::Response::builder()
            .status(429)
            .header("retry-after", "43200")
            .header("cf-ray", "zen-safe-ray")
            .body(
                json!({"error":{"type":kind,"message":"private credential=do-not-leak"}})
                    .to_string(),
            )
            .unwrap();
        let response: reqwest::Response = raw.into();
        let mut response =
            super::super::opencode_zen::transport::ZenResponse::from_reqwest(response);
        let error = zen_http_error(
            &mut response,
            tokio::time::Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert!(error
            .client_message()
            .contains(&format!("category={category}")));
        assert_eq!(error.retry_after_seconds(), Some(43200));
        assert!(error.client_message().contains("zen-safe-ray"));
        assert!(!error.message.contains("do-not-leak"));
    }
}

#[test]
fn opencode_zen_does_not_append_a_second_schema_document_to_a_truncated_answer() {
    use super::super::opencode_zen as zen;
    let request = json!({"model":"chat-free","messages":[],"response_format":{"type":"json_schema","json_schema":{"name":"answer","schema":{"type":"object"}}}});
    let first = json!({"choices":[{"message":{"content":"{\"partial\":"},"finish_reason":"length"}],"usage":{"completion_tokens":100}});
    let admitted = zen::ZenAdmission {
        endpoint: zen::CatalogEndpoint::Chat,
        capabilities: Default::default(),
        stale: false,
        context_window: None,
        max_output_tokens: None,
    };
    assert!(zen_continuation_request(
        &request,
        ProxyRoute::CodexChatCompletions,
        &admitted,
        zen::ContinuationKind::MaxTokens,
        &first,
        tokio::time::Instant::now() + Duration::from_secs(2)
    )
    .unwrap()
    .is_none());
}
