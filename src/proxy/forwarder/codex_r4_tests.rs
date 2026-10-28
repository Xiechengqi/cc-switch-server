// These fixtures exercise the real bridge with a loopback upstream. They do
// not require a vendor account or an external reference checkout.

#[test]
fn codex_r4_previous_id_copy_is_admitted_without_trimming_or_truncation() {
    let body = json!({"previous_response_id": format!(" {} ", "x".repeat(4096))});
    let id = previous_response_id_from_value(&body).unwrap();
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(id.len() - 1));
    let denied = RequestMemoryBudget::with_parent(usize::MAX, parent.clone());
    assert!(owned_previous_response_id_with_memory(&body, Some(&denied))
        .unwrap_err()
        .is_request_memory_exhausted());
    assert_eq!(parent.snapshot().used_bytes, 0);
    let budget = RequestMemoryBudget::new(id.len());
    let (owned, memory) = owned_previous_response_id_with_memory(&body, Some(&budget)).unwrap();
    assert_eq!(owned.as_deref(), Some(id));
    assert_eq!(budget.snapshot().used_bytes, id.len());
    drop(owned);
    drop(memory);
    assert_eq!(budget.snapshot().used_bytes, 0);
}

#[test]
fn codex_r4_websocket_body_extraction_preserves_flat_and_nested_requests() {
    for nested in [false, true] {
        let body =
            json!({"model":"gpt-5.4", "input":[{"role":"user", "content":"x".repeat(4096)}]});
        let frame = if nested {
            json!({"type":"response.create", "response": body})
        } else {
            let mut frame = body.clone();
            frame["type"] = json!("response.create");
            frame
        };
        let wire = frame.to_string();
        assert_eq!(
            axum_responses_websocket_http_body(&AxumWsMessage::Text(wire.clone())).unwrap(),
            Some(body.clone())
        );
        assert_eq!(
            responses_websocket_http_body(&TungsteniteMessage::Text(wire)).unwrap(),
            body
        );
    }
    assert!(axum_responses_websocket_http_body(&AxumWsMessage::Text(
        json!({"type":"response.create", "response":[]}).to_string()
    ))
    .is_err());
}

#[tokio::test]
async fn codex_r4_http_timing_observes_real_forwarding_and_aggregation() {
    let reply = TestOverflowReply {
        status: StatusCode::OK,
        content_type: "text/event-stream",
        body: concat!(
            "data: {\"type\":\"response.metadata\",\"headers\":{}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"m\",\"output_index\":0,\"content_index\":0,\"delta\":\"hello\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"id\":\"m\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
        ).to_string(),
    };
    let upstream = spawn_test_overflow_upstream(vec![reply]).await;
    let (state, execution) =
        codex_bridge_test_context("r4-http-timing", format!("http://{}", upstream.address)).await;
    let attempt = ForwardAttemptContext {
        execution: Some(execution),
        ..Default::default()
    };
    let slot = attempt.timing.clone();
    let response = forward_with_attempt(
        state.clone(),
        ProxyRoute::CodexResponses,
        None,
        HeaderMap::new(),
        Bytes::from_static(br#"{"model":"gpt-5.4","input":"hi","stream":false}"#),
        attempt,
    )
    .await
    .unwrap();
    let timing = slot.get().unwrap().request();
    let before = timing.snapshot();
    for phase in ["admission", "send_started", "headers", "first_semantic"] {
        assert!(
            before["milestonesMs"][phase].is_number(),
            "{phase}: {before}"
        );
    }
    assert!(before["milestonesMs"]["http_body_handoff"].is_null());
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["output"][0]["content"][0]["text"],
        "hello"
    );
    drop(bytes);
    let after = timing.snapshot();
    assert_eq!(after["result"], "complete");
    assert!(after["milestonesMs"]["terminal"].is_number());
    assert!(after["milestonesMs"]["owner_released"].is_number());
    assert!(after["waitsMs"]["upstream_read"].is_number());
    upstream.server.abort();
}

