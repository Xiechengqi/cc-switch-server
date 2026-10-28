// Production forwarding and bridges with synthetic, loopback-only credentials.
// These cases verify observations, never real xAI acceptance or entitlement.

const GROK_CORE03_CONTENT: &str = concat!(
    ": keepalive\n\n",
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_timing\",\"status\":\"in_progress\"}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"content_index\":0,\"delta\":\"hello\"}\n\n",
);
const GROK_CORE03_TERMINAL: &str = concat!(
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_timing\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello\"}]}],\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n",
    "data: [DONE]\n\n",
);

fn grok_core03_body(route: ProxyRoute, streaming: bool) -> (Option<String>, Bytes) {
    let (path, value) = match route {
        ProxyRoute::ClaudeMessages => (
            None,
            json!({"model":"grok-4.6","stream":streaming,"max_tokens":64,"messages":[{"role":"user","content":"hi"}]}),
        ),
        ProxyRoute::CodexChatCompletions => (
            None,
            json!({"model":"grok-4.6","stream":streaming,"messages":[{"role":"user","content":"hi"}]}),
        ),
        ProxyRoute::Gemini => (
            Some(format!(
                "models/grok-4.6:{}",
                if streaming {
                    "streamGenerateContent"
                } else {
                    "generateContent"
                }
            )),
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"stream":streaming}),
        ),
        _ => (
            None,
            json!({"model":"grok-4.6","stream":streaming,"input":"hi"}),
        ),
    };
    (path, Bytes::from(value.to_string()))
}

