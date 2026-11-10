// Backend-only local upstream fixtures. No browser, real credential or payment.
fn value_quota_fixture_stream(app: AppKind, reply: &Value) -> String {
    if reply["object"] == "chat.completion" {
        let chunk = json!({"id":reply["id"],"object":"chat.completion.chunk","model":reply["model"],
            "choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]});
        let end = json!({"id":reply["id"],"object":"chat.completion.chunk","model":reply["model"],
            "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":reply["usage"]});
        return format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n");
    }
    match app {
        AppKind::Claude => {
            let start = json!({"type":"message_start","message":{"id":"msg_fixture","type":"message","role":"assistant","model":reply["model"],"content":[],"usage":{"input_tokens":1000,"output_tokens":0}}});
            let block = json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}});
            let delta = json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}});
            let stop = json!({"type":"content_block_stop","index":0});
            let usage = json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":1000}});
            format!("event: message_start\ndata: {start}\n\nevent: content_block_start\ndata: {block}\n\nevent: content_block_delta\ndata: {delta}\n\nevent: content_block_stop\ndata: {stop}\n\nevent: message_delta\ndata: {usage}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n")
        }
        AppKind::Codex => {
            let start = json!({"type":"response.created","response":{"id":reply["id"],"object":"response","status":"in_progress","model":reply["model"],"output":[]}});
            let completed = json!({"type":"response.completed","response":reply});
            format!("event: response.created\ndata: {start}\n\nevent: response.completed\ndata: {completed}\n\n")
        }
        AppKind::Gemini => format!("data: {reply}\n\n"),
    }
}

