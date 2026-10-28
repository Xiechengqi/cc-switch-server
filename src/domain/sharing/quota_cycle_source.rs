//! Reviewed source selection. Display tier names/expiry fields are not quota
//! reset evidence. Unsupported rails fail closed with a stable reason.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    quota_cycle::{QuotaSourceFence, QuotaTimeBasis, QuotaWindowObservation},
    shares::Share,
    subscription_identity::resolve_provider_subscription_identity,
};
use crate::domain::accounts::store::{Account, AccountStore};
use crate::domain::providers::{
    model::ProviderType, runtime::authoritative_managed_account, store::ProviderStore,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountResetCapability {
    pub support: String,
    pub sync_status: String,
    pub reason: Option<String>,
    pub minimum_contract_version: u16,
}

impl AccountResetCapability {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            support: if reason == "profile_unsupported" {
                "unsupported"
            } else {
                "conditional"
            }
            .into(),
            sync_status: "initializing".into(),
            reason: Some(reason.into()),
            minimum_contract_version: 8,
        }
    }
}

pub fn provider_capability(provider: ProviderType) -> AccountResetCapability {
    use ProviderType::*;
    let (support, reason) = match provider {
        CodexOAuth | ClaudeOAuth => ("supported", "identity_unverified"),
        GrokOAuth | KiroOAuth | AmazonQOAuth | CursorOAuth | GitHubCopilot | KimiCode
        | QoderCosy => ("conditional", "quota_boundary_unavailable"),
        AntigravityOAuth | AgyOAuth | GeminiCli => ("conditional", "multiple_quota_scopes"),
        CodeBuddyOAuth | TraeSolo => ("conditional", "non_recurring_package"),
        OllamaCloud => ("unsupported", "quota_boundary_unavailable"),
        CursorApiKey | DeepSeekAccount | Claude | ClaudeAuth | Codex | Gemini | OpenRouter
        | AwsBedrock | Nvidia | DeepSeekApi => ("unsupported", "profile_unsupported"),
    };
    AccountResetCapability {
        support: support.into(),
        sync_status: "initializing".into(),
        reason: Some(reason.into()),
        minimum_contract_version: 8,
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedQuotaSource {
    pub identity: String,
    pub provider_type: ProviderType,
    pub account_id: String,
    pub bucket_id: String,
    pub fence: QuotaSourceFence,
    pub observation: Option<QuotaWindowObservation>,
}

pub fn resolve_share_source(
    share: &Share,
    providers: &ProviderStore,
    accounts: &AccountStore,
) -> Result<ResolvedQuotaSource, &'static str> {
    resolve_share_source_with_coding(share, providers, accounts, |_| None)
}

pub fn resolve_share_source_with_coding(
    share: &Share,
    providers: &ProviderStore,
    accounts: &AccountStore,
    coding_observation: impl Fn(
        &crate::domain::providers::store::StoredProvider,
    ) -> Option<QuotaWindowObservation>,
) -> Result<ResolvedQuotaSource, &'static str> {
    let bindings = if share.bindings.is_empty() {
        vec![super::shares::ShareBinding {
            app: share.app,
            provider_id: share.provider_id.clone(),
            provider_type: share.provider_type,
        }]
    } else {
        share.bindings.clone()
    };
    super::credential_source::shared_credential_source_for_bindings(providers, accounts, &bindings)
        .map_err(|_| "multiple_quota_scopes")?;
    let mut selected: Option<ResolvedQuotaSource> = None;
    for binding in &bindings {
        let stored = providers
            .providers
            .iter()
            .find(|provider| {
                provider.app == binding.app && provider.provider.id == binding.provider_id
            })
            .ok_or("quota_provider_missing")?;
        if !crate::domain::providers::bundle::surface_enabled(&stored.provider) {
            continue;
        }
        let resolved = if let Some(account) = authoritative_managed_account(stored, accounts) {
            let bucket_id = match account.provider_type {
                ProviderType::CodexOAuth | ProviderType::ClaudeOAuth | ProviderType::KimiCode => {
                    "shared_weekly"
                }
                ProviderType::KiroOAuth | ProviderType::AmazonQOAuth => "agentic_subscription_base",
                ProviderType::CursorOAuth => "included_plan_credits",
                ProviderType::GitHubCopilot => "premium_monthly",
                _ => {
                    return Err(match account.provider_type {
                        ProviderType::AntigravityOAuth
                        | ProviderType::AgyOAuth
                        | ProviderType::GeminiCli => "multiple_quota_scopes",
                        ProviderType::CodeBuddyOAuth | ProviderType::TraeSolo => {
                            "non_recurring_package"
                        }
                        ProviderType::GrokOAuth | ProviderType::QoderCosy => "quota_scope_unproven",
                        _ => "profile_unsupported",
                    })
                }
            };
            let principal = if matches!(
                account.provider_type,
                ProviderType::CodexOAuth | ProviderType::ClaudeOAuth
            ) {
                resolve_provider_subscription_identity(stored, accounts)
                    .map_err(|_| "identity_unverified")?
                    .ok_or("identity_unverified")?
                    .fingerprint()
            } else {
                account_principal(account).ok_or("identity_unverified")?
            };
            ResolvedQuotaSource {
                identity: format!(
                    "{}:{}:{}",
                    account.provider_type.as_str(),
                    principal,
                    bucket_id
                ),
                provider_type: account.provider_type,
                account_id: account.id.clone(),
                bucket_id: bucket_id.into(),
                fence: QuotaSourceFence {
                    account_incarnation: accounts
                        .account_incarnations
                        .get(&account.id)
                        .copied()
                        .unwrap_or(0),
                    identity_generation: account.auth_identity_generation,
                    credential_generation: 0,
                    quota_credential_generation: 0,
                },
                observation: account_observation(account),
            }
        } else {
            let plan = providers
                .runtime_plan(stored.app, &stored.provider.id)
                .ok_or("profile_unsupported")?;
            let coding = plan.coding_plan.as_ref().ok_or("profile_unsupported")?;
            if coding.quota.adapter
                == crate::domain::providers::coding_plan::CodingPlanQuotaAdapter::Unavailable
            {
                return Err("profile_unsupported");
            }
            let identity = super::credential_source::resolve_provider_credential_source(
                providers,
                accounts,
                stored.app,
                &stored.provider.id,
            )
            .map_err(|_| "identity_unverified")?
            .ok_or("identity_unverified")?;
            // The credential scope includes the Family/origin. It is internal
            // only; the ledger registers a random external sourceRef.
            let observation = coding_observation(stored);
            ResolvedQuotaSource {
                identity: format!("coding:{}:shared_weekly", identity.quota_identity()),
                provider_type: stored.provider_type,
                account_id: String::new(),
                bucket_id: "shared_weekly".into(),
                observation,
                fence: QuotaSourceFence {
                    credential_generation: stored.resource.credential_generation,
                    quota_credential_generation: stored.resource.credential_generation,
                    ..Default::default()
                },
            }
        };
        if selected
            .as_ref()
            .is_some_and(|previous| previous.identity != resolved.identity)
        {
            return Err("multiple_quota_scopes");
        }
        selected = Some(resolved);
    }
    selected.ok_or("identity_unverified")
}

