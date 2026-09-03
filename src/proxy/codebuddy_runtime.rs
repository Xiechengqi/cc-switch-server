use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OwnedMutexGuard, RwLock};

use crate::domain::codebuddy::{CodeBuddyAccountProfile, CodeBuddySite};

const MAX_RUNTIME_ENTRIES: usize = 128;
const MAX_MODEL_CATALOG_ENTRIES: usize = 256;
const MAX_MODEL_ID_BYTES: usize = 256;
const MAX_STALE_CATALOG_AGE_MS: i64 = 24 * 60 * 60 * 1_000;
pub const CODEBUDDY_MODEL_CATALOG_TTL_MS: i64 = 8 * 60 * 1_000;

const INTL_REVIEWED_MODELS: &[&str] = &[
    "default-model",
    "fast-model",
    "balanced-model",
    "deep-model",
    "gemini-3.1-pro",
    "gemini-3.5-flash",
    "glm-5.3",
    "glm-5.2",
    "glm-5.0",
    "hy3",
    "kimi-k3",
    "kimi-k2.6",
    "kimi-k2.5",
];

const CN_REVIEWED_MODELS: &[&str] = &[
    "default",
    "deepseek-v4-pro",
    "deepseek-v4-flash",
    "deepseek-v3-2-volc",
    "minimax-m3",
    "minimax-m2.7",
    "minimax-m2.5",
    "glm-5.2",
    "glm-5.1",
    "glm-5.0",
    "glm-5.0-turbo",
    "glm-5v-turbo",
    "glm-4.7",
    "glm-4.6",
    "kimi-k3-1",
    "kimi-k2.7",
    "kimi-k2.6",
    "kimi-k2.5",
    "kimi-k2-thinking",
    "hy3",
    "hunyuan-chat",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodeBuddyRuntimeScope(String);

impl CodeBuddyRuntimeScope {
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        app: &str,
        provider_id: &str,
        provider_revision: u64,
        runtime_fingerprint: &str,
        account_id: &str,
        site: CodeBuddySite,
        domain: &str,
        auth_identity_generation: u64,
        token_refresh_generation: u64,
    ) -> Result<Self, String> {
        let required = [
            app.trim(),
            provider_id.trim(),
            runtime_fingerprint.trim(),
            account_id.trim(),
        ];
        if required.iter().any(|value| value.is_empty()) {
            return Err("CodeBuddy runtime scope contains an empty identity component".to_string());
        }
        let domain = site.canonical_token_domain(domain).ok_or_else(|| {
            format!(
                "CodeBuddy runtime scope domain is outside the bound {} site",
                site.as_str()
            )
        })?;
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-server:codebuddy-runtime:v2\0");
        for value in [
            required[0],
            required[1],
            &provider_revision.to_string(),
            required[2],
            required[3],
            site.as_str(),
            &domain,
            &auth_identity_generation.to_string(),
            &token_refresh_generation.to_string(),
        ] {
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Ok(Self(format!(
            "codebuddy-runtime-v2:{:x}",
            digest.finalize()
        )))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBuddyModelCapability {
    pub id: String,
    pub display_name: Option<String>,
    pub reasoning_efforts: Vec<String>,
    pub default_reasoning_effort: Option<String>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub supports_tools: bool,
    pub supports_reasoning: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeBuddyModelCatalog {
    pub enabled_models: Vec<String>,
    /// Reviewed, non-sensitive projection of each enabled vendor config.
    /// The vendor response can contain prompt/UIN/AppId fields and must never
    /// be retained verbatim past the parser boundary.
    pub raw_configs: BTreeMap<String, Value>,
    pub capabilities: BTreeMap<String, CodeBuddyModelCapability>,
    pub fetched_at_ms: i64,
    pub stale: bool,
    expires_at_ms: i64,
}

impl CodeBuddyModelCatalog {
    pub fn exact_config(&self, model_id: &str) -> Option<&Value> {
        self.raw_configs.get(model_id.trim())
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
pub struct FetchedCodeBuddyModelCatalog {
    pub enabled_models: Vec<String>,
    pub raw_configs: BTreeMap<String, Value>,
    pub capabilities: BTreeMap<String, CodeBuddyModelCapability>,
}

#[derive(Clone)]
pub struct PreparedCodeBuddyRuntime {
    pub scope: CodeBuddyRuntimeScope,
    pub profile: CodeBuddyAccountProfile,
    pub access_token: Arc<str>,
    pub base_url: String,
    pub catalog: CodeBuddyModelCatalog,
    pub account_id: String,
    pub auth_identity_generation: u64,
    pub token_refresh_generation: u64,
}

impl std::fmt::Debug for PreparedCodeBuddyRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedCodeBuddyRuntime")
            .field("scope", &self.scope)
            .field("site", &self.profile.site)
            .field("base_url", &self.base_url)
            .field("account_id", &self.account_id)
            .field("auth_identity_generation", &self.auth_identity_generation)
            .field("token_refresh_generation", &self.token_refresh_generation)
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

#[derive(Default)]
pub struct CodeBuddyRuntimeCache {
    catalogs: RwLock<HashMap<CodeBuddyRuntimeScope, CodeBuddyModelCatalog>>,
    catalog_flights: Mutex<HashMap<CodeBuddyRuntimeScope, Arc<Mutex<()>>>>,
}

impl std::fmt::Debug for CodeBuddyRuntimeCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodeBuddyRuntimeCache")
            .finish_non_exhaustive()
    }
}

impl CodeBuddyRuntimeCache {
    pub async fn catalog(
        &self,
        scope: &CodeBuddyRuntimeScope,
        now_ms: i64,
    ) -> Option<CodeBuddyModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .filter(|catalog| catalog.is_fresh(now_ms))
            .cloned()
    }

    pub async fn stale_catalog(
        &self,
        scope: &CodeBuddyRuntimeScope,
        now_ms: i64,
    ) -> Option<CodeBuddyModelCatalog> {
        self.catalogs
            .read()
            .await
            .get(scope)
            .filter(|catalog| catalog.is_usable_stale(now_ms))
            .map(CodeBuddyModelCatalog::as_stale)
    }

    pub async fn insert_catalog(
        &self,
        scope: CodeBuddyRuntimeScope,
        fetched: FetchedCodeBuddyModelCatalog,
        now_ms: i64,
    ) -> CodeBuddyModelCatalog {
        let catalog = CodeBuddyModelCatalog {
            enabled_models: fetched.enabled_models,
            raw_configs: fetched.raw_configs,
            capabilities: fetched.capabilities,
            fetched_at_ms: now_ms,
            stale: false,
            expires_at_ms: now_ms.saturating_add(CODEBUDDY_MODEL_CATALOG_TTL_MS),
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

    pub async fn invalidate_scope(&self, scope: &CodeBuddyRuntimeScope) {
        self.catalogs.write().await.remove(scope);
    }

    pub async fn catalog_lock(&self, scope: &CodeBuddyRuntimeScope) -> OwnedMutexGuard<()> {
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

pub fn parse_codebuddy_model_catalog(
    value: &Value,
    site: CodeBuddySite,
) -> Result<FetchedCodeBuddyModelCatalog, String> {
    if let Some(code) = json_i64(value.get("code")) {
        if code != 0 {
            return Err(format!(
                "CodeBuddy model catalog returned business code {code}"
            ));
        }
    }
    let entries = value
        .pointer("/data/models")
        .or_else(|| value.get("models"))
        .and_then(Value::as_array)
        .ok_or_else(|| "CodeBuddy model catalog must contain data.models array".to_string())?;
    if entries.len() > MAX_MODEL_CATALOG_ENTRIES {
        return Err(format!(
            "CodeBuddy model catalog exceeds {MAX_MODEL_CATALOG_ENTRIES} entries"
        ));
    }

    let reviewed = reviewed_models(site);
    let mut seen = BTreeMap::<String, ()>::new();
    let mut enabled_models = Vec::new();
    let mut raw_configs = BTreeMap::new();
    let mut capabilities = BTreeMap::new();
    let mut unreviewed_enabled = 0_usize;
    for (index, entry) in entries.iter().enumerate() {
        let object = entry
            .as_object()
            .ok_or_else(|| format!("CodeBuddy model catalog entry {index} must be an object"))?;
        let id = object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| format!("CodeBuddy model catalog entry {index} is missing id"))?
            .to_ascii_lowercase();
        if id.len() > MAX_MODEL_ID_BYTES {
            return Err("CodeBuddy model id exceeds the size limit".to_string());
        }
        if seen.insert(id.clone(), ()).is_some() {
            return Err(format!(
                "CodeBuddy model catalog contains duplicate id {id:?}"
            ));
        }
        let disabled = bool_field(object, "disabled")?.unwrap_or(false)
            || bool_field(object, "enable")?.is_some_and(|enabled| !enabled);
        if disabled {
            continue;
        }
        if !reviewed.contains(&id.as_str()) {
            unreviewed_enabled = unreviewed_enabled.saturating_add(1);
            continue;
        }
        let capability = codebuddy_model_capability(&id, object)?;
        let reviewed_config = json!({
            "id": capability.id,
            "name": capability.display_name,
            "supportsToolCall": capability.supports_tools,
            "supportsReasoning": capability.supports_reasoning,
            "reasoning": {"supportedEfforts": capability.reasoning_efforts},
            "maxInputTokens": capability.max_input_tokens,
            "maxOutputTokens": capability.max_output_tokens,
        });
        enabled_models.push(id.clone());
        raw_configs.insert(id.clone(), reviewed_config);
        capabilities.insert(id, capability);
    }
    if !entries.is_empty() && enabled_models.is_empty() {
        return Err(
            "CodeBuddy upstream catalog is non-empty but has no reviewed text model".to_string(),
        );
    }
    if unreviewed_enabled > 0 {
        tracing::warn!(
            site = site.as_str(),
            upstream_model_count = entries.len(),
            unreviewed_enabled,
            reviewed_enabled = enabled_models.len(),
            client_version = crate::domain::codebuddy::CODEBUDDY_CLIENT_VERSION,
            "CodeBuddy live catalog contains enabled models outside the reviewed allowlist"
        );
    }
    enabled_models.sort();
    Ok(FetchedCodeBuddyModelCatalog {
        enabled_models,
        raw_configs,
        capabilities,
    })
}

fn reviewed_models(site: CodeBuddySite) -> &'static [&'static str] {
    match site {
        CodeBuddySite::Intl => INTL_REVIEWED_MODELS,
        CodeBuddySite::Cn => CN_REVIEWED_MODELS,
    }
}

fn codebuddy_model_capability(
    id: &str,
    object: &Map<String, Value>,
) -> Result<CodeBuddyModelCapability, String> {
    let supports_reasoning = bool_field(object, "supportsReasoning")?.unwrap_or_else(|| {
        object
            .get("reasoning")
            .is_some_and(|reasoning| reasoning.is_object())
            || bool_field(object, "onlyReasoning")
                .ok()
                .flatten()
                .unwrap_or(false)
    });
    let reasoning_efforts = object
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("supportedEfforts"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.len() <= 32)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let default_reasoning_effort = object
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| {
            reasoning
                .get("defaultEffort")
                .or_else(|| reasoning.get("effort"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 32)
        .map(str::to_string);
    Ok(CodeBuddyModelCapability {
        id: id.to_string(),
        display_name: object
            .get("name")
            .or_else(|| object.get("descriptionEn"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty() && value.len() <= MAX_MODEL_ID_BYTES)
            .map(str::to_string),
        reasoning_efforts,
        default_reasoning_effort,
        max_input_tokens: positive_u64(object.get("maxInputTokens")),
        max_output_tokens: positive_u64(object.get("maxOutputTokens")),
        // Missing capability evidence cannot authorize a tool-bearing request.
        supports_tools: bool_field(object, "supportsToolCall")?.unwrap_or(false),
        supports_reasoning,
    })
}

fn bool_field(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("CodeBuddy model {key} must be boolean")),
    }
}

fn positive_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

pub fn resolve_codebuddy_model_id(site: CodeBuddySite, requested: &str) -> Result<String, String> {
    let requested = requested
        .trim()
        .strip_prefix("codebuddy/")
        .unwrap_or(requested.trim())
        .trim()
        .to_ascii_lowercase();
    if requested.is_empty() {
        return Err("CodeBuddy request model is empty".to_string());
    }
    if requested == "auto" {
        return Ok(match site {
            CodeBuddySite::Intl => "default-model",
            CodeBuddySite::Cn => "default",
        }
        .to_string());
    }
    Ok(requested)
}

pub fn build_codebuddy_payload(
    canonical_chat_request: &Value,
    model_id: &str,
    capability: &CodeBuddyModelCapability,
) -> Result<Value, String> {
    let mut request = canonical_chat_request
        .as_object()
        .cloned()
        .ok_or_else(|| "CodeBuddy canonical Chat request must be an object".to_string())?;
    let model_id = model_id.trim();
    if model_id.is_empty() {
        return Err("CodeBuddy request model is empty".to_string());
    }
    if contains_image_content(&Value::Object(request.clone())) {
        return Err("CodeBuddy text rail does not support image input".to_string());
    }
    let messages = request
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "CodeBuddy request must contain messages array".to_string())?;
    if messages
        .first()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        != Some("system")
    {
        messages.insert(
            0,
            json!({"role":"system","content":"You are a helpful coding assistant."}),
        );
    }
    for message in messages {
        let Some(message) = message.as_object_mut() else {
            return Err("CodeBuddy message must be an object".to_string());
        };
        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            if !message.contains_key("reasoning") {
                if let Some(reasoning) = message.remove("reasoning_content") {
                    message.insert("reasoning".to_string(), reasoning);
                }
            } else {
                message.remove("reasoning_content");
            }
        }
    }
    normalize_codebuddy_tool_choice(&mut request)?;
    request.insert("model".to_string(), Value::String(model_id.to_string()));
    request.insert("stream".to_string(), Value::Bool(true));
    request.insert("stream_options".to_string(), json!({"include_usage": true}));
    let explicit_effort = match request.get("reasoning_effort") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        Some(Value::String(_)) => None,
        Some(_) => {
            return Err("CodeBuddy reasoning_effort must be a string".to_string());
        }
    };
    let canonical_reasoning = request.remove("reasoning");
    let (canonical_requested, canonical_effort) = match canonical_reasoning {
        None | Some(Value::Null) => (false, None),
        Some(Value::Object(reasoning)) => {
            let effort = match reasoning.get("effort") {
                None | Some(Value::Null) => None,
                Some(Value::String(value)) if !value.trim().is_empty() => {
                    Some(value.trim().to_string())
                }
                Some(Value::String(_)) => None,
                Some(_) => return Err("CodeBuddy reasoning.effort must be a string".to_string()),
            };
            (true, effort)
        }
        Some(_) => return Err("CodeBuddy reasoning must be an object".to_string()),
    };
    if let (Some(explicit), Some(canonical)) = (&explicit_effort, &canonical_effort) {
        if !explicit.eq_ignore_ascii_case(canonical) {
            return Err(
                "CodeBuddy request contains conflicting reasoning effort values".to_string(),
            );
        }
    }
    let effort = explicit_effort.or(canonical_effort).or_else(|| {
        canonical_requested
            .then(|| capability.default_reasoning_effort.clone())
            .flatten()
    });
    request.remove("reasoning_effort");
    if let Some(mut effort) = effort {
        if !capability.supports_reasoning {
            return Err(format!(
                "CodeBuddy model {model_id} does not advertise reasoning support"
            ));
        }
        if !capability.reasoning_efforts.is_empty() {
            let Some(supported) = capability
                .reasoning_efforts
                .iter()
                .find(|supported| supported.eq_ignore_ascii_case(&effort))
            else {
                return Err(format!(
                    "CodeBuddy model {model_id} does not support reasoning effort {effort:?}"
                ));
            };
            effort = supported.clone();
        }
        request.insert("reasoning_effort".to_string(), Value::String(effort));
    }
    Ok(Value::Object(request))
}

fn normalize_codebuddy_tool_choice(
    request: &mut serde_json::Map<String, Value>,
) -> Result<(), String> {
    let Some(choice) = request.get("tool_choice").cloned() else {
        return Ok(());
    };
    let Some(choice) = choice.as_object() else {
        return Ok(());
    };
    if choice.get("type").and_then(Value::as_str) != Some("function") {
        return Err("CodeBuddy named tool_choice type must be function".to_string());
    }
    let name = choice
        .get("function")
        .and_then(Value::as_object)
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "CodeBuddy named tool_choice is missing function.name".to_string())?;
    if name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("CodeBuddy named tool_choice function.name is unsafe".to_string());
    }
    let tools = request
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "CodeBuddy named tool_choice requires a tools array".to_string())?;
    let selected = tools
        .iter()
        .find(|tool| {
            tool.get("type").and_then(Value::as_str) == Some("function")
                && tool
                    .get("function")
                    .and_then(Value::as_object)
                    .and_then(|function| function.get("name"))
                    .and_then(Value::as_str)
                    .is_some_and(|candidate| candidate == name)
        })
        .cloned()
        .ok_or_else(|| {
            format!("CodeBuddy named tool_choice references unknown function {name:?}")
        })?;
    *tools = vec![selected];
    request.insert(
        "tool_choice".to_string(),
        Value::String("required".to_string()),
    );
    Ok(())
}

