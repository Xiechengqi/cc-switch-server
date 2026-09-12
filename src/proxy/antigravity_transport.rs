use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::RuntimeTransportPolicy;

const DEFAULT_POOL_IDLE_TIMEOUT_MS: u64 = 30_000;
const MAX_POOL_IDLE_TIMEOUT_MS: u64 = 210_000;
const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 2;
const MAX_POOL_MAX_IDLE_PER_HOST: usize = 64;
const MAX_CACHED_CLIENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AntigravityTransportPolicy {
    pub(crate) connection_pool_enabled: bool,
    pub(crate) pool_idle_timeout_ms: u64,
    pub(crate) pool_max_idle_per_host: usize,
}

impl AntigravityTransportPolicy {
    pub(crate) fn resolve(runtime: &RuntimeTransportPolicy) -> Self {
        Self {
            connection_pool_enabled: runtime.connection_pool_enabled.unwrap_or(false),
            pool_idle_timeout_ms: runtime
                .pool_idle_timeout_ms
                .unwrap_or(DEFAULT_POOL_IDLE_TIMEOUT_MS)
                .clamp(1_000, MAX_POOL_IDLE_TIMEOUT_MS),
            pool_max_idle_per_host: runtime
                .pool_max_idle_per_host
                .unwrap_or(DEFAULT_POOL_MAX_IDLE_PER_HOST)
                .clamp(1, MAX_POOL_MAX_IDLE_PER_HOST),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct AntigravityTransportKey {
    app: String,
    provider_type: String,
    provider_id: String,
    provider_revision: u64,
    runtime_fingerprint: String,
    account_id: String,
    auth_identity_generation: u64,
    pool_idle_timeout_ms: u64,
    pool_max_idle_per_host: usize,
}

impl fmt::Debug for AntigravityTransportKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AntigravityTransportKey")
            .field("app", &self.app)
            .field("provider_type", &self.provider_type)
            .field("provider_id", &self.provider_id)
            .field("provider_revision", &self.provider_revision)
            .field("runtime_fingerprint", &"<redacted>")
            .field("account_id", &"<redacted>")
            .field("auth_identity_generation", &self.auth_identity_generation)
            .field("pool_idle_timeout_ms", &self.pool_idle_timeout_ms)
            .field("pool_max_idle_per_host", &self.pool_max_idle_per_host)
            .finish()
    }
}

impl AntigravityTransportKey {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        app: AppKind,
        provider_type: ProviderType,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        auth_identity_generation: u64,
        policy: AntigravityTransportPolicy,
    ) -> Option<Self> {
        if !matches!(
            provider_type,
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
        ) || provider_id.trim().is_empty()
            || runtime_fingerprint.trim().is_empty()
            || account_id.trim().is_empty()
            || !policy.connection_pool_enabled
        {
            return None;
        }
        Some(Self {
            app: app.as_str().to_string(),
            provider_type: provider_type.as_str().to_string(),
            provider_id: provider_id.to_string(),
            provider_revision,
            runtime_fingerprint: runtime_fingerprint.to_string(),
            account_id: account_id.to_string(),
            auth_identity_generation,
            pool_idle_timeout_ms: policy.pool_idle_timeout_ms,
            pool_max_idle_per_host: policy.pool_max_idle_per_host,
        })
    }
}

struct CachedClient {
    client: reqwest::Client,
    last_used: u64,
}

#[derive(Default)]
struct CacheState {
    clients: HashMap<AntigravityTransportKey, CachedClient>,
    clock: u64,
}

#[derive(Default)]
pub(crate) struct AntigravityTransportCache {
    state: Mutex<CacheState>,
}

impl fmt::Debug for AntigravityTransportCache {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AntigravityTransportCache(<credential-scoped>)")
    }
}

impl AntigravityTransportCache {
    pub(crate) async fn client(
        &self,
        key: Option<AntigravityTransportKey>,
        policy: AntigravityTransportPolicy,
    ) -> anyhow::Result<reqwest::Client> {
        let Some(key) = key else {
            crate::metrics::record_antigravity_transport("short", "new_client");
            return build_client(policy);
        };
        let mut state = self.state.lock().await;
        state.clock = state.clock.saturating_add(1).max(1);
        let now = state.clock;
        if let Some(cached) = state.clients.get_mut(&key) {
            cached.last_used = now;
            crate::metrics::record_antigravity_transport("pooled", "cache_hit");
            return Ok(cached.client.clone());
        }
        let client = build_client(policy)?;
        if state.clients.len() >= MAX_CACHED_CLIENTS {
            if let Some(oldest) = state
                .clients
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(key, _)| key.clone())
            {
                state.clients.remove(&oldest);
            }
        }
        state.clients.insert(
            key,
            CachedClient {
                client: client.clone(),
                last_used: now,
            },
        );
        crate::metrics::record_antigravity_transport("pooled", "cache_miss");
        Ok(client)
    }

    #[cfg(test)]
    async fn len(&self) -> usize {
        self.state.lock().await.clients.len()
    }
}

