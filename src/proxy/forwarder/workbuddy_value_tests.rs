// Loopback-only monetary fixtures; no production accounts, browser, or money.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkBuddyValueReply {
    Success,
    ZeroCache,
    Mismatch,
    Conflict,
    MissingModel,
    Priority,
    ConflictingPrelude,
    PriorityPrelude,
    HttpErrorUsage,
    HttpErrorPartialUsage,
    HttpAuthOutput,
    DeclaredRateLimit,
    Truncated,
    EmptyTruncated,
    NoUsage,
    CoalescedError,
}

fn workbuddy_value_reply(reply: WorkBuddyValueReply) -> Response {
    if reply == WorkBuddyValueReply::HttpAuthOutput {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"model":"glm-5.2", "message":"license rejected",
                "choices":[{"index":0,"message":{"role":"assistant","content":"already generated"}}]}).to_string()))
            .unwrap();
    }
    if reply == WorkBuddyValueReply::DeclaredRateLimit {
        let body = async_stream::stream! {
            yield Ok::<_, std::io::Error>(Bytes::from("data: {\"model\":\"glm-5.2\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n"));
            tokio::time::sleep(Duration::from_millis(100)).await;
            yield Ok(Bytes::from("data: {\"code\":6004,\"message\":\"rate limited\"}\n\n"));
        };
        return Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(body))
            .unwrap();
    }
    let model = if reply == WorkBuddyValueReply::Mismatch {
        "hy3"
    } else {
        "glm-5.2"
    };
    let mut first = json!({"id":"value-buddy","object":"chat.completion.chunk","model":model,
        "choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]});
    let mut last = json!({"id":"value-buddy","object":"chat.completion.chunk","model":model,
        "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,
            "cache_read_tokens":3,"cache_write_tokens":2}});
    if reply == WorkBuddyValueReply::MissingModel {
        first.as_object_mut().unwrap().remove("model");
        last.as_object_mut().unwrap().remove("model");
    }
    if reply == WorkBuddyValueReply::Conflict {
        last["model"] = json!("other-model");
    }
    if reply == WorkBuddyValueReply::Priority {
        first["service_tier"] = json!("priority");
        last["service_tier"] = json!("priority");
    }
    if reply == WorkBuddyValueReply::ZeroCache {
        last["usage"]["cache_read_tokens"] = json!(0);
        last["usage"]["cache_write_tokens"] = json!(0);
        last["usage"]["prompt_tokens_details"] = json!({"cached_tokens":9,"cache_write_tokens":1});
    }
    if matches!(
        reply,
        WorkBuddyValueReply::HttpErrorUsage | WorkBuddyValueReply::HttpErrorPartialUsage
    ) {
        let usage = if reply == WorkBuddyValueReply::HttpErrorPartialUsage {
            json!({"prompt_tokens":10})
        } else {
            last["usage"].clone()
        };
        return Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"model":model,"message":"limited","usage":usage}).to_string(),
            ))
            .unwrap();
    }
    if reply == WorkBuddyValueReply::NoUsage {
        last.as_object_mut().unwrap().remove("usage");
    }
    if reply == WorkBuddyValueReply::EmptyTruncated {
        first["choices"][0]["delta"]
            .as_object_mut()
            .unwrap()
            .remove("content");
    }
    let mut chunks = Vec::new();
    if reply == WorkBuddyValueReply::ConflictingPrelude {
        chunks.push(Bytes::from("data: {\"model\":\"other-model\"}\n\n"));
    }
    if reply == WorkBuddyValueReply::PriorityPrelude {
        chunks.push(Bytes::from(
            "data: {\"model\":\"glm-5.2\",\"service_tier\":\"priority\"}\n\n",
        ));
    }
    chunks.push(Bytes::from(format!("data: {first}\n\n")));
    if reply == WorkBuddyValueReply::CoalescedError {
        return sse_response(vec![Bytes::from(format!(
            "data: {first}\n\ndata: {}\n\n",
            json!({"code":12005,"message":"license rejected","usage":last["usage"]})
        ))]);
    }
    if !matches!(
        reply,
        WorkBuddyValueReply::Truncated | WorkBuddyValueReply::EmptyTruncated
    ) {
        // Repeated cumulative usage is one observation, never an additive bill.
        let mut usage_frame = json!({"model":model,"usage":last["usage"]});
        if reply == WorkBuddyValueReply::MissingModel {
            usage_frame.as_object_mut().unwrap().remove("model");
        }
        chunks.push(Bytes::from(format!("data: {usage_frame}\n\n")));
        chunks.push(Bytes::from(format!("data: {last}\n\ndata: [DONE]\n\n")));
    }
    sse_response(chunks)
}

