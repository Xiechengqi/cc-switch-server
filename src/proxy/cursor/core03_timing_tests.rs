// Included inside agent_driver::tests; all traffic uses a loopback h2 peer.
use crate::proxy::cursor::agent_proto::{
    encode_message, encode_string, encode_uint32, ASM_EXEC_SERVER_MESSAGE, ASM_INTERACTION_UPDATE,
    ESM_EXEC_ID, ESM_ID, ESM_MCP_ARGS, IU_HEARTBEAT, IU_TEXT_DELTA, IU_TURN_ENDED,
    MCA_TOOL_CALL_ID, MCA_TOOL_NAME, TDU_TEXT,
};
use crate::proxy::cursor::h2_client::lifecycle_tests::LoopbackPeer;
use crate::proxy::cursor::protocol::CursorResponseFormat;
use crate::proxy::request_timing::ForwardTiming;

fn core03_cursor_text(text: &str) -> Bytes {
    wrap_connect_frame(&encode_message(
        ASM_INTERACTION_UPDATE,
        &[encode_message(
            IU_TEXT_DELTA,
            &[encode_string(TDU_TEXT, text)],
        )],
    ))
}

fn core03_cursor_end() -> Bytes {
    wrap_connect_frame(&encode_message(
        ASM_INTERACTION_UPDATE,
        &[encode_message(IU_TURN_ENDED, &[])],
    ))
}

fn core03_cursor_tool() -> Bytes {
    wrap_connect_frame(&encode_message(
        ASM_EXEC_SERVER_MESSAGE,
        &[
            encode_uint32(ESM_ID, 7),
            encode_string(ESM_EXEC_ID, "fixture-exec"),
            encode_message(
                ESM_MCP_ARGS,
                &[
                    encode_string(MCA_TOOL_NAME, "lookup"),
                    encode_string(MCA_TOOL_CALL_ID, "fixture-call"),
                ],
            ),
        ],
    ))
}