fn build_client(policy: AntigravityTransportPolicy) -> anyhow::Result<reqwest::Client> {
    let mut builder = crate::infra::http::outbound_client_builder()?
        .connect_timeout(Duration::from_secs(30))
        .http1_only()
        .no_gzip();
    if policy.connection_pool_enabled {
        builder = builder
            .pool_idle_timeout(Duration::from_millis(policy.pool_idle_timeout_ms))
            .pool_max_idle_per_host(policy.pool_max_idle_per_host)
            .tcp_keepalive(Duration::from_secs(60));
    } else {
        builder = builder.pool_max_idle_per_host(0);
    }
    builder.build().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex as StdMutex};

    use axum::extract::{ConnectInfo, State};
    use axum::routing::get;
    use axum::Router;

    use super::*;

    fn runtime(
        enabled: Option<bool>,
        idle: Option<u64>,
        max_idle: Option<usize>,
    ) -> RuntimeTransportPolicy {
        RuntimeTransportPolicy {
            connection_pool_enabled: enabled,
            pool_idle_timeout_ms: idle,
            pool_max_idle_per_host: max_idle,
            ..RuntimeTransportPolicy::default()
        }
    }

    fn key(
        fingerprint: &str,
        account: &str,
        generation: u64,
        policy: AntigravityTransportPolicy,
    ) -> AntigravityTransportKey {
        AntigravityTransportKey::new(
            AppKind::Gemini,
            ProviderType::AntigravityOAuth,
            "provider",
            7,
            fingerprint,
            account,
            generation,
            policy,
        )
        .unwrap()
    }

    #[test]
    fn defaults_to_short_connections_and_bounds_pool_settings() {
        let short = AntigravityTransportPolicy::resolve(&runtime(None, None, None));
        assert!(!short.connection_pool_enabled);
        assert_eq!(short.pool_idle_timeout_ms, 30_000);
        assert_eq!(short.pool_max_idle_per_host, 2);
        assert!(AntigravityTransportKey::new(
            AppKind::Gemini,
            ProviderType::AntigravityOAuth,
            "provider",
            1,
            "runtime",
            "account",
            1,
            short,
        )
        .is_none());

        let bounded = AntigravityTransportPolicy::resolve(&runtime(
            Some(true),
            Some(u64::MAX),
            Some(usize::MAX),
        ));
        assert_eq!(bounded.pool_idle_timeout_ms, MAX_POOL_IDLE_TIMEOUT_MS);
        assert_eq!(bounded.pool_max_idle_per_host, MAX_POOL_MAX_IDLE_PER_HOST);
    }

    #[tokio::test]
    async fn cache_key_fences_runtime_account_generation_and_resolved_settings() {
        let cache = AntigravityTransportCache::default();
        let first_policy =
            AntigravityTransportPolicy::resolve(&runtime(Some(true), Some(30_000), Some(2)));
        let changed_policy =
            AntigravityTransportPolicy::resolve(&runtime(Some(true), Some(40_000), Some(2)));
        let first = key("runtime-a", "account-a", 1, first_policy);
        cache
            .client(Some(first.clone()), first_policy)
            .await
            .unwrap();
        cache.client(Some(first), first_policy).await.unwrap();
        assert_eq!(cache.len().await, 1);
        cache
            .client(
                Some(key("runtime-b", "account-a", 1, first_policy)),
                first_policy,
            )
            .await
            .unwrap();
        cache
            .client(
                Some(key("runtime-a", "account-b", 1, first_policy)),
                first_policy,
            )
            .await
            .unwrap();
        cache
            .client(
                Some(key("runtime-a", "account-a", 2, first_policy)),
                first_policy,
            )
            .await
            .unwrap();
        cache
            .client(
                Some(key("runtime-a", "account-a", 1, changed_policy)),
                changed_policy,
            )
            .await
            .unwrap();
        assert_eq!(cache.len().await, 5);
        assert!(
            !format!("{:?}", key("runtime-a", "account-a", 1, first_policy)).contains("account-a")
        );
    }

    #[tokio::test]
    async fn short_mode_rotates_connections_while_opt_in_pool_reuses_http11() {
        async fn observe(
            State(peers): State<Arc<StdMutex<Vec<SocketAddr>>>>,
            ConnectInfo(peer): ConnectInfo<SocketAddr>,
        ) -> &'static str {
            peers.lock().unwrap().push(peer);
            "ok"
        }

        let peers = Arc::new(StdMutex::new(Vec::new()));
        let app = Router::new()
            .route("/", get(observe))
            .with_state(Arc::clone(&peers));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        let cache = AntigravityTransportCache::default();
        let short = AntigravityTransportPolicy::resolve(&runtime(None, None, None));
        for _ in 0..2 {
            let response = cache
                .client(None, short)
                .await
                .unwrap()
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.version(), reqwest::Version::HTTP_11);
            let _ = response.bytes().await.unwrap();
        }
        let short_peers = peers.lock().unwrap().clone();
        assert_eq!(short_peers.len(), 2);
        assert_ne!(short_peers[0], short_peers[1]);

        peers.lock().unwrap().clear();
        let pooled =
            AntigravityTransportPolicy::resolve(&runtime(Some(true), Some(30_000), Some(2)));
        let pooled_key = key("pooled-runtime", "pooled-account", 1, pooled);
        for _ in 0..2 {
            let response = cache
                .client(Some(pooled_key.clone()), pooled)
                .await
                .unwrap()
                .get(format!("http://{address}/"))
                .send()
                .await
                .unwrap();
            assert_eq!(response.version(), reqwest::Version::HTTP_11);
            let _ = response.bytes().await.unwrap();
        }
        let pooled_peers = peers.lock().unwrap().clone();
        assert_eq!(pooled_peers.len(), 2);
        assert_eq!(pooled_peers[0], pooled_peers[1]);
        server.abort();
    }
}
