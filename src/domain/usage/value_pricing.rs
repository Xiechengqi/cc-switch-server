//! Share API-price-equivalent quotas. Repository-owned v1 wire and arithmetic.
//! This module has no IO, wallet, market accounting, or external price lookup.
//! Router and Server implement the same specification and golden vectors.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet, VecDeque};

mod resources;
pub use resources::*;
mod resource_request;
pub use resource_request::*;

pub const TARIFF_POLICY: &str = "api-equivalent-v1";
pub const PICOS_PER_MICRO: i128 = 1_000_000;
pub const PICOS_PER_USD: i128 = 1_000_000_000_000;
pub const MAX_LIMIT_MICROS: i128 = 1_000_000_000_000_000;
pub const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_MODELS: usize = 10_000;

/// Safe diagnostic facts for a pre-send pricing refusal, never request content.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceRejection {
    pub reason_code: &'static str,
    pub model: String,
    pub tier: String,
    pub active_pricing_revision: Option<String>,
    pub pending_pricing_revision: Option<String>,
    pub effective_from_ms: Option<i64>,
}
/// Bounded identifiers only: never permit control characters in logs/errors.
pub(crate) fn diagnostic_identity(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect()
}
impl PriceRejection {
    pub fn message(&self) -> &'static str {
        match self.reason_code {
            "tariff_not_effective" => "Model price update is installed and awaiting its effective time; retry after activation.",
            "tier_unknown" => "The execution service tier is not supported for USD-equivalent pricing.",
            "tier_unpriced" => "The execution service tier has no verified price in the active tariff.",
            "model_unresolved" => "Resolve the execution model or configure an explicit pricing alias in Router settings before using USD-equivalent quotas.",
            "resource_unpriced" => "This operation has no resource price in the active tariff; configure its model and resource prices in Router settings.",
            _ => "The execution model has no verified price in the active tariff; configure its price in Router settings.",
        }
    }
}
impl std::fmt::Display for PriceRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for PriceRejection {}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueUsageEdit {
    pub action: ValueUsageEditAction,
    pub target_picos: Option<String>,
    pub expected_grant_revision: u64,
    pub window_id: String,
    pub expected_watermark: u64,
    pub operation_id: String,
    pub reason: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ValueUsageEditAction {
    Set,
    Clear,
}

/// Text metering follows the authoritative runtime binding, independently of
/// credentials or subscription type. The shared ledger accepts authoritative
/// usage and explicitly marked conservative estimates from every Driver.
/// Resource pricing is checked separately for each operation/payload.
/// Every registered transport declares its lifecycle owner. Adding a Driver
/// requires a CI-reviewed strategy; credentials and subscription type do not
/// determine USD eligibility.
pub fn value_metering_strategy(driver: &str) -> Option<&'static str> {
    match driver {
        "oauth.claude_messages"
        | "http.anthropic_messages"
        | "oauth.openai_codex"
        | "oauth.grok_responses"
        | "oauth.kimi_code"
        | "http.openai_chat"
        | "special.antigravity"
        | "special.agy"
        | "special.copilot"
        | "aws.bedrock_sigv4"
        | "http.bedrock_bearer"
        | "http.openai_responses"
        | "oauth.gemini_native"
        | "oauth.gemini_code_assist"
        | "http.gemini_native" => Some("http"),
        "special.qoder_cosy" => Some("qoder"),
        "special.codebuddy_oauth" | "special.workbuddy_oauth" => Some("buddy"),
        "special.trae_solo" | "special.trae_raw_chat" | "special.trae_raw_chat_intl" => {
            Some("trae")
        }
        "special.kiro" | "special.amazon_q" => Some("kiro"),
        "special.grok_web_session" | "special.perplexity_web_session" => Some("web_session"),
        "special.cursor" => Some("cursor"),
        "special.deepseek_account" => Some("deepseek"),
        "legacy.frozen" => Some("resolved_runtime"),
        _ => None,
    }
}

pub fn supports_value_text_runtime(profile: &str, driver: &str, _coding_plan: bool) -> bool {
    use crate::domain::providers::registry::{provider_registry, DriverBinding, OperationSupport};
    let registry = provider_registry();
    let Some(profile) = registry
        .profiles
        .iter()
        .find(|p| p.profile_id.as_str() == profile)
    else {
        return false;
    };
    // Frozen legacy plans select the actual Driver from provider settings.
    let binding_matches = profile.profile_id.as_str().ends_with(".legacy_compat")
        || match &profile.driver_binding {
            DriverBinding::Fixed { driver_id } => driver_id.as_str() == driver,
            DriverBinding::Custom { custom_policy_id } => registry
                .custom_policies
                .iter()
                .find(|policy| &policy.custom_policy_id == custom_policy_id)
                .is_some_and(|policy| {
                    policy
                        .allowed_driver_ids
                        .iter()
                        .any(|id| id.as_str() == driver)
                }),
        };
    binding_matches
        && value_metering_strategy(driver).is_some()
        && registry.drivers.iter().any(|d| {
            d.driver_id.as_str() == driver && d.operations.forward == OperationSupport::Supported
        })
}

/// Match the WorkBuddy resolver's identifier normalization, without pretending
/// its opaque routing aliases name a billable model. Other runtimes keep their
/// exact identifiers (including case and vendor prefixes).
pub fn value_execution_model_hint(profile: &str, model: &str) -> Option<String> {
    let model = if profile.ends_with(".workbuddy_oauth") {
        let model = model.trim();
        let model = model
            .strip_prefix("workbuddy/")
            .or_else(|| model.strip_prefix("codebuddy/"))
            .unwrap_or(model)
            .trim()
            .to_ascii_lowercase();
        if matches!(model.as_str(), "auto" | "default" | "default-model") {
            return None;
        }
        model
    } else {
        model.to_owned()
    };
    (!model.is_empty()
        && model.len() <= 250
        && model.trim() == model
        && !model.chars().any(char::is_control))
    .then_some(model)
}

