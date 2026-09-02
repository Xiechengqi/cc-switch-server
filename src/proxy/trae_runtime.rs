use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::domain::trae::{TraeAccountProfile, TRAE_DEFAULT_MODEL};

const MAX_RUNTIME_ENTRIES: usize = 128;
const MAX_MODEL_CATALOG_ENTRIES: usize = 256;
const MAX_MODEL_ID_BYTES: usize = 256;
const MAX_STALE_CATALOG_AGE_MS: i64 = 24 * 60 * 60 * 1_000;
pub const TRAE_MODEL_CATALOG_TTL_MS: i64 = 8 * 60 * 1_000;

const HIDDEN_CONFIG_NAMES: &[&str] = &[
    "browser_use_subagent",
    "file_search_agent",
    "explore_sub_agent_v2",
    "summary",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraeRuntimeScope(String);

impl TraeRuntimeScope {
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        profile: &TraeAccountProfile,
        auth_identity_generation: u64,
        token_refresh_generation: u64,
    ) -> Result<Self, String> {
        let required = [
            app.trim(),
            provider_id.trim(),
            runtime_fingerprint.trim(),
            account_id.trim(),
            profile.uid.trim(),
            profile.machine_id.trim(),
            profile.device_id.trim(),
        ];
        if required.iter().any(|value| value.is_empty()) {
            return Err("Trae runtime scope contains an empty identity component".to_string());
        }
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-server:trae-cn-solo-runtime:v1\0");
        for value in [
            required[0],
            required[1],
            &provider_revision.to_string(),
            required[2],
            required[3],
            "cn",
            required[4],
            profile.enterprise_id.trim(),
            required[5],
            required[6],
            &auth_identity_generation.to_string(),
            &token_refresh_generation.to_string(),
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Ok(Self(format!("trae-runtime-v1:{:x}", digest.finalize())))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraeModelCapability {
    pub id: String,
    pub display_name: String,
    pub context_window: Option<u64>,
    pub context_window_max: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub prompt_max_tokens: Option<u64>,
    pub supports_tools: bool,
    pub supports_reasoning: bool,
    pub reasoning_efforts: Vec<String>,
    pub reasoning_default: Option<String>,
    pub reasoning_type: Option<String>,
    pub max_mode: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TraeModelCatalog {
    pub enabled_models: Vec<String>,
    pub raw_configs: BTreeMap<String, Value>,
    pub capabilities: BTreeMap<String, TraeModelCapability>,
    pub fetched_at_ms: i64,
    pub stale: bool,
    expires_at_ms: i64,
}

impl TraeModelCatalog {
    pub fn exact_config(&self, model_id: &str) -> Option<&Value> {
        self.raw_configs.get(model_id.trim())
    }

    pub fn resolve_model(&self, requested: &str) -> Result<String, String> {
        let mut requested = requested.trim();
        if let Some(stripped) = requested.strip_prefix("trae/") {
            requested = stripped.trim();
        }
        if requested.eq_ignore_ascii_case("auto") {
            requested = TRAE_DEFAULT_MODEL;
        }
        if requested.is_empty() {
            return Err("Trae request model is empty".to_string());
        }
        if self.raw_configs.contains_key(requested) {
            return Ok(requested.to_string());
        }
        let lowercase = requested.to_ascii_lowercase();
        let case_matches = self
            .enabled_models
            .iter()
            .filter(|candidate| candidate.to_ascii_lowercase() == lowercase)
            .collect::<Vec<_>>();
        if case_matches.len() == 1 {
            return Ok(case_matches[0].to_string());
        }
        let canonical = canonical_model_id(requested);
        let canonical_matches = self
            .enabled_models
            .iter()
            .filter(|candidate| canonical_model_id(candidate) == canonical)
            .collect::<Vec<_>>();
        if canonical_matches.len() == 1 {
            return Ok(canonical_matches[0].to_string());
        }
        Err(format!(
            "Trae model {requested:?} is not present in the bound account catalog"
        ))
    }

    fn is_fresh(&self, now_ms: i64) -> bool {
        self.expires_at_ms > now_ms
    }

    fn is_usable_stale(&self, now_ms: i64) -> bool {
        now_ms.saturating_sub(self.fetched_at_ms) <= MAX_STALE_CATALOG_AGE_MS
    }

    fn as_stale(&self) -> Self {
        let mut stale = self.clone();
        stale.stale = true;
        stale
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FetchedTraeModelCatalog {
    pub enabled_models: Vec<String>,
    pub raw_configs: BTreeMap<String, Value>,
    pub capabilities: BTreeMap<String, TraeModelCapability>,
}

#[derive(Clone)]
pub struct PreparedTraeRuntime {
    pub scope: TraeRuntimeScope,
    pub profile: TraeAccountProfile,
    pub access_token: Arc<str>,
    pub agent_origin: String,
    pub billing_origin: String,
    pub catalog: TraeModelCatalog,
    pub account_id: String,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
}

impl std::fmt::Debug for PreparedTraeRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedTraeRuntime")
            .field("scope", &self.scope)
            .field("uid", &self.profile.uid)
            .field("agent_origin", &self.agent_origin)
            .field("billing_origin", &self.billing_origin)
            .field("account_id", &self.account_id)
            .field("auth_identity_generation", &self.auth_identity_generation)
            .field("token_refresh_generation", &self.token_refresh_generation)
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
pub struct TraeRuntimeCache {
    catalogs: RwLock<HashMap<TraeRuntimeScope, TraeModelCatalog>>,
    catalog_flights: Mutex<HashMap<TraeRuntimeScope, Arc<Mutex<()>>>>,
}

impl std::fmt::Debug for TraeRuntimeCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TraeRuntimeCache")
            .finish_non_exhaustive()
    }
}

impl TraeRuntimeCache {
    pub async fn catalog(&self, scope: &TraeRuntimeScope, now_ms: i64) -> Option<TraeModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .filter(|catalog| catalog.is_fresh(now_ms))
            .cloned()
    }

    pub async fn stale_catalog(
        &self,
        scope: &TraeRuntimeScope,
        now_ms: i64,
    ) -> Option<TraeModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .filter(|catalog| catalog.is_usable_stale(now_ms))
            .map(TraeModelCatalog::as_stale)
    }

    pub async fn insert_catalog(
        &self,
        scope: TraeRuntimeScope,
        fetched: FetchedTraeModelCatalog,
        now_ms: i64,
    ) -> TraeModelCatalog {
        let catalog = TraeModelCatalog {
            enabled_models: fetched.enabled_models,
            raw_configs: fetched.raw_configs,
            capabilities: fetched.capabilities,
            fetched_at_ms: now_ms,
            stale: false,
            expires_at_ms: now_ms.saturating_add(TRAE_MODEL_CATALOG_TTL_MS),
        };
        let mut catalogs = self.catalogs.write().await;
        catalogs.insert(scope, catalog.clone());
        while catalogs.len() > MAX_RUNTIME_ENTRIES {
            let Some(oldest) = catalogs
                .iter()
                .min_by_key(|(_, catalog)| catalog.fetched_at_ms)
                .map(|(scope, _)| scope.clone())
            else {
                break;
            };
            catalogs.remove(&oldest);
        }
        catalog
    }

    pub async fn invalidate_scope(&self, scope: &TraeRuntimeScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub async fn catalog_lock(&self, scope: &TraeRuntimeScope) -> OwnedMutexGuard<()> {
        let flight = {
            let mut flights = self.catalog_flights.lock().await;
            flights.retain(|key, flight| key == scope || Arc::strong_count(flight) > 1);
            flights
                .entry(scope.clone())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        flight.lock_owned().await
    }
}

pub fn parse_trae_model_catalog(value: &Value) -> Result<FetchedTraeModelCatalog, String> {
    if let Some(code) = value
        .get("Code")
        .or_else(|| value.get("code"))
        .and_then(json_i64)
    {
        if code != 0 {
            return Err(format!("Trae model detail returned business code {code}"));
        }
    }
    let entries = [
        "/config_info_list",
        "/Result/config_info_list",
        "/result/config_info_list",
        "/data/config_info_list",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer))
    .and_then(Value::as_array)
    .ok_or_else(|| "Trae model detail must contain config_info_list array".to_string())?;
    if entries.len() > MAX_MODEL_CATALOG_ENTRIES {
        return Err(format!(
            "Trae model detail exceeds {MAX_MODEL_CATALOG_ENTRIES} entries"
        ));
    }

    let mut seen = BTreeMap::<String, ()>::new();
    let mut enabled_models = Vec::new();
    let mut raw_configs = BTreeMap::new();
    let mut capabilities = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| format!("Trae model detail entry {index} must be an object"))?;
        let id = object
            .get("config_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("Trae model detail entry {index} is missing config_name"))?;
        if id.len() > MAX_MODEL_ID_BYTES {
            return Err("Trae model id exceeds the size limit".to_string());
        }
        let duplicate_key = id.to_ascii_lowercase();
        if seen.insert(duplicate_key, ()).is_some() {
            return Err(format!(
                "Trae model detail contains duplicate config_name {id:?}"
            ));
        }
        let invisible = optional_bool(object.get("is_invisible_to_user"), "is_invisible_to_user")?
            .unwrap_or(false);
        let display_config = optional_object(object.get("display_config"), "display_config")?;
        let display_name = display_config
            .and_then(|display| display.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default();
        let hidden = HIDDEN_CONFIG_NAMES
            .iter()
            .any(|hidden| id.eq_ignore_ascii_case(hidden));
        if invisible
            || hidden
            || id.starts_with("custom_model_")
            || display_name.is_empty()
            || display_name == "-"
        {
            continue;
        }
        let capability = parse_model_capability(id, display_name, object, display_config)?;
        enabled_models.push(id.to_string());
        raw_configs.insert(id.to_string(), entry.clone());
        capabilities.insert(id.to_string(), capability);
    }
    enabled_models.sort_by_key(|id| id.to_ascii_lowercase());
    Ok(FetchedTraeModelCatalog {
        enabled_models,
        raw_configs,
        capabilities,
    })
}

fn parse_model_capability(
    id: &str,
    display_name: &str,
    object: &Map<String, Value>,
    display_config: Option<&Map<String, Value>>,
) -> Result<TraeModelCapability, String> {
    let context = optional_object(object.get("context_window_tokens"), "context_window_tokens")?;
    let context_window = context.and_then(|value| positive_u64(value.get("dev")));
    let context_window_max = context.and_then(|value| positive_u64(value.get("max")));
    let display_max_mode = display_config
        .and_then(|value| value.get("max_mode"))
        .map(|value| required_bool(value, "display_config.max_mode"))
        .transpose()?
        .unwrap_or(false);
    let dollar_max = display_config
        .and_then(|value| value.get("is_dollar_max"))
        .map(|value| required_bool(value, "display_config.is_dollar_max"))
        .transpose()?
        .unwrap_or(false);
    let display_reasoning = display_config
        .and_then(|value| value.get("model_capability"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("reasoning_model"));

    let details = match object.get("model_detail_list") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(details)) => details.as_slice(),
        Some(_) => return Err("Trae model_detail_list must be an array".to_string()),
    };
    let mut max_output_tokens = None;
    let mut prompt_max_tokens = None;
    let mut reasoning_type = None;
    let mut detail_max_mode = false;
    for detail in details {
        let detail = detail
            .as_object()
            .ok_or_else(|| "Trae model detail item must be an object".to_string())?;
        max_output_tokens = max_output_tokens.or_else(|| positive_u64(detail.get("max_tokens")));
        prompt_max_tokens =
            prompt_max_tokens.or_else(|| positive_u64(detail.get("prompt_max_tokens")));
        if let Some(extra) = detail
            .get("model_extra_config")
            .and_then(Value::as_str)
            .and_then(parse_embedded_object)
        {
            detail_max_mode |= extra
                .get("v2_max_mode_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if reasoning_type.is_none() {
                reasoning_type = extra
                    .get("Thinking")
                    .or_else(|| extra.get("thinking"))
                    .and_then(Value::as_object)
                    .and_then(|thinking| {
                        thinking
                            .get("Type")
                            .or_else(|| thinking.get("type"))
                            .and_then(Value::as_str)
                    })
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_ascii_lowercase());
            }
        }
    }
    let (mut reasoning_efforts, mut reasoning_default) =
        parse_reasoning_effort_config(object.get("reasoning_effort_config"))?;
    if reasoning_efforts.is_empty() {
        let values = object
            .get("reasoning_effort_options")
            .and_then(Value::as_array)
            .map(|values| values.as_slice())
            .unwrap_or(&[]);
        (reasoning_efforts, reasoning_default) = parse_reasoning_option_list(
            values,
            object
                .get("default_reasoning_effort")
                .and_then(Value::as_str),
        );
    }
    let contact_reasoning = parse_contact_reasoning(object.get("display_contact_config"));
    let supports_reasoning = !reasoning_efforts.is_empty()
        || reasoning_type
            .as_deref()
            .is_some_and(|value| value != "disabled")
        || display_reasoning
        || contact_reasoning;
    if reasoning_efforts.is_empty() && supports_reasoning {
        reasoning_efforts = vec!["low".to_string(), "high".to_string(), "xhigh".to_string()];
        reasoning_default = Some("high".to_string());
    }
    Ok(TraeModelCapability {
        id: id.to_string(),
        display_name: display_name.to_string(),
        context_window,
        context_window_max,
        max_output_tokens,
        prompt_max_tokens,
        supports_tools: true,
        supports_reasoning,
        reasoning_efforts,
        reasoning_default,
        reasoning_type,
        max_mode: display_max_mode
            || dollar_max
            || detail_max_mode
            || matches!((context_window, context_window_max), (Some(dev), Some(max)) if dev != max),
    })
}

fn parse_reasoning_effort_config(
    value: Option<&Value>,
) -> Result<(Vec<String>, Option<String>), String> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok((Vec::new(), None));
    };
    let owned;
    let value = if let Some(encoded) = value.as_str() {
        owned = serde_json::from_str::<Value>(encoded)
            .map_err(|_| "Trae reasoning_effort_config is not valid JSON".to_string())?;
        &owned
    } else {
        value
    };
    let object = value
        .as_object()
        .ok_or_else(|| "Trae reasoning_effort_config must be an object".to_string())?;
    if !optional_bool(object.get("support_thinking"), "support_thinking")?.unwrap_or(false) {
        return Ok((Vec::new(), None));
    }
    let values = object
        .get("options")
        .and_then(Value::as_array)
        .map(|values| values.as_slice())
        .unwrap_or(&[]);
    let (options, default) =
        parse_reasoning_option_list(values, object.get("default_level").and_then(Value::as_str));
    if options.is_empty() || default.is_none() {
        return Err("Trae reasoning effort config is incomplete".to_string());
    }
    Ok((options, default))
}