async fn workbuddy_value_setup(
    route: ProxyRoute,
    replies: Vec<CodeBuddyGenerationReply>,
    limit: &str,
) -> (
    ServerState,
    String,
    CodeBuddyFixtureState,
    tokio::task::JoinHandle<()>,
    crate::repository::server_sqlite::value_quota::Scope,
) {
    use crate::repository::server_sqlite::{self, value_quota as ledger};
    let (address, fixture, server) = spawn_codebuddy_upstream(CodeBuddySite::Intl, replies).await;
    *fixture.catalog_override.lock().unwrap() = Some(json!({"code":0,"data":{"enterpriseId":"",
        "models":[{"id":"default-model","supportsReasoning":false},{"id":"glm-5.2","supportsToolCall":true,"supportsReasoning":true,
            "reasoning":{"defaultEffort":"high","supportedEfforts":["low","medium","high"]}},
            {"id":"hy3","supportsToolCall":true,"supportsReasoning":true,
            "reasoning":{"defaultEffort":"high","supportedEfforts":["low","medium","high"]}}]}}));
    let state = codebuddy_test_state("workbuddy-value");
    state
        .configure_value_quota_router_for_test("workbuddy-value-installation")
        .await;
    server_sqlite::activate_authority(&state.config_dir).unwrap();
    *fixture.value_admission_dir.lock().unwrap() = Some(state.config_dir.clone());
    let (provider, _) = install_buddy_provider(
        &state,
        ProviderType::WorkBuddyOAuth,
        "value",
        route.app(),
        CodeBuddySite::Intl,
        &format!("http://{address}"),
    )
    .await;
    let mut providers = state.providers_snapshot().await;
    providers.providers[0].provider.settings_config["modelMapping"] = json!({"mode":"passthrough"});
    providers
        .rebuild_runtime_index(&state.accounts_snapshot().await)
        .unwrap();
    providers.promote_to_s2(&state.config_dir).unwrap();
    // Monetary fixtures run with committed SQLite authority: persist the
    // Provider through the normal domain writer before creating its Share.
    state
        .mutate_providers_immediate(move |store| *store = providers)
        .await
        .unwrap();
    let config = state.config_snapshot().await;
    let scope = ledger::Scope {
        router_url: config.router_api_base().unwrap().into(),
        installation_id: "workbuddy-value-installation".into(),
        share_id: "workbuddy-value-share".into(),
        email: "owner@example.com".into(),
        entitlement_id: None,
    };
    let fixture_prices: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/share-value-pricing-v1.json"
    ))
    .unwrap();
    let mut snapshot: crate::domain::usage::value_pricing::PricingSnapshot =
        serde_json::from_value(fixture_prices["snapshot"].clone()).unwrap();
    // Use an actual reviewed WorkBuddy catalog model; unknown names must not
    // be admitted just because a monetary fixture assigned them a price.
    let priced_model = snapshot.models["text-model"].clone();
    snapshot.models.insert("glm-5.2".into(), priced_model);
    // A verified priority tier must not be billed at the cheaper request tier.
    let model = snapshot.models.get_mut("glm-5.2").unwrap();
    let mut priority = model.rates["standard/base"].clone();
    priority.input = Some("6000000".into());
    priority.output = Some("30000000".into());
    model.rates.insert("priority/base".into(), priority);
    let mut priority_long = model.rates["standard/long"].clone();
    priority_long.input = Some("12000000".into());
    priority_long.output = Some("60000000".into());
    model.rates.insert("priority/long".into(), priority_long);
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
    install_codebuddy_share(
        &state,
        "workbuddy-value-share",
        route.app(),
        &provider,
        "owner@example.com",
    )
    .await;
    state
        .mutate_shares_immediate(|shares| {
            shares.shares[0]
                .user_grants
                .get_mut("owner@example.com")
                .unwrap()
                .policy = serde_json::from_value(json!({"tokenPeriod":"lifetime",
            "valueQuota":{"limitUsdMicros":limit,"tariffPolicyId":"api-equivalent-v1"}}))
            .unwrap();
        })
        .await
        .unwrap();
    (state, provider, fixture, server, scope)
}