#[tokio::test]
async fn codex_r4_http_fallback_records_actual_downstream_write_without_semantic_commit() {
    let request = RequestTiming::new(Instant::now(), "responses_websocket");
    let timing = request.attempt(Instant::now());
    let budget = RequestMemoryBudget::new(1024 * 1024);
    budget.set_timing(timing.clone());
    let (finished, completion) = tokio::sync::oneshot::channel();
    let finished = Arc::new(std::sync::Mutex::new(Some(finished)));
    let app = axum::Router::new().route("/bridge", axum::routing::get(move |ws: WebSocketUpgrade| {
        let budget = budget.clone();
        let finished = finished.clone();
        async move { ws.on_upgrade(move |mut socket| async move {
            let result = relay_codex_http_fallback_event(&mut socket, &mut CodexWebsocketOutputPatcher::default(),
                json!({"type":"response.output_text.delta","item_id":"m","output_index":0,"content_index":0,"delta":"hello"}).to_string(),
                Some(&budget), None).await;
            let _ = finished.lock().unwrap().take().unwrap().send(result);
        }) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/bridge"))
        .await
        .unwrap();
    let event = codex_r4_next_json(&mut client).await;
    assert_eq!(event["delta"], "hello");
    assert!(!completion.await.unwrap().unwrap());
    let snapshot = timing.request().snapshot();
    assert!(
        snapshot["milestonesMs"]["downstream_write_completed"].is_number(),
        "{snapshot}"
    );
    assert!(
        snapshot["waitsMs"]["downstream_write"].as_f64().unwrap() > 0.0,
        "{snapshot}"
    );
    assert!(snapshot["milestonesMs"]["first_semantic"].is_null());
    assert!(snapshot["milestonesMs"]["terminal"].is_null());
    assert!(
        snapshot["result"].is_null(),
        "a send observation cannot decide delivery or generation success"
    );
    server.abort();
}

#[tokio::test]
async fn codex_r4_writer_completion_keeps_the_enqueued_attempt_after_retry() {
    let (socket, server) = test_responses_upstream_socket().await;
    let request = RequestTiming::new(Instant::now(), "responses_websocket");
    let original = request.attempt(Instant::now());
    let next = request.attempt(Instant::now());
    let budget = RequestMemoryBudget::new(4096);
    budget.set_timing(original.clone());
    let completion = socket
        .start_send(
            TungsteniteMessage::Text(
                json!({"type":"response.create","model":"gpt-5.4","input":"hi"}).to_string(),
            ),
            Some(
                budget
                    .reserve(RequestMemoryComponent::WebSocketWriteQueue, 128)
                    .unwrap(),
            ),
        )
        .await
        .unwrap();
    budget.set_timing(next.clone());
    completion.await.unwrap().unwrap();
    assert!(original.attempt_snapshot()["milestonesMs"]["upstream_write_completed"].is_number());
    assert!(next.attempt_snapshot()["milestonesMs"]["upstream_write_completed"].is_null());
    drop(socket);
    server.abort();
}

#[test]
fn codex_r4_previous_context_capacity_is_not_a_required_context_miss() {
    let state = forwarder_test_state("r4-context-admission");
    let scope = previous_response_test_scope();
    assert!(state.cache_previous_response_context(scope.clone(), "r", vec![json!({
        "type":"function_call", "call_id":" call ", "name":"run", "arguments":"x".repeat(16 * 1024)
    })], test_now_ms()));
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(128));
    let budget = RequestMemoryBudget::with_parent(usize::MAX, parent.clone());
    let original =
        json!({"input":[{"type":"function_call_output","call_id":" call ","output":"ok"}]});
    let mut body = original.clone();
    let error = inject_previous_response_context_with_memory(
        &state,
        &scope,
        Some("r"),
        &mut body,
        Some(&budget),
    )
    .unwrap_err();
    assert!(error.is_request_memory_exhausted());
    assert_eq!(body, original);
    assert_eq!(
        state
            .previous_response_cache_stats()
            .required_context_unavailable,
        0
    );
    assert_eq!(parent.snapshot().used_bytes, 0);
    assert_eq!(parent.snapshot().rejections, 1);
    assert!(budget.is_exhausted());
}

#[test]
fn codex_r4_previous_context_working_copies_release_and_wire_owner_survives_clones() {
    let state = forwarder_test_state("r4-context-working-copies");
    let scope = previous_response_test_scope();
    assert!(state.cache_previous_response_context(
        scope.clone(),
        "r",
        vec![json!({
            "id":"server-only", "type":"function_call", "call_id":"call", "name":"run",
            "arguments":"{}", "metadata":{"encrypted_content":"private","safe":true}
        })],
        test_now_ms()
    ));
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(None);
    let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
    let original = br#"{"input":[{"type":"function_call_output","call_id":"call","output":"ok"}]}"#;
    let (mut body, parsed) =
        parse_previous_response_working_value(original, Some(&budget)).unwrap();
    let parsed_bytes = parent.snapshot().used_bytes;
    let expansion = inject_previous_response_context_with_memory(
        &state,
        &scope,
        Some("r"),
        &mut body,
        Some(&budget),
    )
    .unwrap();
    assert!(expansion.injected);
    assert!(parent.snapshot().used_bytes > parsed_bytes);
    assert_eq!(body["input"].as_array().unwrap().len(), 2);
    assert!(body["input"][0].get("id").is_none());
    assert!(body["input"][0]["metadata"]
        .get("encrypted_content")
        .is_none());
    assert_eq!(body["input"][0]["metadata"]["safe"], true);
    let (encoded, memory) = encode_previous_response_working_value(&body, Some(&budget)).unwrap();
    let length = encoded.len();
    drop(body);
    drop(expansion);
    drop(parsed);
    assert_eq!(parent.snapshot().used_bytes, length);
    let bytes = memory.unwrap().retain_bytes(Bytes::from(encoded));
    let clone = bytes.clone();
    let slice = bytes.slice(..1);
    drop(bytes);
    drop(clone);
    assert_eq!(parent.snapshot().used_bytes, length);
    drop(slice);
    assert_eq!(parent.snapshot().used_bytes, 0);
    assert_eq!(budget.snapshot().active_reservations, 0);
}

#[test]
fn codex_r4_websocket_context_transfers_wire_charge_for_text_and_binary() {
    let state = forwarder_test_state("r4-context-ws-owner");
    let scope = previous_response_test_scope();
    assert!(state.cache_previous_response_context(
        scope.clone(),
        "r",
        vec![json!({
            "type":"function_call", "call_id":"call", "name":"run", "arguments":"{}"
        })],
        test_now_ms()
    ));
    for binary in [false, true] {
        let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(None);
        let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
        let frame = json!({"type":"response.create","response":{
            "previous_response_id":"r",
            "input":[{"type":"function_call_output","call_id":"call","output":"ok"}]
        }})
        .to_string();
        let mut message = if binary {
            TungsteniteMessage::Binary(frame.into_bytes())
        } else {
            TungsteniteMessage::Text(frame)
        };
        let output = inject_previous_response_context_into_websocket_message_with_memory(
            &state,
            &scope,
            Some("r"),
            &mut message,
            Some(&budget),
        )
        .unwrap();
        assert_eq!(matches!(message, TungsteniteMessage::Binary(_)), binary);
        let length = websocket_message_payload_len(&message);
        assert_eq!(parent.snapshot().used_bytes, length);
        let (normalized, prelude, tools) =
            reserve_responses_websocket_turn_memory(&budget, length, output).unwrap();
        assert_eq!(parent.snapshot().used_bytes, length);
        assert_eq!(parent.snapshot().active_reservations, 1);
        drop(message);
        drop((normalized, prelude, tools));
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}

#[test]
fn codex_r4_websocket_context_capacity_keeps_original_frame_and_scope_miss_contract() {
    let state = forwarder_test_state("r4-context-ws-capacity");
    let scope = previous_response_test_scope();
    assert!(state.cache_previous_response_context(scope.clone(), "r", vec![json!({
        "type":"function_call", "call_id":"call", "name":"run", "arguments":"x".repeat(16 * 1024)
    })], test_now_ms()));
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(None);
    let budget = RequestMemoryBudget::with_parent(4096, parent.clone());
    let original = TungsteniteMessage::Text(json!({"type":"response.create","response":{
        "previous_response_id":"r", "input":[{"type":"function_call_output","call_id":"call","output":"ok"}]
    }}).to_string());
    let mut message = original.clone();
    let error = inject_previous_response_context_into_websocket_message_with_memory(
        &state,
        &scope,
        Some("r"),
        &mut message,
        Some(&budget),
    )
    .unwrap_err();
    assert!(error.is_request_memory_exhausted());
    assert_eq!(message, original);
    assert_eq!(parent.snapshot().used_bytes, 0);
    assert_eq!(
        state
            .previous_response_cache_stats()
            .required_context_unavailable,
        0
    );
    let mut other = scope.clone();
    other.token_refresh_generation += 1;
    let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
    let error = inject_previous_response_context_into_websocket_message_with_memory(
        &state,
        &other,
        Some("r"),
        &mut message,
        Some(&budget),
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.error_code(), "response_context_unavailable");
    assert_eq!(message, original);
    assert_eq!(parent.snapshot().used_bytes, 0);
    assert_eq!(
        state
            .previous_response_cache_stats()
            .required_context_unavailable,
        1
    );
}

#[tokio::test]
async fn codex_r4_quota_observation_does_not_wait_for_persistence_or_write_token_usage() {
    let (state, execution) =
        codex_bridge_test_context("codex-r4-quota-off-reader", "http://127.0.0.1:9".into()).await;
    let execution = execution.with_quota_source_fence(&state.accounts_snapshot().await);
    let target = execution.codex_quota_target.as_ref().unwrap();
    state.fail_codex_quota_commits_for_test(1);
    let commit_gate = state.hold_codex_quota_persist_for_test().await;
    codex::record_quota_websocket_payload(&state, &execution, br#"{"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":0,"window_minutes":300,"reset_after_seconds":100}},"credits":{"has_credits":false,"unlimited":false}}"#);
    codex::record_quota_websocket_payload(&state, &execution, br#"{"type":"codex.response.metadata","headers":{"x-codex-secondary-used-percent":"20","x-codex-secondary-window-minutes":"10080","x-codex-secondary-reset-after-seconds":"200","authorization":"fixture-secret-ignored"}}"#);
    assert!(
        state.accounts_snapshot().await.accounts[0]
            .codex_passive_quota
            .is_none(),
        "delayed persistence must not publish a candidate early"
    );
    drop(commit_gate);
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if state.accounts_snapshot().await.accounts[0]
                .codex_passive_quota
                .is_some()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let account = state.accounts_snapshot().await.accounts.remove(0);
    let snapshot = &account.codex_passive_quota.as_ref().unwrap().snapshot;
    assert_eq!(snapshot.primary.used_percent.as_ref().unwrap().value, 0.0);
    assert_eq!(
        snapshot.secondary.used_percent.as_ref().unwrap().value,
        20.0
    );
    assert!(!serde_json::to_string(snapshot)
        .unwrap()
        .contains("fixture-secret-ignored"));
    assert!(account.rate_limited_until.is_none());
    assert!(state.usage_snapshot().await.logs.is_empty());
    assert_eq!(target.account_id, account.id);
}

#[tokio::test]
async fn codex_r4_response_identity_change_finishes_usage_once() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await.unwrap().unwrap();
        for event in [
            json!({"type":"response.created","response":{"id":"r","status":"in_progress"}}),
            json!({"type":"response.output_text.delta","delta":"partial"}),
            json!({"type":"response.completed","response":{"id":"different-private-id","status":"completed","output":[]}}),
        ] {
            socket
                .send(TungsteniteMessage::Text(event.to_string()))
                .await
                .unwrap();
        }
        let _ = socket.next().await;
    });
    let (state, execution) =
        codex_bridge_test_context("codex-r4-identity-change", "http://127.0.0.1:9".into()).await;
    let (bridge_address, bridge) = spawn_test_responses_bridge(
        state.clone(),
        execution,
        format!("ws://{address}"),
        None,
        "r4-identity-change",
    )
    .await;
    let (events, close) = send_test_bridge_request_with_close(bridge_address, "one identity").await;
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "error"
                && event["error"]["code"] == "upstream_protocol_error")
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| event["type"] == "response.completed"));
    assert!(!events
        .iter()
        .any(|event| event.to_string().contains("different-private-id")));
    assert_eq!(close, Some(CloseCode::Error));
    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].stream_status.as_deref(), Some("interrupted"));
    assert_eq!(usage.logs[0].status_code, 502);
    upstream.await.unwrap();
    bridge.abort();
}

