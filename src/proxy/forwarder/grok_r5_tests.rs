// Synthetic xAI CLI OAuth evidence only. No SSO, vendor receipt, or replay.
#[tokio::test]
async fn grok_r5_long_reasoning_and_repeated_content_survive_delayed_body_demand() {
    let reasoning = format!("r5-thinking-{}", "x".repeat(16 * 1024));
    let mut wire = String::from(": keepalive\n\n");
    for _ in 0..3 {
        wire.push_str(&format!(
            "data: {}\n\n",
            json!({"type":"response.reasoning_summary_text.delta","delta":reasoning})
        ));
    }
    for _ in 0..2 {
        wire.push_str(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"repeat-me\"}\n\n",
        );
    }
    wire.push_str("data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":4}}}\n\ndata: [DONE]\n\n");
    let expected = wire.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let app = axum::Router::new().route(
        "/v1/responses",
        axum::routing::post(move || {
            let wire = wire.clone();
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                let chunks: Vec<_> = wire
                    .as_bytes()
                    .chunks(701)
                    .map(Bytes::copy_from_slice)
                    .collect();
                let body =
                    futures_util::stream::unfold(chunks.into_iter(), |mut chunks| async move {
                        let chunk = chunks.next()?;
                        tokio::time::sleep(Duration::from_millis(1)).await;
                        Some((Ok::<_, std::io::Error>(chunk), chunks))
                    });
                (
                    [(CONTENT_TYPE, "text/event-stream")],
                    Body::from_stream(body),
                )
                    .into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("grok-r5-delayed-demand");
    let execution = install_grok_test_execution(
        &state,
        "grok-r5-delayed-demand",
        format!("http://{address}/v1"),
        None,
        "grok-r5-access",
        None,
        &[],
    )
    .await;
    let attempt = grok_core03_attempt(execution);
    let slot = attempt.timing.clone();
    let response = forward_with_attempt(
        state,
        ProxyRoute::CodexResponses,
        None,
        HeaderMap::new(),
        Bytes::from_static(br#"{"model":"grok-4.6","input":"hi","stream":true}"#),
        attempt,
    )
    .await
    .unwrap();
    let request = slot.get().unwrap().request();
    tokio::time::sleep(Duration::from_millis(40)).await;
    let bytes = tokio::time::timeout(
        Duration::from_secs(3),
        axum::body::to_bytes(response.into_body(), 1024 * 1024),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(bytes.as_ref(), expected.as_bytes());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(bytes);
    let snapshot = request.snapshot();
    assert_eq!(snapshot["providerType"], "grok_oauth");
    assert_eq!(snapshot["result"], "complete");
    assert!(snapshot["milestonesMs"]["first_semantic"].is_number());
    assert!(snapshot["milestonesMs"]["owner_released"].is_number());
    server.abort();
}