async fn workbuddy_value_request(
    state: ServerState,
    provider: String,
    route: ProxyRoute,
    streaming: bool,
    model: &str,
) -> Result<Response, ProxyError> {
    let (_, mut path, body) = surface_request(route.app(), streaming);
    let mut body: Value = serde_json::from_slice(&body).unwrap();
    if route == ProxyRoute::CodexChatCompletions {
        body =
            json!({"model":model,"stream":streaming,"messages":[{"role":"user","content":"hi"}]});
    }
    if let Some(path) = path.as_mut() {
        *path = path.replace("/auto:", &format!("/{model}:"));
    } else {
        body["model"] = json!(model);
    }
    super::forward_for_test_surface(
        state,
        route,
        provider,
        path,
        codebuddy_headers(
            "workbuddy-value-share",
            "owner@example.com",
            "value-fixture",
        ),
        Bytes::from(body.to_string()),
    )
    .await
}

fn workbuddy_value_totals(state: &ServerState) -> (String, String, String, usize) {
    let db = rusqlite::Connection::open(crate::repository::server_sqlite::database_path(
        &state.config_dir,
    ))
    .unwrap();
    db.query_row("SELECT usage_picos,pending_risk_picos,metering_status,(SELECT count(*) FROM value_quota_attempts) FROM value_quota_attempts ORDER BY watermark DESC LIMIT 1",[],|r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap()
}

#[test]
fn workbuddy_value_four_routes_meter_before_send_and_use_raw_evidence() {
    run_codebuddy_async_test(async {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexResponses,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::Gemini,
        ] {
            for streaming in [false, true] {
                let (state, provider, fixture, server, _) = workbuddy_value_setup(
                    route,
                    vec![CodeBuddyGenerationReply::Value(
                        WorkBuddyValueReply::Success,
                    )],
                    "50",
                )
                .await;
                let response = workbuddy_value_request(
                    state.clone(),
                    provider.clone(),
                    route,
                    streaming,
                    "glm-5.2",
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                if streaming {
                    assert_eq!(
                        workbuddy_value_totals(&state).2,
                        "pending",
                        "reservation must live until the body finishes"
                    );
                }
                collect(response).await;
                let totals = workbuddy_value_totals(&state);
                assert_eq!(
                    totals,
                    ("72900000".into(), "0".into(), "conservative".into(), 1),
                    "{route:?}/{streaming}"
                ); // unknown cache TTL uses the higher rate
                let error =
                    workbuddy_value_request(state.clone(), provider, route, streaming, "glm-5.2")
                        .await
                        .unwrap_err();
                assert_eq!(error.error_code(), "value_quota_exhausted");
                assert_eq!(fixture.count(&fixture.generation_requests), 1);
                server.abort();
            }
        }
    });
}

#[test]
fn workbuddy_value_auto_can_resolve_a_named_model_and_explicit_zero_cache_is_exact() {
    run_codebuddy_async_test(async {
        for streaming in [false, true] {
            let route = ProxyRoute::CodexChatCompletions;
            let (state, provider, fixture, server, _) = workbuddy_value_setup(
                route,
                vec![CodeBuddyGenerationReply::Value(
                    WorkBuddyValueReply::ZeroCache,
                )],
                "1000000",
            )
            .await;
            // Without an opaque default, auto resolves the concrete live
            // catalog model before pricing/admission, not an assumed alias.
            *fixture.catalog_override.lock().unwrap() = Some(
                json!({"code":0,"data":{"enterpriseId":"",
                "models":[{"id":"glm-5.2","supportsToolCall":true,"supportsReasoning":true,"reasoning":{"effort":"high"}}]}}),
            );
            collect(
                workbuddy_value_request(state.clone(), provider, route, streaming, "auto")
                    .await
                    .unwrap(),
            )
            .await;
            assert_eq!(
                workbuddy_value_totals(&state),
                ("75000000".into(), "0".into(), "observed".into(), 1)
            );
            assert_eq!(
                fixture.generation_observations()[0].body["model"],
                "glm-5.2"
            );
            server.abort();
        }
    });
}

#[test]
fn workbuddy_value_retries_settle_free_attempt_and_never_replay_paid_output() {
    run_codebuddy_async_test(async {
        let route = ProxyRoute::CodexChatCompletions;
        let (state, provider, fixture, server, _) = workbuddy_value_setup(
            route,
            vec![
                CodeBuddyGenerationReply::HttpUnauthorized,
                CodeBuddyGenerationReply::Value(WorkBuddyValueReply::Success),
            ],
            "1000000",
        )
        .await;
        collect(
            workbuddy_value_request(state.clone(), provider, route, false, "glm-5.2")
                .await
                .unwrap(),
        )
        .await;
        let totals = workbuddy_value_totals(&state);
        assert_eq!(totals.0, "72900000");
        assert_eq!(totals.1, "0");
        assert_eq!(totals.3, 2);
        assert_eq!(fixture.count(&fixture.refresh_requests), 1);
        let db = rusqlite::Connection::open(crate::repository::server_sqlite::database_path(
            &state.config_dir,
        ))
        .unwrap();
        let first: (String, String) = db
            .query_row(
                "SELECT usage_picos,metering_status FROM value_quota_attempts WHERE attempt_id='1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(first, ("0".into(), "observed".into()));
        server.abort();
        let (state, provider, fixture, server, _) = workbuddy_value_setup(
            route,
            vec![
                CodeBuddyGenerationReply::LateLicenseRejected,
                CodeBuddyGenerationReply::Value(WorkBuddyValueReply::Success),
            ],
            "1000000",
        )
        .await;
        assert!(
            workbuddy_value_request(state.clone(), provider, route, false, "glm-5.2")
                .await
                .is_err()
        );
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
        assert_ne!(workbuddy_value_totals(&state).0, "0");
        server.abort();
    });
}

#[test]
fn workbuddy_value_missing_prices_opaque_models_and_media_never_generate() {
    run_codebuddy_async_test(async {
        let route = ProxyRoute::CodexChatCompletions;
        let (state, provider, fixture, server, _) = workbuddy_value_setup(
            route,
            vec![CodeBuddyGenerationReply::Value(
                WorkBuddyValueReply::Success,
            )],
            "1000000",
        )
        .await;
        for (model, reason) in [
            ("hy3", "model_unpriced"),
            ("auto", "model_unresolved"),
            ("default-model", "model_unresolved"),
        ] {
            let error =
                workbuddy_value_request(state.clone(), provider.clone(), route, false, model)
                    .await
                    .unwrap_err();
            assert_eq!(error.error_code(), "price_unavailable");
            assert!(error.message.contains(reason), "{}", error.message);
        }
        let error=super::forward_for_test_surface(state.clone(),route,provider,None,codebuddy_headers("workbuddy-value-share","owner@example.com","media"),Bytes::from(json!({"model":"glm-5.2","messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://invalid.example/image.png"}}]}]}).to_string())).await.unwrap_err();
        assert_eq!(error.error_code(), "cc_switch_invalid_request");
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(fixture.count(&fixture.generation_requests), 0);
        server.abort();
    });
}