#[tokio::test]
async fn codex_r4_websocket_metadata_then_eof_has_no_first_token_or_http_replay() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let request = socket.next().await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(websocket_message_payload(&request).unwrap()).unwrap()
                ["type"],
            "response.create"
        );
        socket
            .send(TungsteniteMessage::Text(
                json!({
                    "type":"codex.response.metadata","response_id":"r","metadata":{}
                })
                .to_string(),
            ))
            .await
            .unwrap();
        socket.close(None).await.unwrap();
    });
    let (state, execution) =
        codex_bridge_test_context("codex-r4-metadata-eof", "http://127.0.0.1:9".into()).await;
    let (bridge_address, bridge) = spawn_test_responses_bridge(
        state.clone(),
        execution,
        format!("ws://{address}"),
        None,
        "r4-metadata-eof",
    )
    .await;
    let events = send_test_bridge_request(bridge_address, "no replay").await;
    assert!(events.iter().any(|e| e["type"] == "response.failed"));
    assert!(!events.iter().any(|e| e["type"] == "response.completed"));
    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].first_token_ms, None);
    upstream.await.unwrap();
    bridge.abort();
}

#[test]
fn codex_r4_websocket_classification_keeps_metadata_separate_from_create_commit() {
    for value in [
        json!({"type":"response.metadata","metadata":{}}),
        json!({"type":"codex.response.metadata","metadata":{}}),
        json!({"type":"codex.rate_limits","rate_limits":{}}),
    ] {
        for message in [
            TungsteniteMessage::Text(value.to_string()),
            TungsteniteMessage::Binary(value.to_string().into_bytes()),
        ] {
            let observation = classify_responses_websocket_message(&message, None)
                .unwrap()
                .unwrap();
            assert!(!observation.counts_as_business_output());
            assert!(!responses_websocket_http_replay_allowed(
                ResponsesWebsocketMode::Codex,
                false,
                true
            ));
        }
    }
}

