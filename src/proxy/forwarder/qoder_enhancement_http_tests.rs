use super::*;

#[test]
fn qoder_first_semantic_deadline_includes_headers_comments_and_error_bodies() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for reply in [
                QoderGenerationReply::DelayedHeadersAndSemantic,
                QoderGenerationReply::UnauthorizedStalledBody,
            ] {
                for streaming in [false, true] {
                    let (address, fixture, server) = spawn_qoder_upstream(rail, vec![reply]).await;
                    let state = qoder_test_state("absolute-first-deadline");
                    let (id, _) = install_qoder_provider(
                        &state,
                        "absolute-first-deadline",
                        AppKind::Codex,
                        rail,
                        &format!("http://{address}"),
                    )
                    .await;
                    install_qoder_share(
                        &state,
                        "deadline-share",
                        AppKind::Codex,
                        &id,
                        "owner@example.com",
                    )
                    .await;
                    let route = ProxyRoute::CodexResponses;
                    let mut attempt = qoder_core03_attempt(&state, route, &id).await;
                    Arc::make_mut(&mut attempt.execution.as_mut().unwrap().plan)
                        .transport_policy
                        .stream_first_byte_timeout_ms = Some(300);
                    let (_, path, body) = surface_request(AppKind::Codex, streaming);
                    let result = tokio::time::timeout(Duration::from_secs(2),forward_with_attempt(
                        state,route,path,qoder_headers("deadline-share","owner@example.com","deadline"),body,attempt
                    )).await.expect("headers, comments and HTTP error bodies cannot restart or escape the absolute deadline");
                    let error = result.expect_err(
                        "the total semantic budget expires even if each stage alone fits",
                    );
                    assert_eq!(
                        error.status,
                        StatusCode::GATEWAY_TIMEOUT,
                        "{rail:?} {reply:?}: {error:?}"
                    );
                    assert_eq!(fixture.count(&fixture.generation_requests), 1);
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_text_tool_recovery_is_explicit_on_every_surface_and_rail() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                let app = route.app();
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::TextToolEcho]).await;
                let state = qoder_test_state("echo-recovery");
                let (id, _) = install_qoder_provider(
                    &state,
                    "echo-recovery",
                    app,
                    rail,
                    &format!("http://{address}"),
                )
                .await;
                install_qoder_share(&state, "echo-share", app, &id, "owner@example.com").await;
                for streaming in [false, true] {
                    for enabled in [false, true] {
                        let (_, path, body) = surface_request(app, streaming);
                        let mut body: Value = serde_json::from_slice(&body).unwrap();
                        if route == ProxyRoute::CodexChatCompletions {
                            body = json!({"model":MODEL_KEY,"stream":streaming,"messages":[{"role":"user","content":"use lookup"}]});
                        }
                        body["tools"] = match route {
                            ProxyRoute::ClaudeMessages => {
                                json!([{"name":"lookup","input_schema":{"type":"object"}}])
                            }
                            ProxyRoute::CodexResponses => {
                                json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}])
                            }
                            ProxyRoute::CodexChatCompletions => {
                                json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}])
                            }
                            ProxyRoute::Gemini => {
                                json!([{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}}]}])
                            }
                            _ => unreachable!(),
                        };
                        if enabled {
                            body["qoder_text_tool_recovery"] = json!(true);
                        }
                        let attempt = qoder_core03_attempt(&state, route, &id).await;
                        let response = forward_with_attempt(
                            state.clone(),
                            route,
                            path,
                            qoder_headers(
                                "echo-share",
                                "owner@example.com",
                                &format!("echo-{streaming}-{enabled}"),
                            ),
                            Bytes::from(body.to_string()),
                            attempt,
                        )
                        .await
                        .unwrap();
                        assert_eq!(response.status(), StatusCode::OK);
                        let text = String::from_utf8(collect(response).await).unwrap();
                        assert_eq!(
                            text.contains("<tool_call>"),
                            !enabled,
                            "{rail:?} {route:?}: {text}"
                        );
                        assert!(text.contains("lookup"));
                    }
                }
                assert_eq!(fixture.count(&fixture.generation_requests), 4);
                assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_enhanced_tools_fail_stream_and_nonstream_on_every_surface_and_rail() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                let app = route.app();
                for reply in [
                    QoderGenerationReply::InvalidToolArguments,
                    QoderGenerationReply::MissingToolName,
                    QoderGenerationReply::UndeclaredToolName,
                ] {
                    for streaming in [false, true] {
                        let (address, fixture, server) =
                            spawn_qoder_upstream(rail, vec![reply]).await;
                        let state = qoder_test_state("enhanced-tools");
                        let (id, _) = install_qoder_provider(
                            &state,
                            "enhanced-tools",
                            app,
                            rail,
                            &format!("http://{address}"),
                        )
                        .await;
                        install_qoder_share(
                            &state,
                            "enhanced-share",
                            app,
                            &id,
                            "owner@example.com",
                        )
                        .await;
                        let (_, path, body) = surface_request(app, streaming);
                        let mut body: Value = serde_json::from_slice(&body).unwrap();
                        if route == ProxyRoute::CodexChatCompletions {
                            body = json!({"model":MODEL_KEY,"stream":streaming,"messages":[{"role":"user","content":"use lookup"}]});
                        }
                        body["tools"] = match app {
                            AppKind::Claude => {
                                json!([{"name":"lookup","input_schema":{"type":"object"}}])
                            }
                            AppKind::Codex => {
                                if route == ProxyRoute::CodexChatCompletions {
                                    json!([{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}])
                                } else {
                                    json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}])
                                }
                            }
                            AppKind::Gemini => {
                                json!([{"functionDeclarations":[{"name":"lookup","parameters":{"type":"object"}}]}])
                            }
                        };
                        let attempt = qoder_core03_attempt(&state, route, &id).await;
                        let response = forward_with_attempt(
                            state.clone(),
                            route,
                            path,
                            qoder_headers("enhanced-share", "owner@example.com", "enhanced"),
                            Bytes::from(body.to_string()),
                            attempt,
                        )
                        .await;
                        match response {
                            Err(error) => assert_eq!(error.status, StatusCode::BAD_GATEWAY),
                            Ok(response) => {
                                assert!(streaming);
                                let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                                    .await
                                    .unwrap();
                                let body = String::from_utf8_lossy(&body);
                                assert!(
                                    body.contains("error") || body.contains("failed"),
                                    "{app:?} {body}"
                                );
                                assert!(!body.contains("response.completed"));
                            }
                        }
                        let usage = state.usage_snapshot().await;
                        assert!(usage
                            .logs
                            .iter()
                            .all(|log| log.outcome != UsageOutcome::Success));
                        assert_eq!(fixture.count(&fixture.generation_requests), 1);
                        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                        server.abort();
                    }
                }
            }
        }
    });
}

