use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

const MAX_CREDENTIAL_CACHE_ENTRIES: usize = 16;

#[derive(Debug, Clone)]
struct CachedToken {
    value: String,
    expires_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct CursorApiKeyTokenCache {
    tokens: RwLock<HashMap<String, CachedToken>>,
    auth_cooldowns: RwLock<HashMap<String, i64>>,
    flights: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

#[derive(Debug, Clone)]
pub struct CursorModelCatalog {
    pub models: Vec<String>,
    pub fetched_at_ms: i64,
    expires_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct CursorModelCatalogCache {
    catalogs: RwLock<HashMap<String, CursorModelCatalog>>,
    flights: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl CursorModelCatalogCache {
    pub async fn fresh(&self, key_hash: &str, now_ms: i64) -> Option<CursorModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(key_hash)
            .filter(|catalog| catalog.expires_at_ms > now_ms)
            .cloned()
    }

    pub async fn last_known_good(&self, key_hash: &str) -> Option<CursorModelCatalog> {
        self.catalogs.read().await.get(key_hash).cloned()
    }

    pub async fn insert(
        &self,
        key_hash: String,
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
        catalogs.insert(key_hash, catalog.clone());
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

    pub async fn lock(&self, key_hash: &str) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.flights.lock().await;
            flights.retain(|key, flight| key == key_hash || Arc::strong_count(flight) > 1);
            flights
                .entry(key_hash.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

impl CursorApiKeyTokenCache {
    pub async fn get(&self, key_hash: &str, now_ms: i64) -> Option<String> {
        let mut tokens = self.tokens.write().await;
        let is_fresh = tokens
            .get(key_hash)
            .is_some_and(|token| token.expires_at_ms > now_ms.saturating_add(30_000));
        if is_fresh {
            return tokens.get(key_hash).map(|token| token.value.clone());
        }
        tokens.remove(key_hash);
        None
    }

    pub async fn insert(&self, key_hash: String, value: String, expires_at_ms: i64) {
        let mut tokens = self.tokens.write().await;
        tokens.insert(
            key_hash,
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

    pub async fn invalidate(&self, key_hash: &str) {
        self.tokens.write().await.remove(key_hash);
    }

    pub async fn mark_auth_cooldown(&self, key_hash: &str, until_ms: i64) {
        let mut cooldowns = self.auth_cooldowns.write().await;
        cooldowns
            .entry(key_hash.to_string())
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

    pub async fn auth_cooldown_until(&self, key_hash: &str, now_ms: i64) -> Option<i64> {
        let mut cooldowns = self.auth_cooldowns.write().await;
        let until = cooldowns.get(key_hash).copied();
        if until.is_some_and(|until| until <= now_ms) {
            cooldowns.remove(key_hash);
            return None;
        }
        until
    }

    pub async fn lock(&self, key_hash: &str) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.flights.lock().await;
            flights.retain(|key, flight| key == key_hash || Arc::strong_count(flight) > 1);
            flights
                .entry(key_hash.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_honors_refresh_buffer_and_invalidation() {
        let cache = CursorApiKeyTokenCache::default();
        cache
            .insert("key".to_string(), "token".to_string(), 100_000)
            .await;
        assert_eq!(cache.get("key", 60_000).await.as_deref(), Some("token"));
        assert!(cache.get("key", 70_001).await.is_none());
        assert!(cache.tokens.read().await.is_empty());
        cache.invalidate("key").await;
        assert!(cache.get("key", 0).await.is_none());
    }

    #[tokio::test]
    async fn auth_cooldown_is_monotonic_and_expires() {
        let cache = CursorApiKeyTokenCache::default();
        cache.mark_auth_cooldown("key", 10_000).await;
        cache.mark_auth_cooldown("key", 9_000).await;
        assert_eq!(cache.auth_cooldown_until("key", 5_000).await, Some(10_000));
        assert!(cache.auth_cooldown_until("key", 10_000).await.is_none());
    }

    #[tokio::test]
    async fn credential_caches_are_bounded_across_key_rotation() {
        let tokens = CursorApiKeyTokenCache::default();
        let catalogs = CursorModelCatalogCache::default();
        for index in 0..(MAX_CREDENTIAL_CACHE_ENTRIES + 4) {
            tokens
                .insert(
                    format!("key-{index}"),
                    format!("token-{index}"),
                    100_000 + index as i64,
                )
                .await;
            catalogs
                .insert(
                    format!("key-{index}"),
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

        drop(tokens.lock("old-key").await);
        drop(tokens.lock("new-key").await);
        assert_eq!(tokens.flights.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn model_catalog_keeps_stale_last_known_good() {
        let cache = CursorModelCatalogCache::default();
        cache
            .insert("key".to_string(), vec!["composer".to_string()], 1_000, 100)
            .await;
        assert!(cache.fresh("key", 1_050).await.is_some());
        assert!(cache.fresh("key", 1_101).await.is_none());
        assert_eq!(
            cache.last_known_good("key").await.unwrap().models,
            vec!["composer"]
        );
    }
}
