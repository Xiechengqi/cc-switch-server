use super::*;

fn qoder_r6_request(
    route: ProxyRoute,
    streaming: bool,
    model: &str,
    effort: Option<&str>,
) -> (Option<String>, Bytes) {
    let (_, path, body) = surface_request(route.app(), streaming);
    let mut body: Value = serde_json::from_slice(&body).unwrap();
    if route == ProxyRoute::CodexChatCompletions {
        body = json!({"stream":streaming,"messages":[{"role":"user","content":"hi"}]});
    }
    body["model"] = json!(model);
    if let Some(effort) = effort {
        match route {
            ProxyRoute::ClaudeMessages => {
                if effort == "none" {
                    body["thinking"] = json!({"type":"disabled"});
                } else {
                    body["thinking"] = json!({"type":"adaptive"});
                    body["output_config"] = json!({"effort":effort});
                }
            }
            ProxyRoute::Gemini => {
                body["generationConfig"] = if effort == "none" {
                    json!({"thinkingConfig":{"thinkingBudget":0}})
                } else {
                    json!({"thinkingConfig":{"thinkingLevel":effort}})
                };
            }
            ProxyRoute::CodexResponses => body["reasoning"] = json!({"effort":effort}),
            ProxyRoute::CodexChatCompletions => body["reasoning_effort"] = json!(effort),
            _ => unreachable!(),
        }
    }
    (
        path.map(|path| path.replace(MODEL_KEY, model)),
        Bytes::from(body.to_string()),
    )
}

async fn qoder_r6_forward(
    state: ServerState,
    route: ProxyRoute,
    id: &str,
    path: Option<String>,
    body: Bytes,
) -> Result<Response, ProxyError> {
    // Qoder Profiles default to a fixed upstream model of auto. Configure the
    // fixture's Provider explicitly before creating the execution snapshot.
    let model = serde_json::from_slice::<Value>(&body).unwrap()["model"]
        .as_str()
        .unwrap()
        .to_string();
    let mut providers = state.providers_snapshot().await;
    let provider = providers
        .providers
        .iter_mut()
        .find(|provider| provider.app == route.app() && provider.provider.id == id)
        .unwrap();
    provider.provider.settings_config =
        json!({"modelMapping":{"mode":"single","upstreamModel":model}});
    let accounts = state.accounts_snapshot().await;
    providers.rebuild_runtime_index(&accounts).unwrap();
    state.replace_provider_store_for_test(providers).await;
    let attempt = qoder_core03_attempt(&state, route, id).await;
    forward_with_attempt(
        state,
        route,
        path,
        qoder_headers("r6-share", "owner@example.com", "r6-session"),
        body,
        attempt,
    )
    .await
}