#[test]
fn workbuddy_value_conflicting_missing_error_and_partial_evidence_settles_conservatively() {
    run_codebuddy_async_test(async {
        for reply in [
            WorkBuddyValueReply::Mismatch,
            WorkBuddyValueReply::Conflict,
            WorkBuddyValueReply::MissingModel,
            WorkBuddyValueReply::Priority,
            WorkBuddyValueReply::ConflictingPrelude,
            WorkBuddyValueReply::PriorityPrelude,
            WorkBuddyValueReply::HttpErrorUsage,
            WorkBuddyValueReply::HttpErrorPartialUsage,
            WorkBuddyValueReply::HttpAuthOutput,
            WorkBuddyValueReply::Truncated,
            WorkBuddyValueReply::EmptyTruncated,
            WorkBuddyValueReply::NoUsage,
            WorkBuddyValueReply::CoalescedError,
        ] {
            for streaming in [false, true] {
                let route = ProxyRoute::CodexChatCompletions;
                let (state, provider, fixture, server, _) = workbuddy_value_setup(
                    route,
                    vec![CodeBuddyGenerationReply::Value(reply)],
                    "1000000",
                )
                .await;
                if let Ok(response) =
                    workbuddy_value_request(state.clone(), provider, route, streaming, "glm-5.2")
                        .await
                {
                    collect(response).await;
                }
                let totals = workbuddy_value_totals(&state);
                assert_eq!(totals.1, "0", "{reply:?}/{streaming}");
                assert_ne!(totals.0, "0", "{reply:?}/{streaming}");
                assert_eq!(totals.2, "conservative", "{reply:?}/{streaming}");
                if matches!(
                    reply,
                    WorkBuddyValueReply::Priority | WorkBuddyValueReply::PriorityPrelude
                ) {
                    assert_eq!(totals.0, "132900000");
                }
                if reply == WorkBuddyValueReply::ConflictingPrelude {
                    let logs = state.usage.read().await;
                    assert_eq!(
                        logs.logs.last().unwrap().reported_model_conflict,
                        Some(true)
                    );
                }
                assert_eq!(fixture.count(&fixture.generation_requests), 1);
                if reply == WorkBuddyValueReply::HttpAuthOutput {
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                }
                server.abort();
            }
        }
    });
}