async fn core03_cursor_session(
    provider_type: ProviderType,
    surface: &'static str,
    tools: bool,
) -> (
    ServerState,
    StoredProvider,
    CursorSessionKey,
    Arc<tokio::sync::Mutex<CursorSession>>,
    LoopbackPeer,
    RequestMemoryBudget,
    TimingSlot,
    ForwardTiming,
) {
    let state = cursor_stream_test_state("core03-timing");
    let stored =
        cursor_endpoint_provider(provider_type, json!({"CURSOR_API_KEY": "fixture-api-key"}));
    let slot = TimingSlot::default();
    let timing = slot
        .begin_for_provider(provider_type, surface, Instant::now())
        .unwrap();
    timing.mark(Milestone::Admission);
    let budget = RequestMemoryBudget::new(16 * 1024 * 1024);
    budget.set_timing(timing.clone());
    let mut peer = LoopbackPeer::open(budget.clone(), Duration::from_secs(2)).await;
    let key = CursorSessionKey::new(CursorSessionScope::fixture("core03"), "core03-conversation");
    let tools = if tools {
        try_build_plan(
            InboundProtocol::OpenAiChat,
            &json!({"model":"composer-2.5","messages":[{"role":"user","content":"lookup"}],
                "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]}),
        )
        .unwrap()
        .tools
    } else {
        Vec::new()
    };
    let entry = state
        .cursor_sessions
        .reserve(
            key.clone(),
            CursorProtocolRail::for_provider(provider_type).unwrap(),
            HashMap::new(),
            tools,
            Vec::new(),
            "/workspace".to_string(),
        )
        .await
        .unwrap();
    entry.lock().await.attach_request_memory(&budget).unwrap();
    state
        .cursor_sessions
        .attach_stream(&entry, peer.take_stream())
        .await
        .unwrap();
    (state, stored, key, entry, peer, budget, slot, timing)
}

#[tokio::test]
async fn cursor_core03_both_rails_all_formats_stream_and_nonstream() {
    for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
        for (surface, format) in [
            (
                "anthropic_messages",
                CursorResponseFormat::AnthropicMessages,
            ),
            (
                "chat_completions",
                CursorResponseFormat::OpenAiChatCompletions,
            ),
            ("responses", CursorResponseFormat::OpenAiResponses),
            (
                "gemini_generate_content",
                CursorResponseFormat::GeminiGenerateContent,
            ),
        ] {
            for streaming in [false, true] {
                let (state, stored, key, entry, mut peer, budget, slot, timing) =
                    core03_cursor_session(provider_type, surface, false).await;
                peer.send(core03_cursor_text("visible core03 result"));
                peer.send(core03_cursor_end());
                let response = if streaming {
                    stream_response(
                        state.clone(),
                        stored,
                        entry.clone(),
                        key,
                        format,
                        "composer-2.5".to_string(),
                        1,
                        UsageLogContext::default(),
                        Instant::now(),
                        UsageModelMetadata::default(),
                        None,
                        None,
                        None,
                        Some(budget.clone()),
                    )
                    .await
                    .unwrap()
                } else {
                    let outcome = drive_non_stream(
                        &state,
                        &stored,
                        entry.clone(),
                        &key,
                        format,
                        "composer-2.5".to_string(),
                        1,
                        Some(&budget),
                    )
                    .await
                    .unwrap();
                    let DriveOutcome::Completed { body, .. } = outcome else {
                        panic!("expected completion")
                    };
                    state
                        .cursor_sessions
                        .release(entry.clone(), SessionState::Closed)
                        .await;
                    Response::new(Body::from(body))
                };
                let bytes = tokio::time::timeout(
                    Duration::from_secs(2),
                    axum::body::to_bytes(slot.response(response).into_body(), 1024 * 1024),
                )
                .await
                .unwrap()
                .unwrap();
                assert!(String::from_utf8_lossy(&bytes).contains("visible core03 result"));
                let snapshot = timing.request().snapshot();
                assert_eq!(snapshot["providerType"], provider_type.as_str());
                assert_eq!(snapshot["result"], "complete");
                let phases = &snapshot["milestonesMs"];
                assert!(phases["first_semantic"].is_number());
                assert!(phases["http_body_handoff"].is_number());
                assert!(phases["terminal"].is_number());
                assert!(phases["upstream_write_completed"].is_null());
                assert!(phases["downstream_write_completed"].is_null());
                assert!(snapshot["waitsMs"]["upstream_read"].as_f64().unwrap() > 0.0);
                drop((bytes, entry, budget));
                peer.assert_closed().await;
                assert!(timing.request().snapshot()["milestonesMs"]["owner_released"].is_number());
            }
        }
    }
}

#[tokio::test]
async fn cursor_core03_control_frames_then_committed_cancellation_close_h2() {
    for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
        let (state, stored, key, entry, mut peer, budget, slot, timing) =
            core03_cursor_session(provider_type, "anthropic_messages", false).await;
        peer.send(wrap_connect_frame(&encode_message(
            ASM_INTERACTION_UPDATE,
            &[encode_message(IU_HEARTBEAT, &[])],
        )));
        peer.send(core03_cursor_text(""));
        let response = stream_response(
            state.clone(),
            stored,
            entry.clone(),
            key.clone(),
            CursorResponseFormat::AnthropicMessages,
            "composer-2.5".to_string(),
            1,
            UsageLogContext::default(),
            Instant::now(),
            UsageModelMetadata::default(),
            None,
            None,
            None,
            Some(budget.clone()),
        )
        .await
        .unwrap();
        let mut body = slot.response(response).into_body().into_data_stream();
        // Drain initial protocol events and then wait for actual business output.
        while let Ok(chunk) = tokio::time::timeout(Duration::from_millis(20), body.next()).await {
            chunk.unwrap().unwrap();
        }
        assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
        peer.send(core03_cursor_text("visible"));
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if String::from_utf8_lossy(&chunk).contains("visible") {
                break;
            }
        }
        assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number());
        drop(body);
        assert_eq!(timing.request().snapshot()["result"], "cancelled");
        tokio::time::timeout(Duration::from_secs(2), async {
            while state.cursor_sessions.has(&key).await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        peer.assert_closed().await;
        drop((entry, budget));
        let snapshot = timing.request().snapshot();
        assert!(snapshot["milestonesMs"]["owner_released"].is_number());
        assert!(snapshot["milestonesMs"]["terminal"].is_null());
    }
}

