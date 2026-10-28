use std::sync::{Mutex, OnceLock};

use rand::RngCore;
use serde::{Deserialize, Serialize};

const VERSION_ENV: &str = "CC_SWITCH_ANTIGRAVITY_CLIENT_VERSION";
const FALLBACK_VERSION: &str = "2.2.1";
const HUB_PLATFORM: &str = "darwin/arm64";

/// One immutable request identity. The opaque generation also fences A -> B -> A
/// transitions and persisted observations from a previous server process.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AntigravityClientIdentity {
    pub version: String,
    pub user_agent: String,
    pub generation: String,
}

impl AntigravityClientIdentity {
    fn new(version: &str) -> Self {
        let mut generation = [0_u8; 16];
        rand::thread_rng().fill_bytes(&mut generation);
        Self {
            version: version.to_string(),
            user_agent: format!("antigravity/hub/{version} {HUB_PLATFORM}"),
            generation: hex::encode(generation),
        }
    }

    pub fn is_current(&self) -> bool {
        self == &antigravity_client_identity()
    }
}

struct IdentityState {
    cached_version: String,
    current: AntigravityClientIdentity,
}

impl IdentityState {
    fn new(version: &str) -> Self {
        Self {
            cached_version: version.to_string(),
            current: AntigravityClientIdentity::new(version),
        }
    }

    fn capture(&mut self, configured: Option<&str>) -> AntigravityClientIdentity {
        let effective = configured.unwrap_or(&self.cached_version);
        if self.current.version != effective {
            self.current = AntigravityClientIdentity::new(effective);
        }
        self.current.clone()
    }

    fn update(&mut self, version: &str, configured: Option<&str>) -> bool {
        if !valid_antigravity_version(version) {
            return false;
        }
        self.cached_version = version.to_string();
        self.capture(configured);
        true
    }
}

fn state() -> &'static Mutex<IdentityState> {
    static STATE: OnceLock<Mutex<IdentityState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(IdentityState::new(FALLBACK_VERSION)))
}

pub fn configured_antigravity_version() -> Option<String> {
    std::env::var(VERSION_ENV)
        .ok()
        .map(|version| version.trim().to_string())
        .filter(|version| valid_antigravity_version(version))
}

pub fn valid_antigravity_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b".-_+".contains(&byte))
}

pub fn antigravity_client_identity() -> AntigravityClientIdentity {
    #[cfg(test)]
    if let Ok(snapshot) = TEST_IDENTITY.try_with(|state| {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .capture(None)
    }) {
        return snapshot;
    }
    let configured = configured_antigravity_version();
    state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .capture(configured.as_deref())
}

pub(crate) fn update_cached_antigravity_version(version: &str) -> bool {
    let configured = configured_antigravity_version();
    state()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .update(version, configured.as_deref())
}

#[cfg(test)]
tokio::task_local! {
    static TEST_IDENTITY: std::sync::Arc<Mutex<IdentityState>>;
}

/// Test-local version changes never mutate process environment or another test.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct TestAntigravityIdentity(std::sync::Arc<Mutex<IdentityState>>);

#[cfg(test)]
impl TestAntigravityIdentity {
    pub(crate) fn new(version: &str) -> Self {
        assert!(valid_antigravity_version(version));
        Self(std::sync::Arc::new(Mutex::new(IdentityState::new(version))))
    }

    pub(crate) fn update(&self, version: &str) -> bool {
        self.0.lock().unwrap().update(version, None)
    }

    pub(crate) async fn scope<F: std::future::Future>(&self, future: F) -> F::Output {
        TEST_IDENTITY
            .scope(std::sync::Arc::clone(&self.0), future)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn antigravity_r4_identity_rotates_only_after_successful_effective_version_changes() {
        let versions = TestAntigravityIdentity::new("2.2.1");
        versions
            .scope(async {
                let first = antigravity_client_identity();
                assert!(versions.update("2.2.1"));
                assert_eq!(first, antigravity_client_identity());
                assert!(!versions.update("invalid version"));
                assert_eq!(first, antigravity_client_identity());
                assert!(versions.update("2.3.4"));
                let second = antigravity_client_identity();
                assert_ne!(first.generation, second.generation);
                assert_eq!(second.user_agent, "antigravity/hub/2.3.4 darwin/arm64");
                assert!(!first.is_current());
                assert!(versions.update("2.2.1"));
                assert_ne!(first, antigravity_client_identity());
                assert_eq!(first.user_agent, antigravity_client_identity().user_agent);
            })
            .await;
    }

    #[test]
    fn antigravity_r4_configured_version_and_restart_cannot_relabel_old_generations() {
        let mut state = IdentityState::new("2.2.1");
        let configured = state.capture(Some("2.4.0"));
        assert!(state.update("2.3.0", Some("2.4.0")));
        assert_eq!(state.capture(Some("2.4.0")), configured);
        let automatic = state.capture(None);
        assert_eq!(automatic.version, "2.3.0");
        assert_ne!(automatic.generation, configured.generation);
        assert_ne!(automatic, IdentityState::new("2.3.0").capture(None));
    }
}