fn grok_core03_attempt(execution: ProviderExecution) -> ForwardAttemptContext {
    ForwardAttemptContext {
        execution: Some(execution),
        provider_binding_pinned: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn grok_core03_http_json_and_sse_cover_three_apps_and_all_inference_surfaces() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |axum::Json(body): axum::Json<Value>| {
        let calls = observed.clone();
        async move {
            calls.fetch_add(1, Ordering::SeqCst);
            if body.get("stream").and_then(Value::as_bool) == Some(true) {
                ([(CONTENT_TYPE, "text/event-stream")], format!("{GROK_CORE03_CONTENT}{GROK_CORE03_TERMINAL}")).into_response()
            } else {
                axum::Json(json!({"id":"resp_json_timing","object":"response","status":"completed","model":"grok-4.6","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":1,"output_tokens":1}})).into_response()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("grok-core03-http-surfaces");
    let execution = install_grok_test_execution(
        &state,
        "grok-core03-http-surfaces",
        format!("http://{address}/v1"),
        None,
        "timing-access",
        None,
        &[],
    )
    .await;
    let id = execution.stored.provider.id.clone();
    let provider = execution.stored.provider.clone();
    state
        .mutate_providers_immediate(move |providers| {
            providers.upsert(AppKind::Claude, provider.clone());
            providers.upsert(AppKind::Gemini, provider);
        })
        .await
        .unwrap();
    for app in [AppKind::Claude, AppKind::Gemini] {
        state
            .override_provider_runtime_endpoint_for_test(app, &id, format!("http://{address}/v1"))
            .await
            .unwrap();
    }
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for streaming in [false, true] {
            let attempt = claude_timing_attempt(&state, route, &id).await;
            let slot = attempt.timing.clone();
            let (path, body) = grok_core03_body(route, streaming);
            let response =
                forward_with_attempt(state.clone(), route, path, HeaderMap::new(), body, attempt)
                    .await
                    .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let request = slot.get().unwrap().request();
            let before = request.snapshot();
            assert_eq!(before["providerType"], "grok_oauth");
            for phase in ["admission", "send_started", "headers"] {
                assert!(before["milestonesMs"][phase].is_number(), "{before}");
            }
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert!(
                String::from_utf8_lossy(&bytes).contains("hello"),
                "{route:?}: {bytes:?}"
            );
            drop(bytes);
            let after = request.snapshot();
            assert_eq!(
                after["result"], "complete",
                "{route:?} streaming={streaming}: {after}"
            );
            for phase in [
                "first_semantic",
                "terminal",
                "http_body_handoff",
                "owner_released",
            ] {
                assert!(
                    after["milestonesMs"][phase].is_number(),
                    "{route:?} streaming={streaming} {phase}: {after}"
                );
            }
            assert!(after["milestonesMs"]["downstream_write_completed"].is_null());
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 8);
    server.abort();
}

#[tokio::test]
async fn grok_core03_stream_observes_before_eof_and_header_cancel_releases_owner() {
    for headers_received in [false, true] {
        let app = axum::Router::new().fallback(axum::routing::post(move || async move {
            if !headers_received {
                std::future::pending::<()>().await;
            }
            let content = stream::once(async {
                Ok::<_, std::convert::Infallible>(Bytes::from_static(
                    GROK_CORE03_CONTENT.as_bytes(),
                ))
            });
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(content.chain(stream::pending())))
                .unwrap()
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let name = format!("grok-core03-cancel-{headers_received}");
        let state = forwarder_test_state(&name);
        let execution = install_grok_test_execution(
            &state,
            &name,
            format!("http://{address}/v1"),
            None,
            "timing-access",
            None,
            &[],
        )
        .await;
        let attempt = grok_core03_attempt(execution);
        let slot = attempt.timing.clone();
        let (_, body) = grok_core03_body(ProxyRoute::CodexResponses, true);
        let mut pending = Box::pin(forward_with_attempt(
            state,
            ProxyRoute::CodexResponses,
            None,
            HeaderMap::new(),
            body,
            attempt,
        ));
        let mut response_body = if headers_received {
            let response = tokio::time::timeout(Duration::from_secs(1), pending.as_mut())
                .await
                .unwrap()
                .unwrap();
            let mut stream = response.into_body().into_data_stream();
            loop {
                let frame = tokio::time::timeout(Duration::from_secs(1), stream.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap();
                if String::from_utf8_lossy(&frame).contains("hello") {
                    break;
                }
            }
            Some(stream)
        } else {
            assert!(
                tokio::time::timeout(Duration::from_millis(150), pending.as_mut())
                    .await
                    .is_err()
            );
            None
        };
        let request = slot.get().unwrap().request();
        let before = request.snapshot();
        assert!(
            before["milestonesMs"]["send_started"].is_number(),
            "{before}"
        );
        assert_eq!(
            before["milestonesMs"]["headers"].is_number(),
            headers_received,
            "{before}"
        );
        assert_eq!(
            before["milestonesMs"]["first_semantic"].is_number(),
            headers_received,
            "{before}"
        );
        assert!(before["milestonesMs"]["owner_released"].is_null());
        drop(pending);
        drop(response_body.take());
        let after = request.snapshot();
        assert_eq!(after["result"], "cancelled", "{after}");
        assert!(after["milestonesMs"]["owner_released"].is_number());
        server.abort();
    }
}

#[test]
fn grok_core03_controls_empty_delta_terminal_output_and_attempts_remain_distinct() {
    let slot = TimingSlot::default();
    let timing = slot
        .begin_for_provider(ProviderType::GrokOAuth, "responses", Instant::now())
        .unwrap();
    let mut inspector = ResponsesSseInspector::with_repeat_guard(true);
    inspector.set_timing(Some(timing.clone()));
    for frame in [
        "event: ping\ndata: {\"type\":\"ping\"}\n\n",
        "data: {\"type\":\"response.doom_loop_check\",\"detected\":false}\n\n",
        "data: {\"type\":\"response.metadata\",\"metadata\":{}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"message\",\"content\":[]}}\n\n",
    ] {
        inspector.push(frame.as_bytes()).unwrap();
        assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
    }
    // Timing alone may exclude billing control, but the protocol normalizer
    // must still reject the shape it did not previously accept.
    timing.observe_responses_value(
        &json!({"cost":"0"}),
        Some("ping"),
        &SemanticObservation::Business,
    );
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
    let mut invalid = ResponsesSseInspector::with_repeat_guard(true);
    invalid.set_timing(Some(timing.clone()));
    assert!(invalid
        .push(b"event: ping\ndata: {\"cost\":\"0\"}\n\n")
        .is_err());
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
    inspector.push(GROK_CORE03_TERMINAL.as_bytes()).unwrap();
    timing.finish_delivery(true);
    assert_eq!(timing.request().snapshot()["result"], "complete");
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number());

    let retry_slot = TimingSlot::default();
    let first = retry_slot
        .begin_for_provider(ProviderType::GrokOAuth, "responses", Instant::now())
        .unwrap();
    first.observe_terminal(TimingResult::Failed);
    assert!(first.request().snapshot()["result"].is_null());
    let second = retry_slot
        .begin_for_provider(ProviderType::GrokOAuth, "responses", Instant::now())
        .unwrap();
    assert_eq!(first.attempt_snapshot()["result"], "retried");
    second.observe_terminal(TimingResult::Complete);
    second.finish_delivery(true);
    assert_eq!(second.request().snapshot()["result"], "complete");
    assert!(second.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
}

#[tokio::test]
async fn grok_core03_committed_error_is_failed_once_and_body_cancel_waits_for_last_owner() {
    for fail in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = axum::Router::new().fallback(axum::routing::post(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            async move {
                let content = stream::once(async { Ok::<_, std::convert::Infallible>(Bytes::from_static(GROK_CORE03_CONTENT.as_bytes())) });
                let tail = stream::once(async move {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    if !fail { std::future::pending::<()>().await; }
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\",\"message\":\"fixture failure\"}}}\n\n"))
                });
                Response::builder().header(CONTENT_TYPE, "text/event-stream").body(Body::from_stream(content.chain(tail))).unwrap()
            }
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let name = format!("grok-core03-committed-{fail}");
        let state = forwarder_test_state(&name);
        let execution = install_grok_test_execution(
            &state,
            &name,
            format!("http://{address}/v1"),
            None,
            "timing-access",
            None,
            &[],
        )
        .await;
        let attempt = grok_core03_attempt(execution);
        let slot = attempt.timing.clone();
        let (_, body) = grok_core03_body(ProxyRoute::CodexResponses, true);
        let response = forward_with_attempt(
            state,
            ProxyRoute::CodexResponses,
            None,
            HeaderMap::new(),
            body,
            attempt,
        )
        .await
        .unwrap();
        let request = slot.get().unwrap().request();
        let mut body = response.into_body().into_data_stream();
        let retained = loop {
            let frame = tokio::time::timeout(Duration::from_secs(1), body.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            if String::from_utf8_lossy(&frame).contains("hello") {
                break frame;
            }
        };
        assert!(request.snapshot()["milestonesMs"]["first_semantic"].is_number());
        if fail {
            while let Some(frame) = body.next().await {
                drop(frame);
            }
            assert_eq!(request.snapshot()["result"], "failed");
        }
        drop(body);
        if !fail {
            assert_eq!(request.snapshot()["result"], "cancelled");
        }
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_null());
        drop(retained);
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn grok_core03_media_heartbeat_progress_and_result_have_separate_clocks() {
    let upstream = spawn_test_grok_image_upstream("text/event-stream", vec![
        (Duration::ZERO, Bytes::from_static(b"data: {\"type\":\"image_generation.progress\",\"progress\":50}\n\n")),
        (Duration::from_millis(70), Bytes::from_static(b"data: {\"type\":\"image_generation.completed\",\"data\":[{\"b64_json\":\"aGVsbG8=\"}]}\n\n")),
    ]).await;
    let state = forwarder_test_state("grok-core03-image-timing");
    let mut execution = install_grok_test_execution(
        &state,
        "grok-core03-image-timing",
        format!("http://{}", upstream.address),
        None,
        "timing-access",
        None,
        &[GrokAccountCapability::ImageGeneration],
    )
    .await;
    Arc::make_mut(&mut execution.plan)
        .driver_options
        .insert("testImageKeepaliveMs".into(), json!(10));
    let body = Bytes::from_static(br#"{"model":"grok-imagine-image","prompt":"fixture"}"#);
    let memory = GrokMediaRequestMemory::new(1024 * 1024, body.len()).unwrap();
    let slot = memory.timing.clone();
    let response = forward_grok_media_with_execution(
        state,
        execution,
        Method::POST,
        "/images/generations".into(),
        HeaderMap::new(),
        body,
        None,
        UsageLogContext::default(),
        None,
        None,
        memory,
    )
    .await
    .unwrap();
    let request = slot.get().unwrap().request();
    let mut body = response.into_body().into_data_stream();
    let initial = body.next().await.unwrap().unwrap();
    assert!(initial.starts_with(b": connected"));
    assert!(request.snapshot()["milestonesMs"]["http_body_handoff"].is_number());
    assert!(request.snapshot()["milestonesMs"]["first_semantic"].is_null());
    drop(initial);
    let progress = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&progress).contains("progress"));
    drop(progress);
    let heartbeat = body.next().await.unwrap().unwrap();
    assert!(heartbeat.starts_with(b": keepalive"));
    assert!(request.snapshot()["milestonesMs"]["first_semantic"].is_null());
    drop(heartbeat);
    let mut retained = None;
    while let Some(frame) = body.next().await {
        let frame = frame.unwrap();
        if String::from_utf8_lossy(&frame).contains("aGVsbG8=") {
            retained = Some(frame);
        }
    }
    drop(body);
    assert!(retained.is_some());
    let completed = request.snapshot();
    assert_eq!(completed["providerType"], "grok_oauth");
    assert_eq!(completed["surface"], "image_generation");
    assert_eq!(completed["result"], "complete", "{completed}");
    assert!(completed["milestonesMs"]["first_semantic"].is_number());
    assert!(completed["waitsMs"]["upstream_read"].as_f64().unwrap() > 0.0);
    assert!(completed["milestonesMs"]["owner_released"].is_null());
    drop(retained);
    assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
    upstream.server.abort();
}

#[test]
fn grok_core03_media_borrowed_observation_ignores_progress_and_preserves_failure_at_eof() {
    for (payload, content, result) in [
        (
            r#"{"progress":50,"status":"processing"}"#,
            false,
            "complete",
        ),
        (
            r#"{"data":[{"url":"https://fixture.invalid/image"}]}"#,
            true,
            "complete",
        ),
        (r#"{"data":[{"b64_json":""}]}"#, false, "complete"),
        (
            r#"{"error":{"message":"fixture failure"}}"#,
            false,
            "failed",
        ),
        (
            r#"{"data":{"status":"failed","video":{"url":"https://fixture.invalid/video"}}}"#,
            false,
            "failed",
        ),
        (
            r#"{"error":false,"data":[{"b64_json":" "}]}"#,
            false,
            "complete",
        ),
        (r#"{"error": { },"status":"processing"}"#, false, "complete"),
        (r#"{"error":"\u0020"}"#, false, "complete"),
        (r#"{"status":"\u0066ailed"}"#, false, "failed"),
    ] {
        for sse in [false, true] {
            let slot = TimingSlot::default();
            let timing = slot
                .begin_for_provider(ProviderType::GrokOAuth, "image_edit", Instant::now())
                .unwrap();
            if sse {
                let budget = RequestMemoryBudget::new(1024 * 1024);
                super::super::grok_timing::observe_sse(
                    &timing,
                    format!("data: {payload}\n\n").as_bytes(),
                    &budget,
                );
            } else {
                super::super::grok_timing::observe_value(
                    &timing,
                    &serde_json::from_str::<Value>(payload).unwrap(),
                );
            }
            timing.observe_media_eof();
            timing.finish_delivery(true);
            let after = timing.request().snapshot();
            assert_eq!(after["result"], result, "sse={sse}: {after}");
            assert_eq!(
                after["milestonesMs"]["first_semantic"].is_number(),
                content,
                "sse={sse}: {after}"
            );
        }
    }
}

#[test]
fn grok_core03_multiline_observation_and_scratch_refusal_preserve_forwarding_budget() {
    for parent_limited in [false, true] {
        let parent =
            crate::infra::resident_memory::ResidentMemoryBudget::new(Some(if parent_limited {
                64
            } else {
                1024
            }));
        let budget = RequestMemoryBudget::with_parent(
            if parent_limited { 1024 } else { 64 },
            parent.clone(),
        );
        let retained = budget
            .reserve(RequestMemoryComponent::InboundRaw, 32)
            .unwrap();
        assert!(budget.reserve_observation_scratch(64).is_none());
        assert!(!budget.is_exhausted());
        assert_eq!(budget.snapshot().used_bytes, 32);
        let scratch = budget.reserve_observation_scratch(16).unwrap();
        let clone = scratch.clone();
        drop(scratch);
        assert_eq!(budget.snapshot().used_bytes, 48);
        drop(clone);
        assert_eq!(budget.snapshot().used_bytes, 32);
        let required = budget
            .reserve(RequestMemoryComponent::NormalizedEvent, 16)
            .unwrap();
        drop(required);
        drop(retained);
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
    let slot = TimingSlot::default();
    let timing = slot
        .begin_for_provider(ProviderType::GrokOAuth, "image_generation", Instant::now())
        .unwrap();
    let budget = RequestMemoryBudget::new(4096);
    let frame = b"event: image_generation.completed\r\ndata: {\"type\":\"image_generation.completed\",\r\ndata: \"data\":[{\"url\":\"https://fixture.invalid/image\"}]}\r\n\r\n";
    super::super::grok_timing::observe_sse(&timing, frame, &budget);
    timing.finish_delivery(true);
    assert_eq!(timing.request().snapshot()["result"], "complete");
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number());
    assert_eq!(budget.snapshot().used_bytes, 0);

    let failure_slot = TimingSlot::default();
    let timing = failure_slot
        .begin_for_provider(ProviderType::GrokOAuth, "image_generation", Instant::now())
        .unwrap();
    for frame in [
        b"data: {\"error\":{\"message\":\"fixture error\"}}\n\n".as_slice(),
        b"data: {\"type\":\"image_generation.completed\"}\n\n",
    ] {
        super::super::grok_timing::observe_sse(&timing, frame, &budget);
    }
    timing.observe_media_eof();
    timing.finish_delivery(true);
    assert_eq!(timing.request().snapshot()["result"], "failed");

    let error_slot = TimingSlot::default();
    let timing = error_slot
        .begin_for_provider(ProviderType::GrokOAuth, "responses", Instant::now())
        .unwrap();
    let observation = response_semantics::classify_json_document_with_timing(
        br#"{"type":"error","error":{"message":"fixture error"}}"#,
        Some(&timing),
    )
    .unwrap();
    assert!(matches!(observation, SemanticObservation::ErrorFrame(_)));
    timing.finish_delivery(true);
    assert_eq!(timing.request().snapshot()["result"], "failed");
}

#[derive(Clone, Default)]
struct GrokTimingSamples(Arc<StdMutex<Vec<(metrics::Key, f64)>>>);

struct GrokTimingSample {
    samples: GrokTimingSamples,
    key: metrics::Key,
}

impl metrics::HistogramFn for GrokTimingSample {
    fn record(&self, value: f64) {
        self.samples
            .0
            .lock()
            .unwrap()
            .push((self.key.clone(), value));
    }
}

impl metrics::Recorder for GrokTimingSamples {
    fn describe_counter(
        &self,
        _: metrics::KeyName,
        _: Option<metrics::Unit>,
        _: metrics::SharedString,
    ) {
    }
    fn describe_gauge(
        &self,
        _: metrics::KeyName,
        _: Option<metrics::Unit>,
        _: metrics::SharedString,
    ) {
    }
    fn describe_histogram(
        &self,
        _: metrics::KeyName,
        _: Option<metrics::Unit>,
        _: metrics::SharedString,
    ) {
    }
    fn register_counter(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Counter {
        metrics::Counter::noop()
    }
    fn register_gauge(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Gauge {
        metrics::Gauge::noop()
    }
    fn register_histogram(
        &self,
        key: &metrics::Key,
        _: &metrics::Metadata<'_>,
    ) -> metrics::Histogram {
        if !key.name().starts_with("cc_switch_forward_") {
            return metrics::Histogram::noop();
        }
        metrics::Histogram::from_arc(Arc::new(GrokTimingSample {
            samples: self.clone(),
            key: key.clone(),
        }))
    }
}

impl GrokTimingSamples {
    fn count(&self, metric: &str, labels: &[(&str, &str)]) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| {
                key.name() == metric
                    && labels.iter().all(|(name, value)| {
                        key.labels()
                            .any(|label| label.key() == *name && label.value() == *value)
                    })
            })
            .count()
    }
}

#[tokio::test]
async fn grok_core03_media_json_edit_video_submission_and_status_preserve_response_semantics() {
    let app = axum::Router::new().fallback(axum::routing::any(|uri: axum::http::Uri| async move {
        let body = match uri.path() {
            "/images/edits" => json!({"data":[{"url":"https://fixture.invalid/image"}]}),
            "/images/generations" => json!({"error":{"message":"fixture image error"}}),
            "/videos/generations" => json!({"request_id":"timing-video"}),
            "/videos/processing" => json!({"status":"processing","progress":50}),
            "/videos/failed" => {
                json!({"status":"failed","error":{"message":"fixture video error"}})
            }
            _ => json!({"status":"completed","video":{"url":"https://fixture.invalid/video"}}),
        };
        axum::Json(body)
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("grok-core03-media-json");
    let execution = install_grok_test_execution(
        &state,
        "grok-core03-media-json",
        format!("http://{address}"),
        None,
        "timing-access",
        None,
        &[
            GrokAccountCapability::ImageGeneration,
            GrokAccountCapability::ImageEdit,
            GrokAccountCapability::VideoGeneration,
        ],
    )
    .await;
    for (path, method, surface, content, result) in [
        (
            "/images/edits",
            Method::POST,
            "image_edit",
            true,
            "complete",
        ),
        (
            "/images/generations",
            Method::POST,
            "image_generation",
            false,
            "failed",
        ),
        (
            "/videos/generations",
            Method::POST,
            "video_generation",
            true,
            "complete",
        ),
        (
            "/videos/processing",
            Method::GET,
            "video_status",
            false,
            "complete",
        ),
        (
            "/videos/failed",
            Method::GET,
            "video_status",
            false,
            "failed",
        ),
        (
            "/videos/completed",
            Method::GET,
            "video_status",
            true,
            "complete",
        ),
    ] {
        let body = Bytes::from_static(br#"{"model":"grok-imagine-video","prompt":"fixture"}"#);
        let memory = GrokMediaRequestMemory::new(1024 * 1024, body.len()).unwrap();
        let slot = memory.timing.clone();
        let response = forward_grok_media_with_execution(
            state.clone(),
            execution.clone(),
            method,
            path.into(),
            HeaderMap::new(),
            body,
            None,
            UsageLogContext::default(),
            None,
            None,
            memory,
        )
        .await
        .unwrap();
        // Image media's existing success decision remains HTTP/JSON based;
        // the independent observation can still identify an embedded error.
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(serde_json::from_slice::<Value>(&bytes).is_ok());
        drop(bytes);
        let observed = slot.get().unwrap().request().snapshot();
        assert_eq!(observed["surface"], surface);
        assert_eq!(observed["result"], result, "{path}: {observed}");
        assert_eq!(
            observed["milestonesMs"]["first_semantic"].is_number(),
            content,
            "{path}: {observed}"
        );
        assert!(
            observed["milestonesMs"]["owner_released"].is_number(),
            "{path}: {observed}"
        );
    }
    server.abort();
}

#[tokio::test]
async fn grok_core03_media_header_and_body_cancellation_record_before_release() {
    for headers_received in [false, true] {
        let app = axum::Router::new().fallback(axum::routing::post(move || async move {
            if !headers_received {
                std::future::pending::<()>().await;
            }
            let content = stream::once(async {
                Ok::<_, std::convert::Infallible>(Bytes::from_static(
                    b"data: {\"progress\":50}\n\n",
                ))
            });
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(content.chain(stream::pending())))
                .unwrap()
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let name = format!("grok-core03-media-cancel-{headers_received}");
        let state = forwarder_test_state(&name);
        let execution = install_grok_test_execution(
            &state,
            &name,
            format!("http://{address}"),
            None,
            "timing-access",
            None,
            &[GrokAccountCapability::ImageGeneration],
        )
        .await;
        let body = Bytes::from_static(br#"{"model":"grok-imagine-image","prompt":"fixture"}"#);
        let memory = GrokMediaRequestMemory::new(1024 * 1024, body.len()).unwrap();
        let slot = memory.timing.clone();
        let mut future = Box::pin(forward_grok_media_with_execution(
            state,
            execution,
            Method::POST,
            "/images/generations".into(),
            HeaderMap::new(),
            body,
            None,
            UsageLogContext::default(),
            None,
            None,
            memory,
        ));
        let mut retained = None;
        if headers_received {
            let response = future.as_mut().await.unwrap();
            let mut body = response.into_body().into_data_stream();
            drop(body.next().await.unwrap().unwrap());
            retained = Some(body.next().await.unwrap().unwrap());
            assert!(
                slot.get().unwrap().request().snapshot()["milestonesMs"]["first_semantic"]
                    .is_null()
            );
            drop(body);
        } else {
            assert!(
                tokio::time::timeout(Duration::from_millis(150), future.as_mut())
                    .await
                    .is_err()
            );
        }
        let request = slot.get().unwrap().request();
        drop(future);
        let cancelled = request.snapshot();
        assert_eq!(cancelled["result"], "cancelled", "{cancelled}");
        assert_eq!(
            cancelled["milestonesMs"]["headers"].is_number(),
            headers_received
        );
        if retained.is_some() {
            assert!(cancelled["milestonesMs"]["owner_released"].is_null());
        }
        drop(retained);
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
        server.abort();
    }
}

#[tokio::test]
async fn grok_core03_media_401_has_independent_attempt_and_refresh_wait_without_extra_submission() {
    let samples = GrokTimingSamples::default();
    let _recorder = metrics::set_default_local_recorder(&samples);
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = axum::Router::new()
        .route("/token", axum::routing::post(|| async {
            tokio::time::sleep(Duration::from_millis(15)).await;
            axum::Json(json!({"access_token":"grok-timing-new","refresh_token":"grok-timing-refresh","expires_in":3600}))
        }))
        .route("/images/generations", axum::routing::post(move |headers: HeaderMap| {
            let calls = observed.clone();
            async move {
                calls.fetch_add(1,Ordering::SeqCst);
                if headers[axum::http::header::AUTHORIZATION] == "Bearer grok-timing-old" {
                    (StatusCode::UNAUTHORIZED, axum::Json(json!({"error":{"message":"refresh"}}))).into_response()
                } else {
                    assert_eq!(headers[axum::http::header::AUTHORIZATION],"Bearer grok-timing-new");
                    axum::Json(json!({"data":[{"b64_json":"aGVsbG8="}]})).into_response()
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("grok-core03-media-refresh");
    let execution = install_grok_test_execution(
        &state,
        "grok-core03-media-refresh",
        format!("http://{address}"),
        None,
        "grok-timing-old",
        Some(format!("http://{address}/token")),
        &[GrokAccountCapability::ImageGeneration],
    )
    .await;
    let body = Bytes::from_static(br#"{"model":"grok-imagine-image","prompt":"fixture"}"#);
    let memory = GrokMediaRequestMemory::new(1024 * 1024, body.len()).unwrap();
    let slot = memory.timing.clone();
    let response = forward_grok_media_with_execution(
        state,
        execution,
        Method::POST,
        "/images/generations".into(),
        HeaderMap::new(),
        body,
        None,
        UsageLogContext::default(),
        None,
        None,
        memory,
    )
    .await
    .unwrap();
    drop(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    );
    let observed = slot.get().unwrap().request().snapshot();
    assert_eq!(observed["result"], "complete", "{observed}");
    assert!(
        observed["waitsMs"]["retry"].as_f64().unwrap() >= 10.0,
        "{observed}"
    );
    assert_eq!(
        samples.count(
            "cc_switch_forward_duration_seconds",
            &[
                ("provider_type", "grok_oauth"),
                ("scope", "attempt"),
                ("result", "retried")
            ]
        ),
        1
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn grok_core03_websocket_native_and_http_fallback_use_grok_labels_and_real_writes() {
    for fallback in [false, true] {
        let samples = GrokTimingSamples::default();
        let _recorder = metrics::set_default_local_recorder(&samples);
        let app = axum::Router::new()
            .route("/ws", axum::routing::get(move |ws: WebSocketUpgrade| async move {
                ws.on_upgrade(move |mut socket| async move {
                    if socket.recv().await.is_some() {
                        if fallback {
                            socket.send(AxumWsMessage::Close(Some(axum::extract::ws::CloseFrame {code:1009, reason:"fallback".into()}))).await.unwrap();
                        } else {
                            for value in [json!({"type":"response.created","response":{"id":"resp_ws_timing","status":"in_progress"}}), json!({"type":"response.output_text.delta","delta":"hello"}), json!({"type":"response.completed","response":{"id":"resp_ws_timing","status":"completed","output":[]}})] {
                                socket.send(AxumWsMessage::Text(value.to_string())).await.unwrap();
                            }
                        }
                    }
                })
            }))
            .route("/v1/responses", axum::routing::post(|| async { ([(CONTENT_TYPE,"text/event-stream")], format!("{GROK_CORE03_CONTENT}{GROK_CORE03_TERMINAL}")) }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let name = format!("grok-core03-ws-{fallback}");
        let state = forwarder_test_state(&name);
        let execution = install_grok_test_execution(
            &state,
            &name,
            format!("http://{address}/v1"),
            Some(format!("ws://{address}/ws")),
            "timing-access",
            None,
            &[GrokAccountCapability::Websocket],
        )
        .await;
        let (bridge, bridge_server) =
            spawn_test_grok_responses_bridge(state, execution, "grok-timing-session", Some(0))
                .await;
        let events = send_test_bridge_request(bridge, "hi").await;
        assert!(
            events.iter().any(
                |value| value.get("type").and_then(Value::as_str) == Some("response.completed")
            ),
            "{events:?}"
        );
        let labels = [
            ("provider_type", "grok_oauth"),
            ("surface", "responses_websocket"),
            ("scope", "request"),
        ];
        for phase in [
            "admission",
            "send_started",
            "headers",
            "first_semantic",
            "upstream_write_completed",
            "downstream_write_completed",
            "terminal",
            "owner_released",
        ] {
            let mut expected = labels.to_vec();
            expected.push(("phase", phase));
            tokio::time::timeout(Duration::from_secs(1), async {
                while samples.count("cc_switch_forward_milestone_seconds", &expected) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "missing {phase}, fallback={fallback}: {:?}",
                    samples.0.lock().unwrap()
                )
            });
            assert_eq!(
                samples.count("cc_switch_forward_milestone_seconds", &expected),
                1
            );
        }
        assert_eq!(
            samples.count(
                "cc_switch_forward_milestone_seconds",
                &[("provider_type", "codex_oauth")]
            ),
            0
        );
        assert_eq!(
            samples.count(
                "cc_switch_forward_duration_seconds",
                &[
                    ("provider_type", "grok_oauth"),
                    ("scope", "request"),
                    ("result", "complete")
                ]
            ),
            1
        );
        assert_eq!(
            samples.count(
                "cc_switch_forward_duration_seconds",
                &[
                    ("provider_type", "grok_oauth"),
                    ("scope", "attempt"),
                    ("result", "retried")
                ]
            ),
            usize::from(fallback)
        );
        bridge_server.abort();
        server.abort();
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn grok_core03_websocket_header_and_body_abort_and_committed_protocol_error_finish_once() {
    for stage in 0..3 {
        let samples = GrokTimingSamples::default();
        let _recorder = metrics::set_default_local_recorder(&samples);
        let release_error = Arc::new(tokio::sync::Notify::new());
        let upstream_error = release_error.clone();
        let http_calls = Arc::new(AtomicUsize::new(0));
        let observed_http = http_calls.clone();
        let app = axum::Router::new()
            .route("/ws", axum::routing::get(move |ws: WebSocketUpgrade| {
                let release_error = upstream_error.clone();
                async move {
                    if stage == 0 { std::future::pending::<()>().await; }
                    ws.on_upgrade(move |mut socket| async move {
                        if socket.recv().await.is_some() {
                            socket.send(AxumWsMessage::Text(json!({"type":"response.output_text.delta","delta":"hello"}).to_string())).await.unwrap();
                            if stage == 2 {
                                release_error.notified().await;
                                let _ = socket.send(AxumWsMessage::Text("{malformed".into())).await;
                            }
                            let _ = socket.recv().await;
                        }
                    })
                }
            }))
            .route("/v1/responses", axum::routing::post(move || {
                observed_http.fetch_add(1, Ordering::SeqCst);
                async { StatusCode::INTERNAL_SERVER_ERROR }
            }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let name = format!("grok-core03-ws-exit-{stage}");
        let state = forwarder_test_state(&name);
        let execution = install_grok_test_execution(
            &state,
            &name,
            format!("http://{address}/v1"),
            Some(format!("ws://{address}/ws")),
            "timing-access",
            None,
            &[GrokAccountCapability::Websocket],
        )
        .await;
        let (abort, registration) = futures_util::future::AbortHandle::new_pair();
        let (bridge, bridge_server) = spawn_test_grok_responses_bridge_with_abort(
            state,
            execution,
            "grok-timing-exit",
            Some(0),
            Some(registration),
        )
        .await;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{bridge}/bridge"))
            .await
            .unwrap();
        socket
            .send(TungsteniteMessage::Text(
                json!({"type":"response.create","model":"grok-4.6","input":"hi"}).to_string(),
            ))
            .await
            .unwrap();
        let base = [
            ("provider_type", "grok_oauth"),
            ("surface", "responses_websocket"),
            ("scope", "request"),
        ];
        if stage == 0 {
            tokio::time::timeout(Duration::from_secs(1), async {
                let mut labels = base.to_vec();
                labels.push(("phase", "send_started"));
                while samples.count("cc_switch_forward_milestone_seconds", &labels) == 0 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        } else {
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let frame = socket.next().await.unwrap().unwrap();
                    if matches!(frame, TungsteniteMessage::Text(ref text) if text.contains("hello"))
                    {
                        break;
                    }
                }
            })
            .await
            .unwrap();
        }
        if stage == 2 {
            release_error.notify_one();
        } else {
            abort.abort();
        }
        let result = if stage == 2 { "failed" } else { "cancelled" };
        let mut result_labels = base.to_vec();
        result_labels.push(("result", result));
        let mut owner_labels = base.to_vec();
        owner_labels.push(("phase", "owner_released"));
        tokio::time::timeout(Duration::from_secs(2), async {
            while samples.count("cc_switch_forward_duration_seconds", &result_labels) == 0
                || samples.count("cc_switch_forward_milestone_seconds", &owner_labels) == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("stage={stage}: {:?}", samples.0.lock().unwrap()));
        assert_eq!(
            samples.count("cc_switch_forward_duration_seconds", &result_labels),
            1
        );
        assert_eq!(
            samples.count("cc_switch_forward_milestone_seconds", &owner_labels),
            1
        );
        for (phase, expected) in [("first_semantic", stage != 0), ("headers", stage != 0)] {
            let mut labels = base.to_vec();
            labels.push(("phase", phase));
            assert_eq!(
                samples.count("cc_switch_forward_milestone_seconds", &labels),
                usize::from(expected)
            );
        }
        if stage != 2 {
            let mut labels = base.to_vec();
            labels.push(("phase", "cancel_drain"));
            assert_eq!(samples.count("cc_switch_forward_wait_seconds", &labels), 1);
        }
        assert_eq!(
            samples.count(
                "cc_switch_forward_duration_seconds",
                &[("provider_type", "grok_oauth"), ("result", "unfinished")]
            ),
            0
        );
        assert_eq!(http_calls.load(Ordering::SeqCst), 0);
        drop(socket);
        bridge_server.abort();
        server.abort();
        tokio::task::yield_now().await;
    }
}

#[test]
fn grok_core03_completed_ws_turn_survives_later_connection_exit_and_slot_holds_no_budget() {
    let slot = TimingSlot::default();
    let timing = RequestTiming::responses_websocket(Instant::now(), ProviderType::GrokOAuth)
        .unwrap()
        .attempt(Instant::now());
    slot.adopt_websocket_attempt(timing.clone());
    let budget = RequestMemoryBudget::new(1024);
    budget.set_timing(timing.clone());
    timing.observe_terminal(TimingResult::Complete);
    timing.finish_delivery(true);
    drop(budget);
    assert!(timing.request().snapshot()["milestonesMs"]["owner_released"].is_number());
    slot.get().unwrap().finish_websocket_exit(true);
    assert_eq!(timing.request().snapshot()["result"], "complete");
    assert_eq!(timing.attempt_snapshot()["result"], "complete");
}