#[tokio::test]
async fn cursor_core03_in_band_failure_stays_failed_after_http_200() {
    for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
        let (state, stored, key, entry, mut peer, budget, slot, timing) =
            core03_cursor_session(provider_type, "responses", false).await;
        peer.send(core03_cursor_text("visible"));
        peer.send(wrap_connect_frame(
            br#"{"error":{"code":"INTERNAL","message":"fixture failure"}}"#,
        ));
        let response = stream_response(
            state.clone(),
            stored,
            entry.clone(),
            key,
            CursorResponseFormat::OpenAiResponses,
            "composer-2.5".to_string(),
            1,
            UsageLogContext::default(),
            Instant::now(),
            UsageModelMetadata::default(),
            None,
            None,
            None,
            Some(budget.clone()),
        )
        .await
        .unwrap();
        let bytes = tokio::time::timeout(
            Duration::from_secs(2),
            axum::body::to_bytes(slot.response(response).into_body(), 1024 * 1024),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("response.failed"));
        assert_eq!(timing.request().snapshot()["result"], "failed");
        assert_eq!(timing.attempt_snapshot()["result"], "failed");
        assert_eq!(state.cursor_sessions.size().await, 0);
        drop((bytes, entry, budget));
        peer.assert_closed().await;
    }
}

