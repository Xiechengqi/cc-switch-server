use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::OnceLock;

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use zeroize::Zeroizing;

use super::registry::ProviderKey;

pub const WEB_SESSION_REGISTRY_FORMAT: &str = "cc-switch-web-session-registry";
pub const WEB_SESSION_REGISTRY_SCHEMA_VERSION: u32 = 1;
pub const WEB_SESSION_CREDENTIAL_SLOT: &str = "/settingsConfig/webSession/cookie";
pub const GROK_WEB_SESSION_DRIVER_ID: &str = "special.grok_web_session";
pub const PERPLEXITY_WEB_SESSION_DRIVER_ID: &str = "special.perplexity_web_session";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSessionRegistry {
    pub format: String,
    pub schema_version: u32,
    pub rail: WebSessionRailSpec,
    pub profiles: Vec<WebSessionProfileSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSessionRailSpec {
    pub id: String,
    pub credential_slot: String,
    pub credential_ownership: WebSessionCredentialOwnership,
    pub max_credential_bytes: usize,
    pub max_cookie_pairs: usize,
    pub redirect_policy: WebSessionRedirectPolicy,
    pub cookie_jar_policy: WebSessionCookieJarPolicy,
    pub cross_origin_policy: WebSessionCrossOriginPolicy,
    pub downstream_set_cookie_policy: WebSessionSetCookiePolicy,
    pub auth_recovery: WebSessionAuthRecovery,
    pub account_binding_allowed: bool,
    pub api_key_slot_allowed: bool,
    pub extra_headers_allowed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionCredentialOwnership {
    ProviderOwned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionRedirectPolicy {
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionCookieJarPolicy {
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionCrossOriginPolicy {
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionSetCookiePolicy {
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionAuthRecovery {
    ExplicitReimportOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSessionProfileSpec {
    pub profile_id: String,
    pub provider_id: String,
    pub label: String,
    pub fixed_origin: String,
    pub method: WebSessionMethod,
    pub path: String,
    pub cookie_rules: Vec<WebSessionCookieRule>,
    pub required_cookie_families: Vec<String>,
    pub csrf_policy: WebSessionCsrfPolicy,
    pub session_refresh_policy: WebSessionRefreshPolicy,
    pub terminal: WebSessionTerminalSpec,
    pub request_body_limit_bytes: usize,
    pub response_body_limit_bytes: usize,
    pub visibility: WebSessionVisibility,
    pub maturity: WebSessionMaturity,
    pub risk: WebSessionRisk,
    pub implementation_state: WebSessionImplementationState,
    pub fixture_state: WebSessionFixtureState,
    pub live_state: WebSessionLiveState,
    pub evidence_refs: Vec<String>,
}

impl WebSessionProfileSpec {
    pub fn endpoint(&self) -> anyhow::Result<Url> {
        exact_endpoint(&self.fixed_origin, &self.path)
    }

    fn cookie_rule<'a>(&'a self, name: &str) -> Option<&'a WebSessionCookieRule> {
        self.cookie_rules.iter().find(|rule| rule.allows_name(name))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum WebSessionMethod {
    Post,
}

impl WebSessionMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WebSessionCookieRule {
    Exact {
        name: String,
        family: String,
    },
    NumericSuffix {
        prefix: String,
        family: String,
        max_index: u8,
    },
}

impl WebSessionCookieRule {
    fn family(&self) -> &str {
        match self {
            Self::Exact { family, .. } | Self::NumericSuffix { family, .. } => family,
        }
    }

    fn allows_name(&self, candidate: &str) -> bool {
        match self {
            Self::Exact { name, .. } => candidate == name,
            Self::NumericSuffix {
                prefix, max_index, ..
            } => candidate
                .strip_prefix(prefix)
                .filter(|suffix| {
                    !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
                })
                .and_then(|suffix| suffix.parse::<u8>().ok())
                .is_some_and(|index| index <= *max_index),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionCsrfPolicy {
    NoneObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionRefreshPolicy {
    ExplicitReimportOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSessionTerminalSpec {
    pub format: WebSessionStreamFormat,
    pub upstream_terminal: String,
    pub downstream_terminal: String,
    pub eof_without_terminal_is_error: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionStreamFormat {
    Ndjson,
    Sse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionVisibility {
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionMaturity {
    Experimental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionRisk {
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionImplementationState {
    FrameworkOnly,
    Implemented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionFixtureState {
    FixtureVerified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebSessionLiveState {
    LivePending,
}

static REGISTRY: OnceLock<WebSessionRegistry> = OnceLock::new();

pub fn validate_embedded_registry() -> anyhow::Result<()> {
    let registry: WebSessionRegistry = serde_json::from_str(include_str!(
        "../../../assets/contract/web-session-registry.json"
    ))
    .context("embedded Web Session registry must decode")?;
    validate_registry(&registry).context("embedded Web Session registry must be valid")
}

pub fn web_session_registry() -> &'static WebSessionRegistry {
    REGISTRY.get_or_init(|| {
        let registry: WebSessionRegistry = serde_json::from_str(include_str!(
            "../../../assets/contract/web-session-registry.json"
        ))
        .expect("embedded Web Session registry must decode");
        validate_registry(&registry).expect("embedded Web Session registry must be valid");
        registry
    })
}

pub fn web_session_profile(profile_id: &str) -> Option<&'static WebSessionProfileSpec> {
    web_session_registry()
        .profiles
        .iter()
        .find(|profile| profile.profile_id == profile_id)
}

pub fn web_session_profile_for_driver(driver_id: &str) -> Option<&'static WebSessionProfileSpec> {
    let profile_id = match driver_id {
        GROK_WEB_SESSION_DRIVER_ID => "web_session.grok_web",
        PERPLEXITY_WEB_SESSION_DRIVER_ID => "web_session.perplexity_web",
        _ => return None,
    };
    web_session_profile(profile_id)
}

pub fn validate_registry(registry: &WebSessionRegistry) -> anyhow::Result<()> {
    if registry.format != WEB_SESSION_REGISTRY_FORMAT
        || registry.schema_version != WEB_SESSION_REGISTRY_SCHEMA_VERSION
    {
        bail!("unsupported Web Session registry format or schema");
    }
    let rail = &registry.rail;
    if rail.id != "web_session"
        || rail.credential_slot != WEB_SESSION_CREDENTIAL_SLOT
        || rail.credential_ownership != WebSessionCredentialOwnership::ProviderOwned
        || rail.max_credential_bytes == 0
        || rail.max_credential_bytes > 64 * 1024
        || rail.max_cookie_pairs == 0
        || rail.max_cookie_pairs > 64
        || rail.redirect_policy != WebSessionRedirectPolicy::Disabled
        || rail.cookie_jar_policy != WebSessionCookieJarPolicy::Disabled
        || rail.cross_origin_policy != WebSessionCrossOriginPolicy::Disabled
        || rail.downstream_set_cookie_policy != WebSessionSetCookiePolicy::Drop
        || rail.auth_recovery != WebSessionAuthRecovery::ExplicitReimportOnly
        || rail.account_binding_allowed
        || rail.api_key_slot_allowed
        || rail.extra_headers_allowed
    {
        bail!("Web Session credential rail weakens a fail-closed invariant");
    }
    if registry.profiles.is_empty() || registry.profiles.len() > 16 {
        bail!("Web Session registry must contain a bounded reviewed profile set");
    }
    let mut profile_ids = BTreeSet::new();
    let mut provider_ids = BTreeSet::new();
    for profile in &registry.profiles {
        validate_profile(profile)?;
        if !profile_ids.insert(profile.profile_id.as_str())
            || !provider_ids.insert(profile.provider_id.as_str())
        {
            bail!("Web Session registry repeats a profile or provider id");
        }
    }
    Ok(())
}

fn validate_profile(profile: &WebSessionProfileSpec) -> anyhow::Result<()> {
    if !profile.profile_id.starts_with("web_session.")
        || profile.provider_id.trim().is_empty()
        || profile.provider_id != profile.provider_id.trim()
        || profile.label.trim().is_empty()
        || profile.label != profile.label.trim()
    {
        bail!("Web Session profile identity is invalid");
    }
    profile.endpoint()?;
    if profile.cookie_rules.is_empty() || profile.cookie_rules.len() > 16 {
        bail!("Web Session profile must declare a bounded Cookie allowlist");
    }
    let mut exact_names = BTreeSet::new();
    let mut families = BTreeSet::new();
    for rule in &profile.cookie_rules {
        let family = rule.family();
        if family.trim().is_empty() || family != family.trim() {
            bail!("Web Session Cookie rule has an invalid family");
        }
        families.insert(family);
        match rule {
            WebSessionCookieRule::Exact { name, .. } => {
                validate_cookie_name(name)?;
                if !exact_names.insert(name.as_str()) {
                    bail!("Web Session Cookie allowlist repeats a name");
                }
            }
            WebSessionCookieRule::NumericSuffix {
                prefix, max_index, ..
            } => {
                if !prefix.ends_with('.') || *max_index > 31 {
                    bail!("Web Session numeric Cookie rule is invalid");
                }
                validate_cookie_name(prefix.trim_end_matches('.'))?;
            }
        }
    }
    let required = profile
        .required_cookie_families
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if required.is_empty()
        || required.len() != profile.required_cookie_families.len()
        || !required.is_subset(&families)
    {
        bail!("Web Session required Cookie families are invalid");
    }
    if profile.request_body_limit_bytes == 0
        || profile.request_body_limit_bytes > 4 * 1024 * 1024
        || profile.response_body_limit_bytes < profile.request_body_limit_bytes
        || profile.response_body_limit_bytes > 32 * 1024 * 1024
        || profile.terminal.upstream_terminal.trim().is_empty()
        || profile.terminal.downstream_terminal.trim().is_empty()
        || !profile.terminal.eof_without_terminal_is_error
        || profile.visibility != WebSessionVisibility::Hidden
        || profile.maturity != WebSessionMaturity::Experimental
        || profile.risk != WebSessionRisk::High
        || profile.implementation_state != WebSessionImplementationState::Implemented
        || profile.fixture_state != WebSessionFixtureState::FixtureVerified
        || profile.live_state != WebSessionLiveState::LivePending
        || profile.evidence_refs.len() < 2
    {
        bail!("Web Session profile overstates maturity or has an incomplete contract");
    }
    Ok(())
}

fn exact_endpoint(origin: &str, path: &str) -> anyhow::Result<Url> {
    let origin = Url::parse(origin).context("parse Web Session origin")?;
    if origin.scheme() != "https"
        || origin.cannot_be_a_base()
        || origin.username() != ""
        || origin.password().is_some()
        || origin.query().is_some()
        || origin.fragment().is_some()
        || origin.path() != "/"
    {
        bail!("Web Session origin must be a fixed HTTPS origin");
    }
    if !path.starts_with('/') || path.starts_with("//") || path.contains('?') || path.contains('#')
    {
        bail!("Web Session path must be exact");
    }
    let mut endpoint = origin;
    endpoint.set_path(path);
    Ok(endpoint)
}

pub fn guard_exact_request(
    profile: &WebSessionProfileSpec,
    method: WebSessionMethod,
    candidate: &str,
) -> anyhow::Result<()> {
    if method != profile.method {
        bail!("Web Session request method does not match the reviewed profile");
    }
    let expected = profile.endpoint()?;
    let candidate = Url::parse(candidate).context("parse Web Session candidate endpoint")?;
    if candidate != expected {
        bail!("Web Session request cannot change origin, path, query, or fragment");
    }
    Ok(())
}

pub fn response_header_is_forwardable(name: &str) -> bool {
    !matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "set-cookie" | "set-cookie2" | "location" | "refresh"
    )
}

pub struct ParsedWebSessionCredential {
    cookie_header: Zeroizing<String>,
    digest: String,
    cookie_names: Vec<String>,
}

impl fmt::Debug for ParsedWebSessionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ParsedWebSessionCredential")
            .field("configured", &true)
            .field("digest", &self.digest)
            .field("cookie_names", &self.cookie_names)
            .finish()
    }
}

impl ParsedWebSessionCredential {
    pub fn parse(profile: &WebSessionProfileSpec, raw: &str) -> anyhow::Result<Self> {
        let rail = &web_session_registry().rail;
        if raw.is_empty() || raw.len() > rail.max_credential_bytes {
            bail!("Web Session credential is empty or exceeds the size limit");
        }
        if raw.bytes().any(|byte| byte.is_ascii_control()) {
            bail!("Web Session credential contains a control character");
        }
        let mut input = raw.trim();
        if input.len() >= 7 && input[..7].eq_ignore_ascii_case("cookie:") {
            input = input[7..].trim();
        }
        let lower = input.to_ascii_lowercase();
        if lower.starts_with("bearer ")
            || lower.starts_with("authorization:")
            || lower.starts_with("set-cookie:")
            || input.starts_with('{')
            || input.starts_with('[')
        {
            bail!("Web Session rail accepts only a Cookie header or Cookie pairs");
        }

        let mut pairs = BTreeMap::<String, String>::new();
        let mut observed_families = BTreeSet::new();
        for part in input.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if pairs.len() >= rail.max_cookie_pairs {
                bail!("Web Session credential contains too many Cookie pairs");
            }
            let (name, value) = part
                .split_once('=')
                .context("Web Session credential contains a malformed Cookie pair")?;
            let name = name.trim();
            let value = value.trim();
            validate_cookie_name(name)?;
            if value.is_empty()
                || value
                    .bytes()
                    .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
            {
                bail!("Web Session Cookie value is empty or invalid");
            }
            let rule = profile
                .cookie_rule(name)
                .with_context(|| format!("Cookie {name} is not allowlisted for this profile"))?;
            if pairs.insert(name.to_string(), value.to_string()).is_some() {
                bail!("Web Session credential repeats Cookie {name}");
            }
            observed_families.insert(rule.family());
        }
        if pairs.is_empty() {
            bail!("Web Session credential contains no Cookie pairs");
        }
        for family in &profile.required_cookie_families {
            if !observed_families.contains(family.as_str()) {
                bail!("Web Session credential is missing required Cookie family {family}");
            }
        }

        let mut canonical_pairs = pairs.iter().collect::<Vec<_>>();
        canonical_pairs.sort_by(|(left, _), (right, _)| {
            cookie_name_order(profile, left).cmp(&cookie_name_order(profile, right))
        });
        let cookie_header = canonical_pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-web-session-credential-v1\0");
        digest.update(profile.profile_id.as_bytes());
        digest.update(b"\0");
        digest.update(cookie_header.as_bytes());
        let digest = hex::encode(digest.finalize());
        Ok(Self {
            cookie_header: Zeroizing::new(cookie_header),
            digest: digest[..24].to_string(),
            cookie_names: canonical_pairs
                .into_iter()
                .map(|(name, _)| name.clone())
                .collect(),
        })
    }

    pub fn cookie_header(&self) -> &str {
        self.cookie_header.as_str()
    }

    pub fn summary(&self, credential_generation: u64) -> WebSessionCredentialSummary {
        WebSessionCredentialSummary {
            configured: true,
            digest: self.digest.clone(),
            credential_generation,
            cookie_names: self.cookie_names.clone(),
        }
    }
}

fn cookie_name_order<'a>(
    profile: &'a WebSessionProfileSpec,
    name: &'a str,
) -> (usize, u16, &'a str) {
    for (rule_index, rule) in profile.cookie_rules.iter().enumerate() {
        match rule {
            WebSessionCookieRule::Exact { name: exact, .. } if name == exact => {
                return (rule_index, 0, name);
            }
            WebSessionCookieRule::NumericSuffix { prefix, .. } => {
                if let Some(index) = name
                    .strip_prefix(prefix)
                    .and_then(|value| value.parse::<u16>().ok())
                {
                    return (rule_index, index, name);
                }
            }
            _ => {}
        }
    }
    (usize::MAX, u16::MAX, name)
}

fn validate_cookie_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty()
        || name.len() > 128
        || name.bytes().any(|byte| {
            !(byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'..=b'+' | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                ))
        })
    {
        bail!("Web Session Cookie name is invalid");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WebSessionCredentialSummary {
    pub configured: bool,
    pub digest: String,
    pub credential_generation: u64,
    pub cookie_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WebSessionScope {
    pub provider_key: ProviderKey,
    pub provider_revision: u64,
    pub runtime_fingerprint: String,
    pub credential_generation: u64,
    pub profile_id: String,
    pub fixed_origin: String,
}

impl WebSessionScope {
    pub fn validate(&self) -> anyhow::Result<()> {
        let profile = web_session_profile(&self.profile_id)
            .with_context(|| format!("unknown Web Session profile {}", self.profile_id))?;
        if self.provider_revision == 0
            || self.credential_generation == 0
            || self.runtime_fingerprint.trim().is_empty()
            || self.runtime_fingerprint != self.runtime_fingerprint.trim()
            || self.fixed_origin != profile.fixed_origin
        {
            bail!("Web Session scope is incomplete or does not match its profile");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct WebSessionTaskKey {
    pub scope: WebSessionScope,
    pub task_kind: String,
    pub upstream_task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSessionStateRecord {
    pub session_id: String,
    pub observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSessionTaskRecord {
    pub state: String,
    pub observed_at_ms: i64,
}

#[derive(Debug, Default)]
pub struct WebSessionRuntimeStore {
    sessions: BTreeMap<WebSessionScope, WebSessionStateRecord>,
    tasks: BTreeMap<WebSessionTaskKey, WebSessionTaskRecord>,
    invalidated: BTreeSet<WebSessionScope>,
}

impl WebSessionRuntimeStore {
    pub fn insert_session(
        &mut self,
        scope: WebSessionScope,
        record: WebSessionStateRecord,
    ) -> anyhow::Result<()> {
        scope.validate()?;
        self.retain_current(&scope);
        self.invalidated.remove(&scope);
        self.sessions.insert(scope, record);
        Ok(())
    }

    pub fn session(&self, scope: &WebSessionScope) -> Option<&WebSessionStateRecord> {
        (!self.invalidated.contains(scope))
            .then(|| self.sessions.get(scope))
            .flatten()
    }

    pub fn insert_task(
        &mut self,
        key: WebSessionTaskKey,
        record: WebSessionTaskRecord,
    ) -> anyhow::Result<()> {
        key.scope.validate()?;
        if key.task_kind.trim().is_empty()
            || key.upstream_task_id.trim().is_empty()
            || self.invalidated.contains(&key.scope)
        {
            bail!("Web Session task scope is invalid or requires explicit re-import");
        }
        self.retain_current(&key.scope);
        self.tasks.insert(key, record);
        Ok(())
    }

    pub fn task(&self, key: &WebSessionTaskKey) -> Option<&WebSessionTaskRecord> {
        (!self.invalidated.contains(&key.scope))
            .then(|| self.tasks.get(key))
            .flatten()
    }

    pub fn invalidate_authentication(&mut self, scope: &WebSessionScope) {
        self.sessions.remove(scope);
        self.tasks.retain(|key, _| &key.scope != scope);
        self.invalidated.insert(scope.clone());
    }

    pub fn requires_explicit_reimport(&self, scope: &WebSessionScope) -> bool {
        self.invalidated.contains(scope)
    }

    pub fn retain_current(&mut self, scope: &WebSessionScope) {
        let provider_key = &scope.provider_key;
        self.sessions
            .retain(|candidate, _| &candidate.provider_key != provider_key || candidate == scope);
        self.tasks.retain(|candidate, _| {
            &candidate.scope.provider_key != provider_key || &candidate.scope == scope
        });
        self.invalidated
            .retain(|candidate| &candidate.provider_key != provider_key || candidate == scope);
    }

    pub fn retain_scopes(&mut self, scopes: &BTreeSet<WebSessionScope>) {
        self.sessions.retain(|scope, _| scopes.contains(scope));
        self.tasks.retain(|key, _| scopes.contains(&key.scope));
        self.invalidated.retain(|scope| scopes.contains(scope));
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize, usize) {
        (
            self.sessions.len(),
            self.tasks.len(),
            self.invalidated.len(),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSessionFailureAction {
    InvalidateAndRequireExplicitReimport,
    Terminal,
}

pub fn failure_action(status: u16, _downstream_committed: bool) -> WebSessionFailureAction {
    if matches!(status, 401 | 403) {
        WebSessionFailureAction::InvalidateAndRequireExplicitReimport
    } else {
        WebSessionFailureAction::Terminal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::AppKind;

    fn profile(id: &str) -> &'static WebSessionProfileSpec {
        web_session_profile(id).unwrap()
    }

    fn scope(provider_id: &str, generation: u64) -> WebSessionScope {
        WebSessionScope {
            provider_key: ProviderKey::new(AppKind::Codex, provider_id).unwrap(),
            provider_revision: generation,
            runtime_fingerprint: format!("runtime-{generation}"),
            credential_generation: generation,
            profile_id: "web_session.grok_web".to_string(),
            fixed_origin: "https://grok.com".to_string(),
        }
    }

    #[test]
    fn embedded_registry_is_fail_closed_and_keeps_candidates_hidden() {
        let registry = web_session_registry();
        assert_eq!(registry.profiles.len(), 2);
        assert_eq!(registry.rail.credential_slot, WEB_SESSION_CREDENTIAL_SLOT);
        assert!(!registry.rail.account_binding_allowed);
        assert!(!registry.rail.api_key_slot_allowed);
        assert!(!registry.rail.extra_headers_allowed);
        for profile in &registry.profiles {
            assert_eq!(profile.visibility, WebSessionVisibility::Hidden);
            assert_eq!(profile.maturity, WebSessionMaturity::Experimental);
            assert_eq!(
                profile.implementation_state,
                WebSessionImplementationState::Implemented
            );
            assert_eq!(profile.live_state, WebSessionLiveState::LivePending);
        }
    }

    #[test]
    fn grok_cookie_parser_keeps_only_allowlisted_minimum_and_redacts_debug() {
        let credential = ParsedWebSessionCredential::parse(
            profile("web_session.grok_web"),
            "Cookie: sso-rw=rw-secret; sso=read-secret; cf_clearance=clear-secret",
        )
        .unwrap();
        assert_eq!(
            credential.cookie_header(),
            "sso=read-secret; sso-rw=rw-secret; cf_clearance=clear-secret"
        );
        let summary = credential.summary(7);
        assert!(summary.configured);
        assert_eq!(summary.credential_generation, 7);
        assert_eq!(summary.digest.len(), 24);
        let debug = format!("{credential:?}");
        assert!(!debug.contains("read-secret"));
        assert!(!debug.contains("rw-secret"));
        assert!(!debug.contains("clear-secret"));
    }

    #[test]
    fn perplexity_cookie_parser_accepts_bounded_chunks_but_not_other_cookies() {
        let profile = profile("web_session.perplexity_web");
        let credential = ParsedWebSessionCredential::parse(
            profile,
            "__Secure-next-auth.session-token.1=bbb; __Secure-next-auth.session-token.0=aaa",
        )
        .unwrap();
        assert_eq!(
            credential.cookie_header(),
            "__Secure-next-auth.session-token.0=aaa; __Secure-next-auth.session-token.1=bbb"
        );
        let ordered = ParsedWebSessionCredential::parse(
            profile,
            "__Secure-next-auth.session-token.12=ccc; __Secure-next-auth.session-token.2=bbb; __Secure-next-auth.session-token.0=aaa",
        )
        .unwrap();
        assert_eq!(
            ordered.cookie_header(),
            "__Secure-next-auth.session-token.0=aaa; __Secure-next-auth.session-token.2=bbb; __Secure-next-auth.session-token.12=ccc"
        );
        assert!(ParsedWebSessionCredential::parse(
            profile,
            "__Secure-next-auth.session-token.16=outside-bound"
        )
        .is_err());
        assert!(ParsedWebSessionCredential::parse(
            profile,
            "__Secure-next-auth.session-token=secret; unrelated=leak"
        )
        .is_err());
    }

    #[test]
    fn credential_rail_rejects_bearer_json_headers_controls_and_missing_family() {
        let profile = profile("web_session.grok_web");
        for value in [
            "Bearer token",
            "Authorization: Bearer token",
            "Set-Cookie: sso=token",
            r#"[{"name":"sso","value":"token"}]"#,
            "sso=token\r\nX-Leak: yes",
            "cf_clearance=only-clearance",
            "sso=one; sso=two",
        ] {
            assert!(ParsedWebSessionCredential::parse(profile, value).is_err());
        }
    }

    #[test]
    fn exact_request_guard_rejects_method_origin_path_query_and_fragment_drift() {
        let profile = profile("web_session.grok_web");
        let endpoint = profile.endpoint().unwrap();
        guard_exact_request(profile, WebSessionMethod::Post, endpoint.as_str()).unwrap();
        for candidate in [
            "https://evil.example/rest/app-chat/conversations/new",
            "https://grok.com/rest/app-chat/conversations/new/other",
            "https://grok.com/rest/app-chat/conversations/new?next=https://evil.example",
            "https://grok.com/rest/app-chat/conversations/new#fragment",
            "http://grok.com/rest/app-chat/conversations/new",
        ] {
            assert!(guard_exact_request(profile, WebSessionMethod::Post, candidate).is_err());
        }
    }

    #[test]
    fn response_cookie_redirect_and_refresh_headers_never_reach_downstream() {
        for name in ["Set-Cookie", "set-cookie2", "Location", "Refresh"] {
            assert!(!response_header_is_forwardable(name));
        }
        assert!(response_header_is_forwardable("content-type"));
    }

    #[test]
    fn credential_rotation_prunes_only_the_same_provider_runtime_scope() {
        let mut store = WebSessionRuntimeStore::default();
        let old = scope("web-a", 1);
        let other = scope("web-b", 1);
        for current in [&old, &other] {
            store
                .insert_session(
                    current.clone(),
                    WebSessionStateRecord {
                        session_id: format!("session-{}", current.provider_key.provider_id),
                        observed_at_ms: 1,
                    },
                )
                .unwrap();
            store
                .insert_task(
                    WebSessionTaskKey {
                        scope: current.clone(),
                        task_kind: "completion".to_string(),
                        upstream_task_id: "task-1".to_string(),
                    },
                    WebSessionTaskRecord {
                        state: "pending".to_string(),
                        observed_at_ms: 1,
                    },
                )
                .unwrap();
        }
        let rotated = scope("web-a", 2);
        store
            .insert_session(
                rotated.clone(),
                WebSessionStateRecord {
                    session_id: "session-new".to_string(),
                    observed_at_ms: 2,
                },
            )
            .unwrap();
        assert!(store.session(&old).is_none());
        assert!(store.session(&rotated).is_some());
        assert!(store.session(&other).is_some());
        assert_eq!(store.counts(), (2, 1, 0));
    }

    #[test]
    fn authentication_failure_invalidates_exact_scope_without_retry_or_fallback() {
        let mut store = WebSessionRuntimeStore::default();
        let failed = scope("web-a", 1);
        let other = scope("web-b", 1);
        for current in [&failed, &other] {
            store
                .insert_session(
                    current.clone(),
                    WebSessionStateRecord {
                        session_id: current.provider_key.provider_id.clone(),
                        observed_at_ms: 1,
                    },
                )
                .unwrap();
        }
        store.invalidate_authentication(&failed);
        assert!(store.session(&failed).is_none());
        assert!(store.requires_explicit_reimport(&failed));
        assert!(store.session(&other).is_some());
        for committed in [false, true] {
            assert_eq!(
                failure_action(401, committed),
                WebSessionFailureAction::InvalidateAndRequireExplicitReimport
            );
            assert_eq!(
                failure_action(403, committed),
                WebSessionFailureAction::InvalidateAndRequireExplicitReimport
            );
            assert_eq!(
                failure_action(429, committed),
                WebSessionFailureAction::Terminal
            );
        }
    }

    #[test]
    fn provider_delete_prunes_session_task_and_invalidation_state() {
        let mut store = WebSessionRuntimeStore::default();
        let kept = scope("web-a", 1);
        let deleted = scope("web-b", 1);
        for current in [&kept, &deleted] {
            store
                .insert_session(
                    current.clone(),
                    WebSessionStateRecord {
                        session_id: current.provider_key.provider_id.clone(),
                        observed_at_ms: 1,
                    },
                )
                .unwrap();
        }
        store.invalidate_authentication(&deleted);
        store.retain_scopes(&BTreeSet::from([kept.clone()]));
        assert!(store.session(&kept).is_some());
        assert!(!store.requires_explicit_reimport(&deleted));
        assert_eq!(store.counts(), (1, 0, 0));
    }
}