pub fn account_observation(account: &Account) -> Option<QuotaWindowObservation> {
    if account.provider_type == ProviderType::CodexOAuth {
        if let Some(observation) = account
            .codex_passive_quota
            .as_ref()
            .filter(|passive| passive.auth_identity_generation == account.auth_identity_generation)
            .and_then(|passive| passive.snapshot.weekly_observation())
            .filter(|observation| {
                observation.observed_at_ms > account.quota_refreshed_at.unwrap_or(-1)
            })
        {
            let now = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
            if observation.observed_at_ms <= now.saturating_add(300_000)
                && now <= observation.observed_at_ms.saturating_add(30 * 60 * 1000)
                && observation.ends_at_ms > now
            {
                return Some(observation);
            }
        }
    }
    if account.provider_type == ProviderType::ClaudeOAuth {
        if let Some(passive) = account
            .quota_window_observations
            .get(crate::domain::accounts::store::CLAUDE_SEVEN_DAY_QUOTA_TIER)
            .filter(|passive| {
                passive.auth_identity_generation == account.auth_identity_generation
                    && passive.observed_at_ms >= account.quota_refreshed_at.unwrap_or(0)
            })
        {
            if let Some(observation) = passive.resets_at_ms.and_then(|end| {
                checked_observation(
                    "shared_weekly",
                    None,
                    end,
                    Some(604_800_000),
                    passive.observed_at_ms,
                )
            }) {
                return Some(observation);
            }
        }
    }
    let quota = account.quota.as_ref().filter(|quota| quota.success)?;
    let observed_at_ms = account.quota_refreshed_at?;
    let extra = quota.extra_usage.as_ref()?;
    let raw = extra.get("raw").unwrap_or(&Value::Null);
    match account.provider_type {
        ProviderType::CodexOAuth => codex_observation(raw, observed_at_ms),
        ProviderType::ClaudeOAuth => {
            let reset = raw.pointer("/seven_day/resets_at")?.as_str()?;
            let end = chrono::DateTime::parse_from_rfc3339(reset)
                .ok()?
                .timestamp_millis();
            Some(QuotaWindowObservation {
                bucket_id: "shared_weekly".into(),
                starts_at_ms: None,
                ends_at_ms: end,
                duration_ms: Some(7 * 24 * 60 * 60 * 1000),
                observed_at_ms,
                basis: QuotaTimeBasis::DerivedFromWindow,
            })
        }
        ProviderType::KimiCode => kimi_observation(raw, observed_at_ms),
        ProviderType::KiroOAuth | ProviderType::AmazonQOAuth => {
            let resources = raw.get("usageBreakdownList")?.as_array()?;
            let mut resources = resources.iter().filter(|resource| {
                resource.get("resourceType").and_then(Value::as_str) == Some("AGENTIC_REQUEST")
            });
            let resource = resources.next()?;
            if resources.next().is_some()
                || resource
                    .get("usageLimitWithPrecision")
                    .or_else(|| resource.get("usageLimit"))?
                    .as_f64()?
                    <= 0.0
            {
                return None;
            }
            let end = resource
                .get("nextDateReset")
                .or_else(|| resource.get("nextResetTimestamp"))
                .or_else(|| raw.get("nextDateReset"))?;
            checked_observation(
                "agentic_subscription_base",
                None,
                timestamp_ms(end)?,
                None,
                observed_at_ms,
            )
        }
        ProviderType::CursorOAuth => {
            if extra.get("source").and_then(Value::as_str) != Some("cursor_dashboard_api") {
                return None;
            }
            let period = extra.pointer("/dashboard/currentPeriodUsage")?;
            if period
                .pointer("/planUsage/limit")
                .or_else(|| period.pointer("/planUsage/includedAmountCents"))?
                .as_f64()?
                <= 0.0
            {
                return None;
            }
            checked_observation(
                "included_plan_credits",
                Some(timestamp_ms(period.get("billingCycleStart")?)?),
                timestamp_ms(period.get("billingCycleEnd")?)?,
                None,
                observed_at_ms,
            )
        }
        ProviderType::GitHubCopilot => {
            if extra.get("source").and_then(Value::as_str) != Some("copilot_internal_user") {
                return None;
            }
            let tier = quota.tiers.iter().find(|tier| {
                tier.name == "premium" && tier.limit.is_some_and(|limit| limit > 0.0)
            })?;
            checked_observation(
                "premium_monthly",
                None,
                tier.resets_at?,
                None,
                observed_at_ms,
            )
        }
        _ => None,
    }
}