#[test]
fn codex_r4_interrupt_requires_exact_mode_and_bounded_id() {
    for mode in [
        Value::Null,
        json!(false),
        json!("cancel"),
        json!(" discard_partial_items "),
    ] {
        let frame = json!({"type":"response.interrupt","response_id":"r","mode":mode});
        assert!(codex_websocket_interrupt_id(&AxumWsMessage::Text(frame.to_string())).is_err());
        assert!(codex_control_message_while_write_pending(
            AxumWsMessage::Text(frame.to_string()),
            true
        )
        .is_err());
    }
    let missing = json!({"type":"response.interrupt","response_id":"r"});
    assert!(codex_websocket_interrupt_id(&AxumWsMessage::Text(missing.to_string())).is_err());
    let oversized = json!({"type":"response.interrupt","response_id":"r".repeat(513),"mode":"discard_partial_items"});
    assert!(codex_websocket_interrupt_id(&AxumWsMessage::Text(oversized.to_string())).is_err());
}

#[test]
fn codex_r4_interrupt_ack_is_not_a_generation_error_or_content() {
    for frame in [
        json!({"type":"response.interrupt.accepted","response_id":"r","sequence_number":2}),
        json!({"type":"response.interrupt.failed","response_id":"r","error":{"code":"interrupt_not_supported","message":"rejected"}}),
    ] {
        let observation = classify_responses_websocket_message(
            &TungsteniteMessage::Text(frame.to_string()),
            None,
        )
        .unwrap()
        .unwrap();
        assert!(!observation.counts_as_business_output());
        assert!(!observation.commits_downstream());
        assert!(!response_semantics::is_responses_terminal_event(
            None, &frame
        ));
        assert_eq!(observation.metric_kind(), "control");
    }
}

