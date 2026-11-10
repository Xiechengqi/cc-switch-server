async fn install_bedrock_r5_bearer(state: &ServerState, endpoint: &str) -> String {
    let mut stored = stored_provider(
        AppKind::Claude,
        ProviderType::AwsBedrock,
        json!({"env":{"ANTHROPIC_AUTH_TOKEN":"provider-owned-key","AWS_REGION":"us-west-2"},"testRuntimeEndpoint":endpoint,"modelMapping":{"mode":"single","upstreamModel":"exact-model"}}),
        None,
    );
    let id = stored.provider.id.clone();
    stored.resource.profile_id = Some(
        crate::domain::providers::registry::ProfileId::parse("claude.aws_bedrock_api_key").unwrap(),
    );
    stored.resource.profile_schema_revision = Some(1);
    let mut providers = ProviderStore {
        providers: vec![stored],
        ..Default::default()
    };
    providers
        .rebuild_runtime_index(&state.accounts_snapshot().await)
        .unwrap();
    state.replace_provider_store_for_test(providers).await;
    id
}

#[tokio::test]
async fn bedrock_r5_bearer_actual_forwarder_normalizes_json_and_fragmented_eventstream() {
    for stream in [false, true] {
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = observed.clone();
        let app=axum::Router::new().fallback(move|uri:axum::http::Uri,headers:HeaderMap,axum::Json(body):axum::Json<Value>|{
            let seen=seen.clone();async move{
                seen.lock().unwrap().push((uri.path().to_string(),headers,body));
                if stream {
                    let bytes=super::super::bedrock::tests::text_stream();
                    let chunks:Vec<_>=bytes.chunks(37).map(|part|Ok::<Bytes,std::io::Error>(Bytes::copy_from_slice(part))).collect();
                    Response::builder().header(CONTENT_TYPE,"application/vnd.amazon.eventstream").body(Body::from_stream(stream::iter(chunks))).unwrap()
                }else{
                    Response::builder().header(CONTENT_TYPE,"application/json").body(Body::from(json!({"output":{"message":{"role":"assistant","content":[{"text":"hello"}]}},"stopReason":"end_turn","usage":{"inputTokens":11,"outputTokens":2,"totalTokens":13}}).to_string())).unwrap()
                }
            }
        });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("bedrock-r5-{stream}"));
        let id = install_bedrock_r5_bearer(&state, &format!("http://{address}")).await;
        let (response, timing)=r5_timing_forward(state,ProxyRoute::ClaudeMessages,id,None,HeaderMap::new(),Bytes::from(json!({"model":"downstream-model","stream":stream,"max_tokens":128,"messages":[{"role":"user","content":"hello"}]}).to_string())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[CONTENT_TYPE],
            if stream {
                "text/event-stream"
            } else {
                "application/json"
            }
        );
        let bytes = collect_response_body(response).await;
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        if stream {
            assert_eq!(text.matches("event: message_stop\n").count(), 1);
            assert!(text.contains("hello"));
            assert!(text.contains("\"input_tokens\":11"));
        } else {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(value["type"], "message");
            assert_eq!(value["model"], "exact-model");
            assert_eq!(value["content"][0]["text"], "hello");
        }
        {
            let seen = observed.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert!(seen[0].0.ends_with(if stream {
                "/converse-stream"
            } else {
                "/converse"
            }));
            assert_eq!(seen[0].1["authorization"], "Bearer provider-owned-key");
            assert_eq!(seen[0].2["messages"][0]["content"][0]["text"], "hello");
        }
        drop(bytes);
        r5_timing_assert_delivery(&timing, "aws_bedrock", "complete").await;
        server.abort();
    }
}

