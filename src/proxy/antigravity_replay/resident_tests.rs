use super::*;

fn scope(index: usize) -> AntigravityReplayScope {
    AntigravityReplayScope(format!("scope-{index}"))
}

fn chain() -> AntigravityReplayChain {
    AntigravityReplayChain {
        turns: vec![ReplayTurn {
            base_history_hash: "history".into(),
            items: vec![ReplayItem::FunctionCall {
                id: Some("call-1".into()),
                name: "lookup".into(),
                args: json!({"q":"\u{0000}\"\\中"}),
                thought_signature: Some("private-signature".into()),
                occurrence: 0,
            }],
        }],
    }
}

#[tokio::test]
async fn antigravity_r4_review_unadmitted_miss_cannot_publish_after_capacity_returns() {
    let parent = ResidentMemoryBudget::new(Some(0));
    let cache = AntigravityReplayCache::with_parent(parent.clone());
    let (first, first_snapshot) = cache.get(&scope(0), 1).await;
    let (_, second_snapshot) = cache.get(&scope(0), 2).await;
    assert!(first.is_none());
    assert_ne!(first_snapshot.generation(), second_snapshot.generation());
    assert!(cache.store.lock().await.entries.is_empty());
    parent.configure_limit(None);
    assert!(
        !cache
            .replace_if_unchanged(scope(0), first_snapshot, chain(), 3)
            .await
    );
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn antigravity_r4_review_request_copy_is_admitted_and_outlives_cache_entry() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = AntigravityReplayCache::with_parent(parent.clone());
    let (_, miss) = cache.get(&scope(0), 1).await;
    assert!(cache.replace_if_unchanged(scope(0), miss, chain(), 2).await);
    let retained = parent.snapshot().used_bytes;
    let rejected = RequestMemoryBudget::with_parent(1, parent.clone());
    assert!(cache
        .get_accounted(&scope(0), 3, Some(&rejected))
        .await
        .is_err());
    assert_eq!(parent.snapshot().used_bytes, retained);
    assert_eq!(rejected.snapshot().used_bytes, 0);
    assert!(rejected.is_exhausted());
    let request = RequestMemoryBudget::with_parent(usize::MAX, parent.clone());
    let (copy, snapshot, memory) = cache
        .get_accounted(&scope(0), 4, Some(&request))
        .await
        .unwrap();
    assert_eq!(
        snapshot.retained_bytes(),
        copy.as_ref().unwrap().retained_bytes()
    );
    assert!(parent.snapshot().used_bytes > retained);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, snapshot.retained_bytes());
    assert!(copy.is_some());
    drop(copy);
    drop(memory);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn antigravity_r4_review_replacement_refusal_removes_old_proof_and_fences_late_writer() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = AntigravityReplayCache::with_parent(parent.clone());
    let (_, miss) = cache.get(&scope(0), 1).await;
    assert!(cache.replace_if_unchanged(scope(0), miss, chain(), 2).await);
    let (_, old) = cache.get(&scope(0), 3).await;
    parent.configure_limit(Some(0));
    assert!(!cache.replace_if_unchanged(scope(0), old, chain(), 4).await);
    assert!(cache.get(&scope(0), 5).await.0.is_none());
    parent.configure_limit(None);
    let (_, fresh) = cache.get(&scope(0), 6).await;
    assert_ne!(old.generation(), fresh.generation());
    assert!(!cache.replace_if_unchanged(scope(0), old, chain(), 7).await);
    assert!(!cache.delete_if_unchanged(&scope(0), old, 7).await);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[tokio::test]
async fn antigravity_r4_review_tombstone_count_expiry_and_drop_release_owners() {
    let parent = ResidentMemoryBudget::new(None);
    let cache = AntigravityReplayCache::with_parent(parent.clone());
    for index in 0..=MAX_ENTRIES {
        cache.get(&scope(index), index as i64).await;
    }
    assert_eq!(cache.store.lock().await.entries.len(), MAX_ENTRIES);
    assert!(!cache.store.lock().await.entries.contains_key(&scope(0)));
    assert_eq!(parent.snapshot().active_reservations, MAX_ENTRIES);
    cache
        .get(&scope(MAX_ENTRIES + 1), REPLAY_TTL_MS + MAX_ENTRIES as i64)
        .await;
    assert_eq!(parent.snapshot().active_reservations, 1);
    drop(cache);
    assert_eq!(parent.snapshot().used_bytes, 0);
}

#[test]
fn antigravity_r4_review_wire_limit_counts_escapes_while_resident_counts_capacity() {
    let mut value = chain();
    let expected = serde_json::to_vec(&value).unwrap().len();
    assert_eq!(valid_chain_bytes(&value), Some(expected));
    assert!(serialized_json_bytes(&value, expected - 1).is_none());
    let before = value.retained_bytes();
    value.turns[0].base_history_hash.reserve(8 * 1024);
    assert!(value.retained_bytes() >= before + 8 * 1024);
    assert_eq!(valid_chain_bytes(&value), Some(expected));
}