#[tokio::test]
async fn codex_r4_failed_interrupt_preserves_generation_and_single_usage() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let rejected = json!({"type":"response.interrupt.failed","response_id":"r",
        "sequence_number":3,"error":{"code":"interrupt_not_supported","message":"rejected"}});
    let expected = rejected.clone();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await.unwrap().unwrap();
        for event in [
            json!({"type":"response.created","response":{"id":"r","status":"in_progress"}}),
            json!({"type":"response.output_text.delta","delta":"before"}),
        ] {
            socket
                .send(TungsteniteMessage::Text(event.to_string()))
                .await
                .unwrap();
        }
        let interrupt = socket.next().await.unwrap().unwrap();
        assert_eq!(
            websocket_message_json_type(&interrupt).as_deref(),
            Some("response.interrupt")
        );
        for event in [
            rejected,
            json!({"type":"response.output_text.delta","delta":"after"}),
            json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}),
        ] {
            socket
                .send(TungsteniteMessage::Text(event.to_string()))
                .await
                .unwrap();
        }
        let _ = socket.next().await;
    });
    let (state, execution) =
        codex_bridge_test_context("codex-r4-interrupt-failed", "http://127.0.0.1:9".into()).await;
    let (address, bridge) = spawn_test_responses_bridge(
        state.clone(),
        execution,
        format!("ws://{address}"),
        None,
        "r4-failed-ack",
    )
    .await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/bridge"))
        .await
        .unwrap();
    client
        .send(TungsteniteMessage::Text(
            json!({"type":"response.create","model":"gpt-5.4","input":"continue"}).to_string(),
        ))
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(2), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let Some(bytes) = websocket_message_payload(&frame) else {
            continue;
        };
        let event: Value = serde_json::from_slice(bytes).unwrap();
        if event["delta"] == "before" {
            client.send(TungsteniteMessage::Text(json!({"type":"response.interrupt","response_id":"r","mode":"discard_partial_items"}).to_string())).await.unwrap();
        }
        let done = event["type"] == "response.completed";
        events.push(event);
        if done {
            break;
        }
    }
    assert!(
        events.contains(&expected),
        "native ACK fields must remain exact"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event["type"].as_str(),
                Some("response.failed" | "response.incomplete" | "response.completed")
            ))
            .count(),
        1
    );
    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].stream_status.as_deref(), Some("completed"));
    let _ = client.close(None).await;
    upstream.await.unwrap();
    bridge.abort();
}

