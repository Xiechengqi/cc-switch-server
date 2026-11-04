#[test]
fn buddy_claude_fixed_reasoning_preferences_use_the_catalog_level() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                for (slot, fixed_effort) in [("kimi", "medium"), ("deepseek", "high")] {
                    for stream in [false, true] {
                        let (address, fixture, server) =
                            spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success])
                                .await;
                        let model = match (site, slot) {
                            (CodeBuddySite::Cn, "kimi") => "kimi-k3-1",
                            (CodeBuddySite::Intl, "kimi") => "kimi-k3",
                            (CodeBuddySite::Cn, _) => "deepseek-v4-pro",
                            (CodeBuddySite::Intl, _) => "deep-model",
                        };
                        // Same capability shape as the read-only CN catalog observation:
                        // mandatory thinking, one declared level, no adjustable menu.
                        *fixture.catalog_override.lock().unwrap() = Some(json!({"code":0,"data":{
                            "enterpriseId":"",
                            "models":[{"id":model,"supportsToolCall":true,"supportsReasoning":true,
                                "onlyReasoning":true,"reasoning":{"effort":fixed_effort,"summary":"auto"}}],
                            "agents":[{"name":"cli","models":[model]}]
                        }}));
                        let state = codebuddy_test_state("buddy-claude-fixed-effort");
                        let (id, _) = install_buddy_provider(
                            &state,
                            product,
                            "fixed-effort",
                            AppKind::Claude,
                            site,
                            &format!("http://{address}"),
                        )
                        .await;
                        let mut providers = state.providers_snapshot().await;
                        providers.providers[0].provider.settings_config["modelMapping"] =
                            json!({"mode":"single","upstreamModel":model});
                        providers
                            .rebuild_runtime_index(&state.accounts_snapshot().await)
                            .unwrap();
                        state.replace_provider_store_for_test(providers).await;
                        install_codebuddy_share(
                            &state,
                            "fixed-effort-share",
                            AppKind::Claude,
                            &id,
                            "owner@example.com",
                        )
                        .await;

                        for effort in ["high", "low", "medium", "max"] {
                            let response = super::forward_for_test_surface(
                            state.clone(), ProxyRoute::ClaudeMessages, id.clone(), None,
                            codebuddy_headers("fixed-effort-share", "owner@example.com", "fixed-effort"),
                            Bytes::from(json!({
                                "model":"claude-opus-5-5","stream":stream,"max_tokens":4096,
                                "system":"You are Claude Code, Anthropic's official CLI for Claude.\nKeep all project instructions.",
                                "thinking":{"type":"adaptive"},"output_config":{"effort":effort},
                                "messages":[{"role":"user","content":"ping"}]
                            }).to_string()),
                        ).await.unwrap_or_else(|error| panic!(
                            "{product:?}/{site:?}/stream={stream}/{effort}: {}", error.message
                        ));
                            assert_eq!(response.status(), StatusCode::OK);
                            assert!(
                                String::from_utf8_lossy(&collect(response).await).contains("ready")
                            );
                            let observations = fixture.generation_observations();
                            let sent = &observations.last().unwrap().body;
                            assert_eq!(sent["model"], model);
                            assert_eq!(sent["reasoning_effort"], fixed_effort);
                            assert_eq!(sent["messages"][0]["content"], "You are a helpful coding assistant.\nKeep all project instructions.");
                            if !stream {
                                let usage = state.usage_snapshot().await;
                                let log = usage.logs.last().unwrap();
                                assert_eq!(log.requested_reasoning_effort.as_deref(), Some(effort));
                                assert_eq!(
                                    log.effective_reasoning_effort.as_deref(),
                                    Some(fixed_effort)
                                );
                            }
                            for key in ["thinking", "output_config", "reasoning"] {
                                assert!(sent.get(key).is_none(), "leaked {key}");
                            }
                        }
                        assert_eq!(fixture.count(&fixture.generation_requests), 4);
                        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                        server.abort();
                    }
                }
            }
        }
    });
}

