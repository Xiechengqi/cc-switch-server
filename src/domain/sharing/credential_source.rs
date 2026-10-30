use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::bundle::surface_enabled;
use crate::domain::providers::credentials::split_provider_credentials;
use crate::domain::providers::model::AppKind;
use crate::domain::providers::registry::{
    family_for_profile, profile_by_id, CredentialPolicy, CredentialSourceScope,
};
use crate::domain::providers::store::{ProviderStore, StoredProvider};

use super::shares::{Share, ShareBinding};

mod custom;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CredentialSourceError {
    #[error("Share must have between one and three bindings (found {count})")]
    InvalidBindingCount { count: usize },
    #[error("resolve credential source for {app}/{provider_id}: {message}")]
    Resolution {
        app: &'static str,
        provider_id: String,
        message: String,
    },
    #[error("Provider {app}/{provider_id} has no verifiable shared credential source; check its endpoint, authentication and key, or use a separate Provider")]
    ReuseUnsupported {
        app: &'static str,
        provider_id: String,
    },
    #[error(
        "all Share bindings must use the same account or API key; {app}/{provider_id} does not match {first_app}/{first_provider_id}"
    )]
    SourceMismatch {
        first_app: &'static str,
        first_provider_id: String,
        app: &'static str,
        provider_id: String,
    },
    #[error("Custom Share bindings must use the same service and tenant endpoint scope; {app}/{provider_id} does not match {first_app}/{first_provider_id}")]
    EndpointMismatch {
        first_app: &'static str,
        first_provider_id: String,
        app: &'static str,
        provider_id: String,
    },
    #[error("Custom Share bindings must use equivalent authentication carriers; {app}/{provider_id} does not match {first_app}/{first_provider_id}")]
    AuthMismatch {
        first_app: &'static str,
        first_provider_id: String,
        app: &'static str,
        provider_id: String,
    },
    #[error("derive anonymous capacity pool id: {message}")]
    CapacityPoolDerivation { message: String },
}

impl CredentialSourceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ReuseUnsupported { .. }
            | Self::SourceMismatch { .. }
            | Self::EndpointMismatch { .. }
            | Self::AuthMismatch { .. } => "cc_switch_share_credential_source_mismatch",
            Self::InvalidBindingCount { .. } => "cc_switch_share_binding_integrity_failed",
            Self::Resolution { .. } | Self::CapacityPoolDerivation { .. } => {
                "cc_switch_share_capacity_pool_derivation_failed"
            }
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Self::ReuseUnsupported { .. } => "source_unverifiable",
            Self::SourceMismatch { .. } => "credential_mismatch",
            Self::EndpointMismatch { .. } => "endpoint_mismatch",
            Self::AuthMismatch { .. } => "auth_mismatch",
            Self::InvalidBindingCount { .. } => "no_enabled_apps",
            _ => "resolution_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("share {share_id}: {source}")]
pub struct ShareCredentialSourceError {
    pub share_id: String,
    #[source]
    pub source: CredentialSourceError,
}