async fn codex_r4_next_json<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("bounded fixture read")
            .expect("open socket")
            .expect("valid frame");
        if let Some(bytes) = websocket_message_payload(&message) {
            return serde_json::from_slice(bytes).unwrap();
        }
    }
}

#[tokio::test]
async fn codex_r4_interrupt_ack_order_timeout_and_duplicate_requests_are_connection_local() {
    for (case, terminal_first, acknowledge) in [
        ("accepted", false, true),
        ("terminal-first", true, true),
        ("missing-ack-active", false, false),
        ("missing-ack-terminal", true, false),
    ] {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert_eq!(
                codex_r4_next_json(&mut socket).await["type"],
                "response.create"
            );
            for event in [
                json!({"type":"response.created","response":{"id":"r","status":"in_progress"}}),
                json!({"type":"response.output_text.delta","delta":"partial"}),
            ] {
                socket
                    .send(TungsteniteMessage::Text(event.to_string()))
                    .await
                    .unwrap();
            }
            let request = codex_r4_next_json(&mut socket).await;
            assert_eq!(request["type"], "response.interrupt");
            assert_eq!(request["response_id"], "r");
            let terminal = json!({"type":"response.incomplete","response":{"id":"r","status":"incomplete","output":[],"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}});
            if terminal_first {
                socket
                    .send(TungsteniteMessage::Text(terminal.to_string()))
                    .await
                    .unwrap();
            }
            if acknowledge {
                // An old response's failure must not terminate or acknowledge r.
                socket.send(TungsteniteMessage::Text(json!({"type":"response.interrupt.failed","response_id":"old-private-response","error":{"code":"rejected"}}).to_string())).await.unwrap();
                tokio::time::sleep(Duration::from_millis(40)).await;
                let ack = json!({"type":"response.interrupt.accepted","response_id":"r","sequence_number":3});
                socket
                    .send(TungsteniteMessage::Text(ack.to_string()))
                    .await
                    .unwrap();
                socket
                    .send(TungsteniteMessage::Text(ack.to_string()))
                    .await
                    .unwrap();
                if !terminal_first {
                    socket
                        .send(TungsteniteMessage::Text(terminal.to_string()))
                        .await
                        .unwrap();
                }
            }
            // A duplicate or wrong-ID interrupt must never reach this socket.
            loop {
                match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
                    Ok(Some(Ok(TungsteniteMessage::Text(text)))) => {
                        panic!("unexpected upstream replay/control: {text}")
                    }
                    Ok(Some(Ok(TungsteniteMessage::Ping(bytes)))) => {
                        let _ = socket.send(TungsteniteMessage::Pong(bytes)).await;
                    }
                    _ => break,
                }
            }
        });
        let name = format!("codex-r4-{case}");
        let (state, mut execution) =
            codex_bridge_test_context(&name, "http://127.0.0.1:9".into()).await;
        Arc::make_mut(&mut execution.plan)
            .driver_options
            .insert("testCodexInterruptAckTimeoutMs".into(), json!(150));
        let (address, bridge) = spawn_test_responses_bridge_with_timeouts(
            state.clone(),
            execution,
            format!("ws://{upstream_address}"),
            None,
            &name,
            Some(Duration::from_secs(1)),
            Some(Duration::from_secs(1)),
        )
        .await;
        let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/bridge"))
            .await
            .unwrap();
        client
            .send(TungsteniteMessage::Text(
                json!({"type":"response.create","model":"gpt-5.4","input":"one generation"})
                    .to_string(),
            ))
            .await
            .unwrap();
        while codex_r4_next_json(&mut client).await["type"] != "response.output_text.delta" {}
        client.send(TungsteniteMessage::Text(json!({"type":"response.interrupt","response_id":"wrong-private-id","mode":"discard_partial_items"}).to_string())).await.unwrap();
        let wrong = codex_r4_next_json(&mut client).await;
        assert_eq!(wrong["type"], "error");
        assert!(!wrong.to_string().contains("wrong-private-id"));
        for _ in 0..2 {
            client.send(TungsteniteMessage::Text(json!({"type":"response.interrupt","response_id":"r","mode":"discard_partial_items"}).to_string())).await.unwrap();
        }
        let mut events = Vec::new();
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(2), client.next())
                .await
                .unwrap();
            match frame {
                Some(Ok(frame)) => {
                    let Some(bytes) = websocket_message_payload(&frame) else {
                        break;
                    };
                    let event: Value = serde_json::from_slice(bytes).unwrap();
                    assert_ne!(event["response_id"], "old-private-response");
                    events.push(event);
                    if acknowledge
                        && events.iter().any(|e| e["type"] == "response.incomplete")
                        && events
                            .iter()
                            .any(|e| e["type"] == "response.interrupt.accepted")
                    {
                        break;
                    }
                }
                _ => break,
            }
        }
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "response.interrupt.accepted")
                .count(),
            usize::from(acknowledge),
            "{case}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "response.incomplete")
                .count(),
            usize::from(terminal_first || acknowledge),
            "{case}"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "response.interrupt.failed")
                .count(),
            usize::from(!acknowledge),
            "{case}"
        );
        let usage = state.usage_snapshot().await;
        assert_eq!(usage.logs.len(), 1, "{case}");
        assert_eq!(
            usage.logs[0].stream_status.as_deref(),
            Some(if terminal_first || acknowledge {
                "incomplete"
            } else {
                "interrupted"
            }),
            "{case}"
        );
        let _ = client.close(None).await;
        upstream.await.unwrap();
        bridge.abort();
    }
}