#[test]
fn qoder_r6_diagnostics_follow_final_wire_controls_without_inventing_toggle_effort() {
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
                for streaming in [false, true] {
                    // Expected values are fixed from the audited existing wire,
                    // never calculated with the production diagnostic helper.
                    for (
                        model,
                        requested,
                        catalog,
                        expected_enabled,
                        expected_wire,
                        expected_effective,
                    ) in [
                        (
                            "gm51model",
                            Some("none"),
                            json!({}),
                            Some(false),
                            Some("none"),
                            Some("none"),
                        ),
                        (
                            "gm51model",
                            Some("high"),
                            json!({}),
                            Some(true),
                            Some("max"),
                            Some("max"),
                        ),
                        (
                            "gmodel",
                            Some("high"),
                            json!({}),
                            Some(true),
                            Some("high"),
                            Some("high"),
                        ),
                        ("qmodel", Some("high"), json!({}), Some(true), None, None),
                        (
                            "gm51model",
                            None,
                            json!({"is_reasoning":true,"reasoning_effort":"high"}),
                            Some(true),
                            Some("high"),
                            Some("high"),
                        ),
                        ("gm51model", None, json!({}), None, None, None),
                        (
                            "gm51model",
                            None,
                            json!({"is_reasoning":true,"reasoning_effort":"fixture-private-default"}),
                            Some(true),
                            Some("fixture-private-default"),
                            None,
                        ),
                    ] {
                        let (address, fixture, server) =
                            spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                        let mut config = model_config();
                        config["key"] = json!(model);
                        for (key, value) in catalog.as_object().unwrap() {
                            config[key] = value.clone();
                        }
                        *fixture.catalog_config.lock().unwrap() = config;
                        let state = qoder_test_state("r6-wire-diagnostics");
                        let (id, _) = install_qoder_provider(
                            &state,
                            "r6-wire",
                            route.app(),
                            rail,
                            &format!("http://{address}"),
                        )
                        .await;
                        install_qoder_share(
                            &state,
                            "r6-share",
                            route.app(),
                            &id,
                            "owner@example.com",
                        )
                        .await;
                        let (path, body) = qoder_r6_request(route, streaming, model, requested);
                        let response = qoder_r6_forward(state.clone(), route, &id, path, body)
                            .await
                            .unwrap();
                        assert_eq!(response.status(), StatusCode::OK);
                        assert!(String::from_utf8_lossy(&collect(response).await).contains("ready"));
                        let sent = fixture.generation_observations();
                        assert_eq!(sent.len(), 1);
                        let config = &sent[0].decoded_body.as_ref().unwrap()["model_config"];
                        assert_eq!(config["key"], model);
                        assert_eq!(
                            config.get("is_reasoning").and_then(Value::as_bool),
                            expected_enabled,
                            "{rail:?} {route:?} {model} {requested:?}"
                        );
                        assert_eq!(
                            config.get("reasoning_effort").and_then(Value::as_str),
                            expected_wire
                        );
                        let usage = state.usage_snapshot().await;
                        let log = usage.logs.last().unwrap();
                        assert_eq!(
                            log.requested_reasoning_effort.as_deref(),
                            requested,
                            "{rail:?} {route:?} {model}"
                        );
                        assert_eq!(
                            log.effective_reasoning_effort.as_deref(),
                            expected_effective,
                            "{rail:?} {route:?} {model}"
                        );
                        assert_eq!(log.outcome, UsageOutcome::Success);
                        assert_eq!(log.attempt_count, 1);
                        server.abort();
                    }
                }
            }
        }
    });
}

