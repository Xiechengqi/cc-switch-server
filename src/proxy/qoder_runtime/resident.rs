//! Logical owners for sparse runtime indexes and immutable shared payloads.
//! Opaque HTTP/crypto allocations and control-plane parsing remain separate.
use super::*;
use crate::infra::resident_memory::process_resident_memory;
use std::sync::OnceLock;

/// The owner lives inside the Arc target so even a caller retaining only the
/// public session Arc cannot release its payload charge ahead of the data.
pub struct QoderResidentSession {
    value: QoderCosySession,
    memory: OnceLock<ResidentMemoryReservation>,
}

impl QoderResidentSession {
    pub(super) fn new(value: QoderCosySession) -> Self {
        Self {
            value,
            memory: OnceLock::new(),
        }
    }

    fn retained_bytes(&self) -> usize {
        let value = &self.value;
        [
            &value.client_version,
            &value.cosy_key,
            &value.encrypted_info,
            &value.identity.name,
            &value.identity.aid,
            &value.identity.uid,
            &value.identity.organization_id,
            &value.identity.organization_name,
            &value.identity.user_type,
            &value.identity.security_oauth_token,
            &value.identity.refresh_token,
            &value.machine.machine_id,
            &value.machine.machine_token,
            &value.machine.machine_type,
        ]
        .iter()
        .fold(
            std::mem::size_of::<Self>() + std::mem::size_of::<usize>() * 2,
            |bytes, text| bytes.saturating_add(text.capacity()),
        )
    }

    pub(super) fn unaccounted_bytes(&self) -> usize {
        if self.memory.get().is_some() {
            0
        } else {
            self.retained_bytes()
        }
    }
}

