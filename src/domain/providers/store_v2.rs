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
use super::store::{
    CursorVerifiedIdentity, ProviderResourceMetadata, ProviderStore, ProviderStoreFormat,
    StoredProvider,
};

pub(crate) const PROVIDER_STORE_FORMAT: &str = "cc-switch-provider-store";
pub(crate) const PROVIDER_STORE_SCHEMA_VERSION: u32 = 4;
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
    aliases: BTreeMap<ProviderKey, ProviderKey>,
    sealed: bool,
}

impl std::fmt::Debug for ProviderCredentialVault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderCredentialVault")
            .field("key_source", &self.key_source)
            .field("key_id", &self.key_id)
            .field("envelope_count", &self.envelopes.len())
            .field("alias_count", &self.aliases.len())
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bundle_order: Vec<String>,
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
    credential_source: Option<ProviderKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    custom_binding: Option<CustomBindingInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    legacy_payload: Option<LegacyPayloadS2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    create_request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor_verified_identity: Option<CursorVerifiedIdentity>,
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
    let mut plaintext_credentials = BTreeMap::new();
    for stored in &mut store.providers {
        let (redacted, credentials) = split_provider_credentials(&stored.provider)
            .with_context(|| provider_context(stored, "split credentials"))?;
        if !credentials.is_empty() {
            let provider_key = ProviderKey::new(stored.app, stored.provider.id.clone())?;
            plaintext_credentials.insert(provider_key, credentials);
        }
        stored.provider = redacted;
    }

    let mut envelopes = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    for stored in &store.providers {
        let provider_key = ProviderKey::new(stored.app, stored.provider.id.clone())?;
        let source_key = super::bundle::shared_credential_source_key(stored)?;
        let credentials = plaintext_credentials.get(&provider_key);
        match source_key {
            Some(source_key) if source_key != provider_key => {
                let source_credentials =
                    plaintext_credentials.get(&source_key).with_context(|| {
                        format!(
                            "shared credential source {}:{} is not configured",
                            source_key.app.as_str(),
                            source_key.provider_id
                        )
                    })?;
                if credentials != Some(source_credentials) {
                    anyhow::bail!(
                        "Provider Bundle credential slots differ between {} and {}",
                        provider_key.app.as_str(),
                        source_key.app.as_str()
                    );
                }
                aliases.insert(provider_key, source_key);
            }
            _ => {
                if let Some(credentials) = credentials {
                    let envelope = encrypt_envelope(stored, credentials, key.as_ref(), &key_id)?;
                    envelopes.insert(provider_key, envelope);
                }
            }
        }
    }

    store.credential_vault = Arc::new(ProviderCredentialVault {
        key: Some(key),
        key_source: Some(resolved.source),
        key_id: Some(key_id),
        envelopes,
        aliases,
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
    materialized.credential_vault = Arc::new(ProviderCredentialVault::default());
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
    let source_key = store
        .credential_vault
        .aliases
        .get(&provider_key)
        .unwrap_or(&provider_key);
    let Some(envelope) = store.credential_vault.envelopes.get(source_key) else {
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
    let source_stored = store
        .providers
        .iter()
        .find(|candidate| {
            candidate.app == source_key.app && candidate.provider.id == source_key.provider_id
        })
        .context("Provider credential source record is missing")?;
    let credentials = decrypt_envelope(source_stored, envelope, key.as_ref(), key_id)?;
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
        let credential_source = store.credential_vault.aliases.get(&key).cloned();
        if envelope.is_some() {
            referenced_envelopes.insert(key.clone(), ());
        }
        let record = record_from_stored(stored, envelope, credential_source)?;
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
    for (alias, source) in &store.credential_vault.aliases {
        if alias == source || !store.credential_vault.envelopes.contains_key(source) {
            anyhow::bail!("Provider credential vault contains an invalid alias");
        }
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
        bundle_order: store.bundle_order.clone(),
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
    let mut aliases = BTreeMap::new();

    for (app, app_records) in persisted.records {
        for (record_key, record) in app_records {
            if record_key != record.provider_id {
                anyhow::bail!("Provider S2 record map key does not match providerId");
            }
            let (stored, envelope, credential_source) = stored_from_record(app, record)?;
            if let Some(envelope) = envelope {
                let key_ref = ProviderKey::new(app, stored.provider.id.clone())?;
                validate_envelope_shape(&stored, &envelope, &expected_key_id)?;
                if envelopes.insert(key_ref, envelope).is_some() {
                    anyhow::bail!("duplicate Provider credential envelope");
                }
            }
            if let Some(source) = credential_source {
                let alias = ProviderKey::new(app, stored.provider.id.clone())?;
                if aliases.insert(alias, source).is_some() {
                    anyhow::bail!("duplicate Provider credential alias");
                }
            }
            providers.push(stored);
        }
    }

    let mut store = ProviderStore {
        providers,
        order: persisted.order_by_app,
        bundle_order: persisted.bundle_order,
        runtime_index: Default::default(),
        runtime_defaults: Default::default(),
        format: ProviderStoreFormat::S2,
        store_generation: persisted.store_generation,
        credential_vault: Arc::new(ProviderCredentialVault {
            key: Some(key),
            key_source: Some(resolved.source),
            key_id: Some(expected_key_id),
            envelopes,
            aliases,
            sealed: true,
        }),
    };
    store.validate_for_commit()?;
    for (alias, source) in &store.credential_vault.aliases {
        if alias == source || !store.credential_vault.envelopes.contains_key(source) {
            anyhow::bail!("Provider credential alias references an invalid source");
        }
        let alias_record = store
            .providers
            .iter()
            .find(|stored| stored.app == alias.app && stored.provider.id == alias.provider_id)
            .context("Provider credential alias record is missing")?;
        let source_record = store
            .providers
            .iter()
            .find(|stored| stored.app == source.app && stored.provider.id == source.provider_id)
            .context("Provider credential source record is missing")?;
        if alias_record.resource.credential_generation
            != source_record.resource.credential_generation
        {
            anyhow::bail!("Provider credential alias generation does not match its source");
        }
    }
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
    credential_source: Option<ProviderKey>,
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
        credential_source,
        custom_binding: stored.resource.custom_binding.clone(),
        legacy_payload,
        create_request_id: stored.resource.create_request_id.clone(),
        cursor_verified_identity: stored.resource.cursor_verified_identity.clone(),
    })
}

fn stored_from_record(
    app: AppKind,
    record: ProviderRecordS2,
) -> anyhow::Result<(
    StoredProvider,
    Option<CredentialEnvelope>,
    Option<ProviderKey>,
)> {
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
        cursor_verified_identity: record.cursor_verified_identity,
    };
    let (provider_type, provider_type_id) = match record.legacy_payload {
        Some(legacy) => (legacy.provider_type, legacy.provider_type_id),
        None => {
            let provider_type = super::store::canonical_provider_type(app, &provider, &resource)?;
            (provider_type, provider_type.as_str().to_string())
        }
    };
    let credential_source = record.credential_source;
    if record.credentials.is_some() && credential_source.is_some() {
        anyhow::bail!("Provider S2 record cannot contain credentials and a credential source");
    }
    Ok((
        StoredProvider {
            app,
            provider,
            provider_type,
            provider_type_id,
            resource,
        },
        record.credentials,
        credential_source,
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
    use std::collections::BTreeMap;

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
                    cursor_verified_identity: Some(CursorVerifiedIdentity {
                        schema_version: 1,
                        account_id: "cursor_apikey_fixture".to_string(),
                        principal_source: "user_id".to_string(),
                        verified_at_ms: 1_700_000_000_000,
                        email: None,
                        display_name: None,
                        credential_name: None,
                        subscription_level: None,
                    }),
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
        assert_eq!(
            decoded.providers[0]
                .resource
                .cursor_verified_identity
                .as_ref()
                .map(|identity| identity.account_id.as_str()),
            Some("cursor_apikey_fixture")
        );

        let mut materialized = decoded.materialized_clone().unwrap();
        materialized.providers[0].provider.settings_config["auth"]["OPENAI_API_KEY"] =
            json!("rotated-secret");
        assert_eq!(
            materialized
                .materialize_provider_record(&materialized.providers[0])
                .unwrap()
                .provider
                .settings_config["auth"]["OPENAI_API_KEY"],
            "rotated-secret"
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

    #[test]
    fn managed_account_bundle_does_not_require_provider_credential_aliases() {
        let providers = [
            (AppKind::Claude, "claude.grok_oauth"),
            (AppKind::Codex, "codex.grok_oauth"),
            (AppKind::Gemini, "gemini.grok_oauth"),
        ]
        .into_iter()
        .map(|(app, profile_id)| StoredProvider {
            app,
            provider: Provider {
                id: "managed-bundle".to_string(),
                name: "Managed Bundle".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some("grok_oauth".to_string()),
                    auth_binding: Some(crate::domain::providers::model::AuthBinding {
                        source: Some("account".to_string()),
                        auth_provider: Some("grok_oauth".to_string()),
                        account_id: Some("account-1".to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: BTreeMap::from([
                    ("bundleId".to_string(), json!("managed-bundle")),
                    ("familyId".to_string(), json!("family.grok_oauth")),
                    ("surfaceEnabled".to_string(), json!(true)),
                ]),
            },
            provider_type: ProviderType::GrokOAuth,
            provider_type_id: ProviderType::GrokOAuth.as_str().to_string(),
            resource: ProviderResourceMetadata {
                profile_id: Some(ProfileId::parse(profile_id).unwrap()),
                profile_schema_revision: Some(1),
                revision: 1,
                ..ProviderResourceMetadata::default()
            },
        })
        .collect();
        let mut store = ProviderStore {
            providers,
            bundle_order: vec!["managed-bundle".to_string()],
            ..ProviderStore::default()
        };

        seal_store(
            &mut store,
            ResolvedCredentialKey {
                key: [7u8; 32],
                source: CredentialKeySource::File,
            },
        )
        .unwrap();

        assert!(store.credential_vault.envelopes.is_empty());
        assert!(store.credential_vault.aliases.is_empty());
    }
}
