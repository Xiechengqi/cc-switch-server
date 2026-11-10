use std::collections::BTreeMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::RwLock;

use crate::infra::resident_flights::{node_bytes, ResidentFlightGuard, ResidentFlights};
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};

const MAX_SESSION_SCOPES: usize = 256;
const MAX_ACTIVE_SESSION_FLIGHTS: usize = 64;
pub const DEEPSEEK_SESSION_TTL_MS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeepSeekRuntimeScope(String);

impl DeepSeekRuntimeScope {
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        auth_identity_generation: u64,
        token_refresh_generation: u64,
        share_id: &str,
        user_identity: &str,
        client_session_id: &str,
        model_family: &str,
    ) -> Result<Self, String> {
        let required = [
            app.trim(),
            provider_id.trim(),
            runtime_fingerprint.trim(),
            account_id.trim(),
            share_id.trim(),
            user_identity.trim(),
            client_session_id.trim(),
            model_family.trim(),
        ];
        if required.iter().any(|value| value.is_empty()) {
            return Err("DeepSeek runtime scope contains an empty identity component".to_string());
        }
        let mut hasher = Sha256::new();
        hasher.update(b"cc-switch-server:deepseek-web-session:v1\0");
        for value in [
            required[0],
            required[1],
            &provider_revision.to_string(),
            required[2],
            required[3],
            &auth_identity_generation.to_string(),
            &token_refresh_generation.to_string(),
            required[4],
            required[5],
            required[6],
            required[7],
        ] {
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        }
        Ok(Self(format!("deepseek-session-v1:{:x}", hasher.finalize())))
    }

    fn key(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct DeepSeekCachedSession(Arc<DeepSeekSessionData>);

#[derive(Debug)]
pub struct DeepSeekSessionData {
    pub session_id: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    _memory: Option<ResidentMemoryReservation>,
}

impl std::ops::Deref for DeepSeekCachedSession {
    type Target = DeepSeekSessionData;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PartialEq for DeepSeekCachedSession {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.created_at_ms == other.created_at_ms
            && self.expires_at_ms == other.expires_at_ms
    }
}
impl Eq for DeepSeekCachedSession {}

impl DeepSeekCachedSession {
    pub fn new(session_id: String, now_ms: i64) -> Result<Self, String> {
        let session_id = session_id.trim().to_string();
        if session_id.is_empty() || session_id.len() > 512 {
            return Err("DeepSeek session id is empty or exceeds 512 bytes".to_string());
        }
        Ok(Self(Arc::new(DeepSeekSessionData {
            session_id,
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(DEEPSEEK_SESSION_TTL_MS),
            _memory: None,
        })))
    }

    fn is_fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms > now_ms
    }
}

#[derive(Debug)]
pub struct DeepSeekRuntimeCache {
    sessions: RwLock<BTreeMap<DeepSeekRuntimeScope, DeepSeekCachedSession>>,
    session_flights: ResidentFlights<DeepSeekRuntimeScope>,
    parent: ResidentMemoryBudget,
}

#[derive(Debug)]
pub struct DeepSeekSessionCapacity;