#[tokio::test]
async fn value_quota_native_text_surfaces_admit_before_send_and_settle_exactly() {
    use crate::domain::providers::credentials::CredentialPatch;
    use crate::domain::providers::registry::ProfileId;
    use crate::domain::usage::value_pricing::PricingSnapshot;
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    use std::sync::atomic::{AtomicUsize, Ordering};

    for (app, route, profile, base_key, request, reply, gemini_path) in [
        (
            AppKind::Claude,
            ProxyRoute::ClaudeMessages,
            "claude.anthropic_api_key",
            "ANTHROPIC_BASE_URL",
            json!({"model":"text-model","max_tokens":1000,"messages":[{"role":"user","content":"hello"}],"stream":false}),
            json!({"id":"msg_fixture","type":"message","role":"assistant","model":"text-model","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":1000,"output_tokens":1000}}),
            None,
        ),
        (
            AppKind::Codex,
            ProxyRoute::CodexResponses,
            "codex.openai_api_key",
            "OPENAI_BASE_URL",
            json!({"model":"text-model","input":"hello","stream":false}),
            json!({"id":"resp_fixture","object":"response","status":"completed","model":"text-model","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":1000,"output_tokens":1000,"total_tokens":2000}}),
            None,
        ),
        (
            AppKind::Codex,
            ProxyRoute::CodexChatCompletions,
            "codex.openai_api_key",
            "OPENAI_BASE_URL",
            json!({"model":"text-model","messages":[{"role":"user","content":"hello"}],"stream":false}),
            // The native OpenAI runtime uses Responses upstream, even when
            // the caller chooses the Chat Completions compatibility surface.
            json!({"id":"resp_chat_fixture","object":"response","status":"completed","model":"text-model","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":1000,"output_tokens":1000,"total_tokens":2000}}),
            None,
        ),
        (
            AppKind::Gemini,
            ProxyRoute::Gemini,
            "gemini.google_api_key",
            "GOOGLE_GEMINI_BASE_URL",
            json!({"contents":[{"role":"user","parts":[{"text":"hello"}]}]}),
            json!({"modelVersion":"text-model","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":1000,"totalTokenCount":2000}}),
            Some("models/text-model:generateContent".to_string()),
        ),
    ] {
        let protocols = if app == AppKind::Codex {
            vec![
                None,
                Some("anthropic_messages"),
                Some("open_ai_chat"),
                Some("open_ai_responses"),
            ]
        } else {
            vec![
                None,
                Some("anthropic_messages"),
                Some("open_ai_chat"),
                Some("open_ai_responses"),
                Some("gemini_native"),
            ]
        };
        for protocol in protocols {
            let (profile, binding) = if let Some(protocol) = protocol {
                (match app { AppKind::Claude=>"claude.custom_http", AppKind::Codex=>"codex.custom_http", AppKind::Gemini=>"gemini.custom_http" },
                Some(serde_json::from_value(json!({"upstreamProtocol":protocol,"authScheme":if protocol=="gemini_native" {"api_key"} else {"bearer"}})).unwrap()))
            } else {
                (profile, None)
            };
            let (upstream_app, reply) = match protocol {
                Some("anthropic_messages") => (
                    AppKind::Claude,
                    json!({"id":"msg_fixture","type":"message","role":"assistant","model":"text-model","content":[{"type":"text","text":"hello"}],"stop_reason":"end_turn","usage":{"input_tokens":1000,"output_tokens":1000}}),
                ),
                Some("open_ai_chat") => (
                    AppKind::Codex,
                    json!({"id":"chat_fixture","object":"chat.completion","model":"text-model","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000,"completion_tokens":1000,"total_tokens":2000}}),
                ),
                Some("open_ai_responses") => (
                    AppKind::Codex,
                    json!({"id":"resp_fixture","object":"response","status":"completed","model":"text-model","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":1000,"output_tokens":1000,"total_tokens":2000}}),
                ),
                Some("gemini_native") => (
                    AppKind::Gemini,
                    json!({"modelVersion":"text-model","candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1000,"candidatesTokenCount":1000,"totalTokenCount":2000}}),
                ),
                _ => (app, reply.clone()),
            };
            for streaming in [false, true] {
                for mismatched_model in [false, true] {
                    let mut request = request.clone();
                    if app != AppKind::Gemini {
                        request["stream"] = json!(streaming);
                    }
                    let gemini_path = gemini_path.as_ref().map(|path| {
                        if streaming {
                            path.replace(":generateContent", ":streamGenerateContent")
                        } else {
                            path.clone()
                        }
                    });
                    let mut reply = reply.clone();
                    if mismatched_model {
                        reply[if upstream_app == AppKind::Gemini {
                            "modelVersion"
                        } else {
                            "model"
                        }] = json!("unpriced-model");
                    }
                    let state = forwarder_test_state("value-quota-native-text");
                    state
                        .configure_value_quota_router_for_test("fixture-value-installation")
                        .await;
                    let config = state.config_snapshot().await;
                    server_sqlite::activate_authority(&state.config_dir).unwrap();
                    let scope = ledger::Scope {
                        router_url: config.router_api_base().unwrap().into(),
                        installation_id: "fixture-value-installation".into(),
                        share_id: "fixture-value-share".into(),
                        email: "owner@example.test".into(),
                        entitlement_id: None,
                    };
                    let fixture: Value = serde_json::from_str(include_str!(
                        "../../../tests/fixtures/share-value-pricing-v1.json"
                    ))
                    .unwrap();
                    let snapshot: PricingSnapshot =
                        serde_json::from_value(fixture["snapshot"].clone()).unwrap();
                    let revision = snapshot.revision().unwrap();
                    ledger::install_snapshot(
                        &state.config_dir,
                        &scope,
                        &revision,
                        &snapshot,
                        crate::infra::time::now_ms() as i64,
                    )
                    .unwrap();
                    ledger::activate_snapshot(&state.config_dir, &scope, &revision).unwrap();

                    let listener =
                        tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                            .await
                            .unwrap();
                    let address = listener.local_addr().unwrap();
                    let calls = Arc::new(AtomicUsize::new(0));
                    let observed_calls = Arc::clone(&calls);
                    let config_dir = state.config_dir.clone();
                    let upstream = axum::Router::new().fallback(axum::routing::post(move |uri:axum::http::Uri, body:Bytes| {
            let calls = Arc::clone(&observed_calls);
            let config_dir = config_dir.clone();
            let reply = reply.clone();
            async move {
                // Verify the durable IMMEDIATE admission exists before the
                // upstream sees the first byte, not merely after settlement.
                let db = rusqlite::Connection::open(server_sqlite::database_path(&config_dir)).unwrap();
                if uri.path().ends_with(":countTokens") {
                    assert_eq!(db.query_row("SELECT count(*) FROM value_quota_attempts", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
                    return Response::builder().header(CONTENT_TYPE, "application/json")
                        .body(Body::from(json!({"totalTokens":7}).to_string())).unwrap();
                }
                let pending: (String, String) = db.query_row(
                    "SELECT pending_risk_picos,metering_status FROM value_quota_attempts ORDER BY watermark DESC LIMIT 1",
                    [], |row| Ok((row.get(0)?, row.get(1)?)),
                ).unwrap();
                assert_eq!(pending, ("100000000".into(), "pending".into()));
                calls.fetch_add(1, Ordering::SeqCst);
                let sent:Value = serde_json::from_slice(&body).unwrap();
                let upstream_stream = sent["stream"] == true || uri.path().ends_with(":streamGenerateContent");
                let body = if upstream_stream {
                    value_quota_fixture_stream(upstream_app, &reply)
                } else {
                    reply.to_string()
                };
                Response::builder()
                    .header(CONTENT_TYPE, if upstream_stream {"text/event-stream"} else {"application/json"})
                    .body(Body::from(body))
                    .unwrap()
            }
        }));
                    let server = tokio::spawn(async move {
                        axum::serve(listener, upstream).await.unwrap();
                    });
                    let stored = state.upsert_provider_draft_command(crate::domain::providers::credentials::ProviderWriteDraft {
            app,
            provider: Provider {
                id: "value-native-provider".into(),
                name: "Value native provider".into(),
                settings_config: json!({"env":{(base_key):format!("http://{address}")},"models":["text-model","unpriced-model"],"modelMapping":{"mode":"passthrough"}}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            profile_id: Some(ProfileId::parse(profile).unwrap()), custom_binding: binding.clone(),
            expected_revision: None, client_request_id: None,
            credential_patches: BTreeMap::from([("/settingsConfig/apiKey".into(), CredentialPatch::Replace {value:"fixture-only-key".into()})]),
        }).await.unwrap().unwrap();
                    // Native profiles intentionally ignore caller-supplied base URLs.
                    // Override only the cfg(test) runtime index, and assert loopback before
                    // forwarding so this fixture never depends on an external endpoint.
                    let mut providers = state.providers_snapshot().await;
                    let mut plan = providers
                        .runtime_plan(app, "value-native-provider")
                        .unwrap()
                        .as_ref()
                        .clone();
                    plan.endpoint = format!("http://{address}");
                    plan.runtime_fingerprint = format!("value-fixture-{}-{route:?}", app.as_str());
                    assert!(url::Url::parse(&plan.endpoint)
                        .unwrap()
                        .host_str()
                        .is_some_and(|host| host == "127.0.0.1"));
                    Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
                    state.replace_provider_store_for_test(providers).await;
                    state.mutate_shares_immediate(|store| {
            store.shares.push(serde_json::from_value(json!({
                "id":"fixture-value-share","app":app.as_str(),"providerId":"value-native-provider",
                "providerType":stored.provider_type,"ownerEmail":"owner@example.test","enabled":true,"status":"active",
                "bindings":[{"app":app.as_str(),"providerId":"value-native-provider","providerType":stored.provider_type}],
                "userGrants":{"owner@example.test":{"email":"owner@example.test","role":"owner","active":true,
                    "policy":{"tokenPeriod":"lifetime","valueQuota":{"limitUsdMicros":"100","tariffPolicyId":"api-equivalent-v1"}}}}
            })).unwrap());
        }).await.unwrap();
                    let grant = state
                        .shares
                        .read()
                        .await
                        .get("fixture-value-share")
                        .unwrap()
                        .user_grants["owner@example.test"]
                        .clone();
                    assert!(grant.pending_value_quota.is_none());
                    let window = ledger::Window {
                        id: "Lifetime:0:0".into(),
                        starts_at_ms: 0,
                        ends_at_ms: None,
                        metered_from_ms: grant
                            .value_metering
                            .as_ref()
                            .unwrap()
                            .value_metered_from_ms,
                    };
                    let mut headers = HeaderMap::new();
                    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                    headers.insert(
                        "x-cc-switch-share-id",
                        HeaderValue::from_static("fixture-value-share"),
                    );
                    headers.insert(
                        "x-cc-switch-user-email",
                        HeaderValue::from_static("owner@example.test"),
                    );

                    if app == AppKind::Gemini {
                        if protocol.is_none() || protocol == Some("gemini_native") {
                            let response = forward(
                                state.clone(),
                                route,
                                Some("models/text-model:countTokens".into()),
                                headers.clone(),
                                Bytes::from(serde_json::to_vec(&request).unwrap()),
                            )
                            .await
                            .unwrap();
                            assert_eq!(response.status(), StatusCode::OK);
                            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                                .await
                                .unwrap();
                            assert_eq!(
                                serde_json::from_slice::<Value>(&body).unwrap()["totalTokens"],
                                7
                            );
                            assert_eq!(calls.load(Ordering::SeqCst), 0);
                            let status =
                                ledger::status(&state.config_dir, &scope, &window).unwrap();
                            assert_eq!(status.pending_risk_picos, "0");
                            assert_eq!(status.observed_picos, "0");
                        }
                        for action in ["embedContent", "batchGenerateContent"] {
                            let error = forward(
                                state.clone(),
                                route,
                                Some(format!("models/text-model:{action}")),
                                headers.clone(),
                                Bytes::from(serde_json::to_vec(&request).unwrap()),
                            )
                            .await
                            .unwrap_err();
                            assert_eq!(error.error_code(), "quota_unavailable", "{action}");
                            assert_eq!(calls.load(Ordering::SeqCst), 0);
                            let status =
                                ledger::status(&state.config_dir, &scope, &window).unwrap();
                            assert_eq!(status.pending_risk_picos, "0", "{action}");
                            assert_eq!(status.observed_picos, "0", "{action}");
                        }
                    }
                    {
                        let mut unpriced = request.clone();
                        if app != AppKind::Gemini {
                            unpriced["model"] = json!("unpriced-model");
                        }
                        let unpriced_path = gemini_path
                            .as_ref()
                            .map(|path| path.replace("text-model", "unpriced-model"));
                        let error = forward(
                            state.clone(),
                            route,
                            unpriced_path,
                            headers.clone(),
                            Bytes::from(serde_json::to_vec(&unpriced).unwrap()),
                        )
                        .await
                        .unwrap_err();
                        assert_eq!(
                            error.error_code(),
                            "price_unavailable",
                            "{profile} {route:?} streaming={streaming}"
                        );
                        assert_eq!(calls.load(Ordering::SeqCst), 0);
                    }
                    let response = forward(
                    state.clone(),
                    route,
                    gemini_path.clone(),
                    headers.clone(),
                    Bytes::from(serde_json::to_vec(&request).unwrap()),
                )
                .await
                .unwrap_or_else(|error| panic!("{profile} {app:?} {route:?} streaming={streaming} mismatch={mismatched_model}: {error:?}"));
                    assert_eq!(response.status(), StatusCode::OK, "{app:?} {route:?}");
                    let response_body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                        .await
                        .unwrap();
                    if route == ProxyRoute::CodexChatCompletions && !streaming {
                        let response: Value = serde_json::from_slice(&response_body).unwrap();
                        assert_eq!(response["choices"][0]["message"]["content"], "hello");
                    }
                    let status = ledger::status(&state.config_dir, &scope, &window).unwrap();
                    let label =
                    format!("{profile} {app:?} {route:?} streaming={streaming} mismatch={mismatched_model}");
                    // A model-evidence mismatch is outcome-unknown: the observed
                    // usage (or the reserved risk, whichever is higher) is charged
                    // conservatively instead of blocking the window.
                    assert_eq!(status.observed_picos, "18000000000", "{label}");
                    assert_eq!(status.pending_risk_picos, "0", "{label}");
                    assert_eq!(
                        status.metering_status,
                        if mismatched_model {
                            "conservative"
                        } else {
                            "observed"
                        },
                        "{label}"
                    );
                    // A soft-limit overshoot is settled exactly; the next request must
                    // fail without touching the local upstream again.
                    let error = forward(
                        state.clone(),
                        route,
                        gemini_path.clone(),
                        headers.clone(),
                        Bytes::from(serde_json::to_vec(&request).unwrap()),
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(error.error_code(), "value_quota_exhausted", "{label}");
                    assert_eq!(error.status, StatusCode::FORBIDDEN, "{label}");
                    assert!(!error.retryable());
                    assert_eq!(calls.load(Ordering::SeqCst), 1);

                    if app == AppKind::Codex && protocol.is_none() {
                        let web = install_web_session_forwarder_test_provider(
                            &state,
                            AppKind::Codex,
                            "perplexity",
                            format!("http://{address}/unpriced-web-session"),
                            1,
                        )
                        .await;
                        // Simulate an existing Provider changing runtime profile without
                        // inventing a new persisted Provider foreign key in the Share.
                        let mut stored = web.stored.clone();
                        stored.provider.id = "value-native-provider".into();
                        let mut providers = ProviderStore {
                            providers: vec![stored],
                            ..ProviderStore::default()
                        };
                        providers
                            .rebuild_runtime_index(&state.accounts_snapshot().await)
                            .unwrap();
                        state.replace_provider_store_for_test(providers).await;
                        let error = forward(
                            state.clone(),
                            route,
                            None,
                            headers,
                            Bytes::from(serde_json::to_vec(&request).unwrap()),
                        )
                        .await
                        .unwrap_err();
                        assert_eq!(
                            error.error_code(),
                            "price_unavailable",
                            "the Web Session execution model needs its own explicit price"
                        );
                        assert!(error.message.contains("model_unpriced"), "{error:?}");
                        assert_eq!(calls.load(Ordering::SeqCst), 1);
                    }
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn value_quota_deepseek_cached_recovery_admits_each_real_generation() {
    use crate::domain::usage::value_pricing::{PricingSnapshot, ValueQuota};
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    let (address, observations, server) =
        spawn_deepseek_session_fixture(DeepSeekSessionFixtureMode::RebuildCachedSession).await;
    let state = forwarder_test_state("value-deepseek-rebuild");
    state
        .configure_value_quota_router_for_test("value-deepseek-install")
        .await;
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    let (provider_id, _) = install_deepseek_forwarder_test_provider(
        &state,
        "value-deepseek-rebuild",
        format!("http://{address}"),
    )
    .await;
    // This helper installs a runtime-only Provider; persist it through the
    // domain writer before the Share creates its SQLite foreign key.
    state.mutate_providers_immediate(|_| ()).await.unwrap();
    let share_id = "value-deepseek-share";
    install_antigravity_test_share(
        &state,
        share_id,
        AppKind::Claude,
        ProviderType::DeepSeekAccount,
        &provider_id,
    )
    .await;
    let config = state.config_snapshot().await;
    let scope = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "value-deepseek-install".into(),
        share_id: share_id.into(),
        email: "owner@example.com".into(),
        entitlement_id: None,
    };
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    let mut snapshot: PricingSnapshot =
        serde_json::from_value(fixture["snapshot"].clone()).unwrap();
    for model in ["deepseek-v4-pro", "deepseek-v4-flash"] {
        snapshot
            .exact_aliases
            .insert(model.into(), "text-model".into());
    }
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
    state
        .mutate_shares_immediate(|store| {
            let share = store
                .shares
                .iter_mut()
                .find(|share| share.id == share_id)
                .unwrap();
            let grant = share.user_grants.get_mut("owner@example.com").unwrap();
            grant.policy.token_period =
                crate::domain::sharing::router_contract::ShareTokenPeriod::Lifetime;
            grant.policy.parallel_limit = Some(1);
            grant.policy.value_quota = Some(ValueQuota {
                limit_usd_micros: "10000000".into(),
                tariff_policy_id: "api-equivalent-v1".into(),
            });
        })
        .await
        .unwrap();
    for _ in 0..2 {
        let response = forward_for_test_surface(
            state.clone(),
            ProxyRoute::ClaudeMessages,
            provider_id.clone(),
            None,
            deepseek_forwarder_headers(share_id, "value-deepseek-session"),
            deepseek_forwarder_request(false),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
    }
    let db = rusqlite::Connection::open(server_sqlite::database_path(&state.config_dir)).unwrap();
    let counts: (i64, i64) = db
        .query_row(
            "SELECT count(*),sum(metering_status='pending') FROM value_quota_attempts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        counts,
        (3, 0),
        "two calls plus cached-session replay, all terminal"
    );
    let repeated: i64 = db
        .query_row(
            "SELECT count(*) FROM value_quota_attempts WHERE attempt_id='2' AND usage_picos<>'0'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        repeated, 1,
        "the cached-session replay owns a distinct paid attempt"
    );
    assert_eq!(
        observations
            .completion_requests
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );
    server.abort();
}
