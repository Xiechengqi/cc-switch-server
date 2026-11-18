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
            if n == 1 { assert!(sent["messages"].to_string().contains("partial")); }
            axum::Json(json!({"id":"chat_1","object":"chat.completion","model":"chat-free","choices":[{"index":0,"message":{"role":"assistant","content":if n==0 {"partial"} else {" rest"}},"finish_reason":if n==0 {"length"} else {"stop"}}],"usage":{"prompt_tokens":10,"completion_tokens":100,"total_tokens":110}}))
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
async fn opencode_zen_loopback_quota_failure_is_not_replayed_or_leaked() {
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
    assert!(!error.message.contains("private-upstream-body"));
    assert!(error.message.contains("safe-123"));
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
        super::super::opencode_zen::CatalogEndpoint::LegacyChat,
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
                let payload = match zen::wire_for(model) {
                    zen::ZenWire::Chat => json!({"id":"chat_test","model":model,"choices":[{"index":0,"message":{"role":"assistant","content":"fixed"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}}),
                    zen::ZenWire::Responses => json!({"id":"resp_test","object":"response","model":model,"status":"completed","output":[{"id":"msg_test","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"fixed","annotations":[]}]}],"usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}}),
                    zen::ZenWire::Messages => json!({"id":"msg_test","type":"message","model":model,"role":"assistant","content":[{"type":"text","text":"fixed"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}),
                };
                axum::Json(payload)
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
