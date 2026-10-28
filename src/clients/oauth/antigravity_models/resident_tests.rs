use super::*;

fn key() -> CacheKey {
    CacheKey {
        provider_type: "antigravity_oauth".into(),
        account_id: "fixture".into(),
        account_incarnation: 1,
        auth_identity_generation: 1,
        token_refresh_generation: 1,
        request_identity_fingerprint: [1; 32],
        project_id: "project".into(),
        source_url: "https://example.test/models".into(),
        client_identity: antigravity_client_identity(),
    }
}

fn catalog(revision: u64, empty: bool) -> CachedCatalog {
    CachedCatalog {
        descriptors: Arc::new(if empty {
            Vec::new()
        } else {
            parse_descriptors(&json!({"models":{"gemini-3-flash":{"displayName":"fixture"}}}))
                .unwrap()
        }),
        source_url: "https://example.test/models".into(),
        fetched_at_ms: revision as i64,
        request_revision: revision,
        _memory: None,
    }
}

#[test]
fn antigravity_r4_review_catalog_snapshot_shares_payload_and_last_owner() {
    let parent = ResidentMemoryBudget::new(None);
    let mut cache = CatalogCache::default();
    let mut current = catalog(1, false);
    assert!(publish_catalog_with_parent(
        &mut cache,
        key(),
        &mut current,
        &parent
    ));
    let snapshot = cache.get(&key()).unwrap().clone();
    assert!(Arc::ptr_eq(&snapshot.descriptors, &current.descriptors));
    let used = parent.snapshot().used_bytes;
    assert!(used > 0);
    drop(cache);
    drop(current);
    assert_eq!(parent.snapshot().used_bytes, used);
    drop(snapshot);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[test]
fn antigravity_r4_review_catalog_refusal_is_uncached_current_and_fences_older_republication() {
    for empty in [false, true] {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = CatalogCache::default();
        assert!(publish_catalog_with_parent(
            &mut cache,
            key(),
            &mut catalog(1, false),
            &parent
        ));
        parent.configure_limit(Some(0));
        let mut current = catalog(3, empty);
        assert!(publish_catalog_with_parent(
            &mut cache,
            key(),
            &mut current,
            &parent
        ));
        assert!(cache.is_empty());
        assert!(current._memory.is_none());
        assert_eq!(current.descriptors.is_empty(), empty);
        assert_eq!(parent.snapshot().used_bytes, 0);
        parent.configure_limit(None);
        // The older fetch can finish, but cannot reinstall old authorization.
        assert!(publish_catalog_with_parent(
            &mut cache,
            key(),
            &mut catalog(2, false),
            &parent
        ));
        assert!(cache.is_empty());
        assert!(publish_catalog_with_parent(
            &mut cache,
            key(),
            &mut catalog(4, empty),
            &parent
        ));
        assert_eq!(cache.get(&key()).unwrap().request_revision, 4);
    }
}

#[test]
fn antigravity_r4_review_catalog_revision_supersession_is_distinct_from_capacity_refusal() {
    let parent = ResidentMemoryBudget::new(None);
    let mut cache = CatalogCache::default();
    let new = CacheKey {
        token_refresh_generation: 2,
        ..key()
    };
    assert!(publish_catalog_with_parent(
        &mut cache,
        new.clone(),
        &mut catalog(3, true),
        &parent
    ));
    let before = parent.snapshot().used_bytes;
    parent.configure_limit(Some(0));
    assert!(!publish_catalog_with_parent(
        &mut cache,
        key(),
        &mut catalog(4, false),
        &parent
    ));
    assert!(!publish_catalog_with_parent(
        &mut cache,
        new.clone(),
        &mut catalog(2, false),
        &parent
    ));
    assert_eq!(parent.snapshot().used_bytes, before);
    assert!(cache.get(&new).unwrap().descriptors.is_empty());
    prune_catalogs(&mut cache, MAX_STALE_CATALOG_AGE_MS + 4);
    assert!(cache.is_empty());
    assert_eq!(parent.snapshot().used_bytes, 0);
}
