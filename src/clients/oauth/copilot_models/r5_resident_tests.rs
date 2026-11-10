use crate::infra::resident_memory::ResidentMemoryBudget;

fn key(suffix: &str) -> CopilotModelCatalogKey {
    CopilotModelCatalogKey {
        account_id: suffix.into(),
        auth_identity_generation: 1,
        account_token_refresh_generation: 1,
        copilot_token_generation: 1,
        github_domain: "github.com".into(),
        api_origin: "https://api.githubcopilot.com".into(),
    }
}
fn descriptors() -> Vec<CopilotModelDescriptor> {
    parse_copilot_models(
        &serde_json::json!({"data":[{"id":"synthetic","supported_endpoints":["/v1/messages"]}]}),
    )
}

#[tokio::test]
async fn r5_owner_regression_copilot_catalog_has_a_process_charge() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = CopilotModelCatalogCache::with_parent(parent.clone());
    let first = cache.insert(key("first"), descriptors(), 1).await.unwrap();
    let held = first.clone();
    let bytes = parent.snapshot().used_bytes;
    assert!(bytes > 0);
    for index in 0..MAX_MODEL_CACHE_ENTRIES {
        cache
            .insert(
                key(&format!("other-{index}")),
                descriptors(),
                index as i64 + 2,
            )
            .await
            .unwrap();
    }
    assert!(cache.fresh(&key("first"), 100).await.is_none());
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(first);
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_copilot_refused_authoritative_empty_replacement_removes_old_authorization() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = CopilotModelCatalogCache::with_parent(parent.clone());
    let held = cache.insert(key("first"), descriptors(), 1).await.unwrap();
    let bytes = parent.snapshot().used_bytes;
    parent.configure_limit(Some(bytes));
    assert!(cache.insert(key("first"), Vec::new(), 2).await.is_none());
    assert!(cache.fresh(&key("first"), 3).await.is_none());
    assert!(cache.stale(&key("first"), 3).await.is_none());
    assert_eq!(parent.snapshot().used_bytes, bytes);
    drop(held);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r5_copilot_fetch_owner_survives_prepublication_await_and_moves_to_snapshot() {
    let seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = seen.clone();
    let router = axum::Router::new().route("/models", axum::routing::get(move || {
        let observed = observed.clone();
        async move {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            axum::Json(serde_json::json!({"data":[{"id":"synthetic","supported_endpoints":["/v1/messages"]}]}))
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/models", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = crate::infra::http::direct_client_builder().build().unwrap();
    let parent = ResidentMemoryBudget::new(Some(0));
    let rejected = fetch_copilot_models_with_parent(
        &client,
        &url,
        "synthetic-token",
        Duration::from_secs(2),
        parent.clone(),
    )
    .await
    .unwrap_err();
    assert!(rejected.capacity_rejected && !rejected.is_transient());
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(parent.snapshot().used_bytes, 0);
    parent.configure_limit(Some(MAX_MODEL_RESPONSE_BODY_BYTES + 1024));
    let rejected = fetch_copilot_models_with_parent(
        &client,
        &url,
        "synthetic-token",
        Duration::from_secs(2),
        parent.clone(),
    )
    .await
    .unwrap_err();
    assert!(rejected.capacity_rejected && !rejected.is_transient());
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(parent.snapshot().used_bytes, 0);
    parent.configure_limit(None);
    let fetched = fetch_copilot_models_with_parent(
        &client,
        &url,
        "synthetic-token",
        Duration::from_secs(2),
        parent.clone(),
    )
    .await
    .unwrap();
    let before = parent.snapshot().used_bytes;
    assert!(before > 0);
    tokio::task::yield_now().await;
    assert_eq!(parent.snapshot().used_bytes, before);
    let cache = CopilotModelCatalogCache::with_parent(parent.clone());
    let snapshot = cache
        .insert_fetched(key("first"), fetched, 1)
        .await
        .unwrap();
    assert_eq!(snapshot.models, ["synthetic"]);
    drop(cache);
    assert!(parent.snapshot().used_bytes >= before);
    drop(snapshot);
    assert_eq!(parent.snapshot().used_bytes, 0);
    server.abort();
}