#[test]
fn buddy_reasoning_all_surfaces_override_native_efforts_and_explicit_disable() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                for route in [
                    ProxyRoute::ClaudeMessages,
                    ProxyRoute::CodexChatCompletions,
                    ProxyRoute::CodexResponses,
                    ProxyRoute::Gemini,
                ] {
                    let (address, fixture, server) =
                        spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success])
                            .await;
                    let model = site_model(site);
                    *fixture.catalog_override.lock().unwrap() = Some(json!({"code":0,"data":{
                        "enterpriseId":"","models":[{"id":model,"onlyReasoning":true,
                            "supportsReasoning":true,"reasoning":{"effort":"medium"}}]
                    }}));
                    let state = codebuddy_test_state("buddy-native-exact-effort");
                    let (id, _) = install_buddy_provider(
                        &state,
                        product,
                        "native-effort",
                        route.app(),
                        site,
                        &format!("http://{address}"),
                    )
                    .await;
                    let mut providers = state.providers_snapshot().await;
                    providers.providers[0].provider.settings_config["modelMapping"] =
                        json!({"mode":"single","upstreamModel":model});
                    providers
                        .rebuild_runtime_index(&state.accounts_snapshot().await)
                        .unwrap();
                    state.replace_provider_store_for_test(providers).await;
                    install_codebuddy_share(
                        &state,
                        "native-effort-share",
                        route.app(),
                        &id,
                        "owner@example.com",
                    )
                    .await;
                    for stream in [false, true] {
                        for effort in ["high", "none", "xhigh", "max"] {
                            let body = match route {
                                ProxyRoute::ClaudeMessages => {
                                    json!({"model":"claude-opus-5-5","stream":stream,"max_tokens":4096,
                                    "thinking":{"type":"disabled"},"output_config":{"effort":effort},
                                    "messages":[{"role":"user","content":"ping"}]})
                                }
                                ProxyRoute::CodexChatCompletions => {
                                    json!({"model":model,"stream":stream,
                                    "reasoning_effort":effort,"reasoning":{"effort":"low"},
                                    "metadata":{"buddyReasoningPolicy":"exact"},
                                    "messages":[{"role":"user","content":"ping"}]})
                                }
                                ProxyRoute::CodexResponses => json!({"model":model,"stream":stream,
                                    "reasoning":{"effort":effort},"input":"ping"}),
                                _ => json!({"contents":[{"role":"user","parts":[{"text":"ping"}]}],
                                    "generationConfig":{"thinkingConfig":{"thinkingLevel":effort.to_ascii_uppercase(),"thinkingBudget":0}}}),
                            };
                            let path = (route == ProxyRoute::Gemini).then(|| {
                                format!(
                                    "models/{model}:{}",
                                    if stream {
                                        "streamGenerateContent"
                                    } else {
                                        "generateContent"
                                    }
                                )
                            });
                            let response = super::forward_for_test_surface(
                                state.clone(),
                                route,
                                id.clone(),
                                path,
                                codebuddy_headers(
                                    "native-effort-share",
                                    "owner@example.com",
                                    "native-effort",
                                ),
                                Bytes::from(body.to_string()),
                            )
                            .await
                            .unwrap_or_else(|error| {
                                panic!("{product:?}/{site:?}/{route:?}/{effort}: {}", error.message)
                            });
                            assert_eq!(response.status(), StatusCode::OK);
                            assert!(
                                String::from_utf8_lossy(&collect(response).await).contains("ready")
                            );
                            let observations = fixture.generation_observations();
                            let sent = &observations.last().unwrap().body;
                            assert_eq!(sent["reasoning_effort"], "medium");
                            assert_eq!(sent["model"], model);
                            for key in ["thinking", "reasoning"] {
                                assert!(sent.get(key).is_none());
                            }
                            assert!(sent.pointer("/generationConfig/thinkingConfig").is_none());
                            assert!(sent
                                .pointer("/metadata/geminiGenerationConfig/thinkingConfig")
                                .is_none());
                        }
                    }
                    assert_eq!(fixture.count(&fixture.generation_requests), 8);
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn buddy_reasoning_unknown_defaults_reject_fixed_models_and_auto_skips_them_consistently() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                for auto in [false, true] {
                    let (address, fixture, server) =
                        spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success])
                            .await;
                    let unavailable = site_model(site);
                    *fixture.catalog_override.lock().unwrap() = Some(json!({"code":0,"data":{
                        "models":[
                            {"id":unavailable,"supportsReasoning":true,"reasoning":{"supportedEfforts":["low","high"]}},
                            {"id":"hy3","supportsReasoning":true,"reasoning":{"defaultEffort":"max","supportedEfforts":["high","xhigh","max"]}}
                        ],"agents":[{"name":"cli","models":[unavailable,"hy3"]}]
                    }}));
                    let state = codebuddy_test_state("buddy-reasoning-default-missing");
                    let (id, _) = install_buddy_provider(
                        &state,
                        product,
                        "default-missing",
                        AppKind::Codex,
                        site,
                        &format!("http://{address}"),
                    )
                    .await;
                    let mut providers = state.providers_snapshot().await;
                    providers.providers[0].provider.settings_config["modelMapping"] = json!({"mode":"single","upstreamModel":if auto { "auto" } else { unavailable }});
                    providers
                        .rebuild_runtime_index(&state.accounts_snapshot().await)
                        .unwrap();
                    state.replace_provider_store_for_test(providers).await;
                    install_codebuddy_share(
                        &state,
                        "default-missing-share",
                        AppKind::Codex,
                        &id,
                        "owner@example.com",
                    )
                    .await;
                    for effort in ["low", "high", "none", "xhigh"] {
                        let result = super::forward_for_test_surface(state.clone(),ProxyRoute::CodexResponses,id.clone(),None,
                            codebuddy_headers("default-missing-share","owner@example.com","default-missing"),
                            Bytes::from(json!({"model":"any-client-model","input":"ping","reasoning":{"effort":effort}}).to_string())
                        ).await;
                        if auto {
                            let response = result.unwrap();
                            assert_eq!(response.status(), StatusCode::OK);
                            collect(response).await;
                            let sent = &fixture
                                .generation_observations()
                                .last()
                                .unwrap()
                                .body
                                .clone();
                            assert_eq!(sent["model"], "hy3");
                            assert_eq!(sent["reasoning_effort"], "max");
                        } else {
                            let error = result.unwrap_err();
                            assert_eq!(error.status, StatusCode::BAD_GATEWAY);
                            assert!(error
                                .message
                                .contains("no unambiguous default reasoning effort"));
                            assert_eq!(fixture.count(&fixture.generation_requests), 0);
                        }
                    }
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                    server.abort();
                }
            }
        }
    });
}
