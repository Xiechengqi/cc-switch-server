const BUDDY_IMAGE_PNG_DATA: &str = "iVBORw0KGgo=";
const BUDDY_IMAGE_HTTPS: &str = "https://images.example.test/invoice.jpg?signature=private-image";

fn buddy_image_surface_request(route: ProxyRoute, stream: bool) -> (Option<String>, Bytes) {
    let (_, path, mut body) = surface_request(route.app(), stream);
    if route == ProxyRoute::CodexChatCompletions {
        body = Bytes::from(json!({"model":"gpt-5.6-sol","messages":[],"stream":stream,
            "tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object"}}}]}).to_string());
    }
    let mut body: Value = serde_json::from_slice(&body).unwrap();
    let uri = format!("data:image/png;base64,{BUDDY_IMAGE_PNG_DATA}");
    let (key, content) = match route {
        ProxyRoute::ClaudeMessages => (
            "messages",
            json!([{"role":"user","content":[
            {"type":"text","text":"识别这张发票"},
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":BUDDY_IMAGE_PNG_DATA}},
            {"type":"image","source":{"type":"url","url":BUDDY_IMAGE_HTTPS}}]}]),
        ),
        ProxyRoute::CodexResponses => (
            "input",
            json!([{"role":"user","content":[
            {"type":"input_text","text":"识别这张发票"},
            {"type":"input_image","image_url":uri,"detail":"high"},
            {"type":"input_image","image_url":BUDDY_IMAGE_HTTPS,"detail":"low"}]}]),
        ),
        ProxyRoute::CodexChatCompletions => (
            "messages",
            json!([{"role":"user","content":[
            {"type":"text","text":"识别这张发票"},
            {"type":"image_url","image_url":{"url":uri,"detail":"high"}},
            {"type":"image_url","image_url":{"url":BUDDY_IMAGE_HTTPS,"detail":"low"}}]}]),
        ),
        ProxyRoute::Gemini => (
            "contents",
            json!([{"role":"user","parts":[
            {"text":"识别这张发票"},
            {"inlineData":{"mimeType":"image/png","data":BUDDY_IMAGE_PNG_DATA}},
            {"fileData":{"mimeType":"image/jpeg","fileUri":BUDDY_IMAGE_HTTPS}}]}]),
        ),
        _ => unreachable!(),
    };
    body[key] = content;
    body["model"] = json!("gpt-5.6-sol");
    (path, Bytes::from(body.to_string()))
}

fn buddy_image_catalog(site: CodeBuddySite) -> Value {
    json!({"code":0,"data":{"enterpriseId":"","models":[
        {"id":site_model(site),"supportsToolCall":true,"supportsReasoning":true,"supportsImages":false},
        {"id":"hy3","supportsImages":true,"supportsToolCall":false},
        {"id":"glm-5.2","supportsImages":true,"supportsToolCall":true,"supportsReasoning":true,
            "reasoning":{"defaultEffort":"medium","supportedEfforts":["low","medium","high"]}}],
        "agents":[{"name":"cli","models":[site_model(site),"hy3","glm-5.2"]}]}})
}