#[test]
fn workbuddy_value_non_auth_rejections_without_usage_remain_unknown() {
    run_codebuddy_async_test(async {
        for streaming in [false, true] {
            let route = ProxyRoute::CodexChatCompletions;
            let (state, provider, fixture, server, _) = workbuddy_value_setup(
                route,
                vec![CodeBuddyGenerationReply::Value(
                    WorkBuddyValueReply::DeclaredRateLimit,
                )],
                "1000000",
            )
            .await;
            let result =
                workbuddy_value_request(state.clone(), provider, route, streaming, "glm-5.2").await;
            if streaming {
                assert!(
                    result.is_ok(),
                    "role prelude must commit before the declared error"
                );
            }
            if let Ok(response) = result {
                collect(response).await;
            }
            let (used, pending, metering, attempts) = workbuddy_value_totals(&state);
            assert_ne!(
                used, "0",
                "a rate limit without usage does not prove no execution"
            );
            assert_eq!(
                (pending, metering, attempts),
                ("0".into(), "conservative".into(), 1)
            );
            assert_eq!(fixture.count(&fixture.generation_requests), 1);
            assert_eq!(fixture.count(&fixture.refresh_requests), 0);
            server.abort();
        }
    });
}

#[test]
fn workbuddy_value_readiness_names_opaque_and_priced_fixed_models() {
    run_codebuddy_async_test(async {
        let (state, _, _, server, _) =
            workbuddy_value_setup(ProxyRoute::CodexChatCompletions, vec![], "1000000").await;
        for (model, ready, reason) in [
            ("auto", false, Some("model_unresolved")),
            ("workbuddy/GLM-5.2", true, None),
            ("hy3", false, Some("model_unpriced")),
        ] {
            let mut providers = state.providers_snapshot().await;
            providers.providers[0].provider.settings_config["modelMapping"] =
                json!({"mode":"single","upstreamModel":model});
            providers
                .rebuild_runtime_index(&state.accounts_snapshot().await)
                .unwrap();
            state.replace_provider_store_for_test(providers).await;
            let revision = state
                .shares
                .read()
                .await
                .get("workbuddy-value-share")
                .unwrap()
                .config_revision;
            let preview = state
                .preview_share_value_quota("workbuddy-value-share", revision, BTreeMap::new())
                .await
                .unwrap();
            let checks = preview["checks"].as_array().unwrap();
            assert!(checks
                .iter()
                .any(|c| c["code"] == "text_surface" && c["ready"] == true));
            let coverage = checks
                .iter()
                .find(|c| c["code"] == "model_price_coverage")
                .unwrap();
            assert_eq!(coverage["ready"], ready, "{model}");
            assert_eq!(coverage["app"], "codex");
            assert_eq!(coverage["profileId"], "codex.workbuddy_oauth");
            assert_eq!(coverage["driverId"], "special.workbuddy_oauth");
            if let Some(reason) = reason {
                assert_eq!(coverage["reasonCode"], reason);
            }
        }
        server.abort();
    });
}