fn contains_image_content(value: &Value) -> bool {
    match value {
        Value::Object(object) => object.iter().any(|(key, value)| {
            matches!(
                key.as_str(),
                "image" | "image_url" | "input_image" | "inline_data" | "inlineData"
            ) || contains_image_content(value)
        }),
        Value::Array(values) => values.iter().any(contains_image_content),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_capability() -> CodeBuddyModelCapability {
        CodeBuddyModelCapability {
            id: "default-model".to_string(),
            display_name: Some("Default".to_string()),
            reasoning_efforts: vec!["low".to_string(), "high".to_string()],
            default_reasoning_effort: Some("high".to_string()),
            max_input_tokens: Some(200_000),
            max_output_tokens: Some(24_000),
            supports_tools: true,
            supports_reasoning: true,
        }
    }

    fn scope(site: CodeBuddySite, domain: &str, token_generation: u64) -> CodeBuddyRuntimeScope {
        CodeBuddyRuntimeScope::derive(
            "claude",
            "provider-a",
            2,
            "runtime-a",
            "account-a",
            site,
            domain,
            7,
            token_generation,
        )
        .unwrap()
    }

    #[test]
    fn catalog_is_site_reviewed_and_authoritative_empty() {
        let intl = parse_codebuddy_model_catalog(
            &json!({"data":{"models":[
                {"id":"default-model","name":"Default","supportsToolCall":true,"supportsReasoning":true,"reasoning":{"supportedEfforts":["low","high"]},"maxInputTokens":200000},
                {"id":"fast-model","name":"Fast","prompt":"must-not-survive","Uin":"must-not-survive","AppId":"must-not-survive"},
                {"id":"gpt-5.6-sol","name":"Unentitled"}
            ]}}),
            CodeBuddySite::Intl,
        )
        .unwrap();
        assert_eq!(intl.enabled_models, ["default-model", "fast-model"]);
        assert_eq!(
            intl.capabilities["default-model"].reasoning_efforts,
            ["low", "high"]
        );
        assert_eq!(
            intl.capabilities["default-model"].max_input_tokens,
            Some(200_000)
        );
        assert!(!intl.capabilities["fast-model"].supports_tools);
        let reviewed = intl.raw_configs["fast-model"].to_string();
        assert!(!reviewed.contains("must-not-survive"));
        assert!(!reviewed.contains("prompt"));
        assert!(!reviewed.contains("Uin"));
        assert!(!reviewed.contains("AppId"));
        assert!(
            parse_codebuddy_model_catalog(&json!({"data":{"models":[]}}), CodeBuddySite::Intl)
                .unwrap()
                .enabled_models
                .is_empty()
        );
        assert!(parse_codebuddy_model_catalog(
            &json!({"data":{"models":[{"id":"gpt-5.6-sol"}]}}),
            CodeBuddySite::Intl
        )
        .is_err());
        assert!(parse_codebuddy_model_catalog(
            &json!({"data":{"models":[{"id":"default"}]}}),
            CodeBuddySite::Intl
        )
        .is_err());
        assert_eq!(
            parse_codebuddy_model_catalog(
                &json!({"data":{"models":[{"id":"default"}]}}),
                CodeBuddySite::Cn
            )
            .unwrap()
            .enabled_models,
            ["default"]
        );
    }

    #[tokio::test]
    async fn cache_fences_site_and_both_generations_and_bounds_stale() {
        let cache = CodeBuddyRuntimeCache::default();
        let current = scope(CodeBuddySite::Intl, "www.codebuddy.ai", 4);
        cache
            .insert_catalog(
                current.clone(),
                FetchedCodeBuddyModelCatalog {
                    enabled_models: vec!["default-model".to_string()],
                    raw_configs: BTreeMap::new(),
                    capabilities: BTreeMap::new(),
                },
                1_000,
            )
            .await;
        assert!(cache.catalog(&current, 1_001).await.is_some());
        assert!(cache
            .catalog(&scope(CodeBuddySite::Cn, "copilot.tencent.com", 4), 1_001)
            .await
            .is_none());
        assert!(cache
            .catalog(&scope(CodeBuddySite::Intl, "www.codebuddy.ai", 5), 1_001,)
            .await
            .is_none());
        assert!(cache
            .catalog(&scope(CodeBuddySite::Intl, "www.workbuddy.ai", 4), 1_001,)
            .await
            .is_none());
        let stale = cache
            .stale_catalog(&current, 1_000 + CODEBUDDY_MODEL_CATALOG_TTL_MS + 1)
            .await
            .unwrap();
        assert!(stale.stale);
        assert!(cache
            .stale_catalog(&current, 1_000 + MAX_STALE_CATALOG_AGE_MS + 1)
            .await
            .is_none());
    }

    #[test]
    fn payload_forces_system_stream_usage_and_reasoning_history() {
        let payload = build_codebuddy_payload(
            &json!({
                "model":"auto",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","content":"answer","reasoning_content":"private"}
                ],
                "reasoning":{"effort":"high"},
                "stream":false
            }),
            "default-model",
            &text_capability(),
        )
        .unwrap();
        assert_eq!(payload["messages"][0]["role"], "system");
        assert_eq!(payload["messages"][2]["reasoning"], "private");
        assert!(payload["messages"][2].get("reasoning_content").is_none());
        assert_eq!(payload["reasoning_effort"], "high");
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["stream_options"]["include_usage"], true);
        assert_eq!(payload["model"], "default-model");
    }

    #[test]
    fn model_alias_is_site_bound_and_images_fail_closed() {
        assert_eq!(
            resolve_codebuddy_model_id(CodeBuddySite::Intl, "auto").unwrap(),
            "default-model"
        );
        assert_eq!(
            resolve_codebuddy_model_id(CodeBuddySite::Cn, "auto").unwrap(),
            "default"
        );
        assert!(build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA"}}]}]}),
            "default-model",
            &text_capability(),
        )
        .is_err());
    }

    #[test]
    fn payload_normalizes_named_tool_choice_and_validates_reasoning_effort() {
        let payload = build_codebuddy_payload(
            &json!({
                "messages":[{"role":"user","content":"run"}],
                "tools":[
                    {"type":"function","function":{"name":"read","parameters":{}}},
                    {"type":"function","function":{"name":"shell","parameters":{}}}
                ],
                "tool_choice":{"type":"function","function":{"name":"shell"}},
                "reasoning_effort":"low"
            }),
            "default-model",
            &text_capability(),
        )
        .unwrap();
        assert_eq!(payload["tool_choice"], "required");
        assert_eq!(payload["tools"].as_array().unwrap().len(), 1);
        assert_eq!(payload["tools"][0]["function"]["name"], "shell");
        assert_eq!(payload["reasoning_effort"], "low");
        assert!(payload.get("reasoning").is_none());

        let canonicalized = build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":"x"}],"reasoning_effort":"HIGH"}),
            "default-model",
            &text_capability(),
        )
        .unwrap();
        assert_eq!(canonicalized["reasoning_effort"], "high");

        let equivalent_dual_form = build_codebuddy_payload(
            &json!({
                "messages":[{"role":"user","content":"x"}],
                "reasoning_effort":"LOW",
                "reasoning":{"effort":"low"}
            }),
            "default-model",
            &text_capability(),
        )
        .unwrap();
        assert_eq!(equivalent_dual_form["reasoning_effort"], "low");

        let defaulted = build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":"x"}],"reasoning":{}}),
            "default-model",
            &text_capability(),
        )
        .unwrap();
        assert_eq!(defaulted["reasoning_effort"], "high");

        assert!(build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":"x"}],"reasoning_effort":"medium"}),
            "default-model",
            &text_capability(),
        )
        .is_err());
        assert!(build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":"x"}],"reasoning_effort":"low","reasoning":{"effort":"high"}}),
            "default-model",
            &text_capability(),
        )
        .unwrap_err()
        .contains("conflicting"));
        assert!(build_codebuddy_payload(
            &json!({"messages":[{"role":"user","content":"x"}],"reasoning_effort":3}),
            "default-model",
            &text_capability(),
        )
        .is_err());
        assert!(build_codebuddy_payload(
            &json!({
                "messages":[{"role":"user","content":"x"}],
                "tools":[{"type":"function","function":{"name":"known"}}],
                "tool_choice":{"type":"function","function":{"name":"unknown"}}
            }),
            "default-model",
            &text_capability(),
        )
        .unwrap_err()
        .contains("unknown function"));
        assert!(build_codebuddy_payload(
            &json!({
                "messages":[{"role":"user","content":"x"}],
                "tools":[{"type":"function","function":{"name":"unsafe name"}}],
                "tool_choice":{"type":"function","function":{"name":"unsafe name"}}
            }),
            "default-model",
            &text_capability(),
        )
        .unwrap_err()
        .contains("unsafe"));
    }
}