#[tokio::test]
async fn cursor_core03_parked_turn_and_resumed_turn_keep_distinct_owners() {
    for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
        let (state, stored, key, entry, mut peer, budget, slot, first) =
            core03_cursor_session(provider_type, "responses", true).await;
        peer.send(core03_cursor_tool());
        let outcome = drive_non_stream(
            &state,
            &stored,
            entry.clone(),
            &key,
            CursorResponseFormat::OpenAiResponses,
            "composer-2.5".to_string(),
            1,
            Some(&budget),
        )
        .await
        .unwrap();
        let DriveOutcome::Parked {
            body,
            memory: held_outcome_memory,
            ..
        } = outcome
        else {
            panic!("expected parked tool")
        };
        state
            .cursor_sessions
            .release(entry.clone(), SessionState::AwaitingToolResult)
            .await;
        let bytes = axum::body::to_bytes(
            slot.response(Response::new(Body::from(body))).into_body(),
            1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(first.request().snapshot()["result"], "complete");
        drop((bytes, budget));
        assert!(first.request().snapshot()["milestonesMs"]["owner_released"].is_null());

        let next_slot = TimingSlot::default();
        let next = next_slot
            .begin_for_provider(provider_type, "responses", Instant::now())
            .unwrap();
        let next_budget = RequestMemoryBudget::new(16 * 1024 * 1024);
        next_budget.set_timing(next.clone());
        entry
            .lock()
            .await
            .attach_request_memory(&next_budget)
            .unwrap();
        // The returned outcome itself still owns buffered state. Transferring
        // the session cannot release that independent response owner.
        assert!(first.request().snapshot()["milestonesMs"]["owner_released"].is_null());
        drop(held_outcome_memory);
        assert!(first.request().snapshot()["milestonesMs"]["owner_released"].is_number());
        let call_id = entry
            .lock()
            .await
            .pending_tool_calls
            .keys()
            .next()
            .unwrap()
            .clone();
        resume_tool_results(
            &entry,
            &[super::super::request_builder::ToolResultBlock {
                tool_call_id: call_id,
                content: "tool result".to_string(),
                is_error: false,
            }],
        )
        .await
        .unwrap();
        assert!(next.request().snapshot()["milestonesMs"]["send_started"].is_number());
        assert!(next.request().snapshot()["milestonesMs"]["headers"].is_null());
        peer.send(core03_cursor_text("resumed result"));
        peer.send(core03_cursor_end());
        let outcome = drive_non_stream(
            &state,
            &stored,
            entry.clone(),
            &key,
            CursorResponseFormat::OpenAiResponses,
            "composer-2.5".to_string(),
            1,
            Some(&next_budget),
        )
        .await
        .unwrap();
        let DriveOutcome::Completed {
            body,
            memory: final_outcome_memory,
            ..
        } = outcome
        else {
            panic!("expected resumed completion")
        };
        state
            .cursor_sessions
            .release(entry.clone(), SessionState::Closed)
            .await;
        let bytes = axum::body::to_bytes(
            next_slot
                .response(Response::new(Body::from(body)))
                .into_body(),
            1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(next.request().snapshot()["result"], "complete");
        assert!(next.request().snapshot()["milestonesMs"]["first_semantic"].is_number());
        drop((bytes, entry, next_budget, final_outcome_memory));
        peer.assert_closed().await;
        assert!(next.request().snapshot()["milestonesMs"]["owner_released"].is_number());
    }
}

#[tokio::test]
async fn cursor_core03_buffered_tool_constraint_observes_accepted_and_exhausted_attempts() {
    for provider_type in [ProviderType::CursorOAuth, ProviderType::CursorApiKey] {
        for accepted in [false, true] {
            let (state, stored, key, entry, mut peer, budget, slot, timing) =
                core03_cursor_session(provider_type, "responses", true).await;
            let plan = try_build_plan(
                InboundProtocol::OpenAiChat,
                &json!({"model":"composer-2.5","messages":[{"role":"user","content":"lookup"}],
                    "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}],
                    "tool_choice":"required"}),
            ).unwrap();
            let mut context = CursorAttemptContext::capture(
                &state,
                &stored,
                "fixture",
                Duration::from_secs(2),
                Some(budget.clone()),
            )
            .await
            .unwrap();
            context.timing = Some(slot.clone());
            if accepted {
                peer.send(core03_cursor_tool());
            } else {
                // Exercise exhaustion without allowing any vendor-bound retry.
                for _ in 0..MAX_CURSOR_RECOVERY_RETRIES {
                    assert_eq!(
                        context
                            .reserve(
                                &state,
                                &stored,
                                RecoveryStage::BodyCompatibility,
                                "fixture_exhausted"
                            )
                            .await
                            .unwrap(),
                        RetryDecision::Reserved
                    );
                }
                peer.send(core03_cursor_text("rejected prose"));
                peer.send(core03_cursor_end());
            }
            let response = stream_semantic_response(
                state.clone(),
                stored,
                entry.clone(),
                key.clone(),
                plan,
                SemanticToolConstraint::Required,
                "fixture".to_string(),
                CursorResponseFormat::OpenAiResponses,
                "composer-2.5".to_string(),
                1,
                UsageLogContext::default(),
                Instant::now(),
                UsageModelMetadata::default(),
                CursorH2Timeouts::default(),
                context,
                None,
                None,
                None,
                Some(budget.clone()),
                None,
            )
            .await;
            let bytes = tokio::time::timeout(
                Duration::from_secs(2),
                axum::body::to_bytes(slot.response(response).into_body(), 1024 * 1024),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(
                timing.request().snapshot()["result"],
                if accepted { "complete" } else { "failed" }
            );
            assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number());
            let body = String::from_utf8_lossy(&bytes);
            assert!(!body.contains("rejected prose"));
            assert_eq!(body.contains("response.failed"), !accepted);
            if accepted {
                assert!(state.cursor_sessions.has(&key).await);
                state
                    .cursor_sessions
                    .release(entry.clone(), SessionState::Closed)
                    .await;
            } else {
                assert!(!state.cursor_sessions.has(&key).await);
            }
            drop((bytes, entry, budget));
            peer.assert_closed().await;
        }
    }
}
