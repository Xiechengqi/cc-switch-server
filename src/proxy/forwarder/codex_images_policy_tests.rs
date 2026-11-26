const CODEX_IMAGES_POLICY_PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

#[derive(Clone)]
struct CodexImagesPolicyCapture {
    path: String,
    headers: HeaderMap,
    body: Value,
}

struct CodexImagesPolicyUpstream {
    endpoint: String,
    captures: std::sync::Arc<std::sync::Mutex<Vec<CodexImagesPolicyCapture>>>,
    task: tokio::task::JoinHandle<()>,
}

async fn codex_images_policy_upstream(
    status: StatusCode,
    payload: String,
) -> CodexImagesPolicyUpstream {
    codex_images_policy_upstream_rejecting_token(status, payload, None).await
}

async fn codex_images_policy_upstream_rejecting_token(
    status: StatusCode,
    payload: String,
    rejected_token: Option<String>,
) -> CodexImagesPolicyUpstream {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let captures = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let shared = captures.clone();
    let app = axum::Router::new().fallback(axum::routing::post(
        move |uri: axum::http::Uri, headers: HeaderMap, body: Bytes| {
            let shared = shared.clone();
            let payload = payload.clone();
            let rejected_token = rejected_token.clone();
            async move {
                let rejected = rejected_token.as_deref().is_some_and(|token| {
                    headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        == Some(token)
                });
                shared.lock().unwrap().push(CodexImagesPolicyCapture {
                    path: uri.path().to_string(),
                    headers,
                    body: serde_json::from_slice(&body).unwrap(),
                });
                if rejected {
                    return Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body(Body::from("unauthorized"))
                        .unwrap();
                }
                Response::builder()
                    .status(status)
                    .header(
                        CONTENT_TYPE,
                        if payload.starts_with('{') {
                            "application/json"
                        } else {
                            "text/event-stream"
                        },
                    )
                    .body(Body::from(payload))
                    .unwrap()
            }
        },
    ));
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    CodexImagesPolicyUpstream {
        endpoint,
        captures,
        task,
    }
}

fn codex_images_policy_response() -> String {
    format!(
        "data: {}\n\n",
        json!({"type":"response.completed","response":{
        "status":"completed","output":[{"type":"image_generation_call","id":"image-1",
        "result":CODEX_IMAGES_POLICY_PNG,"output_format":"png"}],
        "usage":{"input_tokens":3,"output_tokens":5,"total_tokens":8}}})
    )
}

#[tokio::test]
async fn codex_images_policy_bridge_default_override_and_single_match_wire_and_usage() {
    use crate::domain::providers::runtime::RuntimeModelPolicy;
    for (index, configured, single, expected) in [
        (0, None, None, "gpt-6.2-sol"),
        (1, Some("gpt-6.2-astra"), None, "gpt-6.2-astra"),
        (2, Some("gpt-6.2-astra"), Some("gpt-5.5"), "gpt-5.5"),
    ] {
        let upstream =
            codex_images_policy_upstream(StatusCode::OK, codex_images_policy_response()).await;
        let name = format!("image-policy-bridge-{index}");
        let (state, mut execution) =
            codex_bridge_test_context(&name, upstream.endpoint.clone()).await;
        let plan = std::sync::Arc::make_mut(&mut execution.plan);
        if let Some(model) = configured {
            plan.driver_options
                .insert("codexImageResponsesModel".into(), json!(model));
        }
        if let Some(model) = single {
            plan.model_policy = RuntimeModelPolicy::Single {
                upstream_model: model.into(),
            };
        }
        let prepared =
            codex_images_generation_request(br#"{"model":"gpt-image-2","prompt":"a blue circle"}"#)
                .unwrap();
        let response = forward_codex_images_request(
            state.clone(),
            execution,
            HeaderMap::new(),
            prepared,
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["data"][0]["b64_json"],
            CODEX_IMAGES_POLICY_PNG
        );
        let requests = upstream.captures.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].path, "/v1/responses");
        assert_eq!(requests[0].body["model"], expected);
        assert_eq!(requests[0].body["tools"][0]["model"], "gpt-image-2");
        assert!(requests[0].body.pointer("/reasoning/effort").is_none());
        assert!(requests[0]
            .headers
            .get(CODEX_RESPONSES_LITE_HEADER)
            .is_none());
        let usage = state.usage.read().await;
        let log = usage.logs.last().unwrap();
        assert_eq!(log.actual_model.as_deref(), Some(expected));
        assert_eq!(log.requested_model.as_deref(), Some("gpt-image-2"));
        assert_eq!(log.stream_status.as_deref(), Some("completed"));
        assert_eq!(log.total_tokens, Some(8));
        upstream.task.abort();
    }
}