#[tokio::test]
async fn codex_r4_http_fallback_interrupt_is_explicitly_unsupported_and_keeps_stream() {
    let upstream =
        spawn_test_codex_upstream(TestCodexWebSocketBehavior::CloseSizeThenHttpBusinessThenStall)
            .await;
    let (state, execution) =
        codex_bridge_test_context("r4-http-interrupt", format!("http://{}", upstream.address))
            .await;
    let unavailable = unavailable_test_address().await;
    let (address, bridge) = spawn_test_responses_bridge_with_timeouts(
        state.clone(),
        execution,
        format!("ws://{unavailable}/ws"),
        None,
        "r4-http-interrupt",
        Some(Duration::from_secs(1)),
        Some(Duration::from_secs(3)),
    )
    .await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/bridge"))
        .await
        .unwrap();
    client
        .send(TungsteniteMessage::Text(
            json!({"type":"response.create","model":"gpt-5.4","input":"continue"}).to_string(),
        ))
        .await
        .unwrap();
    while codex_r4_next_json(&mut client).await["type"] != "response.output_text.delta" {}
    client.send(TungsteniteMessage::Text(json!({"type":"response.interrupt","response_id":"resp-interrupted","mode":"discard_partial_items"}).to_string())).await.unwrap();
    let rejected = codex_r4_next_json(&mut client).await;
    assert_eq!(rejected["type"], "response.interrupt.failed");
    assert_eq!(rejected["error"]["code"], "interrupt_not_supported");
    assert_eq!(rejected["error"]["source"], "proxy");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), client.next())
            .await
            .is_err(),
        "rejected interrupt cannot manufacture a generation terminal"
    );
    client
        .send(TungsteniteMessage::Ping(vec![1, 2, 3]))
        .await
        .unwrap();
    assert!(
        matches!(tokio::time::timeout(Duration::from_secs(1), client.next()).await.unwrap().unwrap().unwrap(), TungsteniteMessage::Pong(bytes) if bytes == [1,2,3])
    );
    assert_eq!(upstream.http_requests.load(Ordering::SeqCst), 1);
    let _ = client.close(None).await;
    bridge.abort();
    upstream.server.abort();
}

