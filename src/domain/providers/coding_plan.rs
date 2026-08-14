use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Context};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use super::model::AppKind;
use super::registry::{AuthScheme, ProfileId, ProviderKey, UpstreamProtocol};

const MAX_STATIC_MODELS: usize = 64;
const MAX_ROUTE_COUNT: usize = 8;
const MAX_RESET_PAST_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_RESET_FUTURE_MS: i64 = 400 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanProfileSpec {
    pub contract_revision: u32,
    pub inference: CodingPlanInferenceSpec,
    pub routes: Vec<CodingPlanRouteSpec>,
    pub models: Vec<CodingPlanModelSpec>,
    pub quota: CodingPlanQuotaSpec,
    pub cache_tokens: CodingPlanCacheTokenSpec,
    pub stream: CodingPlanStreamSpec,
    pub error: CodingPlanErrorSpec,
    pub pricing: CodingPlanPricingSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanInferenceSpec {
    pub fixed_origin: String,
    pub protocol: UpstreamProtocol,
    pub credential_slot: String,
    pub auth_scheme: AuthScheme,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanRoute {
    ClaudeMessages,
    ClaudeCountTokens,
    CodexChatCompletions,
    CodexResponses,
}

impl CodingPlanRoute {
    pub fn app(self) -> AppKind {
        match self {
            Self::ClaudeMessages | Self::ClaudeCountTokens => AppKind::Claude,
            Self::CodexChatCompletions | Self::CodexResponses => AppKind::Codex,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanRouteSpec {
    pub route: CodingPlanRoute,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanModelSpec {
    pub id: String,
    pub display_name: String,
    pub context_window: u64,
    #[serde(default)]
    pub input_modalities: Vec<CodingPlanModality>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanModality {
    Text,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanQuotaSpec {
    pub adapter: CodingPlanQuotaAdapter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub credential_slots: Vec<CodingPlanQuotaCredentialSlot>,
    pub cache_ttl_ms: u64,
    pub stale_ttl_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanQuotaAdapter {
    Kimi,
    Zhipu,
    Minimax,
    Volcengine,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanQuotaCredentialSlot {
    pub role: CodingPlanQuotaCredentialRole,
    pub slot: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanQuotaCredentialRole {
    InferenceCredential,
    AccessKeyId,
    SecretAccessKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanCacheTokenSpec {
    InputIncludesCached,
    InputExcludesCached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanStreamSpec {
    pub format: CodingPlanStreamFormat,
    pub terminal_event: String,
    pub error_before_terminal_is_fatal: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanStreamFormat {
    AnthropicSse,
    OpenAiChatSse,
    OpenAiResponsesSse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanErrorSpec {
    pub envelope: CodingPlanErrorEnvelope,
    pub retry_same_credential_once_on_401: bool,
    pub retry_after_commit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanErrorEnvelope {
    Anthropic,
    OpenAi,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanPricingSpec {
    pub evidence: CodingPlanPricingEvidence,
    pub source: String,
    pub captured_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanPricingEvidence {
    FlatRateSubscriptionNoUsd,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeCodingPlan {
    pub contract_revision: u32,
    pub fixed_origin: String,
    pub protocol: UpstreamProtocol,
    pub inference_credential_slot: String,
    pub inference_auth_scheme: AuthScheme,
    pub routes: BTreeMap<CodingPlanRoute, String>,
    pub models: Vec<CodingPlanModelSpec>,
    pub quota: CodingPlanQuotaSpec,
    pub cache_tokens: CodingPlanCacheTokenSpec,
    pub stream: CodingPlanStreamSpec,
    pub error: CodingPlanErrorSpec,
    pub pricing: CodingPlanPricingSpec,
}

impl RuntimeCodingPlan {
    pub fn endpoint_for_route(&self, route: CodingPlanRoute) -> anyhow::Result<String> {
        let path = self
            .routes
            .get(&route)
            .with_context(|| format!("coding plan does not allow route {route:?}"))?;
        exact_url(&self.fixed_origin, path)
    }

    pub fn guard_final_endpoint(
        &self,
        route: CodingPlanRoute,
        candidate: &str,
    ) -> anyhow::Result<()> {
        let expected = self.endpoint_for_route(route)?;
        let expected = Url::parse(&expected).context("parse expected coding-plan endpoint")?;
        let candidate = Url::parse(candidate).context("parse final coding-plan endpoint")?;
        if candidate.username() != ""
            || candidate.password().is_some()
            || candidate.fragment().is_some()
            || candidate.query().is_some()
            || candidate.scheme() != expected.scheme()
            || candidate.host_str() != expected.host_str()
            || candidate.port_or_known_default() != expected.port_or_known_default()
            || candidate.path() != expected.path()
        {
            bail!("final coding-plan endpoint does not match the Registry route contract");
        }
        Ok(())
    }

    pub fn allows_model(&self, model: &str) -> bool {
        let model = model.trim();
        !model.is_empty() && self.models.iter().any(|item| item.id == model)
    }
}

pub fn compile_profile_contract(
    app: AppKind,
    spec: &CodingPlanProfileSpec,
) -> anyhow::Result<RuntimeCodingPlan> {
    validate_profile_contract(app, spec)?;
    let fixed_origin = normalized_origin(&spec.inference.fixed_origin)?;
    let routes = spec
        .routes
        .iter()
        .map(|item| (item.route, item.path.clone()))
        .collect::<BTreeMap<_, _>>();
    Ok(RuntimeCodingPlan {
        contract_revision: spec.contract_revision,
        fixed_origin,
        protocol: spec.inference.protocol,
        inference_credential_slot: spec.inference.credential_slot.clone(),
        inference_auth_scheme: spec.inference.auth_scheme,
        routes,
        models: spec.models.clone(),
        quota: spec.quota.clone(),
        cache_tokens: spec.cache_tokens,
        stream: spec.stream.clone(),
        error: spec.error.clone(),
        pricing: spec.pricing.clone(),
    })
}

pub fn validate_profile_contract(app: AppKind, spec: &CodingPlanProfileSpec) -> anyhow::Result<()> {
    if spec.contract_revision == 0 {
        bail!("coding-plan contract revision must be non-zero");
    }
    normalized_origin(&spec.inference.fixed_origin)?;
    validate_json_pointer(&spec.inference.credential_slot, "inference credential")?;
    if !matches!(
        spec.inference.auth_scheme,
        AuthScheme::ApiKey | AuthScheme::Bearer
    ) {
        bail!("coding-plan inference auth must be api_key or bearer");
    }
    if spec.routes.is_empty() || spec.routes.len() > MAX_ROUTE_COUNT {
        bail!("coding-plan contract must define between one and {MAX_ROUTE_COUNT} routes");
    }
    let mut routes = BTreeSet::new();
    for route in &spec.routes {
        if route.route.app() != app {
            bail!("coding-plan route {:?} belongs to another App", route.route);
        }
        if !routes.insert(route.route) {
            bail!("coding-plan contract repeats route {:?}", route.route);
        }
        validate_exact_path(&route.path)?;
        exact_url(&spec.inference.fixed_origin, &route.path)?;
    }
    let required_route = match app {
        AppKind::Claude => CodingPlanRoute::ClaudeMessages,
        AppKind::Codex => CodingPlanRoute::CodexResponses,
        AppKind::Gemini => bail!("coding-plan contracts currently support Claude and Codex only"),
    };
    if !routes.contains(&required_route) {
        bail!("coding-plan contract is missing required route {required_route:?}");
    }

    if spec.models.is_empty() || spec.models.len() > MAX_STATIC_MODELS {
        bail!("coding-plan contract must define a bounded static model catalog");
    }
    let mut models = BTreeSet::new();
    for model in &spec.models {
        if model.id.trim().is_empty()
            || model.id != model.id.trim()
            || model.id.len() > 256
            || model.display_name.trim().is_empty()
            || model.display_name != model.display_name.trim()
            || model.context_window == 0
            || model.context_window > 4_194_304
        {
            bail!("coding-plan model catalog contains an invalid model");
        }
        if !models.insert(model.id.as_str()) {
            bail!("coding-plan model catalog repeats model {}", model.id);
        }
        let modalities = model
            .input_modalities
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        if modalities.len() != model.input_modalities.len()
            || !modalities.contains(&CodingPlanModality::Text)
        {
            bail!(
                "coding-plan model {} must have unique modalities including text",
                model.id
            );
        }
    }

    validate_quota_contract(&spec.inference, &spec.quota)?;
    validate_stream_contract(app, spec)?;
    if spec.error.retry_after_commit {
        bail!("coding-plan contracts cannot retry after downstream commit");
    }
    if spec.pricing.source.trim().is_empty()
        || spec.pricing.source != spec.pricing.source.trim()
        || DateTime::parse_from_rfc3339(&spec.pricing.captured_at).is_err()
    {
        bail!("coding-plan pricing evidence is incomplete");
    }
    Ok(())
}

fn validate_quota_contract(
    inference: &CodingPlanInferenceSpec,
    quota: &CodingPlanQuotaSpec,
) -> anyhow::Result<()> {
    if quota.cache_ttl_ms < 1_000
        || quota.cache_ttl_ms > 60 * 60 * 1_000
        || quota.stale_ttl_ms < quota.cache_ttl_ms
        || quota.stale_ttl_ms > 24 * 60 * 60 * 1_000
    {
        bail!("coding-plan quota cache durations are invalid");
    }
    let mut roles = BTreeSet::new();
    let mut slots = BTreeSet::new();
    for credential in &quota.credential_slots {
        validate_json_pointer(&credential.slot, "quota credential")?;
        if !roles.insert(credential.role) || !slots.insert(credential.slot.as_str()) {
            bail!("coding-plan quota credential roles and slots must be unique");
        }
    }
    match quota.adapter {
        CodingPlanQuotaAdapter::Unavailable => {
            if quota.endpoint.is_some() || !quota.credential_slots.is_empty() {
                bail!("unavailable coding-plan quota cannot define endpoint or credentials");
            }
        }
        CodingPlanQuotaAdapter::Kimi
        | CodingPlanQuotaAdapter::Zhipu
        | CodingPlanQuotaAdapter::Minimax => {
            validate_fixed_url(
                quota
                    .endpoint
                    .as_deref()
                    .context("quota endpoint is required")?,
            )?;
            if quota.credential_slots.len() != 1
                || quota.credential_slots[0].role
                    != CodingPlanQuotaCredentialRole::InferenceCredential
                || quota.credential_slots[0].slot != inference.credential_slot
            {
                bail!("coding-plan quota must use the exact inference credential slot");
            }
        }
        CodingPlanQuotaAdapter::Volcengine => {
            let endpoint = quota
                .endpoint
                .as_deref()
                .context("quota endpoint is required")?;
            validate_fixed_url(endpoint)?;
            let parsed = Url::parse(endpoint)?;
            if parsed.scheme() != "https"
                || parsed.host_str() != Some("open.volcengineapi.com")
                || parsed.path() != "/"
            {
                bail!("Volcengine quota endpoint must use the fixed control-plane origin");
            }
            let expected = BTreeSet::from([
                CodingPlanQuotaCredentialRole::AccessKeyId,
                CodingPlanQuotaCredentialRole::SecretAccessKey,
            ]);
            if roles != expected || slots.contains(inference.credential_slot.as_str()) {
                bail!("Volcengine quota AK/SK must be separate from inference credentials");
            }
        }
    }
    Ok(())
}

fn validate_stream_contract(app: AppKind, spec: &CodingPlanProfileSpec) -> anyhow::Result<()> {
    if spec.stream.terminal_event.trim().is_empty()
        || spec.stream.terminal_event != spec.stream.terminal_event.trim()
        || !spec.stream.error_before_terminal_is_fatal
    {
        bail!("coding-plan stream contract is incomplete");
    }
    let valid = match (
        app,
        spec.inference.protocol,
        spec.stream.format,
        spec.error.envelope,
    ) {
        (
            AppKind::Claude,
            UpstreamProtocol::AnthropicMessages,
            CodingPlanStreamFormat::AnthropicSse,
            CodingPlanErrorEnvelope::Anthropic,
        ) => spec.stream.terminal_event == "message_stop",
        (
            AppKind::Codex,
            UpstreamProtocol::OpenAiChat,
            CodingPlanStreamFormat::OpenAiChatSse,
            CodingPlanErrorEnvelope::OpenAi,
        ) => spec.stream.terminal_event == "[DONE]",
        (
            AppKind::Codex,
            UpstreamProtocol::OpenAiResponses,
            CodingPlanStreamFormat::OpenAiResponsesSse,
            CodingPlanErrorEnvelope::OpenAi,
        ) => spec.stream.terminal_event == "response.completed",
        _ => false,
    };
    if !valid {
        bail!("coding-plan protocol, stream, and error contracts disagree");
    }
    Ok(())
}

fn validate_json_pointer(pointer: &str, label: &str) -> anyhow::Result<()> {
    if !pointer.starts_with("/settingsConfig/")
        || pointer.contains("//")
        || pointer.trim() != pointer
        || pointer.len() > 256
    {
        bail!("{label} slot is not a canonical settings JSON pointer");
    }
    Ok(())
}

fn validate_exact_path(path: &str) -> anyhow::Result<()> {
    if !path.starts_with('/')
        || path == "/"
        || path.ends_with('/')
        || path.contains("//")
        || path.contains('?')
        || path.contains('#')
        || path.contains("..")
        || path.trim() != path
    {
        bail!("coding-plan route path must be exact and canonical");
    }
    Ok(())
}

fn normalized_origin(origin: &str) -> anyhow::Result<String> {
    let parsed = Url::parse(origin).context("coding-plan inference origin is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        bail!("coding-plan inference origin must be a credential-free HTTPS origin");
    }
    Ok(parsed.origin().ascii_serialization())
}

fn validate_fixed_url(value: &str) -> anyhow::Result<()> {
    let parsed = Url::parse(value).context("coding-plan fixed URL is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        bail!("coding-plan fixed URL must be credential-free HTTPS without query or fragment");
    }
    Ok(())
}

fn exact_url(origin: &str, path: &str) -> anyhow::Result<String> {
    validate_exact_path(path)?;
    let origin = normalized_origin(origin)?;
    let mut parsed = Url::parse(&origin)?;
    parsed.set_path(path);
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanQuotaCacheKey {
    pub provider_key: ProviderKey,
    pub provider_revision: u64,
    pub credential_generation: u64,
    pub runtime_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanQuotaState {
    Supported,
    Stale,
    Unknown,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanQuotaView {
    pub state: CodingPlanQuotaState,
    pub windows: Vec<CodingPlanQuotaWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_since_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanQuotaSource {
    Live,
    FreshCache,
    StaleCache,
    Contract,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanQuotaSnapshot {
    pub provider_key: ProviderKey,
    pub provider_revision: u64,
    pub credential_generation: u64,
    pub runtime_fingerprint: String,
    pub profile_id: ProfileId,
    pub source: CodingPlanQuotaSource,
    pub quota: CodingPlanQuotaView,
}

impl CodingPlanQuotaView {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            state: CodingPlanQuotaState::Unavailable,
            windows: Vec::new(),
            plan: None,
            observed_at_ms: None,
            stale_since_ms: None,
            reason: Some(reason.into()),
        }
    }

    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            state: CodingPlanQuotaState::Unknown,
            windows: Vec::new(),
            plan: None,
            observed_at_ms: None,
            stale_since_ms: None,
            reason: Some(reason.into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingPlanQuotaWindowKind {
    FiveHour,
    Weekly,
    Monthly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CodingPlanQuotaWindow {
    pub kind: CodingPlanQuotaWindowKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone)]
struct CodingPlanQuotaCacheEntry {
    view: CodingPlanQuotaView,
}

#[derive(Debug, Default)]
pub struct CodingPlanQuotaCache {
    entries: BTreeMap<CodingPlanQuotaCacheKey, CodingPlanQuotaCacheEntry>,
}

impl CodingPlanQuotaCache {
    pub fn fresh(
        &self,
        key: &CodingPlanQuotaCacheKey,
        now_ms: i64,
        cache_ttl_ms: u64,
    ) -> Option<CodingPlanQuotaView> {
        let entry = self.entries.get(key)?;
        let observed_at_ms = entry.view.observed_at_ms?;
        let age_ms = cache_age_ms(observed_at_ms, now_ms)?;
        (age_ms <= duration_as_i64(cache_ttl_ms)).then(|| entry.view.clone())
    }

    pub fn stale(
        &self,
        key: &CodingPlanQuotaCacheKey,
        now_ms: i64,
        cache_ttl_ms: u64,
        stale_ttl_ms: u64,
        reason: impl Into<String>,
    ) -> Option<CodingPlanQuotaView> {
        let entry = self.entries.get(key)?;
        let observed_at_ms = entry.view.observed_at_ms?;
        let age_ms = cache_age_ms(observed_at_ms, now_ms)?;
        if age_ms > duration_as_i64(stale_ttl_ms) {
            return None;
        }
        let mut view = entry.view.clone();
        view.state = CodingPlanQuotaState::Stale;
        view.stale_since_ms = Some(observed_at_ms.saturating_add(duration_as_i64(cache_ttl_ms)));
        view.reason = Some(reason.into());
        Some(view)
    }

    pub fn insert_supported(
        &mut self,
        key: CodingPlanQuotaCacheKey,
        view: CodingPlanQuotaView,
    ) -> anyhow::Result<()> {
        if view.state != CodingPlanQuotaState::Supported || view.observed_at_ms.is_none() {
            bail!("only observed supported coding-plan quota can be cached");
        }
        self.retain_current(&key);
        self.entries.insert(key, CodingPlanQuotaCacheEntry { view });
        Ok(())
    }

    pub fn invalidate(&mut self, key: &CodingPlanQuotaCacheKey) {
        self.entries.remove(key);
    }

    pub fn retain_current(&mut self, key: &CodingPlanQuotaCacheKey) {
        self.entries
            .retain(|candidate, _| candidate.provider_key != key.provider_key || candidate == key);
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

fn cache_age_ms(observed_at_ms: i64, now_ms: i64) -> Option<i64> {
    let age_ms = now_ms.checked_sub(observed_at_ms)?;
    (age_ms >= 0).then_some(age_ms)
}

fn duration_as_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub fn parse_kimi_quota(body: &Value, observed_at_ms: i64) -> anyhow::Result<CodingPlanQuotaView> {
    let object = body
        .as_object()
        .context("Kimi quota response must be an object")?;
    let mut windows = Vec::new();
    if let Some(usage) = object.get("usage") {
        insert_quota_window(
            &mut windows,
            parse_remaining_window(
                CodingPlanQuotaWindowKind::Weekly,
                usage,
                observed_at_ms,
                &["resetTime", "reset_at", "resetAt"],
            )?,
        )?;
    }
    if let Some(limits) = object.get("limits") {
        let limits = limits.as_array().context("Kimi limits must be an array")?;
        let mut candidates = Vec::new();
        for item in limits {
            let detail = item.get("detail").context("Kimi limit is missing detail")?;
            candidates.push(parse_remaining_window(
                CodingPlanQuotaWindowKind::FiveHour,
                detail,
                observed_at_ms,
                &["resetTime", "reset_at", "resetAt"],
            )?);
        }
        if let Some(window) = candidates.into_iter().max_by(|left, right| {
            left.utilization
                .partial_cmp(&right.utilization)
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            insert_quota_window(&mut windows, window)?;
        }
    }

    for (field, kind) in [
        ("five_hour", CodingPlanQuotaWindowKind::FiveHour),
        ("seven_day", CodingPlanQuotaWindowKind::Weekly),
    ] {
        if windows
            .iter()
            .any(|window| window.kind == kind && window.scope.is_none())
        {
            continue;
        }
        if let Some(value) = object.get(field) {
            insert_quota_window(
                &mut windows,
                parse_utilization_window(kind, value, observed_at_ms, None)?,
            )?;
        }
    }

    for (field, value) in object {
        let Some(scope) = field.strip_prefix("seven_day_") else {
            continue;
        };
        if scope.is_empty() {
            continue;
        }
        insert_quota_window(
            &mut windows,
            parse_utilization_window(
                CodingPlanQuotaWindowKind::Weekly,
                value,
                observed_at_ms,
                Some(normalize_quota_scope(scope)?),
            )?,
        )?;
    }

    let mut view = finish_quota_view(body, windows, observed_at_ms)?;
    view.plan = kimi_membership_plan(body);
    Ok(view)
}

pub fn parse_zhipu_quota(body: &Value, observed_at_ms: i64) -> anyhow::Result<CodingPlanQuotaView> {
    if body.get("success").and_then(Value::as_bool) == Some(false) {
        bail!("Zhipu quota response reports failure");
    }
    let data = body
        .get("data")
        .context("Zhipu quota response is missing data")?;
    let limits = data
        .get("limits")
        .and_then(Value::as_array)
        .context("Zhipu quota data is missing limits")?;
    let mut classified = BTreeMap::new();
    let mut unclassified = Vec::new();
    for item in limits {
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        if !kind.eq_ignore_ascii_case("TOKENS_LIMIT") {
            continue;
        }
        let window_kind = match item.get("unit") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let unit = strict_i64(value)?;
                match unit {
                    3 => Some(CodingPlanQuotaWindowKind::FiveHour),
                    6 => Some(CodingPlanQuotaWindowKind::Weekly),
                    4 if item.get("number").map(strict_i64).transpose()? == Some(7) => {
                        Some(CodingPlanQuotaWindowKind::Weekly)
                    }
                    _ => bail!("Zhipu TOKENS_LIMIT has an unknown unit {unit}"),
                }
            }
        };
        let utilization = strict_percentage(
            item.get("percentage")
                .context("Zhipu TOKENS_LIMIT is missing percentage")?,
        )?;
        let resets_at_ms = optional_reset_ms(item.get("nextResetTime"), observed_at_ms)?;
        let window = CodingPlanQuotaWindow {
            kind: window_kind.unwrap_or(CodingPlanQuotaWindowKind::FiveHour),
            scope: None,
            utilization,
            resets_at_ms,
            used: None,
            limit: None,
            unit: Some("tokens".to_string()),
        };
        if let Some(window_kind) = window_kind {
            if classified.insert(window_kind, window).is_some() {
                bail!("Zhipu quota response repeats a TOKENS_LIMIT window");
            }
        } else {
            unclassified.push(window);
        }
    }
    if classified.len().saturating_add(unclassified.len()) > 2 {
        bail!("Zhipu quota response contains more than two TOKENS_LIMIT windows");
    }
    if classified.is_empty() {
        unclassified.sort_by_key(|window| window.resets_at_ms.unwrap_or(i64::MAX));
    }
    for mut window in unclassified {
        let kind = if !classified.contains_key(&CodingPlanQuotaWindowKind::FiveHour) {
            CodingPlanQuotaWindowKind::FiveHour
        } else if !classified.contains_key(&CodingPlanQuotaWindowKind::Weekly) {
            CodingPlanQuotaWindowKind::Weekly
        } else {
            bail!("Zhipu quota response cannot classify an additional TOKENS_LIMIT window");
        };
        window.kind = kind;
        classified.insert(kind, window);
    }
    finish_quota_view(body, classified.into_values().collect(), observed_at_ms)
}

pub fn parse_minimax_quota(
    body: &Value,
    observed_at_ms: i64,
) -> anyhow::Result<CodingPlanQuotaView> {
    if let Some(base) = body.get("base_resp").or_else(|| body.get("baseResp")) {
        let status = base
            .get("status_code")
            .or_else(|| base.get("statusCode"))
            .context("MiniMax base response is missing status code")?;
        if strict_i64(status)? != 0 {
            bail!("MiniMax quota response reports failure");
        }
    }
    let models = body
        .get("model_remains")
        .or_else(|| body.get("modelRemains"))
        .and_then(Value::as_array)
        .context("MiniMax quota response is missing model_remains")?;
    let general = models
        .iter()
        .filter(|item| {
            item.get("model_name")
                .or_else(|| item.get("modelName"))
                .and_then(Value::as_str)
                == Some("general")
        })
        .collect::<Vec<_>>();
    if general.len() != 1 {
        bail!("MiniMax quota response must contain exactly one general model bucket");
    }
    let item = general[0];
    let mut windows = Vec::new();
    windows.push(parse_minimax_window(
        item,
        CodingPlanQuotaWindowKind::FiveHour,
        observed_at_ms,
        "current_interval_remaining_percent",
        "currentIntervalRemainingPercent",
        "current_interval_total_count",
        "currentIntervalTotalCount",
        "current_interval_usage_count",
        "currentIntervalUsageCount",
        "remains_time",
        "remainsTime",
        "end_time",
        "endTime",
    )?);

    let weekly_status = item
        .get("current_weekly_status")
        .or_else(|| item.get("currentWeeklyStatus"))
        .map(strict_i64)
        .transpose()?;
    if weekly_status == Some(1) {
        windows.push(parse_minimax_window(
            item,
            CodingPlanQuotaWindowKind::Weekly,
            observed_at_ms,
            "current_weekly_remaining_percent",
            "currentWeeklyRemainingPercent",
            "current_weekly_total_count",
            "currentWeeklyTotalCount",
            "current_weekly_usage_count",
            "currentWeeklyUsageCount",
            "weekly_remains_time",
            "weeklyRemainsTime",
            "weekly_end_time",
            "weeklyEndTime",
        )?);
    }
    finish_quota_view(body, windows, observed_at_ms)
}

pub fn parse_volcengine_afp_quota(
    body: &Value,
    observed_at_ms: i64,
) -> anyhow::Result<CodingPlanQuotaView> {
    reject_volcengine_error(body)?;
    let result = body.get("Result").unwrap_or(body);
    let mut windows = Vec::new();
    for (field, kind) in [
        ("AFPFiveHour", CodingPlanQuotaWindowKind::FiveHour),
        ("AFPWeekly", CodingPlanQuotaWindowKind::Weekly),
        ("AFPMonthly", CodingPlanQuotaWindowKind::Monthly),
    ] {
        let Some(window) = result.get(field) else {
            continue;
        };
        let limit = strict_non_negative(
            window
                .get("Quota")
                .context("Volcengine AFP window is missing Quota")?,
        )?;
        if limit == 0.0 {
            continue;
        }
        let used = strict_non_negative(
            window
                .get("Used")
                .context("Volcengine AFP window is missing Used")?,
        )?;
        if used > limit {
            bail!("Volcengine AFP usage exceeds its quota");
        }
        windows.push(CodingPlanQuotaWindow {
            kind,
            scope: None,
            utilization: used / limit * 100.0,
            resets_at_ms: optional_reset_ms(window.get("ResetTime"), observed_at_ms)?,
            used: Some(used),
            limit: Some(limit),
            unit: Some("afp".to_string()),
        });
    }
    let mut view = finish_quota_view(body, windows, observed_at_ms)?;
    view.plan = result
        .get("PlanType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("Agent Plan {value}"));
    Ok(view)
}

pub fn volcengine_afp_has_active_window(body: &Value) -> anyhow::Result<bool> {
    reject_volcengine_error(body)?;
    let result = body.get("Result").unwrap_or(body);
    for field in ["AFPFiveHour", "AFPWeekly", "AFPMonthly"] {
        let Some(window) = result.get(field) else {
            continue;
        };
        let quota = strict_non_negative(
            window
                .get("Quota")
                .context("Volcengine AFP window is missing Quota")?,
        )?;
        if quota > 0.0 {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn parse_volcengine_coding_quota(
    body: &Value,
    observed_at_ms: i64,
) -> anyhow::Result<CodingPlanQuotaView> {
    reject_volcengine_error(body)?;
    let result = body.get("Result").unwrap_or(body);
    let values = result
        .get("QuotaUsage")
        .and_then(Value::as_array)
        .context("Volcengine Coding Plan response is missing QuotaUsage")?;
    let mut windows = BTreeMap::new();
    for item in values {
        let label = item
            .get("Level")
            .and_then(Value::as_str)
            .context("Volcengine Coding Plan window is missing Level")?;
        let kind = match label.to_ascii_lowercase().as_str() {
            "session" => CodingPlanQuotaWindowKind::FiveHour,
            "weekly" => CodingPlanQuotaWindowKind::Weekly,
            "monthly" => CodingPlanQuotaWindowKind::Monthly,
            _ => bail!("Volcengine Coding Plan has an unknown window {label}"),
        };
        let utilization = strict_percentage(
            item.get("Percent")
                .context("Volcengine Coding Plan window is missing Percent")?,
        )?;
        let reset = item.get("ResetTimestamp");
        let resets_at_ms = match reset.map(strict_i64).transpose()? {
            Some(value) if value <= 0 => None,
            Some(value) => Some(validate_reset_ms(timestamp_to_ms(value)?, observed_at_ms)?),
            None => None,
        };
        if windows
            .insert(
                kind,
                CodingPlanQuotaWindow {
                    kind,
                    scope: None,
                    utilization,
                    resets_at_ms,
                    used: None,
                    limit: None,
                    unit: Some("percent".to_string()),
                },
            )
            .is_some()
        {
            bail!("Volcengine Coding Plan response repeats a window");
        }
    }
    let mut view = finish_quota_view(body, windows.into_values().collect(), observed_at_ms)?;
    view.plan = Some("Coding Plan".to_string());
    Ok(view)
}

fn parse_remaining_window(
    kind: CodingPlanQuotaWindowKind,
    value: &Value,
    observed_at_ms: i64,
    reset_fields: &[&str],
) -> anyhow::Result<CodingPlanQuotaWindow> {
    let limit = strict_positive(
        value
            .get("limit")
            .context("quota window is missing limit")?,
    )?;
    let remaining = strict_non_negative(
        value
            .get("remaining")
            .context("quota window is missing remaining")?,
    )?;
    if remaining > limit {
        bail!("quota remaining value exceeds its limit");
    }
    let used = value
        .get("used")
        .map(strict_non_negative)
        .transpose()?
        .unwrap_or(limit - remaining);
    if used > limit {
        bail!("quota used value exceeds its limit");
    }
    let reset = reset_fields.iter().find_map(|field| value.get(*field));
    Ok(CodingPlanQuotaWindow {
        kind,
        scope: None,
        utilization: (limit - remaining) / limit * 100.0,
        resets_at_ms: optional_reset_ms(reset, observed_at_ms)?,
        used: Some(used),
        limit: Some(limit),
        unit: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn parse_minimax_window(
    item: &Value,
    kind: CodingPlanQuotaWindowKind,
    observed_at_ms: i64,
    percent_snake: &str,
    percent_camel: &str,
    total_snake: &str,
    total_camel: &str,
    remaining_snake: &str,
    remaining_camel: &str,
    duration_snake: &str,
    duration_camel: &str,
    end_snake: &str,
    end_camel: &str,
) -> anyhow::Result<CodingPlanQuotaWindow> {
    let percent = item
        .get(percent_snake)
        .or_else(|| item.get(percent_camel))
        .map(strict_percentage)
        .transpose()?;
    let total = item
        .get(total_snake)
        .or_else(|| item.get(total_camel))
        .map(strict_non_negative)
        .transpose()?;
    let remaining_count = item
        .get(remaining_snake)
        .or_else(|| item.get(remaining_camel))
        .map(strict_non_negative)
        .transpose()?;
    let (utilization, used, limit) = if let Some(remaining) = percent {
        (100.0 - remaining, None, None)
    } else if let (Some(total), Some(remaining)) = (total, remaining_count) {
        if total <= 0.0 || remaining > total {
            bail!("MiniMax count-based quota window is invalid");
        }
        (
            (total - remaining) / total * 100.0,
            Some(total - remaining),
            Some(total),
        )
    } else {
        bail!("MiniMax quota window has neither remaining percent nor validated counts");
    };
    let resets_at_ms = if let Some(duration) = item
        .get(duration_snake)
        .or_else(|| item.get(duration_camel))
    {
        let duration = strict_i64(duration)?;
        if duration <= 0 || duration > MAX_RESET_FUTURE_MS {
            bail!("MiniMax quota reset duration is invalid");
        }
        Some(validate_reset_ms(
            observed_at_ms
                .checked_add(duration)
                .context("MiniMax quota reset duration overflow")?,
            observed_at_ms,
        )?)
    } else {
        optional_reset_ms(
            item.get(end_snake).or_else(|| item.get(end_camel)),
            observed_at_ms,
        )?
    };
    Ok(CodingPlanQuotaWindow {
        kind,
        scope: None,
        utilization,
        resets_at_ms,
        used,
        limit,
        unit: None,
    })
}

fn finish_quota_view(
    body: &Value,
    mut windows: Vec<CodingPlanQuotaWindow>,
    observed_at_ms: i64,
) -> anyhow::Result<CodingPlanQuotaView> {
    if windows.is_empty() {
        bail!("coding-plan quota response contains no supported windows");
    }
    windows.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.scope.cmp(&right.scope))
    });
    if windows
        .iter()
        .map(|window| (window.kind, window.scope.as_deref()))
        .collect::<BTreeSet<_>>()
        .len()
        != windows.len()
    {
        bail!("coding-plan quota response repeats a window");
    }
    let plan = body
        .pointer("/user/membership/level")
        .or_else(|| body.pointer("/data/level"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok(CodingPlanQuotaView {
        state: CodingPlanQuotaState::Supported,
        windows,
        plan,
        observed_at_ms: Some(observed_at_ms),
        stale_since_ms: None,
        reason: None,
    })
}

fn insert_quota_window(
    windows: &mut Vec<CodingPlanQuotaWindow>,
    window: CodingPlanQuotaWindow,
) -> anyhow::Result<()> {
    if windows
        .iter()
        .any(|candidate| candidate.kind == window.kind && candidate.scope == window.scope)
    {
        bail!("coding-plan quota response repeats a window");
    }
    windows.push(window);
    Ok(())
}

fn parse_utilization_window(
    kind: CodingPlanQuotaWindowKind,
    value: &Value,
    observed_at_ms: i64,
    scope: Option<String>,
) -> anyhow::Result<CodingPlanQuotaWindow> {
    let utilization = strict_percentage(
        value
            .get("utilization")
            .context("quota utilization window is missing utilization")?,
    )?;
    let reset = ["resets_at", "reset_at", "resetAt", "resetTime"]
        .iter()
        .find_map(|field| value.get(*field));
    Ok(CodingPlanQuotaWindow {
        kind,
        scope,
        utilization,
        resets_at_ms: optional_reset_ms(reset, observed_at_ms)?,
        used: None,
        limit: None,
        unit: Some("percent".to_string()),
    })
}

fn normalize_quota_scope(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty()
        || value.len() > 96
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        bail!("coding-plan quota scope is invalid");
    }
    Ok(value.to_string())
}

fn kimi_membership_plan(body: &Value) -> Option<String> {
    let level = body.pointer("/user/membership/level")?.as_str()?.trim();
    if level.is_empty() {
        return None;
    }
    Some(
        match level {
            "LEVEL_BASIC" => "Moderato",
            "LEVEL_INTERMEDIATE" => "Allegretto",
            "LEVEL_ADVANCED" => "Allegro",
            "LEVEL_STANDARD" => "Vivace",
            _ => level.strip_prefix("LEVEL_").unwrap_or(level),
        }
        .to_string(),
    )
}

fn reject_volcengine_error(body: &Value) -> anyhow::Result<()> {
    if body.pointer("/ResponseMetadata/Error").is_some() || body.get("Error").is_some() {
        bail!("Volcengine quota response contains an OpenAPI error");
    }
    Ok(())
}

fn strict_number(value: &Value) -> anyhow::Result<f64> {
    let parsed = value.as_f64().or_else(|| {
        value
            .as_str()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(|value| value.parse::<f64>().ok())
    });
    let parsed = parsed.context("quota value must be a finite number or numeric string")?;
    if !parsed.is_finite() {
        bail!("quota value must be finite");
    }
    Ok(parsed)
}

fn strict_i64(value: &Value) -> anyhow::Result<i64> {
    value
        .as_i64()
        .or_else(|| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .and_then(|value| value.parse::<i64>().ok())
        })
        .context("quota value must be an integer or integer string")
}

fn strict_non_negative(value: &Value) -> anyhow::Result<f64> {
    let parsed = strict_number(value)?;
    if parsed < 0.0 {
        bail!("quota value must not be negative");
    }
    Ok(parsed)
}

fn strict_positive(value: &Value) -> anyhow::Result<f64> {
    let parsed = strict_number(value)?;
    if parsed <= 0.0 {
        bail!("quota limit must be positive");
    }
    Ok(parsed)
}

fn strict_percentage(value: &Value) -> anyhow::Result<f64> {
    let parsed = strict_number(value)?;
    if !(0.0..=100.0).contains(&parsed) {
        bail!("quota percentage must be between zero and one hundred");
    }
    Ok(parsed)
}

fn optional_reset_ms(value: Option<&Value>, observed_at_ms: i64) -> anyhow::Result<Option<i64>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() || value.as_str().is_some_and(|value| value.trim().is_empty()) {
        return Ok(None);
    }
    let reset = if let Some(value) = value.as_str() {
        if let Ok(timestamp) = value.trim().parse::<i64>() {
            timestamp_to_ms(timestamp)?
        } else {
            DateTime::parse_from_rfc3339(value.trim())
                .context("quota reset timestamp must be RFC3339 or Unix time")?
                .timestamp_millis()
        }
    } else {
        timestamp_to_ms(strict_i64(value)?)?
    };
    Ok(Some(validate_reset_ms(reset, observed_at_ms)?))
}

fn timestamp_to_ms(value: i64) -> anyhow::Result<i64> {
    if value <= 0 {
        bail!("quota reset timestamp must be positive");
    }
    if value < 1_000_000_000_000 {
        value
            .checked_mul(1_000)
            .context("quota reset timestamp overflow")
    } else {
        Ok(value)
    }
}

fn validate_reset_ms(value: i64, observed_at_ms: i64) -> anyhow::Result<i64> {
    let earliest = observed_at_ms.saturating_sub(MAX_RESET_PAST_MS);
    let latest = observed_at_ms.saturating_add(MAX_RESET_FUTURE_MS);
    if !(earliest..=latest).contains(&value) {
        bail!("quota reset timestamp is outside the accepted window");
    }
    DateTime::<Utc>::from_timestamp_millis(value).context("quota reset timestamp is invalid")?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn contract(app: AppKind) -> CodingPlanProfileSpec {
        let (protocol, route, path, stream, terminal, envelope) = match app {
            AppKind::Claude => (
                UpstreamProtocol::AnthropicMessages,
                CodingPlanRoute::ClaudeMessages,
                "/coding/v1/messages",
                CodingPlanStreamFormat::AnthropicSse,
                "message_stop",
                CodingPlanErrorEnvelope::Anthropic,
            ),
            AppKind::Codex => (
                UpstreamProtocol::OpenAiChat,
                CodingPlanRoute::CodexResponses,
                "/coding/v1/chat/completions",
                CodingPlanStreamFormat::OpenAiChatSse,
                "[DONE]",
                CodingPlanErrorEnvelope::OpenAi,
            ),
            AppKind::Gemini => unreachable!(),
        };
        CodingPlanProfileSpec {
            contract_revision: 1,
            inference: CodingPlanInferenceSpec {
                fixed_origin: "https://api.kimi.com".to_string(),
                protocol,
                credential_slot: "/settingsConfig/apiKey".to_string(),
                auth_scheme: AuthScheme::Bearer,
            },
            routes: vec![CodingPlanRouteSpec {
                route,
                path: path.to_string(),
            }],
            models: vec![CodingPlanModelSpec {
                id: "kimi-for-coding".to_string(),
                display_name: "Kimi For Coding".to_string(),
                context_window: 262_144,
                input_modalities: vec![CodingPlanModality::Text],
            }],
            quota: CodingPlanQuotaSpec {
                adapter: CodingPlanQuotaAdapter::Kimi,
                endpoint: Some("https://api.kimi.com/coding/v1/usages".to_string()),
                credential_slots: vec![CodingPlanQuotaCredentialSlot {
                    role: CodingPlanQuotaCredentialRole::InferenceCredential,
                    slot: "/settingsConfig/apiKey".to_string(),
                }],
                cache_ttl_ms: 60_000,
                stale_ttl_ms: 900_000,
            },
            cache_tokens: CodingPlanCacheTokenSpec::InputIncludesCached,
            stream: CodingPlanStreamSpec {
                format: stream,
                terminal_event: terminal.to_string(),
                error_before_terminal_is_fatal: true,
            },
            error: CodingPlanErrorSpec {
                envelope,
                retry_same_credential_once_on_401: false,
                retry_after_commit: false,
            },
            pricing: CodingPlanPricingSpec {
                evidence: CodingPlanPricingEvidence::FlatRateSubscriptionNoUsd,
                source: "reviewed provider preset".to_string(),
                captured_at: "2026-08-13T00:00:00Z".to_string(),
            },
        }
    }

    #[test]
    fn contract_compiles_exact_routes_and_rejects_drift() {
        let runtime = compile_profile_contract(AppKind::Codex, &contract(AppKind::Codex)).unwrap();
        assert_eq!(
            runtime
                .endpoint_for_route(CodingPlanRoute::CodexResponses)
                .unwrap(),
            "https://api.kimi.com/coding/v1/chat/completions"
        );
        runtime
            .guard_final_endpoint(
                CodingPlanRoute::CodexResponses,
                "https://api.kimi.com/coding/v1/chat/completions",
            )
            .unwrap();
        assert!(runtime
            .guard_final_endpoint(
                CodingPlanRoute::CodexResponses,
                "https://api.kimi.com/v1/chat/completions",
            )
            .is_err());
        assert!(runtime
            .guard_final_endpoint(
                CodingPlanRoute::CodexResponses,
                "https://api.kimi.com/coding/v1/chat/completions?key=leak",
            )
            .is_err());
    }

    #[test]
    fn contract_allows_distinct_downstream_routes_to_share_an_upstream_path() {
        let mut value = contract(AppKind::Codex);
        value.routes.push(CodingPlanRouteSpec {
            route: CodingPlanRoute::CodexChatCompletions,
            path: "/coding/v1/chat/completions".to_string(),
        });

        let runtime = compile_profile_contract(AppKind::Codex, &value).unwrap();
        assert_eq!(
            runtime
                .endpoint_for_route(CodingPlanRoute::CodexResponses)
                .unwrap(),
            runtime
                .endpoint_for_route(CodingPlanRoute::CodexChatCompletions)
                .unwrap()
        );
    }

    #[test]
    fn contract_normalizes_ipv6_origins_without_losing_brackets() {
        let mut value = contract(AppKind::Claude);
        value.inference.fixed_origin = "https://[2001:db8::1]:8443/".to_string();
        value.quota.endpoint = Some("https://[2001:db8::1]:8443/quota".to_string());

        let runtime = compile_profile_contract(AppKind::Claude, &value).unwrap();
        assert_eq!(runtime.fixed_origin, "https://[2001:db8::1]:8443");
        assert_eq!(
            runtime
                .endpoint_for_route(CodingPlanRoute::ClaudeMessages)
                .unwrap(),
            "https://[2001:db8::1]:8443/coding/v1/messages"
        );
    }

    #[test]
    fn contract_rejects_cross_app_routes_and_auxiliary_inference_slots() {
        let mut value = contract(AppKind::Claude);
        value.routes[0].route = CodingPlanRoute::CodexResponses;
        assert!(validate_profile_contract(AppKind::Claude, &value).is_err());

        let mut volc = contract(AppKind::Codex);
        volc.quota = CodingPlanQuotaSpec {
            adapter: CodingPlanQuotaAdapter::Volcengine,
            endpoint: Some("https://open.volcengineapi.com/".to_string()),
            credential_slots: vec![
                CodingPlanQuotaCredentialSlot {
                    role: CodingPlanQuotaCredentialRole::AccessKeyId,
                    slot: "/settingsConfig/apiKey".to_string(),
                },
                CodingPlanQuotaCredentialSlot {
                    role: CodingPlanQuotaCredentialRole::SecretAccessKey,
                    slot: "/settingsConfig/env/VOLC_SECRET_ACCESS_KEY".to_string(),
                },
            ],
            cache_ttl_ms: 60_000,
            stale_ttl_ms: 900_000,
        };
        assert!(validate_profile_contract(AppKind::Codex, &volc).is_err());
    }

    #[test]
    fn kimi_parser_maps_detail_to_five_hour_and_usage_to_weekly() {
        let view = parse_kimi_quota(
            &json!({
                "usage": {"limit":"100", "used":"20", "remaining":"80", "resetTime": 1_800_100_000},
                "limits": [{"detail":{"limit":"50", "remaining":"10", "resetTime": 1_800_010_000}}]
            }),
            NOW,
        )
        .unwrap();
        assert_eq!(view.state, CodingPlanQuotaState::Supported);
        assert_eq!(view.windows.len(), 2);
        assert_eq!(view.windows[0].kind, CodingPlanQuotaWindowKind::FiveHour);
        assert_eq!(view.windows[0].utilization, 80.0);
        assert_eq!(view.windows[1].kind, CodingPlanQuotaWindowKind::Weekly);
        assert_eq!(view.windows[1].utilization, 20.0);
    }

    #[test]
    fn kimi_parser_preserves_scoped_windows_and_maps_membership_plan() {
        let view = parse_kimi_quota(
            &json!({
                "user":{"membership":{"level":"LEVEL_ADVANCED"}},
                "five_hour":{"utilization":25,"resets_at":1_800_010_000_000_i64},
                "seven_day":{"utilization":40,"resets_at":1_800_020_000_000_i64},
                "seven_day_kimi_k3":{"utilization":65,"resets_at":1_800_030_000_000_i64}
            }),
            NOW,
        )
        .unwrap();

        assert_eq!(view.plan.as_deref(), Some("Allegro"));
        assert_eq!(view.windows.len(), 3);
        assert_eq!(view.windows[0].kind, CodingPlanQuotaWindowKind::FiveHour);
        assert_eq!(view.windows[0].scope, None);
        assert_eq!(view.windows[0].utilization, 25.0);
        assert_eq!(view.windows[1].kind, CodingPlanQuotaWindowKind::Weekly);
        assert_eq!(view.windows[1].scope, None);
        assert_eq!(view.windows[2].kind, CodingPlanQuotaWindowKind::Weekly);
        assert_eq!(view.windows[2].scope.as_deref(), Some("kimi_k3"));
        assert_eq!(view.windows[2].utilization, 65.0);
    }

    #[test]
    fn kimi_primary_windows_win_over_compatibility_fallbacks() {
        let view = parse_kimi_quota(
            &json!({
                "usage":{"limit":100,"used":20,"remaining":80},
                "limits":[{"detail":{"limit":50,"remaining":10}}],
                "five_hour":{"utilization":5},
                "seven_day":{"utilization":6}
            }),
            NOW,
        )
        .unwrap();

        assert_eq!(view.windows.len(), 2);
        assert_eq!(view.windows[0].utilization, 80.0);
        assert_eq!(view.windows[1].utilization, 20.0);
    }

    #[test]
    fn zhipu_parser_uses_unit_not_reset_order() {
        let view = parse_zhipu_quota(
            &json!({
                "success": true,
                "data": {"level":"pro", "limits":[
                    {"type":"TOKENS_LIMIT", "unit":"6", "percentage":"41.5", "nextResetTime": 1_800_010_000_000_i64},
                    {"type":"TOKENS_LIMIT", "unit":3, "percentage":12, "nextResetTime": 1_800_200_000_000_i64}
                ]}
            }),
            NOW,
        )
        .unwrap();
        assert_eq!(view.windows[0].kind, CodingPlanQuotaWindowKind::FiveHour);
        assert_eq!(view.windows[0].utilization, 12.0);
        assert_eq!(view.windows[1].kind, CodingPlanQuotaWindowKind::Weekly);
        assert_eq!(view.windows[1].utilization, 41.5);
        assert_eq!(view.plan.as_deref(), Some("pro"));
    }

    #[test]
    fn zhipu_parser_accepts_observed_day_unit_weekly_variant() {
        let view = parse_zhipu_quota(
            &json!({
                "success":true,
                "data":{"limits":[
                    {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":10},
                    {"type":"TOKENS_LIMIT","unit":4,"number":7,"percentage":55}
                ]}
            }),
            NOW,
        )
        .unwrap();

        assert_eq!(view.windows.len(), 2);
        assert_eq!(view.windows[0].kind, CodingPlanQuotaWindowKind::FiveHour);
        assert_eq!(view.windows[1].kind, CodingPlanQuotaWindowKind::Weekly);
        assert_eq!(view.windows[1].utilization, 55.0);
    }

    #[test]
    fn zhipu_parser_classifies_legacy_windows_without_unit_by_reset_order() {
        let view = parse_zhipu_quota(
            &json!({
                "data":{"limits":[
                    {"type":"TOKENS_LIMIT","percentage":70,"nextResetTime":1_800_200_000_000_i64},
                    {"type":"TOKENS_LIMIT","percentage":20,"nextResetTime":1_800_010_000_000_i64}
                ]}
            }),
            NOW,
        )
        .unwrap();

        assert_eq!(view.windows[0].kind, CodingPlanQuotaWindowKind::FiveHour);
        assert_eq!(view.windows[0].utilization, 20.0);
        assert_eq!(view.windows[1].kind, CodingPlanQuotaWindowKind::Weekly);
        assert_eq!(view.windows[1].utilization, 70.0);
        assert!(parse_zhipu_quota(
            &json!({"data":{"limits":[
                {"type":"TOKENS_LIMIT","unit":9,"percentage":10}
            ]}}),
            NOW,
        )
        .is_err());
    }

    #[test]
    fn minimax_parser_inverts_percent_and_accepts_count_variant() {
        let percent = parse_minimax_quota(
            &json!({
                "base_resp":{"status_code":0},
                "model_remains":[{
                    "model_name":"general",
                    "current_interval_remaining_percent":"75.5",
                    "end_time":1_800_010_000_000_i64,
                    "current_weekly_status":"1",
                    "current_weekly_remaining_percent":40,
                    "weekly_end_time":1_800_020_000_000_i64
                }, {"model_name":"video", "current_interval_remaining_percent":0}]
            }),
            NOW,
        )
        .unwrap();
        assert_eq!(percent.windows[0].utilization, 24.5);
        assert_eq!(percent.windows[1].utilization, 60.0);

        let counts = parse_minimax_quota(
            &json!({"model_remains":[{
                "model_name":"general",
                "current_interval_total_count":"1000",
                "current_interval_usage_count":"250",
                "current_weekly_status":3
            }]}),
            NOW,
        )
        .unwrap();
        assert_eq!(counts.windows.len(), 1);
        assert_eq!(counts.windows[0].utilization, 75.0);
    }

    #[test]
    fn parsers_never_default_missing_values_to_zero() {
        assert!(parse_kimi_quota(&json!({"usage":{"limit":100}}), NOW).is_err());
        assert!(parse_zhipu_quota(
            &json!({"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3}]}}),
            NOW
        )
        .is_err());
        assert!(
            parse_minimax_quota(&json!({"model_remains":[{"model_name":"general"}]}), NOW).is_err()
        );
    }

    #[test]
    fn volcengine_parsers_validate_windows_and_values() {
        let afp = parse_volcengine_afp_quota(
            &json!({"Result":{
                "PlanType":"Large",
                "AFPFiveHour":{"Quota":"50", "Used":"12.5", "ResetTime":1_800_010_000_000_i64},
                "AFPWeekly":{"Quota":500, "Used":150}
            }}),
            NOW,
        )
        .unwrap();
        assert_eq!(afp.windows[0].utilization, 25.0);
        assert_eq!(afp.windows[1].utilization, 30.0);

        let coding = parse_volcengine_coding_quota(
            &json!({"Result":{"QuotaUsage":[
                {"Level":"session", "Percent":"0", "ResetTimestamp":-1},
                {"Level":"weekly", "Percent":"1.5", "ResetTimestamp":1_800_010_000}
            ]}}),
            NOW,
        )
        .unwrap();
        assert_eq!(coding.windows.len(), 2);
        assert!(coding.windows[0].resets_at_ms.is_none());
        assert!(parse_volcengine_afp_quota(
            &json!({"Result":{"AFPFiveHour":{"Quota":10,"Used":11}}}),
            NOW
        )
        .is_err());
    }

    #[test]
    fn unavailable_and_unknown_are_distinct_public_states() {
        assert_eq!(
            CodingPlanQuotaView::unavailable("no authoritative API").state,
            CodingPlanQuotaState::Unavailable
        );
        assert_eq!(
            CodingPlanQuotaView::unknown("not fetched").state,
            CodingPlanQuotaState::Unknown
        );
    }

    fn cache_key(
        provider_revision: u64,
        credential_generation: u64,
        runtime_fingerprint: &str,
    ) -> CodingPlanQuotaCacheKey {
        CodingPlanQuotaCacheKey {
            provider_key: ProviderKey::new(AppKind::Codex, "coding-provider").unwrap(),
            provider_revision,
            credential_generation,
            runtime_fingerprint: runtime_fingerprint.to_string(),
        }
    }

    fn supported_quota(observed_at_ms: i64) -> CodingPlanQuotaView {
        CodingPlanQuotaView {
            state: CodingPlanQuotaState::Supported,
            windows: vec![CodingPlanQuotaWindow {
                kind: CodingPlanQuotaWindowKind::FiveHour,
                scope: None,
                utilization: 25.0,
                resets_at_ms: None,
                used: Some(25.0),
                limit: Some(100.0),
                unit: Some("requests".to_string()),
            }],
            plan: Some("Coding Plan".to_string()),
            observed_at_ms: Some(observed_at_ms),
            stale_since_ms: None,
            reason: None,
        }
    }

    #[test]
    fn quota_cache_is_exact_across_revision_generation_and_runtime() {
        let mut cache = CodingPlanQuotaCache::default();
        let original = cache_key(4, 7, "runtime-a");
        cache
            .insert_supported(original.clone(), supported_quota(NOW))
            .unwrap();

        assert!(cache.fresh(&original, NOW + 30_000, 60_000).is_some());
        assert!(cache
            .fresh(&cache_key(5, 7, "runtime-a"), NOW + 30_000, 60_000)
            .is_none());
        assert!(cache
            .fresh(&cache_key(4, 8, "runtime-a"), NOW + 30_000, 60_000)
            .is_none());
        assert!(cache
            .fresh(&cache_key(4, 7, "runtime-b"), NOW + 30_000, 60_000)
            .is_none());

        let replacement = cache_key(5, 8, "runtime-b");
        cache
            .insert_supported(replacement.clone(), supported_quota(NOW + 1))
            .unwrap();
        assert_eq!(cache.len(), 1);
        assert!(cache.fresh(&original, NOW + 2, 60_000).is_none());
        assert!(cache.fresh(&replacement, NOW + 2, 60_000).is_some());
    }

    #[test]
    fn quota_cache_exposes_stale_only_inside_the_exact_stale_window() {
        let mut cache = CodingPlanQuotaCache::default();
        let key = cache_key(1, 1, "runtime");
        cache
            .insert_supported(key.clone(), supported_quota(NOW))
            .unwrap();

        assert!(cache.fresh(&key, NOW + 60_001, 60_000).is_none());
        let stale = cache
            .stale(
                &key,
                NOW + 60_001,
                60_000,
                900_000,
                "quota refresh temporarily unavailable",
            )
            .unwrap();
        assert_eq!(stale.state, CodingPlanQuotaState::Stale);
        assert_eq!(stale.stale_since_ms, Some(NOW + 60_000));
        assert_eq!(
            stale.reason.as_deref(),
            Some("quota refresh temporarily unavailable")
        );
        assert!(cache
            .stale(&key, NOW + 900_001, 60_000, 900_000, "transient")
            .is_none());
        assert!(cache.fresh(&key, NOW - 1, 60_000).is_none());
    }

    #[test]
    fn volcengine_afp_active_window_detection_is_strict() {
        assert!(!volcengine_afp_has_active_window(&json!({"Result": {}})).unwrap());
        assert!(
            !volcengine_afp_has_active_window(&json!({"Result":{"AFPFiveHour":{"Quota":0}}}))
                .unwrap()
        );
        assert!(
            volcengine_afp_has_active_window(&json!({"Result":{"AFPWeekly":{"Quota":"100"}}}))
                .unwrap()
        );
        assert!(volcengine_afp_has_active_window(&json!({"Result":{"AFPFiveHour":{}}})).is_err());
    }
}