fn account_principal(account: &Account) -> Option<String> {
    let profile = account.profile.as_ref()?;
    let principal = match account.provider_type {
        ProviderType::KiroOAuth | ProviderType::AmazonQOAuth => {
            let arn = profile.get("profileArn")?.as_str()?;
            let region = profile
                .get("runtimeRegion")
                .or_else(|| profile.get("apiRegion"))?
                .as_str()?;
            format!("{arn}:{region}")
        }
        ProviderType::CursorOAuth => {
            crate::domain::accounts::cursor_import::cursor_subject_from_account(account)?
        }
        ProviderType::GitHubCopilot => format!(
            "{}:{}",
            profile.get("githubDomain")?.as_str()?,
            profile.get("id")?
        ),
        ProviderType::KimiCode => {
            crate::domain::kimi_cli::user_id_from_profile(account.profile.as_ref())?
        }
        _ => return None,
    };
    Some(hex::encode(Sha256::digest(principal.as_bytes())))
}

pub fn timestamp_ms(value: &Value) -> Option<i64> {
    if let Some(text) = value.as_str() {
        if let Ok(date) = chrono::DateTime::parse_from_rfc3339(text) {
            return Some(date.timestamp_millis());
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
            return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis());
        }
        return None;
    }
    let number = value.as_i64().or_else(|| {
        value
            .as_f64()
            .filter(|value| value.fract() == 0.0 && *value >= 0.0 && *value < i64::MAX as f64)
            .map(|value| value as i64)
    })?;
    if number < 100_000_000_000 {
        number.checked_mul(1000)
    } else {
        Some(number)
    }
}