#[test]
fn buddy_images_reach_four_surfaces_both_sites_and_products_without_changing_data() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                for route in [
                    ProxyRoute::ClaudeMessages,
                    ProxyRoute::CodexResponses,
                    ProxyRoute::CodexChatCompletions,
                    ProxyRoute::Gemini,
                ] {
                    for streaming in [false, true] {
                        let (address, fixture, server) =
                            spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success])
                                .await;
                        *fixture.catalog_override.lock().unwrap() = Some(buddy_image_catalog(site));
                        let state = codebuddy_test_state("buddy-images-surfaces");
                        let (id, account_id) = install_buddy_provider(
                            &state,
                            product,
                            "images",
                            route.app(),
                            site,
                            &format!("http://{address}"),
                        )
                        .await;
                        install_codebuddy_share(
                            &state,
                            "images-share",
                            route.app(),
                            &id,
                            "owner@example.com",
                        )
                        .await;
                        let (path, body) = buddy_image_surface_request(route, streaming);
                        // Cross the former 16-image ceiling on every surface,
                        // product, site and streaming mode without losing data.
                        let mut body: Value = serde_json::from_slice(&body).unwrap();
                        let parts = match route {
                            ProxyRoute::CodexResponses => &mut body["input"][0]["content"],
                            ProxyRoute::Gemini => &mut body["contents"][0]["parts"],
                            _ => &mut body["messages"][0]["content"],
                        }
                        .as_array_mut()
                        .unwrap();
                        let image = parts[1].clone();
                        parts.extend(std::iter::repeat_n(image, 15));
                        let body = Bytes::from(body.to_string());
                        let response = super::forward_for_test_surface(
                            state.clone(),
                            route,
                            id.clone(),
                            path.clone(),
                            codebuddy_headers("images-share", "owner@example.com", "images"),
                            body.clone(),
                        )
                        .await
                        .unwrap();
                        assert_eq!(
                            response.status(),
                            StatusCode::OK,
                            "{product:?}/{site:?}/{route:?}/{streaming}"
                        );
                        assert_eq!(
                            response
                                .extensions()
                                .get::<crate::proxy::codebuddy_runtime::BuddyResolvedModel>()
                                .unwrap()
                                .0,
                            "glm-5.2"
                        );
                        assert!(String::from_utf8_lossy(&collect(response).await).contains("ready"));
                        let observation = &fixture.generation_observations()[0];
                        assert_eq!(observation.body["model"], "glm-5.2");
                        let content = &observation.body["messages"][1]["content"];
                        assert_eq!(content.as_array().unwrap().len(), 18);
                        for part in &content.as_array().unwrap()[3..] {
                            assert_eq!(part, &content[1]);
                        }
                        assert_eq!(content[0]["text"], "识别这张发票");
                        assert_eq!(
                            content[1]["image_url"]["url"],
                            format!("data:image/png;base64,{BUDDY_IMAGE_PNG_DATA}")
                        );
                        assert_eq!(content[2]["image_url"]["url"], BUDDY_IMAGE_HTTPS);
                        if route.app() == AppKind::Codex {
                            assert_eq!(content[1]["image_url"]["detail"], "high");
                            assert_eq!(content[2]["image_url"]["detail"], "low");
                        }
                        // A fresh revocation must replace the cached vision
                        // authorization, with no auth recovery or inference.
                        let providers = state.providers_snapshot().await;
                        let plan = providers.runtime_plan(route.app(), &id).unwrap();
                        let runtime = state
                            .prepare_codebuddy_runtime(
                                route.app(),
                                &id,
                                plan.provider_revision,
                                &plan.runtime_fingerprint,
                                &account_id,
                                1,
                                std::time::Duration::from_secs(2),
                            )
                            .await
                            .unwrap();
                        let mut revoked = buddy_image_catalog(site);
                        revoked["data"]["models"][2]["supportsImages"] = json!(false);
                        *fixture.catalog_override.lock().unwrap() = Some(revoked);
                        state
                            .invalidate_codebuddy_runtime_scope(&runtime.scope)
                            .await;
                        let error = super::forward_for_test_surface(
                            state.clone(),
                            route,
                            id,
                            path,
                            codebuddy_headers("images-share", "owner@example.com", "images"),
                            body,
                        )
                        .await
                        .unwrap_err();
                        assert_eq!(error.status, StatusCode::BAD_REQUEST);
                        assert!(error.message.contains("compatible with this request"));
                        assert_eq!(fixture.count(&fixture.generation_requests), 1);
                        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                        assert!(!state.share_in_flight.has_in_flight("images-share"));
                        assert_eq!(
                            state
                                .account_in_flight
                                .snapshot()
                                .current(product, &account_id),
                            0
                        );
                        let log = state.usage_snapshot().await.logs.last().unwrap().clone();
                        assert!(!serde_json::to_string(&log)
                            .unwrap()
                            .contains("private-image"));
                        assert!(!serde_json::to_string(&log)
                            .unwrap()
                            .contains(BUDDY_IMAGE_PNG_DATA));
                        server.abort();
                    }
                }
            }
        }
    });
}

