use std::fs;
use std::path::Path;

use anyhow::Context;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::providers::model::{classify_provider, AppKind, Provider, ProviderType};

const PROVIDERS_FILE_NAME: &str = "providers.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStore {
    #[serde(default)]
    pub providers: Vec<StoredProvider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProvider {
    pub app: AppKind,
    pub provider: Provider,
    pub provider_type: ProviderType,
    pub provider_type_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSortUpdate {
    pub id: String,
    pub sort_index: usize,
}

impl ProviderStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let providers_path = providers_path(config_dir);
        if !providers_path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&providers_path)
            .with_context(|| format!("read providers {}", providers_path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("parse providers {}", providers_path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;

        let providers_path = providers_path(config_dir);
        crate::infra::storage::write_json_pretty(&providers_path, self)
            .with_context(|| format!("write providers {}", providers_path.display()))
    }

    pub fn upsert(&mut self, app: AppKind, mut provider: Provider) -> StoredProvider {
        if provider.id.trim().is_empty() {
            provider.id = generate_provider_id(app);
        }

        let provider_type = classify_provider(app, &provider);
        let stored = StoredProvider {
            app,
            provider,
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
        };

        if let Some(existing) = self
            .providers
            .iter_mut()
            .find(|item| item.app == stored.app && item.provider.id == stored.provider.id)
        {
            *existing = stored.clone();
        } else {
            self.providers.push(stored.clone());
        }

        stored
    }

    pub fn list(&self, app: Option<AppKind>) -> Vec<StoredProvider> {
        let mut providers = self
            .providers
            .iter()
            .filter(|item| app.is_none_or(|app| item.app == app))
            .enumerate()
            .map(|(index, provider)| (index, provider.clone()))
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| {
            let left_sort = provider_sort_index(&left.1).unwrap_or(usize::MAX);
            let right_sort = provider_sort_index(&right.1).unwrap_or(usize::MAX);
            left_sort
                .cmp(&right_sort)
                .then_with(|| left.0.cmp(&right.0))
        });
        providers
            .into_iter()
            .map(|(_, provider)| provider)
            .collect()
    }

    pub fn update_sort_order(&mut self, app: AppKind, updates: Vec<ProviderSortUpdate>) -> bool {
        let mut changed = false;
        for update in updates {
            if let Some(provider) = self
                .providers
                .iter_mut()
                .find(|provider| provider.app == app && provider.provider.id == update.id)
            {
                let previous = provider_sort_index(provider);
                if previous != Some(update.sort_index) {
                    provider.provider.extra.insert(
                        "sortIndex".to_string(),
                        Value::from(update.sort_index as u64),
                    );
                    changed = true;
                }
            }
        }
        changed
    }
}

pub fn provider_sort_index(provider: &StoredProvider) -> Option<usize> {
    provider
        .provider
        .extra
        .get("sortIndex")
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .and_then(|value| usize::try_from(value).ok())
}

pub fn providers_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(PROVIDERS_FILE_NAME)
}

fn generate_provider_id(app: AppKind) -> String {
    let prefix = match app {
        AppKind::Claude => "claude",
        AppKind::Codex => "codex",
        AppKind::Gemini => "gemini",
    };
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("{prefix}-{suffix}")
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    #[test]
    fn upsert_classifies_and_replaces_provider() {
        let mut store = ProviderStore::default();
        let stored = store.upsert(
            AppKind::Claude,
            Provider {
                id: "p1".to_string(),
                name: "OpenRouter".to_string(),
                settings_config: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": "https://openrouter.ai/api"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        assert_eq!(stored.provider_type, ProviderType::OpenRouter);
        assert_eq!(store.providers.len(), 1);

        store.upsert(
            AppKind::Claude,
            Provider {
                id: "p1".to_string(),
                name: "Relay".to_string(),
                settings_config: json!({"auth_mode": "bearer_only"}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        );

        assert_eq!(store.providers.len(), 1);
        assert_eq!(store.providers[0].provider_type, ProviderType::ClaudeAuth);
    }

    #[test]
    fn saves_and_loads_provider_store() {
        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-server-provider-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let mut store = ProviderStore::default();
        store.upsert(
            AppKind::Codex,
            Provider {
                id: "codex-1".to_string(),
                name: "OpenAI OAuth".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(crate::domain::providers::model::ProviderMeta {
                    provider_type: Some("codex_oauth".to_string()),
                    ..Default::default()
                }),
                extra: Default::default(),
            },
        );

        store.save(&config_dir).unwrap();
        let loaded = ProviderStore::load_or_default(&config_dir).unwrap();
        fs::remove_dir_all(&config_dir).unwrap();

        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.providers[0].provider_type, ProviderType::CodexOAuth);
    }
}