impl ShareCredentialSourceError {
    pub fn code(&self) -> &'static str {
        self.source.code()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSourceIdentity {
    family: &'static str,
    digest: [u8; 32],
    custom_scope: Option<CustomSourceScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CustomSourceScope {
    endpoint_digest: [u8; 32],
    auth_digest: [u8; 32],
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleSharePreflight {
    pub bundle_id: String,
    pub bundle_revision: u64,
    pub enabled_apps: Vec<AppKind>,
    pub can_share: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

pub fn bundle_share_preflight(
    providers: &ProviderStore,
    accounts: &AccountStore,
    bundle_id: &str,
) -> Option<BundleSharePreflight> {
    use crate::domain::providers::bundle::{
        bundle_id as stored_bundle_id, is_explicit_bundle_surface,
    };
    let surfaces = providers
        .providers
        .iter()
        .filter(|stored| {
            is_explicit_bundle_surface(&stored.provider)
                && stored_bundle_id(&stored.provider) == Some(bundle_id)
        })
        .collect::<Vec<_>>();
    if surfaces.is_empty() {
        return None;
    }
    let bindings = surfaces
        .iter()
        .map(|stored| ShareBinding {
            app: stored.app,
            provider_id: stored.provider.id.clone(),
            provider_type: stored.provider_type,
        })
        .collect::<Vec<_>>();
    let result = shared_credential_source_for_bindings(providers, accounts, &bindings);
    let error = result.err();
    Some(BundleSharePreflight {
        bundle_id: bundle_id.to_string(),
        bundle_revision: surfaces
            .iter()
            .map(|stored| stored.resource.revision)
            .max()
            .unwrap_or_default(),
        enabled_apps: surfaces
            .iter()
            .filter(|stored| surface_enabled(&stored.provider))
            .map(|stored| stored.app)
            .collect(),
        can_share: error.is_none(),
        reason: error.as_ref().map(CredentialSourceError::reason),
        message: error.map(|error| match error {
            CredentialSourceError::Resolution { .. }
            | CredentialSourceError::CapacityPoolDerivation { .. } => {
                "Unable to verify the Provider credential source".to_string()
            }
            _ => error.to_string(),
        }),
    })
}

impl CredentialSourceIdentity {
    /// Internal only; never expose this enumerable credential digest on wire.
    pub(crate) fn quota_identity(&self) -> String {
        format!("{}:{}", self.family, hex::encode(self.digest))
    }
    pub fn capacity_pool_id(&self, key: &[u8; 32]) -> anyhow::Result<String> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key)
            .map_err(|error| anyhow::anyhow!("initialize capacity pool HMAC: {error}"))?;
        mac.update(b"cc-switch-share-capacity-pool-v1\0");
        mac.update(self.family.as_bytes());
        mac.update(b"\0");
        mac.update(&self.digest);
        let digest = mac.finalize().into_bytes();
        Ok(format!("cp_{}", hex::encode(&digest[..16])))
    }
}

pub fn isolated_capacity_pool_id(
    key: &[u8; 32],
    app: AppKind,
    provider_id: &str,
) -> anyhow::Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|error| anyhow::anyhow!("initialize isolated capacity pool HMAC: {error}"))?;
    mac.update(b"cc-switch-share-isolated-capacity-pool-v1\0");
    mac.update(app.as_str().as_bytes());
    mac.update(b"\0");
    mac.update(provider_id.as_bytes());
    let digest = mac.finalize().into_bytes();
    Ok(format!("cp_{}", hex::encode(&digest[..16])))
}

pub fn resolve_provider_credential_source(
    providers: &ProviderStore,
    accounts: &AccountStore,
    app: AppKind,
    provider_id: &str,
) -> anyhow::Result<Option<CredentialSourceIdentity>> {
    let Some(stored) = providers
        .providers
        .iter()
        .find(|provider| provider.app == app && provider.provider.id == provider_id)
    else {
        return Ok(None);
    };
    resolve_stored_provider_credential_source(providers, accounts, stored)
}