#[test]
fn buddy_fixed_text_models_reject_images_without_silent_substitution_or_refresh() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for site in [CodeBuddySite::Cn, CodeBuddySite::Intl] {
                for route in [
                    ProxyRoute::ClaudeMessages,
                    ProxyRoute::CodexResponses,
                    ProxyRoute::CodexChatCompletions,
                    ProxyRoute::Gemini,
                ] {
                    let (address, fixture, server) = spawn_codebuddy_upstream(site, vec![]).await;
                    *fixture.catalog_override.lock().unwrap() = Some(buddy_image_catalog(site));
                    let state = codebuddy_test_state("buddy-fixed-image-rejection");
                    let (id, account_id) = install_buddy_provider(
                        &state,
                        product,
                        "fixed-image",
                        route.app(),
                        site,
                        &format!("http://{address}"),
                    )
                    .await;
                    let mut providers = state.providers_snapshot().await;
                    providers.providers[0].provider.settings_config["modelMapping"] =
                        json!({"mode":"single","upstreamModel":site_model(site)});
                    providers
                        .rebuild_runtime_index(&state.accounts_snapshot().await)
                        .unwrap();
                    state.replace_provider_store_for_test(providers).await;
                    install_codebuddy_share(
                        &state,
                        "fixed-image-share",
                        route.app(),
                        &id,
                        "owner@example.com",
                    )
                    .await;
                    let (path, mut body) = buddy_image_surface_request(route, false);
                    // Avoid an unrelated reasoning rejection on the fixed model.
                    let mut input: Value = serde_json::from_slice(&body).unwrap();
                    input.as_object_mut().unwrap().remove("reasoning");
                    body = Bytes::from(input.to_string());
                    let error = super::forward_for_test_surface(
                        state.clone(),
                        route,
                        id,
                        path,
                        codebuddy_headers("fixed-image-share", "owner@example.com", "images"),
                        body,
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(error.status, StatusCode::BAD_REQUEST, "{error:?}");
                    assert!(
                        error
                            .message
                            .contains("does not advertise image input support"),
                        "{error:?}"
                    );
                    assert!(error
                        .message
                        .starts_with(crate::domain::codebuddy::buddy_product_label(product)));
                    assert_eq!(fixture.count(&fixture.generation_requests), 0);
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                    assert_eq!(
                        state
                            .account_in_flight
                            .snapshot()
                            .current(product, &account_id),
                        0
                    );
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn buddy_tool_result_images_survive_native_round_trips() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                let site = CodeBuddySite::Cn;
                let (address, fixture, server) =
                    spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success]).await;
                *fixture.catalog_override.lock().unwrap() = Some(buddy_image_catalog(site));
                let state = codebuddy_test_state("buddy-tool-images");
                let (id, _) = install_buddy_provider(
                    &state,
                    product,
                    "tool-images",
                    route.app(),
                    site,
                    &format!("http://{address}"),
                )
                .await;
                install_codebuddy_share(
                    &state,
                    "tool-images-share",
                    route.app(),
                    &id,
                    "owner@example.com",
                )
                .await;
                let (path, body) = buddy_image_surface_request(route, false);
                let mut body: Value = serde_json::from_slice(&body).unwrap();
                let uri = format!("data:image/png;base64,{BUDDY_IMAGE_PNG_DATA}");
                let (key, history) = match route {
                    ProxyRoute::ClaudeMessages => (
                        "messages",
                        json!([
                        {"role":"assistant","content":[{"type":"tool_use","id":"image-call","name":"lookup","input":{}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":"image-call","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":BUDDY_IMAGE_PNG_DATA}}]}]}]),
                    ),
                    ProxyRoute::CodexResponses => (
                        "input",
                        json!([
                        {"type":"function_call","call_id":"image-call","name":"lookup","arguments":"{}"},
                        {"type":"function_call_output","call_id":"image-call","output":[{"type":"input_image","image_url":uri}]}]),
                    ),
                    ProxyRoute::CodexChatCompletions => (
                        "messages",
                        json!([
                        {"role":"assistant","tool_calls":[{"type":"function","id":"image-call","function":{"name":"lookup","arguments":"{}"}}]},
                        {"role":"tool","tool_call_id":"image-call","content":[{"type":"image_url","image_url":{"url":uri}}]}]),
                    ),
                    ProxyRoute::Gemini => (
                        "contents",
                        json!([
                        {"role":"model","parts":[{"functionCall":{"id":"image-call","name":"lookup","args":{}}}]},
                        {"role":"user","parts":[{"functionResponse":{"id":"image-call","name":"lookup","response":{"result":"ok"},"parts":[{"inlineData":{"mimeType":"image/png","data":BUDDY_IMAGE_PNG_DATA}}]}}]}]),
                    ),
                    _ => unreachable!(),
                };
                body[key]
                    .as_array_mut()
                    .unwrap()
                    .extend(history.as_array().unwrap().iter().cloned());
                let response = super::forward_for_test_surface(
                    state,
                    route,
                    id,
                    path,
                    codebuddy_headers("tool-images-share", "owner@example.com", "images"),
                    Bytes::from(body.to_string()),
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{product:?}/{route:?}");
                collect(response).await;
                let messages = fixture.generation_observations()[0].body["messages"]
                    .as_array()
                    .unwrap()
                    .clone();
                let images = messages
                    .iter()
                    .flat_map(|message| message["content"].as_array().into_iter().flatten())
                    .filter_map(|part| part.pointer("/image_url/url").and_then(Value::as_str))
                    .collect::<Vec<_>>();
                assert_eq!(
                    images.iter().filter(|image| **image == uri).count(),
                    2,
                    "{product:?}/{route:?}: {messages:?}"
                );
                assert_eq!(
                    images
                        .iter()
                        .filter(|image| **image == BUDDY_IMAGE_HTTPS)
                        .count(),
                    1
                );
                assert!(messages
                    .iter()
                    .any(|message| message["tool_call_id"] == "image-call"));
                assert!(messages.iter().any(|message| message
                    .pointer("/tool_calls/0/id")
                    .and_then(Value::as_str)
                    == Some("image-call")));
                server.abort();
            }
        }
    });
}

#[test]
fn buddy_malformed_images_fail_before_inference_and_never_lose_image_semantics() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                let site = CodeBuddySite::Cn;
                let (address, fixture, server) = spawn_codebuddy_upstream(site, vec![]).await;
                *fixture.catalog_override.lock().unwrap() = Some(buddy_image_catalog(site));
                let state = codebuddy_test_state("buddy-malformed-images");
                let (id, account_id) = install_buddy_provider(
                    &state,
                    product,
                    "bad-images",
                    route.app(),
                    site,
                    &format!("http://{address}"),
                )
                .await;
                install_codebuddy_share(
                    &state,
                    "bad-images-share",
                    route.app(),
                    &id,
                    "owner@example.com",
                )
                .await;
                let (path, body) = buddy_image_surface_request(route, false);
                let mut body: Value = serde_json::from_slice(&body).unwrap();
                match route {
                    ProxyRoute::ClaudeMessages => {
                        body["messages"][0]["content"][1]["source"]
                            .as_object_mut()
                            .unwrap()
                            .remove("data");
                    }
                    ProxyRoute::CodexResponses => {
                        body["input"][0]["content"][1]
                            .as_object_mut()
                            .unwrap()
                            .remove("image_url");
                    }
                    ProxyRoute::CodexChatCompletions => {
                        body["messages"][0]["content"][1]["image_url"]["url"] =
                            json!("data:image/png;base64,private-invalid-base64");
                    }
                    ProxyRoute::Gemini => {
                        body["contents"][0]["parts"][1]["text"] = json!("would hide the image");
                    }
                    _ => unreachable!(),
                }
                let error = super::forward_for_test_surface(
                    state.clone(),
                    route,
                    id,
                    path,
                    codebuddy_headers("bad-images-share", "owner@example.com", "images"),
                    Bytes::from(body.to_string()),
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_REQUEST, "{error:?}");
                assert!(
                    error
                        .message
                        .starts_with(crate::domain::codebuddy::buddy_product_label(product)),
                    "{error:?}"
                );
                assert!(!error.message.contains("private"));
                assert_eq!(fixture.count(&fixture.generation_requests), 0);
                assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                assert_eq!(
                    state
                        .account_in_flight
                        .snapshot()
                        .current(product, &account_id),
                    0
                );
                server.abort();
            }
        }
    });
}

