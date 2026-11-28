//! Independent local HTTP fixtures for production discovery, without account credentials.
use super::*;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tests::account;

fn page(model: &str) -> Value {
    json!({"models":[{"modelId":model}],"defaultModel":{"modelId":model}})
}

async fn fixture(
    responses: Vec<(u16, Value, u64)>,
) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let queue = Arc::new(StdMutex::new(VecDeque::from(responses)));
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |body: bytes::Bytes| {
        let queue = queue.clone();
        let calls = calls.clone();
        async move {
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["origin"], "CLI");
            calls.fetch_add(1, Ordering::SeqCst);
            let (status, body, delay) = queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected discovery request");
            tokio::time::sleep(Duration::from_millis(delay)).await;
            (
                axum::http::StatusCode::from_u16(status).unwrap(),
                axum::Json(body),
            )
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, count, server)
}

async fn query(cache: &resident::RuntimeCache, endpoint: &str) -> AmazonQModelCatalog {
    model_catalog_cached(
        cache,
        &crate::infra::http::direct_client().unwrap(),
        &account(ProviderType::AmazonQOAuth, 1, "q-r7-runtime"),
        &AmazonQModelCatalogScope::fixture(),
        Some(endpoint),
        Duration::from_secs(2),
    )
    .await
}

#[tokio::test]
async fn r7_amazon_q_failed_flight_is_shared_but_later_request_retries() {
    let (endpoint, calls, server) =
        fixture(vec![(500, json!({}), 100), (200, page("recovered"), 0)]).await;
    let parent = ResidentMemoryBudget::new(None);
    let cache = Arc::new(resident::RuntimeCache::with_parent(parent.clone()));
    let barrier = Arc::new(tokio::sync::Barrier::new(9));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let (cache, endpoint, barrier) = (cache.clone(), endpoint.clone(), barrier.clone());
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            query(&cache, &endpoint).await
        }));
    }
    barrier.wait().await;
    for task in tasks {
        let result = task.await.unwrap();
        assert!(result.descriptors.is_empty());
        assert_eq!(
            result.source,
            "amazon_q_model_discovery_transient_without_cache"
        );
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let retry = query(&cache, &endpoint).await;
    assert!(retry.model("recovered").is_some());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
    drop(retry);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r7_amazon_q_transient_stale_is_exact_scope_and_denial_revokes_it() {
    for denial in [401, 403, 200] {
        let (endpoint, calls, server) = fixture(vec![
            (200, page("stale-allowed"), 0),
            (429, json!({}), 0),
            (500, json!({}), 0),
            (denial, json!({"models":"invalid"}), 0),
            (500, json!({}), 0),
        ])
        .await;
        let cache = resident::RuntimeCache::with_parent(ResidentMemoryBudget::new(None));
        assert!(query(&cache, &endpoint)
            .await
            .model("stale-allowed")
            .is_some());
        cache.age_for_test(CACHE_TTL_MS + 10).await;
        let stale = query(&cache, &endpoint).await;
        assert!(stale.stale);
        assert!(stale.model("stale-allowed").is_some());
        let mut other_scope = AmazonQModelCatalogScope::fixture();
        other_scope.provider_revision += 1;
        let wrong = model_catalog_cached(
            &cache,
            &crate::infra::http::direct_client().unwrap(),
            &account(ProviderType::AmazonQOAuth, 1, "q-r7-runtime"),
            &other_scope,
            Some(&endpoint),
            Duration::from_secs(2),
        )
        .await;
        assert!(wrong.descriptors.is_empty());
        assert!(!wrong.stale);
        let denied = query(&cache, &endpoint).await;
        assert!(!denied.stale);
        assert!(denied.descriptors.is_empty());
        assert_eq!(denied.is_unauthorized(), denial == 401 || denial == 403);
        let later_failure = query(&cache, &endpoint).await;
        assert!(!later_failure.stale);
        assert!(
            later_failure.descriptors.is_empty(),
            "denied/malformed stale catalog must stay revoked"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        server.abort();
    }
}

#[tokio::test]
async fn r7_amazon_q_revocation_blocks_old_flight_and_incarnation_isolates_new_one() {
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let count = Arc::new(AtomicUsize::new(0));
    let (seen, ready, resume) = (count.clone(), started.clone(), release.clone());
    let app = axum::Router::new().fallback(axum::routing::post(move || {
        let (seen, ready, resume) = (seen.clone(), ready.clone(), resume.clone());
        async move {
            let n = seen.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                ready.notify_one();
                resume.notified().await;
            }
            axum::Json(page(if n == 0 {
                "old-incarnation"
            } else {
                "new-incarnation"
            }))
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let parent = ResidentMemoryBudget::new(None);
    let cache = Arc::new(resident::RuntimeCache::with_parent(parent.clone()));
    let (task_cache, task_endpoint) = (cache.clone(), endpoint.clone());
    let old = tokio::spawn(async move { query(&task_cache, &task_endpoint).await });
    tokio::time::timeout(Duration::from_secs(2), started.notified())
        .await
        .unwrap();
    cache.clear_account("q-r7-runtime").await;
    let mut scope = AmazonQModelCatalogScope::fixture();
    scope.account_incarnation += 1;
    let new = model_catalog_cached(
        &cache,
        &crate::infra::http::direct_client().unwrap(),
        &account(ProviderType::AmazonQOAuth, 1, "q-r7-runtime"),
        &scope,
        Some(&endpoint),
        Duration::from_secs(2),
    )
    .await;
    assert!(new.model("new-incarnation").is_some());
    release.notify_one();
    let obsolete = old.await.unwrap();
    assert_eq!(obsolete.source, "amazon_q_catalog_invalidated");
    assert!(obsolete.descriptors.is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 2);
    server.abort();
    drop(new);
    drop(obsolete);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r7_amazon_q_pagination_uses_one_deadline() {
    let mut first = page("deadline-model");
    first["nextToken"] = json!("second");
    let (endpoint, calls, server) =
        fixture(vec![(200, first, 250), (200, page("deadline-model"), 250)]).await;
    let parent = ResidentMemoryBudget::new(None);
    let cache = resident::RuntimeCache::with_parent(parent.clone());
    let result = model_catalog_cached(
        &cache,
        &crate::infra::http::direct_client().unwrap(),
        &account(ProviderType::AmazonQOAuth, 1, "q-r7-runtime"),
        &AmazonQModelCatalogScope::fixture(),
        Some(&endpoint),
        Duration::from_millis(400),
    )
    .await;
    assert!(result.descriptors.is_empty());
    assert!(!result.is_capacity_rejected());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn r7_amazon_q_authoritative_empty_and_discovery_bounds() {
    let mut repeat = page("repeat");
    repeat["nextToken"] = json!("same");
    let mut overflowing = page("model-0");
    overflowing["models"] = json!((0..=MAX_MODELS)
        .map(|n| json!({"modelId":format!("model-{n}")}))
        .collect::<Vec<_>>());
    let cases = vec![
        (vec![(200, json!({"models":[]}), 0)], true, 1),
        (
            vec![(200, json!({"models":[{"modelId":"missing-default"}]}), 0)],
            false,
            1,
        ),
        (
            vec![(
                200,
                json!({"models":[],"defaultModel":{"modelId":"missing"}}),
                0,
            )],
            false,
            1,
        ),
        (vec![(200, repeat.clone(), 0), (200, repeat, 0)], false, 2),
        (
            (0..MAX_PAGES)
                .map(|n| {
                    let mut p = page("many-pages");
                    p["nextToken"] = json!(format!("page-{n}"));
                    (200, p, 0)
                })
                .collect(),
            false,
            MAX_PAGES,
        ),
        (vec![(200, overflowing, 0)], false, 1),
        (
            vec![(
                200,
                json!({"models":[],"extra":"x".repeat(MAX_RESPONSE_BYTES)}),
                0,
            )],
            false,
            1,
        ),
    ];
    for (responses, valid, expected_calls) in cases {
        let (endpoint, calls, server) = fixture(responses).await;
        let parent = ResidentMemoryBudget::new(None);
        let cache = resident::RuntimeCache::with_parent(parent.clone());
        let result = query(&cache, &endpoint).await;
        assert!(result.descriptors.is_empty());
        assert_eq!(result.source == "amazon_q_list_available_models", valid);
        assert_eq!(calls.load(Ordering::SeqCst), expected_calls);
        assert!(!result.is_capacity_rejected());
        server.abort();
        drop(result);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}

#[tokio::test]
async fn r7_amazon_q_local_capacity_is_distinct_from_auth_and_sends_nothing() {
    let (endpoint, calls, server) = fixture(vec![]).await;
    let parent = ResidentMemoryBudget::new(Some(0));
    let cache = resident::RuntimeCache::with_parent(parent.clone());
    let result = query(&cache, &endpoint).await;
    assert!(result.is_capacity_rejected());
    assert!(!result.is_unauthorized());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(parent.snapshot().used_bytes, 0);
    server.abort();
}

#[tokio::test]
async fn r7_amazon_q_same_generation_credentials_have_independent_catalogs() {
    // The key must remain correct even when an account invalidation callback
    // has not run yet. Generation counters alone do not identify wire auth.
    let (endpoint, calls, server) = fixture(vec![
        (200, page("old-credential-model"), 0),
        (200, page("new-credential-model"), 0),
    ])
    .await;
    let parent = ResidentMemoryBudget::new(None);
    let cache = resident::RuntimeCache::with_parent(parent.clone());
    let http = crate::infra::http::direct_client().unwrap();
    let first = account(ProviderType::AmazonQOAuth, 1, "q-r7-same-generation");
    let mut second = first.clone();
    second.access_token = Some("a-different-wire-credential".into());
    let scope = AmazonQModelCatalogScope::fixture();
    let old = model_catalog_cached(
        &cache,
        &http,
        &first,
        &scope,
        Some(&endpoint),
        Duration::from_secs(2),
    )
    .await;
    let new = model_catalog_cached(
        &cache,
        &http,
        &second,
        &scope,
        Some(&endpoint),
        Duration::from_secs(2),
    )
    .await;
    assert!(old.model("old-credential-model").is_some());
    assert!(new.model("new-credential-model").is_some());
    assert!(new.model("old-credential-model").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // Returning to the old credential may use only its own immutable snapshot.
    let old_again = model_catalog_cached(
        &cache,
        &http,
        &first,
        &scope,
        Some(&endpoint),
        Duration::from_secs(2),
    )
    .await;
    assert!(Arc::ptr_eq(&old.data, &old_again.data));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    server.abort();
    drop(old);
    drop(new);
    drop(old_again);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, 0);
}