fn parse_reasoning_option_list(
    values: &[Value],
    default: Option<&str>,
) -> (Vec<String>, Option<String>) {
    let mut options = Vec::new();
    for value in values {
        let Some(value) = value.as_str().and_then(normalize_reasoning_level) else {
            continue;
        };
        if !options.contains(&value) {
            options.push(value);
        }
    }
    if options.is_empty() {
        return (options, None);
    }
    let default = default
        .and_then(normalize_reasoning_level)
        .filter(|default| options.contains(default))
        .or_else(|| {
            options
                .iter()
                .find(|value| value.as_str() == "medium")
                .cloned()
        })
        .or_else(|| options.first().cloned());
    (options, default)
}

fn normalize_reasoning_level(value: &str) -> Option<String> {
    let normalized = match value.trim().to_ascii_lowercase().as_str() {
        "minimal" | "min" | "low" => "low",
        "medium" | "med" | "default" => "medium",
        "high" => "high",
        "xhigh" | "extra_high" | "extra-high" | "max" => "xhigh",
        _ => return None,
    };
    Some(normalized.to_string())
}

fn parse_contact_reasoning(value: Option<&Value>) -> bool {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return false;
    };
    let owned;
    let value = if let Some(encoded) = value.as_str() {
        let Ok(parsed) = serde_json::from_str::<Value>(encoded) else {
            return false;
        };
        owned = parsed;
        &owned
    } else {
        value
    };
    value
        .pointer("/reasoning/enable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn parse_embedded_object(value: &str) -> Option<Map<String, Value>> {
    let value = value.trim();
    if !value.starts_with('{') {
        return None;
    }
    serde_json::from_str::<Value>(value)
        .ok()?
        .as_object()
        .cloned()
}