#[test]
fn buddy_image_inputs_obey_request_memory_before_catalog_or_inference() {
    run_codebuddy_async_test(async {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                let site = CodeBuddySite::Cn;
                let (address, fixture, server) = spawn_codebuddy_upstream(site, vec![]).await;
                let (path, body) = buddy_image_surface_request(route, false);
                let mut state = codebuddy_test_state("buddy-image-memory");
                Arc::get_mut(&mut state)
                    .unwrap()
                    .request_body_limits
                    .memory_budget_bytes = body.len() + 1;
                let (id, _) = install_buddy_provider(
                    &state,
                    product,
                    "memory-images",
                    route.app(),
                    site,
                    &format!("http://{address}"),
                )
                .await;
                install_codebuddy_share(
                    &state,
                    "memory-images-share",
                    route.app(),
                    &id,
                    "owner@example.com",
                )
                .await;
                let error = super::forward_for_test_surface(
                    state,
                    route,
                    id,
                    path,
                    codebuddy_headers("memory-images-share", "owner@example.com", "images"),
                    body,
                )
                .await
                .unwrap_err();
                assert!(
                    error.is_request_memory_exhausted(),
                    "{product:?}/{route:?}: {error:?}"
                );
                assert_eq!(fixture.count(&fixture.config_requests), 0);
                assert_eq!(fixture.count(&fixture.generation_requests), 0);
                assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                server.abort();
            }
        }
    });
}