/// Object keys that positively identify separately billed media, audio
/// output, cached server context, or a provider-executed (paid) tool. A key
/// whose value is `null`/`false` is a harmless replayed placeholder.
const DENIED_KEYS: &[&str] = &[
    "inlineData",
    "inline_data",
    "fileData",
    "file_data",
    "image_url",
    "input_image",
    "input_audio",
    "input_file",
    "audio",
    "video",
    "audioConfig",
    "audio_config",
    "speechConfig",
    "speech_config",
    "cachedContent",
    "cached_content",
    "groundingConfig",
    "grounding_config",
    "googleSearch",
    "google_search",
    "googleSearchRetrieval",
    "google_search_retrieval",
    "enterpriseWebSearch",
    "enterprise_web_search",
    "googleMaps",
    "google_maps",
    "retrieval",
    "fileSearch",
    "file_search",
    "codeExecution",
    "code_execution",
    "urlContext",
    "url_context",
    "computerUse",
    "computer_use",
    "web_search_options",
    "mcp_servers",
    "container",
];
/// Subtrees that carry caller-defined data (JSON schemas, tool arguments,
/// opaque signatures). Their `type` members describe schemas or user data,
/// never billable request content, so they are not interpreted.
const OPAQUE_KEYS: &[&str] = &[
    "parameters",
    "parametersJsonSchema",
    "parameters_json_schema",
    "input_schema",
    "inputSchema",
    "output_schema",
    "outputSchema",
    "schema",
    "json_schema",
    "responseSchema",
    "response_schema",
    "responseJsonSchema",
    "response_json_schema",
    "metadata",
    "arguments",
    "args",
    "encrypted_content",
    "signature",
    "thoughtSignature",
    "thought_signature",
];
/// Typed content/tool entries that are billed separately from text tokens:
/// media parts and provider-executed (server-side/built-in) tools, including
/// their history items (`web_search_call`, `code_interpreter_call`, ...).
fn denied_type(kind: &str) -> bool {
    matches!(
        kind,
        "image"
            | "image_url"
            | "input_image"
            | "output_image"
            | "image_file"
            | "input_audio"
            | "output_audio"
            | "audio"
            | "input_file"
            | "file"
            | "document"
            | "video"
            | "input_video"
    ) || [
        "web_search",
        "web_fetch",
        "file_search",
        "code_interpreter",
        "code_execution",
        "computer",
        "image_generation",
        "mcp",
        "bash_",
        "text_editor_",
        "google_search",
        "url_context",
        "server_tool_use",
        "container_upload",
    ]
    .iter()
    .any(|prefix| kind.starts_with(prefix))
}
fn metered_text_value(value: &serde_json::Value, parent_key: Option<&str>) -> bool {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let kind = map.get("type").and_then(Value::as_str);
            if kind.is_some_and(denied_type) {
                return false;
            }
            for key in ["responseModalities", "response_modalities", "modalities"] {
                if map.get(key).and_then(Value::as_array).is_some_and(|modes| {
                    modes.iter().any(|mode| {
                        !mode
                            .as_str()
                            .is_some_and(|mode| mode.eq_ignore_ascii_case("text"))
                    })
                }) {
                    return false;
                }
            }
            map.iter().all(|(key, child)| {
                let key = key.as_str();
                if DENIED_KEYS.contains(&key) {
                    return matches!(child, Value::Null | Value::Bool(false));
                }
                if OPAQUE_KEYS.contains(&key)
                    || (key == "input" && matches!(kind, Some("tool_use" | "custom_tool_call")))
                    || (key == "response"
                        && matches!(parent_key, Some("functionResponse" | "function_response")))
                {
                    return true;
                }
                metered_text_value(child, Some(key))
            })
        }
        Value::Array(items) => items
            .iter()
            .all(|item| metered_text_value(item, parent_key)),
        _ => true,
    }
}
/// Protocol-aware text classifier for Messages, Chat Completions, Responses
/// and Gemini bodies. Everything is text token usage except positively
/// identified media/audio/file content and provider-executed paid tools.
/// Client tools (function/custom), thinking/reasoning blocks, cache control,
/// tool_choice and structured-output formats are ordinary text.
pub fn is_metered_text_value(body: &serde_json::Value) -> bool {
    body.is_object() && metered_text_value(body, None)
}
pub fn is_metered_text_payload(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(bytes)
        .is_ok_and(|body| is_metered_text_value(&body))
}

pub const DEFAULT_OUTPUT_CEILING: u64 = 32_000;
pub const MAX_OUTPUT_CEILING: u64 = 128_000;
/// Bounded per-request reservation inputs. Input is a byte heuristic
/// (`ceil(bytes / 3)` fresh-input tokens); output is the request's declared
/// maximum, clamped to `[1, 128000]`, or 32000 when the client sent none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RiskEstimate {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
impl RiskEstimate {
    pub fn from_request(body_len: usize, body: Option<&serde_json::Value>) -> Self {
        use serde_json::Value;
        let requested = body.and_then(|body| {
            ["max_tokens", "max_completion_tokens", "max_output_tokens"]
                .iter()
                .find_map(|key| body.get(*key).and_then(Value::as_u64))
                .or_else(|| {
                    body.pointer("/generationConfig/maxOutputTokens")
                        .or_else(|| body.pointer("/generation_config/max_output_tokens"))
                        .and_then(Value::as_u64)
                })
        });
        Self {
            input_tokens: u64::try_from(body_len).unwrap_or(u64::MAX).div_ceil(3),
            output_tokens: requested
                .map(|n| n.clamp(1, MAX_OUTPUT_CEILING))
                .unwrap_or(DEFAULT_OUTPUT_CEILING),
        }
    }
    pub fn tokens(&self) -> ValueTokens {
        ValueTokens {
            fresh_input: self.input_tokens,
            output: self.output_tokens,
            ..Default::default()
        }
    }
}
/// Everything the forwarder needs from a final upstream body, parsed once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRequestFacts {
    pub metered_text: bool,
    pub estimate: RiskEstimate,
    /// Raw `service_tier` exactly as sent; never canonicalized in place.
    pub service_tier: Option<String>,
}
pub fn value_request_facts(bytes: &[u8]) -> ValueRequestFacts {
    let body = serde_json::from_slice::<serde_json::Value>(bytes).ok();
    ValueRequestFacts {
        metered_text: body.as_ref().is_some_and(is_metered_text_value),
        estimate: RiskEstimate::from_request(bytes.len(), body.as_ref()),
        service_tier: body
            .as_ref()
            .and_then(|body| body.get("service_tier"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
    }
}
/// The one canonical tier mapping shared by admission and settlement.
/// OpenAI `auto`/absent/`default` are served at the standard tier unless the
/// provider reports otherwise; unknown tiers (e.g. `scale`) are unpriced.
pub fn canonical_service_tier(raw: Option<&str>) -> Option<&'static str> {
    let Some(raw) = raw.map(str::trim) else {
        return Some("standard");
    };
    if raw.is_empty()
        || ["auto", "default", "standard"]
            .iter()
            .any(|tier| raw.eq_ignore_ascii_case(tier))
    {
        Some("standard")
    } else if raw.eq_ignore_ascii_case("priority") {
        Some("priority")
    } else if raw.eq_ignore_ascii_case("flex") {
        Some("flex")
    } else {
        None
    }
}

/// Typed USD-equivalent admission rejections. Mappers downcast this instead
/// of matching message text; `Display` keeps the stable `VALUE_QUOTA_*` code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueQuotaRejectionKind {
    /// Settled usage reached the limit (403, not retryable).
    Exhausted,
    /// Concurrency cap or in-flight reservations; retry shortly (429).
    Busy,
    /// Private Share authorization was revoked (403).
    AuthorizationRevoked,
    /// SQLite busy/locked; transient (503, retryable).
    StorageBusy,
    /// The scope/window needs an owner reconcile (`set`) (503).
    Blocked,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValueQuotaRejection {
    pub kind: ValueQuotaRejectionKind,
    pub resets_at_ms: Option<i64>,
}
impl ValueQuotaRejection {
    pub fn new(kind: ValueQuotaRejectionKind) -> Self {
        Self {
            kind,
            resets_at_ms: None,
        }
    }
    pub fn code(&self) -> &'static str {
        match self.kind {
            ValueQuotaRejectionKind::Exhausted => "VALUE_QUOTA_EXHAUSTED",
            ValueQuotaRejectionKind::Busy => "VALUE_QUOTA_BUSY",
            ValueQuotaRejectionKind::AuthorizationRevoked => "VALUE_QUOTA_AUTHORIZATION_REVOKED",
            ValueQuotaRejectionKind::StorageBusy => "VALUE_QUOTA_STORAGE_BUSY",
            ValueQuotaRejectionKind::Blocked => "VALUE_QUOTA_BLOCKED_NEEDS_RECONCILE",
        }
    }
}
impl std::fmt::Display for ValueQuotaRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for ValueQuotaRejection {}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueMeteringBinding {
    pub quota_policy_epoch: String,
    pub value_metered_from_ms: i64,
    pub effective_from_ms: i64,
    pub pricing_revision: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueQuotaStatus {
    pub email: String,
    pub policy_epoch: String,
    pub window_id: String,
    pub window_starts_at_ms: i64,
    pub resets_at_ms: Option<i64>,
    pub metered_from_ms: i64,
    pub active_pricing_revision: String,
    pub usage_pricing_revisions_digest: String,
    pub usage_watermark: u64,
    pub limit_usd_micros: Option<String>,
    pub used_picos: String,
    pub observed_picos: String,
    pub manual_offset_picos: String,
    pub pending_risk_picos: String,
    pub metering_status: String,
    pub synced_at_ms: i64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PricingManifest {
    pub installation_id: String,
    pub revision: String,
    pub effective_from_ms: i64,
    pub snapshot: PricingSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    InvalidDecimal,
    OutOfRange,
    InvalidPolicy,
    InvalidSnapshot,
    PriceUnavailable,
    UsageUnavailable,
    Overflow,
}
impl std::fmt::Display for ValueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidDecimal => "invalid canonical unsigned decimal",
            Self::OutOfRange => "value exceeds the supported range",
            Self::InvalidPolicy => "invalid USD-equivalent quota policy",
            Self::InvalidSnapshot => "invalid pricing snapshot",
            Self::PriceUnavailable => "price_unavailable",
            Self::UsageUnavailable => "usage_unavailable",
            Self::Overflow => "quota arithmetic overflow",
        })
    }
}
impl std::error::Error for ValueError {}