fn checked_observation(
    bucket: &str,
    start: Option<i64>,
    end: i64,
    duration: Option<i64>,
    observed: i64,
) -> Option<QuotaWindowObservation> {
    let observation = QuotaWindowObservation {
        bucket_id: bucket.into(),
        starts_at_ms: start,
        ends_at_ms: end,
        duration_ms: duration,
        observed_at_ms: observed,
        basis: if start.is_some() {
            QuotaTimeBasis::UpstreamWindow
        } else {
            QuotaTimeBasis::DerivedFromWindow
        },
    };
    observation.validate_end().ok()?;
    if start.is_some() || duration.is_some() {
        observation.start().ok()?;
    }
    Some(observation)
}

pub fn kimi_observation(raw: &Value, observed_at_ms: i64) -> Option<QuotaWindowObservation> {
    let mut candidates = Vec::new();
    for field in ["usage", "seven_day"] {
        if let Some(window) = raw.get(field) {
            let end = ["resets_at", "resetTime", "reset_at", "resetAt"]
                .into_iter()
                .find_map(|field| window.get(field).and_then(timestamp_ms))?;
            candidates.push(end);
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    if candidates.len() != 1 {
        return None;
    }
    checked_observation(
        "shared_weekly",
        None,
        candidates[0],
        Some(604_800_000),
        observed_at_ms,
    )
}

pub fn coding_plan_capability(
    adapter: crate::domain::providers::coding_plan::CodingPlanQuotaAdapter,
) -> AccountResetCapability {
    use crate::domain::providers::coding_plan::CodingPlanQuotaAdapter::*;
    let mut capability = AccountResetCapability::unavailable("quota_boundary_unavailable");
    match adapter {
        Kimi | Zhipu | Minimax | Volcengine => {
            capability.support = "conditional".into();
        }
        Unavailable => {
            capability.support = "unsupported".into();
            capability.reason = Some("profile_unsupported".into());
        }
    }
    capability
}

pub fn coding_plan_observation(
    adapter: crate::domain::providers::coding_plan::CodingPlanQuotaAdapter,
    raw: &Value,
    observed_at_ms: i64,
) -> Option<QuotaWindowObservation> {
    use crate::domain::providers::coding_plan::CodingPlanQuotaAdapter::*;
    let end = match adapter {
        Kimi => return kimi_observation(raw, observed_at_ms),
        Zhipu => {
            let mut weekly = raw
                .pointer("/data/limits")?
                .as_array()?
                .iter()
                .filter(|window| {
                    matches!(
                        window.get("type").and_then(Value::as_str),
                        Some("TOKENS_LIMIT" | "CREDIT_LIMIT")
                    ) && (window.get("unit").and_then(declared_integer) == Some(6)
                        || (window.get("unit").and_then(declared_integer) == Some(4)
                            && window.get("number").and_then(declared_integer) == Some(7)))
                });
            let window = weekly.next()?;
            if weekly.next().is_some() {
                return None;
            }
            timestamp_ms(window.get("nextResetTime")?)?
        }
        Minimax => {
            let mut general = raw
                .get("model_remains")
                .or_else(|| raw.get("modelRemains"))?
                .as_array()?
                .iter()
                .filter(|window| {
                    window
                        .get("model_name")
                        .or_else(|| window.get("modelName"))
                        .and_then(Value::as_str)
                        == Some("general")
                });
            let window = general.next()?;
            if general.next().is_some()
                || window
                    .get("current_weekly_status")
                    .or_else(|| window.get("currentWeeklyStatus"))
                    .and_then(Value::as_i64)
                    != Some(1)
            {
                return None;
            }
            window
                .get("weekly_end_time")
                .or_else(|| window.get("weeklyEndTime"))
                .and_then(timestamp_ms)
                .filter(|end| *end > observed_at_ms)
                .or_else(|| {
                    window
                        .get("weekly_remains_time")
                        .or_else(|| window.get("weeklyRemainsTime"))
                        .and_then(Value::as_i64)
                        .filter(|remaining| *remaining > 0)
                        .and_then(|remaining| observed_at_ms.checked_add(remaining))
                })?
        }
        Volcengine => {
            let result = raw.get("Result").unwrap_or(raw);
            if let Some(window) = result.get("AFPWeekly") {
                if window.get("Quota")?.as_f64()? <= 0.0 {
                    return None;
                }
                explicit_timestamp(window.get("ResetTime")?, false)?
            } else {
                let mut weekly = result
                    .get("QuotaUsage")?
                    .as_array()?
                    .iter()
                    .filter(|window| {
                        window
                            .get("Level")
                            .and_then(Value::as_str)
                            .is_some_and(|level| level.eq_ignore_ascii_case("weekly"))
                    });
                let window = weekly.next()?;
                if weekly.next().is_some() {
                    return None;
                }
                explicit_timestamp(window.get("ResetTimestamp")?, true)?
            }
        }
        Unavailable => return None,
    };
    checked_observation(
        "shared_weekly",
        None,
        end,
        Some(604_800_000),
        observed_at_ms,
    )
}

fn declared_integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
}

fn explicit_timestamp(value: &Value, seconds: bool) -> Option<i64> {
    if let Some(text) = value.as_str().filter(|text| text.contains('T')) {
        return chrono::DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|date| date.timestamp_millis());
    }
    let value = declared_integer(value)?;
    if seconds {
        value.checked_mul(1000)
    } else {
        Some(value)
    }
}

