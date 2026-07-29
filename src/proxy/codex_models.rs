use std::sync::{OnceLock, RwLock};

use serde_json::Value;

use crate::domain::providers::store::StoredProvider;

const MANIFEST_CACHE_SCOPE_LIMIT: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexModelCapability {
    pub id: &'static str,
    pub reasoning_efforts: &'static [&'static str],
    pub input_modalities: &'static [&'static str],
}

const STANDARD_EFFORTS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];
const GPT_56_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max", "ultra"];
const GPT_56_LUNA_EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const TEXT_IMAGE: &[&str] = &["text", "image"];

pub(crate) const BUILTIN_CODEX_MODELS: &[CodexModelCapability] = &[
    CodexModelCapability {
        id: "gpt-5.6-sol",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
    },
    CodexModelCapability {
        id: "gpt-5.6-terra",
        reasoning_efforts: GPT_56_EFFORTS,
        input_modalities: TEXT_IMAGE,
    },
    CodexModelCapability {
        id: "gpt-5.6-luna",
        reasoning_efforts: GPT_56_LUNA_EFFORTS,
        input_modalities: TEXT_IMAGE,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelScope {
    account_id: String,
    auth_identity_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestModelCacheEntry {
    scope: ManifestModelScope,
    models: Vec<String>,
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

    fn replace(&mut self, scope: Option<ManifestModelScope>, models: Vec<String>) -> usize {
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
            .map(|entry| entry.models.clone())
            .unwrap_or_default()
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

fn parse_manifest_models(body: &[u8]) -> Option<Vec<String>> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let models = value.get("models")?.as_array()?;
    let mut models = models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .map(str::trim)
        .filter(|slug| {
            !slug.is_empty()
                && slug.len() <= 128
                && slug
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Some(models)
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

pub(crate) fn normalize_reasoning_effort(model: &str, effort: &str) -> String {
    let model = normalize_model_id(model);
    let effort = effort.trim().to_ascii_lowercase();
    if let Some(capability) = BUILTIN_CODEX_MODELS
        .iter()
        .find(|capability| model.starts_with(capability.id))
    {
        if capability.reasoning_efforts.contains(&effort.as_str()) {
            return effort;
        }
        return match effort.as_str() {
            "ultra" if capability.reasoning_efforts.contains(&"max") => "max".to_string(),
            "max" | "ultra" => "xhigh".to_string(),
            _ => effort,
        };
    }
    if parse_gpt_version(&model).is_some() && !STANDARD_EFFORTS.contains(&effort.as_str()) {
        return match effort.as_str() {
            "max" | "ultra" => "xhigh".to_string(),
            _ => effort,
        };
    }
    effort
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

fn parse_gpt_version(model: &str) -> Option<(u32, u32)> {
    let rest = model.strip_prefix("gpt-")?;
    let mut parts = rest.split(|character: char| !character.is_ascii_digit() && character != '.');
    let version = parts.next()?;
    let mut numbers = version.split('.');
    let major = numbers.next()?.parse().ok()?;
    let minor = numbers.next().unwrap_or("0").parse().ok()?;
    Some((major, minor))
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
    fn gates_gpt_56_reasoning_from_capability_registry() {
        assert_eq!(normalize_reasoning_effort("gpt-5.6-sol", "ultra"), "ultra");
        assert_eq!(normalize_reasoning_effort("gpt-5.6-luna", "ultra"), "max");
        assert_eq!(normalize_reasoning_effort("gpt-5.5", "max"), "xhigh");
        assert_eq!(normalize_reasoning_effort("vendor/model", "max"), "max");
        assert_eq!(BUILTIN_CODEX_MODELS[0].input_modalities, ["text", "image"]);
    }

    #[test]
    fn manifest_models_are_validated_sorted_and_deduplicated() {
        assert_eq!(
            parse_manifest_models(
                br#"{"models":[{"slug":"gpt-5.6-sol"},{"slug":" gpt-5.5 "},{"slug":"gpt-5.6-sol"},{"slug":"bad/model"},{"slug":""},{"id":"ignored"}]}"#,
            )
            .unwrap(),
            vec!["gpt-5.5".to_string(), "gpt-5.6-sol".to_string()]
        );
        assert!(parse_manifest_models(b"not-json").is_none());
        assert!(parse_manifest_models(br#"{"models":null}"#).is_none());
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
            vec!["account-a-model".to_string()],
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
            vec!["account-b-model".to_string()],
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
                vec![format!("model-{generation}")],
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
}