impl std::ops::Deref for QoderResidentSession {
    type Target = QoderCosySession;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

pub(super) struct ResidentSessionEntry {
    value: QoderCachedSession,
    _memory: ResidentMemoryReservation,
}

#[derive(Debug)]
pub(super) struct QoderResidentCatalog {
    pub(super) value: FetchedQoderModelCatalog,
    pub(super) _memory: Option<ResidentMemoryReservation>,
}

pub(super) type RuntimeFlights = BTreeMap<QoderRuntimeScope, Arc<Mutex<ResidentMemoryReservation>>>;

fn node_bytes<K, V>(dynamic: usize) -> usize {
    (std::mem::size_of::<(K, V)>() + std::mem::size_of::<usize>() * 2)
        .saturating_mul(12)
        .saturating_add(dynamic)
}

impl Default for QoderRuntimeCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl QoderRuntimeCache {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            sessions: RwLock::new(BTreeMap::new()),
            session_flights: Mutex::new(BTreeMap::new()),
            catalogs: RwLock::new(BTreeMap::new()),
            catalog_flights: Mutex::new(BTreeMap::new()),
            response_aliases: RwLock::new(BTreeMap::new()),
            parent,
        }
    }

    pub async fn session(
        &self,
        scope: &QoderRuntimeScope,
        now_ms: i64,
    ) -> Option<QoderCachedSession> {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, entry| entry.value.is_fresh(now_ms));
        sessions.get(scope).map(|entry| entry.value.clone())
    }

    pub(crate) async fn insert_session(
        &self,
        scope: QoderRuntimeScope,
        session: QoderCachedSession,
    ) -> Result<(), ()> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(&scope);
        sessions.retain(|_, entry| entry.value.is_fresh(session.cached_at_ms));
        if session.session.memory.get().is_none() {
            let memory = self
                .parent
                .reserve(session.session.retained_bytes())
                .ok_or(())?;
            // All clones, including pre-insertion clones, observe the same
            // one-time owner. A racing install cannot double-charge it.
            let _ = session.session.memory.set(memory);
        }
        let dynamic = scope
            .0
            .capacity()
            .saturating_add(session.gateway_base_url.capacity());
        if let Some(memory) = self
            .parent
            .reserve(node_bytes::<QoderRuntimeScope, ResidentSessionEntry>(
                dynamic,
            ))
        {
            sessions.insert(
                scope,
                ResidentSessionEntry {
                    value: session,
                    _memory: memory,
                },
            );
            while sessions.len() > MAX_RUNTIME_ENTRIES {
                let oldest = sessions
                    .iter()
                    .min_by_key(|(key, entry)| (entry.value.cached_at_ms, *key))
                    .map(|(key, _)| key.clone())
                    .expect("nonempty session cache");
                sessions.remove(&oldest);
            }
        }
        // Optional index refusal keeps the current admitted session usable.
        Ok(())
    }

    pub async fn invalidate_session(&self, scope: &QoderRuntimeScope) {
        self.sessions.write().await.remove(scope);
    }

    pub async fn catalog(
        &self,
        scope: &QoderRuntimeScope,
        now_ms: i64,
    ) -> Option<QoderModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        catalogs.retain(|_, catalog| catalog.is_fresh(now_ms));
        catalogs.get(scope).cloned()
    }

    pub async fn insert_catalog(
        &self,
        scope: QoderRuntimeScope,
        fetched: FetchedQoderModelCatalog,
        fetched_at_ms: i64,
    ) -> QoderModelCatalog {
        let mut catalog = QoderModelCatalog {
            data: Arc::new(QoderResidentCatalog {
                value: fetched,
                _memory: None,
            }),
            fetched_at_ms,
            expires_at_ms: fetched_at_ms.saturating_add(QODER_MODEL_CATALOG_TTL_MS),
        };
        let mut catalogs = self.catalogs.write().await;
        catalogs.remove(&scope);
        catalogs.retain(|_, catalog| catalog.is_fresh(fetched_at_ms));
        let dynamic = catalog
            .retained_bytes()
            .saturating_add(scope.0.capacity())
            .saturating_add(
                std::mem::size_of::<QoderResidentCatalog>() + std::mem::size_of::<usize>() * 2,
            )
            .saturating_add(
                catalog
                    .raw_configs
                    .len()
                    .saturating_mul(node_bytes::<String, Value>(0)),
            )
            .saturating_add(
                catalog
                    .capabilities
                    .len()
                    .saturating_mul(node_bytes::<String, QoderModelCapability>(0)),
            );
        if let Some(memory) = self
            .parent
            .reserve(node_bytes::<QoderRuntimeScope, QoderModelCatalog>(dynamic))
        {
            Arc::get_mut(&mut catalog.data)
                .expect("unpublished catalog has one owner")
                ._memory = Some(memory);
            catalogs.insert(scope, catalog.clone());
            while catalogs.len() > MAX_RUNTIME_ENTRIES {
                let oldest = catalogs
                    .iter()
                    .min_by_key(|(key, value)| (value.fetched_at_ms, *key))
                    .map(|(key, _)| key.clone())
                    .expect("nonempty catalog cache");
                catalogs.remove(&oldest);
            }
        }
        catalog
    }

    pub async fn invalidate_catalog(&self, scope: &QoderRuntimeScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub async fn invalidate_scope(&self, scope: &QoderRuntimeScope) {
        self.invalidate_session(scope).await;
        self.invalidate_catalog(scope).await;
        self.response_aliases
            .write()
            .await
            .retain(|_, alias| &alias.scope != scope);
    }

    pub(crate) async fn session_lock(
        &self,
        scope: &QoderRuntimeScope,
    ) -> Option<OwnedMutexGuard<ResidentMemoryReservation>> {
        self.flight_lock(&self.session_flights, scope).await
    }

    pub(crate) async fn catalog_lock(
        &self,
        scope: &QoderRuntimeScope,
    ) -> Option<OwnedMutexGuard<ResidentMemoryReservation>> {
        self.flight_lock(&self.catalog_flights, scope).await
    }

    async fn flight_lock(
        &self,
        flights: &Mutex<RuntimeFlights>,
        scope: &QoderRuntimeScope,
    ) -> Option<OwnedMutexGuard<ResidentMemoryReservation>> {
        let flight = {
            let mut flights = flights.lock().await;
            flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
            if let Some(flight) = flights.get(scope) {
                flight.clone()
            } else {
                if flights.len() >= MAX_RUNTIME_ENTRIES {
                    return None;
                }
                let bytes = node_bytes::<QoderRuntimeScope, Arc<Mutex<ResidentMemoryReservation>>>(
                    scope.0.capacity().saturating_add(
                        std::mem::size_of::<Mutex<ResidentMemoryReservation>>()
                            + std::mem::size_of::<usize>() * 2,
                    ),
                );
                let memory = self.parent.reserve(bytes)?;
                let flight = Arc::new(Mutex::new(memory));
                flights.insert(scope.clone(), flight.clone());
                flight
            }
        };
        Some(flight.lock_owned().await)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn response_continuation(
        &self,
        scope: &QoderRuntimeScope,
        share_id: &str,
        user_namespace: &str,
        model_key: &str,
        response_id: &str,
        now_ms: i64,
    ) -> Option<Arc<QoderResponseAlias>> {
        let key = response_alias_key(scope, share_id, user_namespace, model_key, response_id)?;
        let mut aliases = self.response_aliases.write().await;
        aliases.retain(|_, alias| alias.expires_at_ms > now_ms);
        aliases.get(&key).cloned()
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn claim_response_continuation(
        &self,
        scope: &QoderRuntimeScope,
        share_id: &str,
        user_namespace: &str,
        model_key: &str,
        response_id: &str,
        now_ms: i64,
    ) -> Option<Arc<QoderResponseAlias>> {
        let key = response_alias_key(scope, share_id, user_namespace, model_key, response_id)?;
        let mut aliases = self.response_aliases.write().await;
        aliases.retain(|_, alias| alias.expires_at_ms > now_ms);
        aliases.remove(&key)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_response_alias(
        &self,
        scope: QoderRuntimeScope,
        share_id: &str,
        user_namespace: &str,
        model_key: &str,
        response_id: &str,
        continuation: QoderResponseContinuation,
        now_ms: i64,
    ) -> Result<(), String> {
        let key = response_alias_key(&scope, share_id, user_namespace, model_key, response_id)
            .ok_or_else(|| "Qoder response alias identity is invalid".to_string())?;
        if continuation.session_id.trim().is_empty()
            || continuation.session_id.len() > 128
            || continuation.downstream_model.is_empty()
            || continuation.downstream_model.len() > 256
            || continuation.tool_names.len() > 128
            || continuation.pending_tool_calls.len() > 128
            || continuation
                .tool_names
                .iter()
                .any(|name| name.is_empty() || name.trim() != name || name.len() > 256)
        {
            return Err("Qoder response continuation exceeds identity limits".into());
        }
        let payload_bytes = QoderResponseAlias::payload_bytes(&scope, &continuation);
        if payload_bytes > MAX_RESPONSE_ALIAS_ENTRY_BYTES {
            return Err("Qoder response continuation exceeds entry limit".into());
        }
        let mut aliases = self.response_aliases.write().await;
        aliases.retain(|_, value| value.expires_at_ms > now_ms);
        if aliases.contains_key(&key) {
            return Err("Qoder response identifier is already bound".into());
        }
        let bytes = node_bytes::<String, Arc<QoderResponseAlias>>(
            payload_bytes
                .saturating_add(key.capacity())
                .saturating_add(std::mem::size_of::<usize>() * 2),
        );
        let Some(memory) = self.parent.reserve(bytes) else {
            // Optional continuation storage cannot turn a completed ordinary
            // response into a provider failure. A later required lookup misses.
            return Ok(());
        };
        let mut retained = aliases
            .values()
            .map(|alias| alias.retained_bytes())
            .sum::<usize>();
        while aliases.len() >= MAX_RESPONSE_ALIASES
            || retained.saturating_add(payload_bytes) > MAX_RESPONSE_ALIAS_BYTES
        {
            let oldest = aliases
                .iter()
                .min_by_key(|(key, value)| (value.inserted_at_ms, *key))
                .map(|(key, _)| key.clone())
                .expect("nonempty alias cache");
            if let Some(old) = aliases.remove(&oldest) {
                retained = retained.saturating_sub(old.retained_bytes());
            }
        }
        aliases.insert(
            key,
            Arc::new(QoderResponseAlias {
                scope,
                continuation,
                inserted_at_ms: now_ms,
                expires_at_ms: now_ms.saturating_add(RESPONSE_ALIAS_TTL_MS),
                _memory: memory,
            }),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::qoder::{QoderIdentity, QoderMachineIdentity};

    fn scope(index: usize) -> QoderRuntimeScope {
        QoderRuntimeScope(format!("fixture-{index}"))
    }

    fn session() -> QoderCachedSession {
        QoderCachedSession::new(
            QoderCosySession::new_with_key(
                QoderSite::Global,
                QoderIdentity {
                    name: String::new(),
                    aid: "aid".into(),
                    uid: "uid".into(),
                    organization_id: String::new(),
                    organization_name: String::new(),
                    user_type: "personal".into(),
                    security_oauth_token: "private-bearer".repeat(128),
                    refresh_token: "private-refresh".repeat(128),
                },
                QoderMachineIdentity {
                    machine_id: "0123456789abcdef0123456789abcdef0123".into(),
                    machine_token: "fixture-token".into(),
                    machine_type: "fixture".into(),
                },
                b"0123456789abcdef",
            )
            .unwrap(),
            "https://example.test".into(),
            Some(10_000_000),
            1,
        )
        .unwrap()
    }

    fn fetched(model: &str) -> FetchedQoderModelCatalog {
        FetchedQoderModelCatalog {
            enabled_models: vec![model.into()],
            raw_configs: BTreeMap::from([(model.into(), json!({"key":model}))]),
            capabilities: BTreeMap::new(),
        }
    }

    fn continuation() -> QoderResponseContinuation {
        QoderResponseContinuation {
            session_id: "session".into(),
            downstream_model: "auto".into(),
            system_fingerprint: [1; 32],
            tools_fingerprint: [2; 32],
            tool_names: vec!["lookup".into()],
            pending_tool_calls: vec![json!({"id":"call-1","name":"lookup","args":{"q":"fixture"}})],
        }
    }

    #[tokio::test]
    async fn qoder_r4_review_raw_session_arc_keeps_payload_owner_after_cache_eviction() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        let current = session();
        let raw = current.session.clone();
        let payload = raw.retained_bytes();
        cache
            .insert_session(scope(0), current.clone())
            .await
            .unwrap();
        let cached = cache.session(&scope(0), 2).await.unwrap();
        assert!(Arc::ptr_eq(&current.session, &cached.session));
        assert!(parent.snapshot().used_bytes > payload);
        cache.invalidate_scope(&scope(0)).await;
        drop(current);
        drop(cached);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, payload);
        assert!(raw
            .identity
            .security_oauth_token
            .starts_with("private-bearer"));
        drop(raw);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_rejected_session_replacement_removes_old_authorization() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        let old = session();
        let held = old.session.clone();
        cache.insert_session(scope(0), old).await.unwrap();
        parent.configure_limit(Some(0));
        assert!(cache.insert_session(scope(0), session()).await.is_err());
        assert!(cache.session(&scope(0), 2).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, held.retained_bytes());
        drop(held);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_session_index_refusal_preserves_current_admitted_payload() {
        let current = session();
        let raw = current.session.clone();
        let parent = ResidentMemoryBudget::new(Some(raw.retained_bytes()));
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        cache
            .insert_session(scope(0), current.clone())
            .await
            .unwrap();
        assert!(cache.session(&scope(0), 2).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, raw.retained_bytes());
        drop(current);
        drop(cache);
        assert!(parent.snapshot().used_bytes > 0);
        drop(raw);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_catalog_payload_and_owner_follow_last_snapshot() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        let first = cache.insert_catalog(scope(0), fetched("auto"), 1).await;
        let second = cache.catalog(&scope(0), 2).await.unwrap();
        assert!(Arc::ptr_eq(&first.data, &second.data));
        let used = parent.snapshot().used_bytes;
        cache.invalidate_scope(&scope(0)).await;
        drop(first);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(second);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_catalog_replacement_refusal_returns_empty_without_old_models() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        drop(cache.insert_catalog(scope(0), fetched("auto"), 1).await);
        parent.configure_limit(Some(0));
        let empty = cache
            .insert_catalog(
                scope(0),
                FetchedQoderModelCatalog {
                    enabled_models: Vec::new(),
                    raw_configs: BTreeMap::new(),
                    capabilities: BTreeMap::new(),
                },
                2,
            )
            .await;
        assert!(empty.enabled_models.is_empty());
        assert!(empty.data._memory.is_none());
        assert!(cache.catalog(&scope(0), 3).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_runtime_count_and_lazy_expiry_release_sparse_indexes() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        let source = session().session.value.clone();
        for index in 0..=MAX_RUNTIME_ENTRIES {
            cache
                .insert_session(
                    scope(index),
                    QoderCachedSession::new(
                        source.clone(),
                        "https://example.test".into(),
                        Some(10_000_000),
                        index as i64,
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
            drop(
                cache
                    .insert_catalog(scope(index), fetched("auto"), index as i64)
                    .await,
            );
        }
        assert_eq!(cache.sessions.read().await.len(), MAX_RUNTIME_ENTRIES);
        assert_eq!(cache.catalogs.read().await.len(), MAX_RUNTIME_ENTRIES);
        assert!(cache.session(&scope(0), 200).await.is_none());
        assert!(cache.catalog(&scope(0), 200).await.is_none());
        assert!(cache.session(&scope(1), 10_000_000).await.is_none());
        assert!(cache.catalog(&scope(1), 10_000_000).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_both_flights_bound_active_guards_waiters_and_cancellation() {
        for catalog in [false, true] {
            let parent = ResidentMemoryBudget::new(None);
            let cache = Arc::new(QoderRuntimeCache::with_parent(parent.clone()));
            let flights = if catalog {
                &cache.catalog_flights
            } else {
                &cache.session_flights
            };
            let mut guards = Vec::new();
            for index in 0..MAX_RUNTIME_ENTRIES {
                guards.push(cache.flight_lock(flights, &scope(index)).await.unwrap());
            }
            assert!(cache
                .flight_lock(flights, &scope(MAX_RUNTIME_ENTRIES))
                .await
                .is_none());
            let used = parent.snapshot().used_bytes;
            parent.configure_limit(Some(used));
            let waiting_cache = cache.clone();
            let waiting = tokio::spawn(async move {
                if catalog {
                    waiting_cache.catalog_lock(&scope(0)).await
                } else {
                    waiting_cache.session_lock(&scope(0)).await
                }
            });
            tokio::task::yield_now().await;
            assert!(!waiting.is_finished());
            waiting.abort();
            assert!(waiting.await.unwrap_err().is_cancelled());
            assert_eq!(parent.snapshot().used_bytes, used);
            let last = guards.pop().unwrap();
            drop(guards);
            drop(cache);
            assert!(parent.snapshot().used_bytes > 0);
            drop(last);
            assert_eq!(parent.snapshot().used_bytes, 0);
        }
    }

    #[tokio::test]
    async fn qoder_r4_review_continuation_claim_moves_shared_owner_without_deep_copy() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        cache
            .insert_response_alias(scope(0), "share", "user", "auto", "resp", continuation(), 1)
            .await
            .unwrap();
        let lookup = cache
            .response_continuation(&scope(0), "share", "user", "auto", "resp", 2)
            .await
            .unwrap();
        let used = parent.snapshot().used_bytes;
        let claimed = cache
            .claim_response_continuation(&scope(0), "share", "user", "auto", "resp", 3)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&lookup, &claimed));
        assert_eq!(parent.snapshot().used_bytes, used);
        assert!(cache
            .claim_response_continuation(&scope(0), "share", "user", "auto", "resp", 4)
            .await
            .is_none());
        drop(cache);
        drop(lookup);
        assert_eq!(parent.snapshot().used_bytes, used);
        drop(claimed);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_optional_alias_refusal_keeps_success_and_existing_identity() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        cache
            .insert_response_alias(scope(0), "share", "user", "auto", "resp", continuation(), 1)
            .await
            .unwrap();
        parent.configure_limit(Some(0));
        assert!(cache
            .insert_response_alias(scope(0), "share", "user", "auto", "resp", continuation(), 2)
            .await
            .is_err());
        cache
            .insert_response_alias(scope(0), "share", "user", "auto", "new", continuation(), 3)
            .await
            .unwrap();
        assert!(cache
            .response_continuation(&scope(0), "share", "user", "auto", "new", 4)
            .await
            .is_none());
        let held = cache
            .response_continuation(&scope(0), "share", "user", "auto", "resp", 4)
            .await
            .unwrap();
        assert!(cache
            .response_continuation(
                &scope(0),
                "share",
                "user",
                "auto",
                "resp",
                RESPONSE_ALIAS_TTL_MS + 1
            )
            .await
            .is_none());
        drop(cache);
        assert!(parent.snapshot().used_bytes > 0);
        drop(held);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn qoder_r4_review_request_projection_counts_unowned_session_and_skips_shared_payloads() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = QoderRuntimeCache::with_parent(parent.clone());
        let current = session();
        let runtime = PreparedQoderRuntime {
            scope: scope(0),
            session: current.clone(),
            catalog: cache.insert_catalog(scope(0), fetched("auto"), 1).await,
            credential_rail: QoderCredentialRail::GlobalOauth,
            account_id: "account".into(),
            auth_identity_generation: 1,
            token_refresh_generation: 1,
            account_incarnation: 1,
        };
        let before = runtime.retained_bytes();
        let payload = current.session.retained_bytes();
        assert!(before > payload);
        cache.insert_session(scope(0), current).await.unwrap();
        assert_eq!(before - runtime.retained_bytes(), payload);
        drop(cache);
        assert!(parent.snapshot().used_bytes > 0);
        drop(runtime);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }
}