pub fn shared_credential_source_for_bindings(
    providers: &ProviderStore,
    accounts: &AccountStore,
    bindings: &[ShareBinding],
) -> Result<Option<CredentialSourceIdentity>, CredentialSourceError> {
    if !(1..=3).contains(&bindings.len()) {
        return Err(CredentialSourceError::InvalidBindingCount {
            count: bindings.len(),
        });
    }

    let enabled_bindings = bindings
        .iter()
        .filter(|binding| {
            providers
                .providers
                .iter()
                .find(|stored| {
                    stored.app == binding.app && stored.provider.id == binding.provider_id
                })
                .map(|stored| surface_enabled(&stored.provider))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    if enabled_bindings.is_empty() {
        return Err(CredentialSourceError::InvalidBindingCount { count: 0 });
    }

    let mut source: Option<(CredentialSourceIdentity, &ShareBinding)> = None;
    for binding in &enabled_bindings {
        if !providers
            .providers
            .iter()
            .any(|stored| stored.app == binding.app && stored.provider.id == binding.provider_id)
        {
            return Err(CredentialSourceError::Resolution {
                app: binding.app.as_str(),
                provider_id: binding.provider_id.clone(),
                message: "Provider is missing".to_string(),
            });
        }
        let candidate = resolve_provider_credential_source(
            providers,
            accounts,
            binding.app,
            &binding.provider_id,
        )
        .map_err(|error| CredentialSourceError::Resolution {
            app: binding.app.as_str(),
            provider_id: binding.provider_id.clone(),
            message: error.to_string(),
        })?;
        if enabled_bindings.len() > 1 && candidate.is_none() {
            return Err(CredentialSourceError::ReuseUnsupported {
                app: binding.app.as_str(),
                provider_id: binding.provider_id.clone(),
            });
        }
        if let Some(candidate) = candidate {
            if let Some((current, first_binding)) = source.as_ref() {
                if current != &candidate {
                    if let (Some(first), Some(next)) =
                        (&current.custom_scope, &candidate.custom_scope)
                    {
                        if first.endpoint_digest != next.endpoint_digest {
                            return Err(CredentialSourceError::EndpointMismatch {
                                first_app: first_binding.app.as_str(),
                                first_provider_id: first_binding.provider_id.clone(),
                                app: binding.app.as_str(),
                                provider_id: binding.provider_id.clone(),
                            });
                        }
                        if first.auth_digest != next.auth_digest {
                            return Err(CredentialSourceError::AuthMismatch {
                                first_app: first_binding.app.as_str(),
                                first_provider_id: first_binding.provider_id.clone(),
                                app: binding.app.as_str(),
                                provider_id: binding.provider_id.clone(),
                            });
                        }
                    }
                    return Err(CredentialSourceError::SourceMismatch {
                        first_app: first_binding.app.as_str(),
                        first_provider_id: first_binding.provider_id.clone(),
                        app: binding.app.as_str(),
                        provider_id: binding.provider_id.clone(),
                    });
                }
            } else {
                source = Some((candidate, binding));
            }
        }
    }
    Ok(source.map(|(identity, _)| identity))
}

pub fn capacity_pool_id_for_bindings(
    providers: &ProviderStore,
    accounts: &AccountStore,
    bindings: &[ShareBinding],
    root_key: &[u8; 32],
) -> Result<String, CredentialSourceError> {
    let source = shared_credential_source_for_bindings(providers, accounts, bindings)?;
    let derived = match source {
        Some(source) => source.capacity_pool_id(root_key),
        None => {
            let binding = bindings
                .iter()
                .find(|binding| {
                    providers.providers.iter().any(|stored| {
                        stored.app == binding.app
                            && stored.provider.id == binding.provider_id
                            && surface_enabled(&stored.provider)
                    })
                })
                .expect("binding count was validated before isolated capacity derivation");
            isolated_capacity_pool_id(root_key, binding.app, &binding.provider_id)
        }
    };
    derived.map_err(|error| CredentialSourceError::CapacityPoolDerivation {
        message: error.to_string(),
    })
}

pub fn capacity_pool_id_for_share(
    providers: &ProviderStore,
    accounts: &AccountStore,
    share: &Share,
    root_key: &[u8; 32],
) -> Result<String, ShareCredentialSourceError> {
    capacity_pool_id_for_bindings(providers, accounts, &share.bindings, root_key).map_err(
        |source| ShareCredentialSourceError {
            share_id: share.id.clone(),
            source,
        },
    )
}

pub fn resolve_stored_provider_credential_source(
    providers: &ProviderStore,
    accounts: &AccountStore,
    stored: &StoredProvider,
) -> anyhow::Result<Option<CredentialSourceIdentity>> {
    let Some(profile_id) = stored.resource.profile_id.as_ref() else {
        return Ok(None);
    };
    let Some(family) = family_for_profile(profile_id.as_str()) else {
        return Ok(None);
    };
    let Some(profile) = profile_by_id(profile_id.as_str()) else {
        return Ok(None);
    };
    if matches!(profile.credential_policy, CredentialPolicy::Custom) {
        return custom::resolve(providers, accounts, stored, family.family_id.as_str());
    }
    if family.credential_source_scope == CredentialSourceScope::Surface {
        return Ok(None);
    }

    let material = match &profile.credential_policy {
        CredentialPolicy::ManagedAccount {
            account_provider_type,
        } => {
            let Some(binding) = stored
                .provider
                .meta
                .as_ref()
                .and_then(|meta| meta.auth_binding.as_ref())
            else {
                return Ok(None);
            };
            let Some(account_id) = binding
                .account_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                return Ok(None);
            };
            let Some(account) = accounts.accounts.iter().find(|account| {
                account.id == account_id && account.provider_type == *account_provider_type
            }) else {
                return Ok(None);
            };
            if binding.auth_identity_generation != Some(account.auth_identity_generation) {
                return Ok(None);
            }
            format!(
                "managed\0{}\0{}",
                account_provider_type.as_str(),
                account_id
            )
            .into_bytes()
        }
        CredentialPolicy::StaticSecret { .. } | CredentialPolicy::Aws { .. } => {
            let materialized = providers.materialize_provider_record(stored)?;
            let (_, credentials) = split_provider_credentials(&materialized.provider)?;
            let mut values = credentials
                .values()
                .filter_map(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>();
            values.sort_unstable();
            let mut material = Vec::new();
            for value in values {
                material.extend_from_slice(value.as_bytes());
                material.push(0);
            }
            if material.is_empty() {
                return Ok(None);
            }
            material
        }
        _ => return Ok(None),
    };

    let mut digest = Sha256::new();
    digest.update(b"cc-switch-credential-source-v1\0");
    digest.update(family.family_id.as_bytes());
    digest.update(b"\0");
    digest.update(material);
    Ok(Some(CredentialSourceIdentity {
        family: family.family_id.as_str(),
        digest: digest.finalize().into(),
        custom_scope: None,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::Account;
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta, ProviderType};
    use crate::domain::providers::registry::{
        provider_registry, AuthScheme, CustomBindingInput, ProfileId, UpstreamProtocol,
    };
    use crate::domain::providers::store::ProviderResourceMetadata;

    fn account(id: &str) -> Account {
        serde_json::from_value(json!({
            "id": id,
            "providerType": "codex_oauth",
            "authIdentityGeneration": 7
        }))
        .unwrap()
    }

    fn google_account(id: &str) -> Account {
        serde_json::from_value(json!({
            "id": id,
            "providerType": "gemini_cli",
            "authIdentityGeneration": 7
        }))
        .unwrap()
    }

    fn oauth_provider(app: AppKind, id: &str, account_id: &str) -> StoredProvider {
        let profile_id = format!("{}.openai_oauth", app.as_str());
        let mut store = ProviderStore::default();
        store.upsert_with_resource(
            app,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account".to_string()),
                        auth_provider: Some("codex_oauth".to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(7),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: BTreeMap::new(),
            },
            ProviderResourceMetadata {
                profile_id: Some(ProfileId::parse(profile_id).unwrap()),
                ..ProviderResourceMetadata::default()
            },
        )
    }

    fn static_provider(app: AppKind, id: &str, secret: &str) -> StoredProvider {
        let (profile_id, settings_config) = match app {
            AppKind::Claude => (
                "claude.openrouter",
                json!({"env": {"ANTHROPIC_AUTH_TOKEN": secret}}),
            ),
            AppKind::Codex => (
                "codex.openrouter",
                json!({"env": {"OPENAI_API_KEY": secret}}),
            ),
            AppKind::Gemini => (
                "gemini.openrouter",
                json!({"env": {"GEMINI_API_KEY": secret}}),
            ),
        };
        let mut store = ProviderStore::default();
        store.upsert_with_resource(
            app,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config,
                category: None,
                meta: None,
                extra: BTreeMap::new(),
            },
            ProviderResourceMetadata {
                profile_id: Some(ProfileId::parse(profile_id).unwrap()),
                ..ProviderResourceMetadata::default()
            },
        )
    }

    fn google_oauth_provider(app: AppKind, id: &str, account_id: &str) -> StoredProvider {
        let profile_id = format!("{}.google_oauth", app.as_str());
        let mut store = ProviderStore::default();
        store.upsert_with_resource(
            app,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account".to_string()),
                        auth_provider: Some("gemini_cli".to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(7),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: BTreeMap::new(),
            },
            ProviderResourceMetadata {
                profile_id: Some(ProfileId::parse(profile_id).unwrap()),
                ..ProviderResourceMetadata::default()
            },
        )
    }

    fn custom_provider(app: AppKind, id: &str, secret: &str) -> StoredProvider {
        let (base_key, endpoint, upstream_protocol) = match app {
            AppKind::Claude => (
                "ANTHROPIC_BASE_URL",
                "https://custom.example",
                UpstreamProtocol::AnthropicMessages,
            ),
            AppKind::Codex => (
                "OPENAI_BASE_URL",
                "https://custom.example/v1",
                UpstreamProtocol::OpenAiResponses,
            ),
            AppKind::Gemini => (
                "GEMINI_BASE_URL",
                "https://custom.example/v1beta",
                UpstreamProtocol::GeminiNative,
            ),
        };
        let mut store = ProviderStore::default();
        store.upsert_with_resource(
            app,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({"apiKey": secret, "env": {base_key: endpoint}, "modelMapping": {"mode":"passthrough"}}),
                category: Some("custom".to_string()),
                meta: None,
                extra: BTreeMap::new(),
            },
            ProviderResourceMetadata {
                profile_id: Some(
                    ProfileId::parse(format!("{}.custom_http", app.as_str())).unwrap(),
                ),
                custom_binding: Some(CustomBindingInput { upstream_protocol, auth_scheme: AuthScheme::Bearer }),
                ..ProviderResourceMetadata::default()
            },
        )
    }

    #[test]
    fn openai_oauth_nodes_for_the_same_account_share_capacity() {
        let providers = ProviderStore {
            providers: vec![
                oauth_provider(AppKind::Claude, "claude-openai", "account-a"),
                oauth_provider(AppKind::Codex, "codex-openai", "account-a"),
                oauth_provider(AppKind::Codex, "codex-other", "account-b"),
            ],
            ..ProviderStore::default()
        };
        let accounts = AccountStore {
            accounts: vec![account("account-a"), account("account-b")],
            ..AccountStore::default()
        };
        let claude = resolve_provider_credential_source(
            &providers,
            &accounts,
            AppKind::Claude,
            "claude-openai",
        )
        .unwrap()
        .unwrap();
        let codex = resolve_provider_credential_source(
            &providers,
            &accounts,
            AppKind::Codex,
            "codex-openai",
        )
        .unwrap()
        .unwrap();
        let other = resolve_provider_credential_source(
            &providers,
            &accounts,
            AppKind::Codex,
            "codex-other",
        )
        .unwrap()
        .unwrap();

        assert_eq!(claude, codex);
        assert_ne!(claude, other);
        assert_eq!(
            claude.capacity_pool_id(&[9; 32]).unwrap(),
            codex.capacity_pool_id(&[9; 32]).unwrap()
        );
    }

    #[test]
    fn equal_static_keys_share_capacity_but_different_keys_do_not() {
        let providers = ProviderStore {
            providers: vec![
                static_provider(AppKind::Claude, "claude-openrouter", "shared-key"),
                static_provider(AppKind::Codex, "codex-openrouter", "shared-key"),
                static_provider(AppKind::Gemini, "gemini-openrouter", "different-key"),
            ],
            ..ProviderStore::default()
        };
        let accounts = AccountStore::default();
        let claude = resolve_provider_credential_source(
            &providers,
            &accounts,
            AppKind::Claude,
            "claude-openrouter",
        )
        .unwrap()
        .unwrap();
        let codex = resolve_provider_credential_source(
            &providers,
            &accounts,
            AppKind::Codex,
            "codex-openrouter",
        )
        .unwrap()
        .unwrap();
        let gemini = resolve_provider_credential_source(
            &providers,
            &accounts,
            AppKind::Gemini,
            "gemini-openrouter",
        )
        .unwrap()
        .unwrap();

        assert_eq!(claude, codex);
        assert_ne!(claude, gemini);
    }

    #[test]
    fn multi_app_bindings_reject_different_static_keys() {
        let providers = ProviderStore {
            providers: vec![
                static_provider(AppKind::Claude, "claude-openrouter", "shared-key"),
                static_provider(AppKind::Codex, "codex-openrouter", "different-key"),
            ],
            ..ProviderStore::default()
        };
        let bindings = vec![
            ShareBinding {
                app: AppKind::Claude,
                provider_id: "claude-openrouter".to_string(),
                provider_type: ProviderType::OpenRouter,
            },
            ShareBinding {
                app: AppKind::Codex,
                provider_id: "codex-openrouter".to_string(),
                provider_type: ProviderType::OpenRouter,
            },
        ];

        let error =
            shared_credential_source_for_bindings(&providers, &AccountStore::default(), &bindings)
                .unwrap_err();

        assert!(matches!(
            error,
            CredentialSourceError::SourceMismatch { .. }
        ));
        assert_eq!(error.code(), "cc_switch_share_credential_source_mismatch");
    }

    #[test]
    fn disabled_surfaces_are_ignored_when_deriving_shared_capacity() {
        let mut disabled = static_provider(AppKind::Codex, "codex-openrouter", "different-key");
        disabled
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(false));
        let providers = ProviderStore {
            providers: vec![
                static_provider(AppKind::Claude, "claude-openrouter", "shared-key"),
                disabled,
            ],
            ..ProviderStore::default()
        };
        let bindings = vec![
            ShareBinding {
                app: AppKind::Claude,
                provider_id: "claude-openrouter".to_string(),
                provider_type: ProviderType::OpenRouter,
            },
            ShareBinding {
                app: AppKind::Codex,
                provider_id: "codex-openrouter".to_string(),
                provider_type: ProviderType::OpenRouter,
            },
        ];

        assert!(shared_credential_source_for_bindings(
            &providers,
            &AccountStore::default(),
            &bindings
        )
        .is_ok());
    }

    #[test]
    fn google_oauth_surfaces_share_one_registry_declared_credential_source() {
        let providers = ProviderStore {
            providers: vec![
                google_oauth_provider(AppKind::Claude, "google-bundle", "google-account"),
                google_oauth_provider(AppKind::Gemini, "google-bundle", "google-account"),
            ],
            ..ProviderStore::default()
        };
        let accounts = AccountStore {
            accounts: vec![google_account("google-account")],
            ..AccountStore::default()
        };
        let bindings = vec![
            ShareBinding {
                app: AppKind::Claude,
                provider_id: "google-bundle".to_string(),
                provider_type: ProviderType::GeminiCli,
            },
            ShareBinding {
                app: AppKind::Gemini,
                provider_id: "google-bundle".to_string(),
                provider_type: ProviderType::GeminiCli,
            },
        ];

        assert!(
            shared_credential_source_for_bindings(&providers, &accounts, &bindings)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn surface_scoped_custom_http_rejects_different_credentials() {
        let providers = ProviderStore {
            providers: vec![
                custom_provider(AppKind::Claude, "custom-bundle", "claude-key"),
                custom_provider(AppKind::Codex, "custom-bundle", "codex-key"),
            ],
            ..ProviderStore::default()
        };
        let bindings = vec![
            ShareBinding {
                app: AppKind::Claude,
                provider_id: "custom-bundle".to_string(),
                provider_type: ProviderType::Claude,
            },
            ShareBinding {
                app: AppKind::Codex,
                provider_id: "custom-bundle".to_string(),
                provider_type: ProviderType::Codex,
            },
        ];

        assert!(matches!(
            shared_credential_source_for_bindings(&providers, &AccountStore::default(), &bindings),
            Err(CredentialSourceError::SourceMismatch { .. })
        ));
        assert!(resolve_provider_credential_source(
            &providers,
            &AccountStore::default(),
            AppKind::Claude,
            "custom-bundle"
        )
        .unwrap()
        .is_some());
    }

    fn custom_bindings() -> Vec<ShareBinding> {
        [AppKind::Claude, AppKind::Codex]
            .into_iter()
            .map(|app| ShareBinding {
                app,
                provider_id: "custom-bundle".to_string(),
                provider_type: match app {
                    AppKind::Claude => ProviderType::Claude,
                    _ => ProviderType::Codex,
                },
            })
            .collect()
    }

    fn custom_store() -> ProviderStore {
        ProviderStore {
            providers: vec![
                custom_provider(AppKind::Claude, "custom-bundle", "shared-custom-secret"),
                custom_provider(AppKind::Codex, "custom-bundle", "shared-custom-secret"),
            ],
            ..ProviderStore::default()
        }
    }

    #[test]
    fn custom_sources_compare_effective_auth_not_app_profile_or_slot_names() {
        let mut store = custom_store();
        store.providers[0].provider.settings_config["env"]["ANTHROPIC_AUTH_TOKEN"] =
            json!("shared-custom-secret");
        store.providers[0]
            .provider
            .settings_config
            .as_object_mut()
            .unwrap()
            .remove("apiKey");
        let source = shared_credential_source_for_bindings(
            &store,
            &AccountStore::default(),
            &custom_bindings(),
        )
        .unwrap()
        .unwrap();
        assert!(source
            .capacity_pool_id(&[7; 32])
            .unwrap()
            .starts_with("cp_"));
        let mut other = store.clone();
        for stored in &mut other.providers {
            stored.provider.id = "independent-bundle".to_string();
        }
        let other_source = resolve_stored_provider_credential_source(
            &other,
            &AccountStore::default(),
            &other.providers[1],
        )
        .unwrap()
        .unwrap();
        assert_eq!(source, other_source);
        store.providers[0].provider.settings_config["apiKey"] = json!("conflicting-alias-secret");
        assert!(matches!(
            shared_credential_source_for_bindings(
                &store,
                &AccountStore::default(),
                &custom_bindings()
            ),
            Err(CredentialSourceError::ReuseUnsupported { .. })
        ));
    }

    #[test]
    fn custom_sources_reject_other_hosts_tenant_paths_and_authentication_carriers() {
        for endpoint in [
            "https://other.example/v1",
            "https://custom.example/tenant/v1",
            "http://custom.example/v1",
            "https://custom.example:8443/v1",
        ] {
            let mut store = custom_store();
            store.providers[1].provider.settings_config["env"]["OPENAI_BASE_URL"] = json!(endpoint);
            assert!(matches!(
                shared_credential_source_for_bindings(
                    &store,
                    &AccountStore::default(),
                    &custom_bindings()
                ),
                Err(CredentialSourceError::EndpointMismatch { .. })
            ));
        }
        let mut store = custom_store();
        store.providers[0]
            .resource
            .custom_binding
            .as_mut()
            .unwrap()
            .auth_scheme = AuthScheme::ApiKey;
        assert!(matches!(
            shared_credential_source_for_bindings(
                &store,
                &AccountStore::default(),
                &custom_bindings()
            ),
            Err(CredentialSourceError::AuthMismatch { .. })
        ));
    }

    #[test]
    fn custom_sources_include_named_extra_headers_and_ignore_inactive_surfaces() {
        let mut store = custom_store();
        store.providers[0].provider.settings_config["extraHeaders"] =
            json!({"X-Tenant":"tenant-a", "x-role":"role-b"});
        store.providers[1].provider.settings_config["extraHeaders"] =
            json!({"x-tenant":"tenant-a", "X-Role":"role-b"});
        assert!(shared_credential_source_for_bindings(
            &store,
            &AccountStore::default(),
            &custom_bindings()
        )
        .is_ok());
        store.providers[1].provider.settings_config["extraHeaders"] =
            json!({"x-tenant":"role-b", "x-role":"tenant-a"});
        assert!(matches!(
            shared_credential_source_for_bindings(
                &store,
                &AccountStore::default(),
                &custom_bindings()
            ),
            Err(CredentialSourceError::SourceMismatch { .. })
        ));
        store.providers[1]
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(false));
        store.providers[1].provider.settings_config = json!({});
        assert!(shared_credential_source_for_bindings(
            &store,
            &AccountStore::default(),
            &custom_bindings()
        )
        .is_ok());
    }

    #[test]
    fn isolated_custom_pool_uses_the_enabled_binding_not_the_first_disabled_one() {
        let mut store = custom_store();
        store.providers[0]
            .provider
            .extra
            .insert("surfaceEnabled".to_string(), json!(false));
        store.providers[1].provider.settings_config["env"]["OPENAI_BASE_URL"] =
            json!("https://custom.example/v1?tenant=a");
        let pool = capacity_pool_id_for_bindings(
            &store,
            &AccountStore::default(),
            &custom_bindings(),
            &[7; 32],
        )
        .unwrap();
        assert_eq!(
            pool,
            isolated_capacity_pool_id(&[7; 32], AppKind::Codex, "custom-bundle").unwrap()
        );
    }

    #[test]
    fn custom_header_and_query_names_are_identity_significant() {
        let mut store = custom_store();
        for (index, name) in ["X-Vendor-Key", "x-vendor-key"].into_iter().enumerate() {
            store.providers[index]
                .resource
                .custom_binding
                .as_mut()
                .unwrap()
                .auth_scheme = AuthScheme::CustomHeader;
            store.providers[index].provider.meta = Some(ProviderMeta {
                api_key_field: Some(name.to_string()),
                ..ProviderMeta::default()
            });
        }
        assert!(shared_credential_source_for_bindings(
            &store,
            &AccountStore::default(),
            &custom_bindings()
        )
        .is_ok());
        store.providers[1]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .api_key_field = Some("x-other-key".to_string());
        assert!(matches!(
            shared_credential_source_for_bindings(
                &store,
                &AccountStore::default(),
                &custom_bindings()
            ),
            Err(CredentialSourceError::AuthMismatch { .. })
        ));
        // Both Custom App Profiles can use the Gemini-native Query rail. The
        // Anthropic/OpenAI drivers themselves do not accept Query authentication.
        let mut gemini = custom_provider(AppKind::Gemini, "custom-bundle", "shared-custom-secret");
        gemini.resource.custom_binding.as_mut().unwrap().auth_scheme = AuthScheme::Query;
        gemini.provider.meta = Some(ProviderMeta {
            api_key_field: Some("key".to_string()),
            ..ProviderMeta::default()
        });
        store.providers[0]
            .resource
            .custom_binding
            .as_mut()
            .unwrap()
            .auth_scheme = AuthScheme::Query;
        store.providers[0]
            .resource
            .custom_binding
            .as_mut()
            .unwrap()
            .upstream_protocol = UpstreamProtocol::GeminiNative;
        store.providers[0].provider.settings_config["env"]["ANTHROPIC_BASE_URL"] =
            json!("https://custom.example/v1beta");
        store.providers[0]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .api_key_field = Some("key".to_string());
        store.providers[1] = gemini;
        let mut bindings = custom_bindings();
        bindings[1].app = AppKind::Gemini;
        for (stored, binding) in store.providers.iter_mut().zip(&mut bindings) {
            let canonical = crate::domain::providers::store::canonical_provider_type(
                stored.app,
                &stored.provider,
                &stored.resource,
            )
            .unwrap();
            stored.provider_type = canonical;
            stored.provider_type_id = canonical.as_str().to_string();
            binding.provider_type = canonical;
        }
        assert!(
            shared_credential_source_for_bindings(&store, &AccountStore::default(), &bindings)
                .is_ok()
        );
        store.providers[1]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .api_key_field = Some("Key".to_string());
        assert!(matches!(
            shared_credential_source_for_bindings(&store, &AccountStore::default(), &bindings),
            Err(CredentialSourceError::AuthMismatch { .. })
        ));
    }

    #[test]
    fn legacy_single_custom_pool_is_repaired_without_resetting_share_data() {
        use crate::domain::sharing::shares::{ShareStore, UpsertShareInput};
        let mut providers = custom_store();
        providers.providers.truncate(1);
        let stored = &providers.providers[0];
        let input: UpsertShareInput = serde_json::from_value(json!({
            "id":"legacy-custom-share","ownerEmail":"owner@example.com","app":"claude",
            "providerId":"custom-bundle","providerType":stored.provider_type,
            "enabled":true,"status":"active","tokenLimit":123,"parallelLimit":4,
            "description":"keep description","tunnelSubdomain":"customlegacy",
            "userGrants":{"friend@example.com":{"email":"friend@example.com","role":"shareto","active":true,"policy":{"tokenLimit":99,"tokenPeriod":"day"}}}
        })).unwrap();
        let root_key = [7; 32];
        let old_pool =
            isolated_capacity_pool_id(&root_key, AppKind::Claude, "custom-bundle").unwrap();
        let mut shares = ShareStore::default();
        shares
            .upsert_with_capacity(input, Some(old_pool.clone()))
            .unwrap();
        shares.shares[0].tokens_used = 42;
        shares.shares[0].requests_count = 5;
        let original = shares.shares[0].clone();
        let outcomes = shares.repair_integrity(&providers, &AccountStore::default(), &root_key);
        assert!(outcomes[0].changed());
        let repaired = &shares.shares[0];
        assert_ne!(repaired.capacity_pool_id, old_pool);
        assert!(repaired.enabled);
        assert!(repaired.integrity_error.is_none());
        assert_eq!(repaired.tokens_used, 42);
        assert_eq!(repaired.requests_count, 5);
        assert_eq!(repaired.token_limit, original.token_limit);
        assert_eq!(repaired.parallel_limit, original.parallel_limit);
        assert_eq!(repaired.tunnel_subdomain, original.tunnel_subdomain);
        assert_eq!(
            serde_json::to_value(&repaired.user_grants).unwrap(),
            serde_json::to_value(&original.user_grants).unwrap()
        );
        assert!(
            !shares.repair_integrity(&providers, &AccountStore::default(), &root_key)[0].changed()
        );
    }

    #[test]
    fn every_multi_surface_family_declares_a_compatible_credential_scope() {
        for family in provider_registry()
            .families
            .iter()
            .filter(|family| family.surfaces.len() > 1)
        {
            let profile = profile_by_id(family.credential_profile_id.as_str()).unwrap();
            match family.credential_source_scope {
                CredentialSourceScope::Bundle => assert!(matches!(
                    profile.credential_policy,
                    CredentialPolicy::ManagedAccount { .. }
                        | CredentialPolicy::StaticSecret { .. }
                        | CredentialPolicy::Aws { .. }
                )),
                CredentialSourceScope::Surface => assert!(matches!(
                    profile.credential_policy,
                    CredentialPolicy::Custom | CredentialPolicy::Legacy
                )),
            }
        }
    }
}