pub fn codex_observation(raw: &Value, observed_at_ms: i64) -> Option<QuotaWindowObservation> {
    let rate = raw.get("rate_limit")?;
    let mut weekly = ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|name| rate.get(name))
        .filter(|window| {
            window.get("limit_window_seconds").and_then(Value::as_i64) == Some(604_800)
        });
    let window = weekly.next()?;
    if weekly.next().is_some() {
        return None;
    }
    let absolute = window
        .get("reset_at")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .and_then(|value| value.checked_mul(1000))
        .filter(|value| {
            *value > observed_at_ms && value.saturating_sub(observed_at_ms) <= 604_800_000
        });
    let relative = window
        .get("reset_after_seconds")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .and_then(|value| value.checked_mul(1000))
        .and_then(|value| observed_at_ms.checked_add(value));
    let observation = QuotaWindowObservation {
        bucket_id: "shared_weekly".into(),
        starts_at_ms: None,
        ends_at_ms: absolute.or(relative)?,
        duration_ms: Some(604_800_000),
        observed_at_ms,
        basis: QuotaTimeBasis::DerivedFromWindow,
    };
    observation.start().ok()?;
    Some(observation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn codex_selects_explicit_weekly_independently_of_position_and_uses_relative_seconds() {
        let raw = json!({"rate_limit":{"primary_window":{"limit_window_seconds":18000,"reset_at":1_000_100},
            "secondary_window":{"limit_window_seconds":604800,"reset_after_seconds":100}}});
        let observation = codex_observation(&raw, 1_000_000_000).unwrap();
        assert_eq!(observation.ends_at_ms, 1_000_100_000);
        assert_eq!(observation.duration_ms, Some(604_800_000));
        assert!(codex_observation(&json!({"rate_limit":{"primary_window":{"limit_window_seconds":18000,"reset_at":1_000_100}}}), 1_000_000_000).is_none());
    }

    #[test]
    fn quota_capability_covers_all_provider_types_and_twenty_coding_profiles() {
        assert_eq!(
            AccountResetCapability::unavailable("profile_unsupported").support,
            "unsupported"
        );
        assert_eq!(
            AccountResetCapability::unavailable("identity_unverified").support,
            "conditional"
        );
        let types: Vec<ProviderType> = serde_json::from_value(json!([
            "claude",
            "claude_auth",
            "claude_oauth",
            "codex",
            "codex_oauth",
            "gemini",
            "gemini_cli",
            "openrouter",
            "github_copilot",
            "deepseek_account",
            "kiro_oauth",
            "amazon_q_oauth",
            "kimi_code",
            "qoder_cosy",
            "codebuddy_oauth",
            "trae_solo",
            "cursor_oauth",
            "cursor_apikey",
            "antigravity_oauth",
            "agy_oauth",
            "ollama_cloud",
            "aws_bedrock",
            "nvidia",
            "deepseek_api",
            "grok_oauth"
        ]))
        .unwrap();
        assert_eq!(types.len(), 25);
        for provider in types {
            let capability = provider_capability(provider);
            assert_ne!(capability.sync_status, "ready");
            assert!(capability.reason.is_some());
            assert_eq!(capability.minimum_contract_version, 8);
        }
        let mut count = 0;
        for profile in &crate::domain::providers::registry::provider_registry().profiles {
            if let Some(plan) = &profile.coding_plan {
                count += 1;
                let capability = coding_plan_capability(plan.quota.adapter);
                assert_ne!(capability.sync_status, "ready");
                assert!(capability.reason.is_some());
            }
        }
        assert_eq!(count, 20);
    }

    #[test]
    fn quota_coding_main_buckets_reject_guesses_and_keep_explicit_units() {
        use crate::domain::providers::coding_plan::CodingPlanQuotaAdapter::*;
        let now = 1_800_000_000_000i64;
        let glm = json!({"data":{"limits":[{"type":"CREDIT_LIMIT","unit":"6","nextResetTime":now + 100_000}]}});
        assert!(coding_plan_observation(Zhipu, &glm, now).is_some());
        assert!(coding_plan_observation(
            Zhipu,
            &json!({"data":{"limits":[{"type":"TOKENS_LIMIT","nextResetTime":now+100_000}]}}),
            now
        )
        .is_none());
        assert!(coding_plan_observation(Minimax, &json!({"model_remains":[{"model_name":"general","current_weekly_status":0,"weekly_remains_time":10_000}]}), now).is_none());
        assert_eq!(coding_plan_observation(Minimax, &json!({"model_remains":[{"model_name":"general","current_weekly_status":1,"weekly_remains_time":10_000}]}), now).unwrap().ends_at_ms, now+10_000);
        let afp = json!({"Result":{"AFPWeekly":{"Quota":100,"ResetTime":now+100_000}}});
        let coding = json!({"Result":{"QuotaUsage":[{"Level":"weekly","ResetTimestamp":(now+100_000)/1000}]}});
        assert_eq!(
            coding_plan_observation(Volcengine, &afp, now)
                .unwrap()
                .ends_at_ms,
            coding_plan_observation(Volcengine, &coding, now)
                .unwrap()
                .ends_at_ms
        );
        assert!(coding_plan_observation(
            Volcengine,
            &json!({"Result":{"AFPWeekly":{"Quota":100,"ResetTime":(now+100_000)/1000}}}),
            now
        )
        .is_none());
        assert!(coding_plan_observation(
            Volcengine,
            &json!({"Result":{"QuotaUsage":[{"Level":"weekly","ResetTimestamp":now+100_000}]}}),
            now
        )
        .is_none());
    }

    fn account(provider: ProviderType, raw: Value) -> Account {
        serde_json::from_value(
            json!({"id":"fixture", "providerType":provider, "quotaRefreshedAt":1_800_000_000_000i64,
            "quota":{"success":true,"tiers":[],"extraUsage":{"raw":raw}}}),
        )
        .unwrap()
    }

    #[test]
    fn quota_kiro_q_base_and_cursor_included_windows_do_not_follow_packages_or_snapshots() {
        let end = 1_800_100_000_000i64;
        for provider in [ProviderType::KiroOAuth, ProviderType::AmazonQOAuth] {
            let raw = json!({"nextDateReset":end/1000, "usageBreakdownList":[{"resourceType":"AGENTIC_REQUEST",
                "usageLimitWithPrecision":100,"freeTrialInfo":{"freeTrialExpiry":1_800_050_000},
                "bonuses":[{"expiresAt":1_800_025_000}]}]});
            let account = account(provider, raw.clone());
            let observation = account_observation(&account).unwrap();
            assert_eq!(observation.ends_at_ms, end);
            assert!(observation.starts_at_ms.is_none());
            assert_eq!(observation.bucket_id, "agentic_subscription_base");
            let mut trial_only = raw;
            trial_only["usageBreakdownList"][0]["usageLimitWithPrecision"] = json!(0);
            assert!(account_observation(&self::account(provider, trial_only)).is_none());
        }
        let mut cursor = account(ProviderType::CursorOAuth, Value::Null);
        cursor.quota.as_mut().unwrap().extra_usage = Some(
            json!({"source":"cursor_dashboard_api", "dashboard":{"currentPeriodUsage":{
            "billingCycleStart":1_799_000_000_123i64,"billingCycleEnd":end,"planUsage":{"limit":100}}}}),
        );
        let cycle = account_observation(&cursor).unwrap();
        assert_eq!(cycle.starts_at_ms, Some(1_799_000_000_123));
        assert_eq!(cycle.ends_at_ms, end);
        cursor.quota.as_mut().unwrap().extra_usage.as_mut().unwrap()["source"] =
            json!("imported_snapshot");
        assert!(account_observation(&cursor).is_none());
    }

    #[test]
    fn quota_principals_are_separated_by_region_and_missing_identity_is_not_guessed() {
        let mut kiro = account(ProviderType::KiroOAuth, Value::Null);
        assert!(account_principal(&kiro).is_none());
        kiro.profile = Some(json!({"profileArn":"opaque-profile","runtimeRegion":"us-east-1"}));
        let east = account_principal(&kiro).unwrap();
        kiro.profile.as_mut().unwrap()["runtimeRegion"] = json!("eu-west-1");
        assert_ne!(account_principal(&kiro).unwrap(), east);
    }

    #[test]
    fn quota_copilot_uses_only_finite_live_premium_allowance_and_real_reset() {
        let mut copilot = account(ProviderType::GitHubCopilot, Value::Null);
        copilot.quota.as_mut().unwrap().extra_usage =
            Some(json!({"source":"copilot_internal_user"}));
        copilot.quota.as_mut().unwrap().tiers.push(
            serde_json::from_value(json!({
                "name":"premium","limit":50,"resetsAt":1_800_100_000_000i64
            }))
            .unwrap(),
        );
        let observation = account_observation(&copilot).unwrap();
        assert_eq!(observation.ends_at_ms, 1_800_100_000_000);
        assert_eq!(observation.starts_at_ms, None);
        assert_eq!(observation.duration_ms, None); // no guessed 30-day month
        copilot.quota.as_mut().unwrap().tiers[0].limit = None; // unlimited
        assert!(account_observation(&copilot).is_none());
        copilot.quota.as_mut().unwrap().tiers[0].limit = Some(50.0);
        copilot.quota.as_mut().unwrap().tiers[0].resets_at = None;
        assert!(account_observation(&copilot).is_none());
        copilot.quota.as_mut().unwrap().tiers[0].resets_at = Some(1_800_100_000_000);
        copilot
            .quota
            .as_mut()
            .unwrap()
            .extra_usage
            .as_mut()
            .unwrap()["source"] = json!("imported_snapshot");
        assert!(account_observation(&copilot).is_none());
    }

    #[test]
    fn quota_unsupported_package_model_and_billing_fields_never_trigger() {
        for provider in [
            ProviderType::GrokOAuth,
            ProviderType::QoderCosy,
            ProviderType::CodeBuddyOAuth,
            ProviderType::TraeSolo,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
            ProviderType::GeminiCli,
            ProviderType::OllamaCloud,
            ProviderType::CursorApiKey,
        ] {
            let account = account(
                provider,
                json!({"expiresAt":"9999-12-31T00:00:00Z", "CycleEndTime":1_800_100_000,
                "nextResetAt":1_800_100_000, "weekly":{"reset_at":1_800_100_000}, "last_4_weeks":10}),
            );
            assert!(
                account_observation(&account).is_none(),
                "{}",
                provider.as_str()
            );
        }
        let account = account(
            ProviderType::ClaudeOAuth,
            json!({"five_hour":{"resets_at":"2027-01-15T00:00:00Z"},
            "seven_day_opus":{"resets_at":"2027-01-15T00:00:00Z"}}),
        );
        assert!(account_observation(&account).is_none());
    }

    #[test]
    fn quota_shared_aliases_and_absolute_relative_bounds_are_strict() {
        let now = 1_800_000_000_000i64;
        assert_eq!(
            kimi_observation(&json!({"usage":{"reset_at":(now+100_000)/1000}}), now),
            kimi_observation(
                &json!({"seven_day":{"reset_at":(now+100_000)/1000},"seven_day_opus":{"reset_at":1}}),
                now
            )
        );
        assert!(kimi_observation(&json!({"usage":{"reset_at":(now+100_000)/1000},"seven_day":{"reset_at":(now+200_000)/1000}}), now).is_none());
        for seconds in [-1, i64::MAX, 604_801] {
            assert!(codex_observation(&json!({"rate_limit":{"primary_window":{"limit_window_seconds":604800,"reset_after_seconds":seconds}}}), now).is_none());
        }
        assert!(codex_observation(&json!({"code_review_rate_limit":{"primary_window":{"limit_window_seconds":604800,"reset_after_seconds":100}}}), now).is_none());
        let raw = json!({"rate_limit":{"primary_window":{"limit_window_seconds":604800,"reset_at":(now+200_000)/1000,"reset_after_seconds":100}}});
        assert_eq!(
            codex_observation(&raw, now).unwrap().ends_at_ms,
            now + 200_000
        );
    }
}
