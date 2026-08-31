use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

const MAX_CREDENTIAL_CACHE_ENTRIES: usize = 16;
pub const CURSOR_MODEL_STALE_TTL_MS: i64 = 60 * 60 * 1000;

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    expires_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct CursorApiKeyTokenCache {
    tokens: RwLock<HashMap<CursorApiKeyCredentialScope, CachedToken>>,
    auth_cooldowns: RwLock<HashMap<CursorApiKeyCredentialScope, i64>>,
    flights: Mutex<HashMap<CursorApiKeyCredentialScope, Arc<Mutex<()>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorApiKeyCredentialScope(String);

impl CursorApiKeyCredentialScope {
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        credential_generation: u64,
        runtime_fingerprint: &str,
        api_key_hash: &str,
    ) -> Self {
        Self(scoped_digest(
            b"cc-switch-server:cursor-api-key-credential:v1\0",
            [
                app,
                provider_id,
                &provider_revision.to_string(),
                &credential_generation.to_string(),
                runtime_fingerprint,
                api_key_hash,
            ],
            "cursor-api-key-credential-v1",
        ))
    }
}

#[derive(Debug, Clone)]
pub struct CursorModelCatalog {
    pub models: Vec<String>,
    pub fetched_at_ms: i64,
    expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorModelCatalogScope(String);

impl CursorModelCatalogScope {
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        credential_generation: u64,
        runtime_fingerprint: &str,
        api_key_hash: &str,
    ) -> Self {
        Self(scoped_digest(
            b"cc-switch-server:cursor-model-catalog:v1\0",
            [
                app,
                provider_id,
                &provider_revision.to_string(),
                &credential_generation.to_string(),
                runtime_fingerprint,
                api_key_hash,
            ],
            "cursor-model-catalog-v1",
        ))
    }
}