#[tokio::test]
async fn bedrock_r5_normalized_output_and_decoder_charge_their_actual_last_owners() {
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(2 * 1024 * 1024));
    let budget = RequestMemoryBudget::with_parent(2 * 1024 * 1024, parent.clone());
    let source =
        stream::iter([Ok(Bytes::from(super::super::bedrock::tests::text_stream()))]).boxed();
    let mut normalized = normalize_bedrock_stream(source, "m".to_string(), Some(&budget)).unwrap();
    let output = normalized.try_next().await.ok().flatten().unwrap();
    let last_view = output.clone();
    assert!(budget.snapshot().used_bytes > output.len());
    drop(normalized);
    drop(output);
    assert_eq!(budget.snapshot().used_bytes, last_view.len());
    drop(last_view);
    assert_eq!(budget.snapshot().used_bytes, 0);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn bedrock_r5_cancellation_releases_pending_frame_and_source_without_replay() {
    struct Owner(Arc<AtomicUsize>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let read_counter = reads.clone();
    let prefix = super::super::bedrock::tests::text_stream()[..10].to_vec();
    let source = stream::unfold((Owner(drops.clone()), false), move |(owner, sent)| {
        let reads = read_counter.clone();
        let prefix = prefix.clone();
        async move {
            if sent {
                futures_util::future::pending::<()>().await;
            }
            reads.fetch_add(1, Ordering::SeqCst);
            Some((Ok::<_, reqwest::Error>(Bytes::from(prefix)), (owner, true)))
        }
    })
    .boxed();
    let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(1024 * 1024));
    let budget = RequestMemoryBudget::with_parent(1024 * 1024, parent.clone());
    let mut normalized = normalize_bedrock_stream(source, "m".to_string(), Some(&budget)).unwrap();
    let task = tokio::spawn(async move { normalized.try_next().await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while reads.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(budget.snapshot().used_bytes > 0);
    task.abort();
    let _ = task.await;
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(budget.snapshot().used_bytes, 0);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[test]
fn bedrock_r5_memory_refusal_precedes_upstream_poll() {
    let budget = RequestMemoryBudget::with_parent(
        1,
        crate::infra::resident_memory::ResidentMemoryBudget::new(Some(1)),
    );
    let source = stream::pending::<Result<Bytes, reqwest::Error>>().boxed();
    match normalize_bedrock_stream(source, "m".to_string(), Some(&budget)) {
        Err(error) => {
            assert!(error.is_request_memory_exhausted());
            assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(_) => panic!("must reject before polling upstream"),
    }
    assert_eq!(budget.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn bedrock_r5_http_errors_and_corrupt_stream_never_replay_or_leak_provider_payload() {
    for (status, streaming) in [
        (401, false),
        (429, false),
        (503, false),
        (200, false),
        (200, true),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let app = axum::Router::new().fallback(move || {
            let seen = seen.clone();
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                if streaming {
                    let mut bytes = super::super::bedrock::tests::text_stream();
                    *bytes.last_mut().unwrap() ^= 1;
                    let chunks: Vec<_> = bytes
                        .chunks(37)
                        .map(|part| Ok::<Bytes, std::io::Error>(Bytes::copy_from_slice(part)))
                        .collect();
                    Response::builder()
                        .header(CONTENT_TYPE, "application/vnd.amazon.eventstream")
                        .body(Body::from_stream(stream::iter(chunks)))
                        .unwrap()
                } else {
                    Response::builder()
                        .status(status)
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(r#"{"message":"secret token user text"}"#))
                        .unwrap()
                }
            }
        });
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("bedrock-r5-failure-{status}-{streaming}"));
        let id = install_bedrock_r5_bearer(&state, &format!("http://{address}")).await;
        let result=forward_for_test_surface(state,ProxyRoute::ClaudeMessages,id,None,HeaderMap::new(),Bytes::from(json!({"model":"m","stream":streaming,"messages":[{"role":"user","content":"hello"}]}).to_string())).await;
        match result {
            Ok(response) => {
                if !streaming {
                    assert_eq!(response.status().as_u16(), status);
                }
                let bytes = collect_response_body(response).await;
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    !text.contains("secret"),
                    "status={status} streaming={streaming} response={text}"
                );
                assert!(text.contains("error"));
                if streaming {
                    let events: Vec<Value> = text
                        .lines()
                        .filter_map(|line| line.strip_prefix("data: "))
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect();
                    let error = events
                        .iter()
                        .position(|event| event["type"] == "error")
                        .expect("corrupt upstream must emit an error");
                    assert_eq!(
                        events[error]["error"]["code"],
                        "upstream_stream_protocol_error"
                    );
                    assert!(!events.iter().any(|event| event["type"] == "message_delta"));
                    // The shared error framing closes after its error event;
                    // that closing frame must not be mistaken for success.
                    for (index, event) in events.iter().enumerate() {
                        if event["type"] == "message_stop" {
                            assert!(index > error);
                        }
                    }
                } else {
                    assert!(!text.contains("event: message_stop\n"));
                }
            }
            Err(error) => {
                assert_eq!(
                    error.status.as_u16(),
                    if status == 200 { 502 } else { status }
                );
                assert!(!error.message.contains("secret"));
            }
        }
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }
}
