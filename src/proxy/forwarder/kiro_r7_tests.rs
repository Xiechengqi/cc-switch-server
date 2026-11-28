// Server-owned loopback fixtures. These exercise the production canonical,
// endpoint and auth-recovery paths; they make no vendor acceptance claim.
fn kiro_r7_origin_literal() -> Value {
    json!({
        "origin":"AI_EDITOR",
        "nested":{"origin":"AI_EDITOR","modelId":"literal-model"},
        "items":[{"origin":"AI_EDITOR"}],
        "userInputMessage":{"origin":"AI_EDITOR"},
        "agentContinuationId":"literal-continuation"
    })
}

fn kiro_r7_origin_schema() -> Value {
    json!({
        "type":"object",
        "properties":{"origin":{"type":"string","default":"AI_EDITOR"}},
        "additionalProperties":true,
        "default":kiro_r7_origin_literal(),
        "examples":[kiro_r7_origin_literal()]
    })
}

fn kiro_r7_origin_body(route: ProxyRoute, model: &str, streaming: bool) -> Bytes {
    let literal = kiro_r7_origin_literal();
    let text = literal.to_string();
    let schema = kiro_r7_origin_schema();
    let body = match route {
        ProxyRoute::ClaudeMessages => json!({
            "model":model,"stream":streaming,"max_tokens":64,
            "tools":[{"name":"inspect_payload","input_schema":schema}],
            "messages":[{"role":"user","content":text},
                {"role":"assistant","content":[{"type":"tool_use","id":"call_origin","name":"inspect_payload","input":literal}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_origin","content":text},{"type":"text","text":"continue"}]}]
        }),
        ProxyRoute::CodexChatCompletions => json!({
            "model":model,"stream":streaming,
            "tools":[{"type":"function","function":{"name":"inspect_payload","parameters":schema}}],
            "messages":[{"role":"user","content":text},
                {"role":"assistant","tool_calls":[{"type":"function","id":"call_origin","function":{"name":"inspect_payload","arguments":text}}]},
                {"role":"tool","tool_call_id":"call_origin","content":text},
                {"role":"user","content":"continue"}]
        }),
        ProxyRoute::CodexResponses => json!({
            "model":model,"stream":streaming,
            "tools":[{"type":"function","name":"inspect_payload","parameters":schema}],
            "input":[{"role":"user","content":text},
                {"type":"function_call","call_id":"call_origin","name":"inspect_payload","arguments":text},
                {"type":"function_call_output","call_id":"call_origin","output":text},
                {"role":"user","content":"continue"}]
        }),
        _ => unreachable!(),
    };
    Bytes::from(body.to_string())
}

async fn kiro_r7_check_origin(rail: &str, recover: bool) {
    let kind = if rail == "amazon_q_cli" {
        ProviderType::AmazonQOAuth
    } else {
        ProviderType::KiroOAuth
    };
    let expected_origin = match rail {
        "ide" => "AI_EDITOR",
        "cli" => "KIRO_CLI",
        _ => "CLI",
    };
    let captured = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let seen = captured.clone();
    let token_calls = Arc::new(AtomicUsize::new(0));
    let refreshed = token_calls.clone();
    let upstream = axum::Router::new().fallback(axum::routing::post(
        move |uri: axum::http::Uri, headers: HeaderMap, body: Bytes| {
            let seen = seen.clone();
            let refreshed = refreshed.clone();
            async move {
                // AWS's application/x-amz-json-1.0 is not Axum Json's
                // application/json or +json media type. Parse the wire bytes.
                let body: Value = serde_json::from_slice(&body).unwrap();
                if uri.path() == "/token" {
                    refreshed.fetch_add(1, Ordering::SeqCst);
                    return axum::Json(json!({"accessToken":"r7-new-access","expiresIn":3600,"tokenType":"Bearer"})).into_response();
                }
                if let Some(catalog) = kiro_core03_catalog(&headers) {
                    return catalog;
                }
                seen.lock().unwrap().push(body);
                if recover && headers.get("authorization").and_then(|v|v.to_str().ok()) != Some("Bearer r7-new-access") {
                    return (StatusCode::UNAUTHORIZED, "expired fixture token").into_response();
                }
                ([(CONTENT_TYPE,"application/vnd.amazon.eventstream")],kiro_text_eventstream("origin preserved")).into_response()
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
    ] {
        for streaming in [false, true] {
            let label = format!("kiro-r7-origin-{rail}-{route:?}-{streaming}-{recover}");
            let state = forwarder_test_state(&label);
            let endpoint = format!("http://{address}");
            let id = if kind == ProviderType::AmazonQOAuth {
                let (claude, codex, _) = install_amazon_q_test_bundle(
                    &state,
                    &label,
                    endpoint.clone(),
                    endpoint.clone(),
                    endpoint.clone(),
                )
                .await;
                if route.app() == AppKind::Claude {
                    claude
                } else {
                    codex
                }
            } else {
                let id = install_kiro_test_provider_for_app(
                    &state,
                    route.app(),
                    &label,
                    endpoint.clone(),
                    format!("{endpoint}/token"),
                )
                .await;
                let account_id = format!("{label}-account");
                let endpoint_kind = rail.to_string();
                // Complete the fixture identity before its first admission.
                state
                    .mutate_accounts_immediate(move |accounts| {
                        let account = accounts
                            .accounts
                            .iter_mut()
                            .find(|a| a.id == account_id)
                            .unwrap();
                        account.profile.as_mut().unwrap()["endpoint"] = json!(endpoint_kind);
                    })
                    .await
                    .unwrap();
                id
            };
            let response = Box::pin(forward_for_test_surface(
                state,
                route,
                id,
                None,
                HeaderMap::new(),
                kiro_r7_origin_body(route, kiro_core03_model(kind), streaming),
            ))
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{label}");
            let bytes = collect_response_body(response).await;
            assert!(
                String::from_utf8_lossy(&bytes).contains("origin preserved"),
                "{label}"
            );
            let observations = std::mem::take(&mut *captured.lock().unwrap());
            assert_eq!(observations.len(), if recover { 2 } else { 1 }, "{label}");
            for wire in observations {
                let conversation = &wire["conversationState"];
                assert_eq!(
                    conversation["currentMessage"]["userInputMessage"]["origin"], expected_origin,
                    "{label}"
                );
                let history = conversation["history"].as_array().unwrap();
                let calls = history
                    .iter()
                    .filter_map(|m| {
                        m.pointer("/assistantResponseMessage/toolUses")
                            .and_then(Value::as_array)
                    })
                    .flatten()
                    .collect::<Vec<_>>();
                assert_eq!(calls.len(), 1, "{label}: {wire}");
                assert_eq!(
                    calls[0]["input"],
                    kiro_r7_origin_literal(),
                    "business tool input changed on {label}"
                );
                let tools = conversation
                    .pointer("/currentMessage/userInputMessage/userInputMessageContext/tools")
                    .and_then(Value::as_array)
                    .unwrap();
                let schema = tools
                    .iter()
                    .find_map(|t| t.pointer("/toolSpecification/inputSchema/json"))
                    .unwrap();
                assert_eq!(
                    schema["default"],
                    kiro_r7_origin_literal(),
                    "schema literal changed on {label}"
                );
                assert_eq!(
                    schema["examples"],
                    json!([kiro_r7_origin_literal()]),
                    "{label}"
                );
                assert!(
                    wire.to_string().contains(
                        &serde_json::to_string(&kiro_r7_origin_literal().to_string()).unwrap()
                    ),
                    "quoted user/tool text changed on {label}"
                );
                for message in history {
                    if let Some(user) = message.get("userInputMessage") {
                        assert_eq!(user["origin"], expected_origin, "{label}");
                        assert_eq!(user.get("modelId").is_none(), rail != "ide", "{label}");
                    }
                }
                assert_eq!(
                    conversation.get("agentContinuationId").is_none(),
                    rail != "ide",
                    "{label}"
                );
            }
        }
    }
    assert_eq!(
        token_calls.load(Ordering::SeqCst),
        if recover { 6 } else { 0 }
    );
    server.abort();
}

#[tokio::test]
async fn r7_kiro_ide_origin_keeps_business_payload() {
    kiro_r7_check_origin("ide", false).await;
}

#[tokio::test]
async fn r7_kiro_cli_origin_keeps_business_payload() {
    kiro_r7_check_origin("cli", false).await;
}

#[tokio::test]
async fn r7_kiro_amazon_q_origin_keeps_business_payload() {
    kiro_r7_check_origin("amazon_q_cli", false).await;
}

#[tokio::test]
async fn r7_kiro_cli_origin_recovery_keeps_business_payload_and_single_replay() {
    kiro_r7_check_origin("cli", true).await;
}

#[tokio::test]
async fn r7_kiro_amazon_q_origin_recovery_keeps_business_payload_and_single_replay() {
    kiro_r7_check_origin("amazon_q_cli", true).await;
}

#[tokio::test]
async fn r7_amazon_q_state_recreation_fences_a_paused_discovery() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let (seen, ready, resume) = (calls.clone(), started.clone(), release.clone());
    let app = axum::Router::new().fallback(axum::routing::post(move |headers: HeaderMap| {
        let (seen, ready, resume) = (seen.clone(), ready.clone(), resume.clone());
        async move {
            assert_eq!(
                headers.get("x-amz-target").unwrap(),
                "AmazonCodeWhispererService.ListAvailableModels"
            );
            let n = seen.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ready.notify_one();
                resume.notified().await;
            }
            let model = if n == 0 {
                "old-account-model"
            } else {
                "new-account-model"
            };
            axum::Json(json!({"models":[{"modelId":model}],"defaultModel":{"modelId":model}}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let label = "r7-amazon-q-account-recreation";
    let state = forwarder_test_state(label);
    let (provider, _, _) = install_amazon_q_test_bundle(
        &state,
        label,
        endpoint.clone(),
        endpoint.clone(),
        endpoint.clone(),
    )
    .await;
    let account_id = format!("{label}-account");
    let old_account = state
        .find_account_for_provider_with_incarnation(ProviderType::AmazonQOAuth, &account_id)
        .await
        .unwrap();
    let plan = state
        .providers_snapshot()
        .await
        .runtime_plan(AppKind::Claude, &provider)
        .unwrap()
        .clone();
    let (task_state, task_plan, task_provider, task_account, task_endpoint) = (
        state.clone(),
        plan.clone(),
        provider.clone(),
        account_id.clone(),
        endpoint.clone(),
    );
    let pending = tokio::spawn(async move {
        task_state
            .amazon_q_model_catalog(
                AppKind::Claude,
                &task_provider,
                task_plan.provider_revision,
                &task_plan.runtime_fingerprint,
                &task_account,
                old_account.0.auth_identity_generation,
                Some(&task_endpoint),
                Duration::from_secs(3),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    let restored = old_account.0.clone();
    state
        .mutate_accounts_immediate(|accounts| {
            assert!(accounts.delete(&account_id));
            accounts
                .upsert(serde_json::from_value(serde_json::to_value(restored).unwrap()).unwrap());
        })
        .await
        .unwrap();
    let current = state
        .find_account_for_provider_with_incarnation(ProviderType::AmazonQOAuth, &account_id)
        .await
        .unwrap();
    assert_ne!(old_account.1, current.1);
    assert_eq!(
        old_account.0.auth_identity_generation,
        current.0.auth_identity_generation
    );
    assert_eq!(
        old_account.0.token_refresh_generation,
        current.0.token_refresh_generation
    );
    let plan = state
        .providers_snapshot()
        .await
        .runtime_plan(AppKind::Claude, &provider)
        .unwrap()
        .clone();
    let replacement = state
        .amazon_q_model_catalog(
            AppKind::Claude,
            &provider,
            plan.provider_revision,
            &plan.runtime_fingerprint,
            &account_id,
            current.0.auth_identity_generation,
            Some(&endpoint),
            Duration::from_secs(2),
        )
        .await;
    assert!(replacement.model("new-account-model").is_some());
    release.notify_one();
    let obsolete = pending.await.unwrap();
    assert!(obsolete.descriptors.is_empty());
    assert_eq!(
        obsolete.source,
        "amazon_q_identity_changed_during_discovery"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(
        !state
            .find_account_for_provider_with_incarnation(ProviderType::AmazonQOAuth, &account_id)
            .await
            .unwrap()
            .0
            .needs_relogin
    );
    server.abort();
}

// The process parent is initialized once. Run this fixture alone in a fresh
// test process with CC_SWITCH_PROCESS_MEMORY_BUDGET_MB=1 (see execution ledger).
#[tokio::test]
#[ignore = "requires isolated process with CC_SWITCH_PROCESS_MEMORY_BUDGET_MB=1"]
async fn r7_amazon_q_forwarder_parent_capacity_is_local_503_without_network() {
    assert_eq!(
        std::env::var("CC_SWITCH_PROCESS_MEMORY_BUDGET_MB").as_deref(),
        Ok("1")
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            axum::Json(json!({}))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let label = "r7-amazon-q-parent-capacity";
    let state = forwarder_test_state(label);
    let (provider, _, _) = install_amazon_q_test_bundle(
        &state,
        label,
        endpoint.clone(),
        endpoint.clone(),
        endpoint.clone(),
    )
    .await;
    let error = Box::pin(forward_for_test_surface(
        state.clone(),
        ProxyRoute::ClaudeMessages,
        provider,
        None,
        HeaderMap::new(),
        Bytes::from(
            json!({
                "model":"amazon-q-test-model", "max_tokens":32,
                "messages":[{"role":"user","content":"hello"}]
            })
            .to_string(),
        ),
    ))
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(error.is_request_memory_exhausted(), "{error:?}");
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "catalog, generation and refresh must all remain unsent"
    );
    let account = state
        .find_account_for_provider_with_incarnation(
            ProviderType::AmazonQOAuth,
            &format!("{label}-account"),
        )
        .await
        .unwrap()
        .0;
    assert!(!account.needs_relogin);
    assert!(account.last_refresh_error.is_none());
    server.abort();
}
