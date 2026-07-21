use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::{classify_provider, AppKind, Provider, ProviderType};
use crate::domain::providers::model_routing::normalize_provider_model_routing;
use crate::domain::providers::registry::{
    profile_by_id, resolve_custom_binding, CustomBindingInput, DriverBinding, ProfileId,
    UpstreamProtocol,
};
use crate::domain::providers::runtime::{ProviderRuntimeIndex, ProviderRuntimePlan};

const PROVIDERS_FILE_NAME: &str = "providers.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStoreFormat {
    #[default]
    S1,
    S2,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStoreStatus {
    pub format: ProviderStoreFormat,
    pub schema_version: u32,
    pub store_generation: u64,
    pub credential_key_source: Option<crate::infra::credentials::CredentialKeySource>,
    pub committed_credentials_encrypted: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStore {
    #[serde(default)]
    pub providers: Vec<StoredProvider>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub order: BTreeMap<AppKind, Vec<String>>,
    #[serde(skip)]
    pub(crate) runtime_index: Arc<ProviderRuntimeIndex>,
    #[serde(skip)]
    pub(crate) format: ProviderStoreFormat,
    #[serde(skip)]
    pub(crate) store_generation: u64,
    #[serde(skip)]
    pub(crate) credential_vault: Arc<super::store_v2::ProviderCredentialVault>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProviderStoreS1<'a> {
    providers: &'a [StoredProvider],
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    order: &'a BTreeMap<AppKind, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProvider {
    pub app: AppKind,
    pub provider: Provider,
    pub provider_type: ProviderType,
    pub provider_type_id: String,
    #[serde(default, flatten)]
    pub resource: ProviderResourceMetadata,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderResourceMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<ProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_schema_revision: Option<u32>,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub revision: u64,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub credential_generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_binding: Option<CustomBindingInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_request_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSortUpdate {
    pub id: String,
    pub sort_index: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LegacyRuntimeViewReport {
    pub normalized_entries: usize,
    pub unresolved_entries: usize,
}

impl ProviderStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let providers_path = providers_path(config_dir);
        if !providers_path.exists() {
            return Ok(Self::default());
        }

        let content = fs::read_to_string(&providers_path)
            .with_context(|| format!("read providers {}", providers_path.display()))?;
        let value: Value = serde_json::from_str(&content)
            .with_context(|| format!("parse providers {}", providers_path.display()))?;
        if super::store_v2::looks_like_s2(&value) {
            let key = crate::infra::credentials::load_root_key(config_dir)
                .context("load key for Provider S2 store")?;
            return super::store_v2::decode_s2(value, key)
                .with_context(|| format!("parse providers {}", providers_path.display()));
        }
        serde_json::from_value(value)
            .with_context(|| format!("parse providers {}", providers_path.display()))
    }

    /// Builds the legacy execution view in memory without mutating the source file.
    pub fn prepare_legacy_runtime_view(&mut self) -> LegacyRuntimeViewReport {
        let mut report = LegacyRuntimeViewReport::default();
        let mut normalized = 0usize;
        for stored in &mut self.providers {
            let mut changed = false;
            let provider_type =
                canonical_provider_type(stored.app, &stored.provider, &stored.resource)
                    .unwrap_or_else(|_| classify_provider(stored.app, &stored.provider));
            if stored.provider_type != provider_type
                || stored.provider_type_id != provider_type.as_str()
            {
                stored.provider_type = provider_type;
                stored.provider_type_id = provider_type.as_str().to_string();
                changed = true;
            }
            let result = normalize_provider_model_routing(stored.app, &mut stored.provider);
            if result.changed {
                changed = true;
            }
            if changed {
                normalized += 1;
            }
            if result.required && !result.resolved {
                report.unresolved_entries += 1;
                tracing::warn!(
                    app = stored.app.as_str(),
                    provider_id = %stored.provider.id,
                    provider_name = %stored.provider.name,
                    "provider model routing could not be inferred; configure one actual upstream model"
                );
            }
        }
        report.normalized_entries = normalized;
        if report.normalized_entries > 0 {
            tracing::info!(
                normalized_entries = report.normalized_entries,
                unresolved_entries = report.unresolved_entries,
                "prepared legacy provider runtime view without modifying persisted providers"
            );
        }
        report
    }

    pub fn load_runtime_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let persisted = providers_path(config_dir).exists();
        let mut store = Self::load_or_default(config_dir)?;
        if !persisted {
            store.format = ProviderStoreFormat::S2;
            return Ok(store);
        }
        if store.format == ProviderStoreFormat::S1 {
            store.prepare_legacy_runtime_view();
            if !store.providers.is_empty() {
                let key = crate::infra::credentials::load_or_create_root_key(config_dir)
                    .context("resolve key for Provider runtime credential vault")?;
                super::store_v2::seal_store(&mut store, key)?;
            }
        }
        Ok(store)
    }

    pub fn rebuild_runtime_index(&mut self, accounts: &AccountStore) -> anyhow::Result<()> {
        self.runtime_index = Arc::new(ProviderRuntimeIndex::compile(self, accounts)?);
        Ok(())
    }

    pub fn runtime_plan(
        &self,
        app: AppKind,
        provider_id: &str,
    ) -> Option<Arc<ProviderRuntimePlan>> {
        self.runtime_index.get(app, provider_id)
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(config_dir, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("secure config dir {}", config_dir.display()))?;
        }

        let providers_path = providers_path(config_dir);
        let value = match self.format {
            ProviderStoreFormat::S1 => {
                let materialized = self.materialized_clone()?;
                serde_json::to_value(ProviderStoreS1 {
                    providers: &materialized.providers,
                    order: &materialized.order,
                })
                .context("encode Provider S1 store")?
            }
            ProviderStoreFormat::S2 => super::store_v2::encode_s2(self)?,
        };
        crate::infra::storage::write_json_pretty(&providers_path, &value)
            .with_context(|| format!("write providers {}", providers_path.display()))
    }

    pub fn storage_status(&self) -> ProviderStoreStatus {
        ProviderStoreStatus {
            format: self.format,
            schema_version: match self.format {
                ProviderStoreFormat::S1 => 1,
                ProviderStoreFormat::S2 => super::store_v2::PROVIDER_STORE_SCHEMA_VERSION,
            },
            store_generation: self.store_generation,
            credential_key_source: self.credential_vault.key_source(),
            committed_credentials_encrypted: self.format == ProviderStoreFormat::S2
                || self.credential_vault.is_sealed(),
        }
    }

    pub(crate) fn materialized_clone(&self) -> anyhow::Result<Self> {
        super::store_v2::materialize_store(self)
    }

    pub(crate) fn materialize_provider_record(
        &self,
        stored: &StoredProvider,
    ) -> anyhow::Result<StoredProvider> {
        let mut materialized = stored.clone();
        materialized.provider = super::store_v2::materialize_provider(self, stored)?;
        Ok(materialized)
    }

    pub(crate) fn seal_for_commit(&mut self, config_dir: &Path) -> anyhow::Result<()> {
        let key = crate::infra::credentials::load_or_create_root_key(config_dir)
            .context("resolve key for Provider commit")?;
        super::store_v2::seal_store(self, key)?;
        if self.format == ProviderStoreFormat::S2 {
            self.store_generation = self.store_generation.saturating_add(1).max(1);
        }
        Ok(())
    }

    pub(crate) fn promote_to_s2(&mut self, config_dir: &Path) -> anyhow::Result<()> {
        if self.format == ProviderStoreFormat::S2 {
            return Ok(());
        }
        let mut materialized = self.materialized_clone()?;
        materialized.format = ProviderStoreFormat::S2;
        materialized.store_generation = 0;
        materialized.seal_for_commit(config_dir)?;
        *self = materialized;
        Ok(())
    }

    pub fn validate_for_commit(&self) -> anyhow::Result<()> {
        let mut keys = std::collections::BTreeSet::new();
        for stored in &self.providers {
            let id = stored.provider.id.trim();
            if id.is_empty() {
                anyhow::bail!("provider id is required");
            }
            if stored.provider.name.trim().is_empty() {
                anyhow::bail!("provider name is required for {}:{id}", stored.app.as_str());
            }
            if !keys.insert((stored.app, id.to_string())) {
                anyhow::bail!("duplicate provider key {}:{id}", stored.app.as_str());
            }
            let expected = canonical_provider_type(stored.app, &stored.provider, &stored.resource)?;
            if stored.provider_type != expected || stored.provider_type_id != expected.as_str() {
                anyhow::bail!(
                    "provider classification mismatch for {}:{id}",
                    stored.app.as_str()
                );
            }
            if let Some(profile_id) = stored.resource.profile_id.as_ref() {
                let profile =
                    crate::domain::providers::registry::profile_by_id(profile_id.as_str())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "unknown profileId {profile_id} for {}:{id}",
                                stored.app.as_str()
                            )
                        })?;
                if profile.app != stored.app {
                    anyhow::bail!(
                        "profileId {profile_id} belongs to a different app for {}:{id}",
                        stored.app.as_str()
                    );
                }
                if stored.resource.profile_schema_revision.is_none() {
                    anyhow::bail!(
                        "profileSchemaRevision is required with profileId for {}:{id}",
                        stored.app.as_str()
                    );
                }
            } else if stored.resource.profile_schema_revision.is_some() {
                anyhow::bail!(
                    "profileSchemaRevision requires profileId for {}:{id}",
                    stored.app.as_str()
                );
            }
        }
        for (app, order) in &self.order {
            let expected = self
                .providers
                .iter()
                .filter(|provider| provider.app == *app)
                .map(|provider| provider.provider.id.as_str())
                .collect::<std::collections::BTreeSet<_>>();
            let actual = order
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            if actual.len() != order.len() || actual != expected {
                anyhow::bail!(
                    "Provider order for {} must contain every Provider exactly once",
                    app.as_str()
                );
            }
        }
        let mut request_ids = std::collections::BTreeSet::new();
        for stored in &self.providers {
            if let Some(request_id) = stored.resource.create_request_id.as_deref() {
                if !request_ids.insert(request_id) {
                    anyhow::bail!("duplicate Provider createRequestId");
                }
            }
        }
        Ok(())
    }

    pub fn upsert(&mut self, app: AppKind, provider: Provider) -> StoredProvider {
        let resource = self
            .providers
            .iter()
            .find(|item| item.app == app && item.provider.id == provider.id)
            .map(|item| item.resource.clone())
            .unwrap_or_default();
        self.upsert_with_resource(app, provider, resource)
    }

    pub fn upsert_with_resource(
        &mut self,
        app: AppKind,
        mut provider: Provider,
        resource: ProviderResourceMetadata,
    ) -> StoredProvider {
        if provider.id.trim().is_empty() {
            provider.id = generate_provider_id(app);
        }

        let routing = normalize_provider_model_routing(app, &mut provider);
        if routing.required && !routing.resolved {
            tracing::warn!(
                app = app.as_str(),
                provider_id = %provider.id,
                provider_name = %provider.name,
                "provider saved without a resolvable actual upstream model"
            );
        }

        let provider_type = canonical_provider_type(app, &provider, &resource)
            .unwrap_or_else(|_| classify_provider(app, &provider));
        let stored = StoredProvider {
            app,
            provider,
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource,
        };

        if let Some(existing) = self
            .providers
            .iter_mut()
            .find(|item| item.app == stored.app && item.provider.id == stored.provider.id)
        {
            *existing = stored.clone();
        } else {
            self.providers.push(stored.clone());
            if let Some(order) = self.order.get_mut(&app) {
                order.push(stored.provider.id.clone());
            }
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
            let left_sort = self.provider_order_index(&left.1).unwrap_or(usize::MAX);
            let right_sort = self.provider_order_index(&right.1).unwrap_or(usize::MAX);
            left.1
                .app
                .cmp(&right.1.app)
                .then_with(|| left_sort.cmp(&right_sort))
                .then_with(|| left.0.cmp(&right.0))
        });
        providers
            .into_iter()
            .map(|(_, provider)| provider)
            .collect()
    }

    pub fn update_sort_order(
        &mut self,
        app: AppKind,
        updates: Vec<ProviderSortUpdate>,
    ) -> anyhow::Result<bool> {
        let app_provider_ids = self
            .providers
            .iter()
            .filter(|provider| provider.app == app)
            .map(|provider| provider.provider.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if updates.len() != app_provider_ids.len() {
            anyhow::bail!("Provider sort order must include every Provider in the app");
        }
        let mut indexed = Vec::with_capacity(updates.len());
        let mut ids = std::collections::BTreeSet::new();
        let mut indexes = std::collections::BTreeSet::new();
        for update in updates {
            if !app_provider_ids.contains(update.id.as_str()) {
                anyhow::bail!("Provider sort order contains an unknown Provider");
            }
            if !ids.insert(update.id.clone()) || !indexes.insert(update.sort_index) {
                anyhow::bail!("Provider sort order contains duplicate ids or indexes");
            }
            indexed.push((update.sort_index, update.id));
        }
        indexed.sort_by_key(|(index, _)| *index);
        if indexed
            .iter()
            .enumerate()
            .any(|(expected, (actual, _))| expected != *actual)
        {
            anyhow::bail!("Provider sort indexes must be contiguous from zero");
        }
        let next = indexed.into_iter().map(|(_, id)| id).collect::<Vec<_>>();
        if self.order.get(&app) == Some(&next) {
            return Ok(false);
        }
        self.order.insert(app, next);
        Ok(true)
    }

    pub fn remove(&mut self, app: AppKind, provider_id: &str) -> Option<StoredProvider> {
        let index = self
            .providers
            .iter()
            .position(|provider| provider.app == app && provider.provider.id == provider_id)?;
        let removed = self.providers.remove(index);
        if let Some(order) = self.order.get_mut(&app) {
            order.retain(|id| id != provider_id);
            if order.is_empty() {
                self.order.remove(&app);
            }
        }
        Some(removed)
    }

    pub fn provider_order_index(&self, provider: &StoredProvider) -> Option<usize> {
        self.order
            .get(&provider.app)
            .and_then(|order| {
                order
                    .iter()
                    .position(|provider_id| provider_id == &provider.provider.id)
            })
            .or_else(|| provider_sort_index(provider))
    }
}

