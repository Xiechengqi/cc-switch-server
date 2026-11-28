// Exercise the production preparation facade and native send boundary with a
// loopback upstream. No fabricated caller binding or timing-based barrier.
async fn buddy_r6_prepared_catalog_change_sends_no_generation(change: &str) {
    for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
        for site in [CodeBuddySite::Intl, CodeBuddySite::Cn] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let (address, fixture, server) =
                    spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success]).await;
                let name = format!("r6-buddy-{product:?}-{site:?}-{app:?}-{change}");
                let state = codebuddy_test_state(&name);
                let (id, _) = install_buddy_provider(
                    &state,
                    product,
                    &name,
                    app,
                    site,
                    &format!("http://{address}"),
                )
                .await;
                let route = match app {
                    AppKind::Claude => ProxyRoute::ClaudeMessages,
                    AppKind::Codex => ProxyRoute::CodexResponses,
                    AppKind::Gemini => ProxyRoute::Gemini,
                };
                let attempt = codebuddy_core03_attempt(&state, route, &id).await;
                let execution = attempt.execution.as_ref().unwrap();
                let bound = codebuddy_provider::bound_account_identity(execution).unwrap();
                let runtime = codebuddy_provider::prepare_runtime(&state, execution, &bound)
                    .await
                    .unwrap();
                let canonical = codebuddy_provider::CanonicalRequest {
                    requested_model: site_model(site).to_string(),
                    body: json!({
                        "model":site_model(site),
                        "messages":[{"role":"user","content":"hello"}],
                        "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}],
                        "reasoning_effort":"low",
                    }),
                };
                let generation =
                    codebuddy_provider::prepare_generation(&runtime, &canonical, None).unwrap();
                assert_eq!(
                    generation.payload["reasoning_effort"], "high",
                    "the bound catalog default is authoritative"
                );
                let mut replacement = (*runtime.catalog).clone();
                match change {
                    "empty" => {
                        replacement.enabled_models.clear();
                        replacement.preferred_models.clear();
                    }
                    "tools" => {
                        replacement
                            .capabilities
                            .get_mut(site_model(site))
                            .unwrap()
                            .supports_tools = false
                    }
                    "default" => {
                        replacement
                            .capabilities
                            .get_mut(site_model(site))
                            .unwrap()
                            .default_reasoning_effort = Some("low".to_string())
                    }
                    _ => unreachable!(),
                }
                state
                    .codebuddy_runtime
                    .insert_catalog(
                        runtime.scope.clone(),
                        replacement,
                        crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
                    )
                    .await;
                let mut usage = StreamUsageAccumulator::default();
                let result = super::send_codebuddy_generation(
                    &state,
                    execution,
                    &runtime,
                    &generation.payload,
                    None,
                    None,
                    &mut usage,
                )
                .await;
                assert_eq!(fixture.count(&fixture.generation_requests), 0,
                    "an old prepared {change} grant reached the upstream: {product:?}/{site:?}/{app:?}");
                match result {
                    Err(CodeBuddyForwardAttemptError::Proxy(error)) => {
                        assert_eq!(error.status, StatusCode::CONFLICT)
                    }
                    _ => panic!("catalog replacement must reject the old prepared request"),
                }
                assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                if change == "default" {
                    let fresh = codebuddy_provider::prepare_runtime(&state, execution, &bound)
                        .await
                        .unwrap();
                    let generation =
                        codebuddy_provider::prepare_generation(&fresh, &canonical, None).unwrap();
                    assert_eq!(generation.payload["reasoning_effort"], "low");
                    let result = super::send_codebuddy_generation(
                        &state,
                        execution,
                        &fresh,
                        &generation.payload,
                        None,
                        None,
                        &mut usage,
                    )
                    .await;
                    assert!(
                        result.is_ok(),
                        "a newly prepared current snapshot must remain usable"
                    );
                    assert_eq!(fixture.count(&fixture.generation_requests), 1);
                    assert_eq!(
                        fixture.generation_observations()[0].body["reasoning_effort"],
                        "low"
                    );
                }
                server.abort();
            }
        }
    }
}

#[test]
fn buddy_r6_prepared_empty_catalog_is_fenced_before_native_send() {
    run_codebuddy_async_test(buddy_r6_prepared_catalog_change_sends_no_generation(
        "empty",
    ));
}

#[test]
fn buddy_r6_prepared_tool_permission_is_fenced_before_native_send() {
    run_codebuddy_async_test(buddy_r6_prepared_catalog_change_sends_no_generation(
        "tools",
    ));
}

#[test]
fn buddy_r6_prepared_default_change_is_fenced_before_native_send() {
    run_codebuddy_async_test(buddy_r6_prepared_catalog_change_sends_no_generation(
        "default",
    ));
}