#[test]
fn qoder_r6_same_account_auth_recovery_retains_prepared_diagnostics_and_mapping() {
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
                for streaming in [false, true] {
                    let (address, fixture, server) = spawn_qoder_upstream(
                        rail,
                        vec![
                            QoderGenerationReply::HttpUnauthorized,
                            QoderGenerationReply::Success,
                        ],
                    )
                    .await;
                    let mut config = model_config();
                    config["key"] = json!("gm51model");
                    *fixture.catalog_config.lock().unwrap() = config;
                    let state = qoder_test_state("r6-refresh-diagnostics");
                    let (id, account_id) = install_qoder_provider(
                        &state,
                        "r6-refresh",
                        route.app(),
                        rail,
                        &format!("http://{address}"),
                    )
                    .await;
                    install_qoder_share(&state, "r6-share", route.app(), &id, "owner@example.com")
                        .await;
                    let (before, _) = state
                        .find_account_for_provider_with_incarnation(
                            ProviderType::QoderCosy,
                            &account_id,
                        )
                        .await
                        .unwrap();
                    let (path, body) =
                        qoder_r6_request(route, streaming, "gm51model", Some("high"));
                    let response = qoder_r6_forward(state.clone(), route, &id, path, body)
                        .await
                        .unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    assert!(String::from_utf8_lossy(&collect(response).await).contains("ready"));
                    let sent = fixture.generation_observations();
                    assert_eq!(sent.len(), 2);
                    for request in &sent {
                        assert_eq!(
                            request.decoded_body.as_ref().unwrap()["model_config"]
                                ["reasoning_effort"],
                            "max"
                        );
                        assert_eq!(
                            request.decoded_body.as_ref().unwrap()["model_config"]["is_reasoning"],
                            true
                        );
                    }
                    let (after, _) = state
                        .find_account_for_provider_with_incarnation(
                            ProviderType::QoderCosy,
                            &account_id,
                        )
                        .await
                        .unwrap();
                    assert_eq!(
                        before.auth_identity_generation,
                        after.auth_identity_generation
                    );
                    let usage = state.usage_snapshot().await;
                    let log = usage.logs.last().unwrap();
                    assert_eq!(log.requested_reasoning_effort.as_deref(), Some("high"));
                    assert_eq!(log.effective_reasoning_effort.as_deref(), Some("max"));
                    assert_eq!(log.attempt_count, 2);
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_r6_failed_preparation_does_not_claim_an_effective_effort() {
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
                for streaming in [false, true] {
                    let (address, fixture, server) =
                        spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                    let mut config = model_config();
                    config["key"] = json!("gm51model");
                    *fixture.catalog_config.lock().unwrap() = config;
                    let state = qoder_test_state("r6-unsent-diagnostics");
                    let (id, _) = install_qoder_provider(
                        &state,
                        "r6-unsent",
                        route.app(),
                        rail,
                        &format!("http://{address}"),
                    )
                    .await;
                    install_qoder_share(&state, "r6-share", route.app(), &id, "owner@example.com")
                        .await;
                    let (path, body) =
                        qoder_r6_request(route, streaming, "gm51model", Some("high"));
                    let mut body: Value = serde_json::from_slice(&body).unwrap();
                    body["context_length"] = json!(-1);
                    let error = qoder_r6_forward(
                        state.clone(),
                        route,
                        &id,
                        path,
                        Bytes::from(body.to_string()),
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(error.status, StatusCode::BAD_REQUEST);
                    assert!(error.message.contains("context_length"));
                    assert_eq!(fixture.count(&fixture.generation_requests), 0);
                    let usage = state.usage_snapshot().await;
                    let log = usage.logs.last().unwrap();
                    assert_eq!(log.requested_reasoning_effort.as_deref(), Some("high"));
                    assert_eq!(log.effective_reasoning_effort, None);
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_r6_exact_budgets_invalid_and_conflicting_controls_do_not_generate() {
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
                for streaming in [false, true] {
                    for case in ["budget", "bad_type", "unknown", "conflict"] {
                        let (address, fixture, server) =
                            spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                        let mut config = model_config();
                        config["key"] = json!("gm51model");
                        *fixture.catalog_config.lock().unwrap() = config;
                        let state = qoder_test_state("r6-source-controls");
                        let (id, _) = install_qoder_provider(
                            &state,
                            "r6-source",
                            route.app(),
                            rail,
                            &format!("http://{address}"),
                        )
                        .await;
                        install_qoder_share(
                            &state,
                            "r6-share",
                            route.app(),
                            &id,
                            "owner@example.com",
                        )
                        .await;
                        let (path, body) = qoder_r6_request(
                            route,
                            streaming,
                            "gm51model",
                            Some(if case == "unknown" {
                                "fixture-private-effort"
                            } else {
                                "high"
                            }),
                        );
                        let mut body: Value = serde_json::from_slice(&body).unwrap();
                        match (case, route) {
                            ("unknown", _) => (),
                            ("budget", ProxyRoute::ClaudeMessages) => {
                                body["thinking"] = json!({"type":"enabled","budget_tokens":1024})
                            }
                            ("budget", ProxyRoute::Gemini) => {
                                body["generationConfig"]["thinkingConfig"]["thinkingBudget"] =
                                    json!(1024)
                            }
                            ("budget", ProxyRoute::CodexResponses) => {
                                body["reasoning"]["budget_tokens"] = json!(1024)
                            }
                            ("budget", _) => body["reasoning_budget_tokens"] = json!(1024),
                            ("bad_type", ProxyRoute::ClaudeMessages) => {
                                body["output_config"]["effort"] = json!(123)
                            }
                            ("bad_type", ProxyRoute::Gemini) => {
                                body["generationConfig"]["thinkingConfig"]["thinkingLevel"] =
                                    json!(123)
                            }
                            ("bad_type", ProxyRoute::CodexResponses) => {
                                body["reasoning"]["effort"] = json!(123)
                            }
                            ("bad_type", _) => body["reasoning_effort"] = json!(123),
                            ("conflict", ProxyRoute::ClaudeMessages) => {
                                body["thinking"] = json!({"type":"disabled"})
                            }
                            ("conflict", ProxyRoute::Gemini) => {
                                body["generationConfig"]["thinkingConfig"]["thinkingBudget"] =
                                    json!(0)
                            }
                            ("conflict", _) => {
                                body["reasoning_effort"] = json!("high");
                                body["reasoning"] = json!({"effort":"none"});
                            }
                            _ => unreachable!(),
                        }
                        match qoder_r6_forward(
                            state,
                            route,
                            &id,
                            path,
                            Bytes::from(body.to_string()),
                        )
                        .await
                        {
                            Err(error) => {
                                assert_eq!(
                                    error.status,
                                    StatusCode::BAD_REQUEST,
                                    "{rail:?} {route:?} {case}"
                                );
                                assert!(!error.message.contains("fixture-private-effort"));
                            }
                            Ok(response) => {
                                drop(collect(response).await);
                                panic!("{rail:?} {route:?} {case}: unsupported source control was accepted");
                            }
                        }
                        assert_eq!(fixture.count(&fixture.generation_requests), 0);
                        assert_eq!(fixture.count(&fixture.model_requests), 0);
                        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                        server.abort();
                    }
                }
            }
        }
    });
}

#[test]
fn qoder_r6_gemini_dynamic_budget_does_not_invent_an_effort() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for streaming in [false, true] {
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let mut config = model_config();
                config["key"] = json!("gm51model");
                *fixture.catalog_config.lock().unwrap() = config;
                let state = qoder_test_state("r6-dynamic-budget");
                let route = ProxyRoute::Gemini;
                let (id, _) = install_qoder_provider(
                    &state,
                    "r6-dynamic",
                    route.app(),
                    rail,
                    &format!("http://{address}"),
                )
                .await;
                install_qoder_share(&state, "r6-share", route.app(), &id, "owner@example.com")
                    .await;
                let (path, body) = qoder_r6_request(route, streaming, "gm51model", None);
                let mut body: Value = serde_json::from_slice(&body).unwrap();
                body["generationConfig"] = json!({"thinkingConfig":{"thinkingBudget":-1}});
                let response = qoder_r6_forward(
                    state.clone(),
                    route,
                    &id,
                    path,
                    Bytes::from(body.to_string()),
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                drop(collect(response).await);
                let sent = fixture.generation_observations();
                assert_eq!(sent.len(), 1);
                let config = &sent[0].decoded_body.as_ref().unwrap()["model_config"];
                assert!(config.get("reasoning_effort").is_none());
                assert!(config.get("is_reasoning").is_none());
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().unwrap();
                assert_eq!(log.requested_reasoning_effort, None);
                assert_eq!(log.effective_reasoning_effort, None);
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_r6_unreviewed_catalog_thinking_does_not_authorize_disable() {
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
                for streaming in [false, true] {
                    for requested in [None, Some("none")] {
                        let (address, fixture, server) =
                            spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                        let model = "future-r6-control";
                        let mut config = model_config();
                        config["key"] = json!(model);
                        config["thinking_config"] = json!({"enabled":{"efforts":{"low":{"is_default":true}}},"disabled":{}});
                        *fixture.catalog_config.lock().unwrap() = config;
                        let state = qoder_test_state("r6-unknown-thinking");
                        let (id, _) = install_qoder_provider(
                            &state,
                            "r6-unknown",
                            route.app(),
                            rail,
                            &format!("http://{address}"),
                        )
                        .await;
                        install_qoder_share(
                            &state,
                            "r6-share",
                            route.app(),
                            &id,
                            "owner@example.com",
                        )
                        .await;
                        let (path, body) = qoder_r6_request(route, streaming, model, requested);
                        let result = qoder_r6_forward(state.clone(), route, &id, path, body).await;
                        if requested.is_some() {
                            let error = result.unwrap_err();
                            assert_eq!(error.status, StatusCode::BAD_REQUEST);
                            assert!(error
                                .message
                                .contains("no audited reasoning-disable control"));
                            assert_eq!(fixture.count(&fixture.generation_requests), 0);
                        } else {
                            let response = result.unwrap();
                            assert_eq!(response.status(), StatusCode::OK);
                            drop(collect(response).await);
                            let sent = fixture.generation_observations();
                            assert_eq!(sent.len(), 1);
                            let config = &sent[0].decoded_body.as_ref().unwrap()["model_config"];
                            assert!(config.get("is_reasoning").is_none());
                            assert!(config.get("reasoning_effort").is_none());
                        }
                        let usage = state.usage_snapshot().await;
                        let log = usage.logs.last().unwrap();
                        assert_eq!(log.requested_reasoning_effort.as_deref(), requested);
                        assert_eq!(log.effective_reasoning_effort, None);
                        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                        server.abort();
                    }
                }
            }
        }
    });
}
