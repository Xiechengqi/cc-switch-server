use axum::http::HeaderMap;

use crate::domain::failover::{current_time_ms, FailoverStore};
use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::{ProviderStore, StoredProvider};

use super::ProxyError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyRoute {
    ClaudeMessages,
    CodexChatCompletions,
    CodexResponses,
    Gemini,
}

impl ProxyRoute {
    pub fn app(self) -> AppKind {
        match self {
            Self::ClaudeMessages => AppKind::Claude,
            Self::CodexChatCompletions | Self::CodexResponses => AppKind::Codex,
            Self::Gemini => AppKind::Gemini,
        }
    }

    pub fn path(self, gemini_path: Option<String>) -> String {
        match self {
            Self::ClaudeMessages => "/v1/messages".to_string(),
            Self::CodexChatCompletions => "/v1/chat/completions".to_string(),
            Self::CodexResponses => "/v1/responses".to_string(),
            Self::Gemini => format!("/v1beta/{}", gemini_path.unwrap_or_default()),
        }
    }
}

pub(super) struct ProviderRouteSelection {
    pub provider: StoredProvider,
    pub failover_state_changed: bool,
}

pub(super) fn select_provider(
    store: &ProviderStore,
    failover: &mut FailoverStore,
    app: AppKind,
    headers: &HeaderMap,
) -> Result<ProviderRouteSelection, ProxyError> {
    let provider_id = headers
        .get("x-cc-provider-id")
        .and_then(|value| value.to_str().ok());

    if let Some(provider_id) = provider_id {
        let provider = store
            .providers
            .iter()
            .find(|item| item.app == app && item.provider.id == provider_id)
            .cloned()
            .ok_or_else(|| ProxyError::not_found(format!("provider not found: {provider_id}")))?;
        return Ok(ProviderRouteSelection {
            provider,
            failover_state_changed: false,
        });
    }

    let candidates = store
        .providers
        .iter()
        .filter(|item| item.app == app)
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Err(ProxyError::not_found(format!(
            "no provider configured for {:?}",
            app
        )));
    }

    let selection = failover
        .select_provider(app, &candidates, current_time_ms())
        .ok_or_else(|| ProxyError::not_found(format!("no provider configured for {:?}", app)))?;
    Ok(ProviderRouteSelection {
        provider: selection.provider.clone(),
        failover_state_changed: selection.state_changed,
    })
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};
    use serde_json::json;

    use super::*;
    use crate::domain::failover::{FailoverStore, ProviderOutcome, UpdateFailoverAppInput};
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

    fn provider_store() -> ProviderStore {
        ProviderStore {
            providers: vec![
                provider(AppKind::Codex, "p1"),
                provider(AppKind::Codex, "p2"),
            ],
        }
    }

    fn enabled_failover(store: &ProviderStore) -> FailoverStore {
        let mut failover = FailoverStore::default();
        failover.update_app_config(
            AppKind::Codex,
            UpdateFailoverAppInput {
                enabled: Some(true),
                provider_queue: Some(vec!["p1".to_string(), "p2".to_string()]),
                failure_threshold: Some(1),
                open_duration_ms: Some(1_000),
                half_open_max_probes: Some(1),
            },
            store,
        );
        failover
    }

    #[test]
    fn explicit_provider_header_bypasses_failover_selection() {
        let store = provider_store();
        let mut failover = enabled_failover(&store);
        failover.record_outcome(AppKind::Codex, "p1", ProviderOutcome::NetworkFailure, 100);
        let mut headers = HeaderMap::new();
        headers.insert("x-cc-provider-id", HeaderValue::from_static("p1"));

        let selected = select_provider(&store, &mut failover, AppKind::Codex, &headers).unwrap();

        assert_eq!(selected.provider.provider.id, "p1");
        assert!(!selected.failover_state_changed);
    }

    #[test]
    fn automatic_selection_skips_open_breaker() {
        let store = provider_store();
        let mut failover = enabled_failover(&store);
        let now = current_time_ms();
        failover.record_outcome(AppKind::Codex, "p1", ProviderOutcome::NetworkFailure, now);

        let selected =
            select_provider(&store, &mut failover, AppKind::Codex, &HeaderMap::new()).unwrap();

        assert_eq!(selected.provider.provider.id, "p2");
        assert!(!selected.failover_state_changed);
    }
}