pub fn unsigned_decimal(raw: &str) -> Result<i128, ValueError> {
    if raw.is_empty()
        || raw.len() > 39
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ValueError::InvalidDecimal);
    }
    raw.parse().map_err(|_| ValueError::OutOfRange)
}

/// USD user input to exact micro-USD. The wire itself uses integer strings.
pub fn usd_to_micros(raw: &str) -> Result<String, ValueError> {
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if whole.is_empty()
        || (raw.contains('.') && fraction.is_empty())
        || fraction.len() > 6
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ValueError::InvalidDecimal);
    }
    let whole: i128 = whole.parse().map_err(|_| ValueError::OutOfRange)?;
    let fraction: i128 = format!("{fraction:0<6}")
        .parse()
        .map_err(|_| ValueError::InvalidDecimal)?;
    let micros = whole
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_add(fraction))
        .ok_or(ValueError::Overflow)?;
    if micros == 0 || micros > MAX_LIMIT_MICROS {
        return Err(ValueError::OutOfRange);
    }
    Ok(micros.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueQuota {
    pub limit_usd_micros: String,
    pub tariff_policy_id: String,
}
impl ValueQuota {
    pub fn limit_picos(&self) -> Result<i128, ValueError> {
        let micros = unsigned_decimal(&self.limit_usd_micros)?;
        if !matches!(
            self.tariff_policy_id.as_str(),
            TARIFF_POLICY | RESOURCE_TARIFF_POLICY
        ) || micros == 0
            || micros > MAX_LIMIT_MICROS
        {
            return Err(ValueError::InvalidPolicy);
        }
        micros
            .checked_mul(PICOS_PER_MICRO)
            .ok_or(ValueError::Overflow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextInputBasis {
    FreshInput,
    RawInput,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheWritePolicy {
    Unavailable,
    FiveMinutesOnly,
    OneHourOnly,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRates {
    pub input: Option<String>,
    pub output: Option<String>,
    pub cache_read: Option<String>,
    pub cache_write5m: Option<String>,
    pub cache_write1h: Option<String>,
    pub cache_write_policy: CacheWritePolicy,
}
impl ExecutionRates {
    fn rate(raw: &Option<String>) -> Result<Option<i128>, ValueError> {
        raw.as_deref()
            .map(|raw| {
                let value = unsigned_decimal(raw)?;
                if value > i128::from(i64::MAX) {
                    return Err(ValueError::OutOfRange);
                }
                Ok(value)
            })
            .transpose()
    }
    pub fn validate(&self) -> Result<(), ValueError> {
        for rate in [
            &self.input,
            &self.output,
            &self.cache_read,
            &self.cache_write5m,
            &self.cache_write1h,
        ] {
            Self::rate(rate)?;
        }
        if self.input.is_none() || self.output.is_none() {
            return Err(ValueError::PriceUnavailable);
        }
        match self.cache_write_policy {
            CacheWritePolicy::Unavailable => {
                if self.cache_write5m.is_some() || self.cache_write1h.is_some() {
                    return Err(ValueError::InvalidSnapshot);
                }
            }
            CacheWritePolicy::FiveMinutesOnly
                if self.cache_write5m.is_none() || self.cache_write1h.is_some() =>
            {
                return Err(ValueError::InvalidSnapshot)
            }
            CacheWritePolicy::OneHourOnly
                if self.cache_write1h.is_none() || self.cache_write5m.is_some() =>
            {
                return Err(ValueError::InvalidSnapshot)
            }
            CacheWritePolicy::Both
                if self.cache_write5m.is_none() || self.cache_write1h.is_none() =>
            {
                return Err(ValueError::InvalidSnapshot)
            }
            _ => {}
        }
        Ok(())
    }
    pub fn price(&self, tokens: &ValueTokens) -> Result<PricedValue, ValueError> {
        self.validate()?;
        let mut total = 0i128;
        let mut add = |count: u64, rate: Option<i128>| -> Result<(), ValueError> {
            if count == 0 {
                return Ok(());
            }
            let value = i128::from(count)
                .checked_mul(rate.ok_or(ValueError::PriceUnavailable)?)
                .ok_or(ValueError::Overflow)?;
            total = total.checked_add(value).ok_or(ValueError::Overflow)?;
            Ok(())
        };
        add(tokens.fresh_input, Self::rate(&self.input)?)?;
        add(tokens.output, Self::rate(&self.output)?)?;
        add(tokens.cache_read, Self::rate(&self.cache_read)?)?;
        add(tokens.cache_write5m, Self::rate(&self.cache_write5m)?)?;
        add(tokens.cache_write1h, Self::rate(&self.cache_write1h)?)?;
        let unknown_rate = match self.cache_write_policy {
            CacheWritePolicy::Unavailable => None,
            CacheWritePolicy::FiveMinutesOnly => Self::rate(&self.cache_write5m)?,
            CacheWritePolicy::OneHourOnly => Self::rate(&self.cache_write1h)?,
            CacheWritePolicy::Both => Some(
                Self::rate(&self.cache_write5m)?
                    .ok_or(ValueError::PriceUnavailable)?
                    .max(Self::rate(&self.cache_write1h)?.ok_or(ValueError::PriceUnavailable)?),
            ),
        };
        add(tokens.cache_write_unknown, unknown_rate)?;
        Ok(PricedValue {
            usage_picos: total,
            conservative_cache_ttl: tokens.cache_write_unknown > 0
                && self.cache_write_policy == CacheWritePolicy::Both,
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ValueTokens {
    pub fresh_input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write5m: u64,
    pub cache_write1h: u64,
    pub cache_write_unknown: u64,
}
impl ValueTokens {
    pub fn raw_input(&self) -> Result<u64, ValueError> {
        [
            self.cache_read,
            self.cache_write5m,
            self.cache_write1h,
            self.cache_write_unknown,
        ]
        .into_iter()
        .try_fold(self.fresh_input, |sum, n| {
            sum.checked_add(n).ok_or(ValueError::Overflow)
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PricedValue {
    pub usage_picos: i128,
    pub conservative_cache_ttl: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionModelPrice {
    pub context_input_basis: ContextInputBasis,
    pub long_context_threshold: Option<u64>,
    pub long_context_inclusive: bool,
    #[serde(deserialize_with = "unique_map")]
    pub rates: BTreeMap<String, ExecutionRates>,
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "unique_map"
    )]
    pub resources: BTreeMap<String, String>,
}
impl ExecutionModelPrice {
    pub fn validate(&self) -> Result<(), ValueError> {
        validate_resource_rates(&self.resources)?;
        if self.long_context_threshold == Some(0)
            || (self.rates.is_empty() && self.resources.is_empty())
            || self.rates.len() > 6
        {
            return Err(ValueError::InvalidSnapshot);
        }
        for (key, rates) in &self.rates {
            if !matches!(
                key.as_str(),
                "standard/base"
                    | "standard/long"
                    | "priority/base"
                    | "priority/long"
                    | "flex/base"
                    | "flex/long"
            ) {
                return Err(ValueError::InvalidSnapshot);
            }
            if key.ends_with("/long") && self.long_context_threshold.is_none() {
                return Err(ValueError::InvalidSnapshot);
            }
            rates.validate()?;
        }
        for tier in ["standard", "priority", "flex"] {
            if self
                .rates
                .keys()
                .any(|key| key.starts_with(&format!("{tier}/")))
                && !self.supports_tier(tier)
            {
                return Err(ValueError::InvalidSnapshot);
            }
        }
        Ok(())
    }
    pub fn price(
        &self,
        effective_tier: &str,
        tokens: &ValueTokens,
    ) -> Result<PricedValue, ValueError> {
        if !matches!(effective_tier, "standard" | "priority" | "flex") {
            return Err(ValueError::PriceUnavailable);
        }
        let context_input = match self.context_input_basis {
            ContextInputBasis::FreshInput => tokens.fresh_input,
            ContextInputBasis::RawInput => tokens.raw_input()?,
        };
        let long = self.long_context_threshold.is_some_and(|n| {
            if self.long_context_inclusive {
                context_input >= n
            } else {
                context_input > n
            }
        });
        let key = format!("{effective_tier}/{}", if long { "long" } else { "base" });
        self.rates
            .get(&key)
            .ok_or(ValueError::PriceUnavailable)?
            .price(tokens)
    }
    pub fn supports_tier(&self, tier: &str) -> bool {
        self.rates.contains_key(&format!("{tier}/base"))
            && (self.long_context_threshold.is_none()
                || self.rates.contains_key(&format!("{tier}/long")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PricingSnapshot {
    pub schema_version: u16,
    pub tariff_policy_id: String,
    pub currency: String,
    #[serde(deserialize_with = "unique_map")]
    pub models: BTreeMap<String, ExecutionModelPrice>,
    #[serde(deserialize_with = "unique_map")]
    pub exact_aliases: BTreeMap<String, String>,
}
fn valid_model_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 250 && key.trim() == key && !key.chars().any(char::is_control)
}
impl PricingSnapshot {
    pub fn validate(&self) -> Result<(), ValueError> {
        if !matches!(
            (self.schema_version, self.tariff_policy_id.as_str()),
            (1, TARIFF_POLICY) | (2, RESOURCE_TARIFF_POLICY)
        ) || (self.schema_version == 1
            && self
                .models
                .values()
                .any(|model| !model.resources.is_empty()))
            || self.currency != "USD"
            || self.models.is_empty()
            || self.models.len() > MAX_MODELS
            || self.exact_aliases.len() > MAX_MODELS * 4
        {
            return Err(ValueError::InvalidSnapshot);
        }
        for (key, price) in &self.models {
            if !valid_model_key(key) {
                return Err(ValueError::InvalidSnapshot);
            }
            price.validate()?;
        }
        for (alias, key) in &self.exact_aliases {
            if !valid_model_key(alias)
                || !self.models.contains_key(key)
                || self.models.contains_key(alias)
            {
                return Err(ValueError::InvalidSnapshot);
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| ValueError::InvalidSnapshot)?
            .len()
            > MAX_SNAPSHOT_BYTES
        {
            return Err(ValueError::InvalidSnapshot);
        }
        Ok(())
    }
    pub fn revision(&self) -> Result<String, ValueError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| ValueError::InvalidSnapshot)?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
    /// Resolves the wire model exactly as the Router price display does: the
    /// name as sent, then with provider qualifiers and runtime suffixes peeled
    /// (`anthropic.`, `publishers/…/models/`, `-thinking`, `[1m]`, `-v1:0`,
    /// date stamps), each candidate matched as an exact alias or model key.
    /// Shared vectors: `tests/fixtures/share-value-model-identity-v1.json`.
    pub fn model(&self, effective_model: &str) -> Result<&ExecutionModelPrice, ValueError> {
        std::iter::once(effective_model.to_string())
            .chain(identity_candidates(effective_model))
            .find_map(|candidate| {
                let key = self
                    .exact_aliases
                    .get(&candidate)
                    .map(String::as_str)
                    .unwrap_or(&candidate);
                self.models.get(key)
            })
            .ok_or(ValueError::PriceUnavailable)
    }
}

// Model identity, ported verbatim from the Router's `model_price_catalog.rs`
// so a USD quota charges the model the price display shows.

/// Exact-match candidates derived from a logged model name.
///
/// Order is most-specific first: the original text, then successively
/// unwrapped identities. Every candidate is a whole-string identity, never a
/// family prefix, so `claude-sonnet-4-5-thinking` can land on
/// `claude-sonnet-4-5` while `gpt-5-mini` cannot land on `gpt-5`.
fn identity_candidates(model_text: &str) -> Vec<String> {
    let mut ordered = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(model_text.trim().to_ascii_lowercase());
    while let Some(current) = queue.pop_front() {
        if current.is_empty() || !seen.insert(current.clone()) {
            continue;
        }
        ordered.push(current.clone());
        for next in unwrap_identity(&current) {
            if !next.is_empty() && !seen.contains(&next) {
                queue.push_back(next);
            }
        }
    }
    ordered
}

fn unwrap_identity(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    push_unique(&mut out, strip_path_leaf(value));
    push_unique(&mut out, strip_provider_dot_prefix(value));
    push_unique(&mut out, strip_runtime_suffix(value));
    push_unique(&mut out, strip_date_stamp(value));
    push_unique(&mut out, claude_dot_version_to_hyphen(value));
    out
}

fn push_unique(out: &mut Vec<String>, next: Option<String>) {
    if let Some(next) = next {
        if !out.iter().any(|existing| existing == &next) {
            out.push(next);
        }
    }
}

/// `publishers/anthropic/models/claude-sonnet-4-5` → `claude-sonnet-4-5`.
/// Also covers `moonshotai/kimi-k2.5` and OpenRouter-style `anthropic/...`.
fn strip_path_leaf(value: &str) -> Option<String> {
    let leaf = value.rsplit('/').next()?.trim();
    if leaf.is_empty() || leaf == value {
        return None;
    }
    Some(leaf.to_string())
}

/// `anthropic.claude-sonnet-4-5-20250514-v1:0` → `claude-sonnet-4-5-20250514-v1:0`
/// and `global.anthropic.claude-opus-4-8` → `claude-opus-4-8`.
///
/// Only a known vendor token before the first `claude-` / `gpt-` / `o[0-9]`
/// stem is peeled, so `gpt-5.5` is left alone.
fn strip_provider_dot_prefix(value: &str) -> Option<String> {
    const STEMS: &[&str] = &[
        "claude-",
        "gpt-",
        "gemini-",
        "kimi-",
        "glm-",
        "deepseek-",
        "codex-",
    ];
    let mut stem_at = None;
    for stem in STEMS {
        if let Some(at) = value.find(stem) {
            if at > 0 && value.as_bytes()[at - 1] == b'.' {
                stem_at = Some(stem_at.map_or(at, |current: usize| current.min(at)));
            }
        }
    }
    // OpenAI o-series keys (`o1`, `o3`, `o4-mini`) sit after a vendor
    // qualifier too, but the stem is a single letter+digit so it must be
    // bounded or `global.` would match the `o1` inside `anthropic`.
    for (i, _) in value.match_indices('o') {
        if i == 0 || value.as_bytes()[i - 1] != b'.' {
            continue;
        }
        let after = &value[i + 1..];
        if after.starts_with(|ch: char| ch.is_ascii_digit()) {
            stem_at = Some(stem_at.map_or(i, |current| current.min(i)));
        }
    }
    let stem_at = stem_at?;
    let prefix = &value[..stem_at - 1];
    if prefix.is_empty()
        || !prefix
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    {
        return None;
    }
    Some(value[stem_at..].to_string())
}

/// Runtime / vendor packaging that does not change the priced identity:
/// `-thinking`, `-latest`, `[1m]`, `-1m`, Bedrock `-v1:0` / `:0`.
/// Date stamps stay — they are themselves catalog keys.
fn strip_runtime_suffix(value: &str) -> Option<String> {
    let mut current = value.to_string();
    let mut changed = false;
    loop {
        let mut stripped = false;
        for suffix in ["-thinking", "-latest"] {
            if let Some(next) = current.strip_suffix(suffix) {
                if next.is_empty() {
                    break;
                }
                current = next.to_string();
                stripped = true;
                changed = true;
            }
        }
        for suffix in ["[1m]", "-1m"] {
            if let Some(next) = current.strip_suffix(suffix) {
                if next.is_empty() {
                    break;
                }
                current = next.to_string();
                stripped = true;
                changed = true;
            }
        }
        if let Some(next) = strip_bedrock_revision(&current) {
            current = next;
            stripped = true;
            changed = true;
        }
        if !stripped {
            break;
        }
    }
    changed.then_some(current)
}

/// `claude-sonnet-4-5-20250929-v1:0` → `claude-sonnet-4-5-20250929`.
/// The dated-v1:0 form is itself a catalog key, so the original is tried
/// first; this only fires when that exact key is absent.
fn strip_bedrock_revision(value: &str) -> Option<String> {
    if let Some(next) = value.strip_suffix(":0") {
        if let Some(base) = next.rsplit_once("-v") {
            if !base.0.is_empty() && base.1.bytes().all(|byte| byte.is_ascii_digit()) {
                return Some(base.0.to_string());
            }
        }
        if !next.is_empty() {
            return Some(next.to_string());
        }
    }
    None
}

/// `claude-sonnet-4-5-20250514` → `claude-sonnet-4-5`.
///
/// Date stamps that are themselves catalog keys are tried first (BFS
/// most-specific-first). This only fires when that dated key is absent, so
/// `claude-sonnet-4-5-20250929` still prefers its own catalog row.
fn strip_date_stamp(value: &str) -> Option<String> {
    let (base, suffix) = value.rsplit_once('-')?;
    if suffix.len() == 8 && suffix.bytes().all(|byte| byte.is_ascii_digit()) && !base.is_empty() {
        return Some(base.to_string());
    }
    None
}

/// Kiro writes `claude-sonnet-4.8`; the catalog key is `claude-sonnet-4-8`.
fn claude_dot_version_to_hyphen(value: &str) -> Option<String> {
    let rest = value.strip_prefix("claude-")?;
    let (family, version) = rest.split_once('-')?;
    if !matches!(family, "sonnet" | "opus" | "haiku") {
        return None;
    }
    if !version.contains('.')
        || !version
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    Some(format!("claude-{family}-{}", version.replace('.', "-")))
}

// Serde's default map reader silently accepts repeated model/rate keys.
// Executable snapshots reject them before hashing or persisting anything.
fn unique_map<'de, D, V>(deserializer: D) -> Result<BTreeMap<String, V>, D::Error>
where
    D: serde::Deserializer<'de>,
    V: Deserialize<'de>,
{
    struct Unique<V>(std::marker::PhantomData<V>);
    impl<'de, V: Deserialize<'de>> serde::de::Visitor<'de> for Unique<V> {
        type Value = BTreeMap<String, V>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a map with unique keys")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut values = BTreeMap::new();
            while let Some((key, value)) = map.next_entry::<String, V>()? {
                if values.insert(key, value).is_some() {
                    return Err(serde::de::Error::custom("duplicate pricing key"));
                }
            }
            Ok(values)
        }
    }
    deserializer.deserialize_map(Unique(std::marker::PhantomData))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_identity_matches_shared_vectors() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/share-value-model-identity-v1.json"
        ))
        .unwrap();
        let rates = ExecutionRates {
            input: Some("1".into()),
            output: Some("1".into()),
            cache_read: None,
            cache_write5m: None,
            cache_write1h: None,
            cache_write_policy: CacheWritePolicy::Unavailable,
        };
        let model = ExecutionModelPrice {
            resources: Default::default(),
            context_input_basis: ContextInputBasis::RawInput,
            long_context_threshold: None,
            long_context_inclusive: false,
            rates: [("standard/base".to_string(), rates)].into_iter().collect(),
        };
        let snapshot = PricingSnapshot {
            schema_version: 1,
            tariff_policy_id: TARIFF_POLICY.into(),
            currency: "USD".into(),
            models: fixture["models"]
                .as_array()
                .unwrap()
                .iter()
                .map(|key| (key.as_str().unwrap().to_string(), model.clone()))
                .collect(),
            exact_aliases: serde_json::from_value(fixture["exactAliases"].clone()).unwrap(),
        };
        for case in fixture["cases"].as_array().unwrap() {
            let observed = case["model"].as_str().unwrap();
            let resolved = snapshot.model(observed).ok().map(|price| {
                snapshot
                    .models
                    .iter()
                    .find(|(_, candidate)| std::ptr::eq(*candidate, price))
                    .map(|(key, _)| key.as_str())
                    .unwrap()
            });
            assert_eq!(resolved, case["expected"].as_str(), "{observed:?}");
        }
    }

    #[test]
    fn every_registered_runtime_supports_usd_independently_of_credential_type() {
        use crate::domain::providers::registry::{provider_registry, DriverBinding};
        let registry = provider_registry();
        for profile in &registry.profiles {
            let drivers = match &profile.driver_binding {
                DriverBinding::Fixed { driver_id } => vec![driver_id],
                DriverBinding::Custom { custom_policy_id } => registry
                    .custom_policies
                    .iter()
                    .find(|p| &p.custom_policy_id == custom_policy_id)
                    .unwrap()
                    .allowed_driver_ids
                    .iter()
                    .collect(),
            };
            for driver in drivers {
                for coding_plan in [false, true] {
                    assert!(
                        supports_value_text_runtime(
                            profile.profile_id.as_str(),
                            driver.as_str(),
                            coding_plan
                        ),
                        "{} / {}",
                        profile.profile_id,
                        driver
                    );
                }
            }
            assert!(!supports_value_text_runtime(
                profile.profile_id.as_str(),
                "unknown.driver",
                false
            ));
        }
        assert!(!supports_value_text_runtime(
            "unknown.profile",
            "http.openai_chat",
            false
        ));
        assert!(!supports_value_text_runtime(
            "codex.custom_http",
            "http.gemini_native",
            false
        ));
        assert!(!supports_value_text_runtime(
            "codex.workbuddy_oauth",
            "special.codebuddy_oauth",
            false
        ));
    }
    #[test]
    fn workbuddy_value_models_require_concrete_identity_without_changing_other_runtimes() {
        for opaque in [
            "auto",
            "default",
            "default-model",
            "workbuddy/auto",
            "codebuddy/default",
        ] {
            assert_eq!(
                value_execution_model_hint("claude.workbuddy_oauth", opaque),
                None
            );
        }
        assert_eq!(
            value_execution_model_hint("claude.workbuddy_oauth", "workbuddy/GLM-5.2"),
            Some("glm-5.2".into())
        );
        assert_eq!(
            value_execution_model_hint("claude.custom_http", "Vendor/My-Model"),
            Some("Vendor/My-Model".into())
        );
    }
    #[test]
    fn protocol_golden_vectors_freeze_snapshot_hash_aliases_and_exact_amounts() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/share-value-pricing-v1.json"
        ))
        .unwrap();
        let snapshot: PricingSnapshot =
            serde_json::from_value(fixture["snapshot"].clone()).unwrap();
        assert_eq!(
            snapshot.revision().unwrap(),
            fixture["revision"].as_str().unwrap()
        );
        for vector in fixture["vectors"].as_array().unwrap() {
            let tokens: ValueTokens = serde_json::from_value(vector["tokens"].clone()).unwrap();
            let price = snapshot
                .model(vector["model"].as_str().unwrap())
                .unwrap()
                .price(vector["tier"].as_str().unwrap(), &tokens)
                .unwrap();
            assert_eq!(
                price.usage_picos.to_string(),
                vector["picos"].as_str().unwrap(),
                "{}",
                vector["name"]
            );
            assert_eq!(
                price.conservative_cache_ttl,
                vector["conservative"].as_bool().unwrap()
            );
        }
        assert!(snapshot.model("guessed-model-family").is_err());
        assert!(snapshot
            .model("text-model")
            .unwrap()
            .price("priority", &ValueTokens::default())
            .is_err());
    }
    #[test]
    fn only_text_and_client_function_tools_are_metered_not_media_or_paid_builtin_tools() {
        assert!(is_metered_text_payload(br#"{"messages":[{"role":"user","content":"hello"}],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"image":{"type":"string"}}}}}]}"#));
        for request in [
            br#"{"tools":[{"googleSearch":{}}]}"#.as_slice(),
            br#"{"tools":[{"type":"web_search"}]}"#,
            br#"{"messages":[{"content":[{"type":"image_url","image_url":"url"}]}]}"#,
            br#"{"generationConfig":{"responseModalities":["IMAGE"]}}"#,
            br#"{"cachedContent":"unverified-context"}"#,
        ] {
            assert!(!is_metered_text_payload(request));
        }
    }
    #[test]
    fn realistic_claude_code_codex_and_chat_bodies_are_metered_text() {
        let claude_code = serde_json::json!({
            "model": "claude-sonnet-4-5",
            "max_tokens": 32000,
            "stream": true,
            "system": [
                {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}},
                {"type": "text", "text": "Env info", "cache_control": {"type": "ephemeral", "ttl": "1h"}}
            ],
            "thinking": {"type": "enabled", "budget_tokens": 10000},
            "tool_choice": {"type": "auto"},
            "tools": [
                {"name": "Bash", "description": "Run", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}, "image": {"type": "image"}}}},
                {"type": "custom", "name": "Edit", "input_schema": {"type": "object"}, "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "fix it"}]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "plan", "signature": "sig"},
                    {"type": "redacted_thinking", "data": "opaque"},
                    {"type": "text", "text": "Running"},
                    {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls", "image": {"type": "image"}}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": "ok"}], "is_error": false},
                    {"type": "text", "text": "continue", "cache_control": {"type": "ephemeral"}}
                ]}
            ],
            "metadata": {"user_id": "u"}
        });
        let disabled_thinking = serde_json::json!({"thinking": {"type": "disabled"},
            "tool_choice": {"type": "tool", "name": "Bash"}, "messages": []});
        let codex = serde_json::json!({
            "model": "gpt-5-codex",
            "instructions": "codex",
            "stream": true,
            "store": false,
            "include": ["reasoning.encrypted_content"],
            "reasoning": {"effort": "high", "summary": "auto"},
            "text": {"verbosity": "low", "format": {"type": "json_schema", "name": "out", "schema": {"type": "object"}}},
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "tools": [
                {"type": "function", "name": "shell", "parameters": {"type": "object", "properties": {"command": {"type": "array"}}}},
                {"type": "custom", "name": "apply_patch", "format": {"type": "grammar", "syntax": "lark", "definition": "start: x"}},
                {"type": "local_shell"}
            ],
            "input": [
                {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "rules"}]},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "go"}]},
                {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "s"}], "encrypted_content": "gAAA"},
                {"type": "function_call", "call_id": "c1", "name": "shell", "arguments": "{\"image_url\":\"x\"}"},
                {"type": "function_call_output", "call_id": "c1", "output": "done"},
                {"type": "custom_tool_call", "call_id": "c2", "name": "apply_patch", "input": "*** Begin Patch"},
                {"type": "custom_tool_call_output", "call_id": "c2", "output": "ok"},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "hi"}, {"type": "refusal", "refusal": "no"}]}
            ]
        });
        let chat = serde_json::json!({
            "model": "gpt-4.1",
            "max_completion_tokens": 512,
            "response_format": {"type": "json_schema", "json_schema": {"name": "x", "schema": {"type": "object", "properties": {"audio": {"type": "string"}}}}},
            "tools": [{"type": "function", "function": {"name": "f", "parameters": {"type": "object"}}}],
            "tool_choice": {"type": "function", "function": {"name": "f"}},
            "messages": [
                {"role": "system", "content": "s"},
                {"role": "assistant", "content": null, "audio": null, "refusal": null,
                 "tool_calls": [{"id": "1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "tool", "tool_call_id": "1", "content": "r"},
                {"role": "user", "content": [{"type": "text", "text": "hello"}]}
            ]
        });
        let gemini = serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": "hi"}]},
                {"role": "model", "parts": [{"functionCall": {"name": "f", "args": {"image": {"type": "image"}}}, "thoughtSignature": "x"}]},
                {"role": "user", "parts": [{"functionResponse": {"name": "f", "response": {"type": "file"}}}]}],
            "tools": [{"functionDeclarations": [{"name": "f", "parameters": {"type": "object"}}]}],
            "generationConfig": {"maxOutputTokens": 1024, "responseModalities": ["TEXT"], "thinkingConfig": {"thinkingBudget": 1024}}
        });
        for body in [&claude_code, &disabled_thinking, &codex, &chat, &gemini] {
            assert!(is_metered_text_value(body), "{body}");
            assert!(is_metered_text_payload(&serde_json::to_vec(body).unwrap()));
        }
    }
    #[test]
    fn media_documents_and_server_tools_are_not_metered_text() {
        for body in [
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "image", "source": {"type": "base64", "data": "x"}}]}]}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "document", "source": {"type": "url", "url": "x"}}]}]}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "tool_result", "tool_use_id": "1", "content": [{"type": "image", "source": {}}]}]}]}),
            serde_json::json!({"tools": [{"type": "web_search_20250305", "name": "web_search"}], "messages": []}),
            serde_json::json!({"tools": [{"type": "code_execution_20250522", "name": "code_execution"}]}),
            serde_json::json!({"tools": [{"type": "computer_20250124", "name": "computer"}]}),
            serde_json::json!({"tools": [{"type": "bash_20250124", "name": "bash"}]}),
            serde_json::json!({"tools": [{"type": "text_editor_20250429", "name": "str_replace_based_edit_tool"}]}),
            serde_json::json!({"mcp_servers": [{"type": "url", "url": "x"}]}),
            serde_json::json!({"input": [{"type": "message", "role": "user", "content": [{"type": "input_image", "image_url": "x"}]}]}),
            serde_json::json!({"input": [{"type": "message", "role": "user", "content": [{"type": "input_file", "file_id": "f"}]}]}),
            serde_json::json!({"input": [{"type": "web_search_call", "id": "ws", "status": "completed"}]}),
            serde_json::json!({"tools": [{"type": "web_search_preview"}]}),
            serde_json::json!({"tools": [{"type": "file_search", "vector_store_ids": ["v"]}]}),
            serde_json::json!({"tools": [{"type": "code_interpreter", "container": {"type": "auto"}}]}),
            serde_json::json!({"tools": [{"type": "image_generation"}]}),
            serde_json::json!({"tools": [{"type": "mcp", "server_label": "x"}]}),
            serde_json::json!({"tool_choice": {"type": "web_search_preview"}}),
            serde_json::json!({"messages": [{"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": "x"}}]}]}),
            serde_json::json!({"modalities": ["text", "audio"], "audio": {"voice": "alloy"}}),
            serde_json::json!({"web_search_options": {}}),
            serde_json::json!({"contents": [{"parts": [{"inlineData": {"mimeType": "image/png", "data": "x"}}]}]}),
            serde_json::json!({"contents": [{"parts": [{"fileData": {"fileUri": "x"}}]}]}),
            serde_json::json!({"tools": [{"googleSearch": {}}]}),
            serde_json::json!({"tools": [{"codeExecution": {}}]}),
            serde_json::json!({"tools": [{"urlContext": {}}]}),
            serde_json::json!({"generationConfig": {"responseModalities": ["TEXT", "IMAGE"]}}),
            serde_json::json!({"generationConfig": {"speechConfig": {"voiceConfig": {}}}}),
            serde_json::json!({"cachedContent": "cachedContents/x"}),
        ] {
            assert!(!is_metered_text_value(&body), "{body}");
        }
        assert!(!is_metered_text_payload(b"[1]"));
        assert!(!is_metered_text_payload(b"not json"));
    }
    #[test]
    fn risk_estimate_uses_declared_output_ceiling_and_byte_heuristic() {
        let facts = value_request_facts(br#"{"max_tokens":4096,"service_tier":"auto"}"#);
        assert_eq!(facts.estimate.output_tokens, 4096);
        assert_eq!(facts.estimate.input_tokens, 14);
        assert_eq!(facts.service_tier.as_deref(), Some("auto"));
        assert!(facts.metered_text);
        for (body, ceiling) in [
            (serde_json::json!({}), DEFAULT_OUTPUT_CEILING),
            (serde_json::json!({"max_output_tokens": 0}), 1),
            (
                serde_json::json!({"max_completion_tokens": 999_999}),
                MAX_OUTPUT_CEILING,
            ),
            (
                serde_json::json!({"generationConfig": {"maxOutputTokens": 77}}),
                77,
            ),
        ] {
            assert_eq!(
                RiskEstimate::from_request(0, Some(&body)).output_tokens,
                ceiling
            );
        }
        assert_eq!(RiskEstimate::from_request(7, None).input_tokens, 3);
    }
    #[test]
    fn canonical_service_tier_is_shared_and_never_guesses_unknown_tiers() {
        for raw in [
            None,
            Some(""),
            Some("auto"),
            Some("default"),
            Some("Standard"),
        ] {
            assert_eq!(canonical_service_tier(raw), Some("standard"), "{raw:?}");
        }
        assert_eq!(canonical_service_tier(Some("priority")), Some("priority"));
        assert_eq!(canonical_service_tier(Some("flex")), Some("flex"));
        assert_eq!(canonical_service_tier(Some("scale")), None);
    }
    #[test]
    fn value_quota_rejections_keep_stable_codes() {
        let error = anyhow::Error::new(ValueQuotaRejection {
            kind: ValueQuotaRejectionKind::Busy,
            resets_at_ms: None,
        })
        .context("admission");
        assert_eq!(
            error.downcast_ref::<ValueQuotaRejection>().unwrap().code(),
            "VALUE_QUOTA_BUSY"
        );
        assert!(ValueQuotaRejection::new(ValueQuotaRejectionKind::Exhausted)
            .to_string()
            .starts_with("VALUE_QUOTA_"));
    }
    fn rates() -> ExecutionRates {
        ExecutionRates {
            input: Some("3000000".into()),
            output: Some("15000000".into()),
            cache_read: Some("300000".into()),
            cache_write5m: Some("3750000".into()),
            cache_write1h: Some("6000000".into()),
            cache_write_policy: CacheWritePolicy::Both,
        }
    }
    #[test]
    fn exact_picos_keep_sub_micro_requests_and_real_zero_separate_from_missing() {
        let r = rates();
        let t = ValueTokens {
            fresh_input: 1000,
            output: 1000,
            ..Default::default()
        };
        assert_eq!(r.price(&t).unwrap().usage_picos, 18_000_000_000);
        let mut tiny = r.clone();
        tiny.input = Some("1".into());
        assert_eq!(
            tiny.price(&ValueTokens {
                fresh_input: 1,
                ..Default::default()
            })
            .unwrap()
            .usage_picos,
            1
        );
        tiny.input = Some("0".into());
        assert_eq!(tiny.price(&t).unwrap().usage_picos, 15_000_000_000);
        tiny.input = None;
        assert_eq!(tiny.price(&t), Err(ValueError::PriceUnavailable));
    }
    #[test]
    fn unknown_cache_ttl_uses_proven_upper_bound_without_double_counting() {
        let mut r = rates();
        let t = ValueTokens {
            cache_write_unknown: 4,
            ..Default::default()
        };
        assert_eq!(
            r.price(&t).unwrap(),
            PricedValue {
                usage_picos: 24_000_000,
                conservative_cache_ttl: true
            }
        );
        r.cache_write_policy = CacheWritePolicy::FiveMinutesOnly;
        r.cache_write1h = None;
        assert_eq!(r.price(&t).unwrap().usage_picos, 15_000_000);
        let t = ValueTokens {
            cache_write1h: 1,
            ..Default::default()
        };
        assert_eq!(r.price(&t), Err(ValueError::PriceUnavailable));
    }
    #[test]
    fn raw_context_and_strict_tiers_do_not_underprice_cache_heavy_requests() {
        let mut long = rates();
        long.input = Some("6000000".into());
        let p = ExecutionModelPrice {
            resources: Default::default(),
            context_input_basis: ContextInputBasis::RawInput,
            long_context_threshold: Some(200_000),
            long_context_inclusive: false,
            rates: BTreeMap::from([
                ("standard/base".into(), rates()),
                ("standard/long".into(), long),
            ]),
        };
        let t = ValueTokens {
            fresh_input: 100,
            cache_read: 200_000,
            ..Default::default()
        };
        assert_eq!(p.price("standard", &t).unwrap().usage_picos, 60_600_000_000);
        assert_eq!(p.price("priority", &t), Err(ValueError::PriceUnavailable));
        assert_eq!(p.price("auto", &t), Err(ValueError::PriceUnavailable));
    }
    #[test]
    fn wire_decimals_and_limits_are_bounded_and_exact() {
        for raw in ["", "-1", "+1", "01", "1.0", " 1", "1e6"] {
            assert!(unsigned_decimal(raw).is_err());
        }
        for raw in ["0", "1.", ".1", "1e2", "-1", "1.0000001", " 1"] {
            assert!(usd_to_micros(raw).is_err());
        }
        assert_eq!(usd_to_micros("10.000001").unwrap(), "10000001");
        assert_eq!(
            ValueQuota {
                limit_usd_micros: "10000000".into(),
                tariff_policy_id: TARIFF_POLICY.into()
            }
            .limit_picos()
            .unwrap(),
            10 * PICOS_PER_USD
        );
        assert!(ValueQuota {
            limit_usd_micros: "0".into(),
            tariff_policy_id: TARIFF_POLICY.into()
        }
        .limit_picos()
        .is_err());
    }
    #[test]
    fn snapshots_are_canonical_bounded_and_reject_duplicate_keys() {
        let snapshot = PricingSnapshot {
            schema_version: 1,
            tariff_policy_id: TARIFF_POLICY.into(),
            currency: "USD".into(),
            models: BTreeMap::from([(
                "test-model".into(),
                ExecutionModelPrice {
                    resources: Default::default(),
                    context_input_basis: ContextInputBasis::RawInput,
                    long_context_threshold: None,
                    long_context_inclusive: false,
                    rates: BTreeMap::from([("standard/base".into(), rates())]),
                },
            )]),
            exact_aliases: BTreeMap::new(),
        };
        let encoded = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            snapshot.revision().unwrap(),
            serde_json::from_str::<PricingSnapshot>(&encoded)
                .unwrap()
                .revision()
                .unwrap()
        );
        let duplicate = encoded.replace(
            "\"standard/base\":",
            &format!(
                "\"standard/base\":{},\"standard/base\":",
                serde_json::to_string(&rates()).unwrap()
            ),
        );
        assert!(serde_json::from_str::<PricingSnapshot>(&duplicate).is_err());
        let mut corrupt = snapshot;
        corrupt
            .exact_aliases
            .insert("test-model".into(), "missing".into());
        assert!(corrupt.revision().is_err());
    }
}
