use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::domain::providers::store::StoredProvider;
use crate::infra::resident_memory::{
    process_resident_memory, ResidentMemoryBudget, ResidentMemoryReservation,
};

const MANIFEST_CACHE_SCOPE_LIMIT: usize = 4;
const MANIFEST_TTL: Duration = Duration::from_secs(5 * 60);
const MANIFEST_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexModelCapability {
    pub id: &'static str,
    pub reasoning_efforts: &'static [&'static str],
    pub input_modalities: &'static [&'static str],
    pub service_tiers: &'static [&'static str],
    pub use_responses_lite: bool,
    pub context_window: Option<u64>,
    pub supports_reasoning_effort_updates: Option<bool>,
}

const GPT_56_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];
const GPT_6_ASTRA_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const GPT_56_LUNA_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const STANDARD_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
const TEXT_IMAGE: &[&str] = &["text", "image"];
const TEXT_ONLY: &[&str] = &["text"];
const PRIORITY: &[&str] = &["priority"];
const NO_SERVICE_TIERS: &[&str] = &[];

pub(crate) const BUILTIN_CODEX_MODELS: &[CodexModelCapability] = &[
    CodexModelCapability {
        id: "gpt-6.2-sol",
        reasoning_efforts: GPT_6_ASTRA_EFFORTS,
        input_modalities: &["text", "image", "audio"],
        service_tiers: &["priority", "ultrafast"],
        use_responses_lite: true,
        context_window: Some(500_000),
        supports_reasoning_effort_updates: Some(true),
    },
    CodexModelCapability {
        id: "gpt-6.2-astra",
        reasoning_efforts: GPT_6_ASTRA_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: true,
        context_window: Some(272_000),
        supports_reasoning_effort_updates: Some(true),
    },
    // Keep a cold-start capability for Astra. The bound account manifest still
    // wins field-by-field and remains the entitlement authority.
    CodexModelCapability {
        id: "gpt-6-astra",
        reasoning_efforts: GPT_6_ASTRA_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: true,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.6-sol",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: true,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.6-terra",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: true,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.6-luna",
        reasoning_efforts: GPT_56_LUNA_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: true,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.5",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: false,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.4",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
        use_responses_lite: false,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.4-mini",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: NO_SERVICE_TIERS,
        use_responses_lite: false,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.3-codex-spark",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_ONLY,
        service_tiers: NO_SERVICE_TIERS,
        use_responses_lite: false,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "gpt-5.2",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: NO_SERVICE_TIERS,
        use_responses_lite: false,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
    CodexModelCapability {
        id: "codex-auto-review",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: NO_SERVICE_TIERS,
        use_responses_lite: true,
        context_window: None,
        supports_reasoning_effort_updates: None,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilitySupport {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedCodexModelCapability {
    pub reasoning_efforts: Option<Vec<String>>,
    pub input_modalities: Option<Vec<String>>,
    pub service_tiers: Option<Vec<String>>,
    pub use_responses_lite: Option<bool>,
    pub context_window: Option<u64>,
    pub supports_search_tool: Option<bool>,
    pub supports_reasoning_effort_updates: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelCapability {
    id: String,
    reasoning_efforts: Option<Vec<String>>,
    input_modalities: Option<Vec<String>>,
    service_tiers: Option<Vec<String>>,
    use_responses_lite: Option<bool>,
    context_window: Option<u64>,
    supports_search_tool: Option<bool>,
    supports_reasoning_effort_updates: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelScope {
    account_id: String,
    auth_identity_generation: u64,
    client_version: String,
}

#[derive(Debug)]
struct ManifestModelCacheEntry {
    scope: ManifestModelScope,
    models: Vec<ManifestModelCapability>,
    expires_at: Instant,
    _memory: ResidentMemoryReservation,
}

#[derive(Debug)]
struct ManifestModelCache {
    entries: Vec<ManifestModelCacheEntry>,
    parent: ResidentMemoryBudget,
    index_memory: Option<ResidentMemoryReservation>,
}

impl Default for ManifestModelCache {
    fn default() -> Self {
        Self::with_parent(process_resident_memory())
    }
}

impl ManifestModelCache {
    fn with_parent(parent: ResidentMemoryBudget) -> Self {
        Self {
            entries: Vec::new(),
            parent,
            index_memory: None,
        }
    }

    #[cfg(test)]
    fn update_from_body(&mut self, scope: Option<ManifestModelScope>, body: &[u8]) -> usize {
        let Some(models) = parse_manifest_models(body) else {
            return self.model_ids(scope.as_ref()).len();
        };
        self.replace(scope, models)
    }

    fn replace(
        &mut self,
        scope: Option<ManifestModelScope>,
        models: Vec<ManifestModelCapability>,
    ) -> usize {
        let Some(scope) = scope else {
            return 0;
        };
        let bytes = models.iter().fold(
            scope
                .account_id
                .capacity()
                .saturating_add(scope.client_version.capacity())
                .saturating_add(
                    models
                        .capacity()
                        .saturating_mul(std::mem::size_of::<ManifestModelCapability>()),
                ),
            |bytes, model| {
                [
                    &model.reasoning_efforts,
                    &model.input_modalities,
                    &model.service_tiers,
                ]
                .into_iter()
                .fold(
                    bytes.saturating_add(model.id.capacity()),
                    |bytes, values| {
                        values.as_ref().map_or(bytes, |values| {
                            values.iter().fold(
                                bytes.saturating_add(
                                    values
                                        .capacity()
                                        .saturating_mul(std::mem::size_of::<String>()),
                                ),
                                |bytes, value| bytes.saturating_add(value.capacity()),
                            )
                        })
                    },
                )
            },
        );
        let Some(memory) = self.parent.reserve(bytes) else {
            return self.model_ids(Some(&scope)).len();
        };
        if self.index_memory.is_none() {
            let Some(index_memory) = self.parent.reserve(
                MANIFEST_CACHE_SCOPE_LIMIT
                    .saturating_mul(std::mem::size_of::<ManifestModelCacheEntry>()),
            ) else {
                return self.model_ids(Some(&scope)).len();
            };
            self.entries = Vec::with_capacity(MANIFEST_CACHE_SCOPE_LIMIT);
            self.index_memory = Some(index_memory);
        }
        self.entries.retain(|entry| entry.scope != scope);
        if self.entries.len() == MANIFEST_CACHE_SCOPE_LIMIT {
            self.entries.remove(0);
        }
        let model_count = models.len();
        self.entries.push(ManifestModelCacheEntry {
            scope,
            models,
            expires_at: Instant::now() + MANIFEST_TTL,
            _memory: memory,
        });
        model_count
    }

    fn model_ids(&self, scope: Option<&ManifestModelScope>) -> Vec<String> {
        let Some(scope) = scope else {
            return Vec::new();
        };
        self.entries
            .iter()
            .rev()
            .find(|entry| &entry.scope == scope && entry.expires_at > Instant::now())
            .map(|entry| entry.models.iter().map(|model| model.id.clone()).collect())
            .unwrap_or_default()
    }

    fn capability(
        &self,
        scope: Option<&ManifestModelScope>,
        model: &str,
    ) -> Option<ManifestModelCapability> {
        let scope = scope?;
        let model = normalize_model_id(model);
        self.entries
            .iter()
            .rev()
            .find(|entry| &entry.scope == scope && entry.expires_at > Instant::now())?
            .models
            .iter()
            .find(|capability| normalize_model_id(&capability.id) == model)
            .cloned()
    }
}

fn manifest_model_cache() -> &'static RwLock<ManifestModelCache> {
    static CACHE: OnceLock<RwLock<ManifestModelCache>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(ManifestModelCache::default()))
}

fn manifest_model_scope(provider: &StoredProvider) -> Option<ManifestModelScope> {
    let binding = provider.provider.meta.as_ref()?.auth_binding.as_ref()?;
    let account_id = binding.account_id.as_deref()?.trim();
    if account_id.is_empty() {
        return None;
    }
    Some(ManifestModelScope {
        account_id: account_id.to_string(),
        auth_identity_generation: binding.auth_identity_generation?,
        client_version: crate::codex_identity::configured_version(),
    })
}

#[cfg(test)]
pub(crate) fn update_manifest_models(provider: &StoredProvider, body: &[u8]) -> usize {
    update_manifest_models_for_version(provider, &crate::codex_identity::configured_version(), body)
}

pub(crate) fn update_manifest_models_for_version(
    provider: &StoredProvider,
    version: &str,
    body: &[u8],
) -> usize {
    let scope = manifest_model_scope(provider).map(|mut scope| {
        scope.client_version = version.to_string();
        scope
    });
    let previous_count = || {
        manifest_model_cache()
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .model_ids(scope.as_ref())
            .len()
    };
    let Some(_working_memory) =
        process_resident_memory().reserve(super::request_memory::json_parse_working_bytes(body))
    else {
        return previous_count();
    };
    let Some(models) = parse_manifest_models(body) else {
        return previous_count();
    };
    // Parsing is outside the global cache lock; only bounded snapshot install
    // and eviction need exclusive access.
    manifest_model_cache()
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .replace(scope, models)
}

/// One bounded singleflight slot per identity/version, independent of inference leases.
struct ManifestRefreshFlight {
    scope: ManifestModelScope,
    gate: Arc<tokio::sync::Mutex<()>>,
    last_attempt: std::sync::Mutex<Option<Instant>>,
    _memory: ResidentMemoryReservation,
}

pub(crate) struct ManifestRefreshGuard {
    flight: Arc<ManifestRefreshFlight>,
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl ManifestRefreshGuard {
    pub(crate) fn client_version(&self) -> &str {
        &self.flight.scope.client_version
    }

    pub(crate) fn should_refresh(&self) -> bool {
        let cache = manifest_model_cache()
            .read()
            .unwrap_or_else(|p| p.into_inner());
        if cache
            .entries
            .iter()
            .any(|entry| entry.scope == self.flight.scope && entry.expires_at > Instant::now())
        {
            return false;
        }
        let mut attempted = self
            .flight
            .last_attempt
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if attempted.is_some_and(|at| at.elapsed() < MANIFEST_RETRY_DELAY) {
            return false;
        }
        *attempted = Some(Instant::now());
        true
    }
}

pub(crate) async fn manifest_refresh_guard(
    provider: &StoredProvider,
) -> Option<ManifestRefreshGuard> {
    static FLIGHTS: OnceLock<std::sync::Mutex<Vec<Arc<ManifestRefreshFlight>>>> = OnceLock::new();
    let scope = manifest_model_scope(provider)?;
    let flight = {
        let mut flights = FLIGHTS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(flight) = flights.iter().find(|flight| flight.scope == scope) {
            Arc::clone(flight)
        } else {
            if flights.len() >= 16 {
                let index = flights
                    .iter()
                    .position(|flight| Arc::strong_count(flight) == 1)?;
                flights.remove(index);
            }
            let memory = process_resident_memory()
                .reserve(2048 + scope.account_id.len() + scope.client_version.len())?;
            let flight = Arc::new(ManifestRefreshFlight {
                scope,
                gate: Arc::new(tokio::sync::Mutex::new(())),
                last_attempt: std::sync::Mutex::new(None),
                _memory: memory,
            });
            flights.push(Arc::clone(&flight));
            flight
        }
    };
    let guard = Arc::clone(&flight.gate).lock_owned().await;
    Some(ManifestRefreshGuard {
        flight,
        _guard: guard,
    })
}

fn parse_manifest_models(body: &[u8]) -> Option<Vec<ManifestModelCapability>> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let models = value.get("models")?.as_array()?;
    if models.len() > 512 {
        return None;
    }
    let mut parsed = BTreeMap::new();
    for model in models {
        let Some(id) = model
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|slug| valid_capability_token(slug, 128))
        else {
            continue;
        };
        if parsed.contains_key(id) {
            return None;
        }
        parsed.insert(
            id.to_string(),
            ManifestModelCapability {
                id: id.to_string(),
                reasoning_efforts: parse_capability_values(
                    model,
                    "supported_reasoning_levels",
                    "effort",
                ),
                input_modalities: parse_string_capability_values(model, "input_modalities"),
                service_tiers: parse_capability_values(model, "service_tiers", "id"),
                use_responses_lite: model.get("use_responses_lite").and_then(Value::as_bool),
                context_window: model.get("context_window").and_then(Value::as_u64),
                supports_search_tool: model.get("supports_search_tool").and_then(Value::as_bool),
                supports_reasoning_effort_updates: model
                    .get("supports_reasoning_effort_updates")
                    .and_then(Value::as_bool),
            },
        );
    }
    Some(parsed.into_values().collect())
}

fn parse_capability_values(model: &Value, array_key: &str, value_key: &str) -> Option<Vec<String>> {
    let values = model.get(array_key)?.as_array()?;
    if values.len() > 64 {
        return None;
    }
    Some(normalize_capability_values(values.iter().filter_map(
        |value| value.get(value_key).and_then(Value::as_str),
    )))
}

fn parse_string_capability_values(model: &Value, array_key: &str) -> Option<Vec<String>> {
    let values = model.get(array_key)?.as_array()?;
    if values.len() > 64 {
        return None;
    }
    Some(normalize_capability_values(
        values.iter().filter_map(Value::as_str),
    ))
}

fn normalize_capability_values<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut values = values
        .map(str::trim)
        .filter(|value| valid_capability_token(value, 64))
        .map(|value| value.to_ascii_lowercase())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn valid_capability_token(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn manifest_model_ids(provider: &StoredProvider) -> Vec<String> {
    let scope = manifest_model_scope(provider);
    manifest_model_cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .model_ids(scope.as_ref())
}

pub(crate) fn capability_for_model(model: &str) -> Option<&'static CodexModelCapability> {
    let model = normalize_model_id(model);
    BUILTIN_CODEX_MODELS
        .iter()
        .find(|capability| model == capability.id)
}

pub(crate) fn resolved_capability_for_model(
    provider: &StoredProvider,
    model: &str,
) -> Option<ResolvedCodexModelCapability> {
    let manifest = manifest_model_cache()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .capability(manifest_model_scope(provider).as_ref(), model);
    let builtin = capability_for_model(model);
    if manifest.is_none() && builtin.is_none() {
        return None;
    }
    Some(ResolvedCodexModelCapability {
        reasoning_efforts: manifest
            .as_ref()
            .and_then(|capability| capability.reasoning_efforts.clone())
            .or_else(|| builtin.map(|capability| strings(capability.reasoning_efforts))),
        input_modalities: manifest
            .as_ref()
            .and_then(|capability| capability.input_modalities.clone())
            .or_else(|| builtin.map(|capability| strings(capability.input_modalities))),
        service_tiers: manifest
            .as_ref()
            .and_then(|capability| capability.service_tiers.clone())
            .or_else(|| builtin.map(|capability| strings(capability.service_tiers))),
        use_responses_lite: manifest
            .as_ref()
            .and_then(|capability| capability.use_responses_lite)
            .or_else(|| builtin.map(|capability| capability.use_responses_lite)),
        context_window: manifest
            .as_ref()
            .and_then(|capability| capability.context_window)
            .or_else(|| builtin.and_then(|capability| capability.context_window)),
        supports_reasoning_effort_updates: manifest
            .as_ref()
            .and_then(|capability| capability.supports_reasoning_effort_updates)
            .or_else(|| {
                builtin.and_then(|capability| capability.supports_reasoning_effort_updates)
            }),
        supports_search_tool: manifest
            .as_ref()
            .and_then(|capability| capability.supports_search_tool),
    })
}

pub(crate) fn responses_lite_support(provider: &StoredProvider, model: &str) -> CapabilitySupport {
    match resolved_capability_for_model(provider, model)
        .and_then(|capability| capability.use_responses_lite)
    {
        Some(true) => CapabilitySupport::Supported,
        Some(false) => CapabilitySupport::Unsupported,
        None => CapabilitySupport::Unknown,
    }
}

pub(crate) fn service_tier_support(
    provider: &StoredProvider,
    model: &str,
    service_tier: &str,
) -> CapabilitySupport {
    let Some(capability) = resolved_capability_for_model(provider, model) else {
        return CapabilitySupport::Unknown;
    };
    let Some(service_tiers) = capability.service_tiers else {
        return CapabilitySupport::Unknown;
    };
    if service_tiers
        .iter()
        .any(|tier| tier.eq_ignore_ascii_case(service_tier))
    {
        CapabilitySupport::Supported
    } else {
        CapabilitySupport::Unsupported
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

pub(crate) fn normalize_reasoning_effort(_model: &str, effort: &str) -> String {
    let effort = effort.trim().to_ascii_lowercase();
    if effort == "ultra" {
        "max".to_string()
    } else {
        effort
    }
}

fn normalize_model_id(model: &str) -> String {
    model
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
        .replace('_', "-")
}

/// Applies the transport-specific structured-output subset observed for Codex.
/// String length constraints are accepted only by Astra on native Responses
/// Lite WebSocket. Tool schemas are normalized separately and never use this
/// exception.
pub(crate) fn normalize_structured_output_for_transport(
    body: &mut Value,
    final_model: Option<&str>,
    websocket: bool,
    responses_lite: bool,
) {
    let preserve_string_lengths = websocket
        && responses_lite
        && final_model.is_some_and(|model| normalize_model_id(model) == "gpt-6-astra");
    if preserve_string_lengths {
        return;
    }
    let Some(format) = body.pointer_mut("/text/format") else {
        return;
    };
    if let Some(schema) = format.get_mut("schema") {
        strip_structured_string_lengths(schema);
    }
    if let Some(schema) = format.pointer_mut("/json_schema/schema") {
        strip_structured_string_lengths(schema);
    }
}

fn strip_structured_string_lengths(schema: &mut Value) {
    let mut stack = vec![schema];
    while let Some(value) = stack.pop() {
        match value {
            Value::Object(object) => {
                object.remove("minLength");
                object.remove("maxLength");
                stack.extend(object.values_mut());
            }
            Value::Array(values) => stack.extend(values.iter_mut()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::{
        AppKind, AuthBinding, Provider, ProviderMeta, ProviderType,
    };
    use crate::domain::providers::store::ProviderResourceMetadata;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn manifest_provider(account_id: &str, auth_identity_generation: u64) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: format!("provider-{account_id}-{auth_identity_generation}"),
                name: "Codex OAuth".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    provider_type: Some("codex_oauth".to_string()),
                    auth_binding: Some(AuthBinding {
                        source: Some("account".to_string()),
                        auth_provider: Some("codex_oauth".to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(auth_identity_generation),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: BTreeMap::new(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: ProviderType::CodexOAuth.as_str().to_string(),
            resource: ProviderResourceMetadata::default(),
        }
    }

    #[test]
    fn preserves_wire_reasoning_effort_except_for_ultra() {
        assert_eq!(normalize_reasoning_effort("gpt-5.6-sol", "ultra"), "max");
        assert_eq!(normalize_reasoning_effort("gpt-5.6-luna", "ultra"), "max");
        assert_eq!(normalize_reasoning_effort("gpt-5.5", "max"), "max");
        assert_eq!(normalize_reasoning_effort("vendor/model", "max"), "max");
        assert_eq!(
            BUILTIN_CODEX_MODELS[0].input_modalities,
            ["text", "image", "audio"]
        );
        assert_eq!(BUILTIN_CODEX_MODELS[0].id, "gpt-6.2-sol");
        assert!(BUILTIN_CODEX_MODELS[0].reasoning_efforts.contains(&"max"));
        assert!(BUILTIN_CODEX_MODELS[0].use_responses_lite);
    }

    #[test]
    fn manifest_cache_expires_and_isolates_client_versions() {
        let provider = manifest_provider("cache-version-test", 1);
        let scope = manifest_model_scope(&provider).unwrap();
        let mut cache = ManifestModelCache::default();
        cache.replace(Some(scope.clone()), vec![manifest_model("model-a")]);
        assert!(cache.capability(Some(&scope), "model-a").is_some());
        let mut newer = scope.clone();
        newer.client_version = "99.0.0".into();
        assert!(cache.model_ids(Some(&newer)).is_empty());
        assert!(cache.capability(Some(&newer), "model-a").is_none());
        cache.entries[0].expires_at = Instant::now() - Duration::from_secs(1);
        assert!(cache.model_ids(Some(&scope)).is_empty());
        assert!(cache.capability(Some(&scope), "model-a").is_none());
        cache.replace(Some(scope.clone()), vec![manifest_model("model-b")]);
        assert_eq!(cache.model_ids(Some(&scope)), ["model-b"]);
    }

    #[tokio::test]
    async fn manifest_refresh_singleflight_serializes_and_backs_off_failures() {
        use futures_util::FutureExt;
        let provider = manifest_provider("refresh-singleflight-test", 1);
        let guard = manifest_refresh_guard(&provider).await.unwrap();
        assert_eq!(
            guard.client_version(),
            crate::codex_identity::configured_version()
        );
        assert!(guard.should_refresh());
        let mut waiter = Box::pin(manifest_refresh_guard(&provider));
        assert!(waiter.as_mut().now_or_never().is_none());
        drop(guard);
        let guard = waiter.await.unwrap();
        assert!(
            !guard.should_refresh(),
            "failed refreshes must not stampede"
        );
        *guard.flight.last_attempt.lock().unwrap() = Some(Instant::now() - MANIFEST_RETRY_DELAY);
        assert!(
            guard.should_refresh(),
            "backoff is bounded even without a cache entry"
        );
    }

    #[test]
    fn manifest_parser_rejects_duplicates_and_bounds_model_count() {
        assert!(parse_manifest_models(
            br#"{"models":[{"slug":"duplicate"},{"slug":"duplicate"}]}"#
        )
        .is_none());
        let models = (0..513)
            .map(|i| json!({"slug": format!("model-{i}")}))
            .collect::<Vec<_>>();
        assert!(
            parse_manifest_models(&serde_json::to_vec(&json!({"models": models})).unwrap())
                .is_none()
        );
        let parsed = parse_manifest_models(br#"{"models":[{"slug":"gpt-6.2-sol","supports_reasoning_effort_updates":true,"context_window":500000}]}"#).unwrap();
        assert_eq!(parsed[0].supports_reasoning_effort_updates, Some(true));
        assert_eq!(parsed[0].context_window, Some(500_000));
    }

    #[test]
    fn astra_lite_websocket_preserves_structured_string_lengths_only() {
        let template = json!({
            "text": {"format": {"type": "json_schema", "schema": {
                "type": "object",
                "properties": {"name": {"type": "string", "minLength": 2, "maxLength": 8}}
            }}},
            "tools": [{"type": "function", "parameters": {
                "type": "object", "properties": {"query": {"type": "string", "minLength": 1}}
            }}]
        });

        let mut astra_ws_lite = template.clone();
        normalize_structured_output_for_transport(
            &mut astra_ws_lite,
            Some("gpt-6-astra"),
            true,
            true,
        );
        assert_eq!(
            astra_ws_lite.pointer("/text/format/schema/properties/name/minLength"),
            Some(&json!(2))
        );

        for (model, websocket, lite) in [
            ("gpt-6-astra", false, true),
            ("gpt-6-astra", true, false),
            ("gpt-5.6-luna", true, true),
        ] {
            let mut body = template.clone();
            normalize_structured_output_for_transport(&mut body, Some(model), websocket, lite);
            assert!(body
                .pointer("/text/format/schema/properties/name/minLength")
                .is_none());
            assert!(body
                .pointer("/text/format/schema/properties/name/maxLength")
                .is_none());
            assert_eq!(
                body.pointer("/tools/0/parameters/properties/query/minLength"),
                Some(&json!(1))
            );
        }
    }

    #[test]
    fn astra_builtin_is_overridden_field_by_field_by_manifest() {
        let provider = manifest_provider("astra-manifest-account", 1);
        update_manifest_models(
            &provider,
            br#"{"models":[{"slug":"gpt-6-astra","use_responses_lite":false,"service_tiers":[],"supported_reasoning_levels":[{"effort":"high"}]}]}"#,
        );
        let capability = resolved_capability_for_model(&provider, "gpt-6-astra").unwrap();
        assert_eq!(capability.reasoning_efforts, Some(vec!["high".to_string()]));
        assert_eq!(capability.service_tiers, Some(Vec::new()));
        assert_eq!(capability.use_responses_lite, Some(false));
        assert_eq!(capability.input_modalities, Some(strings(TEXT_IMAGE)));
    }

    #[test]
    fn manifest_models_are_validated_sorted_and_ignore_invalid_ids() {
        let models = parse_manifest_models(
            br#"{"models":[{"slug":"gpt-5.6-sol"},{"slug":" gpt-5.5 "},{"slug":"bad/model"},{"slug":""},{"id":"ignored"}]}"#,
        )
        .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.clone())
                .collect::<Vec<_>>(),
            vec!["gpt-5.5".to_string(), "gpt-5.6-sol".to_string()]
        );
        assert!(parse_manifest_models(b"not-json").is_none());
        assert!(parse_manifest_models(br#"{"models":null}"#).is_none());
    }

    #[test]
    fn manifest_preserves_explicit_empty_and_missing_capabilities() {
        let models = parse_manifest_models(
            br#"{"models":[{"slug":"known-empty","service_tiers":[],"supported_reasoning_levels":[{"effort":"HIGH"}],"context_window":400000,"supports_search_tool":true},{"slug":"unknown-fields"}]}"#,
        )
        .unwrap();
        assert_eq!(models[0].id, "known-empty");
        assert_eq!(models[0].service_tiers, Some(Vec::new()));
        assert_eq!(models[0].reasoning_efforts, Some(vec!["high".to_string()]));
        assert_eq!(models[0].context_window, Some(400000));
        assert_eq!(models[0].supports_search_tool, Some(true));
        assert_eq!(models[1].id, "unknown-fields");
        assert_eq!(models[1].service_tiers, None);
        assert_eq!(models[1].reasoning_efforts, None);
        assert_eq!(models[1].context_window, None);
        assert_eq!(models[1].supports_search_tool, None);
    }

    #[test]
    fn responses_lite_support_is_tri_state_and_manifest_overrides_builtin() {
        let provider = manifest_provider("responses-lite-capability", 1);
        update_manifest_models(
            &provider,
            br#"{"models":[{"slug":"gpt-5.6-sol","use_responses_lite":false},{"slug":"manifest-lite","use_responses_lite":true},{"slug":"manifest-unknown"}]}"#,
        );
        assert_eq!(
            responses_lite_support(&provider, "gpt-5.6-sol"),
            CapabilitySupport::Unsupported
        );
        assert_eq!(
            responses_lite_support(&provider, "manifest-lite"),
            CapabilitySupport::Supported
        );
        assert_eq!(
            responses_lite_support(&provider, "manifest-unknown"),
            CapabilitySupport::Unknown
        );
        assert_eq!(
            responses_lite_support(&provider, "future-model"),
            CapabilitySupport::Unknown
        );
    }

    #[test]
    fn manifest_service_tier_support_is_tri_state() {
        let provider = manifest_provider("service-tier-capability", 1);
        update_manifest_models(
            &provider,
            br#"{"models":[{"slug":"manifest-supported","service_tiers":[{"id":"priority"}]},{"slug":"manifest-unsupported","service_tiers":[]},{"slug":"manifest-unknown"}]}"#,
        );

        assert_eq!(
            service_tier_support(&provider, "manifest-supported", "priority"),
            CapabilitySupport::Supported
        );
        assert_eq!(
            service_tier_support(&provider, "manifest-unsupported", "priority"),
            CapabilitySupport::Unsupported
        );
        assert_eq!(
            service_tier_support(&provider, "manifest-unknown", "priority"),
            CapabilitySupport::Unknown
        );
    }

    #[test]
    fn manifest_cache_isolated_by_account_identity_generation() {
        let first = manifest_provider("account-a", 1);
        let same_identity = manifest_provider("account-a", 1);
        let reauthorized = manifest_provider("account-a", 2);
        let switched = manifest_provider("account-b", 1);
        let mut cache = ManifestModelCache::default();

        cache.replace(
            manifest_model_scope(&first),
            vec![manifest_model("account-a-model")],
        );

        assert_eq!(
            cache.model_ids(manifest_model_scope(&same_identity).as_ref()),
            ["account-a-model"]
        );
        assert!(cache
            .model_ids(manifest_model_scope(&reauthorized).as_ref())
            .is_empty());
        assert!(cache
            .model_ids(manifest_model_scope(&switched).as_ref())
            .is_empty());

        cache.replace(
            manifest_model_scope(&switched),
            vec![manifest_model("account-b-model")],
        );
        assert_eq!(
            cache.model_ids(manifest_model_scope(&first).as_ref()),
            ["account-a-model"]
        );
        assert_eq!(
            cache.model_ids(manifest_model_scope(&switched).as_ref()),
            ["account-b-model"]
        );
    }

    #[test]
    fn manifest_cache_is_bounded_without_stale_scope_overwrite() {
        let mut cache = ManifestModelCache::default();
        for generation in 1..=MANIFEST_CACHE_SCOPE_LIMIT as u64 + 1 {
            let provider = manifest_provider("account-a", generation);
            cache.replace(
                manifest_model_scope(&provider),
                vec![manifest_model(&format!("model-{generation}"))],
            );
        }

        assert!(cache
            .model_ids(manifest_model_scope(&manifest_provider("account-a", 1)).as_ref())
            .is_empty());
        assert_eq!(
            cache.model_ids(
                manifest_model_scope(&manifest_provider(
                    "account-a",
                    MANIFEST_CACHE_SCOPE_LIMIT as u64 + 1,
                ))
                .as_ref(),
            ),
            [format!("model-{}", MANIFEST_CACHE_SCOPE_LIMIT + 1)]
        );
    }

    #[test]
    fn invalid_manifest_keeps_last_valid_snapshot() {
        let provider = manifest_provider("last-known-good-account", 1);
        let mut cache = ManifestModelCache::default();
        cache.update_from_body(
            manifest_model_scope(&provider),
            br#"{"models":[{"slug":"gpt-last-known-good"}]}"#,
        );

        assert_eq!(
            cache.update_from_body(manifest_model_scope(&provider), b"not-json"),
            1
        );
        assert_eq!(
            cache.model_ids(manifest_model_scope(&provider).as_ref()),
            ["gpt-last-known-good"]
        );
    }

    #[test]
    fn codex_r4_manifest_owners_release_on_replacement_eviction_and_drop() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = ManifestModelCache::with_parent(parent.clone());
        let scope = manifest_model_scope(&manifest_provider("owner-account", 1));
        let mut model = manifest_model("large-capability");
        model.reasoning_efforts = Some(vec!["effort".repeat(1000)]);
        assert_eq!(cache.replace(scope.clone(), vec![model]), 1);
        let large = parent.snapshot().used_bytes;
        assert!(large > 6_000);
        assert_eq!(
            cache.replace(scope.clone(), vec![manifest_model("small")]),
            1
        );
        assert!(parent.snapshot().used_bytes < large);
        for generation in 2..=8 {
            cache.replace(
                manifest_model_scope(&manifest_provider("owner-account", generation)),
                vec![manifest_model("other")],
            );
        }
        assert!(cache.model_ids(scope.as_ref()).is_empty());
        assert_eq!(cache.entries.len(), MANIFEST_CACHE_SCOPE_LIMIT);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
        assert_eq!(parent.snapshot().active_reservations, 0);
    }

    #[test]
    fn codex_r4_manifest_capacity_denial_keeps_last_good_identity_snapshot() {
        let parent = ResidentMemoryBudget::new(None);
        let mut cache = ManifestModelCache::with_parent(parent.clone());
        let scope = manifest_model_scope(&manifest_provider("capacity-account", 1));
        cache.replace(scope.clone(), vec![manifest_model("known-good")]);
        let used = parent.snapshot().used_bytes;
        parent.configure_limit(Some(used));
        assert_eq!(
            cache.replace(scope.clone(), vec![manifest_model("rejected")]),
            1
        );
        assert_eq!(cache.model_ids(scope.as_ref()), ["known-good"]);
        assert_eq!(parent.snapshot().used_bytes, used);
        assert!(parent.snapshot().rejections > 0);
        drop(cache);
        assert_eq!(parent.snapshot().used_bytes, 0);
    }

    fn manifest_model(id: &str) -> ManifestModelCapability {
        ManifestModelCapability {
            id: id.to_string(),
            reasoning_efforts: None,
            input_modalities: None,
            service_tiers: None,
            use_responses_lite: None,
            context_window: None,
            supports_search_tool: None,
            supports_reasoning_effort_updates: None,
        }
    }
}