#[tokio::test]
async fn codex_r4_metadata_never_retimes_first_content_or_idle_deadline() {
    for case in ["text", "overload", "first-deadline", "idle-deadline"] {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let http_requests = Arc::new(AtomicUsize::new(0));
        let counter = http_requests.clone();
        let upstream_address = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/ws", axum::routing::get(move |ws: WebSocketUpgrade| async move {
            ws.on_upgrade(move |mut socket| async move {
                let _ = socket.recv().await;
                let metadata = json!({"type":"response.metadata","metadata":{}}).to_string();
                if case == "idle-deadline" {
                    let _ = socket.send(AxumWsMessage::Text(json!({"type":"response.output_text.delta","delta":"start"}).to_string())).await;
                }
                let rounds = if case.ends_with("deadline") { 12 } else { 1 };
                for _ in 0..rounds {
                    if socket.send(AxumWsMessage::Text(metadata.clone())).await.is_err() { return; }
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                tokio::time::sleep(Duration::from_millis(60)).await;
                if case == "overload" {
                    let _ = socket.send(AxumWsMessage::Text(json!({"type":"response.failed","response":{"id":"r","status":"failed","error":{"code":"server_is_overloaded","message":"overloaded"}}}).to_string())).await;
                } else {
                    let _ = socket.send(AxumWsMessage::Text(json!({"type":"response.output_text.delta","delta":"answer"}).to_string())).await;
                    let _ = socket.send(AxumWsMessage::Text(json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[]}}).to_string())).await;
                }
                let _ = socket.recv().await;
            })
        })).fallback(move || { let counter = counter.clone(); async move { counter.fetch_add(1, Ordering::SeqCst); StatusCode::BAD_GATEWAY } });
        let upstream = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let name = format!("r4-metadata-{case}");
        let (state, execution) =
            codex_bridge_test_context(&name, format!("http://{upstream_address}")).await;
        let first = if case == "first-deadline" {
            Duration::from_millis(80)
        } else {
            Duration::from_secs(1)
        };
        let idle = if case == "idle-deadline" {
            Duration::from_millis(80)
        } else {
            Duration::from_secs(1)
        };
        let (address, bridge) = spawn_test_responses_bridge_with_timeouts(
            state.clone(),
            execution,
            format!("ws://{upstream_address}/ws"),
            None,
            &name,
            Some(first),
            Some(idle),
        )
        .await;
        let started = Instant::now();
        let events = send_test_bridge_request(address, "metadata is not output").await;
        let elapsed = started.elapsed();
        let usage = state.usage_snapshot().await;
        assert_eq!(usage.logs.len(), 1, "{case}");
        assert_eq!(
            http_requests.load(Ordering::SeqCst),
            0,
            "{case}: a sent create cannot replay"
        );
        if case == "text" {
            assert!(usage.logs[0].first_token_ms.unwrap() >= 70);
            assert_eq!(usage.logs[0].stream_status.as_deref(), Some("completed"));
        } else if case == "overload" {
            assert_eq!(usage.logs[0].first_token_ms, None);
            assert!(events.iter().any(|e| e["type"] == "response.failed"));
        } else {
            assert!(
                elapsed < Duration::from_millis(280),
                "{case}: telemetry extended deadline: {elapsed:?}"
            );
            assert!(
                events
                    .iter()
                    .any(|e| e["error"]["code"] == "upstream_stream_timeout"),
                "{case}: {events:?}"
            );
            if case == "first-deadline" {
                assert_eq!(usage.logs[0].first_token_ms, None);
            }
        }
        bridge.abort();
        upstream.abort();
    }
}

#[tokio::test]
async fn codex_r4_idle_telemetry_is_not_delivered_but_unknown_response_events_are() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        let _ = socket.next().await;
        socket.send(TungsteniteMessage::Text(json!({"type":"response.completed","response":{"id":"r","status":"completed","output":[]}}).to_string())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        for event in [
            json!({"type":"codex.rate_limits","rate_limits":{}}),
            json!({"type":"codex.response.metadata","headers":{}}),
            json!({"type":"response.future.delta","delta":"unknown"}),
        ] {
            socket
                .send(TungsteniteMessage::Text(event.to_string()))
                .await
                .unwrap();
        }
        let _ = socket.next().await;
    });
    let (state, execution) =
        codex_bridge_test_context("r4-idle-telemetry", "http://127.0.0.1:9".into()).await;
    let (address, bridge) = spawn_test_responses_bridge(
        state.clone(),
        execution,
        format!("ws://{address}"),
        None,
        "r4-idle-telemetry",
    )
    .await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{address}/bridge"))
        .await
        .unwrap();
    client
        .send(TungsteniteMessage::Text(
            json!({"type":"response.create","model":"gpt-5.4","input":"first"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        codex_r4_next_json(&mut client).await["type"],
        "response.completed"
    );
    assert_eq!(
        codex_r4_next_json(&mut client).await["type"],
        "response.future.delta"
    );
    assert_eq!(state.usage_snapshot().await.logs.len(), 1);
    let _ = client.close(None).await;
    upstream.await.unwrap();
    bridge.abort();
}