fn scoped_digest<const N: usize>(domain: &[u8], values: [&str; N], prefix: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for value in values {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{prefix}:{:x}", hasher.finalize())
}

#[derive(Debug, Default)]
pub struct CursorModelCatalogCache {
    catalogs: RwLock<HashMap<CursorModelCatalogScope, CursorModelCatalog>>,
    flights: Mutex<HashMap<CursorModelCatalogScope, Arc<Mutex<()>>>>,
}

impl CursorModelCatalogCache {
    pub async fn fresh(
        &self,
        scope: &CursorModelCatalogScope,
        now_ms: i64,
    ) -> Option<CursorModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .filter(|catalog| catalog.expires_at_ms > now_ms)
            .cloned()
    }

    pub async fn last_known_good(
        &self,
        scope: &CursorModelCatalogScope,
        now_ms: i64,
    ) -> Option<CursorModelCatalog> {
        let mut catalogs = self.catalogs.write().await;
        let usable = catalogs.get(scope).is_some_and(|catalog| {
            catalog
                .fetched_at_ms
                .saturating_add(CURSOR_MODEL_STALE_TTL_MS)
                > now_ms
        });
        if usable {
            return catalogs.get(scope).cloned();
        }
        catalogs.remove(scope);
        None
    }

    pub async fn insert(
        &self,
        scope: CursorModelCatalogScope,
        models: Vec<String>,
        fetched_at_ms: i64,
        ttl_ms: i64,
    ) -> CursorModelCatalog {
        let catalog = CursorModelCatalog {
            models,
            fetched_at_ms,
            expires_at_ms: fetched_at_ms.saturating_add(ttl_ms),
        };
        let mut catalogs = self.catalogs.write().await;
        catalogs.insert(scope, catalog.clone());
        while catalogs.len() > MAX_CREDENTIAL_CACHE_ENTRIES {
            let Some(oldest) = catalogs
                .iter()
                .min_by_key(|(_, catalog)| catalog.fetched_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            catalogs.remove(&oldest);
        }
        catalog
    }

    pub async fn invalidate(&self, scope: &CursorModelCatalogScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub async fn lock(&self, scope: &CursorModelCatalogScope) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.flights.lock().await;
            flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
            flights
                .entry(scope.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

impl CursorApiKeyTokenCache {
    pub async fn get(&self, scope: &CursorApiKeyCredentialScope, now_ms: i64) -> Option<String> {
        let mut tokens = self.tokens.write().await;
        let is_fresh = tokens
            .get(scope)
            .is_some_and(|token| token.expires_at_ms > now_ms.saturating_add(30_000));
        if is_fresh {
            return tokens.get(scope).map(|token| token.value.clone());
        }
        tokens.remove(scope);
        None
    }

    pub async fn insert(
        &self,
        scope: CursorApiKeyCredentialScope,
        value: String,
        expires_at_ms: i64,
    ) {
        let mut tokens = self.tokens.write().await;
        tokens.insert(
            scope,
            CachedToken {
                value,
                expires_at_ms,
            },
        );
        while tokens.len() > MAX_CREDENTIAL_CACHE_ENTRIES {
            let Some(oldest) = tokens
                .iter()
                .min_by_key(|(_, token)| token.expires_at_ms)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            tokens.remove(&oldest);
        }
    }

    pub async fn invalidate(&self, scope: &CursorApiKeyCredentialScope) {
        self.tokens.write().await.remove(scope);
    }

    pub async fn mark_auth_cooldown(&self, scope: &CursorApiKeyCredentialScope, until_ms: i64) {
        let mut cooldowns = self.auth_cooldowns.write().await;
        cooldowns
            .entry(scope.clone())
            .and_modify(|current| *current = (*current).max(until_ms))
            .or_insert(until_ms);
        while cooldowns.len() > MAX_CREDENTIAL_CACHE_ENTRIES {
            let Some(oldest) = cooldowns
                .iter()
                .min_by_key(|(_, until)| **until)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            cooldowns.remove(&oldest);
        }
    }

    pub async fn auth_cooldown_until(
        &self,
        scope: &CursorApiKeyCredentialScope,
        now_ms: i64,
    ) -> Option<i64> {
        let mut cooldowns = self.auth_cooldowns.write().await;
        let until = cooldowns.get(scope).copied();
        if until.is_some_and(|until| until <= now_ms) {
            cooldowns.remove(scope);
            return None;
        }
        until
    }

    pub async fn lock(&self, scope: &CursorApiKeyCredentialScope) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.flights.lock().await;
            flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
            flights
                .entry(scope.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token_scope(index: usize) -> CursorApiKeyCredentialScope {
        CursorApiKeyCredentialScope::derive(
            "codex",
            "provider-a",
            3,
            index as u64,
            &format!("runtime-{index}"),
            &format!("key-hash-{index}"),
        )
    }

    fn catalog_scope(index: usize) -> CursorModelCatalogScope {
        CursorModelCatalogScope::derive(
            "codex",
            "provider-a",
            3,
            index as u64,
            &format!("runtime-{index}"),
            &format!("key-hash-{index}"),
        )
    }

    #[tokio::test]
    async fn cache_honors_refresh_buffer_and_invalidation() {
        let cache = CursorApiKeyTokenCache::default();
        let scope = token_scope(1);
        cache
            .insert(scope.clone(), "token".to_string(), 100_000)
            .await;
        assert_eq!(cache.get(&scope, 60_000).await.as_deref(), Some("token"));
        assert!(cache.get(&scope, 70_001).await.is_none());
        assert!(cache.tokens.read().await.is_empty());
        cache.invalidate(&scope).await;
        assert!(cache.get(&scope, 0).await.is_none());
    }

    #[tokio::test]
    async fn auth_cooldown_is_monotonic_and_expires() {
        let cache = CursorApiKeyTokenCache::default();
        let scope = token_scope(1);
        cache.mark_auth_cooldown(&scope, 10_000).await;
        cache.mark_auth_cooldown(&scope, 9_000).await;
        assert_eq!(cache.auth_cooldown_until(&scope, 5_000).await, Some(10_000));
        assert!(cache.auth_cooldown_until(&scope, 10_000).await.is_none());
    }

    #[tokio::test]
    async fn credential_caches_are_bounded_across_key_rotation() {
        let tokens = CursorApiKeyTokenCache::default();
        let catalogs = CursorModelCatalogCache::default();
        for index in 0..(MAX_CREDENTIAL_CACHE_ENTRIES + 4) {
            tokens
                .insert(
                    token_scope(index),
                    format!("token-{index}"),
                    100_000 + index as i64,
                )
                .await;
            catalogs
                .insert(
                    catalog_scope(index),
                    vec![format!("model-{index}")],
                    index as i64,
                    100,
                )
                .await;
        }
        assert_eq!(
            tokens.tokens.read().await.len(),
            MAX_CREDENTIAL_CACHE_ENTRIES
        );
        assert_eq!(
            catalogs.catalogs.read().await.len(),
            MAX_CREDENTIAL_CACHE_ENTRIES
        );

        drop(tokens.lock(&token_scope(90)).await);
        drop(tokens.lock(&token_scope(91)).await);
        assert_eq!(tokens.flights.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn token_and_cooldown_are_isolated_by_exact_provider_runtime_scope() {
        let cache = CursorApiKeyTokenCache::default();
        let original = CursorApiKeyCredentialScope::derive(
            "codex",
            "provider-a",
            3,
            7,
            "runtime-a",
            "same-key-hash",
        );
        let other_provider = CursorApiKeyCredentialScope::derive(
            "codex",
            "provider-b",
            3,
            7,
            "runtime-a",
            "same-key-hash",
        );
        let other_runtime = CursorApiKeyCredentialScope::derive(
            "codex",
            "provider-a",
            3,
            7,
            "runtime-b",
            "same-key-hash",
        );
        let other_generation = CursorApiKeyCredentialScope::derive(
            "codex",
            "provider-a",
            3,
            8,
            "runtime-a",
            "same-key-hash",
        );
        cache
            .insert(original.clone(), "token-a".to_string(), 100_000)
            .await;
        cache.mark_auth_cooldown(&original, 90_000).await;

        assert_eq!(
            cache.get(&original, 1_000).await.as_deref(),
            Some("token-a")
        );
        for scope in [&other_provider, &other_runtime, &other_generation] {
            assert!(cache.get(scope, 1_000).await.is_none());
            assert!(cache.auth_cooldown_until(scope, 1_000).await.is_none());
        }
        assert_eq!(
            cache.auth_cooldown_until(&original, 1_000).await,
            Some(90_000)
        );
    }

    #[tokio::test]
    async fn model_catalog_keeps_only_bounded_stale_last_known_good() {
        let cache = CursorModelCatalogCache::default();
        let scope = catalog_scope(1);
        cache
            .insert(scope.clone(), vec!["composer".to_string()], 1_000, 100)
            .await;
        assert!(cache.fresh(&scope, 1_050).await.is_some());
        assert!(cache.fresh(&scope, 1_101).await.is_none());
        assert_eq!(
            cache.last_known_good(&scope, 2_000).await.unwrap().models,
            vec!["composer"]
        );
        assert!(cache
            .last_known_good(&scope, 1_000 + CURSOR_MODEL_STALE_TTL_MS)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn model_catalog_scope_fences_runtime_credential_and_key_generations() {
        let cache = CursorModelCatalogCache::default();
        let original =
            CursorModelCatalogScope::derive("codex", "provider-a", 3, 7, "runtime-a", "key-hash-a");
        let changed_runtime =
            CursorModelCatalogScope::derive("codex", "provider-a", 3, 7, "runtime-b", "key-hash-a");
        let changed_generation =
            CursorModelCatalogScope::derive("codex", "provider-a", 3, 8, "runtime-a", "key-hash-a");
        let changed_key =
            CursorModelCatalogScope::derive("codex", "provider-a", 3, 7, "runtime-a", "key-hash-b");
        cache
            .insert(original.clone(), vec!["composer".to_string()], 1_000, 100)
            .await;

        assert!(cache.last_known_good(&original, 2_000).await.is_some());
        assert!(cache
            .last_known_good(&changed_runtime, 2_000)
            .await
            .is_none());
        assert!(cache
            .last_known_good(&changed_generation, 2_000)
            .await
            .is_none());
        assert!(cache.last_known_good(&changed_key, 2_000).await.is_none());
    }

    #[tokio::test]
    async fn authoritative_empty_catalog_replaces_stale_and_invalidation_fails_closed() {
        let cache = CursorModelCatalogCache::default();
        let scope = catalog_scope(2);
        cache
            .insert(scope.clone(), vec!["old-model".to_string()], 1_000, 100)
            .await;
        cache.insert(scope.clone(), Vec::new(), 2_000, 100).await;
        assert!(cache
            .last_known_good(&scope, 2_050)
            .await
            .is_some_and(|catalog| catalog.models.is_empty()));

        cache.invalidate(&scope).await;
        assert!(cache.last_known_good(&scope, 2_050).await.is_none());
    }
}
