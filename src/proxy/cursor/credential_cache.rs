use std::collections::BTreeMap;

use super::resident::{CursorFlights, ResidentEntry};
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};
use crate::proxy::ProxyError;

use sha2::{Digest, Sha256};
use tokio::sync::{OwnedMutexGuard, RwLock};

const MAX_CREDENTIAL_CACHE_ENTRIES: usize = 16;
pub const CURSOR_MODEL_STALE_TTL_MS: i64 = 60 * 60 * 1000;

#[derive(Debug)]
struct CachedToken {
    value: String,
    expires_at_ms: i64,
}

#[derive(Debug)]
pub struct CursorApiKeyTokenCache {
    tokens: RwLock<BTreeMap<CursorApiKeyCredentialScope, ResidentEntry<CachedToken>>>,
    auth_cooldowns: RwLock<BTreeMap<CursorApiKeyCredentialScope, ResidentEntry<i64>>>,
    flights: CursorFlights<CursorApiKeyCredentialScope>,
    parent: ResidentMemoryBudget,
}

impl Default for CursorApiKeyTokenCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
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

#[derive(Debug)]
pub struct CursorModelCatalogCache {
    catalogs: RwLock<BTreeMap<CursorModelCatalogScope, ResidentEntry<CursorModelCatalog>>>,
    flights: CursorFlights<CursorModelCatalogScope>,
    parent: ResidentMemoryBudget,
}

