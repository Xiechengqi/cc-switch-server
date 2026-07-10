use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::infra::time::now_ms;

const FAILOVER_FILE_NAME: &str = "failover.json";
const DEFAULT_FAILURE_THRESHOLD: u32 = 2;
const DEFAULT_OPEN_DURATION_MS: u128 = 5 * 60 * 1000;
const DEFAULT_HALF_OPEN_MAX_PROBES: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverStore {
    #[serde(default)]
    pub apps: BTreeMap<AppKind, FailoverAppConfig>,
    #[serde(default)]
    pub breakers: BTreeMap<String, ProviderBreaker>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverAppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider_queue: Vec<String>,
    #[serde(default = "default_failure_threshold")]
    pub failure_threshold: u32,
    #[serde(default = "default_open_duration_ms")]
    pub open_duration_ms: u128,
    #[serde(default = "default_half_open_max_probes")]
    pub half_open_max_probes: u32,
}

impl Default for FailoverAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_queue: Vec::new(),
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            open_duration_ms: DEFAULT_OPEN_DURATION_MS,
            half_open_max_probes: DEFAULT_HALF_OPEN_MAX_PROBES,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderBreaker {
    pub app: AppKind,
    pub provider_id: String,
    #[serde(default)]
    pub state: BreakerState,
    #[serde(default)]
    pub consecutive_failures: u32,
    #[serde(default)]
    pub opened_at_ms: Option<u128>,
    #[serde(default)]
    pub open_until_ms: Option<u128>,
    #[serde(default)]
    pub half_open_started_at_ms: Option<u128>,
    #[serde(default)]
    pub half_open_probe_count: u32,
    #[serde(default)]
    pub last_status_code: Option<u16>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub last_failure_at_ms: Option<u128>,
    #[serde(default)]
    pub last_success_at_ms: Option<u128>,
}