impl Default for DeepSeekRuntimeCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl DeepSeekRuntimeCache {
    pub(crate) fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            sessions: RwLock::new(BTreeMap::new()),
            session_flights: ResidentFlights::new(parent.clone(), MAX_ACTIVE_SESSION_FLIGHTS),
            parent,
        }
    }

    pub async fn session(
        &self,
        scope: &DeepSeekRuntimeScope,
        now_ms: i64,
    ) -> Option<DeepSeekCachedSession> {
        let mut sessions = self.sessions.write().await;
        sessions.retain(|_, session| session.is_fresh(now_ms));
        sessions.get(scope).cloned()
    }

    pub async fn insert_session(
        &self,
        scope: DeepSeekRuntimeScope,
        session: DeepSeekCachedSession,
    ) -> Result<DeepSeekCachedSession, DeepSeekSessionCapacity> {
        let mut sessions = self.sessions.write().await;
        // Replacement revokes the old index even when admission is refused;
        // snapshots already held by callers keep their original charge.
        sessions.remove(&scope);
        sessions.retain(|_, previous| previous.is_fresh(session.created_at_ms));
        if sessions.len() >= MAX_SESSION_SCOPES {
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, session)| session.created_at_ms)
                .map(|(scope, _)| scope.clone())
            {
                sessions.remove(&oldest);
            }
        }
        let bytes = node_bytes::<DeepSeekRuntimeScope, DeepSeekCachedSession>(
            scope
                .0
                .capacity()
                .saturating_add(std::mem::size_of::<DeepSeekSessionData>())
                .saturating_add(std::mem::size_of::<usize>() * 2)
                .saturating_add(session.session_id.len()),
        );
        let Some(memory) = self.parent.reserve(bytes) else {
            crate::metrics::record_resident_retention_refused("deepseek_session");
            return Err(DeepSeekSessionCapacity);
        };
        let session = DeepSeekCachedSession(Arc::new(DeepSeekSessionData {
            session_id: session.session_id.as_str().into(),
            created_at_ms: session.created_at_ms,
            expires_at_ms: session.expires_at_ms,
            _memory: Some(memory),
        }));
        sessions.insert(scope, session.clone());
        Ok(session)
    }

    pub async fn invalidate_session(&self, scope: &DeepSeekRuntimeScope) {
        self.sessions.write().await.remove(scope);
    }

    pub(crate) async fn session_lock(
        &self,
        scope: &DeepSeekRuntimeScope,
    ) -> Option<ResidentFlightGuard> {
        let guard = self.session_flights.lock(scope, scope.0.capacity()).await;
        if guard.is_none() {
            crate::metrics::record_resident_retention_refused("deepseek_session_flight");
        }
        guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(account: &str, generation: u64, share: &str) -> DeepSeekRuntimeScope {
        DeepSeekRuntimeScope::derive(
            "claude",
            "provider-a",
            7,
            "runtime-a",
            account,
            generation,
            3,
            share,
            "user@example.com",
            "session-a",
            "deepseek-v4",
        )
        .unwrap()
    }

    #[tokio::test]
    async fn session_cache_fences_provider_account_generation_share_and_ttl() {
        let cache = DeepSeekRuntimeCache::default();
        let exact = scope("account-a", 2, "share-a");
        cache
            .insert_session(
                exact.clone(),
                DeepSeekCachedSession::new("upstream-session-a".to_string(), 1_000).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            cache.session(&exact, 1_001).await.unwrap().session_id,
            "upstream-session-a"
        );
        assert!(cache
            .session(&scope("account-b", 2, "share-a"), 1_001)
            .await
            .is_none());
        assert!(cache
            .session(&scope("account-a", 3, "share-a"), 1_001)
            .await
            .is_none());
        assert!(cache
            .session(&scope("account-a", 2, "share-b"), 1_001)
            .await
            .is_none());
        assert!(cache
            .session(&exact, 1_000 + DEEPSEEK_SESSION_TTL_MS)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn session_cache_invalidation_is_exact_scope_only() {
        let cache = DeepSeekRuntimeCache::default();
        let first = scope("account-a", 2, "share-a");
        let second = scope("account-a", 2, "share-b");
        for (scope, id) in [(&first, "first"), (&second, "second")] {
            cache
                .insert_session(
                    scope.clone(),
                    DeepSeekCachedSession::new(id.to_string(), 1_000).unwrap(),
                )
                .await
                .unwrap();
        }
        cache.invalidate_session(&first).await;
        assert!(cache.session(&first, 1_001).await.is_none());
        assert_eq!(
            cache.session(&second, 1_001).await.unwrap().session_id,
            "second"
        );
    }

    #[tokio::test]
    async fn r5_resource_deepseek_last_owner_survives_invalidation_replacement_and_expiry() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = DeepSeekRuntimeCache::with_parent(parent.clone());
        let key = scope("account", 1, "share");
        let first = cache
            .insert_session(
                key.clone(),
                DeepSeekCachedSession::new("first".into(), 1000).unwrap(),
            )
            .await
            .unwrap();
        let bytes = parent.snapshot().used_bytes;
        let first_view = cache.session(&key, 1001).await.unwrap();
        assert!(Arc::ptr_eq(&first.0, &first_view.0));
        assert_eq!(parent.snapshot().used_bytes, bytes);
        cache.invalidate_session(&key).await;
        assert_eq!(parent.snapshot().used_bytes, bytes);
        let second = cache
            .insert_session(
                key.clone(),
                DeepSeekCachedSession::new("second".into(), 1001).unwrap(),
            )
            .await
            .unwrap();
        assert!(parent.snapshot().used_bytes > bytes);
        drop(first);
        drop(first_view);
        let second_bytes = parent.snapshot().used_bytes;
        assert!(second_bytes > 0);
        assert!(cache
            .session(&key, 1001 + DEEPSEEK_SESSION_TTL_MS)
            .await
            .is_none());
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, second_bytes);
        drop(second);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r5_resource_deepseek_refused_replacement_revokes_old_index_without_relabeling_auth() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = DeepSeekRuntimeCache::with_parent(parent.clone());
        let key = scope("account", 1, "share");
        let held = cache
            .insert_session(
                key.clone(),
                DeepSeekCachedSession::new("old".into(), 1000).unwrap(),
            )
            .await
            .unwrap();
        let bytes = parent.snapshot().used_bytes;
        parent.configure_limit(Some(bytes));
        assert!(cache
            .insert_session(
                key.clone(),
                DeepSeekCachedSession::new("new".into(), 1001).unwrap()
            )
            .await
            .is_err());
        assert!(cache.session(&key, 1002).await.is_none());
        assert_eq!(held.session_id, "old");
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, bytes);
        drop(held);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r5_resource_deepseek_cache_eviction_preserves_snapshot_charge() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = DeepSeekRuntimeCache::with_parent(parent.clone());
        let first_key = scope("account", 1, "first");
        let first = cache
            .insert_session(
                first_key.clone(),
                DeepSeekCachedSession::new("first".into(), 1000).unwrap(),
            )
            .await
            .unwrap();
        let first_bytes = parent.snapshot().used_bytes;
        for index in 0..MAX_SESSION_SCOPES {
            cache
                .insert_session(
                    scope("account", 1, &format!("share-{index}")),
                    DeepSeekCachedSession::new(format!("session-{index}"), 1001 + index as i64)
                        .unwrap(),
                )
                .await
                .unwrap();
        }
        assert_eq!(cache.sessions.read().await.len(), MAX_SESSION_SCOPES);
        assert!(cache.session(&first_key, 2000).await.is_none());
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, first_bytes);
        drop(first);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn r5_resource_deepseek_distinct_flights_waiter_cancel_and_guard_drop_are_bounded() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = Arc::new(DeepSeekRuntimeCache::with_parent(parent.clone()));
        let mut guards = Vec::new();
        for index in 0..MAX_ACTIVE_SESSION_FLIGHTS {
            guards.push(
                cache
                    .session_lock(&scope("account", 1, &format!("share-{index}")))
                    .await
                    .unwrap(),
            );
        }
        assert!(cache
            .session_lock(&scope("account", 1, "overflow"))
            .await
            .is_none());
        let bytes = parent.snapshot().used_bytes;
        let waiting_cache = cache.clone();
        let waiter = tokio::spawn(async move {
            waiting_cache
                .session_lock(&scope("account", 1, "share-0"))
                .await
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        assert!(parent.snapshot().used_bytes > bytes);
        waiter.abort();
        assert!(waiter.await.unwrap_err().is_cancelled());
        assert_eq!(parent.snapshot().used_bytes, bytes);
        drop(cache);
        assert!(parent.snapshot().used_bytes > 0);
        drop(guards);
        assert_eq!(parent.snapshot().used_bytes, 0);
        let zero = DeepSeekRuntimeCache::with_parent(ResidentMemoryBudget::new(Some(0)));
        assert!(zero
            .session_lock(&scope("account", 1, "zero"))
            .await
            .is_none());
    }
}