#[tokio::test]
async fn codex_images_policy_direct_generation_and_edit_preserve_modes_inputs_and_usage() {
    use crate::domain::providers::runtime::RuntimeModelPolicy;
    for edit in [false, true] {
        for json_upstream in [false, true] {
            for stream in [false, true] {
                let prefix = if edit {
                    "image_edit"
                } else {
                    "image_generation"
                };
                let image = json!({"b64_json":CODEX_IMAGES_POLICY_PNG,"output_format":"png"});
                let usage = json!({"input_tokens":3,"output_tokens":5,"total_tokens":8});
                let payload = if json_upstream {
                    json!({"created":1700000000,"data":[image],"usage":usage}).to_string()
                } else {
                    format!(
                        "event: {prefix}.completed\ndata: {}\n\n",
                        json!({"type":format!("{prefix}.completed"),
                        "created_at":1700000000,"b64_json":CODEX_IMAGES_POLICY_PNG,"output_format":"png","usage":usage})
                    )
                };
                let upstream = codex_images_policy_upstream(StatusCode::OK, payload).await;
                let name = format!("image-policy-direct-{edit}-{json_upstream}-{stream}");
                let (state, mut execution) =
                    codex_bridge_test_context(&name, upstream.endpoint.clone()).await;
                let plan = std::sync::Arc::make_mut(&mut execution.plan);
                plan.driver_options
                    .insert("codexImageTransport".into(), json!("images"));
                plan.model_policy = RuntimeModelPolicy::Single {
                    upstream_model: "gpt-6.2-sol".into(),
                };
                let input = json!({"model":"gpt-image-2","prompt":"a blue circle","quality":"high","stream":stream,
                    "image":format!("data:image/png;base64,{CODEX_IMAGES_POLICY_PNG}"),
                    "mask":format!("data:image/png;base64,{CODEX_IMAGES_POLICY_PNG}")});
                let prepared = if edit {
                    codex_images_edit_request_from_value(input).unwrap()
                } else {
                    codex_images_generation_request(input.to_string().as_bytes()).unwrap()
                };
                let response = forward_codex_images_request(
                    state.clone(),
                    execution,
                    HeaderMap::new(),
                    prepared,
                    UsageLogContext::default(),
                    None,
                    None,
                )
                .await
                .unwrap();
                let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                if stream {
                    assert!(String::from_utf8_lossy(&body)
                        .contains(&format!("event: {prefix}.completed")));
                } else {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&body).unwrap()["data"][0]["b64_json"],
                        CODEX_IMAGES_POLICY_PNG
                    );
                }
                let requests = upstream.captures.lock().unwrap().clone();
                assert_eq!(
                    requests.len(),
                    1,
                    "direct generation must never replay through Responses"
                );
                assert_eq!(
                    requests[0].path,
                    if edit {
                        "/v1/images/edits"
                    } else {
                        "/v1/images/generations"
                    }
                );
                assert_eq!(requests[0].body["model"], "gpt-image-2");
                assert_eq!(requests[0].body["stream"], true);
                assert_eq!(requests[0].body["response_format"], "b64_json");
                assert_eq!(requests[0].body["quality"], "high");
                assert!(requests[0].body.get("reasoning").is_none());
                assert!(requests[0].body.get("tools").is_none());
                assert_eq!(
                    requests[0].headers["authorization"],
                    format!("Bearer {name}-access-token")
                );
                assert_eq!(
                    requests[0].headers["chatgpt-account-id"],
                    format!("{name}-workspace")
                );
                if edit {
                    assert_eq!(
                        requests[0].body["images"][0]["image_url"],
                        format!("data:image/png;base64,{CODEX_IMAGES_POLICY_PNG}")
                    );
                    assert_eq!(
                        requests[0].body["mask"]["image_url"],
                        format!("data:image/png;base64,{CODEX_IMAGES_POLICY_PNG}")
                    );
                }
                let usage = state.usage.read().await;
                let log = usage.logs.last().unwrap();
                assert_eq!(log.actual_model.as_deref(), Some("gpt-image-2"));
                assert_eq!(
                    log.actual_model_source.as_deref(),
                    Some("codex_images_direct")
                );
                assert_eq!(log.image_count, Some(1));
                assert_eq!(log.total_tokens, Some(8));
                upstream.task.abort();
            }
        }
    }
}

