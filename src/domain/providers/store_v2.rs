use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Context;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

use crate::domain::providers::credentials::{
    materialize_provider_credentials, provider_credential_slot_is_supported,
    split_provider_credentials,
};
use crate::infra::credentials::{
    derive_provider_key, provider_key_id, CredentialKeySource, ResolvedCredentialKey,
};

use super::model::{AppKind, AuthBinding, Provider, ProviderMeta, ProviderType};
use super::registry::{CustomBindingInput, ProfileId, ProviderKey};
use super::store::{ProviderResourceMetadata, ProviderStore, ProviderStoreFormat, StoredProvider};

pub(crate) const PROVIDER_STORE_FORMAT: &str = "cc-switch-provider-store";
pub(crate) const PROVIDER_STORE_SCHEMA_VERSION: u32 = 2;
pub(crate) const PROVIDER_STORE_GUARD: &str = "s2-encrypted-typed-records";
const CREDENTIAL_ENVELOPE_VERSION: u32 = 1;
const CREDENTIAL_ALGORITHM: &str = "xchacha20poly1305";
const LEGACY_RESOLVER_REVISION: u32 = 1;

#[derive(Clone, Default)]
pub(crate) struct ProviderCredentialVault {
    key: Option<Arc<Zeroizing<[u8; 32]>>>,
    key_source: Option<CredentialKeySource>,
    key_id: Option<String>,
    envelopes: BTreeMap<ProviderKey, CredentialEnvelope>,
    sealed: bool,
}

impl std::fmt::Debug for ProviderCredentialVault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCredentialVault")
            .field("key_source", &self.key_source)
            .field("key_id", &self.key_id)
            .field("envelope_count", &self.envelopes.len())
            .field("sealed", &self.sealed)
            .finish()
    }
}