impl Default for CursorModelCatalogCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl CursorModelCatalogCache {
    fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            catalogs: RwLock::new(BTreeMap::new()),
            flights: CursorFlights::new(parent.clone()),
            parent,
        }
    }

    pub async fn fresh(
        &self,
        scope: &CursorModelCatalogScope,
        now_ms: i64,
    ) -> Option<CursorModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .map(|entry| &entry.value)
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
                .value
                .fetched_at_ms
                .saturating_add(CURSOR_MODEL_STALE_TTL_MS)
                > now_ms
        });
        if usable {
            return catalogs.get(scope).map(|entry| entry.value.clone());
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
        // A rejected replacement must not leave an older catalog able to
        // authorize stale model IDs, including after an authoritative empty.
        catalogs.remove(&scope);
        let dynamic = catalog.models.iter().fold(
            scope.0.capacity().saturating_add(
                catalog
                    .models
                    .capacity()
                    .saturating_mul(std::mem::size_of::<String>()),
            ),
            |bytes, model| bytes.saturating_add(model.capacity()),
        );
        let Some(entry) =
            ResidentEntry::with::<CursorModelCatalogScope>(&self.parent, dynamic, || {
                catalog.clone()
            })
        else {
            return catalog;
        };
        catalogs.insert(scope, entry);
        while catalogs.len() > MAX_CREDENTIAL_CACHE_ENTRIES {
            let Some(oldest) = catalogs
                .iter()
                .min_by_key(|(_, catalog)| catalog.value.fetched_at_ms)
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

    pub(crate) async fn lock(
        &self,
        scope: &CursorModelCatalogScope,
    ) -> Result<OwnedMutexGuard<ResidentMemoryReservation>, ProxyError> {
        self.flights.lock(scope, scope.0.capacity()).await
    }
}

impl CursorApiKeyTokenCache {
    fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            tokens: RwLock::new(BTreeMap::new()),
            auth_cooldowns: RwLock::new(BTreeMap::new()),
            flights: CursorFlights::new(parent.clone()),
            parent,
        }
    }

    pub async fn get(&self, scope: &CursorApiKeyCredentialScope, now_ms: i64) -> Option<String> {
        let mut tokens = self.tokens.write().await;
        let is_fresh = tokens
            .get(scope)
            .is_some_and(|token| token.value.expires_at_ms > now_ms.saturating_add(30_000));
        if is_fresh {
            return tokens.get(scope).map(|token| token.value.value.clone());
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
        tokens.remove(&scope);
        let dynamic = scope.0.capacity().saturating_add(value.capacity());
        let Some(entry) = ResidentEntry::new::<CursorApiKeyCredentialScope>(
            &self.parent,
            CachedToken {
                value,
                expires_at_ms,
            },
            dynamic,
        ) else {
            return;
        };
        tokens.insert(scope, entry);
        while tokens.len() > MAX_CREDENTIAL_CACHE_ENTRIES {
            let Some(oldest) = tokens
                .iter()
                .min_by_key(|(_, token)| token.value.expires_at_ms)
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
        if let Some(current) = cooldowns.get_mut(scope) {
            current.value = current.value.max(until_ms);
        } else if let Some(entry) = ResidentEntry::new::<CursorApiKeyCredentialScope>(
            &self.parent,
            until_ms,
            scope.0.capacity(),
        ) {
            cooldowns.insert(scope.clone(), entry);
        }
        while cooldowns.len() > MAX_CREDENTIAL_CACHE_ENTRIES {
            let Some(oldest) = cooldowns
                .iter()
                .min_by_key(|(_, until)| until.value)
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
        let until = cooldowns.get(scope).map(|entry| entry.value);
        if until.is_some_and(|until| until <= now_ms) {
            cooldowns.remove(scope);
            return None;
        }
        until
    }

    pub(crate) async fn lock(
        &self,
        scope: &CursorApiKeyCredentialScope,
    ) -> Result<OwnedMutexGuard<ResidentMemoryReservation>, ProxyError> {
        self.flights.lock(scope, scope.0.capacity()).await
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
    async fn cursor_r4_credential_and_cooldown_owners_follow_invalidation_and_expiry() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CursorApiKeyTokenCache::with_parent(parent.clone());
        let scope = token_scope(1);
        cache
            .insert(scope.clone(), "token".repeat(2048), 100_000)
            .await;
        let token_bytes = parent.snapshot().used_bytes;
        assert!(token_bytes > 8192);
        cache.mark_auth_cooldown(&scope, 90_000).await;
        assert!(parent.snapshot().used_bytes > token_bytes);
        cache.invalidate(&scope).await;
        assert!(cache.get(&scope, 1_000).await.is_none());
        assert_eq!(cache.auth_cooldown_until(&scope, 1_000).await, Some(90_000));
        assert!(cache.auth_cooldown_until(&scope, 90_000).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
        parent.configure_limit(Some(0));
        cache
            .insert(scope.clone(), "rejected-token".to_string(), 100_000)
            .await;
        assert!(cache.get(&scope, 1_000).await.is_none());
        assert!(cache
            .lock(&scope)
            .await
            .unwrap_err()
            .is_request_memory_exhausted());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn cursor_r4_catalog_pressure_never_restores_an_old_authorization() {
        let parent = ResidentMemoryBudget::new(None);
        let cache = CursorModelCatalogCache::with_parent(parent.clone());
        let scope = catalog_scope(1);
        cache
            .insert(
                scope.clone(),
                vec!["old-fast-model".to_string()],
                1_000,
                100,
            )
            .await;
        assert!(parent.snapshot().used_bytes > 0);
        parent.configure_limit(Some(0));
        let empty = cache.insert(scope.clone(), Vec::new(), 2_000, 100).await;
        assert!(empty.models.is_empty());
        assert!(cache.fresh(&scope, 2_001).await.is_none());
        assert!(cache.last_known_good(&scope, 2_001).await.is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
        parent.configure_limit(None);
        cache
            .insert(scope.clone(), vec!["new-model".to_string()], 3_000, 100)
            .await;
        assert!(cache
            .last_known_good(&scope, 3_000 + CURSOR_MODEL_STALE_TTL_MS)
            .await
            .is_none());
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    #[tokio::test]
    async fn cursor_r4_rotated_caches_drop_all_process_owners() {
        let parent = ResidentMemoryBudget::new(None);
        let tokens = CursorApiKeyTokenCache::with_parent(parent.clone());
        let catalogs = CursorModelCatalogCache::with_parent(parent.clone());
        for index in 0..MAX_CREDENTIAL_CACHE_ENTRIES + 4 {
            tokens
                .insert(
                    token_scope(index),
                    "token".to_string(),
                    100_000 + index as i64,
                )
                .await;
            tokens
                .mark_auth_cooldown(&token_scope(index), 90_000 + index as i64)
                .await;
            catalogs
                .insert(
                    catalog_scope(index),
                    vec!["model".to_string()],
                    index as i64,
                    100,
                )
                .await;
        }
        assert_eq!(
            parent.snapshot().active_reservations,
            MAX_CREDENTIAL_CACHE_ENTRIES * 3
        );
        assert!(tokens.get(&token_scope(0), 1).await.is_none());
        assert!(catalogs.fresh(&catalog_scope(0), 1).await.is_none());
        assert!(tokens
            .auth_cooldown_until(&token_scope(0), 1)
            .await
            .is_none());
        drop((tokens, catalogs));
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().active_reservations, 0);
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

        drop(tokens.lock(&token_scope(90)).await.unwrap());
        drop(tokens.lock(&token_scope(91)).await.unwrap());
        assert_eq!(tokens.flights.len().await, 1);
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
