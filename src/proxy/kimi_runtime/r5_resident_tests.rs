use crate::infra::resident_memory::ResidentMemoryBudget;

fn scope(suffix: &str) -> KimiThinkingReplayScope {
    KimiThinkingReplayScope::derive(
        "claude", "p", 1, "r", "a", 1, 1, "share", "user", suffix, "k3",
    )
    .unwrap()
}
fn catalog_scope(suffix: &str) -> KimiModelCatalogScope {
    KimiModelCatalogScope::derive("claude", suffix, 1, "r", "a", 1, 1)
}
fn content() -> Bytes {
    Bytes::from_static(br#"[{"type":"thinking","thinking":"x","signature":"sig"},{"type":"tool_use","id":"t","name":"read","input":{}}]"#)
}

#[tokio::test]
async fn r5_owner_regression_kimi_replay_has_a_process_charge() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = KimiThinkingReplayCache::with_parent(parent.clone());
    let scope = scope("first");
    let (_, snapshot) = cache.get(&scope, 1).await;
    assert_eq!(
        cache
            .replace_if_unchanged(scope.clone(), snapshot, content(), 1)
            .await,
        KimiReplayWriteOutcome::Stored
    );
    let bytes = parent.snapshot().used_bytes;
    assert!(bytes > content().len());
    let (loaded, snapshot) = cache.get(&scope, 2).await;
    let loaded = loaded.unwrap();
    let slice = loaded.slice(1..3);
    assert!(cache.delete_if_unchanged(&scope, snapshot).await);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(loaded);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(slice);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_owner_regression_kimi_catalog_has_a_process_charge() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = KimiModelCatalogCache::with_parent(parent.clone());
    let first = cache
        .insert(catalog_scope("first"), vec!["k3".into()], 1)
        .await;
    let held = first.clone();
    let bytes = parent.snapshot().used_bytes;
    assert!(bytes > "k3".len());
    for index in 0..MAX_MODEL_CATALOG_ENTRIES {
        cache
            .insert(
                catalog_scope(&format!("other-{index}")),
                vec!["k3".into()],
                index as i64 + 2,
            )
            .await;
    }
    assert!(cache.fresh(&catalog_scope("first"), 100).await.is_none());
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(first);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_owner_regression_kimi_flight_has_a_process_charge() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = KimiModelCatalogCache::with_parent(parent.clone());
    let guard = cache.lock(&catalog_scope("first")).await.unwrap();
    let bytes = parent.snapshot().used_bytes;
    assert!(bytes > 0);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(guard);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_kimi_refused_catalog_replacement_cannot_revive_stale_authorization() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = KimiModelCatalogCache::with_parent(parent.clone());
    let scope = catalog_scope("first");
    let held = cache.insert(scope.clone(), vec!["k3".into()], 1).await;
    let bytes = parent.snapshot().used_bytes;
    parent.configure_limit(Some(bytes));
    let rejected = cache.insert(scope.clone(), Vec::new(), 2).await;
    assert_eq!(rejected.source, "kimi_models_capacity_rejected");
    assert!(cache.fresh(&scope, 3).await.is_none());
    assert!(cache.stale(&scope, 3).await.is_none());
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_kimi_optional_replay_refusal_keeps_cas_and_expired_bytes_owners() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = KimiThinkingReplayCache::with_parent(parent.clone());
    let scope = scope("first");
    let (_, missing) = cache.get(&scope, 1).await;
    assert_eq!(
        cache
            .replace_if_unchanged(scope.clone(), missing, content(), 1)
            .await,
        KimiReplayWriteOutcome::Stored
    );
    let (held, current) = cache.get(&scope, 2).await;
    let bytes = parent.snapshot().used_bytes;
    parent.configure_limit(Some(bytes));
    assert_eq!(
        cache
            .replace_if_unchanged(scope.clone(), missing, content(), 3)
            .await,
        KimiReplayWriteOutcome::Conflict
    );
    assert_eq!(
        cache
            .replace_if_unchanged(scope.clone(), current, content(), 3)
            .await,
        KimiReplayWriteOutcome::CapacityRejected
    );
    assert!(cache.get(&scope, 4).await.0.is_none());
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
    parent.configure_limit(None);
    let (_, missing) = cache.get(&scope, 5).await;
    assert_eq!(
        cache
            .replace_if_unchanged(scope.clone(), missing, content(), 5)
            .await,
        KimiReplayWriteOutcome::Stored
    );
    let held = cache.get(&scope, 6).await.0.unwrap();
    assert!(cache
        .get(&scope, 5 + KIMI_THINKING_REPLAY_TTL_MS)
        .await
        .0
        .is_none());
    assert!(parent.snapshot().used_bytes > 0);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[test]
fn r5_kimi_stream_replay_refusal_is_optional_and_output_keeps_its_charge() {
    let parent = ResidentMemoryBudget::new(Some(0));
    let mut rejected = KimiThinkingReplayStreamAccumulator::with_parent(parent.clone());
    rejected.push(b"data: {\"type\":\"message_start\"}\n\n");
    assert!(matches!(
        rejected.finish(),
        KimiThinkingReplayStreamOutcome::Incomplete
    ));
    assert!(rejected.pending.is_empty() && rejected.blocks.is_empty());
    assert_eq!(parent.snapshot().used_bytes, 0);
    parent.configure_limit(None);
    let mut accumulator = KimiThinkingReplayStreamAccumulator::with_parent(parent.clone());
    for event in [
        serde_json::json!({"type":"message_start"}),
        serde_json::json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"x","signature":"sig"}}),
        serde_json::json!({"type":"content_block_stop","index":0}),
        serde_json::json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t","name":"read","input":{}}}),
        serde_json::json!({"type":"content_block_stop","index":1}),
        serde_json::json!({"type":"message_stop"}),
    ] {
        accumulator.push(format!("data: {event}\n\n").as_bytes());
    }
    let KimiThinkingReplayStreamOutcome::Replayable(output) = accumulator.finish() else {
        panic!("complete signed turn");
    };
    drop(accumulator);
    assert!(parent.snapshot().used_bytes > output.len());
    let slice = output.slice(1..3);
    drop(output);
    assert!(parent.snapshot().used_bytes > 0);
    drop(slice);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_kimi_fetch_owner_survives_prepublication_await_and_moves_to_snapshot() {
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = seen.clone();
    let router = axum::Router::new().route(
        "/models",
        axum::routing::get(move || {
            let observed = observed.clone();
            async move {
                observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                axum::Json(serde_json::json!({"data":[{"id":"k3"}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/models", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = crate::infra::http::direct_client_builder().build().unwrap();
    let identity = KimiDeviceIdentity {
        device_id: "synthetic".into(),
        device_name: "fixture".into(),
        device_model: "fixture".into(),
        os_version: "fixture".into(),
    };
    let parent = ResidentMemoryBudget::new(Some(0));
    let rejected = fetch_kimi_models_with_parent(
        &client,
        &url,
        "synthetic-token",
        &identity,
        Duration::from_secs(2),
        parent.clone(),
    )
    .await
    .unwrap_err();
    assert!(rejected.capacity_rejected && !rejected.retryable());
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 0);
    parent.configure_limit(Some(MAX_MODEL_RESPONSE_BODY_BYTES + 1024));
    let rejected = fetch_kimi_models_with_parent(
        &client,
        &url,
        "synthetic-token",
        &identity,
        Duration::from_secs(2),
        parent.clone(),
    )
    .await
    .unwrap_err();
    assert!(rejected.capacity_rejected && !rejected.retryable());
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(parent.snapshot().used_bytes, 0);
    parent.configure_limit(None);
    let fetched = fetch_kimi_models_with_parent(
        &client,
        &url,
        "synthetic-token",
        &identity,
        Duration::from_secs(2),
        parent.clone(),
    )
    .await
    .unwrap();
    let before = parent.snapshot().used_bytes;
    assert!(before > 0);
    tokio::task::yield_now().await;
    assert_eq!(parent.snapshot().used_bytes, before);
    let cache = KimiModelCatalogCache::with_parent(parent.clone());
    let snapshot = cache
        .insert_fetched(catalog_scope("first"), fetched, 1)
        .await;
    assert_eq!(snapshot.models, ["k3"]);
    drop(cache);
    assert!(parent.snapshot().used_bytes >= before);
    drop(snapshot);
    assert_eq!(parent.snapshot().used_bytes, 0);
    server.abort();
}