impl ProviderCredentialVault {
    pub(crate) fn key_source(&self) -> Option<CredentialKeySource> {
        self.key_source
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.sealed
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CredentialEnvelope {
    version: u32,
    key_id: String,
    generation: u64,
    slots: BTreeMap<String, EncryptedCredentialSlot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EncryptedCredentialSlot {
    algorithm: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderStoreS2 {
    format: String,
    schema_version: u32,
    guard: String,
    store_generation: u64,
    /// Deliberately incompatible with the S1 `providers: Vec` field.
    providers: LegacyDecoderRejectGuard,
    records: BTreeMap<AppKind, BTreeMap<String, ProviderRecordS2>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    order_by_app: BTreeMap<AppKind, Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyDecoderRejectGuard {
    guard: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderRecordS2 {
    provider_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_id: Option<ProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_schema_revision: Option<u32>,
    revision: u64,
    credential_generation: u64,
    display: ProviderDisplayS2,
    runtime_config: ProviderRuntimeConfigS2,
    control_policy: ProviderControlPolicyS2,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_binding: Option<AuthBinding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credentials: Option<CredentialEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_binding: Option<CustomBindingInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_payload: Option<LegacyPayloadS2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    create_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderDisplayS2 {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    category: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderRuntimeConfigS2 {
    settings: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderControlPolicyS2 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    meta: Option<ProviderMeta>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyPayloadS2 {
    resolver_revision: u32,
    provider_type: ProviderType,
    provider_type_id: String,
}

pub(crate) fn looks_like_s2(value: &Value) -> bool {
    value.get("format").is_some()
        || value.get("schemaVersion").is_some()
        || value.get("guard").is_some()
        || value.get("records").is_some()
}

pub(crate) fn seal_store(
    store: &mut ProviderStore,
    resolved: ResolvedCredentialKey,
) -> anyhow::Result<()> {
    let provider_key = derive_provider_key(&resolved.key)?;
    let key_id = provider_key_id(&provider_key);
    let key = Arc::new(Zeroizing::new(provider_key));
    let mut envelopes = BTreeMap::new();

    for stored in &mut store.providers {
        let (redacted, credentials) = split_provider_credentials(&stored.provider)
            .with_context(|| provider_context(stored, "split credentials"))?;
        if !credentials.is_empty() {
            let provider_key = ProviderKey::new(stored.app, stored.provider.id.clone())?;
            let envelope = encrypt_envelope(stored, &credentials, key.as_ref(), &key_id)?;
            envelopes.insert(provider_key, envelope);
        }
        stored.provider = redacted;
    }

    store.credential_vault = Arc::new(ProviderCredentialVault {
        key: Some(key),
        key_source: Some(resolved.source),
        key_id: Some(key_id),
        envelopes,
        sealed: true,
    });
    Ok(())
}

pub(crate) fn materialize_store(store: &ProviderStore) -> anyhow::Result<ProviderStore> {
    let mut materialized = store.clone();
    if !store.credential_vault.sealed {
        return Ok(materialized);
    }
    for stored in &mut materialized.providers {
        stored.provider = materialize_provider(store, stored)?;
    }
    Ok(materialized)
}

pub(crate) fn materialize_provider(
    store: &ProviderStore,
    stored: &StoredProvider,
) -> anyhow::Result<Provider> {
    if !store.credential_vault.sealed {
        return Ok(stored.provider.clone());
    }
    let provider_key = ProviderKey::new(stored.app, stored.provider.id.clone())?;
    let (_, summary) = super::credentials::redact_provider(&stored.provider);
    let Some(envelope) = store.credential_vault.envelopes.get(&provider_key) else {
        if summary.slots.is_empty() {
            return Ok(stored.provider.clone());
        }
        anyhow::bail!(
            "sealed Provider {} has no credential envelope",
            stored.provider.id
        );
    };
    let key = store
        .credential_vault
        .key
        .as_ref()
        .context("Provider credential vault has no decryption key")?;
    let key_id = store
        .credential_vault
        .key_id
        .as_deref()
        .context("Provider credential vault has no key id")?;
    let credentials = decrypt_envelope(stored, envelope, key.as_ref(), key_id)?;
    materialize_provider_credentials(&stored.provider, &credentials)
        .with_context(|| provider_context(stored, "materialize credentials"))
}

pub(crate) fn encode_s2(store: &ProviderStore) -> anyhow::Result<Value> {
    if store.format != ProviderStoreFormat::S2 {
        anyhow::bail!("cannot encode a non-S2 Provider store as S2");
    }
    if !store.credential_vault.sealed {
        anyhow::bail!("cannot encode an unsealed Provider store as S2");
    }

    let mut records: BTreeMap<AppKind, BTreeMap<String, ProviderRecordS2>> = BTreeMap::new();
    let mut referenced_envelopes = BTreeMap::<ProviderKey, ()>::new();
    for stored in &store.providers {
        let key = ProviderKey::new(stored.app, stored.provider.id.clone())?;
        let envelope = store.credential_vault.envelopes.get(&key).cloned();
        if envelope.is_some() {
            referenced_envelopes.insert(key.clone(), ());
        }
        let record = record_from_stored(stored, envelope)?;
        if records
            .entry(stored.app)
            .or_default()
            .insert(stored.provider.id.clone(), record)
            .is_some()
        {
            anyhow::bail!("duplicate Provider key while encoding S2");
        }
    }
    if referenced_envelopes.len() != store.credential_vault.envelopes.len() {
        anyhow::bail!("Provider credential vault contains orphan envelopes");
    }

    serde_json::to_value(ProviderStoreS2 {
        format: PROVIDER_STORE_FORMAT.to_string(),
        schema_version: PROVIDER_STORE_SCHEMA_VERSION,
        guard: PROVIDER_STORE_GUARD.to_string(),
        store_generation: store.store_generation,
        providers: LegacyDecoderRejectGuard {
            guard: "old-decoder-must-reject".to_string(),
        },
        records,
        order_by_app: store.order.clone(),
    })
    .context("encode Provider S2 store")
}

pub(crate) fn decode_s2(
    value: Value,
    resolved: ResolvedCredentialKey,
) -> anyhow::Result<ProviderStore> {
    let persisted: ProviderStoreS2 =
        serde_json::from_value(value).context("decode guarded Provider S2 store")?;
    if persisted.format != PROVIDER_STORE_FORMAT
        || persisted.schema_version != PROVIDER_STORE_SCHEMA_VERSION
        || persisted.guard != PROVIDER_STORE_GUARD
        || persisted.providers.guard != "old-decoder-must-reject"
    {
        anyhow::bail!("unsupported or invalid Provider S2 format/schema/guard");
    }
    if persisted.store_generation == 0 {
        anyhow::bail!("Provider S2 storeGeneration must be positive");
    }

    let provider_key = derive_provider_key(&resolved.key)?;
    let expected_key_id = provider_key_id(&provider_key);
    let key = Arc::new(Zeroizing::new(provider_key));
    let mut providers = Vec::new();
    let mut envelopes = BTreeMap::new();

    for (app, app_records) in persisted.records {
        for (record_key, record) in app_records {
            if record_key != record.provider_id {
                anyhow::bail!("Provider S2 record map key does not match providerId");
            }
            let (stored, envelope) = stored_from_record(app, record)?;
            if let Some(envelope) = envelope {
                let key_ref = ProviderKey::new(app, stored.provider.id.clone())?;
                validate_envelope_shape(&stored, &envelope, &expected_key_id)?;
                if envelopes.insert(key_ref, envelope).is_some() {
                    anyhow::bail!("duplicate Provider credential envelope");
                }
            }
            providers.push(stored);
        }
    }

    let mut store = ProviderStore {
        providers,
        order: persisted.order_by_app,
        runtime_index: Default::default(),
        format: ProviderStoreFormat::S2,
        store_generation: persisted.store_generation,
        credential_vault: Arc::new(ProviderCredentialVault {
            key: Some(key),
            key_source: Some(resolved.source),
            key_id: Some(expected_key_id),
            envelopes,
            sealed: true,
        }),
    };
    store.validate_for_commit()?;
    // Full authenticated decryption also proves every redacted slot has exactly one envelope slot.
    for stored in &store.providers {
        let _ = materialize_provider(&store, stored)?;
    }
    store.runtime_index = Default::default();
    Ok(store)
}

fn record_from_stored(
    stored: &StoredProvider,
    credentials: Option<CredentialEnvelope>,
) -> anyhow::Result<ProviderRecordS2> {
    let mut meta = stored.provider.meta.clone();
    let account_binding = meta.as_mut().and_then(|meta| meta.auth_binding.take());
    let legacy_payload = if provider_uses_legacy_payload(stored.resource.profile_id.as_ref()) {
        Some(LegacyPayloadS2 {
            resolver_revision: LEGACY_RESOLVER_REVISION,
            provider_type: stored.provider_type,
            provider_type_id: stored.provider_type_id.clone(),
        })
    } else {
        None
    };
    Ok(ProviderRecordS2 {
        provider_id: stored.provider.id.clone(),
        profile_id: stored.resource.profile_id.clone(),
        profile_schema_revision: stored.resource.profile_schema_revision,
        revision: stored.resource.revision,
        credential_generation: stored.resource.credential_generation,
        display: ProviderDisplayS2 {
            name: stored.provider.name.clone(),
            category: stored.provider.category.clone(),
        },
        runtime_config: ProviderRuntimeConfigS2 {
            settings: stored.provider.settings_config.clone(),
        },
        control_policy: ProviderControlPolicyS2 {
            meta,
            extra: stored.provider.extra.clone(),
        },
        account_binding,
        credentials,
        custom_binding: stored.resource.custom_binding.clone(),
        legacy_payload,
        create_request_id: stored.resource.create_request_id.clone(),
    })
}

fn stored_from_record(
    app: AppKind,
    record: ProviderRecordS2,
) -> anyhow::Result<(StoredProvider, Option<CredentialEnvelope>)> {
    if record.provider_id.trim().is_empty() || record.provider_id != record.provider_id.trim() {
        anyhow::bail!("Provider S2 providerId must be non-empty and trimmed");
    }
    if record.display.name.trim().is_empty() {
        anyhow::bail!("Provider S2 display name is required");
    }
    if record.profile_id.is_some() != record.profile_schema_revision.is_some() {
        anyhow::bail!("Provider S2 profileId/profileSchemaRevision must appear together");
    }
    if provider_uses_legacy_payload(record.profile_id.as_ref()) != record.legacy_payload.is_some() {
        anyhow::bail!("Provider S2 legacy payload disposition does not match profile identity");
    }
    if let Some(legacy) = record.legacy_payload.as_ref() {
        if legacy.resolver_revision != LEGACY_RESOLVER_REVISION
            || legacy.provider_type_id != legacy.provider_type.as_str()
        {
            anyhow::bail!("Provider S2 legacy payload is not supported");
        }
    }

    let mut meta = record.control_policy.meta;
    if let Some(binding) = record.account_binding {
        meta.get_or_insert_with(ProviderMeta::default).auth_binding = Some(binding);
    }
    let provider = Provider {
        id: record.provider_id,
        name: record.display.name,
        settings_config: record.runtime_config.settings,
        category: record.display.category,
        meta,
        extra: record.control_policy.extra,
    };
    let resource = ProviderResourceMetadata {
        profile_id: record.profile_id,
        profile_schema_revision: record.profile_schema_revision,
        revision: record.revision,
        credential_generation: record.credential_generation,
        custom_binding: record.custom_binding,
        create_request_id: record.create_request_id,
    };
    let (provider_type, provider_type_id) = match record.legacy_payload {
        Some(legacy) => (legacy.provider_type, legacy.provider_type_id),
        None => {
            let provider_type = super::store::canonical_provider_type(app, &provider, &resource)?;
            (provider_type, provider_type.as_str().to_string())
        }
    };
    Ok((
        StoredProvider {
            app,
            provider,
            provider_type,
            provider_type_id,
            resource,
        },
        record.credentials,
    ))
}

fn encrypt_envelope(
    stored: &StoredProvider,
    credentials: &BTreeMap<String, Value>,
    key: &[u8; 32],
    key_id: &str,
) -> anyhow::Result<CredentialEnvelope> {
    let mut slots = BTreeMap::new();
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    for (slot, value) in credentials {
        if !provider_credential_slot_is_supported(slot) {
            anyhow::bail!("unsupported Provider credential slot {slot}");
        }
        let mut nonce = [0u8; 24];
        rand::thread_rng().fill_bytes(&mut nonce);
        let plaintext = Zeroizing::new(
            serde_json::to_vec(value).context("encode Provider credential slot value")?,
        );
        let aad = credential_aad(stored, slot, stored.resource.credential_generation)?;
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext.as_slice(),
                    aad: &aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("encrypt Provider credential slot {slot}"))?;
        slots.insert(
            slot.clone(),
            EncryptedCredentialSlot {
                algorithm: CREDENTIAL_ALGORITHM.to_string(),
                nonce: URL_SAFE_NO_PAD.encode(nonce),
                ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
            },
        );
    }
    Ok(CredentialEnvelope {
        version: CREDENTIAL_ENVELOPE_VERSION,
        key_id: key_id.to_string(),
        generation: stored.resource.credential_generation,
        slots,
    })
}

fn decrypt_envelope(
    stored: &StoredProvider,
    envelope: &CredentialEnvelope,
    key: &[u8; 32],
    expected_key_id: &str,
) -> anyhow::Result<BTreeMap<String, Value>> {
    validate_envelope_shape(stored, envelope, expected_key_id)?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key));
    let mut credentials = BTreeMap::new();
    for (slot, encrypted) in &envelope.slots {
        let nonce = URL_SAFE_NO_PAD
            .decode(&encrypted.nonce)
            .with_context(|| format!("decode Provider credential nonce for {slot}"))?;
        let nonce: [u8; 24] = nonce
            .try_into()
            .map_err(|_| anyhow::anyhow!("invalid Provider credential nonce for {slot}"))?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&encrypted.ciphertext)
            .with_context(|| format!("decode Provider credential ciphertext for {slot}"))?;
        let aad = credential_aad(stored, slot, envelope.generation)?;
        let plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: &aad,
                    },
                )
                .map_err(|_| anyhow::anyhow!("decrypt Provider credential slot {slot}"))?,
        );
        let value = serde_json::from_slice(plaintext.as_slice())
            .with_context(|| format!("decode Provider credential value for {slot}"))?;
        if credentials.insert(slot.clone(), value).is_some() {
            anyhow::bail!("duplicate Provider credential slot {slot}");
        }
    }
    Ok(credentials)
}

fn validate_envelope_shape(
    stored: &StoredProvider,
    envelope: &CredentialEnvelope,
    expected_key_id: &str,
) -> anyhow::Result<()> {
    if envelope.version != CREDENTIAL_ENVELOPE_VERSION {
        anyhow::bail!("unsupported Provider credential envelope version");
    }
    if envelope.key_id != expected_key_id {
        anyhow::bail!("Provider credential envelope key id does not match the configured key");
    }
    if envelope.generation != stored.resource.credential_generation {
        anyhow::bail!("Provider credential envelope generation mismatch");
    }
    if envelope.slots.is_empty() {
        anyhow::bail!("Provider credential envelope has no slots");
    }
    for (slot, encrypted) in &envelope.slots {
        if !provider_credential_slot_is_supported(slot) {
            anyhow::bail!("unsupported Provider credential slot {slot}");
        }
        if encrypted.algorithm != CREDENTIAL_ALGORITHM {
            anyhow::bail!("unsupported Provider credential algorithm for {slot}");
        }
        if encrypted.nonce.is_empty() || encrypted.ciphertext.is_empty() {
            anyhow::bail!("Provider credential slot {slot} has an empty envelope field");
        }
    }
    Ok(())
}

fn credential_aad(stored: &StoredProvider, slot: &str, generation: u64) -> anyhow::Result<Vec<u8>> {
    serde_json::to_vec(&(
        "cc-switch-provider-credential",
        CREDENTIAL_ENVELOPE_VERSION,
        stored.app,
        stored.provider.id.as_str(),
        stored
            .resource
            .profile_id
            .as_ref()
            .map(ProfileId::as_str)
            .unwrap_or("legacy_compat"),
        slot,
        generation,
    ))
    .context("encode Provider credential AAD")
}

fn provider_context(stored: &StoredProvider, action: &str) -> String {
    format!(
        "{action} for Provider {}:{}",
        stored.app.as_str(),
        stored.provider.id
    )
}

fn provider_uses_legacy_payload(profile_id: Option<&ProfileId>) -> bool {
    let Some(profile_id) = profile_id else {
        return true;
    };
    super::registry::profile_by_id(profile_id.as_str()).is_some_and(|profile| {
        matches!(
            &profile.driver_binding,
            super::registry::DriverBinding::Fixed { driver_id }
                if driver_id.as_str() == "legacy.frozen"
        )
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn test_store() -> ProviderStore {
        ProviderStore {
            providers: vec![StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "provider-1".to_string(),
                    name: "Provider One".to_string(),
                    settings_config: json!({
                        "auth": {"OPENAI_API_KEY": "secret-value"},
                        "base_url": "https://example.test/v1",
                    }),
                    category: Some("api".to_string()),
                    meta: Some(ProviderMeta {
                        provider_type: Some("openrouter".to_string()),
                        ..Default::default()
                    }),
                    extra: Default::default(),
                },
                provider_type: ProviderType::OpenRouter,
                provider_type_id: "openrouter".to_string(),
                resource: ProviderResourceMetadata {
                    profile_id: Some(ProfileId::parse("codex.openrouter").unwrap()),
                    profile_schema_revision: Some(1),
                    revision: 3,
                    credential_generation: 2,
                    ..Default::default()
                },
            }],
            ..Default::default()
        }
    }

    #[test]
    fn s2_roundtrip_keeps_plaintext_out_of_json_and_materializes_on_demand() {
        let mut store = test_store();
        seal_store(
            &mut store,
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::File,
            },
        )
        .unwrap();
        store.format = ProviderStoreFormat::S2;
        store.store_generation = 1;
        let value = encode_s2(&store).unwrap();
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains("secret-value"));

        #[derive(Deserialize)]
        struct OldDecoder {
            #[serde(default, rename = "providers")]
            _providers: Vec<Value>,
        }
        assert!(serde_json::from_value::<OldDecoder>(value.clone()).is_err());

        let decoded = decode_s2(
            value,
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::File,
            },
        )
        .unwrap();
        let provider = materialize_provider(&decoded, &decoded.providers[0]).unwrap();
        assert_eq!(
            provider.settings_config["auth"]["OPENAI_API_KEY"],
            "secret-value"
        );
    }

    #[test]
    fn s2_decode_rederives_typed_provider_type_from_profile_identity() {
        let mut store = test_store();
        store.providers[0]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .provider_type = Some("grok_oauth".to_string());
        seal_store(
            &mut store,
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::File,
            },
        )
        .unwrap();
        store.format = ProviderStoreFormat::S2;
        store.store_generation = 1;

        let decoded = decode_s2(
            encode_s2(&store).unwrap(),
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::File,
            },
        )
        .unwrap();

        assert_eq!(decoded.providers[0].provider_type, ProviderType::OpenRouter);
        assert_eq!(decoded.providers[0].provider_type_id, "openrouter");
    }

    #[test]
    fn s2_wrong_key_and_tampered_ciphertext_fail_closed() {
        let mut store = test_store();
        seal_store(
            &mut store,
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::Environment,
            },
        )
        .unwrap();
        store.format = ProviderStoreFormat::S2;
        store.store_generation = 1;
        let value = encode_s2(&store).unwrap();
        assert!(decode_s2(
            value.clone(),
            ResolvedCredentialKey {
                key: [8u8; 32],
                source: CredentialKeySource::Environment,
            }
        )
        .is_err());

        let mut tampered = value;
        let ciphertext = tampered
            .pointer("/records/codex/provider-1/credentials/slots/~1settingsConfig~1auth~1OPENAI_API_KEY/ciphertext")
            .and_then(Value::as_str)
            .unwrap()
            .to_string();
        *tampered
            .pointer_mut("/records/codex/provider-1/credentials/slots/~1settingsConfig~1auth~1OPENAI_API_KEY/ciphertext")
            .unwrap() = Value::String(format!("{ciphertext}AA"));
        assert!(decode_s2(
            tampered,
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::Environment,
            }
        )
        .is_err());
    }
}
