use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

const MAX_SESSION_SCOPES: usize = 256;
pub const DEEPSEEK_SESSION_TTL_MS: i64 = 30 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeepSeekCachedSession {
    pub session_id: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
}

impl DeepSeekCachedSession {
    pub fn new(session_id: String, now_ms: i64) -> Result<Self, String> {
        let session_id = session_id.trim().to_string();
        if session_id.is_empty() || session_id.len() > 512 {
            return Err("DeepSeek session id is empty or exceeds 512 bytes".to_string());
        }
        Ok(Self {
            session_id,
            created_at_ms: now_ms,
            expires_at_ms: now_ms.saturating_add(DEEPSEEK_SESSION_TTL_MS),
        })
    }

    fn is_fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms > now_ms
    }
}

#[derive(Debug, Default)]
pub struct DeepSeekRuntimeCache {
    sessions: RwLock<HashMap<DeepSeekRuntimeScope, DeepSeekCachedSession>>,
    session_flights: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl DeepSeekRuntimeCache {
    pub async fn session(
        &self,
        scope: &DeepSeekRuntimeScope,
        now_ms: i64,
    ) -> Option<DeepSeekCachedSession> {
        let candidate = self.sessions.read().await.get(scope).cloned();
        if candidate
            .as_ref()
            .is_some_and(|session| session.is_fresh(now_ms))
        {
            return candidate;
        }
        if candidate.is_some() {
            self.sessions.write().await.remove(scope);
        }
        None
    }

    pub async fn insert_session(
        &self,
        scope: DeepSeekRuntimeScope,
        session: DeepSeekCachedSession,
    ) {
        let mut sessions = self.sessions.write().await;
        if sessions.len() >= MAX_SESSION_SCOPES && !sessions.contains_key(&scope) {
            if let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, session)| session.created_at_ms)
                .map(|(scope, _)| scope.clone())
            {
                sessions.remove(&oldest);
            }
        }
        sessions.insert(scope, session);
    }

    pub async fn invalidate_session(&self, scope: &DeepSeekRuntimeScope) {
        self.sessions.write().await.remove(scope);
    }

    pub async fn session_lock(&self, scope: &DeepSeekRuntimeScope) -> OwnedMutexGuard<()> {
        let key = scope.key().to_string();
        let flight = {
            let mut flights = self.session_flights.lock().await;
            flights.retain(|_, flight| Arc::strong_count(flight) > 1 || flight.try_lock().is_err());
            Arc::clone(
                flights
                    .entry(key)
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        flight.lock_owned().await
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
            .await;

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
                .await;
        }
        cache.invalidate_session(&first).await;
        assert!(cache.session(&first, 1_001).await.is_none());
        assert_eq!(
            cache.session(&second, 1_001).await.unwrap().session_id,
            "second"
        );
    }
}
