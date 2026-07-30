use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use serde_json::Value;

use crate::domain::providers::store::StoredProvider;

const MANIFEST_CACHE_SCOPE_LIMIT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexModelCapability {
    pub id: &'static str,
    pub reasoning_efforts: &'static [&'static str],
    pub input_modalities: &'static [&'static str],
    pub service_tiers: &'static [&'static str],
}

const GPT_56_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];
const GPT_56_LUNA_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const STANDARD_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh"];
const TEXT_IMAGE: &[&str] = &["text", "image"];
const TEXT_ONLY: &[&str] = &["text"];
const PRIORITY: &[&str] = &["priority"];
const NO_SERVICE_TIERS: &[&str] = &[];

pub(crate) const BUILTIN_CODEX_MODELS: &[CodexModelCapability] = &[
    CodexModelCapability {
        id: "gpt-5.6-sol",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
    },
    CodexModelCapability {
        id: "gpt-5.6-terra",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
    },
    CodexModelCapability {
        id: "gpt-5.6-luna",
        reasoning_efforts: GPT_56_LUNA_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
    },
    CodexModelCapability {
        id: "gpt-5.5",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
    },
    CodexModelCapability {
        id: "gpt-5.4",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: PRIORITY,
    },
    CodexModelCapability {
        id: "gpt-5.4-mini",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: NO_SERVICE_TIERS,
    },
    CodexModelCapability {
        id: "gpt-5.3-codex-spark",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_ONLY,
        service_tiers: NO_SERVICE_TIERS,
    },
    CodexModelCapability {
        id: "gpt-5.2",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: NO_SERVICE_TIERS,
    },
    CodexModelCapability {
        id: "codex-auto-review",
        reasoning_efforts: STANDARD_EFFORTS,
        input_modalities: TEXT_IMAGE,
        service_tiers: NO_SERVICE_TIERS,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelCapability {
    id: String,
    reasoning_efforts: Option<Vec<String>>,
    input_modalities: Option<Vec<String>>,
    service_tiers: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelScope {
    account_id: String,
    auth_identity_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelCacheEntry {
    scope: ManifestModelScope,
    models: Vec<ManifestModelCapability>,
}

#[derive(Debug, Default)]
struct ManifestModelCache {
    entries: Vec<ManifestModelCacheEntry>,
}

impl ManifestModelCache {
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
        self.entries.retain(|entry| entry.scope != scope);
        let model_count = models.len();
        self.entries.push(ManifestModelCacheEntry { scope, models });
        if self.entries.len() > MANIFEST_CACHE_SCOPE_LIMIT {
            let overflow = self.entries.len() - MANIFEST_CACHE_SCOPE_LIMIT;
            self.entries.drain(..overflow);
        }
        model_count
    }

    fn model_ids(&self, scope: Option<&ManifestModelScope>) -> Vec<String> {
        let Some(scope) = scope else {
            return Vec::new();
        };
        self.entries
            .iter()
            .rev()
            .find(|entry| &entry.scope == scope)
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
            .find(|entry| &entry.scope == scope)?
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
    })
}

pub(crate) fn update_manifest_models(provider: &StoredProvider, body: &[u8]) -> usize {
    manifest_model_cache()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .update_from_body(manifest_model_scope(provider), body)
}

fn parse_manifest_models(body: &[u8]) -> Option<Vec<ManifestModelCapability>> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let models = value.get("models")?.as_array()?;
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
            },
        );
    }
    Some(parsed.into_values().collect())
}

fn parse_capability_values(model: &Value, array_key: &str, value_key: &str) -> Option<Vec<String>> {
    let values = model.get(array_key)?.as_array()?;
    Some(normalize_capability_values(values.iter().filter_map(
        |value| value.get(value_key).and_then(Value::as_str),
    )))
}

fn parse_string_capability_values(model: &Value, array_key: &str) -> Option<Vec<String>> {
    let values = model.get(array_key)?.as_array()?;
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
    })
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
        assert_eq!(BUILTIN_CODEX_MODELS[0].input_modalities, ["text", "image"]);
    }

    #[test]
    fn manifest_models_are_validated_sorted_and_deduplicated() {
        let models = parse_manifest_models(
            br#"{"models":[{"slug":"gpt-5.6-sol"},{"slug":" gpt-5.5 "},{"slug":"gpt-5.6-sol"},{"slug":"bad/model"},{"slug":""},{"id":"ignored"}]}"#,
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
            br#"{"models":[{"slug":"known-empty","service_tiers":[],"supported_reasoning_levels":[{"effort":"HIGH"}]},{"slug":"unknown-fields"}]}"#,
        )
        .unwrap();
        assert_eq!(models[0].id, "known-empty");
        assert_eq!(models[0].service_tiers, Some(Vec::new()));
        assert_eq!(models[0].reasoning_efforts, Some(vec!["high".to_string()]));
        assert_eq!(models[1].id, "unknown-fields");
        assert_eq!(models[1].service_tiers, None);
        assert_eq!(models[1].reasoning_efforts, None);
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

    fn manifest_model(id: &str) -> ManifestModelCapability {
        ManifestModelCapability {
            id: id.to_string(),
            reasoning_efforts: None,
            input_modalities: None,
            service_tiers: None,
        }
    }
}