impl ProviderBreaker {
    pub fn new(app: AppKind, provider_id: &str) -> Self {
        Self {
            app,
            provider_id: provider_id.to_string(),
            state: BreakerState::Closed,
            consecutive_failures: 0,
            opened_at_ms: None,
            open_until_ms: None,
            half_open_started_at_ms: None,
            half_open_probe_count: 0,
            last_status_code: None,
            last_error: None,
            last_failure_at_ms: None,
            last_success_at_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakerState {
    #[default]
    Closed,
    Open,
    HalfOpen,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailoverSnapshot {
    pub apps: BTreeMap<AppKind, FailoverAppConfig>,
    pub breakers: Vec<ProviderBreaker>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateFailoverAppInput {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub provider_queue: Option<Vec<String>>,
    #[serde(default)]
    pub failure_threshold: Option<u32>,
    #[serde(default)]
    pub open_duration_ms: Option<u128>,
    #[serde(default)]
    pub half_open_max_probes: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOutcome {
    Success {
        status_code: u16,
    },
    Failure {
        status_code: u16,
    },
    RateLimited {
        status_code: u16,
        open_until_ms: Option<u128>,
    },
    NetworkFailure,
}

impl ProviderOutcome {
    pub fn from_status(status_code: u16) -> Self {
        if should_trip_status(status_code) {
            Self::Failure { status_code }
        } else {
            Self::Success { status_code }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FailoverSelection<'a> {
    pub provider: &'a StoredProvider,
    pub state_changed: bool,
}

impl FailoverStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = failover_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("read failover {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse failover {}", path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = failover_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write failover {}", path.display()))
    }

    pub fn snapshot_for_providers(&self, providers: &ProviderStore) -> FailoverSnapshot {
        let mut breakers = self.breakers.values().cloned().collect::<Vec<_>>();
        for provider in &providers.providers {
            let key = breaker_key(provider.app, &provider.provider.id);
            if !self.breakers.contains_key(&key) {
                breakers.push(ProviderBreaker::new(provider.app, &provider.provider.id));
            }
        }
        breakers.sort_by(|left, right| {
            left.app
                .as_str()
                .cmp(right.app.as_str())
                .then(left.provider_id.cmp(&right.provider_id))
        });
        FailoverSnapshot {
            apps: self.apps.clone(),
            breakers,
        }
    }

    pub fn update_app_config(
        &mut self,
        app: AppKind,
        input: UpdateFailoverAppInput,
        providers: &ProviderStore,
    ) -> FailoverAppConfig {
        let mut config = self.apps.get(&app).cloned().unwrap_or_default();
        if let Some(enabled) = input.enabled {
            config.enabled = enabled;
        }
        if let Some(queue) = input.provider_queue {
            config.provider_queue = normalized_queue(app, queue, providers);
        }
        if let Some(threshold) = input.failure_threshold {
            config.failure_threshold = threshold.max(1);
        }
        if let Some(open_duration_ms) = input.open_duration_ms {
            config.open_duration_ms = open_duration_ms.max(1);
        }
        if let Some(probes) = input.half_open_max_probes {
            config.half_open_max_probes = probes.max(1);
        }
        self.apps.insert(app, config.clone());
        config
    }

    pub fn app_enabled(&self, app: AppKind) -> bool {
        self.apps.get(&app).is_some_and(|config| config.enabled)
    }

    pub fn select_provider<'a>(
        &mut self,
        app: AppKind,
        candidates: &'a [&StoredProvider],
        now_ms: u128,
    ) -> Option<FailoverSelection<'a>> {
        self.select_provider_with_load(app, candidates, now_ms, |_| 0)
    }

    pub fn select_provider_with_load<'a>(
        &mut self,
        app: AppKind,
        candidates: &'a [&StoredProvider],
        now_ms: u128,
        load: impl Fn(&StoredProvider) -> u64,
    ) -> Option<FailoverSelection<'a>> {
        let config = self.apps.get(&app).cloned().unwrap_or_default();
        if !config.enabled {
            return candidates
                .iter()
                .copied()
                .min_by_key(|provider| load(provider))
                .map(|provider| FailoverSelection {
                    provider,
                    state_changed: false,
                });
        }

        let queue = provider_queue_for_candidates(&config.provider_queue, candidates);
        let mut fallback = None;
        let mut selected = None;
        let mut selected_load = u64::MAX;
        for provider_id in queue {
            let Some(provider) = candidates
                .iter()
                .copied()
                .find(|item| item.provider.id == provider_id)
            else {
                continue;
            };
            fallback.get_or_insert(provider);
            if !self.provider_can_be_selected(provider, &config, now_ms) {
                continue;
            }
            let provider_load = load(provider);
            if provider_load < selected_load {
                selected = Some(provider);
                selected_load = provider_load;
            }
        }

        if let Some(provider) = selected {
            if let ProviderAvailability::Available { state_changed } =
                self.provider_availability_for_selection(provider, &config, now_ms)
            {
                return Some(FailoverSelection {
                    provider,
                    state_changed,
                });
            }
        }

        fallback.map(|provider| FailoverSelection {
            provider,
            state_changed: false,
        })
    }

    pub fn record_outcome(
        &mut self,
        app: AppKind,
        provider_id: &str,
        outcome: ProviderOutcome,
        now_ms: u128,
    ) -> bool {
        let config = self.apps.get(&app).cloned().unwrap_or_default();
        if !config.enabled {
            return false;
        }
        match outcome {
            ProviderOutcome::Success { status_code } => {
                let Some(breaker) = self.breakers.get_mut(&breaker_key(app, provider_id)) else {
                    return false;
                };
                let should_persist = breaker.state != BreakerState::Closed
                    || breaker.consecutive_failures > 0
                    || breaker.opened_at_ms.is_some()
                    || breaker.open_until_ms.is_some()
                    || breaker.half_open_started_at_ms.is_some()
                    || breaker.half_open_probe_count > 0
                    || breaker.last_error.is_some();
                breaker.state = BreakerState::Closed;
                breaker.consecutive_failures = 0;
                breaker.opened_at_ms = None;
                breaker.open_until_ms = None;
                breaker.half_open_started_at_ms = None;
                breaker.half_open_probe_count = 0;
                breaker.last_status_code = Some(status_code);
                breaker.last_error = None;
                breaker.last_success_at_ms = Some(now_ms);
                should_persist
            }
            ProviderOutcome::Failure { status_code } => {
                let breaker = self.breaker_mut(app, provider_id);
                record_failure(breaker, &config, now_ms, Some(status_code), None, None);
                true
            }
            ProviderOutcome::RateLimited {
                status_code,
                open_until_ms,
            } => {
                let breaker = self.breaker_mut(app, provider_id);
                record_failure(
                    breaker,
                    &config,
                    now_ms,
                    Some(status_code),
                    None,
                    open_until_ms,
                );
                true
            }
            ProviderOutcome::NetworkFailure => {
                let breaker = self.breaker_mut(app, provider_id);
                record_failure(
                    breaker,
                    &config,
                    now_ms,
                    None,
                    Some("network failure".to_string()),
                    None,
                );
                true
            }
        }
    }

    pub fn reset_provider(&mut self, app: AppKind, provider_id: &str) -> ProviderBreaker {
        let breaker = ProviderBreaker::new(app, provider_id);
        self.breakers
            .insert(breaker_key(app, provider_id), breaker.clone());
        breaker
    }

    fn provider_availability_for_selection(
        &mut self,
        provider: &StoredProvider,
        config: &FailoverAppConfig,
        now_ms: u128,
    ) -> ProviderAvailability {
        let Some(breaker) = self
            .breakers
            .get_mut(&breaker_key(provider.app, &provider.provider.id))
        else {
            return ProviderAvailability::Available {
                state_changed: false,
            };
        };
        match breaker.state {
            BreakerState::Closed => ProviderAvailability::Available {
                state_changed: false,
            },
            BreakerState::Open => {
                let open_until_ms = breaker.open_until_ms.unwrap_or_else(|| {
                    breaker
                        .opened_at_ms
                        .unwrap_or(now_ms)
                        .saturating_add(config.open_duration_ms)
                });
                if now_ms < open_until_ms {
                    return ProviderAvailability::Unavailable;
                }
                breaker.state = BreakerState::HalfOpen;
                breaker.half_open_started_at_ms = Some(now_ms);
                breaker.open_until_ms = None;
                breaker.half_open_probe_count = 1;
                ProviderAvailability::Available {
                    state_changed: true,
                }
            }
            BreakerState::HalfOpen => {
                if breaker.half_open_probe_count >= config.half_open_max_probes {
                    return ProviderAvailability::Unavailable;
                }
                breaker.half_open_probe_count = breaker.half_open_probe_count.saturating_add(1);
                ProviderAvailability::Available {
                    state_changed: true,
                }
            }
        }
    }

    fn provider_can_be_selected(
        &self,
        provider: &StoredProvider,
        config: &FailoverAppConfig,
        now_ms: u128,
    ) -> bool {
        let Some(breaker) = self
            .breakers
            .get(&breaker_key(provider.app, &provider.provider.id))
        else {
            return true;
        };
        match breaker.state {
            BreakerState::Closed => true,
            BreakerState::Open => {
                let open_until_ms = breaker.open_until_ms.unwrap_or_else(|| {
                    breaker
                        .opened_at_ms
                        .unwrap_or(now_ms)
                        .saturating_add(config.open_duration_ms)
                });
                now_ms >= open_until_ms
            }
            BreakerState::HalfOpen => breaker.half_open_probe_count < config.half_open_max_probes,
        }
    }

    fn breaker_mut(&mut self, app: AppKind, provider_id: &str) -> &mut ProviderBreaker {
        let key = breaker_key(app, provider_id);
        self.breakers
            .entry(key)
            .or_insert_with(|| ProviderBreaker::new(app, provider_id))
    }
}

enum ProviderAvailability {
    Available { state_changed: bool },
    Unavailable,
}

pub fn failover_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(FAILOVER_FILE_NAME)
}

pub fn should_trip_status(status_code: u16) -> bool {
    status_code == 429 || (500..=599).contains(&status_code)
}

pub fn current_time_ms() -> u128 {
    now_ms()
}

fn record_failure(
    breaker: &mut ProviderBreaker,
    config: &FailoverAppConfig,
    now_ms: u128,
    status_code: Option<u16>,
    error: Option<String>,
    open_until_ms: Option<u128>,
) {
    breaker.consecutive_failures = breaker.consecutive_failures.saturating_add(1);
    breaker.last_status_code = status_code;
    breaker.last_error = error;
    breaker.last_failure_at_ms = Some(now_ms);
    if breaker.state == BreakerState::HalfOpen
        || breaker.consecutive_failures >= config.failure_threshold
    {
        breaker.state = BreakerState::Open;
        breaker.opened_at_ms = Some(now_ms);
        breaker.open_until_ms = open_until_ms;
        breaker.half_open_started_at_ms = None;
        breaker.half_open_probe_count = 0;
    }
}

fn provider_queue_for_candidates(
    configured_queue: &[String],
    candidates: &[&StoredProvider],
) -> Vec<String> {
    let mut queue = configured_queue
        .iter()
        .filter(|provider_id| {
            candidates
                .iter()
                .any(|candidate| candidate.provider.id == provider_id.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    for candidate in candidates {
        if !queue.iter().any(|id| id == &candidate.provider.id) {
            queue.push(candidate.provider.id.clone());
        }
    }
    queue
}

fn normalized_queue(app: AppKind, queue: Vec<String>, providers: &ProviderStore) -> Vec<String> {
    let mut output = Vec::new();
    for provider_id in queue
        .into_iter()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
    {
        if output.iter().any(|existing| existing == &provider_id) {
            continue;
        }
        if providers
            .providers
            .iter()
            .any(|provider| provider.app == app && provider.provider.id == provider_id)
        {
            output.push(provider_id);
        }
    }
    output
}

fn breaker_key(app: AppKind, provider_id: &str) -> String {
    format!("{}:{provider_id}", app.as_str())
}

fn default_failure_threshold() -> u32 {
    DEFAULT_FAILURE_THRESHOLD
}

fn default_open_duration_ms() -> u128 {
    DEFAULT_OPEN_DURATION_MS
}

fn default_half_open_max_probes() -> u32 {
    DEFAULT_HALF_OPEN_MAX_PROBES
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::providers::model::{Provider, ProviderType};

    fn provider(app: AppKind, id: &str) -> StoredProvider {
        StoredProvider {
            app,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type: ProviderType::Codex,
            provider_type_id: "codex".to_string(),
        }
    }

    #[test]
    fn consecutive_failures_open_breaker_and_skip_provider() {
        let mut store = FailoverStore::default();
        store.update_app_config(
            AppKind::Codex,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p1".to_string(), "p2".to_string()]),
                failure_threshold: Some(2),
                open_duration_ms: Some(1_000),
                half_open_max_probes: Some(1),
            },
            &ProviderStore {
                providers: vec![
                    provider(AppKind::Codex, "p1"),
                    provider(AppKind::Codex, "p2"),
                ],
            },
        );
        store.record_outcome(
            AppKind::Codex,
            "p1",
            ProviderOutcome::Failure { status_code: 429 },
            100,
        );
        store.record_outcome(
            AppKind::Codex,
            "p1",
            ProviderOutcome::Failure { status_code: 500 },
            200,
        );
        let p1 = provider(AppKind::Codex, "p1");
        let p2 = provider(AppKind::Codex, "p2");
        let candidates = [&p1, &p2];

        let selected = store
            .select_provider(AppKind::Codex, &candidates, 300)
            .unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
        assert_eq!(store.breakers["codex:p1"].state, BreakerState::Open);
    }

    #[test]
    fn half_open_success_closes_breaker() {
        let p1 = provider(AppKind::Codex, "p1");
        let providers = ProviderStore {
            providers: vec![p1.clone()],
        };
        let mut store = FailoverStore::default();
        store.update_app_config(
            AppKind::Codex,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p1".to_string()]),
                failure_threshold: Some(1),
                open_duration_ms: Some(100),
                half_open_max_probes: Some(1),
            },
            &providers,
        );
        store.record_outcome(AppKind::Codex, "p1", ProviderOutcome::NetworkFailure, 100);
        let candidates = [&p1];

        let selection = store
            .select_provider(AppKind::Codex, &candidates, 250)
            .unwrap();
        assert!(selection.state_changed);
        assert_eq!(store.breakers["codex:p1"].state, BreakerState::HalfOpen);

        store.record_outcome(
            AppKind::Codex,
            "p1",
            ProviderOutcome::Success { status_code: 200 },
            300,
        );

        assert_eq!(store.breakers["codex:p1"].state, BreakerState::Closed);
        assert_eq!(store.breakers["codex:p1"].consecutive_failures, 0);
    }

    #[test]
    fn explicit_disabled_config_selects_first_candidate() {
        let mut store = FailoverStore::default();
        let p1 = provider(AppKind::Claude, "p1");
        let p2 = provider(AppKind::Claude, "p2");
        let candidates = [&p1, &p2];

        let selected = store
            .select_provider(AppKind::Claude, &candidates, 100)
            .unwrap();

        assert_eq!(selected.provider.provider.id, "p1");
        assert!(!selected.state_changed);
    }

    #[test]
    fn closed_success_without_breaker_does_not_create_hot_path_state() {
        let mut store = FailoverStore::default();
        let providers = ProviderStore {
            providers: vec![provider(AppKind::Codex, "p1")],
        };
        store.update_app_config(
            AppKind::Codex,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p1".to_string()]),
                failure_threshold: None,
                open_duration_ms: None,
                half_open_max_probes: None,
            },
            &providers,
        );

        let persisted = store.record_outcome(
            AppKind::Codex,
            "p1",
            ProviderOutcome::Success { status_code: 200 },
            100,
        );

        assert!(!persisted);
        assert!(store.breakers.is_empty());
    }

    #[test]
    fn all_open_fallback_uses_configured_queue_order() {
        let p1 = provider(AppKind::Codex, "p1");
        let p2 = provider(AppKind::Codex, "p2");
        let providers = ProviderStore {
            providers: vec![p1.clone(), p2.clone()],
        };
        let mut store = FailoverStore::default();
        store.update_app_config(
            AppKind::Codex,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p2".to_string(), "p1".to_string()]),
                failure_threshold: Some(1),
                open_duration_ms: Some(1_000),
                half_open_max_probes: Some(1),
            },
            &providers,
        );
        store.record_outcome(AppKind::Codex, "p1", ProviderOutcome::NetworkFailure, 100);
        store.record_outcome(AppKind::Codex, "p2", ProviderOutcome::NetworkFailure, 100);
        let candidates = [&p1, &p2];

        let selected = store
            .select_provider(AppKind::Codex, &candidates, 200)
            .unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
        assert!(!selected.state_changed);
    }
}
