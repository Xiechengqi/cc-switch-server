use hmac::{Hmac, Mac};
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
    #[error("Provider {app}/{provider_id} does not support credential-source reuse")]
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
    #[error("derive anonymous capacity pool id: {message}")]
    CapacityPoolDerivation { message: String },
}

impl CredentialSourceError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ReuseUnsupported { .. } | Self::SourceMismatch { .. } => {
                "cc_switch_share_credential_source_mismatch"
            }
            Self::InvalidBindingCount { .. } => "cc_switch_share_binding_integrity_failed",
            Self::Resolution { .. } | Self::CapacityPoolDerivation { .. } => {
                "cc_switch_share_capacity_pool_derivation_failed"
            }
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
}

impl CredentialSourceIdentity {
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
                .first()
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
    if family.credential_source_scope == CredentialSourceScope::Surface {
        return Ok(None);
    }
    let Some(profile) = profile_by_id(profile_id.as_str()) else {
        return Ok(None);
    };

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
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::Account;
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta, ProviderType};
    use crate::domain::providers::registry::{provider_registry, ProfileId};
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
        let mut store = ProviderStore::default();
        store.upsert_with_resource(
            app,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({"apiKey": secret}),
                category: Some("custom".to_string()),
                meta: None,
                extra: BTreeMap::new(),
            },
            ProviderResourceMetadata {
                profile_id: Some(
                    ProfileId::parse(format!("{}.custom_http", app.as_str())).unwrap(),
                ),
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
    fn surface_scoped_custom_http_rejects_multi_surface_reuse() {
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
            Err(CredentialSourceError::ReuseUnsupported { .. })
        ));
        assert!(resolve_provider_credential_source(
            &providers,
            &AccountStore::default(),
            AppKind::Claude,
            "custom-bundle"
        )
        .unwrap()
        .is_none());
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