pub fn canonical_provider_type(
    app: AppKind,
    provider: &Provider,
    resource: &ProviderResourceMetadata,
) -> anyhow::Result<ProviderType> {
    let Some(profile_id) = resource.profile_id.as_ref() else {
        return Ok(classify_provider(app, provider));
    };
    let profile = profile_by_id(profile_id.as_str())
        .with_context(|| format!("unknown profileId {profile_id}"))?;
    if profile.app != app {
        anyhow::bail!("profileId {profile_id} belongs to a different app");
    }
    if let Some(provider_type) = profile.compatibility_provider_type {
        return Ok(provider_type);
    }
    match &profile.driver_binding {
        DriverBinding::Custom { .. } => {
            let binding = resource.custom_binding.as_ref().with_context(|| {
                format!("custom Provider profile {profile_id} has no customBinding")
            })?;
            resolve_custom_binding(profile, binding)?;
            Ok(match binding.upstream_protocol {
                UpstreamProtocol::AnthropicMessages => ProviderType::Claude,
                UpstreamProtocol::OpenAiChat | UpstreamProtocol::OpenAiResponses => {
                    ProviderType::Codex
                }
                UpstreamProtocol::GeminiNative => ProviderType::Gemini,
                UpstreamProtocol::Bedrock => ProviderType::AwsBedrock,
                UpstreamProtocol::Special | UpstreamProtocol::Custom | UpstreamProtocol::Legacy => {
                    anyhow::bail!(
                        "custom Provider profile {profile_id} has no compatibility type for {:?}",
                        binding.upstream_protocol
                    )
                }
            })
        }
        DriverBinding::Fixed { driver_id } if driver_id.as_str() == "legacy.frozen" => {
            Ok(classify_provider(app, provider))
        }
        DriverBinding::Fixed { .. } => {
            anyhow::bail!("fixed Provider profile {profile_id} has no compatibilityProviderType")
        }
    }
}