#[test]
fn workbuddy_value_stream_cancellation_releases_its_durable_reservation() {
    run_codebuddy_async_test(async {
        for poll_body in [false, true] {
            let route = ProxyRoute::CodexChatCompletions;
            let (state, provider, _, server, _) = workbuddy_value_setup(
                route,
                vec![CodeBuddyGenerationReply::Value(
                    WorkBuddyValueReply::Success,
                )],
                "1000000",
            )
            .await;
            let response = workbuddy_value_request(state.clone(), provider, route, true, "glm-5.2")
                .await
                .unwrap();
            assert_eq!(workbuddy_value_totals(&state).2, "pending");
            let mut body = response.into_body().into_data_stream();
            if poll_body {
                assert!(body.next().await.unwrap().is_ok());
            }
            drop(body); // Both unread and partially-read bodies must settle.
            for _ in 0..100 {
                if workbuddy_value_totals(&state).1 == "0" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let totals = workbuddy_value_totals(&state);
            assert_eq!(totals.1, "0");
            assert_ne!(totals.0, "0");
            assert_eq!(totals.2, "conservative");
            server.abort();
        }
    });
}

#[test]
fn workbuddy_value_market_trial_reads_the_same_ledger_and_stops_at_its_budget() {
    run_codebuddy_async_test(async {
        let route = ProxyRoute::CodexChatCompletions;
        let (state, provider, fixture, server, _) = workbuddy_value_setup(
            route,
            vec![CodeBuddyGenerationReply::Value(
                WorkBuddyValueReply::Success,
            )],
            "1000000",
        )
        .await;
        state.enable_value_market_trials_for_test().await;
        state.mutate_shares_immediate(|shares| {
            let share=&mut shares.shares[0];
            let grant=share.user_grants.get_mut("owner@example.com").unwrap();
            grant.manager=crate::domain::sharing::router_contract::ShareGrantManager::RouterShareMarket;
            grant.entitlement_id=Some("workbuddy-trial".into()); grant.role="shareto".into();
            share.market_entitlements.insert("workbuddy-trial".into(),serde_json::from_value(json!({
                "trialScopeId":"buddy-trial-scope","email":"owner@example.com","phase":"trial","active":true,
                "trialTokensUsed":0,"usageRevision":0,"appliedAtMs":crate::infra::time::now_ms() as i64,
                "trialValueAllowanceUsdMicros":"50","trialValueLimitUsdMicros":"50","trialValueUsedPicos":"0"
            })).unwrap());
        }).await.unwrap();
        collect(
            workbuddy_value_request(state.clone(), provider.clone(), route, false, "glm-5.2")
                .await
                .unwrap(),
        )
        .await;
        let shares = state.shares.read().await;
        let share = shares.get("workbuddy-value-share").unwrap();
        assert_ne!(
            share.market_entitlements["workbuddy-trial"].trial_value_used_picos,
            "0"
        );
        assert!(crate::domain::sharing::market_trials::exhausted(
            share,
            "workbuddy-trial"
        ));
        drop(shares);
        assert!(
            workbuddy_value_request(state.clone(), provider, route, false, "glm-5.2")
                .await
                .is_err()
        );
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        server.abort();
    });
}