#[tokio::test]
async fn codex_images_policy_fresh_missing_catalog_rejects_before_generation() {
    let upstream =
        codex_images_policy_upstream(StatusCode::OK, codex_images_policy_response()).await;
    let (state, execution) =
        codex_bridge_test_context("image-policy-catalog", upstream.endpoint.clone()).await;
    let stored = execution.runtime_stored_view();
    super::super::codex_models::update_manifest_models(&stored, br#"{"models":[]}"#);
    let prepared = codex_images_generation_request(br#"{"prompt":"blue"}"#).unwrap();
    let error = forward_codex_images_request(
        state,
        execution,
        HeaderMap::new(),
        prepared,
        UsageLogContext::default(),
        None,
        None,
    )
    .await
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error.message.contains("current model catalog"));
    assert!(upstream.captures.lock().unwrap().is_empty());
    upstream.task.abort();
}

#[test]
fn codex_images_policy_rejections_distinguish_text_image_and_unrelated_errors() {
    let mut prepared = codex_images_generation_request(br#"{"prompt":"blue"}"#).unwrap();
    for structured in [false, true] {
        let message =
            "The 'gpt-6.2-sol' model is not supported when using Codex with a ChatGPT account.";
        let body = if structured {
            json!({"error":{"code":"model_not_found","subreason":"model_not_in_accessible_catalog","message":message}})
        } else {
            json!({"detail":message})
        };
        assert!(codex_images_text_driver_rejected(
            body.to_string().as_bytes(),
            &prepared
        ));
    }
    for message in [
        "The 'gpt-image-2' model is not supported",
        "The 'gpt-5.4-mini' model is not supported",
        "Invalid size",
    ] {
        assert!(!codex_images_text_driver_rejected(
            json!({"detail":message}).to_string().as_bytes(),
            &prepared
        ));
    }
    prepared.transport = CodexImageTransport::Images;
    assert!(!codex_images_text_driver_rejected(
        br#"{"detail":"The 'gpt-6.2-sol' model is not supported"}"#,
        &prepared
    ));
}

#[tokio::test]
async fn codex_images_policy_direct_401_refreshes_once_without_changing_path_or_account() {
    let name = "image-policy-direct-refresh";
    let token = signed_openai_access_token(&format!("{name}-workspace")).await;
    let refresh = spawn_test_codex_refresh_endpoint(token.clone()).await;
    let payload = format!(
        "data: {}\n\n",
        json!({"type":"image_generation.completed",
        "b64_json":CODEX_IMAGES_POLICY_PNG,"output_format":"png"})
    );
    let upstream = codex_images_policy_upstream_rejecting_token(
        StatusCode::OK,
        payload,
        Some(format!("Bearer {name}-access-token")),
    )
    .await;
    let (state, mut execution) = codex_bridge_test_context(name, upstream.endpoint.clone()).await;
    configure_codex_refresh_test_account(&state, name, refresh.url.clone()).await;
    std::sync::Arc::make_mut(&mut execution.plan)
        .driver_options
        .insert("codexImageTransport".into(), json!("images"));
    let response = forward_codex_images_request(
        state,
        execution,
        HeaderMap::new(),
        codex_images_generation_request(br#"{"prompt":"blue"}"#).unwrap(),
        UsageLogContext::default(),
        None,
        None,
    )
    .await
    .unwrap();
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["data"][0]["b64_json"],
        CODEX_IMAGES_POLICY_PNG
    );
    assert_eq!(
        refresh.requests.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let captures = upstream.captures.lock().unwrap();
    assert_eq!(captures.len(), 2);
    assert!(captures
        .iter()
        .all(|request| request.path == "/v1/images/generations"
            && request.body["model"] == "gpt-image-2"));
    assert_eq!(
        captures[1].headers["authorization"],
        format!("Bearer {token}")
    );
    upstream.task.abort();
    refresh.server.abort();
}

#[tokio::test]
async fn codex_images_policy_http_rejection_preserves_body_and_never_replays() {
    for direct in [false, true] {
        let payload = json!({"detail":"The 'gpt-6.2-sol' model is not supported when using Codex with a ChatGPT account.",
            "error":{"code":"model_not_found","subreason":"model_not_in_accessible_catalog",
            "message":"The 'gpt-6.2-sol' model is not supported when using Codex with a ChatGPT account."}}).to_string();
        let upstream = codex_images_policy_upstream(StatusCode::BAD_REQUEST, payload.clone()).await;
        let (state, mut execution) = codex_bridge_test_context(
            &format!("image-policy-no-replay-{direct}"),
            upstream.endpoint.clone(),
        )
        .await;
        if direct {
            std::sync::Arc::make_mut(&mut execution.plan)
                .driver_options
                .insert("codexImageTransport".into(), json!("images"));
        }
        let response = forward_codex_images_request(
            state.clone(),
            execution,
            HeaderMap::new(),
            codex_images_generation_request(br#"{"prompt":"blue"}"#).unwrap(),
            UsageLogContext::default(),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), payload.as_bytes());
        assert_eq!(upstream.captures.lock().unwrap().len(), 1);
        let usage = state.usage.read().await;
        let log = usage.logs.last().unwrap();
        assert_eq!(
            log.error_message
                .as_deref()
                .unwrap()
                .starts_with("image_text_model_unavailable:"),
            !direct
        );
        upstream.task.abort();
    }
}

#[test]
fn codex_images_policy_stream_model_rejection_is_request_scoped() {
    let mut parser = CodexImagesResponseParser::new();
    parser.text_driver = Some("gpt-6.2-sol".into());
    let value = json!({"type":"response.failed","response":{"status":"failed","error":{
        "code":"model_not_found","subreason":"model_not_in_accessible_catalog",
        "message":"The 'gpt-6.2-sol' model is not supported when using Codex with a ChatGPT account."}}});
    let parsed = parser
        .push(format!("data: {value}\n\n").as_bytes())
        .unwrap();
    let CodexImagesProtocolEvent::Failed(error) = &parsed.events[0] else {
        panic!("expected model rejection");
    };
    assert_eq!(error.code, "image_text_model_unavailable");
    assert!(error.provider_outcome().is_none());
    let rendered: Value = serde_json::from_slice(&render_codex_images_error(false, error)).unwrap();
    assert_eq!(rendered["error"]["type"], "invalid_request_error");
}

#[test]
fn codex_images_policy_direct_terminal_and_resource_evidence_are_strict() {
    let completed = format!(
        "data: {}\n\n",
        json!({"type":"image_generation.completed","b64_json":CODEX_IMAGES_POLICY_PNG})
    );
    for bad in [
        "data: [DONE]\n\n".to_string(),
        "data: {\"type\":\"image_edit.completed\",\"b64_json\":\"aA==\"}\n\n".to_string(),
        format!("{completed}{completed}"),
        format!("{completed}data: {{\"type\":\"error\",\"error\":{{\"message\":\"late failure\"}}}}\n\n"),
    ] {
        let mut parser = CodexImagesResponseParser::for_request(CodexImageTransport::Images, CodexImagesOperation::Generation);
        assert!(parser.push(bad.as_bytes()).is_err());
    }
    let mut parser = CodexImagesResponseParser::for_request(
        CodexImageTransport::Images,
        CodexImagesOperation::Generation,
    );
    parser.push(b": keepalive\n\n").unwrap();
    assert!(parser.finish().is_err());
    for boundary in 0..=completed.len() {
        let mut parser = CodexImagesResponseParser::for_request(
            CodexImageTransport::Images,
            CodexImagesOperation::Generation,
        );
        let first = parser.push(&completed.as_bytes()[..boundary]).unwrap();
        let second = parser.push(&completed.as_bytes()[boundary..]).unwrap();
        assert_eq!(first.events.len() + second.events.len(), 1);
        parser.finish().unwrap();
    }
    let mut observer = crate::domain::usage::value_pricing::ResourceUsageObserver::default();
    observer.observe(&json!({"type":"image_generation.partial_image","b64_json":"partial"}));
    assert!(observer.usage().is_none());
    for _ in 0..2 {
        observer.observe(
            &json!({"type":"image_generation.completed","b64_json":CODEX_IMAGES_POLICY_PNG}),
        );
    }
    assert_eq!(observer.usage().unwrap().quantities["image.output"], 1);
}

#[test]
fn codex_images_policy_direct_json_requires_eof_and_one_document() {
    let json =
        json!({"data":[{"b64_json":CODEX_IMAGES_POLICY_PNG}],"created":1700000000}).to_string();
    let mut parser = CodexImagesResponseParser::for_request(
        CodexImageTransport::Images,
        CodexImagesOperation::Generation,
    );
    assert!(parser.push(json.as_bytes()).unwrap().events.is_empty());
    assert_eq!(parser.finish().unwrap().events.len(), 1);
    let mut parser = CodexImagesResponseParser::for_request(
        CodexImageTransport::Images,
        CodexImagesOperation::Generation,
    );
    parser.push(json.as_bytes()).unwrap();
    assert!(parser.push(b"{}").is_err());
}