fn is_zero_revision(revision: &u64) -> bool {
    *revision == 0
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
    fn fixed_profile_provider_type_ignores_misleading_legacy_hints() {
        let provider = Provider {
            id: "typed-provider".to_string(),
            name: "Grok OAuth".to_string(),
            settings_config: json!({
                "base_url": "https://openrouter.ai/api/v1"
            }),
            category: Some("oauth".to_string()),
            meta: Some(crate::domain::providers::model::ProviderMeta {
                provider_type: Some("grok_oauth".to_string()),
                ..Default::default()
            }),
            extra: Default::default(),
        };
        assert_eq!(
            classify_provider(AppKind::Codex, &provider),
            ProviderType::GrokOAuth
        );

        let resource = ProviderResourceMetadata {
            profile_id: Some(ProfileId::parse("codex.openai_api_key").unwrap()),
            profile_schema_revision: Some(1),
            ..Default::default()
        };
        assert_eq!(
            canonical_provider_type(AppKind::Codex, &provider, &resource).unwrap(),
            ProviderType::Codex
        );
    }

    #[test]
    fn custom_profile_provider_type_is_derived_from_explicit_protocol() {
        let provider = Provider {
            id: "custom-provider".to_string(),
            name: "Misleading OpenRouter".to_string(),
            settings_config: json!({
                "base_url": "https://example.test/v1"
            }),
            category: Some("oauth".to_string()),
            meta: Some(crate::domain::providers::model::ProviderMeta {
                provider_type: Some("grok_oauth".to_string()),
                ..Default::default()
            }),
            extra: Default::default(),
        };

        for (protocol, expected) in [
            (UpstreamProtocol::AnthropicMessages, ProviderType::Claude),
            (UpstreamProtocol::OpenAiResponses, ProviderType::Codex),
            (UpstreamProtocol::GeminiNative, ProviderType::Gemini),
        ] {
            let resource = ProviderResourceMetadata {
                profile_id: Some(ProfileId::parse("claude.custom_http").unwrap()),
                profile_schema_revision: Some(1),
                custom_binding: Some(CustomBindingInput {
                    upstream_protocol: protocol,
                    auth_scheme: crate::domain::providers::registry::AuthScheme::ApiKey,
                }),
                ..Default::default()
            };
            assert_eq!(
                canonical_provider_type(AppKind::Claude, &provider, &resource).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn legacy_compat_profile_keeps_compatibility_classification() {
        let provider = Provider {
            id: "legacy-provider".to_string(),
            name: "OpenRouter".to_string(),
            settings_config: json!({
                "env": {"ANTHROPIC_BASE_URL": "https://openrouter.ai/api"}
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        };
        let resource = ProviderResourceMetadata {
            profile_id: Some(ProfileId::parse("claude.legacy_compat").unwrap()),
            profile_schema_revision: Some(1),
            ..Default::default()
        };

        assert_eq!(
            canonical_provider_type(AppKind::Claude, &provider, &resource).unwrap(),
            ProviderType::OpenRouter
        );
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

    #[test]
    fn runtime_load_normalizes_without_persisting_legacy_model_routing() {
        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-server-provider-routing-migration-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            providers_path(&config_dir),
            serde_json::to_vec_pretty(&json!({
                "providers": [{
                    "app": "codex",
                    "provider": {
                        "id": "grok-1",
                        "name": "Grok OAuth",
                        "settingsConfig": {"config": "model = \"grok-4.3\""},
                        "meta": {"providerType": "grok_oauth"}
                    },
                    "providerType": "codex",
                    "providerTypeId": "codex"
                }]
            }))
            .unwrap(),
        )
        .unwrap();

        let before = fs::read(providers_path(&config_dir)).unwrap();
        let raw = ProviderStore::load_or_default(&config_dir).unwrap();
        assert_eq!(raw.providers[0].provider_type, ProviderType::Codex);
        assert!(raw.providers[0].provider.settings_config["modelMapping"].is_null());

        let loaded = ProviderStore::load_runtime_or_default(&config_dir).unwrap();
        assert_eq!(loaded.providers[0].provider_type, ProviderType::GrokOAuth);
        assert_eq!(
            loaded.providers[0].provider.settings_config["modelMapping"],
            json!({"mode": "single", "upstreamModel": "grok-4.5"})
        );

        assert_eq!(fs::read(providers_path(&config_dir)).unwrap(), before);
        fs::remove_dir_all(config_dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn provider_store_is_saved_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-server-provider-permissions-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ProviderStore::default().save(&config_dir).unwrap();

        assert_eq!(
            fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(providers_path(&config_dir))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::remove_dir_all(config_dir).unwrap();
    }

    #[test]
    fn authoritative_order_is_per_app_and_ignores_legacy_sort_index() {
        let mut store = ProviderStore::default();
        store.upsert(
            AppKind::Claude,
            provider_with_sort_index("claude-first", "Claude first", 99),
        );
        store.upsert(
            AppKind::Claude,
            provider_with_sort_index("claude-second", "Claude second", 0),
        );
        store.upsert(
            AppKind::Codex,
            provider_with_sort_index("codex-first", "Codex first", 50),
        );
        store
            .update_sort_order(
                AppKind::Claude,
                vec![
                    ProviderSortUpdate {
                        id: "claude-first".to_string(),
                        sort_index: 0,
                    },
                    ProviderSortUpdate {
                        id: "claude-second".to_string(),
                        sort_index: 1,
                    },
                ],
            )
            .unwrap();

        let listed = store.list(None);
        let keys = listed
            .iter()
            .map(|stored| (stored.app, stored.provider.id.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                (AppKind::Claude, "claude-first"),
                (AppKind::Claude, "claude-second"),
                (AppKind::Codex, "codex-first"),
            ]
        );
        assert_eq!(store.provider_order_index(&listed[0]), Some(0));
        assert_eq!(store.provider_order_index(&listed[1]), Some(1));
        assert_eq!(store.provider_order_index(&listed[2]), Some(50));
    }

    #[test]
    fn sort_order_preserves_revision_and_survives_restart() {
        let config_dir = std::env::temp_dir().join(format!(
            "cc-switch-server-provider-order-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = ProviderStore::default();
        for (id, revision) in [("p1", 7), ("p2", 11)] {
            store.upsert_with_resource(
                AppKind::Codex,
                provider_with_sort_index(id, id, 0),
                ProviderResourceMetadata {
                    revision,
                    ..Default::default()
                },
            );
        }

        assert!(store
            .update_sort_order(
                AppKind::Codex,
                vec![
                    ProviderSortUpdate {
                        id: "p2".to_string(),
                        sort_index: 0,
                    },
                    ProviderSortUpdate {
                        id: "p1".to_string(),
                        sort_index: 1,
                    },
                ],
            )
            .unwrap());
        assert_eq!(store.providers[0].resource.revision, 7);
        assert_eq!(store.providers[1].resource.revision, 11);
        store.validate_for_commit().unwrap();
        store.save(&config_dir).unwrap();

        let loaded = ProviderStore::load_or_default(&config_dir).unwrap();
        fs::remove_dir_all(config_dir).unwrap();
        assert_eq!(loaded.order[&AppKind::Codex], vec!["p2", "p1"]);
        assert_eq!(loaded.list(Some(AppKind::Codex))[0].provider.id, "p2");
        assert_eq!(loaded.providers[0].resource.revision, 7);
        assert_eq!(loaded.providers[1].resource.revision, 11);
    }

    #[test]
    fn add_and_delete_keep_existing_authoritative_order_valid() {
        let mut store = ProviderStore::default();
        store.upsert(AppKind::Gemini, provider_with_sort_index("p1", "p1", 0));
        store
            .update_sort_order(
                AppKind::Gemini,
                vec![ProviderSortUpdate {
                    id: "p1".to_string(),
                    sort_index: 0,
                }],
            )
            .unwrap();

        store.upsert(AppKind::Gemini, provider_with_sort_index("p2", "p2", 0));
        assert_eq!(store.order[&AppKind::Gemini], vec!["p1", "p2"]);
        store.validate_for_commit().unwrap();

        store.remove(AppKind::Gemini, "p1").unwrap();
        assert_eq!(store.order[&AppKind::Gemini], vec!["p2"]);
        store.validate_for_commit().unwrap();
    }

    fn provider_with_sort_index(id: &str, name: &str, sort_index: usize) -> Provider {
        let mut extra = BTreeMap::new();
        extra.insert("sortIndex".to_string(), json!(sort_index));
        Provider {
            id: id.to_string(),
            name: name.to_string(),
            settings_config: json!({}),
            category: None,
            meta: None,
            extra,
        }
    }
}
