// Real forwarder, loopback-only transport: this preserves the current retry
// boundary while the reference Build resubmission contract remains unverified.
#[tokio::test]
async fn grok_r4_5xx_is_returned_once_without_new_transient_or_idempotency_headers() {
    for code in [500, 503, 525, 526] {
        let status = StatusCode::from_u16(code).unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observed = seen.clone();
        let app = axum::Router::new().route("/v1/responses", axum::routing::post(move |headers: HeaderMap| {
            let observed = observed.clone();
            async move {
                observed.lock().unwrap().push(headers);
                (status, axum::Json(json!({"error":{"type":"server_error","message":"fixture failure"}})))
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let name = format!("grok-r4-status-{code}");
        let state = forwarder_test_state(&name);
        let execution = install_grok_test_execution(&state, &name, format!("http://{address}/v1"), None,
            "fixture-access", None, &[]).await;
        let mut headers = HeaderMap::new();
        headers.insert("x-grok-turn-idx", HeaderValue::from_static("7"));
        headers.insert("x-grok-transient-retry", HeaderValue::from_static("99"));
        headers.insert("idempotency-key", HeaderValue::from_static("untrusted-client-key"));
        let result = forward_for_test_surface(state.clone(), ProxyRoute::CodexResponses,
            execution.stored.provider.id.clone(), None, headers,
            Bytes::from_static(br#"{"model":"grok-4.6","input":"ping","stream":false}"#)).await;
        let actual = match result {
            Ok(response) => {
                let actual = response.status();
                let _ = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
                actual
            }
            Err(error) => error.status,
        };
        assert_eq!(actual, status);
        let observed = seen.lock().unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0]["x-grok-turn-idx"], "7");
        assert!(!observed[0].contains_key("x-grok-transient-retry"));
        assert!(!observed[0].contains_key("idempotency-key"));
        server.abort();
    }
}

#[test]
fn grok_r4_media_memory_capacity_keeps_local_shed_classification() {
    let error = grok_media_task_store_error(crate::state::GrokMediaMemoryCapacity.into());
    assert!(error.is_request_memory_exhausted());
    assert_eq!(error.error_code(), "cc_switch_request_memory_exhausted");
}

#[tokio::test]
async fn grok_r4_video_owner_capacity_failure_does_not_penalize_the_account_or_retry() {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = axum::Router::new().route("/v1/videos/generations", axum::routing::post(move || {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, Ordering::SeqCst);
            axum::Json(json!({"request_id":"capacity-video"}))
        }
    }));
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
    let state = forwarder_test_state("grok-r4-video-owner-capacity");
    let execution = install_grok_test_execution(&state, "grok-r4-video-owner-capacity",
        format!("http://{address}/v1"), None, "fixture-access", None,
        &[GrokAccountCapability::VideoGeneration]).await;
    let account_id = execution.managed_account_id().unwrap().to_string();
    let provider_id = execution.stored.provider.id.clone();
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(None);
    state.set_grok_media_parent_for_test(parent.clone());
    parent.configure_limit(Some(parent.snapshot().used_bytes));
    let mut headers = HeaderMap::new();
    headers.insert("x-cc-switch-request-id", HeaderValue::from_static("req_grok_r4_video_owner_capacity"));
    let error = forward_grok_media_provider_test(state.clone(), execution, Method::POST,
        "/videos/generations".into(), headers,
        Bytes::from_static(br#"{"model":"grok-imagine-video","prompt":"fixture"}"#)).await.unwrap_err();
    assert!(error.is_request_memory_exhausted(), "{error:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let usage = state.usage_snapshot().await;
    let log = usage.logs.iter().find(|log| log.request_id == "req_grok_r4_video_owner_capacity").unwrap();
    assert_eq!(log.stream_status.as_deref(), Some("memory_capacity"));
    assert_eq!(log.failure_kind.as_deref(), Some("memory_capacity"));
    assert!(usage.provider_health.get(AppKind::Codex, &provider_id).is_none_or(|health| health.consecutive_failures == 0));
    let account = state.find_account_by_id(&account_id).await.unwrap();
    assert!(!account.needs_relogin);
    assert!(account.last_refresh_error.is_none());
    assert!(state.grok_media_task_binding(&format!("test-share:{provider_id}"), None,
        GrokMediaTaskKind::VideoGeneration, "capacity-video").await.unwrap().is_none());
    server.abort();
}