#[test]
fn qoder_enhanced_images_reach_cosy_unchanged_on_every_surface_and_rail() {
    run_qoder_async_test(async {
        const IMAGE: &str = "data:image/png;base64,iVBORw0KGgo=";
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                fixture.catalog_config.lock().unwrap()["is_vl"] = json!(true);
                let state = qoder_test_state("enhanced-images");
                let (id, _) = install_qoder_provider(
                    &state,
                    "enhanced-images",
                    route.app(),
                    rail,
                    &format!("http://{address}"),
                )
                .await;
                install_qoder_share(
                    &state,
                    "enhanced-share",
                    route.app(),
                    &id,
                    "owner@example.com",
                )
                .await;
                let (_, path, body) = surface_request(route.app(), false);
                let mut body: Value = serde_json::from_slice(&body).unwrap();
                match route {
                    ProxyRoute::ClaudeMessages => {
                        body["messages"] = json!([{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"iVBORw0KGgo="}}]}])
                    }
                    ProxyRoute::CodexResponses => {
                        body["input"] = json!([{"role":"user","content":[{"type":"input_text","text":"describe"},{"type":"input_image","image_url":IMAGE}]}])
                    }
                    ProxyRoute::CodexChatCompletions => {
                        body = json!({"model":MODEL_KEY,"messages":[{"role":"user","content":[{"type":"text","text":"describe"},{"type":"image_url","image_url":{"url":IMAGE}}]}]})
                    }
                    ProxyRoute::Gemini => {
                        body["contents"] = json!([{"role":"user","parts":[{"text":"describe"},{"inlineData":{"mimeType":"image/png","data":"iVBORw0KGgo="}}]}])
                    }
                    _ => unreachable!(),
                }
                let attempt = qoder_core03_attempt(&state, route, &id).await;
                let response = forward_with_attempt(
                    state,
                    route,
                    path,
                    qoder_headers("enhanced-share", "owner@example.com", "enhanced"),
                    Bytes::from(body.to_string()),
                    attempt,
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let _ = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                let observations = fixture.observations.lock().unwrap();
                let wire = observations
                    .iter()
                    .filter_map(|observation| observation.decoded_body.as_ref())
                    .find(|body| body.get("messages").is_some())
                    .unwrap();
                assert!(
                    wire["messages"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(
                            |message| message["content"].as_array().is_some_and(|parts| parts
                                .iter()
                                .any(|part| part
                                    .pointer("/image_url/url")
                                    .and_then(Value::as_str)
                                    == Some(IMAGE)))
                        ),
                    "{rail:?} {route:?} {wire}"
                );
                assert_eq!(fixture.count(&fixture.generation_requests), 1);
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_enhanced_committed_heartbeat_does_not_reset_idle_timeout_or_replay() {
    run_qoder_async_test(async {
        let rail = QoderFixtureRail::GlobalOauth;
        let (address, fixture, server) =
            spawn_qoder_upstream(rail, vec![QoderGenerationReply::IdleAfterContent]).await;
        let state = qoder_test_state("enhanced-keepalive");
        let (id, _) = install_qoder_provider(
            &state,
            "enhanced-keepalive",
            AppKind::Codex,
            rail,
            &format!("http://{address}"),
        )
        .await;
        install_qoder_share(
            &state,
            "enhanced-share",
            AppKind::Codex,
            &id,
            "owner@example.com",
        )
        .await;
        let route = ProxyRoute::CodexResponses;
        let mut attempt = qoder_core03_attempt(&state, route, &id).await;
        Arc::make_mut(&mut attempt.execution.as_mut().unwrap().plan)
            .transport_policy
            .stream_idle_timeout_ms = Some(16_000);
        let (_, path, body) = surface_request(AppKind::Codex, true);
        let response = forward_with_attempt(
            state.clone(),
            route,
            path,
            qoder_headers("enhanced-share", "owner@example.com", "enhanced"),
            body,
            attempt,
        )
        .await
        .unwrap();
        let began = tokio::time::Instant::now();
        let body = tokio::time::timeout(
            Duration::from_secs(20),
            axum::body::to_bytes(response.into_body(), 1024 * 1024),
        )
        .await
        .expect("heartbeat must not extend idle timeout")
        .unwrap();
        let body = String::from_utf8_lossy(&body);
        assert!(body.contains(": keep-alive\n\n"), "{body}");
        assert!(body.contains("response.failed"));
        assert!(!body.contains("response.completed"));
        assert!(began.elapsed() < Duration::from_secs(20));
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
        assert_ne!(
            state.usage_snapshot().await.logs.last().unwrap().outcome,
            UsageOutcome::Success
        );
        server.abort();
    });
}