fn optional_object<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<Option<&'a Map<String, Value>>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value)),
        Some(_) => Err(format!("Trae {field} must be an object")),
    }
}

fn optional_bool(value: Option<&Value>, field: &str) -> Result<Option<bool>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("Trae {field} must be boolean")),
    }
}

fn required_bool(value: &Value, field: &str) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("Trae {field} must be boolean"))
}

fn positive_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn canonical_model_id(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| {
            if matches!(character, '_' | ' ' | '/') {
                '-'
            } else {
                character.to_ascii_lowercase()
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(enterprise_id: &str) -> TraeAccountProfile {
        TraeAccountProfile {
            uid: "uid-1".to_string(),
            enterprise_id: enterprise_id.to_string(),
            name: String::new(),
            email: String::new(),
            machine_id: "machine-1".to_string(),
            device_id: "device-1".to_string(),
        }
    }

    fn scope(enterprise_id: &str, token_generation: u64) -> TraeRuntimeScope {
        TraeRuntimeScope::derive(
            "claude",
            "provider-a",
            2,
            "runtime-a",
            "account-a",
            &profile(enterprise_id),
            7,
            token_generation,
        )
        .unwrap()
    }

    #[test]
    fn trae_catalog_keeps_visible_models_and_authoritative_empty() {
        let catalog = parse_trae_model_catalog(&serde_json::json!({
            "config_info_list": [
                {
                    "config_name": "GLM_5.2",
                    "is_invisible_to_user": false,
                    "context_window_tokens": {"dev": 128000, "max": 256000},
                    "display_config": {
                        "display_name": "GLM 5.2",
                        "max_mode": false,
                        "model_capability": "reasoning_model"
                    },
                    "reasoning_effort_config": {
                        "support_thinking": true,
                        "options": ["low", "high", "xhigh"],
                        "default_level": "high"
                    },
                    "model_detail_list": [{
                        "max_tokens": 32000,
                        "prompt_max_tokens": 120000,
                        "model_extra_config": "{\"v2_max_mode_enabled\":true,\"Thinking\":{\"Type\":\"enabled\"}}"
                    }]
                },
                {
                    "config_name": "summary",
                    "display_config": {"display_name": "Internal"}
                }
            ]
        }))
        .unwrap();
        assert_eq!(catalog.enabled_models, ["GLM_5.2"]);
        let capability = &catalog.capabilities["GLM_5.2"];
        assert!(capability.max_mode);
        assert!(capability.supports_reasoning);
        assert_eq!(capability.reasoning_efforts, ["low", "high", "xhigh"]);
        assert_eq!(capability.context_window, Some(128_000));
        assert_eq!(capability.max_output_tokens, Some(32_000));

        assert!(
            parse_trae_model_catalog(&serde_json::json!({"config_info_list": []}))
                .unwrap()
                .enabled_models
                .is_empty()
        );
        assert!(parse_trae_model_catalog(&serde_json::json!({})).is_err());
    }

    #[test]
    fn trae_catalog_resolution_preserves_native_config_name() {
        let fetched = parse_trae_model_catalog(&serde_json::json!({
            "config_info_list": [{
                "config_name": "GLM_5.2",
                "display_config": {"display_name": "GLM"}
            }]
        }))
        .unwrap();
        let catalog = TraeModelCatalog {
            enabled_models: fetched.enabled_models,
            raw_configs: fetched.raw_configs,
            capabilities: fetched.capabilities,
            fetched_at_ms: 0,
            stale: false,
            expires_at_ms: i64::MAX,
        };
        assert_eq!(catalog.resolve_model("glm-5.2").unwrap(), "GLM_5.2");
        assert!(catalog.resolve_model("unknown").is_err());
    }

    #[tokio::test]
    async fn trae_cache_fences_device_enterprise_and_both_generations() {
        let cache = TraeRuntimeCache::default();
        let current = scope("enterprise-1", 4);
        cache
            .insert_catalog(
                current.clone(),
                FetchedTraeModelCatalog {
                    enabled_models: vec!["GLM_5.2".to_string()],
                    raw_configs: BTreeMap::new(),
                    capabilities: BTreeMap::new(),
                },
                1_000,
            )
            .await;
        assert!(cache.catalog(&current, 1_001).await.is_some());
        assert!(cache
            .catalog(&scope("enterprise-2", 4), 1_001)
            .await
            .is_none());
        assert!(cache
            .catalog(&scope("enterprise-1", 5), 1_001)
            .await
            .is_none());
        assert!(
            cache
                .stale_catalog(&current, 1_000 + TRAE_MODEL_CATALOG_TTL_MS + 1)
                .await
                .unwrap()
                .stale
        );
        assert!(cache
            .stale_catalog(&current, 1_000 + MAX_STALE_CATALOG_AGE_MS + 1)
            .await
            .is_none());
    }
}
