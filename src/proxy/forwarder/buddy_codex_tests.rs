#[test]
fn buddy_codex_cli_preamble_tool_history_and_connection_probe_share_the_adapter() {
    use crate::domain::codebuddy::BUDDY_CODEX_CLI_PREAMBLE;
    use crate::domain::providers::runtime::build_provider_model_probe;

    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                let (address, fixture, server) =
                    spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success]).await;
                let state = codebuddy_test_state("buddy-codex-preamble");
                let (id, _) = install_buddy_provider(
                    &state,
                    product,
                    "codex-preamble",
                    AppKind::Codex,
                    site,
                    &format!("http://{address}"),
                )
                .await;
                install_codebuddy_share(
                    &state,
                    "preamble-share",
                    AppKind::Codex,
                    &id,
                    "owner@example.com",
                )
                .await;
                for stream in [false, true] {
                    let body = json!({
                        "model":"auto","stream":stream,
                        "instructions":format!("{BUDDY_CODEX_CLI_PREAMBLE}\n\n# AGENTS.md\nKeep project policy."),
                        "tools":[{"type":"function","name":"lookup","description":BUDDY_CODEX_CLI_PREAMBLE,
                            "parameters":{"type":"object","properties":{"q":{"type":"string"}}}}],
                        "input":[
                            {"role":"developer","content":"Developer policy: preserve me."},
                            {"role":"user","content":BUDDY_CODEX_CLI_PREAMBLE},
                            {"type":"function_call","call_id":"call-1","name":"lookup","arguments":json!({"q":BUDDY_CODEX_CLI_PREAMBLE}).to_string()},
                            {"type":"function_call_output","call_id":"call-1","output":BUDDY_CODEX_CLI_PREAMBLE},
                            {"role":"user","content":"Continue."}
                        ]
                    });
                    let response = super::forward_for_test_surface(
                        state.clone(),
                        ProxyRoute::CodexResponses,
                        id.clone(),
                        None,
                        codebuddy_headers(
                            "preamble-share",
                            "owner@example.com",
                            "preamble-session",
                        ),
                        Bytes::from(body.to_string()),
                    )
                    .await
                    .unwrap();
                    assert!(String::from_utf8_lossy(&collect(response).await).contains("ready"));
                    let observations = fixture.generation_observations();
                    let sent = &observations.last().unwrap().body;
                    assert_eq!(sent["messages"][0]["content"],
                        "You are a helpful coding assistant. You are expected to be precise, safe, and helpful.\n\n# AGENTS.md\nKeep project policy.");
                    assert_eq!(
                        sent["messages"][1]["content"],
                        "Developer policy: preserve me."
                    );
                    assert_eq!(sent["messages"][2]["content"], BUDDY_CODEX_CLI_PREAMBLE);
                    let args: Value = serde_json::from_str(
                        sent["messages"][3]["tool_calls"][0]["function"]["arguments"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap();
                    assert_eq!(args["q"], BUDDY_CODEX_CLI_PREAMBLE);
                    assert_eq!(sent["messages"][4]["content"], BUDDY_CODEX_CLI_PREAMBLE);
                    assert_eq!(
                        sent["tools"][0]["function"]["description"],
                        BUDDY_CODEX_CLI_PREAMBLE
                    );
                    assert_eq!(sent["model"], site_model(site));

                    let probe = build_provider_model_probe(
                        AppKind::Codex,
                        product,
                        "auto",
                        "ping",
                        stream,
                        "probe-fingerprint",
                    );
                    let response = super::forward_for_test_surface(
                        state.clone(),
                        ProxyRoute::CodexResponses,
                        id.clone(),
                        None,
                        codebuddy_headers("preamble-share", "owner@example.com", "probe-session"),
                        Bytes::from(probe.body_json()),
                    )
                    .await
                    .unwrap();
                    collect(response).await;
                    let observations = fixture.generation_observations();
                    let sent = &observations.last().unwrap().body;
                    assert_eq!(sent["messages"][0]["content"],
                        "You are a helpful coding assistant. You are expected to be precise, safe, and helpful.");
                    assert_eq!(sent["max_tokens"], 1);
                }
                assert_eq!(fixture.count(&fixture.generation_requests), 4);
                assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                server.abort();
            }
        }
    });
}

#[test]
fn buddy_channel_policy_errors_keep_product_label_and_never_refresh_or_replay() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                for reply in [
                    CodeBuddyGenerationReply::HttpUnauthorizedWithTerminalCode(11_128),
                    CodeBuddyGenerationReply::ChannelRejected,
                    CodeBuddyGenerationReply::LateChannelRejected,
                ] {
                    for stream in [false, true] {
                        let (address, fixture, server) =
                            spawn_codebuddy_upstream(site, vec![reply]).await;
                        let state = codebuddy_test_state("buddy-channel-rejected");
                        let (id, _) = install_buddy_provider(
                            &state,
                            product,
                            "channel-rejected",
                            AppKind::Codex,
                            site,
                            &format!("http://{address}"),
                        )
                        .await;
                        install_codebuddy_share(
                            &state,
                            "channel-share",
                            AppKind::Codex,
                            &id,
                            "owner@example.com",
                        )
                        .await;
                        let result = forward_codebuddy_surface(
                            state.clone(),
                            AppKind::Codex,
                            id,
                            "channel-share",
                            stream,
                        )
                        .await;
                        let message = if stream
                            && reply == CodeBuddyGenerationReply::LateChannelRejected
                        {
                            let body = String::from_utf8(collect(result.unwrap()).await).unwrap();
                            assert!(body.contains("response.failed"), "{body}");
                            assert!(!body.contains("response.completed"), "{body}");
                            body
                        } else {
                            let error = result.expect_err("11128 must remain a client rejection");
                            assert_eq!(error.status, StatusCode::BAD_REQUEST);
                            assert_eq!(error.retry_after_seconds(), None);
                            error.message
                        };
                        let product_label = crate::domain::codebuddy::buddy_product_label(product);
                        assert!(
                            message.contains(&format!("{product_label} upstream error 11128")),
                            "{message}"
                        );
                        assert!(message.contains("channel-policy rejection"), "{message}");
                        if product == ProviderType::WorkBuddyOAuth {
                            assert!(!message.contains("CodeBuddy upstream error"), "{message}");
                        }
                        assert_eq!(fixture.count(&fixture.generation_requests), 1);
                        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                        let usage = state.usage_snapshot().await;
                        assert!(usage.logs.iter().all(|log| log.attempt_count == 1));
                        server.abort();
                    }
                }
            }
        }
    });
}
