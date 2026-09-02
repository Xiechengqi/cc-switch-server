use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::clients::oauth::claude_models::CLAUDE_FABLE_MODEL_FAMILY;
use crate::clients::oauth::codex_reset_credits::{
    codex_authenticated_get, fetch_reset_credit_details, merge_reset_credit_snapshot,
    normalize_imported_snapshot, parse_usage_available_count,
};
use crate::clients::oauth::kiro_device::{
    agentic_quota_percent, fetch_usage_limits, machine_id_from_refresh_token,
    quota_from_usage_limits,
};
use crate::cursor_client_contract::cursor_membership_label;
use crate::domain::accounts::capability_evidence::{
    AccountCapabilityObservationDraft, AccountCapabilityObservationState,
    CLAUDE_QUOTA_FAMILY_DIMENSION, GEMINI_QUOTA_FAMILY_DIMENSION, MEDIA_ENTITLEMENT_DIMENSION,
    MODEL_CAPACITY_DIMENSION, MODEL_ENTITLEMENT_DIMENSION, PRIVACY_DIMENSION,
    PROJECT_BOOTSTRAP_DIMENSION, TIER_ENTITLEMENT_DIMENSION,
};
use crate::domain::accounts::claude_subscription::{
    parse_claude_subscription_plan, resolve_claude_subscription, ClaudeFableEligibility,
    ClaudeSubscriptionCandidate, ClaudeSubscriptionResolution, ClaudeSubscriptionSource,
};
use crate::domain::accounts::grok_subscription::canonical_grok_subscription_level;
use crate::domain::accounts::store::{
    gemini_v1internal_project_id, Account, AccountQuota, AccountQuotaTier, AccountRefreshUpdate,
    CLAUDE_FABLE_CAPACITY_POOL, CLAUDE_FABLE_QUOTA_TIER, CLAUDE_FABLE_RELATIVE_WEEKLY_CAPACITY,
};
use crate::domain::claude_cli::{
    claude_axios_user_agent, claude_cli_user_agent, claude_code_user_agent,
};
use crate::domain::grok_cli::{
    grok_cli_user_agent, grok_cli_version, GROK_CLI_CLIENT_IDENTIFIER,
    GROK_CLI_MONTHLY_BILLING_URL, GROK_CLI_TOKEN_AUTH, GROK_CLI_USER_URL,
    GROK_CLI_WEEKLY_BILLING_URL, GROK_SUBSCRIPTIONS_URL, GROK_TASK_USAGE_URL,
};
use crate::domain::providers::model::ProviderType;

pub const QUOTA_FAILURE_COOLDOWN_MS: i64 = 2 * 60 * 1000;
const MAX_QUOTA_RESPONSE_BODY_BYTES: usize = 4 * 1024 * 1024;

fn quota_request_timeout(timeout_ms: i64) -> Duration {
    Duration::from_millis(timeout_ms.clamp(1_000, 120_000) as u64)
}

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CLAUDE_BOOTSTRAP_URL: &str = "https://api.anthropic.com/api/claude_cli/bootstrap";
const CLAUDE_ROLES_URL: &str = "https://api.anthropic.com/api/oauth/claude_cli/roles";
const MAX_CLAUDE_CONTROL_RESPONSE_BODY_BYTES: usize = 512 * 1024;
const CLAUDE_PLAN_CACHE_MAX_AGE_MS: i64 = 24 * 60 * 60 * 1000;
const CLAUDE_PLAN_CACHE_CLOCK_SKEW_MS: i64 = 5 * 60 * 1000;
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CHATGPT_ACCOUNTS_CHECK_URL: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
const CHATGPT_SUBSCRIPTIONS_URL: &str = "https://chatgpt.com/backend-api/subscriptions";
const GEMINI_LOAD_CODE_ASSIST_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const GEMINI_RETRIEVE_USER_QUOTA_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
const ANTIGRAVITY_FETCH_USER_INFO_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchUserInfo";
const OLLAMA_ME_URL: &str = "https://ollama.com/api/me";

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum QuotaRefreshResult {
    Updated {
        update: AccountRefreshUpdate,
        message: String,
    },
    SkippedCooldown {
        next_refresh_at: i64,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct QuotaRefreshFailure {
    pub status_code: u16,
    pub upstream_status: Option<u16>,
    pub message: String,
    pub retryable: bool,
    pub next_refresh_at: Option<i64>,
    pub partial_update: Option<Box<AccountRefreshUpdate>>,
}

impl QuotaRefreshFailure {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status_code: 400,
            upstream_status: None,
            message: message.into(),
            retryable: false,
            next_refresh_at: None,
            partial_update: None,
        }
    }

    fn upstream(
        provider_type: ProviderType,
        upstream_status: reqwest::StatusCode,
        body: String,
        retry_after: Option<String>,
        now_ms: i64,
    ) -> Self {
        let status_code = match upstream_status.as_u16() {
            401 | 403 => 400,
            402 => 402,
            429 => 429,
            _ => 502,
        };
        let retryable = !matches!(upstream_status.as_u16(), 401..=403);
        let next_refresh_at = retry_after
            .as_deref()
            .and_then(parse_retry_after_ms)
            .map(|delay| now_ms.saturating_add(delay))
            .or_else(|| retryable.then_some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)));
        Self {
            status_code,
            upstream_status: Some(upstream_status.as_u16()),
            message: format!(
                "{} quota request failed: upstream HTTP {}: {}",
                provider_type.as_str(),
                upstream_status.as_u16(),
                truncate(&crate::logging::redact_sensitive_text(&body), 240)
            ),
            retryable,
            next_refresh_at,
            partial_update: None,
        }
    }

    fn network(provider_type: ProviderType, error: reqwest::Error, now_ms: i64) -> Self {
        Self {
            status_code: 502,
            upstream_status: None,
            message: format!("{} quota request failed: {error}", provider_type.as_str()),
            retryable: true,
            next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
            partial_update: None,
        }
    }

    fn response_body(
        provider_type: ProviderType,
        error: crate::infra::http::BoundedResponseBodyError,
        now_ms: i64,
    ) -> Self {
        match error {
            crate::infra::http::BoundedResponseBodyError::Request(error) => {
                Self::network(provider_type, error, now_ms)
            }
            error @ crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => Self {
                status_code: 502,
                upstream_status: None,
                message: format!(
                    "{} quota response could not be read: {error}",
                    provider_type.as_str()
                ),
                retryable: false,
                next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
                partial_update: None,
            },
        }
    }

    fn parse(provider_type: ProviderType, error: impl std::fmt::Display, now_ms: i64) -> Self {
        Self {
            status_code: 502,
            upstream_status: None,
            message: format!(
                "{} quota response is not valid JSON: {error}",
                provider_type.as_str()
            ),
            retryable: false,
            next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
            partial_update: None,
        }
    }

    fn missing_gemini_project(provider_type: ProviderType, now_ms: i64) -> Self {
        Self {
            status_code: 502,
            upstream_status: None,
            message: format!(
                "{} loadCodeAssist returned no usable projectId",
                provider_type.as_str()
            ),
            retryable: true,
            next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
            partial_update: None,
        }
    }

    fn with_partial_update(mut self, update: AccountRefreshUpdate) -> Self {
        self.partial_update = Some(Box::new(update));
        self
    }
}

pub async fn refresh_account_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    force: bool,
    success_cooldown_ms: i64,
    request_timeout_ms: i64,
) -> Result<QuotaRefreshResult, QuotaRefreshFailure> {
    if !force {
        if let Some(next_refresh_at) = account.quota_next_refresh_at {
            if next_refresh_at > now_ms {
                return Ok(QuotaRefreshResult::SkippedCooldown {
                    next_refresh_at,
                    message: format!("quota refresh skipped until {}", next_refresh_at),
                });
            }
        }
    }

    #[cfg(test)]
    if let Some(delay_ms) = account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("testQuotaRefreshDelayMs"))
        .and_then(Value::as_u64)
    {
        tokio::time::sleep(Duration::from_millis(delay_ms.min(5_000))).await;
    }

    let request_timeout = quota_request_timeout(request_timeout_ms);
    let update = match account.provider_type {
        ProviderType::CodexOAuth => {
            refresh_codex_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::ClaudeOAuth => {
            refresh_claude_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        ProviderType::GeminiCli => {
            refresh_gemini_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            refresh_antigravity_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        ProviderType::KiroOAuth => {
            refresh_kiro_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::AmazonQOAuth => {
            refresh_amazon_q_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        ProviderType::GrokOAuth => {
            refresh_grok_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::GitHubCopilot => {
            refresh_copilot_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        ProviderType::KimiCode => {
            refresh_kimi_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::QoderCosy => {
            refresh_qoder_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::CodeBuddyOAuth => {
            refresh_codebuddy_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        ProviderType::TraeSolo => {
            refresh_trae_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::CursorOAuth => {
            refresh_cursor_dashboard_quota(
                http,
                account,
                now_ms,
                success_cooldown_ms,
                request_timeout,
            )
            .await?
        }
        ProviderType::CursorApiKey => {
            refresh_imported_snapshot_quota(account, now_ms, success_cooldown_ms)?
        }
        ProviderType::OllamaCloud => {
            refresh_ollama_cloud_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
                .await?
        }
        provider_type => {
            return Err(QuotaRefreshFailure::bad_request(format!(
                "{} real quota refresh is not implemented",
                provider_type.as_str()
            )))
        }
    };

    if crate::domain::accounts::store::account_refresh_replaces_auth_identity(account, &update) {
        return Err(QuotaRefreshFailure::bad_request(format!(
            "{} quota refresh returned a different subscription identity; re-login as a new account",
            account.provider_type.as_str()
        )));
    }

    Ok(QuotaRefreshResult::Updated {
        update,
        message: "quota refreshed from upstream provider".to_string(),
    })
}

async fn refresh_codebuddy_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let body =
        crate::clients::oauth::codebuddy::fetch_billing_resource(http, account, request_timeout)
            .await
            .map_err(|error| codebuddy_quota_failure(error, now_ms))?;
    parse_codebuddy_quota_update(account, &body, now_ms, success_cooldown_ms)
}

fn codebuddy_quota_failure(
    error: crate::clients::oauth::codebuddy::CodeBuddyClientError,
    now_ms: i64,
) -> QuotaRefreshFailure {
    let upstream_status = error.upstream_status.map(|status| status.as_u16());
    let retryable = error.is_transient();
    QuotaRefreshFailure {
        status_code: error.status.as_u16(),
        upstream_status,
        message: crate::logging::redact_sensitive_text(&error.message),
        retryable,
        next_refresh_at: retryable.then_some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        partial_update: None,
    }
}

#[derive(Debug, Clone)]
struct CodeBuddyQuotaPackage {
    package_name: Option<String>,
    sub_product_code: Option<String>,
    capacity_size: f64,
    capacity_remain: f64,
    capacity_unit: String,
    cycle_end_time: String,
    expires_at_ms: i64,
}

fn parse_codebuddy_quota_update(
    account: &Account,
    body: &Value,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let data = body.pointer("/data/Response/Data").ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::CodeBuddyOAuth,
            "response contains no data.Response.Data billing envelope",
            now_ms,
        )
    })?;
    let object = data.as_object().ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::CodeBuddyOAuth,
            "data.Response.Data must be an object",
            now_ms,
        )
    })?;
    let total_count =
        codebuddy_required_nonnegative_integer(object.get("TotalCount"), "TotalCount").map_err(
            |message| QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms),
        )?;
    let total_dosage =
        codebuddy_required_nonnegative_number(object.get("TotalDosage"), "TotalDosage").map_err(
            |message| QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms),
        )?;
    let accounts = object
        .get("Accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            QuotaRefreshFailure::parse(
                ProviderType::CodeBuddyOAuth,
                "data.Response.Data.Accounts must be an array",
                now_ms,
            )
        })?;
    if accounts.is_empty() {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::CodeBuddyOAuth,
            "billing resource package list is empty; availability is unknown",
            now_ms,
        ));
    }
    if total_count != accounts.len() as u64 {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::CodeBuddyOAuth,
            format!(
                "billing TotalCount {total_count} does not match the complete Accounts page of {} packages",
                accounts.len()
            ),
            now_ms,
        ));
    }

    let mut packages = Vec::with_capacity(accounts.len());
    for (index, value) in accounts.iter().enumerate() {
        let package = value.as_object().ok_or_else(|| {
            QuotaRefreshFailure::parse(
                ProviderType::CodeBuddyOAuth,
                format!("billing resource package {index} must be an object"),
                now_ms,
            )
        })?;
        let package_name =
            codebuddy_optional_bounded_string(package.get("PackageName"), "PackageName").map_err(
                |message| QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms),
            )?;
        let sub_product_code =
            codebuddy_optional_bounded_string(package.get("SubProductCode"), "SubProductCode")
                .map_err(|message| {
                    QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms)
                })?;
        if package_name.is_none() && sub_product_code.is_none() {
            return Err(QuotaRefreshFailure::parse(
                ProviderType::CodeBuddyOAuth,
                format!(
                    "billing resource package {index} has neither PackageName nor SubProductCode"
                ),
                now_ms,
            ));
        }
        let capacity_size =
            codebuddy_required_nonnegative_number(package.get("CapacitySize"), "CapacitySize")
                .map_err(|message| {
                    QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms)
                })?;
        let capacity_remain =
            codebuddy_required_nonnegative_number(package.get("CapacityRemain"), "CapacityRemain")
                .map_err(|message| {
                    QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms)
                })?;
        if capacity_remain > capacity_size {
            return Err(QuotaRefreshFailure::parse(
                ProviderType::CodeBuddyOAuth,
                format!("billing resource package {index} CapacityRemain exceeds CapacitySize"),
                now_ms,
            ));
        }
        let capacity_unit =
            codebuddy_optional_bounded_string(package.get("CapacityUnit"), "CapacityUnit")
                .map_err(|message| {
                    QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms)
                })?
                .ok_or_else(|| {
                    QuotaRefreshFailure::parse(
                        ProviderType::CodeBuddyOAuth,
                        format!("billing resource package {index} is missing CapacityUnit"),
                        now_ms,
                    )
                })?;
        let capacity_unit = match capacity_unit.to_ascii_lowercase().as_str() {
            "credit" | "credits" => "credits".to_string(),
            _ => {
                return Err(QuotaRefreshFailure::parse(
                    ProviderType::CodeBuddyOAuth,
                    format!("billing resource package {index} has unsupported CapacityUnit"),
                    now_ms,
                ))
            }
        };
        let cycle_end_time =
            codebuddy_optional_bounded_string(package.get("CycleEndTime"), "CycleEndTime")
                .map_err(|message| {
                    QuotaRefreshFailure::parse(ProviderType::CodeBuddyOAuth, message, now_ms)
                })?
                .ok_or_else(|| {
                    QuotaRefreshFailure::parse(
                        ProviderType::CodeBuddyOAuth,
                        format!("billing resource package {index} is missing CycleEndTime"),
                        now_ms,
                    )
                })?;
        let expires_at_ms = codebuddy_billing_timestamp_ms(&cycle_end_time).ok_or_else(|| {
            QuotaRefreshFailure::parse(
                ProviderType::CodeBuddyOAuth,
                format!("billing resource package {index} has invalid CycleEndTime"),
                now_ms,
            )
        })?;
        packages.push(CodeBuddyQuotaPackage {
            package_name,
            sub_product_code,
            capacity_size,
            capacity_remain,
            capacity_unit,
            cycle_end_time,
            expires_at_ms,
        });
    }
    packages.sort_by(|left, right| {
        left.expires_at_ms
            .cmp(&right.expires_at_ms)
            .then_with(|| left.package_name.cmp(&right.package_name))
            .then_with(|| left.sub_product_code.cmp(&right.sub_product_code))
    });
    if !packages.iter().any(|package| package.capacity_size > 0.0) {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::CodeBuddyOAuth,
            "billing resource packages contain no positive authoritative capacity; availability is unknown",
            now_ms,
        ));
    }

    let mut tiers = Vec::with_capacity(packages.len());
    let mut projections = Vec::with_capacity(packages.len());
    for (index, package) in packages.iter().enumerate() {
        let used = (package.capacity_size - package.capacity_remain).max(0.0);
        let utilization =
            (package.capacity_size > 0.0).then_some((used / package.capacity_size).clamp(0.0, 1.0));
        let label = package
            .package_name
            .clone()
            .or_else(|| package.sub_product_code.clone());
        tiers.push(AccountQuotaTier {
            name: format!("codebuddy_package_{}", index + 1),
            label: label.clone(),
            utilization,
            used: Some(used),
            limit: Some(package.capacity_size),
            unit: Some(package.capacity_unit.clone()),
            resets_at: Some(package.expires_at_ms),
            source: Some("codebuddy_billing_resource".to_string()),
            ..Default::default()
        });
        // Deliberate whitelist: never preserve Tencent billing account IDs,
        // deal/resource IDs, payer UINs, or future vendor fields.
        projections.push(json!({
            "packageName": package.package_name,
            "subProductCode": package.sub_product_code,
            "capacitySize": package.capacity_size,
            "capacityRemain": package.capacity_remain,
            "capacityUnit": package.capacity_unit,
            "cycleEndTime": package.cycle_end_time,
            "expiresAt": package.expires_at_ms,
        }));
    }
    let availability = if packages.iter().any(|package| package.capacity_remain > 0.0) {
        "available"
    } else {
        "exhausted"
    };
    let subscription_level = packages
        .first()
        .and_then(|package| {
            package
                .package_name
                .clone()
                .or_else(|| package.sub_product_code.clone())
        })
        .or_else(|| account.subscription_level.clone())
        .or_else(|| Some("CodeBuddy".to_string()));
    let quota = AccountQuota {
        success: true,
        credential_message: subscription_level.clone(),
        tiers,
        extra_usage: Some(json!({
            "source": "codebuddy_billing_resource",
            "codeBuddyBilling": {
                "availability": availability,
                "totalCount": total_count,
                "totalDosage": total_dosage,
                "packages": projections,
            }
        })),
    };
    let mut update =
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms);
    update.entitlement_status = Some(availability.to_string());
    // This quota is display/control-plane evidence only. It must never create
    // a routing cooldown or participate in account selection.
    update.rate_limited_until = None;
    update.clear_rate_limited_until_if = None;
    Ok(update)
}

fn codebuddy_required_nonnegative_integer(
    value: Option<&Value>,
    field: &str,
) -> Result<u64, String> {
    let value = value.ok_or_else(|| format!("CodeBuddy billing response is missing {field}"))?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
        .ok_or_else(|| format!("CodeBuddy billing {field} must be a non-negative integer"))
}

fn codebuddy_required_nonnegative_number(
    value: Option<&Value>,
    field: &str,
) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("CodeBuddy billing response is missing {field}"))?;
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| format!("CodeBuddy billing {field} must be a finite non-negative number"))
}

fn codebuddy_optional_bounded_string(
    value: Option<&Value>,
    field: &str,
) -> Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| format!("CodeBuddy billing {field} must be a string"))?
        .trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 256 || value.chars().any(char::is_control) {
        return Err(format!(
            "CodeBuddy billing {field} exceeds its safe display bounds"
        ));
    }
    Ok(Some(value.to_string()))
}

fn codebuddy_billing_timestamp_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|value| value.timestamp_millis())
        .or_else(|| {
            NaiveDateTime::parse_from_str(value.trim(), "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|value| value.and_utc().timestamp_millis())
        })
}

async fn refresh_trae_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let body = crate::clients::oauth::trae::fetch_entitlement_usage(http, account, request_timeout)
        .await
        .map_err(|error| trae_quota_failure(error, now_ms))?;
    parse_trae_quota_update(account, &body, now_ms, success_cooldown_ms)
}

fn trae_quota_failure(
    error: crate::clients::oauth::trae::TraeClientError,
    now_ms: i64,
) -> QuotaRefreshFailure {
    let upstream_status = error.upstream_status.map(|status| status.as_u16());
    let retryable = error.is_transient();
    QuotaRefreshFailure {
        status_code: error.status.as_u16(),
        upstream_status,
        message: crate::logging::redact_sensitive_text(&error.message),
        retryable,
        next_refresh_at: retryable.then_some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        partial_update: None,
    }
}

fn parse_trae_quota_update(
    account: &Account,
    body: &Value,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let packs = trae_entitlement_packs(body).ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::TraeSolo,
            "response contains no user_entitlement_pack_list",
            now_ms,
        )
    })?;
    if packs.is_empty() {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::TraeSolo,
            "entitlement pack list is explicitly empty; availability is unknown",
            now_ms,
        ));
    }

    let mut tiers = Vec::new();
    let mut projections = Vec::with_capacity(packs.len());
    let mut total_limit = 0.0;
    let mut total_used = 0.0;
    let mut plan = None;
    let mut latest_expiry: Option<i64> = None;
    for (index, pack) in packs.iter().enumerate() {
        let object = pack.as_object().ok_or_else(|| {
            QuotaRefreshFailure::parse(
                ProviderType::TraeSolo,
                format!("entitlement pack {index} must be an object"),
                now_ms,
            )
        })?;
        let pack_plan = trae_first_string(
            pack,
            &[
                &["entitlement_base_info", "plan_name"],
                &["entitlement_base_info", "display_name"],
                &["entitlement_base_info", "entitlement_name"],
                &["plan_name"],
                &["display_name"],
                &["name"],
            ],
        );
        if plan.is_none() {
            plan = pack_plan.clone();
        }
        let status = trae_first_string(
            pack,
            &[
                &["entitlement_base_info", "status"],
                &["entitlement_base_info", "entitlement_status"],
                &["status"],
            ],
        );
        let period = trae_first_string(
            pack,
            &[
                &["entitlement_base_info", "period"],
                &["quota", "period"],
                &["period"],
            ],
        );
        let period_start = trae_first_timestamp(
            pack,
            &[
                &["entitlement_base_info", "period_start"],
                &["entitlement_base_info", "start_time"],
                &["quota", "start_time"],
                &["start_time"],
                &["start_at"],
            ],
        );
        let expires_at = trae_first_timestamp(
            pack,
            &[
                &["entitlement_base_info", "period_end"],
                &["entitlement_base_info", "end_time"],
                &["entitlement_base_info", "expire_time"],
                &["quota", "end_time"],
                &["end_time"],
                &["expire_time"],
                &["expires_at"],
            ],
        );
        latest_expiry = match (latest_expiry, expires_at) {
            (Some(current), Some(candidate)) => Some(current.max(candidate)),
            (None, candidate) => candidate,
            (current, None) => current,
        };
        let limit = trae_first_nonnegative_number(
            pack,
            &[
                &["entitlement_base_info", "quota", "credits_limit"],
                &["quota", "credits_limit"],
            ],
        )?;
        let used = trae_first_nonnegative_number(
            pack,
            &[
                &["usage", "credits_amount"],
                &["usage", "credits_used"],
                &["credits_used"],
            ],
        )?;
        let (remaining, utilization) = match (limit, used) {
            (Some(limit), Some(used)) if limit > 0.0 && used <= limit => (
                Some((limit - used).max(0.0)),
                Some((used / limit).clamp(0.0, 1.0)),
            ),
            (Some(limit), None) if limit > 0.0 => {
                return Err(QuotaRefreshFailure::parse(
                    ProviderType::TraeSolo,
                    format!(
                        "entitlement pack {index} has a positive credit limit but no authoritative usage; availability is unknown"
                    ),
                    now_ms,
                ));
            }
            (Some(limit), Some(used)) if used > limit => {
                return Err(QuotaRefreshFailure::parse(
                    ProviderType::TraeSolo,
                    format!("entitlement pack {index} usage exceeds its credit limit"),
                    now_ms,
                ));
            }
            _ => (None, None),
        };
        if let Some(limit) = limit.filter(|value| *value > 0.0) {
            let used = used.expect("positive Trae credit limits require authoritative usage");
            total_limit += limit;
            total_used += used;
            tiers.push(AccountQuotaTier {
                name: format!("trae_entitlement_{}", index + 1),
                label: pack_plan
                    .clone()
                    .or_else(|| Some(format!("Trae entitlement pack {}", index + 1))),
                utilization,
                used: Some(used),
                limit: Some(limit),
                unit: Some("entitlement_pack".to_string()),
                resets_at: expires_at,
                ..Default::default()
            });
        }
        // This is deliberately a whitelist projection. Never retain the raw
        // pack because vendor payloads may grow UID/device/token fields.
        projections.push(json!({
            "plan": pack_plan,
            "status": status,
            "period": period,
            "periodStart": period_start,
            "expiresAt": expires_at,
            "credits": {
                "limit": limit,
                "used": used,
                "remaining": remaining,
                "utilization": utilization,
                "unit": "entitlement_pack",
            }
        }));
        let _ = object;
    }
    if tiers.is_empty() || total_limit <= 0.0 {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::TraeSolo,
            "entitlement packs contain no positive credit limit; availability is unknown",
            now_ms,
        ));
    }

    let utilization = (total_used / total_limit).clamp(0.0, 1.0);
    let remaining = (total_limit - total_used).max(0.0);
    let availability = if remaining > 0.0 {
        "available"
    } else {
        "exhausted"
    };
    let plan = plan
        .or_else(|| account.subscription_level.clone())
        .or_else(|| Some("Trae CN Solo".to_string()));
    let quota = AccountQuota {
        success: true,
        credential_message: plan.clone(),
        tiers,
        extra_usage: Some(json!({
            "source": "trae_entitlement_usage",
            "traeEntitlement": {
                "availability": availability,
                "totalCredits": total_limit,
                "usedCredits": total_used,
                "remainingCredits": remaining,
                "utilization": utilization,
                "expiresAt": latest_expiry,
                "packs": projections,
            }
        })),
    };
    let mut update = update_from_quota(quota, plan, None, now_ms, success_cooldown_ms);
    update.entitlement_status = Some(availability.to_string());
    update.quota_percent = Some(utilization * 100.0);
    // Quota observation is informational for this single-account rail. It
    // must not install a cooldown that could participate in account routing.
    update.rate_limited_until = None;
    update.clear_rate_limited_until_if = None;
    Ok(update)
}

fn trae_entitlement_packs(value: &Value) -> Option<&[Value]> {
    match value {
        Value::Array(values) => Some(values.as_slice()),
        Value::Object(object) => {
            if let Some(value) = trae_object_value(object, "user_entitlement_pack_list") {
                return value.as_array().map(Vec::as_slice);
            }
            ["data", "Result", "result"]
                .iter()
                .find_map(|key| trae_object_value(object, key).and_then(trae_entitlement_packs))
        }
        _ => None,
    }
}

fn trae_object_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Option<&'a Value> {
    object.get(key).or_else(|| {
        object
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    })
}

fn trae_path<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in path {
        current = trae_object_value(current.as_object()?, key)?;
    }
    Some(current)
}

fn trae_first_string(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        trae_path(value, path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| truncate(value, 256))
    })
}

fn trae_first_nonnegative_number(
    value: &Value,
    paths: &[&[&str]],
) -> Result<Option<f64>, QuotaRefreshFailure> {
    for path in paths {
        let Some(value) = trae_path(value, path).filter(|value| !value.is_null()) else {
            continue;
        };
        let number = value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .filter(|value: &f64| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                QuotaRefreshFailure::bad_request(
                    "Trae entitlement credits must be finite non-negative numbers",
                )
            })?;
        return Ok(Some(number));
    }
    Ok(None)
}

fn trae_first_timestamp(value: &Value, paths: &[&[&str]]) -> Option<i64> {
    paths.iter().find_map(|path| {
        let value = trae_path(value, path)?;
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .and_then(timestamp_number_to_unix_ms)
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|value| DateTime::parse_from_rfc3339(value.trim()).ok())
                    .map(|value| value.timestamp_millis())
            })
    })
}

async fn refresh_qoder_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let body = crate::clients::oauth::qoder::fetch_quota_usage(http, account, request_timeout)
        .await
        .map_err(|error| qoder_quota_failure(error, now_ms))?;
    parse_qoder_quota_update(account, &body, now_ms, success_cooldown_ms)
}

fn qoder_quota_failure(
    error: crate::clients::oauth::qoder::QoderClientError,
    now_ms: i64,
) -> QuotaRefreshFailure {
    let upstream_status = error.upstream_status.map(|status| status.as_u16());
    let retryable = upstream_status.is_none_or(|status| status == 429 || status >= 500);
    QuotaRefreshFailure {
        status_code: error.status.as_u16(),
        upstream_status,
        message: crate::logging::redact_sensitive_text(&error.message),
        retryable,
        next_refresh_at: retryable.then_some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        partial_update: None,
    }
}

#[derive(Debug, Clone)]
struct QoderQuotaBucket {
    name: &'static str,
    label: &'static str,
    source: &'static str,
    total: Option<f64>,
    used: Option<f64>,
    remaining: Option<f64>,
    utilization: Option<f64>,
    unit: Option<String>,
    available: Option<bool>,
}

impl QoderQuotaBucket {
    fn capacity(&self) -> Option<f64> {
        self.total
    }

    fn tier(&self, resets_at: Option<i64>) -> AccountQuotaTier {
        AccountQuotaTier {
            name: self.name.to_string(),
            label: Some(self.label.to_string()),
            utilization: self.utilization,
            used: self.used,
            limit: self.total,
            unit: self.unit.clone().or_else(|| Some("credits".to_string())),
            resets_at,
            ..Default::default()
        }
    }

    fn projection(&self) -> Value {
        json!({
            "source": self.source,
            "total": self.total,
            "used": self.used,
            "remaining": self.remaining,
            "utilization": self.utilization,
            "unit": self.unit,
            "available": self.available,
        })
    }
}

fn parse_qoder_quota_update(
    account: &Account,
    body: &Value,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let object = body.as_object().ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::QoderCosy,
            "top-level value must be an object",
            now_ms,
        )
    })?;
    let user_type = qoder_optional_string(object, "userType", now_ms)?;
    let usage_type = qoder_optional_string(object, "usageType", now_ms)?;
    let exceeded = qoder_optional_bool(object, "isQuotaExceeded", now_ms)?;
    let total_usage_percentage =
        qoder_optional_nonnegative_number(object, "totalUsagePercentage", now_ms)?
            .map(qoder_percentage_fraction);
    let expires_at = qoder_optional_timestamp(object, "expiresAt", now_ms)?;

    let mut buckets = Vec::new();
    if let Some((source, value)) = qoder_alias_value(object, &["userQuota"], now_ms)? {
        buckets.push(parse_qoder_quota_bucket(
            "qoder_user",
            "Qoder personal credits",
            source,
            value,
            now_ms,
        )?);
    }
    if let Some((source, value)) =
        qoder_alias_value(object, &["addOnQuota", "add_on_quota"], now_ms)?
    {
        buckets.push(parse_qoder_quota_bucket(
            "qoder_add_on",
            "Qoder add-on credits",
            source,
            value,
            now_ms,
        )?);
    }
    if let Some((source, value)) = qoder_alias_value(
        object,
        &[
            "orgResourcePackage",
            "org_resource_package",
            "sharedQuota",
            "shared_quota",
        ],
        now_ms,
    )? {
        buckets.push(parse_qoder_quota_bucket(
            "qoder_organization",
            "Qoder organization/shared credits",
            source,
            value,
            now_ms,
        )?);
    }
    if buckets.is_empty() {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::QoderCosy,
            "response contains no recognized quota bucket",
            now_ms,
        ));
    }

    let positive_remaining = buckets
        .iter()
        .filter_map(|bucket| bucket.remaining)
        .any(|remaining| remaining > 0.0);
    let all_remaining_known = buckets.iter().all(|bucket| bucket.remaining.is_some());
    let total_remaining = all_remaining_known.then(|| {
        buckets
            .iter()
            .filter_map(|bucket| bucket.remaining)
            .sum::<f64>()
    });
    let total_capacity = buckets
        .iter()
        .filter_map(QoderQuotaBucket::capacity)
        .sum::<f64>();
    let personal_zero_unknown = user_type
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("personal_standard"))
        && buckets
            .iter()
            .find(|bucket| bucket.name == "qoder_user")
            .is_some_and(|bucket| {
                bucket.total == Some(0.0)
                    && bucket.remaining == Some(0.0)
                    && total_capacity <= 0.0
                    && !positive_remaining
            });
    let exhausted = if positive_remaining || personal_zero_unknown || !all_remaining_known {
        false
    } else {
        total_remaining == Some(0.0) && (exceeded == Some(true) || total_capacity > 0.0)
    };
    let availability = if exhausted {
        "exhausted"
    } else if positive_remaining {
        "available"
    } else {
        "unknown"
    };
    let tiers = buckets
        .iter()
        .map(|bucket| bucket.tier(expires_at))
        .collect::<Vec<_>>();
    let aggregate_utilization = if total_capacity > 0.0 && all_remaining_known {
        total_remaining.map(|remaining| (1.0 - remaining / total_capacity).clamp(0.0, 1.0))
    } else {
        total_usage_percentage
    };
    let plan = user_type
        .as_deref()
        .filter(|value| !value.is_empty())
        .map(|value| format!("Qoder {}", value.replace('_', " ")))
        .or_else(|| account.subscription_level.clone())
        .or_else(|| Some("Qoder".to_string()));
    let bucket_projection = buckets
        .iter()
        .map(|bucket| (bucket.name.to_string(), bucket.projection()))
        .collect::<serde_json::Map<String, Value>>();
    let quota = AccountQuota {
        success: true,
        credential_message: plan.clone(),
        tiers,
        extra_usage: Some(json!({
            "source": "qoder_cosy_quota_usage",
            "qoderQuota": {
                "userType": user_type,
                "usageType": usage_type,
                "totalUsagePercentage": total_usage_percentage,
                "isQuotaExceeded": exceeded,
                "expiresAt": expires_at,
                "availability": availability,
                "exhausted": exhausted,
                "personalZeroUnknown": personal_zero_unknown,
                "totalCapacity": (total_capacity > 0.0).then_some(total_capacity),
                "totalRemaining": total_remaining,
                "buckets": bucket_projection,
            }
        })),
    };
    let mut update = update_from_quota(quota, plan, None, now_ms, success_cooldown_ms);
    update.quota_percent = aggregate_utilization.map(|value| value * 100.0);
    if let Some(reset_at) = expires_at.filter(|reset_at| *reset_at > now_ms) {
        if exhausted {
            update.rate_limited_until = Some(reset_at);
        } else {
            update.clear_rate_limited_until_if = Some(reset_at);
        }
    }
    Ok(update)
}

fn parse_qoder_quota_bucket(
    name: &'static str,
    label: &'static str,
    source: &'static str,
    value: &Value,
    now_ms: i64,
) -> Result<QoderQuotaBucket, QuotaRefreshFailure> {
    let object = value.as_object().ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::QoderCosy,
            format!("{source} must be an object"),
            now_ms,
        )
    })?;
    let explicit_total = qoder_optional_nonnegative_number(object, "total", now_ms)?;
    let cap = qoder_optional_nonnegative_number(object, "cap", now_ms)?;
    let mut used = qoder_optional_nonnegative_number(object, "used", now_ms)?;
    let mut remaining = qoder_optional_nonnegative_number(object, "remaining", now_ms)?;
    let total = explicit_total.or(cap).or(match (used, remaining) {
        (Some(used), Some(remaining)) => Some(used + remaining),
        _ => None,
    });
    if used.is_none() {
        used = match (total, remaining) {
            (Some(total), Some(remaining)) if remaining <= total => Some(total - remaining),
            _ => None,
        };
    }
    if remaining.is_none() {
        remaining = match (total, used) {
            (Some(total), Some(used)) if used <= total => Some(total - used),
            _ => None,
        };
    }
    if matches!((total, used), (Some(total), Some(used)) if used > total)
        || matches!((total, remaining), (Some(total), Some(remaining)) if remaining > total)
    {
        return Err(QuotaRefreshFailure::parse(
            ProviderType::QoderCosy,
            format!("{source} contains usage greater than its capacity"),
            now_ms,
        ));
    }
    let percentage = qoder_optional_nonnegative_number(object, "percentage", now_ms)?;
    let utilization = percentage
        .map(qoder_percentage_fraction)
        .or_else(|| match (used, total) {
            (Some(used), Some(total)) if total > 0.0 => Some((used / total).clamp(0.0, 1.0)),
            _ => None,
        });
    let unit = qoder_optional_string(object, "unit", now_ms)?;
    let available = qoder_optional_bool(object, "available", now_ms)?;
    Ok(QoderQuotaBucket {
        name,
        label,
        source,
        total,
        used,
        remaining,
        utilization,
        unit,
        available,
    })
}

fn qoder_alias_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    aliases: &[&'static str],
    now_ms: i64,
) -> Result<Option<(&'static str, &'a Value)>, QuotaRefreshFailure> {
    let mut selected: Option<(&'static str, &Value)> = None;
    for alias in aliases {
        let Some(value) = object.get(*alias).filter(|value| !value.is_null()) else {
            continue;
        };
        if let Some((selected_alias, selected_value)) = selected {
            if selected_value != value {
                return Err(QuotaRefreshFailure::parse(
                    ProviderType::QoderCosy,
                    format!("conflicting {selected_alias} and {alias} quota aliases"),
                    now_ms,
                ));
            }
        } else {
            selected = Some((*alias, value));
        }
    }
    Ok(selected)
}

fn qoder_optional_nonnegative_number(
    object: &serde_json::Map<String, Value>,
    field: &str,
    now_ms: i64,
) -> Result<Option<f64>, QuotaRefreshFailure> {
    let Some(value) = object.get(field).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let parsed = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .filter(|value: &f64| value.is_finite() && *value >= 0.0)
        .ok_or_else(|| {
            QuotaRefreshFailure::parse(
                ProviderType::QoderCosy,
                format!("{field} must be a finite non-negative number"),
                now_ms,
            )
        })?;
    Ok(Some(parsed))
}

fn qoder_optional_string(
    object: &serde_json::Map<String, Value>,
    field: &str,
    now_ms: i64,
) -> Result<Option<String>, QuotaRefreshFailure> {
    let Some(value) = object.get(field).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let value = value.as_str().ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::QoderCosy,
            format!("{field} must be a string"),
            now_ms,
        )
    })?;
    Ok(Some(value.trim().to_string()))
}

fn qoder_optional_bool(
    object: &serde_json::Map<String, Value>,
    field: &str,
    now_ms: i64,
) -> Result<Option<bool>, QuotaRefreshFailure> {
    let Some(value) = object.get(field).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    value.as_bool().map(Some).ok_or_else(|| {
        QuotaRefreshFailure::parse(
            ProviderType::QoderCosy,
            format!("{field} must be a boolean"),
            now_ms,
        )
    })
}

fn qoder_optional_timestamp(
    object: &serde_json::Map<String, Value>,
    field: &str,
    now_ms: i64,
) -> Result<Option<i64>, QuotaRefreshFailure> {
    let Some(value) = object.get(field).filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let parsed = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .and_then(timestamp_number_to_unix_ms)
        .ok_or_else(|| {
            QuotaRefreshFailure::parse(
                ProviderType::QoderCosy,
                format!("{field} must be a positive Unix timestamp"),
                now_ms,
            )
        })?;
    Ok(Some(parsed))
}

fn qoder_percentage_fraction(value: f64) -> f64 {
    if value <= 1.0 {
        value.clamp(0.0, 1.0)
    } else {
        (value / 100.0).clamp(0.0, 1.0)
    }
}

async fn refresh_kimi_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let identity = crate::domain::kimi_cli::device_identity_from_profile(account.profile.as_ref())
        .ok_or_else(|| {
            QuotaRefreshFailure::bad_request(
                "kimi_code account is missing its account-scoped device identity",
            )
        })?;
    #[cfg(test)]
    let usage_url = account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("testKimiUsagesUrl"))
        .and_then(Value::as_str)
        .unwrap_or(crate::domain::kimi_cli::KIMI_USAGES_URL);
    #[cfg(not(test))]
    let usage_url = crate::domain::kimi_cli::KIMI_USAGES_URL;
    let mut request = http
        .get(usage_url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .timeout(request_timeout);
    for (name, value) in identity.headers() {
        request = request.header(name, value);
    }
    let body = request_json(ProviderType::KimiCode, request, now_ms).await?;
    let quota = parse_kimi_usage_quota(&body, now_ms)?;
    let subscription_level = quota.credential_message.clone();
    let has_quota = !quota.tiers.is_empty();
    let mut update =
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms);
    update
        .capability_observations
        .push(AccountCapabilityObservationDraft::kimi_feature(
            MODEL_ENTITLEMENT_DIMENSION,
            if has_quota {
                AccountCapabilityObservationState::Supported
            } else {
                AccountCapabilityObservationState::Unknown
            },
            "coding_v1_usages",
            (!has_quota).then_some("usage_response_has_no_quota_windows"),
            now_ms,
        ));
    Ok(update)
}

fn parse_kimi_usage_quota(body: &Value, now_ms: i64) -> Result<AccountQuota, QuotaRefreshFailure> {
    let mut tiers = Vec::new();
    if let Some(usage) = body.get("usage") {
        push_kimi_count_tier(&mut tiers, "weekly", "Weekly", usage);
    }
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        for (index, limit) in limits.iter().enumerate() {
            let detail = limit.get("detail").unwrap_or(limit);
            let label = string_at(limit, &["/window/name", "/window/type", "/name", "/type"])
                .unwrap_or_else(|| format!("Rate limit {}", index.saturating_add(1)));
            push_kimi_count_tier(
                &mut tiers,
                &format!("rate_limit_{}", index.saturating_add(1)),
                &label,
                detail,
            );
        }
    }
    for (name, label) in [("five_hour", "Session (5h)"), ("seven_day", "Weekly (7d)")] {
        if let Some(window) = body.get(name) {
            push_kimi_utilization_tier(&mut tiers, name, label, window);
        }
    }
    if let Some(object) = body.as_object() {
        for (name, value) in object {
            let Some(model) = name.strip_prefix("seven_day_") else {
                continue;
            };
            if model.is_empty() {
                continue;
            }
            push_kimi_utilization_tier(&mut tiers, name, &format!("Weekly {model} (7d)"), value);
        }
    }
    tiers.sort_by(|left, right| left.name.cmp(&right.name));
    tiers.dedup_by(|left, right| left.name == right.name);
    let membership = string_at(
        body,
        &["/user/membership/level", "/membership/level", "/plan"],
    );
    let plan = membership
        .as_deref()
        .map(kimi_plan_name)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Kimi Coding".to_string());
    Ok(AccountQuota {
        success: true,
        credential_message: Some(plan.clone()),
        tiers,
        extra_usage: Some(json!({
            "source": "coding_v1_usages",
            "plan": plan,
            "queriedAt": now_ms,
        })),
    })
}

fn push_kimi_count_tier(tiers: &mut Vec<AccountQuotaTier>, name: &str, label: &str, value: &Value) {
    let Some(limit) = number_at(value, &["/limit", "/Limit"]).filter(|value| *value > 0.0) else {
        return;
    };
    let used = number_at(value, &["/used", "/Used"])
        .or_else(|| {
            number_at(value, &["/remaining", "/Remaining"])
                .map(|remaining| (limit - remaining).max(0.0))
        })
        .unwrap_or(0.0)
        .clamp(0.0, limit);
    tiers.push(AccountQuotaTier {
        name: name.to_string(),
        label: Some(label.to_string()),
        utilization: Some((used / limit).clamp(0.0, 1.0)),
        used: Some(used),
        limit: Some(limit),
        unit: Some("requests".to_string()),
        resets_at: kimi_reset_at(value),
        ..Default::default()
    });
}

fn push_kimi_utilization_tier(
    tiers: &mut Vec<AccountQuotaTier>,
    name: &str,
    label: &str,
    value: &Value,
) {
    let Some(raw) = number_at(value, &["/utilization"]) else {
        return;
    };
    let utilization = if raw > 1.0 {
        percent_to_fraction(raw)
    } else {
        raw.clamp(0.0, 1.0)
    };
    tiers.push(AccountQuotaTier {
        name: name.to_string(),
        label: Some(label.to_string()),
        utilization: Some(utilization),
        used: None,
        limit: None,
        unit: Some("percent".to_string()),
        resets_at: kimi_reset_at(value),
        ..Default::default()
    });
}

fn kimi_reset_at(value: &Value) -> Option<i64> {
    string_at(
        value,
        &[
            "/resetTime",
            "/ResetTime",
            "/reset_at",
            "/resetAt",
            "/resets_at",
        ],
    )
    .as_deref()
    .and_then(rfc3339_to_unix_ms)
    .or_else(|| {
        number_at(
            value,
            &[
                "/resetTime",
                "/ResetTime",
                "/reset_at",
                "/resetAt",
                "/resets_at",
            ],
        )
        .and_then(timestamp_number_to_unix_ms)
    })
}

fn kimi_plan_name(level: &str) -> String {
    match level.trim().to_ascii_uppercase().as_str() {
        "LEVEL_BASIC" => "Moderato".to_string(),
        "LEVEL_INTERMEDIATE" => "Allegretto".to_string(),
        "LEVEL_ADVANCED" => "Allegro".to_string(),
        "LEVEL_STANDARD" => "Vivace".to_string(),
        value => value
            .strip_prefix("LEVEL_")
            .unwrap_or(value)
            .to_ascii_lowercase(),
    }
}

async fn refresh_codex_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let request_workspace_id = codex_account_id(account);
    let mut trusted_workspace = crate::domain::accounts::store::trusted_codex_workspace(account);
    let usage_request = codex_authenticated_get(
        http,
        &format!("{CHATGPT_USAGE_URL}?supports_rewardless_invites=true"),
        access_token,
        request_workspace_id.as_deref(),
        request_timeout,
    );
    let (body, reset_credit_details) = tokio::join!(
        request_json(account.provider_type, usage_request, now_ms),
        fetch_reset_credit_details(
            http,
            access_token,
            request_workspace_id.as_deref(),
            request_timeout,
        ),
    );
    let body = body?;
    let usage: CodexUsageResponse = serde_json::from_value(body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let previous_reset_credits = codex_banked_reset_status_from_account(account);
    let reset_credits = merge_reset_credit_snapshot(
        parse_usage_available_count(&body),
        reset_credit_details,
        previous_reset_credits.as_ref(),
        request_workspace_id.as_deref(),
        now_ms,
    );

    let usage_plan_type = usage
        .plan_type
        .as_deref()
        .map(normalize_chatgpt_plan_type)
        .filter(|value| !value.is_empty());
    let usage_plan_label = usage_plan_type.as_deref().map(format_chatgpt_plan_label);
    let usage_allowed = usage
        .rate_limit
        .as_ref()
        .and_then(|rate_limit| rate_limit.allowed);
    let usage_limit_reached = usage
        .rate_limit
        .as_ref()
        .and_then(|rate_limit| rate_limit.limit_reached);
    let personal_credits = usage
        .credits
        .as_ref()
        .map(codex_personal_credits_projection);
    let signed_recovery = if trusted_workspace.is_none() {
        recover_signed_codex_workspace(http, account, now_ms).await
    } else {
        None
    };
    let legacy_workspace_id = legacy_codex_workspace_candidate(account);
    let mut profile_update = None;
    let discovery_workspace_id = trusted_workspace
        .as_ref()
        .map(|workspace| workspace.id.clone())
        .or_else(|| legacy_workspace_id.clone())
        .or_else(|| {
            signed_recovery
                .as_ref()
                .map(|(workspace, _)| workspace.id.clone())
        });
    let mut account_probe_workspace_id = discovery_workspace_id.clone();
    let mut account_probe = fetch_chatgpt_account_lookup(
        http,
        access_token,
        discovery_workspace_id.as_deref(),
        now_ms,
        request_timeout,
    )
    .await;
    if trusted_workspace.is_none() {
        let authenticated =
            if chatgpt_probe_matches_usage(&account_probe, usage_plan_type.as_deref()) {
                discovery_workspace_id
                    .as_ref()
                    .zip(account_probe.lookup.clone())
                    .map(|(workspace_id, lookup)| ChatGptWorkspaceCandidate {
                        workspace_id: workspace_id.clone(),
                        lookup,
                    })
            } else {
                unique_chatgpt_workspace_matching_usage(&account_probe, usage_plan_type.as_deref())
            };
        if let Some(authenticated) = authenticated {
            account_probe_workspace_id = Some(authenticated.workspace_id.clone());
            account_probe.status = ChatGptProbeStatus::Success;
            account_probe.lookup = Some(authenticated.lookup);
            if let Some((workspace, profile)) = signed_recovery
                .as_ref()
                .filter(|(workspace, _)| workspace.id == authenticated.workspace_id)
            {
                trusted_workspace = Some(workspace.clone());
                profile_update = profile.clone();
            } else {
                let (workspace, profile) = authenticated_codex_workspace_update(
                    account,
                    &authenticated.workspace_id,
                    now_ms,
                );
                profile_update = profile;
                trusted_workspace = Some(workspace);
            }
        }
    }
    let subscription_request_workspace_id = trusted_workspace
        .as_ref()
        .map(|workspace| workspace.id.clone())
        .or_else(|| account_probe_workspace_id.clone());
    let subscription_probe = fetch_chatgpt_subscription_lookup(
        http,
        access_token,
        subscription_request_workspace_id.as_deref(),
        request_timeout,
    )
    .await;
    if trusted_workspace.is_none()
        && chatgpt_probe_matches_usage(&subscription_probe, usage_plan_type.as_deref())
    {
        if let Some((workspace, profile)) = signed_recovery.as_ref().filter(|(workspace, _)| {
            subscription_request_workspace_id.as_deref() == Some(workspace.id.as_str())
        }) {
            trusted_workspace = Some(workspace.clone());
            profile_update = profile.clone();
        } else if legacy_workspace_id.as_deref() == subscription_request_workspace_id.as_deref() {
            let workspace_id = subscription_request_workspace_id
                .as_deref()
                .expect("subscription discovery workspace was checked");
            let (workspace, profile) =
                authenticated_codex_workspace_update(account, workspace_id, now_ms);
            trusted_workspace = Some(workspace);
            profile_update = profile;
        }
    }
    let trusted_workspace_id = trusted_workspace
        .as_ref()
        .map(|workspace| workspace.id.as_str());
    let account_lookup_plan_type = account_probe
        .lookup
        .as_ref()
        .and_then(|lookup| lookup.plan_type.clone());
    let subscription_lookup_plan_type = subscription_probe
        .lookup
        .as_ref()
        .and_then(|lookup| lookup.plan_type.clone());
    let resolution = reconcile_chatgpt_subscription(
        usage_plan_type.as_deref(),
        usage_allowed,
        trusted_workspace.is_some(),
        account_probe.lookup.clone(),
        subscription_probe.lookup.clone(),
        now_ms,
    );
    if !resolution.discarded_reasons.is_empty() {
        tracing::warn!(
            account_id = %account.id,
            request_workspace_id = ?request_workspace_id,
            trusted_workspace_id = ?trusted_workspace_id,
            usage_plan_type = ?usage_plan_type,
            discarded_reasons = ?resolution.discarded_reasons,
            "discarded inconsistent ChatGPT subscription metadata"
        );
    }
    let (subscription, expiry_snapshot) = finalize_codex_subscription(
        account,
        resolution.subscription,
        trusted_workspace.as_ref(),
        usage_plan_type.as_deref(),
        &account_probe,
        &subscription_probe,
        now_ms,
    );
    let subscription_level = usage_plan_label.or_else(|| {
        subscription
            .as_ref()
            .and_then(|item| item.plan_label.clone())
    });
    let expiry_availability = subscription
        .as_ref()
        .and_then(|item| item.expiry_availability.as_deref());
    let expiry_warning_code = match expiry_availability {
        Some("workspace_unverified") => Some("codex_subscription_workspace_unverified"),
        Some("probe_unavailable") => Some("codex_subscription_probe_unavailable"),
        _ => None,
    };
    let subscription_json = subscription.as_ref().map(|item| {
        json!({
            "planType": item.plan_type,
            "planLabel": item.plan_label,
            "expiresAt": item.expires_at,
            "expiresSource": item.expires_source,
            "expiresKind": item.expires_kind,
            "expiryCapability": "automatic",
            "expiryAvailability": item.expiry_availability,
            "expiryStale": item.expiry_stale,
        })
    });

    let mut tiers = codex_tiers_from_rate_limit(usage.rate_limit);
    tiers.extend(codex_review_tiers_from_usage(&body));

    let quota = AccountQuota {
        success: true,
        credential_message: subscription_level.clone(),
        tiers,
        extra_usage: Some(json!({
            "raw": body,
            "subscription": subscription_json,
            "subscriptionEvidence": {
                "requestWorkspaceId": request_workspace_id,
                "trustedWorkspaceId": trusted_workspace_id,
                "trustedWorkspaceSource": trusted_workspace.as_ref().map(|workspace| &workspace.source),
                "workspaceVerified": trusted_workspace.is_some(),
                "usagePlanType": usage_plan_type,
                "usageAllowed": usage_allowed,
                "usageLimitReached": usage_limit_reached,
                "accountsCheckWorkspaceId": account_probe_workspace_id,
                "accountsCheckWorkspaceCandidateCount": account_probe.workspace_candidates.len(),
                "accountsCheckPlanType": account_lookup_plan_type,
                "accountsCheckStatus": account_probe.status.as_str(),
                "accountsCheckHttpStatus": account_probe.http_status,
                "subscriptionsRequestWorkspaceId": subscription_request_workspace_id,
                "subscriptionsPlanType": subscription_lookup_plan_type,
                "subscriptionsStatus": subscription_probe.status.as_str(),
                "subscriptionsHttpStatus": subscription_probe.http_status,
                "discardedReasons": resolution.discarded_reasons,
            },
            "subscriptionExpirySnapshot": expiry_snapshot,
            "personalCredits": personal_credits,
            "bankedReset": reset_credits,
            "warningCodes": expiry_warning_code.into_iter().collect::<Vec<_>>(),
            "queriedAt": now_ms,
        })),
    };
    Ok(update_from_quota(
        quota,
        subscription_level,
        profile_update,
        now_ms,
        success_cooldown_ms,
    ))
}

async fn refresh_claude_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let usage_request = http
        .get(CLAUDE_USAGE_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(ACCEPT, "application/json")
        .header("accept-language", "*")
        .header(USER_AGENT, claude_code_user_agent())
        .header("x-app", "cli")
        .timeout(request_timeout);
    let (body, profile_lookup, bootstrap_profile, roles_profile) = tokio::join!(
        request_json(account.provider_type, usage_request, now_ms),
        fetch_claude_profile_lookup(http, access_token, request_timeout, now_ms),
        fetch_claude_bootstrap_profile_with_timeout(http, access_token, request_timeout, now_ms,),
        fetch_claude_roles_profile(http, access_token, request_timeout, now_ms),
    );
    let body = body?;
    let subscription = resolve_claude_quota_subscription(
        account,
        &body,
        profile_lookup.as_ref(),
        bootstrap_profile.as_ref(),
        now_ms,
    );
    let quota = parse_claude_quota(&body, subscription.as_ref(), now_ms);
    let profile = merge_claude_profile_enrichments(
        account.profile.as_ref(),
        roles_profile,
        bootstrap_profile,
        profile_lookup.and_then(|lookup| lookup.profile_overlay),
    );
    Ok(update_from_claude_quota(
        quota,
        profile,
        now_ms,
        success_cooldown_ms,
    ))
}

pub async fn fetch_claude_bootstrap_profile(
    http: &reqwest::Client,
    access_token: &str,
    request_timeout_ms: i64,
    now_ms: i64,
) -> Option<Value> {
    fetch_claude_bootstrap_profile_with_timeout(
        http,
        access_token,
        quota_request_timeout(request_timeout_ms),
        now_ms,
    )
    .await
}

async fn fetch_claude_bootstrap_profile_with_timeout(
    http: &reqwest::Client,
    access_token: &str,
    request_timeout: Duration,
    now_ms: i64,
) -> Option<Value> {
    let response = match http
        .get(CLAUDE_BOOTSTRAP_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(ACCEPT, "application/json")
        .header("accept-language", "*")
        .header(USER_AGENT, claude_cli_user_agent())
        .header("x-app", "cli")
        .timeout(request_timeout)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => {
            crate::metrics::record_claude_bootstrap("network_error");
            return None;
        }
    };
    if !response.status().is_success() {
        crate::metrics::record_claude_bootstrap("http_error");
        return None;
    }
    let mut response = response;
    let body = match read_claude_control_json(&mut response).await {
        Ok(body) => body,
        Err(_) => {
            crate::metrics::record_claude_bootstrap("parse_error");
            return None;
        }
    };
    let profile = normalize_claude_bootstrap_profile(&body, now_ms);
    crate::metrics::record_claude_bootstrap(if profile.is_some() {
        "success"
    } else {
        "empty"
    });
    profile
}

async fn fetch_claude_roles_profile(
    http: &reqwest::Client,
    access_token: &str,
    request_timeout: Duration,
    now_ms: i64,
) -> Option<Value> {
    let response = http
        .get(CLAUDE_ROLES_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json, text/plain, */*")
        .header(USER_AGENT, claude_axios_user_agent())
        .timeout(request_timeout)
        .send()
        .await;
    let mut response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(_) => {
            crate::metrics::record_claude_roles("http_error");
            return None;
        }
        Err(_) => {
            crate::metrics::record_claude_roles("network_error");
            return None;
        }
    };
    let roles = match read_claude_control_json(&mut response).await {
        Ok(roles) => roles,
        Err(_) => {
            crate::metrics::record_claude_roles("parse_error");
            return None;
        }
    };
    crate::metrics::record_claude_roles("success");
    Some(json!({
        "claudeCliRoles": roles,
        "rolesRefreshedAt": now_ms,
    }))
}

async fn read_claude_control_json(response: &mut reqwest::Response) -> Result<Value, String> {
    let body = crate::infra::http::read_response_body_limited(
        response,
        MAX_CLAUDE_CONTROL_RESPONSE_BODY_BYTES,
    )
    .await
    .map_err(|error| error.to_string())?;
    serde_json::from_slice(&body).map_err(|error| error.to_string())
}

fn normalize_claude_bootstrap_profile(body: &Value, now_ms: i64) -> Option<Value> {
    let source = body
        .get("oauth_account")
        .or_else(|| body.get("account"))
        .unwrap_or(body);
    let mut profile = serde_json::Map::new();
    let mappings = [
        ("accountUUID", "account_uuid"),
        ("email", "account_email"),
        ("organizationUUID", "organization_uuid"),
        ("organizationName", "organization_name"),
        ("organizationType", "organization_type"),
        ("organizationRateLimitTier", "organization_rate_limit_tier"),
    ];
    for (target, source_key) in mappings {
        if let Some(value) = source
            .get(source_key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            profile.insert(target.to_string(), Value::String(value.to_string()));
        }
    }
    if profile.is_empty() {
        return None;
    }
    profile.insert("bootstrapRefreshedAt".to_string(), json!(now_ms));
    if profile.contains_key("organizationType") {
        profile.insert("organizationTypeObservedAt".to_string(), json!(now_ms));
    }
    if profile.contains_key("organizationRateLimitTier") {
        profile.insert(
            "organizationRateLimitTierObservedAt".to_string(),
            json!(now_ms),
        );
    }
    Some(Value::Object(profile))
}

fn merge_profile_overlay(existing: Option<&Value>, overlay: Option<Value>) -> Option<Value> {
    let overlay = overlay?;
    let mut merged = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(overlay) = overlay.as_object() {
        for (key, value) in overlay {
            merged.insert(key.clone(), value.clone());
        }
    }
    Some(Value::Object(merged))
}

fn merge_claude_profile_enrichments(
    existing: Option<&Value>,
    roles_profile: Option<Value>,
    bootstrap_profile: Option<Value>,
    profile_lookup: Option<Value>,
) -> Option<Value> {
    let mut profile = existing.cloned();
    for overlay in [roles_profile, bootstrap_profile, profile_lookup]
        .into_iter()
        .flatten()
    {
        profile = merge_profile_overlay(profile.as_ref(), Some(overlay));
    }
    profile
}

async fn refresh_gemini_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    refresh_gemini_v1internal_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
        .await
}

#[derive(Debug, Clone)]
struct GeminiV1InternalLoadResult {
    body: Value,
    project_id: Option<String>,
    subscription_level: Option<String>,
    profile: Option<Value>,
    observed_at_ms: i64,
}

impl GeminiV1InternalLoadResult {
    fn account_update(&self, provider_type: ProviderType) -> AccountRefreshUpdate {
        let mut update = AccountRefreshUpdate {
            profile: self.profile.clone(),
            subscription_level: self.subscription_level.clone(),
            capability_observations: vec![
                crate::domain::accounts::capability_evidence::AccountCapabilityObservationDraft::gemini_project(
                    self.project_id.is_some(),
                    self.observed_at_ms,
                    self.project_id
                        .is_none()
                        .then_some("load_code_assist_returned_no_project"),
                ),
            ],
            ..Default::default()
        };
        if is_antigravity_provider_type(provider_type) {
            update.capability_observations.extend([
                AccountCapabilityObservationDraft::antigravity_feature(
                    PROJECT_BOOTSTRAP_DIMENSION,
                    if self.project_id.is_some() {
                        AccountCapabilityObservationState::Supported
                    } else {
                        AccountCapabilityObservationState::Unknown
                    },
                    "load_code_assist",
                    self.project_id
                        .is_none()
                        .then_some("load_code_assist_returned_no_project"),
                    self.observed_at_ms,
                    None,
                ),
                AccountCapabilityObservationDraft::antigravity_feature(
                    TIER_ENTITLEMENT_DIMENSION,
                    if self.subscription_level.is_some() {
                        AccountCapabilityObservationState::Supported
                    } else {
                        AccountCapabilityObservationState::Unknown
                    },
                    "load_code_assist",
                    self.subscription_level
                        .is_none()
                        .then_some("load_code_assist_returned_no_tier"),
                    self.observed_at_ms,
                    None,
                ),
            ]);
        }
        update
    }
}

#[derive(Debug, Clone)]
struct GeminiV1InternalSelectedTier {
    source: &'static str,
    value: Value,
    label: String,
}

pub async fn load_gemini_v1internal_project(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    request_timeout_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    load_gemini_v1internal_project_with_timeout(
        http,
        account,
        now_ms,
        quota_request_timeout(request_timeout_ms),
    )
    .await
    .and_then(|loaded| {
        if loaded.project_id.is_some() {
            Ok(loaded.account_update(account.provider_type))
        } else {
            Err(
                QuotaRefreshFailure::missing_gemini_project(account.provider_type, now_ms)
                    .with_partial_update(loaded.account_update(account.provider_type)),
            )
        }
    })
}

async fn load_gemini_v1internal_project_with_timeout(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    request_timeout: Duration,
) -> Result<GeminiV1InternalLoadResult, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let metadata = gemini_code_assist_metadata(account.provider_type)?;
    let load_url = gemini_code_assist_url(account, "loadCodeAssist")?;
    let load_request = gemini_code_assist_post_request(
        http,
        account,
        &load_url,
        access_token,
        &json!({"metadata": metadata}),
        request_timeout,
        true,
    )?;
    let load_body = request_json(account.provider_type, load_request, now_ms).await?;
    let load: GeminiLoadCodeAssistResponse = serde_json::from_value(load_body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let project_id = load
        .cloudaicompanion_project
        .as_ref()
        .and_then(extract_project_id)
        .or_else(|| gemini_v1internal_project_id(account));
    let selected_tier = gemini_selected_tier(load.paid_tier.as_ref(), load.current_tier.as_ref());
    let subscription_level = selected_tier.as_ref().map(|tier| tier.label.clone());
    let profile =
        gemini_v1internal_profile_from_load(account, project_id.as_deref(), selected_tier.as_ref());
    Ok(GeminiV1InternalLoadResult {
        body: load_body,
        project_id,
        subscription_level,
        profile,
        observed_at_ms: now_ms,
    })
}

async fn refresh_gemini_v1internal_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let loaded =
        load_gemini_v1internal_project_with_timeout(http, account, now_ms, request_timeout).await?;
    let partial_update = loaded.account_update(account.provider_type);
    let access_token = required_access_token(account)?;
    let mut quota_body = json!({});
    if let Some(project_id) = loaded.project_id.as_deref() {
        quota_body["project"] = Value::String(project_id.to_string());
    }
    let quota_url = gemini_code_assist_url(account, "retrieveUserQuota")
        .map_err(|error| error.with_partial_update(partial_update.clone()))?;
    let quota_request = gemini_code_assist_post_request(
        http,
        account,
        &quota_url,
        access_token,
        &quota_body,
        request_timeout,
        false,
    )
    .map_err(|error| error.with_partial_update(partial_update.clone()))?;
    let body = request_json(account.provider_type, quota_request, now_ms)
        .await
        .map_err(|error| error.with_partial_update(partial_update.clone()))?;
    let quota_response: GeminiQuotaResponse =
        serde_json::from_value(body.clone()).map_err(|error| {
            QuotaRefreshFailure::parse(account.provider_type, error, now_ms)
                .with_partial_update(partial_update.clone())
        })?;
    let has_model_entitlement = quota_response.buckets.as_ref().is_some_and(|buckets| {
        buckets.iter().any(|bucket| {
            bucket
                .model_id
                .as_deref()
                .is_some_and(|model| !model.trim().is_empty())
        })
    });
    let quota = parse_gemini_quota(
        &quota_response,
        loaded.subscription_level.clone(),
        loaded.body,
        body,
        now_ms,
    );
    let mut update = update_from_quota(
        quota,
        loaded.subscription_level,
        loaded.profile,
        now_ms,
        success_cooldown_ms,
    );
    update
        .capability_observations
        .extend(partial_update.capability_observations);
    update.capability_observations.push(
        crate::domain::accounts::capability_evidence::AccountCapabilityObservationDraft::gemini_model_entitlement(
            has_model_entitlement,
            now_ms,
            now_ms.saturating_add(success_cooldown_ms.saturating_mul(2).max(60_000)),
        ),
    );
    if is_antigravity_provider_type(account.provider_type) {
        update
            .capability_observations
            .extend(antigravity_quota_observations(
                &quota_response,
                now_ms,
                now_ms.saturating_add(success_cooldown_ms.saturating_mul(2).max(60_000)),
            ));
        update.capability_observations.push(
            observe_antigravity_privacy(
                http,
                account,
                loaded.project_id.as_deref(),
                access_token,
                now_ms,
                request_timeout,
            )
            .await,
        );
    }
    Ok(update)
}

fn is_antigravity_provider_type(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
    )
}

fn antigravity_quota_observations(
    quota: &GeminiQuotaResponse,
    observed_at_ms: i64,
    expires_at_ms: i64,
) -> Vec<AccountCapabilityObservationDraft> {
    let buckets = quota.buckets.as_deref().unwrap_or_default();
    let explicit_buckets = buckets
        .iter()
        .filter(|bucket| {
            bucket
                .model_id
                .as_deref()
                .is_some_and(|model| !model.trim().is_empty())
        })
        .collect::<Vec<_>>();
    let family_observation = |dimension, family: &str| {
        let supported = explicit_buckets.iter().any(|bucket| {
            bucket
                .model_id
                .as_deref()
                .is_some_and(|model| model.trim().to_ascii_lowercase().starts_with(family))
        });
        AccountCapabilityObservationDraft::antigravity_feature(
            dimension,
            if supported {
                AccountCapabilityObservationState::Supported
            } else {
                AccountCapabilityObservationState::Unknown
            },
            "retrieve_user_quota",
            (!supported).then_some("quota_has_no_explicit_family_bucket"),
            observed_at_ms,
            Some(expires_at_ms),
        )
    };
    let remaining = explicit_buckets
        .iter()
        .filter_map(|bucket| bucket.remaining_fraction)
        .filter(|remaining| remaining.is_finite())
        .collect::<Vec<_>>();
    let (capacity_state, capacity_reason) = if remaining.iter().any(|remaining| *remaining > 0.0) {
        (AccountCapabilityObservationState::Supported, None)
    } else if !explicit_buckets.is_empty() && remaining.len() == explicit_buckets.len() {
        (
            AccountCapabilityObservationState::Unsupported,
            Some("all_explicit_model_buckets_exhausted"),
        )
    } else {
        (
            AccountCapabilityObservationState::Unknown,
            Some("quota_has_no_explicit_capacity"),
        )
    };

    vec![
        family_observation(GEMINI_QUOTA_FAMILY_DIMENSION, "gemini"),
        family_observation(CLAUDE_QUOTA_FAMILY_DIMENSION, "claude"),
        AccountCapabilityObservationDraft::antigravity_feature(
            MODEL_CAPACITY_DIMENSION,
            capacity_state,
            "retrieve_user_quota",
            capacity_reason,
            observed_at_ms,
            Some(expires_at_ms),
        ),
    ]
}

async fn observe_antigravity_privacy(
    http: &reqwest::Client,
    account: &Account,
    project_id: Option<&str>,
    access_token: &str,
    observed_at_ms: i64,
    request_timeout: Duration,
) -> AccountCapabilityObservationDraft {
    let Some(project_id) = project_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return antigravity_privacy_observation(
            AccountCapabilityObservationState::Unknown,
            "project_not_available",
            observed_at_ms,
        );
    };
    let url = match antigravity_fetch_user_info_url(account) {
        Ok(url) => url,
        Err(_) => {
            return antigravity_privacy_observation(
                AccountCapabilityObservationState::Unknown,
                "privacy_endpoint_invalid",
                observed_at_ms,
            )
        }
    };
    let request = http
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .header(
            USER_AGENT,
            crate::provider_identity::antigravity_user_agent(),
        )
        .timeout(request_timeout)
        .json(&json!({"project": project_id}));
    let body = match request_json(account.provider_type, request, observed_at_ms).await {
        Ok(body) => body,
        Err(error) => {
            let reason = match error.upstream_status {
                Some(401) => "privacy_probe_unauthorized",
                Some(403) => "privacy_probe_forbidden",
                Some(429) => "privacy_probe_rate_limited",
                Some(_) => "privacy_probe_upstream_error",
                None => "privacy_probe_unavailable",
            };
            return antigravity_privacy_observation(
                AccountCapabilityObservationState::Unknown,
                reason,
                observed_at_ms,
            );
        }
    };
    let Some(settings) = body.get("userSettings").and_then(Value::as_object) else {
        return antigravity_privacy_observation(
            AccountCapabilityObservationState::Unknown,
            "privacy_response_missing_user_settings",
            observed_at_ms,
        );
    };
    if settings.contains_key("telemetryEnabled") {
        antigravity_privacy_observation(
            AccountCapabilityObservationState::Unsupported,
            "telemetry_setting_present",
            observed_at_ms,
        )
    } else {
        antigravity_privacy_observation(
            AccountCapabilityObservationState::Supported,
            "telemetry_setting_absent",
            observed_at_ms,
        )
    }
}

fn antigravity_privacy_observation(
    state: AccountCapabilityObservationState,
    reason: &'static str,
    observed_at_ms: i64,
) -> AccountCapabilityObservationDraft {
    AccountCapabilityObservationDraft::antigravity_feature(
        PRIVACY_DIMENSION,
        state,
        "fetch_user_info_read_only",
        Some(reason),
        observed_at_ms,
        None,
    )
}

fn gemini_code_assist_metadata(provider_type: ProviderType) -> Result<Value, QuotaRefreshFailure> {
    match provider_type {
        ProviderType::GeminiCli => Ok(crate::provider_identity::gemini_cli_code_assist_metadata()),
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            Ok(crate::provider_identity::antigravity_client_metadata())
        }
        _ => Err(QuotaRefreshFailure::bad_request(format!(
            "{} does not use the Gemini Code Assist protocol",
            provider_type.as_str()
        ))),
    }
}

fn gemini_code_assist_post_request(
    http: &reqwest::Client,
    account: &Account,
    url: &str,
    access_token: &str,
    body: &Value,
    request_timeout: Duration,
    include_client_metadata: bool,
) -> Result<reqwest::RequestBuilder, QuotaRefreshFailure> {
    let request = http
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .timeout(request_timeout);
    let request = match account.provider_type {
        ProviderType::GeminiCli => request
            .header(
                USER_AGENT,
                crate::provider_identity::gemini_cli_user_agent(),
            )
            .header(
                "x-goog-api-client",
                crate::provider_identity::GEMINI_CLI_X_GOOG_API_CLIENT,
            ),
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            let request = request.header(
                USER_AGENT,
                crate::provider_identity::antigravity_user_agent(),
            );
            if include_client_metadata {
                request.header(
                    "client-metadata",
                    crate::provider_identity::antigravity_client_metadata().to_string(),
                )
            } else {
                request
            }
        }
        _ => {
            return Err(QuotaRefreshFailure::bad_request(format!(
                "{} does not use the Gemini Code Assist protocol",
                account.provider_type.as_str()
            )))
        }
    };
    Ok(request.json(body))
}

fn gemini_code_assist_url(
    account: &Account,
    operation: &'static str,
) -> Result<String, QuotaRefreshFailure> {
    #[cfg(test)]
    if let Some(raw) = account.raw.as_ref() {
        let explicit_key = match operation {
            "loadCodeAssist" => "testGeminiLoadCodeAssistUrl",
            "retrieveUserQuota" => "testGeminiRetrieveUserQuotaUrl",
            _ => "",
        };
        if let Some(url) = raw
            .get(explicit_key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return parse_test_gemini_code_assist_url(account.provider_type, url, None);
        }
        if let Some(base_url) = raw
            .get("testGeminiCodeAssistBaseUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return parse_test_gemini_code_assist_url(
                account.provider_type,
                base_url,
                Some(operation),
            );
        }
    }

    #[cfg(not(test))]
    let _ = account;
    match operation {
        "loadCodeAssist" => Ok(GEMINI_LOAD_CODE_ASSIST_URL.to_string()),
        "retrieveUserQuota" => Ok(GEMINI_RETRIEVE_USER_QUOTA_URL.to_string()),
        _ => Err(QuotaRefreshFailure::bad_request(format!(
            "unsupported Gemini Code Assist operation: {operation}"
        ))),
    }
}

fn antigravity_fetch_user_info_url(account: &Account) -> Result<String, QuotaRefreshFailure> {
    #[cfg(test)]
    if let Some(raw) = account.raw.as_ref() {
        let configured = raw
            .get("testAntigravityFetchUserInfoUrl")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| {
                raw.get("testGeminiCodeAssistBaseUrl")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|base| format!("{}/v1internal:fetchUserInfo", base.trim_end_matches('/')))
            });
        if let Some(configured) = configured {
            let mut url = reqwest::Url::parse(&configured).map_err(|error| {
                QuotaRefreshFailure::bad_request(format!(
                    "{} test Antigravity fetchUserInfo URL is invalid: {error}",
                    account.provider_type.as_str()
                ))
            })?;
            url.set_fragment(None);
            return Ok(url.to_string());
        }
    }

    #[cfg(not(test))]
    let _ = account;
    Ok(ANTIGRAVITY_FETCH_USER_INFO_URL.to_string())
}

#[cfg(test)]
fn parse_test_gemini_code_assist_url(
    provider_type: ProviderType,
    value: &str,
    operation: Option<&str>,
) -> Result<String, QuotaRefreshFailure> {
    let mut url = reqwest::Url::parse(value).map_err(|error| {
        QuotaRefreshFailure::bad_request(format!(
            "{} test Gemini Code Assist URL is invalid: {error}",
            provider_type.as_str()
        ))
    })?;
    if let Some(operation) = operation {
        let current_path = url.path().trim_end_matches('/');
        let path = if let Some((prefix, action)) = current_path.rsplit_once(':') {
            if matches!(action, "loadCodeAssist" | "retrieveUserQuota") {
                format!("{prefix}:{operation}")
            } else if current_path.ends_with("/v1internal") {
                format!("{current_path}:{operation}")
            } else {
                format!("{current_path}/v1internal:{operation}")
            }
        } else if current_path.ends_with("/v1internal") {
            format!("{current_path}:{operation}")
        } else {
            format!("{current_path}/v1internal:{operation}")
        };
        url.set_path(&path);
        url.set_query(None);
    }
    url.set_fragment(None);
    Ok(url.to_string())
}

fn gemini_selected_tier(
    paid_tier: Option<&Value>,
    current_tier: Option<&Value>,
) -> Option<GeminiV1InternalSelectedTier> {
    [("paidTier", paid_tier), ("currentTier", current_tier)]
        .into_iter()
        .find_map(|(source, value)| {
            let value = value.filter(|value| !value.is_null())?;
            let label = match value {
                Value::String(value) => {
                    let value = value.trim();
                    (!value.is_empty()).then(|| value.to_string())
                }
                Value::Object(_) => ["name", "id"].into_iter().find_map(|key| {
                    value
                        .get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string)
                }),
                _ => None,
            }?;
            Some(GeminiV1InternalSelectedTier {
                source,
                value: value.clone(),
                label,
            })
        })
}

fn gemini_v1internal_profile_from_load(
    account: &Account,
    project_id: Option<&str>,
    selected_tier: Option<&GeminiV1InternalSelectedTier>,
) -> Option<Value> {
    let mut overlay = serde_json::Map::new();
    if let Some(project_id) = project_id {
        overlay.insert(
            "projectId".to_string(),
            Value::String(project_id.to_string()),
        );
    }
    if let Some(selected_tier) = selected_tier {
        overlay.insert(
            selected_tier.source.to_string(),
            selected_tier.value.clone(),
        );
        overlay.insert("selectedTier".to_string(), selected_tier.value.clone());
        overlay.insert(
            "selectedTierSource".to_string(),
            Value::String(selected_tier.source.to_string()),
        );
        overlay.insert(
            "tier".to_string(),
            Value::String(selected_tier.label.clone()),
        );
        overlay.insert(
            "subscriptionTier".to_string(),
            Value::String(selected_tier.label.clone()),
        );
    }
    if project_id.is_some()
        && matches!(
            account.provider_type,
            ProviderType::AntigravityOAuth | ProviderType::AgyOAuth
        )
    {
        overlay.insert(
            "postExchangeEnrichment".to_string(),
            Value::String("project_loaded".to_string()),
        );
    }
    if overlay.is_empty() {
        account.profile.clone()
    } else {
        merge_profile_overlay(account.profile.as_ref(), Some(Value::Object(overlay)))
    }
}

async fn refresh_grok_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let (user_probe, weekly_probe, monthly_probe, task_probe) = tokio::join!(
        grok_probe_json(
            http,
            account,
            GROK_CLI_USER_URL,
            access_token,
            request_timeout,
            now_ms,
        ),
        grok_probe_json(
            http,
            account,
            GROK_CLI_WEEKLY_BILLING_URL,
            access_token,
            request_timeout,
            now_ms,
        ),
        grok_probe_json(
            http,
            account,
            GROK_CLI_MONTHLY_BILLING_URL,
            access_token,
            request_timeout,
            now_ms,
        ),
        grok_probe_json(
            http,
            account,
            GROK_TASK_USAGE_URL,
            access_token,
            request_timeout,
            now_ms,
        ),
    );
    let user_probe = user_probe?;
    let weekly_probe = grok_optional_probe("weekly_billing", weekly_probe, true);
    let monthly_probe = grok_optional_probe("monthly_billing", monthly_probe, true);
    let task_probe = grok_optional_probe("task_usage", task_probe, false);

    let needs_subscription_probe = grok_subscription_expiry_at(&user_probe).is_none()
        && weekly_probe
            .body
            .as_ref()
            .and_then(grok_subscription_expiry_at)
            .is_none()
        && monthly_probe
            .body
            .as_ref()
            .and_then(grok_subscription_expiry_at)
            .is_none();
    let subscription_probe = if needs_subscription_probe {
        grok_optional_probe(
            "subscriptions",
            grok_probe_json(
                http,
                account,
                GROK_SUBSCRIPTIONS_URL,
                access_token,
                request_timeout,
                now_ms,
            )
            .await,
            false,
        )
    } else {
        GrokProbe::skipped("subscription details already available")
    };

    let billing_body = weekly_probe.body.as_ref().or(monthly_probe.body.as_ref());
    let subscription_level = grok_subscription_level(&user_probe)
        .or_else(|| weekly_probe.body.as_ref().and_then(grok_subscription_level))
        .or_else(|| {
            monthly_probe
                .body
                .as_ref()
                .and_then(grok_subscription_level)
        })
        .or_else(|| {
            subscription_probe
                .body
                .as_ref()
                .and_then(grok_subscription_level)
        })
        .or_else(|| grok_access_plan(&user_probe, billing_body))
        .or_else(|| {
            account
                .subscription_level
                .as_deref()
                .and_then(canonical_grok_subscription_level)
        })
        .or_else(|| account.entitlement_status.clone());
    let previous_billing_tiers = account
        .quota
        .as_ref()
        .map(|quota| {
            quota
                .tiers
                .iter()
                .filter(|tier| tier.name.starts_with("grok_"))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let quota = grok_quota_from_probes(
        &user_probe,
        GrokQuotaProbes {
            weekly: &weekly_probe,
            monthly: &monthly_probe,
            task_usage: &task_probe,
            subscriptions: &subscription_probe,
        },
        subscription_level.clone(),
        now_ms,
        &previous_billing_tiers,
    );
    let quota_unavailable = quota
        .extra_usage
        .as_ref()
        .and_then(|extra| extra.get("quotaStatus"))
        .and_then(Value::as_str)
        == Some("unavailable");
    let profile = merge_profile_overlay(
        account.profile.as_ref(),
        Some(grok_profile_from_user_probe(
            &user_probe,
            billing_body,
            now_ms,
        )),
    );
    let mut update = update_from_quota(
        quota,
        subscription_level,
        profile,
        now_ms,
        success_cooldown_ms,
    );
    update
        .capability_observations
        .push(grok_media_entitlement_observation(
            &user_probe,
            &weekly_probe,
            &monthly_probe,
            &subscription_probe,
            now_ms,
        ));
    update.email = grok_email(&user_probe);
    update.entitlement_status = grok_entitlement_status(&user_probe);
    let issues = [
        weekly_probe.issue.as_ref(),
        monthly_probe.issue.as_ref(),
        task_probe.issue.as_ref(),
        subscription_probe.issue.as_ref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if !issues.is_empty() {
        if quota_unavailable {
            update.last_refresh_error = Some(
                issues
                    .iter()
                    .map(|issue| format!("{}: {}", issue.probe, issue.message))
                    .collect::<Vec<_>>()
                    .join("; "),
            );
        }
        if let Some(next_refresh_at) = issues
            .iter()
            .filter_map(|issue| issue.next_refresh_at)
            .min()
        {
            update.quota_next_refresh_at = Some(next_refresh_at);
        }
    }
    Ok(update)
}

#[derive(Debug)]
struct GrokProbeIssue {
    probe: &'static str,
    message: String,
    next_refresh_at: Option<i64>,
}

#[derive(Debug)]
struct GrokProbe {
    body: Option<Value>,
    issue: Option<GrokProbeIssue>,
    spending_limited: bool,
    status_code: Option<u16>,
    skipped_reason: Option<&'static str>,
}

impl GrokProbe {
    fn skipped(reason: &'static str) -> Self {
        Self {
            body: None,
            issue: None,
            spending_limited: false,
            status_code: None,
            skipped_reason: Some(reason),
        }
    }
}

fn grok_optional_probe(
    probe: &'static str,
    result: Result<Value, QuotaRefreshFailure>,
    treats_402_as_spending_limit: bool,
) -> GrokProbe {
    match result {
        Ok(body) => GrokProbe {
            body: Some(body),
            issue: None,
            spending_limited: false,
            status_code: Some(200),
            skipped_reason: None,
        },
        Err(error) if treats_402_as_spending_limit && error.status_code == 402 => GrokProbe {
            body: None,
            issue: None,
            spending_limited: true,
            status_code: Some(402),
            skipped_reason: None,
        },
        Err(error) => {
            let status_code = error.upstream_status.unwrap_or(error.status_code);
            GrokProbe {
                body: None,
                issue: Some(GrokProbeIssue {
                    probe,
                    message: error.message,
                    next_refresh_at: error.next_refresh_at,
                }),
                spending_limited: false,
                status_code: Some(status_code),
                skipped_reason: None,
            }
        }
    }
}

fn grok_media_entitlement_observation(
    user: &Value,
    weekly: &GrokProbe,
    monthly: &GrokProbe,
    subscriptions: &GrokProbe,
    now_ms: i64,
) -> AccountCapabilityObservationDraft {
    use AccountCapabilityObservationState::{Supported, Unknown, Unsupported};

    let (state, reason) = if [weekly, monthly]
        .into_iter()
        .any(|probe| probe.status_code == Some(403))
    {
        (Unsupported, "billing_probe_forbidden")
    } else {
        let current_values = [
            Some(user),
            weekly.body.as_ref(),
            monthly.body.as_ref(),
            subscriptions.body.as_ref(),
        ];
        let paid_access = current_values
            .into_iter()
            .flatten()
            .any(grok_value_reports_paid_access);
        let explicit_free = current_values
            .into_iter()
            .flatten()
            .any(grok_value_reports_free_plan);
        let spending_exhausted = user
            .pointer("/spendingLimitReached")
            .or_else(|| user.pointer("/spending_limit_reached"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || weekly.spending_limited
            || monthly.spending_limited
            || weekly
                .body
                .as_ref()
                .is_some_and(|body| grok_billing_reports_exhausted(body, paid_access))
            || monthly
                .body
                .as_ref()
                .is_some_and(|body| grok_billing_reports_exhausted(body, paid_access));
        let authoritative_quota = [weekly.body.as_ref(), monthly.body.as_ref()]
            .into_iter()
            .flatten()
            .any(grok_has_authoritative_media_quota_shape);
        let complete_billing = weekly.status_code == Some(200)
            && weekly.body.is_some()
            && monthly.status_code == Some(200)
            && monthly.body.is_some();

        if paid_access && authoritative_quota {
            (Supported, "paid_access_with_authoritative_quota")
        } else if spending_exhausted && !paid_access {
            (Unsupported, "spending_exhausted_without_subscription")
        } else if explicit_free && complete_billing {
            (Unsupported, "free_plan_with_complete_billing")
        } else if paid_access {
            (Unknown, "paid_access_without_authoritative_quota")
        } else if weekly.issue.is_some()
            || monthly.issue.is_some()
            || weekly.body.is_none()
            || monthly.body.is_none()
        {
            (Unknown, "billing_probe_incomplete")
        } else {
            (Unknown, "no_authoritative_media_entitlement")
        }
    };

    AccountCapabilityObservationDraft::grok_feature(
        MEDIA_ENTITLEMENT_DIMENSION,
        state,
        "grok_quota_probes",
        Some(reason),
        now_ms,
    )
}

fn grok_value_reports_paid_access(value: &Value) -> bool {
    let explicit_access = [
        "/hasGrokCodeAccess",
        "/has_grok_code_access",
        "/hasGrokBuildAccess",
        "/has_grok_build_access",
        "/entitlement/hasGrokCodeAccess",
        "/entitlement/has_grok_code_access",
        "/config/isUnifiedBillingUser",
        "/config/is_unified_billing_user",
        "/isUnifiedBillingUser",
    ]
    .into_iter()
    .any(|pointer| value.pointer(pointer).and_then(Value::as_bool) == Some(true));
    explicit_access
        || grok_subscription_level(value)
            .as_deref()
            .is_some_and(grok_plan_name_is_paid)
}

fn grok_value_reports_free_plan(value: &Value) -> bool {
    grok_subscription_level(value)
        .as_deref()
        .is_some_and(grok_plan_name_is_free)
}

fn grok_plan_name_is_paid(plan: &str) -> bool {
    let normalized = plan.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    !normalized.is_empty()
        && !matches!(
            normalized.as_str(),
            "free" | "none" | "null" | "unknown" | "unsubscribed" | "no_subscription"
        )
}

fn grok_plan_name_is_free(plan: &str) -> bool {
    let normalized = plan.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "free" | "none" | "null" | "unsubscribed" | "no_subscription"
    )
}

fn grok_has_authoritative_media_quota_shape(body: &Value) -> bool {
    grok_weekly_billing_tiers(body, true)
        .into_iter()
        .chain(grok_monthly_billing_tiers(body))
        .any(|tier| tier.name != "grok_spending_limit")
}

async fn grok_probe_json(
    http: &reqwest::Client,
    account: &Account,
    url: &str,
    access_token: &str,
    request_timeout: Duration,
    now_ms: i64,
) -> Result<Value, QuotaRefreshFailure> {
    let mut attempt = 0_u64;
    loop {
        attempt += 1;
        let mut request = http
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {access_token}"))
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, grok_cli_user_agent())
            .header("x-xai-token-auth", GROK_CLI_TOKEN_AUTH)
            .header("x-grok-client-identifier", GROK_CLI_CLIENT_IDENTIFIER)
            .header("x-grok-client-version", grok_cli_version())
            .header("x-grok-client-surface", "grok-cli")
            .header("x-grok-client-mode", "headless")
            .timeout(request_timeout);
        if let Some(email) = account
            .email
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            request = request.header("x-email", email);
        }
        if let Some(user_id) = grok_account_user_id(account) {
            request = request.header("x-userid", user_id);
        }
        match request_json(account.provider_type, request, now_ms).await {
            Err(error) if error.retryable && error.status_code == 502 && attempt < 3 => {
                tokio::time::sleep(Duration::from_millis(250 * attempt)).await;
            }
            result => return result,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct GrokQuotaProbes<'a> {
    weekly: &'a GrokProbe,
    monthly: &'a GrokProbe,
    task_usage: &'a GrokProbe,
    subscriptions: &'a GrokProbe,
}

fn grok_quota_from_probes(
    user: &Value,
    probes: GrokQuotaProbes<'_>,
    subscription_level: Option<String>,
    now_ms: i64,
    previous_billing_tiers: &[AccountQuotaTier],
) -> AccountQuota {
    let subscription_level = subscription_level
        .as_deref()
        .and_then(canonical_grok_subscription_level);
    let GrokQuotaProbes {
        weekly,
        monthly,
        task_usage,
        subscriptions,
    } = probes;
    let subscription_access = grok_subscription_level(user)
        .or_else(|| weekly.body.as_ref().and_then(grok_subscription_level))
        .or_else(|| monthly.body.as_ref().and_then(grok_subscription_level))
        .or_else(|| {
            subscriptions
                .body
                .as_ref()
                .and_then(grok_subscription_level)
        })
        .is_some_and(|tier| {
            !matches!(tier.to_ascii_lowercase().as_str(), "free" | "none" | "null")
        });
    let spending_limited =
        user.pointer("/spendingLimitReached")
            .or_else(|| user.pointer("/spending_limit_reached"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || weekly.spending_limited
            || monthly.spending_limited
            || weekly.body.as_ref().is_some_and(|billing| {
                grok_billing_reports_exhausted(billing, subscription_access)
            })
            || monthly.body.as_ref().is_some_and(|billing| {
                grok_billing_reports_exhausted(billing, subscription_access)
            });
    let mut tiers = Vec::new();
    if let Some(body) = weekly.body.as_ref() {
        merge_grok_tiers(
            &mut tiers,
            grok_weekly_billing_tiers(body, subscription_access),
        );
    }
    if let Some(body) = monthly.body.as_ref() {
        merge_grok_tiers(&mut tiers, grok_monthly_billing_tiers(body));
    }
    if let Some(body) = task_usage.body.as_ref() {
        merge_grok_tiers(&mut tiers, grok_task_usage_tiers(body));
    }
    let current_tier_count = tiers.len();
    let mut stale_tier_names = Vec::new();
    for previous in previous_billing_tiers {
        let should_preserve = (weekly.body.is_none() && grok_tier_is_weekly(&previous.name))
            || (monthly.body.is_none() && grok_tier_is_monthly(&previous.name))
            || (task_usage.body.is_none() && grok_tier_is_task(&previous.name));
        if should_preserve && !tiers.iter().any(|tier| tier.name == previous.name) {
            stale_tier_names.push(previous.name.clone());
            tiers.push(previous.clone());
        }
    }
    if spending_limited && tiers.is_empty() {
        tiers.push(AccountQuotaTier {
            name: "grok_spending_limit".to_string(),
            utilization: Some(1.0),
            resets_at: Some(now_ms.saturating_add(60 * 60_000)),
            ..Default::default()
        });
    }
    let quota_issues = [
        weekly.issue.as_ref(),
        monthly.issue.as_ref(),
        task_usage.issue.as_ref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let issues = quota_issues
        .iter()
        .copied()
        .chain(subscriptions.issue.iter())
        .collect::<Vec<_>>();
    let quota_status = if spending_limited {
        "spending_limited"
    } else if weekly.body.is_none()
        && monthly.body.is_none()
        && current_tier_count == 0
        && stale_tier_names.is_empty()
    {
        "unavailable"
    } else if !quota_issues.is_empty() || !stale_tier_names.is_empty() {
        "partial"
    } else if current_tier_count > 0 {
        "valid_numeric"
    } else {
        "valid_non_numeric"
    };
    let mut warning_codes = if quota_status == "valid_non_numeric" {
        vec!["grok_numeric_quota_not_exposed"]
    } else if quota_status == "partial" {
        vec!["grok_quota_partial"]
    } else if quota_status == "unavailable" {
        vec!["grok_quota_unavailable"]
    } else {
        Vec::new()
    };
    if subscriptions.issue.is_some() {
        warning_codes.push("grok_subscription_expiry_unavailable");
    }
    let subscription = grok_subscription_json(
        user,
        weekly.body.as_ref(),
        monthly.body.as_ref(),
        subscriptions,
        subscription_level.clone(),
    );
    AccountQuota {
        success: quota_status != "unavailable",
        credential_message: subscription_level.clone(),
        tiers,
        extra_usage: Some(json!({
            "provider": "grok",
            "user": user,
            "weeklyBilling": weekly.body,
            "monthlyBilling": monthly.body,
            "quotaStatus": quota_status,
            "warningCodes": warning_codes,
            "warnings": issues.iter().map(|issue| format!("{}: {}", issue.probe, issue.message)).collect::<Vec<_>>(),
            "staleTierNames": stale_tier_names,
            "probes": {
                "weeklyBilling": grok_probe_metadata(weekly),
                "monthlyBilling": grok_probe_metadata(monthly),
                "taskUsage": grok_probe_metadata(task_usage),
                "subscriptions": grok_probe_metadata(subscriptions),
            },
            "spendingLimitReached": spending_limited,
            "subscription": subscription,
            "queriedAt": now_ms,
        })),
    }
}

fn grok_probe_metadata(probe: &GrokProbe) -> Value {
    json!({
        "ok": probe.body.is_some(),
        "statusCode": probe.status_code,
        "spendingLimited": probe.spending_limited,
        "error": probe.issue.as_ref().map(|issue| issue.message.as_str()),
        "skippedReason": probe.skipped_reason,
    })
}

fn grok_weekly_billing_tiers(body: &Value, subscription_access: bool) -> Vec<AccountQuotaTier> {
    let resets_at = grok_billing_reset_at(body);
    let mut tiers = Vec::new();
    if let Some(percent) = grok_number_at(
        body,
        &[
            "/config/creditUsagePercent",
            "/config/credit_usage_percent",
            "/creditUsagePercent",
            "/credit_usage_percent",
        ],
    ) {
        tiers.push(grok_percentage_tier(
            "grok_weekly",
            Some("Weekly credits".to_string()),
            percent,
            resets_at,
        ));
    }
    let products = body
        .pointer("/config/productUsage")
        .or_else(|| body.pointer("/config/product_usage"))
        .or_else(|| body.get("productUsage"))
        .or_else(|| body.get("product_usage"))
        .and_then(Value::as_array);
    if let Some(products) = products {
        for product in products {
            let Some(label) = string_at(product, &["/product", "/name", "/productName"]) else {
                continue;
            };
            let percent = grok_number_at(
                product,
                &[
                    "/usagePercent",
                    "/usage_percent",
                    "/usedPercent",
                    "/used_percent",
                ],
            )
            .or_else(|| {
                let (used, total, _) = grok_credit_bag_amounts(product)?;
                let total = total.filter(|total| *total > 0.0)?;
                Some(used.unwrap_or(0.0) / total * 100.0)
            });
            if let Some(percent) = percent {
                tiers.push(grok_percentage_tier(
                    &format!("grok_product_{}", grok_tier_slug(&label)),
                    Some(label),
                    percent,
                    resets_at,
                ));
            }
        }
    }
    if !tiers.iter().any(|tier| tier.name == "grok_weekly") {
        for pointer in [
            "/credits",
            "/creditBalance",
            "/usage",
            "/config/credits",
            "/config/includedCredits",
            "/config/subscriptionCredits",
            "/config/weeklyCredits",
            "/config/sharedPool",
        ] {
            let Some(bag) = body.pointer(pointer) else {
                continue;
            };
            let Some((used, total, remaining)) = grok_credit_bag_amounts(bag) else {
                continue;
            };
            if let Some(total) = total.filter(|value| *value > 0.0) {
                let used = used
                    .or_else(|| remaining.map(|remaining| (total - remaining).max(0.0)))
                    .unwrap_or(0.0);
                tiers.push(grok_credit_tier("grok_weekly", used, total, resets_at));
                break;
            }
        }
    }
    merge_grok_tiers(&mut tiers, grok_monthly_billing_tiers(body));
    merge_grok_tiers(&mut tiers, grok_billing_tiers(body, subscription_access));
    tiers
}

fn grok_monthly_billing_tiers(body: &Value) -> Vec<AccountQuotaTier> {
    let limit_cents = grok_number_at(
        body,
        &[
            "/config/monthlyLimit",
            "/config/monthly_limit",
            "/monthlyLimit",
            "/monthly_limit",
        ],
    );
    let used_cents = grok_number_at(
        body,
        &[
            "/config/used",
            "/used",
            "/config/includedUsed",
            "/config/included_used",
            "/includedUsed",
            "/included_used",
            "/config/totalUsed",
            "/config/total_used",
            "/totalUsed",
            "/total_used",
        ],
    );
    let Some(limit_cents) = limit_cents.filter(|value| *value > 0.0) else {
        return Vec::new();
    };
    let limit = limit_cents / 100.0;
    let used = used_cents.unwrap_or(0.0).clamp(0.0, limit_cents) / 100.0;
    vec![AccountQuotaTier {
        name: "grok_monthly".to_string(),
        label: Some("Monthly included".to_string()),
        utilization: Some((used / limit).clamp(0.0, 1.0)),
        used: Some(used),
        limit: Some(limit),
        unit: Some("USD".to_string()),
        resets_at: grok_billing_reset_at(body),
        ..Default::default()
    }]
}

fn grok_task_usage_tiers(body: &Value) -> Vec<AccountQuotaTier> {
    let mut tiers = Vec::new();
    for (name, label, used_key, limit_key) in [
        (
            "grok_frequent",
            "Frequent tasks",
            "frequentUsage",
            "frequentLimit",
        ),
        (
            "grok_occasional",
            "Occasional tasks",
            "occasionalUsage",
            "occasionalLimit",
        ),
    ] {
        let used = grok_nested_number(body, used_key);
        let limit = grok_nested_number(body, limit_key);
        if let Some(limit) = limit.filter(|value| *value > 0.0) {
            let used = used.unwrap_or(0.0).max(0.0);
            let mut tier = grok_credit_tier(name, used, limit, None);
            tier.label = Some(label.to_string());
            tier.unit = Some("tasks".to_string());
            tiers.push(tier);
        }
    }
    tiers
}

fn grok_billing_reset_at(body: &Value) -> Option<i64> {
    grok_timestamp_at(
        body,
        &[
            "/config/currentPeriod/end",
            "/config/billingPeriodEnd",
            "/config/billing_period_end",
            "/config/resetAt",
            "/config/resetsAt",
            "/config/periodEnd",
            "/billingPeriodEnd",
            "/billing_period_end",
            "/resetAt",
            "/reset_at",
            "/resetsAt",
            "/resets_at",
            "/periodEnd",
            "/period_end",
            "/usage/resetAt",
            "/data/resetAt",
        ],
    )
}

fn grok_percentage_tier(
    name: &str,
    label: Option<String>,
    percent: f64,
    resets_at: Option<i64>,
) -> AccountQuotaTier {
    AccountQuotaTier {
        name: name.to_string(),
        label,
        utilization: Some((percent / 100.0).clamp(0.0, 1.0)),
        resets_at,
        ..Default::default()
    }
}

fn grok_credit_bag_amounts(value: &Value) -> Option<(Option<f64>, Option<f64>, Option<f64>)> {
    if let Some(items) = value.as_array() {
        return items.iter().find_map(grok_credit_bag_amounts);
    }
    let object = value.as_object()?;
    let total = grok_number_from_object(object, &["total", "limit", "cap", "allocation", "amount"]);
    let used = grok_number_from_object(object, &["used", "spent", "consumed", "usage"]);
    let remaining = grok_number_from_object(object, &["remaining", "balance", "left"]);
    if total.is_none() && used.is_none() && remaining.is_none() {
        return object
            .get("bags")
            .or_else(|| object.get("items"))
            .and_then(grok_credit_bag_amounts);
    }
    let used = used.or_else(|| match (total, remaining) {
        (Some(total), Some(remaining)) => Some((total - remaining).max(0.0)),
        _ => None,
    });
    let remaining = remaining.or_else(|| match (total, used) {
        (Some(total), Some(used)) => Some((total - used).max(0.0)),
        _ => None,
    });
    Some((used, total, remaining))
}

fn grok_number_from_object(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(grok_number_value))
}

fn grok_number_value(value: &Value) -> Option<f64> {
    let value = value.get("val").unwrap_or(value);
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        .filter(|value| value.is_finite())
}

fn grok_nested_number(value: &Value, key: &str) -> Option<f64> {
    match value {
        Value::Object(object) => object.get(key).and_then(grok_number_value).or_else(|| {
            object
                .values()
                .find_map(|value| grok_nested_number(value, key))
        }),
        Value::Array(items) => items
            .iter()
            .find_map(|value| grok_nested_number(value, key)),
        _ => None,
    }
}

fn grok_tier_slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    if slug.is_empty() {
        "unknown".to_string()
    } else {
        slug
    }
}

fn merge_grok_tiers(target: &mut Vec<AccountQuotaTier>, incoming: Vec<AccountQuotaTier>) {
    for tier in incoming {
        if let Some(existing) = target
            .iter_mut()
            .find(|existing| existing.name == tier.name)
        {
            *existing = tier;
        } else {
            target.push(tier);
        }
    }
}

fn grok_tier_is_weekly(name: &str) -> bool {
    matches!(
        name,
        "grok_weekly" | "grok_credits" | "grok_on_demand" | "grok_prepaid" | "grok_spending_limit"
    ) || name.starts_with("grok_product_")
}

fn grok_tier_is_monthly(name: &str) -> bool {
    name == "grok_monthly"
}

fn grok_tier_is_task(name: &str) -> bool {
    matches!(name, "grok_frequent" | "grok_occasional")
}

fn grok_subscription_json(
    user: &Value,
    weekly: Option<&Value>,
    monthly: Option<&Value>,
    subscriptions: &GrokProbe,
    subscription_level: Option<String>,
) -> Value {
    let sources = [
        ("grok_subscriptions", subscriptions.body.as_ref()),
        ("grok_user", Some(user)),
        ("grok_weekly_billing", weekly),
        ("grok_monthly_billing", monthly),
    ];
    let expiry = sources.iter().find_map(|(source, value)| {
        let expires_at = grok_subscription_expiry_at((*value)?)?;
        let expires_at = Utc.timestamp_millis_opt(expires_at).single()?.to_rfc3339();
        Some((expires_at, *source))
    });
    let status = sources
        .iter()
        .find_map(|(_, value)| grok_subscription_status((*value)?));
    json!({
        "planType": subscription_level.clone(),
        "planLabel": subscription_level,
        "status": status,
        "expiresAt": expiry.as_ref().map(|(expires_at, _)| expires_at),
        "expiresSource": expiry.as_ref().map(|(_, source)| source),
        "expiresKind": expiry.as_ref().map(|_| "subscription"),
        "expiryCapability": "automatic_or_manual",
        "expiryAvailability": if expiry.is_some() {
            "available"
        } else if subscriptions.issue.is_some() {
            "probe_unavailable"
        } else {
            "upstream_not_provided"
        },
    })
}

fn grok_subscription_expiry_at(value: &Value) -> Option<i64> {
    grok_subscription_object(value)
        .and_then(|subscription| {
            grok_timestamp_at(
                subscription,
                &[
                    "/expiresAt",
                    "/expires_at",
                    "/activeUntil",
                    "/active_until",
                    "/subscriptionExpiresAt",
                    "/subscription_expires_at",
                    "/endAt",
                    "/end_at",
                ],
            )
        })
        .or_else(|| {
            // A top-level generic expiresAt is commonly the OAuth token expiry.
            // Only the subscription-qualified root field is trusted here.
            grok_timestamp_at(
                value,
                &["/subscriptionExpiresAt", "/subscription_expires_at"],
            )
        })
}

fn grok_subscription_status(value: &Value) -> Option<String> {
    let subscription = grok_subscription_object(value)?;
    string_at(
        subscription,
        &["/status", "/subscriptionStatus", "/subscription_status"],
    )
}

fn grok_subscription_object(value: &Value) -> Option<&Value> {
    if let Some(user) = value.get("user").filter(|value| value.is_object()) {
        if let Some(subscription) = grok_subscription_object(user) {
            return Some(subscription);
        }
    }
    if let Some(subscription) = value.get("subscription").filter(|value| value.is_object()) {
        return grok_subscription_is_current(subscription).then_some(subscription);
    }
    for subscriptions in [
        value.get("subscriptions"),
        value.pointer("/config/subscriptions"),
        value.pointer("/data/subscriptions"),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_array)
    {
        if let Some(active) = subscriptions
            .iter()
            .find(|subscription| grok_subscription_status_is_active(subscription))
        {
            return Some(active);
        }
        if let Some(without_status) = subscriptions.iter().find(|subscription| {
            string_at(
                subscription,
                &["/status", "/subscriptionStatus", "/subscription_status"],
            )
            .is_none()
        }) {
            return Some(without_status);
        }
    }
    if grok_subscription_status_is_active(value) {
        return Some(value);
    }
    None
}

fn grok_subscription_is_current(value: &Value) -> bool {
    let status = string_at(
        value,
        &["/status", "/subscriptionStatus", "/subscription_status"],
    );
    status
        .as_deref()
        .map(grok_status_name_is_active)
        .unwrap_or(true)
}

fn grok_subscription_status_is_active(value: &Value) -> bool {
    string_at(
        value,
        &["/status", "/subscriptionStatus", "/subscription_status"],
    )
    .as_deref()
    .is_some_and(grok_status_name_is_active)
}

fn grok_status_name_is_active(status: &str) -> bool {
    let normalized = status.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "active" | "subscription_status_active" | "trialing" | "subscription_status_trialing"
    )
}

fn grok_billing_tiers(body: &Value, subscription_access: bool) -> Vec<AccountQuotaTier> {
    let resets_at = grok_billing_reset_at(body);
    let on_demand_cap = grok_number_at(
        body,
        &[
            "/config/onDemandCap",
            "/config/on_demand_cap",
            "/onDemandCap",
            "/on_demand_cap",
        ],
    );
    let on_demand_used = grok_number_at(
        body,
        &[
            "/config/onDemandUsed",
            "/config/on_demand_used",
            "/onDemandUsed",
            "/on_demand_used",
        ],
    );
    let prepaid_balance = grok_number_at(
        body,
        &[
            "/config/prepaidBalance",
            "/config/prepaid_balance",
            "/prepaidBalance",
            "/prepaid_balance",
        ],
    );
    let mut tiers = Vec::new();
    if let Some(limit) = on_demand_cap.filter(|value| *value > 0.0) {
        tiers.push(grok_credit_tier(
            "grok_on_demand",
            on_demand_used.unwrap_or(0.0).max(0.0),
            limit,
            resets_at,
        ));
    } else if !subscription_access && on_demand_cap == Some(0.0) && on_demand_used.is_some() {
        tiers.push(grok_credit_tier("grok_spending_limit", 1.0, 1.0, resets_at));
    }
    if let Some(balance) = prepaid_balance.filter(|value| *value > 0.0) {
        tiers.push(grok_credit_tier("grok_prepaid", 0.0, balance, None));
    }
    if !tiers.is_empty() {
        return tiers;
    }

    grok_legacy_billing_tier(body).into_iter().collect()
}

fn grok_legacy_billing_tier(body: &Value) -> Option<AccountQuotaTier> {
    let used = grok_number_at(
        body,
        &[
            "/used",
            "/creditsUsed",
            "/credits_used",
            "/usage/used",
            "/data/used",
        ],
    );
    let limit = grok_number_at(
        body,
        &[
            "/limit",
            "/creditsLimit",
            "/credits_limit",
            "/usage/limit",
            "/data/limit",
        ],
    );
    let remaining = grok_number_at(
        body,
        &[
            "/remaining",
            "/creditsRemaining",
            "/credits_remaining",
            "/usage/remaining",
            "/data/remaining",
        ],
    );
    let inferred_used = used.or_else(|| match (limit, remaining) {
        (Some(limit), Some(remaining)) if limit.is_finite() && remaining.is_finite() => {
            Some((limit - remaining).max(0.0))
        }
        _ => None,
    });
    let utilization = match (inferred_used, limit) {
        (Some(used), Some(limit)) if limit > 0.0 => Some((used / limit).clamp(0.0, 10_000.0)),
        _ => grok_number_at(
            body,
            &["/utilization", "/usage/utilization", "/data/utilization"],
        )
        .map(|value| if value > 1.0 { value / 100.0 } else { value }),
    };
    let resets_at = grok_timestamp_at(
        body,
        &[
            "/resetAt",
            "/reset_at",
            "/resetsAt",
            "/resets_at",
            "/periodEnd",
            "/period_end",
            "/billingPeriodEnd",
            "/billing_period_end",
            "/usage/resetAt",
            "/data/resetAt",
        ],
    );
    (inferred_used.is_some() || limit.is_some() || utilization.is_some()).then(|| {
        AccountQuotaTier {
            name: "grok_credits".to_string(),
            label: Some("Credits".to_string()),
            utilization,
            used: inferred_used,
            limit,
            unit: Some("credits".to_string()),
            resets_at,
            ..Default::default()
        }
    })
}

fn grok_credit_tier(name: &str, used: f64, limit: f64, resets_at: Option<i64>) -> AccountQuotaTier {
    AccountQuotaTier {
        name: name.to_string(),
        label: None,
        utilization: (limit > 0.0).then(|| (used / limit).clamp(0.0, 1.0)),
        used: Some(used),
        limit: Some(limit),
        unit: Some("credits".to_string()),
        resets_at,
        ..Default::default()
    }
}

fn grok_number_at(value: &Value, pointers: &[&str]) -> Option<f64> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(grok_number_value))
}

fn grok_billing_reports_exhausted(body: &Value, subscription_access: bool) -> bool {
    !subscription_access
        && grok_number_at(
            body,
            &[
                "/config/onDemandCap",
                "/config/on_demand_cap",
                "/onDemandCap",
                "/on_demand_cap",
            ],
        ) == Some(0.0)
        && grok_number_at(
            body,
            &[
                "/config/onDemandUsed",
                "/config/on_demand_used",
                "/onDemandUsed",
                "/on_demand_used",
            ],
        )
        .is_some()
}

fn grok_timestamp_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        match value {
            Value::String(value) => dateish_to_unix_ms(value).or_else(|| {
                value
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .and_then(timestamp_number_to_unix_ms)
            }),
            Value::Number(value) => value.as_f64().and_then(timestamp_number_to_unix_ms),
            _ => None,
        }
    })
}

fn grok_subscription_level(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/subscriptionTier",
            "/subscription_tier",
            "/tier",
            "/entitlement/tier",
            "/subscription/tier",
            "/user/subscriptionTier",
            "/user/subscription_tier",
            "/config/subscriptionTier",
            "/config/subscription_tier",
            "/data/subscriptionTier",
            "/data/tier",
        ],
    )
    .or_else(|| {
        grok_subscription_object(value).and_then(|subscription| {
            string_at(
                subscription,
                &["/tier", "/subscriptionTier", "/subscription_tier", "/plan"],
            )
        })
    })
    .and_then(|value| canonical_grok_subscription_level(&value))
}

fn grok_access_plan(user: &Value, billing: Option<&Value>) -> Option<String> {
    if user
        .pointer("/hasGrokCodeAccess")
        .or_else(|| user.pointer("/has_grok_code_access"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return Some("Grok Code".to_string());
    }
    billing
        .and_then(|billing| {
            billing
                .pointer("/config/isUnifiedBillingUser")
                .or_else(|| billing.pointer("/config/is_unified_billing_user"))
                .or_else(|| billing.pointer("/isUnifiedBillingUser"))
                .and_then(Value::as_bool)
        })
        .filter(|value| *value)
        .map(|_| "Grok Build".to_string())
}

fn grok_account_user_id(account: &Account) -> Option<String> {
    account
        .profile
        .as_ref()
        .and_then(|value| {
            string_at(
                value,
                &[
                    "/userId",
                    "/principalId",
                    "/sub",
                    "/claims/sub",
                    "/grokUser/userId",
                ],
            )
        })
        .or_else(|| {
            account.raw.as_ref().and_then(|value| {
                string_at(value, &["/userId", "/principalId", "/sub", "/claims/sub"])
            })
        })
}

fn grok_email(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/email",
            "/preferredUsername",
            "/preferred_username",
            "/user/email",
            "/profile/email",
            "/data/email",
            "/data/preferredUsername",
        ],
    )
}

fn grok_entitlement_status(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/entitlementStatus",
            "/entitlement_status",
            "/entitlement/status",
            "/data/entitlementStatus",
            "/data/entitlement_status",
        ],
    )
}

fn grok_profile_from_user_probe(user: &Value, billing: Option<&Value>, now_ms: i64) -> Value {
    json!({
        "grokUser": user,
        "grokBilling": billing,
        "quotaRefreshedAt": now_ms,
    })
}

async fn refresh_antigravity_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    refresh_gemini_v1internal_quota(http, account, now_ms, success_cooldown_ms, request_timeout)
        .await
}

fn refresh_imported_snapshot_quota(
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let quota = match account.provider_type {
        ProviderType::KiroOAuth => parse_kiro_imported_quota(account, now_ms),
        ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
            parse_cursor_imported_quota(account, now_ms)
        }
        provider_type => {
            return Err(QuotaRefreshFailure::bad_request(format!(
                "{} imported quota snapshot is not supported",
                provider_type.as_str()
            )))
        }
    }?;
    let subscription_level = quota.credential_message.clone();
    let mut update =
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms);
    if account.provider_type == ProviderType::KiroOAuth {
        update.quota_percent = update.quota.as_ref().and_then(agentic_quota_percent);
    }
    Ok(update)
}

async fn refresh_cursor_dashboard_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(access_token) = access_token else {
        // Compatibility-only path for historical imports that contain a
        // captured usage snapshot but no usable bearer credential. New and
        // refreshable OAuth accounts always use the live Dashboard path below.
        let quota = parse_cursor_imported_quota(account, now_ms)?;
        let subscription_level = quota.credential_message.clone();
        let mut update = update_from_quota(
            quota,
            subscription_level.clone(),
            None,
            now_ms,
            success_cooldown_ms,
        );
        update.subscription_level = subscription_level;
        return Ok(update);
    };
    let dashboard_request = async {
        crate::clients::oauth::cursor_dashboard::fetch_cursor_dashboard_snapshot(
            http,
            access_token,
            request_timeout,
        )
        .await
    };
    let presentation_request = async {
        crate::clients::oauth::cursor::fetch_cursor_oauth_presentation(http, access_token).await
    };
    let (snapshot, presentation) = tokio::join!(dashboard_request, presentation_request);
    let presentation = presentation.ok().flatten();
    let subscription_level = snapshot
        .subscription_level()
        .or_else(|| {
            presentation
                .as_ref()
                .and_then(|value| value.subscription_level.clone())
        })
        .or_else(|| account.subscription_level.clone());
    if !snapshot.has_data() {
        let error = snapshot.errors.first().cloned().unwrap_or_else(|| {
            crate::clients::oauth::cursor_dashboard::CursorDashboardError {
                status_code: None,
                retryable: true,
                retry_after_ms: None,
                message: "Cursor dashboard returned no account information".to_string(),
            }
        });
        return Err(QuotaRefreshFailure {
            status_code: match error.status_code {
                Some(401 | 403) => 400,
                Some(429) => 429,
                Some(status) if status >= 500 => 502,
                _ => 502,
            },
            upstream_status: error.status_code,
            message: error.message,
            retryable: error.retryable,
            next_refresh_at: Some(
                now_ms.saturating_add(
                    error
                        .retry_after_ms
                        .and_then(|value| i64::try_from(value).ok())
                        .unwrap_or(QUOTA_FAILURE_COOLDOWN_MS),
                ),
            ),
            partial_update: presentation.map(|presentation| {
                let mut profile = account.profile.clone().unwrap_or_else(|| json!({}));
                profile["accountDisplay"] = json!({
                    "email": presentation.email,
                    "displayName": presentation.display_name,
                    "credentialName": presentation.credential_name,
                    "subscriptionLevel": presentation.subscription_level,
                });
                Box::new(AccountRefreshUpdate {
                    email: profile["accountDisplay"]["email"]
                        .as_str()
                        .map(str::to_string),
                    profile: Some(profile),
                    subscription_level: subscription_level.clone(),
                    ..Default::default()
                })
            }),
        });
    }

    let safe_dashboard = snapshot.safe_profile();
    let quota = snapshot
        .account_quota(now_ms)
        .unwrap_or_else(|| AccountQuota {
            success: true,
            credential_message: subscription_level.clone(),
            tiers: Vec::new(),
            extra_usage: Some(json!({
                "source": "cursor_dashboard_api",
                "queriedAt": now_ms,
                "dashboard": safe_dashboard.clone(),
                "partial": true,
            })),
        });
    let mut update = update_from_quota(
        quota,
        subscription_level.clone(),
        None,
        now_ms,
        success_cooldown_ms,
    );
    update.subscription_level = subscription_level;
    let mut profile = account.profile.clone().unwrap_or_else(|| json!({}));
    if let Some(presentation) = presentation {
        update.email = presentation.email.clone();
        profile["accountDisplay"] = json!({
            "email": presentation.email,
            "displayName": presentation.display_name,
            "credentialName": presentation.credential_name,
            "subscriptionLevel": presentation.subscription_level,
        });
    }
    profile["cursorDashboard"] = safe_dashboard;
    profile["cursorDashboardObservedAt"] = json!(now_ms);
    update.profile = Some(profile);
    Ok(update)
}

async fn refresh_ollama_cloud_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let token = account
        .api_key
        .as_deref()
        .or(account.access_token.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            QuotaRefreshFailure::bad_request("ollama_cloud account requires an api key")
        })?;
    let request = http
        .post(OLLAMA_ME_URL)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header(CONTENT_TYPE, "application/json")
        .timeout(request_timeout);
    let body = request_json(account.provider_type, request, now_ms).await?;
    Ok(parse_ollama_me_update(&body, now_ms, success_cooldown_ms))
}

async fn refresh_kiro_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = account
        .access_token
        .as_deref()
        .or(account.api_key.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            QuotaRefreshFailure::bad_request("kiro_oauth access token or api key is required")
        })?;
    let raw = account.raw.as_ref();
    let profile = account.profile.as_ref();
    let runtime = kiro_quota_runtime_identity(account)?;
    let machine_id = raw
        .and_then(|value| string_at(value, &["/machineId", "/machine_id"]))
        .or_else(|| profile.and_then(|value| string_at(value, &["/machineId", "/machine_id"])))
        .or_else(|| {
            account
                .refresh_token
                .as_deref()
                .map(machine_id_from_refresh_token)
        })
        .unwrap_or_else(|| "kiro-api-key".to_string());
    let http = http.clone();
    let usage = tokio::time::timeout(
        request_timeout,
        fetch_usage_limits(
            &http,
            &runtime.runtime_region,
            runtime.profile_arn.as_deref(),
            &machine_id,
            access_token,
            kiro_quota_token_type(account),
        ),
    )
    .await
    .map_err(|_| QuotaRefreshFailure {
        status_code: 504,
        upstream_status: None,
        message: "kiro_oauth quota request timed out".to_string(),
        retryable: true,
        next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        partial_update: None,
    })?
    .map_err(|error| {
        QuotaRefreshFailure::upstream(
            account.provider_type,
            error.status,
            error.message,
            None,
            now_ms,
        )
    })?;
    let subscription_level = string_at(
        &usage,
        &[
            "/subscriptionInfo/subscriptionTitle",
            "/subscription_info/subscription_title",
        ],
    )
    .or_else(|| account.subscription_level.clone())
    .or_else(|| Some("Kiro OAuth".to_string()));
    let quota = quota_from_usage_limits(usage.clone(), subscription_level.clone(), now_ms);
    let mut raw = account
        .raw
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(object) = raw.as_object_mut() {
        object.insert("kiroUsageLimits".to_string(), usage);
        object.insert("quotaRefreshedAtMs".to_string(), Value::from(now_ms));
    }
    let quota_percent = agentic_quota_percent(&quota);
    let mut update =
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms)
            .with_raw(raw);
    update.quota_percent = quota_percent;
    Ok(update)
}

async fn refresh_amazon_q_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    #[cfg(test)]
    let endpoint_override = account
        .raw
        .as_ref()
        .and_then(|value| string_at(value, &["/testAmazonQRuntimeUrl"]));
    #[cfg(not(test))]
    let endpoint_override: Option<String> = None;
    let usage = crate::clients::oauth::amazon_q_runtime::usage_snapshot(
        http,
        account,
        endpoint_override.as_deref(),
        request_timeout,
    )
    .await
    .map_err(|message| QuotaRefreshFailure {
        status_code: 502,
        upstream_status: None,
        message: format!("amazon_q_oauth quota request failed: {message}"),
        retryable: true,
        next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        partial_update: None,
    })?;
    let subscription_level = string_at(
        &usage,
        &[
            "/subscriptionInfo/subscriptionTitle",
            "/subscription_info/subscription_title",
        ],
    )
    .or_else(|| account.subscription_level.clone())
    .or_else(|| Some("Amazon Q Developer".to_string()));
    let quota = quota_from_usage_limits(usage.clone(), subscription_level.clone(), now_ms);
    let mut raw = account
        .raw
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(object) = raw.as_object_mut() {
        object.insert("amazonQUsageLimits".to_string(), usage);
        object.insert("quotaRefreshedAtMs".to_string(), Value::from(now_ms));
    }
    let quota_percent = agentic_quota_percent(&quota);
    let mut update =
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms)
            .with_raw(raw);
    update.quota_percent = quota_percent;
    Ok(update)
}

fn kiro_quota_runtime_identity(
    account: &Account,
) -> Result<crate::domain::providers::kiro::KiroRuntimeIdentity, QuotaRefreshFailure> {
    crate::domain::providers::kiro::operational_runtime_identity_from_account(account)
        .map_err(|error| QuotaRefreshFailure::bad_request(error.to_string()))
}

fn kiro_quota_token_type(account: &Account) -> Option<&'static str> {
    let method = account
        .raw
        .as_ref()
        .and_then(|value| string_at(value, &["/authMethod", "/auth_method", "/provider"]))
        .or_else(|| {
            account
                .profile
                .as_ref()
                .and_then(|value| string_at(value, &["/authMethod", "/auth_method", "/provider"]))
        })
        .unwrap_or_default()
        .to_ascii_lowercase();
    match method.as_str() {
        "api_key" | "api-key" | "apikey" => Some("API_KEY"),
        "external_idp" | "external-idp" | "externalidp" => Some("EXTERNAL_IDP"),
        _ => None,
    }
}

trait AccountRefreshUpdateExt {
    fn with_raw(self, raw: Value) -> Self;
}

impl AccountRefreshUpdateExt for AccountRefreshUpdate {
    fn with_raw(mut self, raw: Value) -> Self {
        self.raw = Some(raw);
        self
    }
}

async fn request_json(
    provider_type: ProviderType,
    request: reqwest::RequestBuilder,
    now_ms: i64,
) -> Result<Value, QuotaRefreshFailure> {
    let mut response = request
        .send()
        .await
        .map_err(|error| QuotaRefreshFailure::network(provider_type, error, now_ms))?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = match crate::infra::http::read_response_body_limited(
        &mut response,
        MAX_QUOTA_RESPONSE_BODY_BYTES,
    )
    .await
    {
        Ok(body) => body,
        Err(error) if !status.is_success() => {
            return Err(QuotaRefreshFailure::upstream(
                provider_type,
                status,
                error.to_string(),
                retry_after,
                now_ms,
            ));
        }
        Err(error) => {
            return Err(QuotaRefreshFailure::response_body(
                provider_type,
                error,
                now_ms,
            ))
        }
    };
    if !status.is_success() {
        return Err(QuotaRefreshFailure::upstream(
            provider_type,
            status,
            String::from_utf8_lossy(&body).into_owned(),
            retry_after,
            now_ms,
        ));
    }
    serde_json::from_slice(&body)
        .map_err(|error| QuotaRefreshFailure::parse(provider_type, error, now_ms))
}

fn required_access_token(account: &Account) -> Result<&str, QuotaRefreshFailure> {
    account
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            QuotaRefreshFailure::bad_request(format!(
                "{} account requires an access token",
                account.provider_type.as_str()
            ))
        })
}

fn update_from_quota(
    quota: AccountQuota,
    subscription_level: Option<String>,
    profile: Option<Value>,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> AccountRefreshUpdate {
    let quota_percent = quota_percent_from_tiers(&quota.tiers);
    AccountRefreshUpdate {
        subscription_level,
        quota_percent,
        quota: Some(quota),
        quota_refreshed_at: Some(now_ms),
        quota_next_refresh_at: Some(now_ms.saturating_add(success_cooldown_ms)),
        profile,
        ..Default::default()
    }
}

fn update_from_claude_quota(
    quota: AccountQuota,
    profile: Option<Value>,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> AccountRefreshUpdate {
    let subscription_level = quota.credential_message.clone();
    let clear_subscription_level = subscription_level.is_none();
    let mut update = update_from_quota(
        quota,
        subscription_level,
        profile,
        now_ms,
        success_cooldown_ms,
    );
    update.clear_subscription_level = clear_subscription_level;
    update
}

fn quota_percent_from_tiers(tiers: &[AccountQuotaTier]) -> Option<f64> {
    tiers
        .iter()
        .filter(|tier| !tier.name.starts_with("review_"))
        .filter_map(|tier| tier.utilization)
        .filter(|value| value.is_finite())
        .map(|value| (value * 100.0).clamp(0.0, 10_000.0))
        .max_by(|left, right| left.total_cmp(right))
}

fn codex_tiers_from_rate_limit(rate_limit: Option<CodexRateLimit>) -> Vec<AccountQuotaTier> {
    let mut tiers = Vec::new();
    if let Some(rate_limit) = rate_limit {
        for window in
            normalize_codex_rate_windows(rate_limit.primary_window, rate_limit.secondary_window)
        {
            let Some(utilization) = codex_window_used_fraction(&window) else {
                continue;
            };
            tiers.push(AccountQuotaTier {
                name: window
                    .limit_window_seconds
                    .map(window_seconds_to_tier_name)
                    .unwrap_or_else(|| "unknown".to_string()),
                label: None,
                utilization: Some(utilization),
                used: None,
                limit: None,
                unit: Some("percent".to_string()),
                resets_at: window.reset_at.and_then(codex_reset_at_ms),
                ..Default::default()
            });
        }
    }
    sort_codex_quota_tiers(&mut tiers);
    tiers
}

fn codex_review_tiers_from_usage(body: &Value) -> Vec<AccountQuotaTier> {
    let Some(rate_limit) = explicit_codex_review_rate_limit(body) else {
        return Vec::new();
    };
    normalize_codex_rate_windows(rate_limit.primary_window, rate_limit.secondary_window)
        .into_iter()
        .enumerate()
        .filter_map(|(index, window)| {
            let utilization = codex_window_used_fraction(&window)?;
            let name = match codex_window_role(window.limit_window_seconds) {
                CodexWindowRole::Session => "review_session",
                CodexWindowRole::Weekly => "review_weekly",
                CodexWindowRole::Monthly => "review_monthly",
                CodexWindowRole::Unknown if index == 0 => "review_session",
                CodexWindowRole::Unknown => "review_weekly",
            };
            Some(AccountQuotaTier {
                name: name.to_string(),
                label: None,
                utilization: Some(utilization),
                used: None,
                limit: None,
                unit: Some("percent".to_string()),
                resets_at: window.reset_at.and_then(codex_reset_at_ms),
                ..Default::default()
            })
        })
        .collect()
}

fn explicit_codex_review_rate_limit(body: &Value) -> Option<CodexRateLimit> {
    let direct = ["code_review_rate_limit", "review_rate_limit"]
        .into_iter()
        .filter_map(|field| body.get(field));
    let by_limit_id = body
        .get("rate_limits_by_limit_id")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|limits| {
            ["code_review", "codex_review", "review"]
                .into_iter()
                .filter_map(|id| limits.get(id))
        });
    let additional = body
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| {
            ["limit_name", "metered_feature", "id"]
                .into_iter()
                .filter_map(|field| entry.get(field).and_then(Value::as_str))
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .any(|id| matches!(id.as_str(), "code_review" | "codex_review" | "review"))
        });

    direct
        .chain(by_limit_id)
        .chain(additional)
        .find_map(parse_codex_rate_limit_candidate)
}

fn parse_codex_rate_limit_candidate(candidate: &Value) -> Option<CodexRateLimit> {
    let candidate = candidate
        .get("rate_limit")
        .filter(|value| value.is_object())
        .unwrap_or(candidate);
    let parsed = serde_json::from_value::<CodexRateLimit>(candidate.clone()).ok()?;
    parsed
        .primary_window
        .as_ref()
        .into_iter()
        .chain(parsed.secondary_window.as_ref())
        .any(|window| codex_window_used_fraction(window).is_some())
        .then_some(parsed)
}

fn codex_reset_at_ms(value: i64) -> Option<i64> {
    if value <= 0 {
        return None;
    }
    Some(if value >= 1_000_000_000_000 {
        value
    } else {
        value.saturating_mul(1000)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexWindowRole {
    Session,
    Weekly,
    Monthly,
    Unknown,
}

fn codex_window_role(limit_window_seconds: Option<i64>) -> CodexWindowRole {
    match limit_window_seconds {
        Some(18_000) => CodexWindowRole::Session,
        Some(604_800) => CodexWindowRole::Weekly,
        Some(2_592_000) => CodexWindowRole::Monthly,
        Some(secs) if secs.div_euclid(60) == 300 => CodexWindowRole::Session,
        Some(secs) if secs.div_euclid(60) == 10_080 => CodexWindowRole::Weekly,
        _ => CodexWindowRole::Unknown,
    }
}

/// Codex rate windows are normalized with session (5h) first and weekly (7d) second.
fn normalize_codex_rate_windows(
    primary: Option<CodexRateLimitWindow>,
    secondary: Option<CodexRateLimitWindow>,
) -> Vec<CodexRateLimitWindow> {
    match (primary, secondary) {
        (Some(primary_window), Some(secondary_window)) => {
            let primary_role = codex_window_role(primary_window.limit_window_seconds);
            let secondary_role = codex_window_role(secondary_window.limit_window_seconds);
            match (primary_role, secondary_role) {
                (CodexWindowRole::Weekly, CodexWindowRole::Session)
                | (CodexWindowRole::Weekly, CodexWindowRole::Unknown) => {
                    vec![secondary_window, primary_window]
                }
                _ => vec![primary_window, secondary_window],
            }
        }
        (Some(primary_window), None) => {
            match codex_window_role(primary_window.limit_window_seconds) {
                CodexWindowRole::Weekly => vec![primary_window],
                _ => vec![primary_window],
            }
        }
        (None, Some(secondary_window)) => vec![secondary_window],
        (None, None) => Vec::new(),
    }
}

fn sort_codex_quota_tiers(tiers: &mut [AccountQuotaTier]) {
    const ORDER: &[&str] = &["five_hour", "seven_day", "30_day"];
    tiers.sort_by_key(|tier| {
        ORDER
            .iter()
            .position(|name| *name == tier.name)
            .unwrap_or(ORDER.len())
    });
}

/// `/wham/usage` reports consumed quota on a 0..100 percent scale. Keep the raw
/// value; treating `(0, 1]` as a 0..1 fraction would turn `1.0` into 100%.
fn codex_window_used_fraction(window: &CodexRateLimitWindow) -> Option<f64> {
    let used_percent = window.used_percent?;
    if !used_percent.is_finite() {
        return Some(0.0);
    }

    let mut normalized = used_percent.clamp(0.0, 100.0);
    // TokenRouter uses upstream used_percent directly for weekly windows. Only the
    // short session (5h) window can report consumed quota as remaining after reset.
    if codex_window_role(window.limit_window_seconds) == CodexWindowRole::Session {
        if let (Some(reset_after), Some(limit_secs)) =
            (window.reset_after_seconds, window.limit_window_seconds)
        {
            if limit_secs > 0 && reset_after >= 0 {
                let remaining_ratio = (reset_after as f64 / limit_secs as f64).clamp(0.0, 1.0);
                let remaining_percent = remaining_ratio * 100.0;
                if remaining_ratio > 0.85
                    && normalized > 50.0
                    && (remaining_percent - normalized).abs() < 25.0
                {
                    normalized = (100.0 - normalized).clamp(0.0, 100.0);
                }
            }
        }
    }

    Some((normalized / 100.0).clamp(0.0, 1.0))
}

#[derive(Debug, Clone)]
struct ClaudeQuotaSubscriptionResolution {
    resolution: ClaudeSubscriptionResolution,
    observed_at_ms: i64,
}

fn resolve_claude_quota_subscription(
    account: &Account,
    usage: &Value,
    profile_lookup: Option<&ClaudeProfileLookup>,
    bootstrap_profile: Option<&Value>,
    now_ms: i64,
) -> Option<ClaudeQuotaSubscriptionResolution> {
    let usage_tier = string_at(usage, &["/tier"]);
    let usage_plan = string_at(usage, &["/plan"]);
    let usage_subscription_type = string_at(usage, &["/subscription_type"]);
    let bootstrap_rate_limit_tier =
        bootstrap_profile.and_then(|value| string_at(value, &["/organizationRateLimitTier"]));
    let bootstrap_organization_type =
        bootstrap_profile.and_then(|value| string_at(value, &["/organizationType"]));
    let profile_rate_limit_tier = profile_lookup.and_then(|lookup| lookup.rate_limit_tier.clone());
    let profile_organization_type =
        profile_lookup.and_then(|lookup| lookup.organization_type.clone());
    let cached_profile = account.profile.as_ref();
    let cached_profile_rate_limit_tier = cached_profile.and_then(|value| {
        string_at(
            value,
            &[
                "/organizationRateLimitTier",
                "/organization_rate_limit_tier",
                "/profileRaw/organizationRateLimitTier",
                "/profileRaw/organization_rate_limit_tier",
                "/raw/organizationRateLimitTier",
                "/raw/organization_rate_limit_tier",
            ],
        )
    });
    let cached_profile_organization_type = cached_profile.and_then(|value| {
        string_at(
            value,
            &[
                "/organizationType",
                "/organization_type",
                "/profileRaw/organizationType",
                "/profileRaw/organization_type",
                "/raw/organizationType",
                "/raw/organization_type",
            ],
        )
    });
    let cached_subscription_level = account.subscription_level.clone();
    let cached_profile_rate_limit_observed_at = cached_profile
        .and_then(|profile| {
            cached_claude_profile_plan_observed_at(
                profile,
                ClaudeCachedProfileEvidence::RateLimitTier,
            )
        })
        .and_then(|observed_at| reusable_claude_plan_observation(observed_at, now_ms));
    let cached_profile_organization_observed_at = cached_profile
        .and_then(|profile| {
            cached_claude_profile_plan_observed_at(
                profile,
                ClaudeCachedProfileEvidence::OrganizationType,
            )
        })
        .and_then(|observed_at| reusable_claude_plan_observation(observed_at, now_ms));
    let cached_subscription_observed_at = cached_claude_canonical_plan_observed_at(account)
        .and_then(|observed_at| reusable_claude_plan_observation(observed_at, now_ms));

    let resolution = resolve_claude_subscription(
        [
            usage_tier.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(ClaudeSubscriptionSource::UsageTier, value)
            }),
            usage_plan.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(ClaudeSubscriptionSource::UsagePlan, value)
            }),
            usage_subscription_type.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(
                    ClaudeSubscriptionSource::UsageSubscriptionType,
                    value,
                )
            }),
            bootstrap_rate_limit_tier.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(
                    ClaudeSubscriptionSource::BootstrapRateLimitTier,
                    value,
                )
            }),
            profile_rate_limit_tier.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(
                    ClaudeSubscriptionSource::ProfileRateLimitTier,
                    value,
                )
            }),
            bootstrap_organization_type.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(
                    ClaudeSubscriptionSource::BootstrapOrganizationType,
                    value,
                )
            }),
            profile_organization_type.as_deref().map(|value| {
                ClaudeSubscriptionCandidate::new(
                    ClaudeSubscriptionSource::ProfileOrganizationType,
                    value,
                )
            }),
            cached_profile_rate_limit_tier
                .as_deref()
                .zip(cached_profile_rate_limit_observed_at)
                .map(|(value, _)| {
                    ClaudeSubscriptionCandidate::new(
                        ClaudeSubscriptionSource::CachedProfileRateLimitTier,
                        value,
                    )
                }),
            cached_profile_organization_type
                .as_deref()
                .zip(cached_profile_organization_observed_at)
                .map(|(value, _)| {
                    ClaudeSubscriptionCandidate::new(
                        ClaudeSubscriptionSource::CachedProfileOrganizationType,
                        value,
                    )
                }),
            cached_subscription_level
                .as_deref()
                .zip(cached_subscription_observed_at)
                .map(|(value, _)| {
                    ClaudeSubscriptionCandidate::new(
                        ClaudeSubscriptionSource::CachedSubscriptionLevel,
                        value,
                    )
                }),
        ]
        .into_iter()
        .flatten(),
    )?;
    let observed_at_ms = match resolution.source {
        ClaudeSubscriptionSource::UsageTier
        | ClaudeSubscriptionSource::UsagePlan
        | ClaudeSubscriptionSource::UsageSubscriptionType
        | ClaudeSubscriptionSource::BootstrapRateLimitTier
        | ClaudeSubscriptionSource::ProfileRateLimitTier
        | ClaudeSubscriptionSource::BootstrapOrganizationType
        | ClaudeSubscriptionSource::ProfileOrganizationType => now_ms,
        ClaudeSubscriptionSource::CachedProfileRateLimitTier => {
            cached_profile_rate_limit_observed_at?
        }
        ClaudeSubscriptionSource::CachedProfileOrganizationType => {
            cached_profile_organization_observed_at?
        }
        ClaudeSubscriptionSource::CachedSubscriptionLevel => cached_subscription_observed_at?,
    };
    Some(ClaudeQuotaSubscriptionResolution {
        resolution,
        observed_at_ms,
    })
}

#[derive(Debug, Clone, Copy)]
enum ClaudeCachedProfileEvidence {
    RateLimitTier,
    OrganizationType,
}

fn cached_claude_profile_plan_observed_at(
    profile: &Value,
    evidence: ClaudeCachedProfileEvidence,
) -> Option<i64> {
    let evidence_paths: &[&str] = match evidence {
        ClaudeCachedProfileEvidence::RateLimitTier => &[
            "/organizationRateLimitTierObservedAt",
            "/profileRaw/organizationRateLimitTierObservedAt",
            "/raw/organizationRateLimitTierObservedAt",
        ],
        ClaudeCachedProfileEvidence::OrganizationType => &[
            "/organizationTypeObservedAt",
            "/profileRaw/organizationTypeObservedAt",
            "/raw/organizationTypeObservedAt",
        ],
    };
    latest_timestamp_at(profile, evidence_paths).or_else(|| {
        latest_timestamp_at(
            profile,
            &[
                "/profileRefreshedAt",
                "/bootstrapRefreshedAt",
                "/profileRaw/profileRefreshedAt",
                "/profileRaw/bootstrapRefreshedAt",
                "/raw/profileRefreshedAt",
                "/raw/bootstrapRefreshedAt",
            ],
        )
    })
}

fn cached_claude_canonical_plan_observed_at(account: &Account) -> Option<i64> {
    let account_plan = account
        .subscription_level
        .as_deref()
        .and_then(parse_claude_subscription_plan)?;
    let subscription = account
        .quota
        .as_ref()
        .and_then(|quota| quota.extra_usage.as_ref())
        .and_then(|extra| extra.pointer("/subscription"));
    let recorded_plan = subscription
        .and_then(|value| string_at(value, &["/planType", "/planLabel"]))
        .as_deref()
        .and_then(parse_claude_subscription_plan)?;
    if recorded_plan != account_plan {
        return None;
    }
    subscription
        .and_then(|value| latest_timestamp_at(value, &["/planObservedAt"]))
        .or_else(|| {
            (subscription.and_then(|value| value.get("planStale").and_then(Value::as_bool))
                == Some(false))
            .then_some(account.quota_refreshed_at)
            .flatten()
        })
}

fn reusable_claude_plan_observation(observed_at_ms: i64, now_ms: i64) -> Option<i64> {
    (observed_at_ms > 0
        && observed_at_ms <= now_ms.saturating_add(CLAUDE_PLAN_CACHE_CLOCK_SKEW_MS)
        && now_ms.saturating_sub(observed_at_ms) <= CLAUDE_PLAN_CACHE_MAX_AGE_MS)
        .then_some(observed_at_ms)
}

fn parse_claude_quota(
    body: &Value,
    subscription: Option<&ClaudeQuotaSubscriptionResolution>,
    now_ms: i64,
) -> AccountQuota {
    const KNOWN_TIERS: &[&str] = &[
        "five_hour",
        "seven_day",
        "seven_day_overage_included",
        "seven_day_opus",
        "seven_day_omelette",
        "seven_day_sonnet",
    ];
    let mut tiers = Vec::new();
    for tier_name in KNOWN_TIERS {
        push_claude_tier(&mut tiers, tier_name, body.get(*tier_name));
    }
    if let Some(object) = body.as_object() {
        for (name, value) in object {
            if name == "extra_usage" || KNOWN_TIERS.contains(&name.as_str()) {
                continue;
            }
            push_claude_tier(&mut tiers, name, Some(value));
        }
    }
    if subscription.map(|value| value.resolution.fable_eligibility())
        != Some(ClaudeFableEligibility::Eligible)
    {
        tiers.retain(|tier| tier.name != CLAUDE_FABLE_QUOTA_TIER);
    }
    let plan_label =
        subscription.map(|subscription| subscription.resolution.plan.label().to_string());
    let subscription_json = subscription.map(|subscription| {
        let resolution = &subscription.resolution;
        json!({
            "planType": resolution.plan.plan_type(),
            "planLabel": resolution.plan.label(),
            "planSource": resolution.source.as_str(),
            "planStale": resolution.stale,
            "planObservedAt": subscription.observed_at_ms,
        })
    });
    let subscription_evidence = subscription.map(|subscription| {
        let resolution = &subscription.resolution;
        let mut observed_sources = Vec::new();
        for observation in &resolution.observations {
            let source = observation.source.as_str();
            if !observed_sources.contains(&source) {
                observed_sources.push(source);
            }
        }
        json!({
            "source": resolution.source.as_str(),
            "stale": resolution.stale,
            "observedAt": subscription.observed_at_ms,
            "cacheMaxAgeMs": CLAUDE_PLAN_CACHE_MAX_AGE_MS,
            "conflict": resolution.conflict,
            "conflictingPlanTypes": resolution.conflicting_plan_types,
            "observedSources": observed_sources,
        })
    });
    let warning_codes = subscription
        .filter(|subscription| subscription.resolution.conflict)
        .map(|_| vec!["claude_plan_conflict"])
        .unwrap_or_default();
    let warnings = subscription
        .filter(|subscription| subscription.resolution.conflict)
        .map(|_| {
            vec!["Conflicting Claude subscription plan evidence was returned; the highest-authority source was used."]
        })
        .unwrap_or_default();
    AccountQuota {
        success: true,
        credential_message: plan_label,
        tiers,
        extra_usage: Some(json!({
            "raw": body,
            "extraUsage": body.get("extra_usage"),
            "subscription": subscription_json,
            "subscriptionEvidence": subscription_evidence,
            "warningCodes": warning_codes,
            "warnings": warnings,
            "queriedAt": now_ms,
        })),
    }
}

fn push_claude_tier(tiers: &mut Vec<AccountQuotaTier>, name: &str, value: Option<&Value>) {
    let Some(value) = value else {
        return;
    };
    let Ok(window) = serde_json::from_value::<ClaudeUsageWindow>(value.clone()) else {
        return;
    };
    let Some(utilization) = window.utilization else {
        return;
    };
    let normalized_name = normalize_claude_tier_name(name);
    let is_fable = normalized_name == CLAUDE_FABLE_QUOTA_TIER;
    tiers.push(AccountQuotaTier {
        name: normalized_name.to_string(),
        label: None,
        utilization: Some(percent_to_fraction(utilization)),
        used: None,
        limit: None,
        unit: Some("percent".to_string()),
        resets_at: window.resets_at.as_deref().and_then(rfc3339_to_unix_ms),
        scope: is_fable.then(|| "model_family".to_string()),
        capacity_pool: is_fable.then(|| CLAUDE_FABLE_CAPACITY_POOL.to_string()),
        model_family: is_fable.then(|| CLAUDE_FABLE_MODEL_FAMILY.to_string()),
        relative_weekly_capacity: is_fable.then_some(CLAUDE_FABLE_RELATIVE_WEEKLY_CAPACITY),
        source: is_fable.then(|| "anthropic_usage_7d_oi".to_string()),
    });
}

fn parse_gemini_quota(
    quota_response: &GeminiQuotaResponse,
    plan_label: Option<String>,
    load_body: Value,
    quota_body: Value,
    now_ms: i64,
) -> AccountQuota {
    let mut buckets: Vec<(String, f64, Option<String>)> = Vec::new();
    if let Some(items) = quota_response.buckets.as_ref() {
        for bucket in items {
            let model_id = bucket.model_id.as_deref().unwrap_or("unknown");
            let category = classify_gemini_model(model_id).to_string();
            let remaining = bucket.remaining_fraction.unwrap_or(1.0).clamp(0.0, 1.0);
            if let Some(existing) = buckets.iter_mut().find(|item| item.0 == category) {
                if remaining < existing.1 {
                    existing.1 = remaining;
                    existing.2 = bucket.reset_time.clone();
                }
            } else {
                buckets.push((category, remaining, bucket.reset_time.clone()));
            }
        }
    }
    buckets.sort_by_key(|item| gemini_sort_order(&item.0));
    let tiers = buckets
        .into_iter()
        .map(|(name, remaining, reset_time)| AccountQuotaTier {
            name,
            label: None,
            utilization: Some((1.0 - remaining).clamp(0.0, 1.0)),
            used: None,
            limit: None,
            unit: Some("percent".to_string()),
            resets_at: reset_time.as_deref().and_then(rfc3339_to_unix_ms),
            ..Default::default()
        })
        .collect();
    AccountQuota {
        success: true,
        credential_message: plan_label,
        tiers,
        extra_usage: Some(json!({
            "loadCodeAssist": load_body,
            "retrieveUserQuota": quota_body,
            "queriedAt": now_ms,
        })),
    }
}

fn parse_ollama_me_update(
    body: &Value,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> AccountRefreshUpdate {
    let email = string_at(body, &["/Email", "/email"]);
    let name = string_at(body, &["/Name", "/name"]);
    let plan = string_at(body, &["/Plan", "/plan"]);
    let subscription_level = plan
        .as_deref()
        .map(|value| format!("ollama {value}"))
        .or_else(|| Some("ollama".to_string()));
    let period_end = valid_time_field(body, "/SubscriptionPeriodEnd")
        .or_else(|| valid_time_field(body, "/subscriptionPeriodEnd"));
    let period_start = valid_time_field(body, "/SubscriptionPeriodStart")
        .or_else(|| valid_time_field(body, "/subscriptionPeriodStart"));
    let remaining_ms = period_end
        .as_deref()
        .and_then(rfc3339_to_unix_ms)
        .map(|end_ms| end_ms.saturating_sub(now_ms).max(0));
    let quota = AccountQuota {
        success: true,
        credential_message: subscription_level.clone(),
        tiers: Vec::new(),
        extra_usage: Some(json!({
            "raw": body,
            "displayOnly": true,
            "email": email,
            "name": name,
            "plan": plan,
            "subscriptionPeriodStart": period_start,
            "subscriptionPeriodEnd": period_end,
            "subscriptionRemainingMs": remaining_ms,
            "queriedAt": now_ms,
        })),
    };
    AccountRefreshUpdate {
        email: email.clone(),
        subscription_level,
        quota_percent: None,
        quota: Some(quota),
        quota_refreshed_at: Some(now_ms),
        quota_next_refresh_at: Some(now_ms.saturating_add(success_cooldown_ms)),
        profile: Some(json!({
            "providerType": ProviderType::OllamaCloud.as_str(),
            "email": email,
            "name": name,
            "plan": plan,
            "source": "ollama_api_me",
        })),
        ..Default::default()
    }
}

async fn refresh_copilot_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let domain = copilot_account_domain_for_quota(account)?;
    let github_token = copilot_github_token_for_quota(account).ok_or_else(|| {
        QuotaRefreshFailure::bad_request(
            "github_copilot account requires the GitHub OAuth token used for device login",
        )
    })?;
    #[cfg(test)]
    let usage_url_override = account
        .raw
        .as_ref()
        .and_then(|raw| raw.get("testCopilotUsageUrl"))
        .and_then(Value::as_str);
    #[cfg(test)]
    let usage = match usage_url_override {
        Some(url) => {
            crate::clients::oauth::copilot_device::fetch_copilot_usage_from_url_with_timeout(
                http,
                url,
                &github_token,
                Some(request_timeout),
            )
            .await
        }
        None => {
            crate::clients::oauth::copilot_device::fetch_copilot_usage_with_timeout(
                http,
                &domain,
                &github_token,
                Some(request_timeout),
            )
            .await
        }
    };
    #[cfg(not(test))]
    let usage = crate::clients::oauth::copilot_device::fetch_copilot_usage_with_timeout(
        http,
        &domain,
        &github_token,
        Some(request_timeout),
    )
    .await;
    let usage = usage.map_err(|error| {
        if error.status.is_client_error() || error.status.is_server_error() {
            QuotaRefreshFailure::upstream(
                account.provider_type,
                error.status,
                error.message,
                None,
                now_ms,
            )
        } else {
            QuotaRefreshFailure {
                status_code: 502,
                upstream_status: None,
                message: error.message,
                retryable: true,
                next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
                partial_update: None,
            }
        }
    })?;
    let quota = parse_copilot_usage_quota(&usage, now_ms)?;
    let subscription_level = quota.credential_message.clone();
    let mut raw = account
        .raw
        .clone()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(object) = raw.as_object_mut() {
        object.insert("copilotUsage".to_string(), usage);
        object.insert("quotaRefreshedAtMs".to_string(), Value::from(now_ms));
    }
    let mut update =
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms);
    update.raw = Some(raw);
    update
        .capability_observations
        .push(AccountCapabilityObservationDraft::copilot_feature(
            crate::domain::accounts::capability_evidence::PREMIUM_INTERACTIONS_DIMENSION,
            AccountCapabilityObservationState::Supported,
            "copilot_internal_user",
            None,
            now_ms,
        ));
    Ok(update)
}

fn copilot_account_domain_for_quota(account: &Account) -> Result<String, QuotaRefreshFailure> {
    let domain = [&account.raw, &account.profile]
        .into_iter()
        .filter_map(|value| value.as_ref())
        .find_map(|value| string_at(value, &["/githubDomain", "/github_domain"]))
        .unwrap_or_else(|| "github.com".to_string());
    crate::clients::oauth::copilot_device::normalize_github_domain(&domain)
        .map_err(|error| QuotaRefreshFailure::bad_request(error.message))
}

fn copilot_github_token_for_quota(account: &Account) -> Option<String> {
    account
        .raw
        .as_ref()
        .and_then(|raw| string_at(raw, &["/githubToken", "/github_token"]))
        .or_else(|| account.refresh_token.clone())
        .or_else(|| account.api_key.clone())
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn parse_copilot_usage_quota(
    snapshot: &Value,
    now_ms: i64,
) -> Result<AccountQuota, QuotaRefreshFailure> {
    let usage = serde_json::from_value::<CopilotImportedUsage>(snapshot.clone()).ok();
    let plan = usage
        .as_ref()
        .and_then(|usage| usage.copilot_plan.clone())
        .or_else(|| string_at(snapshot, &["/copilotPlan", "/copilot_plan", "/plan"]));
    let paid_reset = usage
        .as_ref()
        .and_then(|usage| usage.quota_reset_date.clone())
        .or_else(|| {
            string_at(
                snapshot,
                &["/quotaResetDate", "/quota_reset_date", "/resetAt"],
            )
        });
    let limited_reset = string_at(
        snapshot,
        &[
            "/limited_user_reset_date",
            "/limitedUserResetDate",
            "/monthly_reset_date",
            "/monthlyResetDate",
        ],
    );
    let paid = usage
        .as_ref()
        .and_then(|usage| usage.quota_snapshots.as_ref())
        .and_then(|snapshots| snapshots.premium_interactions.clone())
        .or_else(|| {
            value_at(
                snapshot,
                &[
                    "/quota_snapshots/premium_interactions",
                    "/quotaSnapshots/premiumInteractions",
                    "/premium_interactions",
                    "/premiumInteractions",
                ],
            )
            .and_then(|value| serde_json::from_value::<CopilotQuotaDetail>(value).ok())
        })
        .filter(CopilotQuotaDetail::has_evidence);
    let limited_total = number_at(
        snapshot,
        &[
            "/monthly_quotas/premium_interactions",
            "/monthlyQuotas/premiumInteractions",
        ],
    );
    let limited_remaining = number_at(
        snapshot,
        &[
            "/limited_user_quotas/premium_interactions",
            "/limitedUserQuotas/premiumInteractions",
        ],
    );

    let (tier, reset, source_shape) = if let Some(premium) = paid {
        (
            copilot_tier_from_detail(&premium, paid_reset.as_deref()),
            paid_reset,
            "quota_snapshots",
        )
    } else if let Some(total) = limited_total.filter(|value| *value > 0.0) {
        let remaining = limited_remaining.unwrap_or(0.0).clamp(0.0, total);
        (
            AccountQuotaTier {
                name: "premium".to_string(),
                label: None,
                utilization: Some(((total - remaining) / total).clamp(0.0, 1.0)),
                used: Some((total - remaining).max(0.0)),
                limit: Some(total),
                unit: Some("premium_interactions".to_string()),
                resets_at: limited_reset.as_deref().and_then(dateish_to_unix_ms),
                ..Default::default()
            },
            limited_reset,
            "monthly_quotas",
        )
    } else {
        return Err(QuotaRefreshFailure::bad_request(
            "github_copilot usage response is missing premium_interactions quota",
        ));
    };
    Ok(AccountQuota {
        success: true,
        credential_message: Some(copilot_plan_label(
            snapshot,
            plan.as_deref(),
            tier.limit,
            source_shape,
        )),
        tiers: vec![tier],
        extra_usage: Some(json!({
            "raw": snapshot,
            "source": "copilot_internal_user",
            "shape": source_shape,
            "resetAt": reset,
            "queriedAt": now_ms,
        })),
    })
}

fn copilot_tier_from_detail(premium: &CopilotQuotaDetail, reset: Option<&str>) -> AccountQuotaTier {
    let unlimited = premium.unlimited.unwrap_or(false);
    let limit = premium
        .entitlement
        .or(premium.total)
        .map(|value| value.max(0.0));
    let remaining = premium.remaining.map(|value| {
        let value = value.max(0.0);
        limit.map_or(value, |limit| value.min(limit))
    });
    let used = premium
        .used
        .map(|value| {
            let value = value.max(0.0);
            limit.map_or(value, |limit| value.min(limit))
        })
        .or_else(|| {
            limit
                .zip(remaining)
                .map(|(limit, remaining)| (limit - remaining).max(0.0))
        });
    let utilization = if unlimited {
        0.0
    } else if let Some(percent_remaining) = premium.percent_remaining {
        percent_to_fraction(100.0 - percent_remaining.clamp(0.0, 100.0))
    } else if let Some((limit, remaining)) = limit.zip(remaining).filter(|(limit, _)| *limit > 0.0)
    {
        ((limit - remaining) / limit).clamp(0.0, 1.0)
    } else {
        0.0
    };
    AccountQuotaTier {
        name: "premium".to_string(),
        label: unlimited.then(|| "Unlimited".to_string()),
        utilization: Some(utilization),
        used: unlimited.then_some(0.0).or(used),
        limit: if unlimited { None } else { limit },
        unit: Some("premium_interactions".to_string()),
        resets_at: reset.and_then(dateish_to_unix_ms),
        ..Default::default()
    }
}

impl CopilotQuotaDetail {
    fn has_evidence(&self) -> bool {
        self.unlimited == Some(true)
            || self.entitlement.is_some()
            || self.total.is_some()
            || self.remaining.is_some()
            || self.used.is_some()
            || self.percent_remaining.is_some()
    }
}

fn copilot_plan_label(
    snapshot: &Value,
    plan: Option<&str>,
    premium_limit: Option<f64>,
    source_shape: &str,
) -> String {
    let sku = string_at(snapshot, &["/access_type_sku", "/accessTypeSku"]).unwrap_or_default();
    let combined = format!("{} {}", sku, plan.unwrap_or_default()).to_ascii_uppercase();
    if combined.contains("PRO+") || combined.contains("PRO_PLUS") || combined.contains("PROPLUS") {
        "Copilot Pro+".to_string()
    } else if combined.contains("ENTERPRISE") {
        "Copilot Enterprise".to_string()
    } else if combined.contains("BUSINESS") {
        "Copilot Business".to_string()
    } else if combined.contains("STUDENT") {
        "Copilot Student".to_string()
    } else if combined.contains("FREE") || source_shape == "monthly_quotas" {
        "Copilot Free".to_string()
    } else if combined.contains("PRO") {
        "Copilot Pro".to_string()
    } else if premium_limit.is_some_and(|limit| limit >= 1_400.0) {
        "Copilot Pro+".to_string()
    } else if let Some(plan) = plan {
        format_copilot_plan_label(plan)
    } else {
        "GitHub Copilot".to_string()
    }
}

fn parse_kiro_imported_quota(
    account: &Account,
    now_ms: i64,
) -> Result<AccountQuota, QuotaRefreshFailure> {
    let snapshot = require_imported_snapshot(
        account,
        &[
            "/kiroUsageLimits",
            "/kiro_usage_limits",
            "/usageLimits",
            "/usage_limits",
            "/usage",
            "/quota",
            "/billingOrQuotaSnapshot",
            "",
        ],
        "Kiro getUsageLimits response",
    )?;
    if !snapshot.is_object() {
        return Err(QuotaRefreshFailure::bad_request(
            "kiro_oauth imported usage limits snapshot must be a JSON object",
        ));
    }
    let plan = string_at(
        &snapshot,
        &[
            "/subscriptionInfo/subscriptionTitle",
            "/subscription_info/subscription_title",
        ],
    )
    .or_else(|| Some("Kiro OAuth".to_string()));
    let mut quota = quota_from_usage_limits(snapshot, plan, now_ms);
    if let Some(extra_usage) = quota.extra_usage.as_mut().and_then(Value::as_object_mut) {
        extra_usage.insert("source".to_string(), json!("imported_snapshot"));
    }
    Ok(quota)
}

fn parse_cursor_imported_quota(
    account: &Account,
    now_ms: i64,
) -> Result<AccountQuota, QuotaRefreshFailure> {
    let snapshot = require_imported_snapshot(
        account,
        &[
            "/cursorUsage",
            "/cursor_usage",
            "/currentPeriodUsage",
            "/current_period_usage",
            "/usage",
            "/quota",
            "/billingOrQuotaSnapshot",
            "",
        ],
        "Cursor current period usage response",
    )?;
    let usage = value_at(
        &snapshot,
        &["/currentPeriodUsage", "/current_period_usage", "/usage"],
    )
    .unwrap_or_else(|| snapshot.clone());
    let plan_usage = usage.get("planUsage").or_else(|| usage.get("plan_usage"));
    let plan_paths = [
        "/stripeStatus/membershipType",
        "/stripe_status/membership_type",
        "/profile/stripeStatus/membershipType",
        "/profile/stripe_status/membership_type",
        "/membershipType",
        "/membership_type",
        "/profile/membershipType",
        "/profile/membership_type",
        "/subscription/planLabel",
        "/profile/subscription/planLabel",
        "/plan",
        "/profile/plan",
    ];
    let plan = account
        .raw
        .as_ref()
        .and_then(|raw| string_at(raw, &plan_paths))
        .or_else(|| string_at(&snapshot, &plan_paths))
        .and_then(|value| cursor_membership_label(&value))
        .or_else(|| account.subscription_level.clone())
        .or_else(|| Some("Cursor".to_string()));
    let resets_at = number_at(&usage, &["/billingCycleEnd", "/billing_cycle_end"])
        .and_then(timestamp_number_to_unix_ms)
        .or_else(|| {
            string_at(&usage, &["/billingCycleEnd", "/billing_cycle_end"])
                .and_then(|value| dateish_to_unix_ms(&value))
        });
    let Some(plan_usage) = plan_usage else {
        return Err(QuotaRefreshFailure::bad_request(
            "cursor imported usage snapshot is missing planUsage",
        ));
    };
    let limit = number_at(plan_usage, &["/limit"]).unwrap_or(0.0);
    let (name, utilization, used, limit, unit) = if limit > 0.0 {
        let used = number_at(plan_usage, &["/used", "/totalSpend", "/total_spend"]).or_else(|| {
            number_at(plan_usage, &["/remaining"]).map(|remaining| (limit - remaining).max(0.0))
        });
        let utilization = number_at(plan_usage, &["/totalPercentUsed", "/total_percent_used"])
            .map(percent_to_fraction)
            .or_else(|| used.map(|used| (used / limit).clamp(0.0, 1.0)))
            .unwrap_or(0.0);
        (
            "cursor_credits",
            utilization,
            used.map(|value| value / 100.0),
            Some(limit / 100.0),
            Some("USD".to_string()),
        )
    } else {
        (
            "cursor_included_usage",
            number_at(plan_usage, &["/totalPercentUsed", "/total_percent_used"])
                .map(percent_to_fraction)
                .unwrap_or(0.0),
            None,
            None,
            None,
        )
    };

    Ok(AccountQuota {
        success: true,
        credential_message: plan,
        tiers: vec![AccountQuotaTier {
            name: name.to_string(),
            label: None,
            utilization: Some(utilization),
            used,
            limit,
            unit,
            resets_at,
            ..Default::default()
        }],
        extra_usage: Some(json!({
            "raw": snapshot,
            "source": if account
                .raw
                .as_ref()
                .and_then(|raw| raw.get("source"))
                .and_then(Value::as_str)
                == Some("cursor_dashboard_api")
            {
                "cursor_dashboard_api"
            } else {
                "imported_snapshot"
            },
            "queriedAt": now_ms,
        })),
    })
}

#[derive(Debug, Clone, PartialEq)]
struct ClaudeProfileLookup {
    organization_type: Option<String>,
    rate_limit_tier: Option<String>,
    profile_overlay: Option<Value>,
}

async fn fetch_claude_profile_lookup(
    http: &reqwest::Client,
    access_token: &str,
    request_timeout: Duration,
    now_ms: i64,
) -> Option<ClaudeProfileLookup> {
    let response = http
        .get(CLAUDE_PROFILE_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json, text/plain, */*")
        .header(USER_AGENT, claude_axios_user_agent())
        .timeout(request_timeout)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut response = response;
    let body = read_claude_control_json(&mut response).await.ok()?;
    parse_claude_profile_lookup(&body, now_ms)
}

fn parse_claude_profile_lookup(body: &Value, now_ms: i64) -> Option<ClaudeProfileLookup> {
    let organization = body.get("organization")?.as_object()?;
    let organization_type = organization
        .get("organization_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let rate_limit_tier = organization
        .get("rate_limit_tier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let mut overlay = serde_json::Map::new();
    for (target, source) in [
        ("organizationUUID", "uuid"),
        ("organizationName", "name"),
        ("organizationType", "organization_type"),
        ("organizationRateLimitTier", "rate_limit_tier"),
    ] {
        if let Some(value) = organization
            .get(source)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            overlay.insert(target.to_string(), Value::String(value.to_string()));
        }
    }
    if let Some(billing_source) = organization
        .get("billing_type")
        .and_then(Value::as_str)
        .and_then(normalize_claude_billing_source)
    {
        overlay.insert("billingSource".to_string(), Value::String(billing_source));
    }
    if organization_type.is_none() && rate_limit_tier.is_none() && overlay.is_empty() {
        return None;
    }
    overlay.insert("profileRefreshedAt".to_string(), json!(now_ms));
    if organization_type.is_some() {
        overlay.insert("organizationTypeObservedAt".to_string(), json!(now_ms));
    }
    if rate_limit_tier.is_some() {
        overlay.insert(
            "organizationRateLimitTierObservedAt".to_string(),
            json!(now_ms),
        );
    }
    Some(ClaudeProfileLookup {
        organization_type,
        rate_limit_tier,
        profile_overlay: Some(Value::Object(overlay)),
    })
}

fn normalize_claude_billing_source(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(match value.to_ascii_lowercase().as_str() {
        "apple_subscription" => "apple_subscription".to_string(),
        "stripe_subscription" => "stripe_subscription".to_string(),
        _ => value.to_string(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatGptProbeStatus {
    Success,
    NotProvided,
    SkippedNoTrustedWorkspace,
    HttpError,
    NetworkError,
    ParseError,
}

async fn read_chatgpt_probe_json(
    response: &mut reqwest::Response,
) -> Result<Value, ChatGptProbeStatus> {
    let body =
        crate::infra::http::read_response_body_limited(response, MAX_QUOTA_RESPONSE_BODY_BYTES)
            .await
            .map_err(|error| {
                tracing::debug!(error = %error, "ChatGPT probe response body could not be read");
                match error {
                    crate::infra::http::BoundedResponseBodyError::Request(_) => {
                        ChatGptProbeStatus::NetworkError
                    }
                    crate::infra::http::BoundedResponseBodyError::TooLarge { .. } => {
                        ChatGptProbeStatus::ParseError
                    }
                }
            })?;
    serde_json::from_slice(&body).map_err(|error| {
        tracing::debug!(error = %error, "ChatGPT probe response was invalid JSON");
        ChatGptProbeStatus::ParseError
    })
}

impl ChatGptProbeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::NotProvided => "not_provided",
            Self::SkippedNoTrustedWorkspace => "skipped_no_trusted_workspace",
            Self::HttpError => "http_error",
            Self::NetworkError => "network_error",
            Self::ParseError => "parse_error",
        }
    }

    fn unavailable(self) -> bool {
        matches!(
            self,
            Self::HttpError | Self::NetworkError | Self::ParseError
        )
    }
}

#[derive(Debug, Clone)]
struct ChatGptSubscriptionProbe {
    status: ChatGptProbeStatus,
    http_status: Option<u16>,
    lookup: Option<ChatGptSubscriptionLookup>,
    workspace_candidates: Vec<ChatGptWorkspaceCandidate>,
}

impl ChatGptSubscriptionProbe {
    fn skipped_no_trusted_workspace() -> Self {
        Self {
            status: ChatGptProbeStatus::SkippedNoTrustedWorkspace,
            http_status: None,
            lookup: None,
            workspace_candidates: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct ChatGptWorkspaceCandidate {
    workspace_id: String,
    lookup: ChatGptSubscriptionLookup,
}

async fn fetch_chatgpt_account_lookup(
    http: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    now_ms: i64,
    request_timeout: Duration,
) -> ChatGptSubscriptionProbe {
    let mut response = match http
        .get(CHATGPT_ACCOUNTS_CHECK_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .header(ACCEPT, "application/json")
        .timeout(request_timeout)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(error = %error, "ChatGPT accounts/check request failed");
            return ChatGptSubscriptionProbe {
                status: ChatGptProbeStatus::NetworkError,
                http_status: None,
                lookup: None,
                workspace_candidates: Vec::new(),
            };
        }
    };
    if !response.status().is_success() {
        return ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::HttpError,
            http_status: Some(response.status().as_u16()),
            lookup: None,
            workspace_candidates: Vec::new(),
        };
    }
    let status = response.status().as_u16();
    let body = match read_chatgpt_probe_json(&mut response).await {
        Ok(body) => body,
        Err(probe_status) => {
            return ChatGptSubscriptionProbe {
                status: probe_status,
                http_status: Some(status),
                lookup: None,
                workspace_candidates: Vec::new(),
            };
        }
    };
    let workspace_candidates = parse_chatgpt_workspace_candidates(&body, now_ms);
    let lookup = parse_chatgpt_accounts_check_lookup(&body, account_id, now_ms);
    ChatGptSubscriptionProbe {
        status: if lookup.is_some() {
            ChatGptProbeStatus::Success
        } else {
            ChatGptProbeStatus::NotProvided
        },
        http_status: Some(status),
        lookup,
        workspace_candidates,
    }
}

async fn fetch_chatgpt_subscription_lookup(
    http: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    request_timeout: Duration,
) -> ChatGptSubscriptionProbe {
    let Some(account_id) = account_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return ChatGptSubscriptionProbe::skipped_no_trusted_workspace();
    };
    let mut response = match http
        .get(CHATGPT_SUBSCRIPTIONS_URL)
        .query(&[("account_id", account_id)])
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .header(ACCEPT, "application/json")
        .timeout(request_timeout)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!(error = %error, "ChatGPT subscriptions request failed");
            return ChatGptSubscriptionProbe {
                status: ChatGptProbeStatus::NetworkError,
                http_status: None,
                lookup: None,
                workspace_candidates: Vec::new(),
            };
        }
    };
    if !response.status().is_success() {
        return ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::HttpError,
            http_status: Some(response.status().as_u16()),
            lookup: None,
            workspace_candidates: Vec::new(),
        };
    }
    let status = response.status().as_u16();
    let body = match read_chatgpt_probe_json(&mut response).await {
        Ok(body) => body,
        Err(probe_status) => {
            return ChatGptSubscriptionProbe {
                status: probe_status,
                http_status: Some(status),
                lookup: None,
                workspace_candidates: Vec::new(),
            };
        }
    };
    let lookup = parse_chatgpt_subscription_lookup(&body);
    ChatGptSubscriptionProbe {
        status: if lookup.is_some() {
            ChatGptProbeStatus::Success
        } else {
            ChatGptProbeStatus::NotProvided
        },
        http_status: Some(status),
        lookup,
        workspace_candidates: Vec::new(),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ChatGptSubscriptionLookup {
    plan_type: Option<String>,
    plan_label: Option<String>,
    expires_at: Option<String>,
    expires_source: Option<String>,
    expires_kind: Option<String>,
    expiry_availability: Option<String>,
    expiry_stale: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexSubscriptionExpirySnapshot {
    workspace_id: String,
    plan_family: String,
    expires_at: String,
    source: String,
    kind: String,
    observed_at: i64,
    stale: bool,
}

async fn recover_signed_codex_workspace(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
) -> Option<(
    crate::domain::accounts::store::TrustedCodexWorkspace,
    Option<Value>,
)> {
    let id_token = account
        .id_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let claims = match crate::clients::oauth::openai_jwks::verify_openai_id_token_identity(
        http, id_token,
    )
    .await
    {
        Ok(claims) => claims,
        Err(error) => {
            tracing::debug!(account_id = %account.id, error = %error, "could not recover Codex workspace from persisted ID token");
            return None;
        }
    };
    let identity = crate::domain::accounts::oauth::openai_identity_from_claims(&claims);
    let canonical_claims = crate::domain::accounts::oauth::canonical_openai_claims(&identity);
    let mut profile = account.profile.clone();
    crate::domain::accounts::store::set_verified_openai_claims(
        &mut profile,
        Some(canonical_claims),
    );
    let mut candidate = account.clone();
    candidate.profile = profile.clone();
    let workspace = crate::domain::accounts::store::trusted_codex_workspace(&candidate)?;
    crate::domain::accounts::store::set_codex_workspace_provenance(
        &mut profile,
        &workspace.id,
        "signed_id_token_migration",
        now_ms,
    );
    Some((workspace, profile))
}

fn authenticated_codex_workspace_update(
    account: &Account,
    workspace_id: &str,
    now_ms: i64,
) -> (
    crate::domain::accounts::store::TrustedCodexWorkspace,
    Option<Value>,
) {
    let mut profile = account.profile.clone();
    crate::domain::accounts::store::set_codex_workspace_provenance(
        &mut profile,
        workspace_id,
        "authenticated_discovery",
        now_ms,
    );
    (
        crate::domain::accounts::store::TrustedCodexWorkspace {
            id: workspace_id.to_string(),
            source: "authenticated_discovery".to_string(),
        },
        profile,
    )
}

fn legacy_codex_workspace_candidate(account: &Account) -> Option<String> {
    const POINTERS: &[&str] = &[
        "/accountId",
        "/account_id",
        "/chatgptAccountId",
        "/chatgpt_account_id",
        "/openai_auth/chatgpt_account_id",
        "/openaiAuth/chatgptAccountId",
        "/raw/chatgpt_account_id",
        "/raw/openai_auth/chatgpt_account_id",
    ];
    let mut observations = Vec::new();
    for value in [account.profile.as_ref(), account.raw.as_ref()]
        .into_iter()
        .flatten()
    {
        for pointer in POINTERS {
            if let Some(candidate) = value
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                observations.push(candidate.to_string());
            }
        }
    }
    if let Some(account_id) = account
        .access_token
        .as_deref()
        .and_then(crate::domain::accounts::oauth::chatgpt_account_id_from_jwt)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        observations.push(account_id);
    }
    let mut unique = observations.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != 1 {
        return None;
    }
    let candidate = unique.pop().expect("one unique candidate was checked");
    Some(candidate)
}

fn chatgpt_probe_matches_usage(
    probe: &ChatGptSubscriptionProbe,
    usage_plan_type: Option<&str>,
) -> bool {
    probe.status == ChatGptProbeStatus::Success
        && usage_plan_type
            .zip(
                probe
                    .lookup
                    .as_ref()
                    .and_then(|lookup| lookup.plan_type.as_deref()),
            )
            .is_some_and(|(usage_plan, probe_plan)| {
                chatgpt_plan_types_match(usage_plan, probe_plan)
            })
}

fn finalize_codex_subscription(
    account: &Account,
    subscription: Option<ChatGptSubscriptionLookup>,
    trusted_workspace: Option<&crate::domain::accounts::store::TrustedCodexWorkspace>,
    usage_plan_type: Option<&str>,
    account_probe: &ChatGptSubscriptionProbe,
    subscription_probe: &ChatGptSubscriptionProbe,
    now_ms: i64,
) -> (Option<ChatGptSubscriptionLookup>, Option<Value>) {
    let mut subscription = subscription;
    let availability = if trusted_workspace.is_none() {
        "workspace_unverified"
    } else if account_probe.status.unavailable() || subscription_probe.status.unavailable() {
        "probe_unavailable"
    } else {
        "upstream_not_provided"
    };

    if let Some(item) = subscription.as_mut() {
        if item.expires_at.is_some() {
            item.expiry_availability = Some("available".to_string());
            item.expiry_stale = false;
            let snapshot = codex_expiry_snapshot_from_lookup(
                item,
                trusted_workspace,
                usage_plan_type,
                now_ms,
                false,
            );
            return (subscription, snapshot.map(codex_expiry_snapshot_json));
        }
    }

    if let Some(mut snapshot) = previous_codex_expiry_snapshot(account).filter(|snapshot| {
        trusted_workspace.is_some_and(|workspace| workspace.id == snapshot.workspace_id)
            && usage_plan_type
                .map(chatgpt_plan_family)
                .is_some_and(|family| family == snapshot.plan_family)
            && !chatgpt_expiry_is_past(&snapshot.expires_at, now_ms)
    }) {
        snapshot.stale = true;
        let item = subscription.get_or_insert_with(ChatGptSubscriptionLookup::default);
        item.expires_at = Some(snapshot.expires_at.clone());
        item.expires_source = Some(snapshot.source.clone());
        item.expires_kind = Some(snapshot.kind.clone());
        item.expiry_availability = Some("available".to_string());
        item.expiry_stale = true;
        return (subscription, Some(codex_expiry_snapshot_json(snapshot)));
    }

    if let Some(item) = subscription.as_mut() {
        item.expiry_availability = Some(availability.to_string());
        item.expiry_stale = false;
    }
    (subscription, None)
}

fn codex_expiry_snapshot_from_lookup(
    lookup: &ChatGptSubscriptionLookup,
    trusted_workspace: Option<&crate::domain::accounts::store::TrustedCodexWorkspace>,
    usage_plan_type: Option<&str>,
    observed_at: i64,
    stale: bool,
) -> Option<CodexSubscriptionExpirySnapshot> {
    Some(CodexSubscriptionExpirySnapshot {
        workspace_id: trusted_workspace?.id.clone(),
        plan_family: chatgpt_plan_family(usage_plan_type?),
        expires_at: lookup.expires_at.clone()?,
        source: lookup.expires_source.clone()?,
        kind: lookup
            .expires_kind
            .clone()
            .unwrap_or_else(|| "subscription".to_string()),
        observed_at,
        stale,
    })
}

fn previous_codex_expiry_snapshot(account: &Account) -> Option<CodexSubscriptionExpirySnapshot> {
    let extra = account.quota.as_ref()?.extra_usage.as_ref()?;
    if let Some(snapshot) = extra.get("subscriptionExpirySnapshot") {
        return Some(CodexSubscriptionExpirySnapshot {
            workspace_id: snapshot.get("workspaceId")?.as_str()?.trim().to_string(),
            plan_family: snapshot.get("planFamily")?.as_str()?.trim().to_string(),
            expires_at: normalize_rfc3339_string(snapshot.get("expiresAt")?.as_str()?)?,
            source: snapshot
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("last_known_good")
                .to_string(),
            kind: snapshot
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("subscription")
                .to_string(),
            observed_at: snapshot
                .get("observedAt")
                .and_then(Value::as_i64)
                .unwrap_or_default(),
            stale: snapshot
                .get("stale")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    let subscription = extra.get("subscription")?;
    let evidence = extra.get("subscriptionEvidence")?;
    Some(CodexSubscriptionExpirySnapshot {
        workspace_id: evidence
            .get("trustedWorkspaceId")?
            .as_str()?
            .trim()
            .to_string(),
        plan_family: chatgpt_plan_family(evidence.get("usagePlanType")?.as_str()?),
        expires_at: normalize_rfc3339_string(subscription.get("expiresAt")?.as_str()?)?,
        source: subscription
            .get("expiresSource")
            .and_then(Value::as_str)
            .unwrap_or("last_known_good")
            .to_string(),
        kind: subscription
            .get("expiresKind")
            .and_then(Value::as_str)
            .unwrap_or("subscription")
            .to_string(),
        observed_at: extra
            .get("queriedAt")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        stale: false,
    })
}

fn codex_expiry_snapshot_json(snapshot: CodexSubscriptionExpirySnapshot) -> Value {
    json!({
        "workspaceId": snapshot.workspace_id,
        "planFamily": snapshot.plan_family,
        "expiresAt": snapshot.expires_at,
        "source": snapshot.source,
        "kind": snapshot.kind,
        "observedAt": snapshot.observed_at,
        "stale": snapshot.stale,
    })
}

fn parse_chatgpt_workspace_candidates(body: &Value, now_ms: i64) -> Vec<ChatGptWorkspaceCandidate> {
    let Some(accounts) = body.get("accounts").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut candidates = std::collections::BTreeMap::new();
    for (map_id, account) in accounts {
        if !chatgpt_account_is_usable(account, now_ms) {
            continue;
        }
        let Some(lookup) = chatgpt_lookup_from_account(account) else {
            continue;
        };
        let workspace_id = [
            "/account/id",
            "/account/account_id",
            "/account/chatgpt_account_id",
            "/account/organization_id",
            "/id",
            "/account_id",
            "/chatgpt_account_id",
            "/organization_id",
        ]
        .into_iter()
        .find_map(|pointer| account.pointer(pointer).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(map_id)
        .to_string();
        if !workspace_id.is_empty() {
            candidates.insert(
                workspace_id.clone(),
                ChatGptWorkspaceCandidate {
                    workspace_id,
                    lookup,
                },
            );
        }
    }
    candidates.into_values().collect()
}

fn unique_chatgpt_workspace_matching_usage(
    probe: &ChatGptSubscriptionProbe,
    usage_plan_type: Option<&str>,
) -> Option<ChatGptWorkspaceCandidate> {
    let usage_plan_type = usage_plan_type?;
    let mut matches = probe
        .workspace_candidates
        .iter()
        .filter(|candidate| {
            candidate
                .lookup
                .plan_type
                .as_deref()
                .is_some_and(|plan| chatgpt_plan_types_match(usage_plan_type, plan))
        })
        .cloned();
    let candidate = matches.next()?;
    matches.next().is_none().then_some(candidate)
}

fn parse_chatgpt_accounts_check_lookup(
    body: &Value,
    account_id: Option<&str>,
    now_ms: i64,
) -> Option<ChatGptSubscriptionLookup> {
    let accounts = body.get("accounts")?.as_object()?;
    let account_id = account_id.map(str::trim).filter(|value| !value.is_empty());

    if let Some(account_id) = account_id {
        if let Some(account) = accounts.get(account_id) {
            return chatgpt_account_is_usable(account, now_ms)
                .then(|| chatgpt_lookup_from_account(account))
                .flatten();
        }
        for account in accounts.values() {
            if chatgpt_account_matches_id(account, account_id) {
                return chatgpt_account_is_usable(account, now_ms)
                    .then(|| chatgpt_lookup_from_account(account))
                    .flatten();
            }
        }
        return None;
    }

    let mut default_candidate = None;
    let mut paid_candidate = None;
    let mut any_candidate = None;
    for account in accounts.values() {
        if !chatgpt_account_is_usable(account, now_ms) {
            continue;
        }
        let Some(lookup) = chatgpt_lookup_from_account(account) else {
            continue;
        };
        any_candidate.get_or_insert_with(|| lookup.clone());
        if default_candidate.is_none()
            && account
                .pointer("/account/is_default")
                .and_then(Value::as_bool)
                == Some(true)
        {
            default_candidate = Some(lookup.clone());
        }
        if paid_candidate.is_none()
            && lookup
                .plan_type
                .as_deref()
                .is_some_and(|plan| plan != "free")
        {
            paid_candidate = Some(lookup);
        }
    }
    default_candidate.or(paid_candidate).or(any_candidate)
}

fn parse_chatgpt_subscription_lookup(body: &Value) -> Option<ChatGptSubscriptionLookup> {
    let plan_type = body
        .get("plan_type")
        .and_then(Value::as_str)
        .map(normalize_chatgpt_plan_type)
        .filter(|value| !value.is_empty());
    let plan_label = plan_type.as_deref().map(format_chatgpt_plan_label);
    let expires_at = body
        .get("active_until")
        .and_then(Value::as_str)
        .and_then(normalize_rfc3339_string);
    if plan_type.is_none() && plan_label.is_none() && expires_at.is_none() {
        return None;
    }
    let has_expiry = expires_at.is_some();
    Some(ChatGptSubscriptionLookup {
        plan_type,
        plan_label,
        expires_at,
        expires_source: has_expiry.then(|| "subscriptions_active_until".to_string()),
        expires_kind: has_expiry.then(|| "subscription".to_string()),
        expiry_availability: None,
        expiry_stale: false,
    })
}

fn chatgpt_lookup_from_account(account: &Value) -> Option<ChatGptSubscriptionLookup> {
    let plan_type = account
        .pointer("/account/plan_type")
        .and_then(Value::as_str)
        .or_else(|| {
            account
                .pointer("/entitlement/subscription_plan")
                .and_then(Value::as_str)
        })
        .map(normalize_chatgpt_plan_type)
        .filter(|value| !value.is_empty());
    let plan_label = plan_type.as_deref().map(format_chatgpt_plan_label);
    let expires_at = account
        .pointer("/entitlement/expires_at")
        .and_then(Value::as_str)
        .and_then(normalize_rfc3339_string);
    if plan_type.is_none() && plan_label.is_none() && expires_at.is_none() {
        return None;
    }
    let has_expiry = expires_at.is_some();
    Some(ChatGptSubscriptionLookup {
        plan_type,
        plan_label,
        expires_at,
        expires_source: has_expiry.then(|| "accounts_check_entitlement".to_string()),
        expires_kind: has_expiry.then(|| "subscription".to_string()),
        expiry_availability: None,
        expiry_stale: false,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ChatGptSubscriptionResolution {
    subscription: Option<ChatGptSubscriptionLookup>,
    discarded_reasons: Vec<String>,
}

fn reconcile_chatgpt_subscription(
    usage_plan_type: Option<&str>,
    usage_allowed: Option<bool>,
    trusted_workspace: bool,
    account_lookup: Option<ChatGptSubscriptionLookup>,
    subscription_lookup: Option<ChatGptSubscriptionLookup>,
    now_ms: i64,
) -> ChatGptSubscriptionResolution {
    let usage_plan_type = usage_plan_type
        .map(normalize_chatgpt_plan_type)
        .filter(|value| !value.is_empty());
    let mut discarded_reasons = Vec::new();
    let account_lookup = constrain_chatgpt_subscription_lookup(
        account_lookup,
        usage_plan_type.as_deref(),
        usage_allowed,
        trusted_workspace,
        now_ms,
        "accounts_check",
        &mut discarded_reasons,
    );
    let mut subscription_lookup = constrain_chatgpt_subscription_lookup(
        subscription_lookup,
        usage_plan_type.as_deref(),
        usage_allowed,
        trusted_workspace,
        now_ms,
        "subscriptions",
        &mut discarded_reasons,
    );

    if usage_plan_type.is_none()
        && account_lookup
            .as_ref()
            .and_then(|lookup| lookup.plan_type.as_deref())
            .zip(
                subscription_lookup
                    .as_ref()
                    .and_then(|lookup| lookup.plan_type.as_deref()),
            )
            .is_some_and(|(left, right)| !chatgpt_plan_types_match(left, right))
    {
        subscription_lookup = None;
        discarded_reasons.push("subscription_sources_plan_mismatch".to_string());
    }

    let mut subscription = merge_subscription_lookup(account_lookup, subscription_lookup);
    if let Some(usage_plan_type) = usage_plan_type {
        let resolved = subscription.get_or_insert_with(ChatGptSubscriptionLookup::default);
        resolved.plan_label = Some(format_chatgpt_plan_label(&usage_plan_type));
        resolved.plan_type = Some(usage_plan_type);
    }

    ChatGptSubscriptionResolution {
        subscription,
        discarded_reasons,
    }
}

#[allow(clippy::too_many_arguments)]
fn constrain_chatgpt_subscription_lookup(
    mut lookup: Option<ChatGptSubscriptionLookup>,
    usage_plan_type: Option<&str>,
    usage_allowed: Option<bool>,
    trusted_workspace: bool,
    now_ms: i64,
    source: &str,
    discarded_reasons: &mut Vec<String>,
) -> Option<ChatGptSubscriptionLookup> {
    let item = lookup.as_mut()?;
    if usage_plan_type
        .zip(item.plan_type.as_deref())
        .is_some_and(|(usage_plan, lookup_plan)| !chatgpt_plan_types_match(usage_plan, lookup_plan))
    {
        discarded_reasons.push(format!("{source}_plan_mismatch"));
        return None;
    }

    if item.expires_at.is_some() && !trusted_workspace {
        item.clear_expiry();
        discarded_reasons.push(format!("{source}_untrusted_workspace_expiry"));
    } else if item
        .expires_at
        .as_deref()
        .is_some_and(|expires_at| chatgpt_expiry_is_past(expires_at, now_ms))
        && usage_allowed == Some(true)
        && usage_plan_type.is_some_and(chatgpt_plan_is_paid)
    {
        item.clear_expiry();
        discarded_reasons.push(format!("{source}_expired_while_usage_available"));
    }

    lookup
}

impl ChatGptSubscriptionLookup {
    fn clear_expiry(&mut self) {
        self.expires_at = None;
        self.expires_source = None;
        self.expires_kind = None;
    }
}

fn merge_subscription_lookup(
    primary: Option<ChatGptSubscriptionLookup>,
    fallback: Option<ChatGptSubscriptionLookup>,
) -> Option<ChatGptSubscriptionLookup> {
    match (primary, fallback) {
        (Some(mut primary), Some(fallback)) => {
            if primary.plan_type.is_none() {
                primary.plan_type = fallback.plan_type;
            }
            if primary.plan_label.is_none() {
                primary.plan_label = fallback.plan_label;
            }
            if primary.expires_at.is_none() {
                primary.expires_at = fallback.expires_at;
                primary.expires_source = fallback.expires_source;
                primary.expires_kind = fallback.expires_kind;
            }
            Some(primary)
        }
        (Some(primary), None) => Some(primary),
        (None, fallback) => fallback,
    }
}

fn chatgpt_account_matches_id(account: &Value, account_id: &str) -> bool {
    [
        "/account/id",
        "/account/account_id",
        "/account/chatgpt_account_id",
        "/account/organization_id",
        "/id",
        "/account_id",
        "/chatgpt_account_id",
        "/organization_id",
    ]
    .iter()
    .any(|path| account.pointer(path).and_then(Value::as_str) == Some(account_id))
}

fn chatgpt_account_is_usable(account: &Value, now_ms: i64) -> bool {
    if [
        Some(account),
        account.get("account"),
        account.get("entitlement"),
    ]
    .into_iter()
    .flatten()
    .any(has_chatgpt_account_inactive_marker)
    {
        return false;
    }

    account
        .pointer("/entitlement/expires_at")
        .and_then(Value::as_str)
        .and_then(rfc3339_to_unix_ms)
        .is_none_or(|expires_at_ms| expires_at_ms > now_ms)
}

fn has_chatgpt_account_inactive_marker(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if ["deactivated", "is_deactivated", "disabled", "is_disabled"]
        .into_iter()
        .any(|key| object.get(key).and_then(Value::as_bool) == Some(true))
    {
        return true;
    }
    if ["deactivated_at", "disabled_at", "deleted_at"]
        .into_iter()
        .any(|key| {
            object
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
        })
    {
        return true;
    }
    ["status", "state"].into_iter().any(|key| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "deactivated" | "disabled" | "deleted" | "inactive" | "suspended" | "expired"
                )
            })
    })
}

fn chatgpt_expiry_is_past(expires_at: &str, now_ms: i64) -> bool {
    rfc3339_to_unix_ms(expires_at).is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
}

fn chatgpt_plan_is_paid(plan: &str) -> bool {
    chatgpt_plan_family(plan) != "free"
}

fn chatgpt_plan_types_match(left: &str, right: &str) -> bool {
    chatgpt_plan_family(left) == chatgpt_plan_family(right)
}

fn chatgpt_plan_family(plan: &str) -> String {
    match normalize_chatgpt_plan_type(plan).as_str() {
        "team" | "business" | "self_serve_business" | "self_serve_business_usage_based" => {
            "business".to_string()
        }
        "enterprise" | "hc" | "enterprise_cbp_usage_based" => "enterprise".to_string(),
        "edu" | "education" | "edu_plus" | "edu_pro" => "edu".to_string(),
        "prolite" | "pro_lite" => "pro_lite".to_string(),
        normalized => normalized.to_string(),
    }
}

fn codex_account_id(account: &Account) -> Option<String> {
    crate::domain::accounts::store::effective_codex_workspace_id(account)
}

#[derive(Debug, Deserialize)]
struct CodexUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<CodexRateLimit>,
    credits: Option<CodexPersonalCredits>,
}

#[derive(Debug, Deserialize)]
struct CodexPersonalCredits {
    #[serde(default)]
    has_credits: Option<bool>,
    #[serde(default)]
    unlimited: Option<bool>,
    #[serde(default)]
    overage_limit_reached: Option<bool>,
    #[serde(default)]
    balance: Option<Value>,
    #[serde(default)]
    approx_local_messages: Vec<i64>,
    #[serde(default)]
    approx_cloud_messages: Vec<i64>,
}

fn codex_personal_credits_projection(credits: &CodexPersonalCredits) -> Value {
    let balance = credits.balance.as_ref().and_then(decimal_value_string);
    let balance_positive = balance.as_deref().and_then(decimal_string_is_positive);
    let available = credits.unlimited == Some(true)
        || (credits.has_credits == Some(true)
            && credits.overage_limit_reached != Some(true)
            && balance_positive == Some(true));
    json!({
        "hasCredits": credits.has_credits,
        "unlimited": credits.unlimited,
        "overageLimitReached": credits.overage_limit_reached,
        "balance": balance,
        "balancePositive": balance_positive,
        "available": available,
        "approxLocalMessages": credits.approx_local_messages,
        "approxCloudMessages": credits.approx_cloud_messages,
    })
}

fn decimal_value_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.trim().to_string()).filter(|value| !value.is_empty()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn decimal_string_is_positive(value: &str) -> Option<bool> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let value = value.strip_prefix('+').unwrap_or(value);
    if value.starts_with('-') || value.is_empty() {
        return Some(false);
    }
    let mut decimal_points = 0_u8;
    let mut digits = 0_usize;
    let mut non_zero = false;
    for character in value.chars() {
        match character {
            '0'..='9' => {
                digits = digits.saturating_add(1);
                non_zero |= character != '0';
            }
            '.' if decimal_points == 0 => decimal_points = 1,
            _ => return None,
        }
    }
    (digits > 0).then_some(non_zero)
}

#[derive(Debug, Deserialize)]
struct CodexRateLimit {
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    limit_reached: Option<bool>,
    #[serde(default, alias = "primary")]
    primary_window: Option<CodexRateLimitWindow>,
    #[serde(default, alias = "secondary")]
    secondary_window: Option<CodexRateLimitWindow>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimitWindow {
    #[serde(default, alias = "percent_used")]
    used_percent: Option<f64>,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_after_seconds: Option<i64>,
    #[serde(default, alias = "resets_at", alias = "resetAt")]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct ClaudeUsageWindow {
    utilization: Option<f64>,
    resets_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiLoadCodeAssistResponse {
    #[serde(rename = "cloudaicompanionProject")]
    cloudaicompanion_project: Option<Value>,
    #[serde(rename = "paidTier")]
    paid_tier: Option<Value>,
    #[serde(rename = "currentTier")]
    current_tier: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct GeminiQuotaResponse {
    buckets: Option<Vec<GeminiBucketInfo>>,
}

#[derive(Debug, Deserialize)]
struct GeminiBucketInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotImportedUsage {
    #[serde(default, alias = "copilotPlan")]
    copilot_plan: Option<String>,
    #[serde(default, alias = "quotaResetDate")]
    quota_reset_date: Option<String>,
    #[serde(default, alias = "quotaSnapshots")]
    quota_snapshots: Option<CopilotQuotaSnapshots>,
}

#[derive(Debug, Deserialize)]
struct CopilotQuotaSnapshots {
    #[serde(default, alias = "premiumInteractions")]
    premium_interactions: Option<CopilotQuotaDetail>,
}

#[derive(Debug, Clone, Deserialize)]
struct CopilotQuotaDetail {
    #[serde(default)]
    entitlement: Option<f64>,
    #[serde(default)]
    total: Option<f64>,
    #[serde(default)]
    remaining: Option<f64>,
    #[serde(default)]
    used: Option<f64>,
    #[serde(default, alias = "percentRemaining")]
    percent_remaining: Option<f64>,
    #[serde(default)]
    unlimited: Option<bool>,
}

fn normalize_claude_tier_name(name: &str) -> &str {
    match name {
        "seven_day_omelette" => "seven_day_opus",
        "seven_day_overage_included" | "seven_day_oi" | "7d_oi" => CLAUDE_FABLE_QUOTA_TIER,
        _ => name,
    }
}

fn normalize_chatgpt_plan_type(plan: &str) -> String {
    plan.trim().to_ascii_lowercase().replace(['-', ' '], "_")
}

fn format_chatgpt_plan_label(plan: &str) -> String {
    match normalize_chatgpt_plan_type(plan).as_str() {
        "free" => "ChatGPT Free".to_string(),
        "plus" => "ChatGPT Plus".to_string(),
        "prolite" | "pro_lite" => "ChatGPT Pro 5x".to_string(),
        "pro" => "ChatGPT Pro 20x".to_string(),
        "team" => "ChatGPT Team".to_string(),
        "business" | "self_serve_business" | "self_serve_business_usage_based" => {
            "ChatGPT Business".to_string()
        }
        "enterprise" | "hc" | "enterprise_cbp_usage_based" => "ChatGPT Enterprise".to_string(),
        "edu" | "education" | "edu_plus" | "edu_pro" => "ChatGPT Edu".to_string(),
        _ => plan.trim().to_string(),
    }
}

fn window_seconds_to_tier_name(secs: i64) -> String {
    match secs {
        18_000 => "five_hour".to_string(),
        604_800 => "seven_day".to_string(),
        2_592_000 => "30_day".to_string(),
        value => {
            let hours = value / 3600;
            if hours >= 24 {
                format!("{}_day", hours / 24)
            } else {
                format!("{}_hour", hours)
            }
        }
    }
}

fn extract_project_id(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_string())
        }
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("projectId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        _ => None,
    }
}

fn classify_gemini_model(model_id: &str) -> &str {
    if model_id.contains("flash-lite") {
        "gemini_flash_lite"
    } else if model_id.contains("flash") {
        "gemini_flash"
    } else if model_id.contains("pro") {
        "gemini_pro"
    } else {
        model_id
    }
}

fn gemini_sort_order(name: &str) -> usize {
    match name {
        "gemini_pro" => 0,
        "gemini_flash" => 1,
        "gemini_flash_lite" => 2,
        _ => 3,
    }
}

fn require_imported_snapshot(
    account: &Account,
    pointers: &[&str],
    label: &str,
) -> Result<Value, QuotaRefreshFailure> {
    let raw = account.raw.as_ref().ok_or_else(|| {
        QuotaRefreshFailure::bad_request(format!(
            "{} account requires an imported raw {} snapshot",
            account.provider_type.as_str(),
            label
        ))
    })?;
    value_at(raw, pointers).ok_or_else(|| {
        QuotaRefreshFailure::bad_request(format!(
            "{} account raw data is missing imported {} snapshot",
            account.provider_type.as_str(),
            label
        ))
    })
}

fn codex_banked_reset_status_from_account(account: &Account) -> Option<Value> {
    if let Some(cached) = account.quota.as_ref().and_then(|quota| {
        quota
            .extra_usage
            .as_ref()
            .and_then(|extra| value_at(extra, &["/bankedReset", "/codexBankedReset"]))
    }) {
        let cached = if cached.get("countSource").is_some() && cached.get("detailsSource").is_some()
        {
            cached
        } else {
            normalize_imported_snapshot(&cached)
        };
        let cached_workspace = string_at(&cached, &["/workspaceId", "/workspace_id"]);
        if cached_workspace != codex_account_id(account) {
            return None;
        }
        return Some(cached);
    }

    account.raw.as_ref().and_then(|raw| {
        value_at(
            raw,
            &[
                "/bankedReset",
                "/banked_reset",
                "/codexBankedReset",
                "/codex_banked_reset",
                "/rateLimitResetCredits",
                "/rate_limit_reset_credits",
            ],
        )
        .map(|source| normalize_imported_snapshot(&source))
        .filter(|snapshot| {
            string_at(snapshot, &["/workspaceId", "/workspace_id"]) == codex_account_id(account)
        })
    })
}

pub fn codex_banked_reset_status_snapshot(account: &Account, _now_ms: i64) -> Value {
    codex_banked_reset_status_from_account(account).unwrap_or_else(|| {
        crate::clients::oauth::codex_reset_credits::empty_snapshot(
            codex_account_id(account).as_deref(),
        )
    })
}

fn value_at(value: &Value, pointers: &[&str]) -> Option<Value> {
    pointers.iter().find_map(|pointer| {
        if pointer.is_empty() {
            return Some(value.clone());
        }
        value.pointer(pointer).cloned()
    })
}

fn string_at(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn latest_timestamp_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .filter_map(|pointer| {
            let value = value.pointer(pointer)?;
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
                .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        })
        .filter(|value| *value > 0)
        .max()
}

fn number_at(value: &Value, pointers: &[&str]) -> Option<f64> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .or_else(|| value.as_u64().map(|value| value as f64))
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .filter(|value| value.is_finite())
    })
}

fn valid_time_field(value: &Value, pointer: &str) -> Option<String> {
    let field = value.pointer(pointer)?;
    let valid = field
        .get("Valid")
        .or_else(|| field.get("valid"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if !valid {
        return None;
    }
    field
        .get("Time")
        .or_else(|| field.get("time"))
        .and_then(Value::as_str)
        .and_then(normalize_rfc3339_string)
}

fn normalize_rfc3339_string(value: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.to_rfc3339())
}

fn rfc3339_to_unix_ms(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|dt| dt.with_timezone(&Utc).timestamp_millis())
}

fn dateish_to_unix_ms(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    rfc3339_to_unix_ms(trimmed).or_else(|| {
        let date = chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d").ok()?;
        date.and_hms_opt(0, 0, 0)
            .map(|dt| dt.and_utc().timestamp_millis())
    })
}

fn timestamp_number_to_unix_ms(value: f64) -> Option<i64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    if value > 1_000_000_000_000.0 {
        Some(value.round() as i64)
    } else {
        Some((value * 1000.0).round() as i64)
    }
}

fn format_copilot_plan_label(plan: &str) -> String {
    match plan.trim().to_ascii_lowercase().as_str() {
        "individual" => "Copilot Individual".to_string(),
        "business" => "Copilot Business".to_string(),
        "enterprise" => "Copilot Enterprise".to_string(),
        "free" => "Copilot Free".to_string(),
        other if !other.is_empty() => format!("Copilot {other}"),
        _ => "GitHub Copilot".to_string(),
    }
}

fn percent_to_fraction(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    if value > 1.0 {
        (value / 100.0).clamp(0.0, 1.0)
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn parse_retry_after_ms(value: &str) -> Option<i64> {
    let trimmed = value.trim();
    if let Ok(seconds) = trimmed.parse::<i64>() {
        return (seconds >= 0).then_some(seconds.saturating_mul(1000));
    }
    let retry_at = DateTime::parse_from_rfc2822(trimmed)
        .ok()?
        .with_timezone(&Utc);
    let diff = retry_at - Utc::now();
    Some(diff.num_milliseconds().max(0))
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Debug, Clone)]
    struct GeminiCodeAssistObservation {
        operation: &'static str,
        authorization: String,
        user_agent: String,
        x_goog_api_client: Option<String>,
        client_metadata: Option<String>,
        body: Value,
    }

    async fn serve_oversized_json_response() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        MAX_QUOTA_RESPONSE_BODY_BYTES + 1
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        });
        (format!("http://{address}/quota"), server)
    }

    async fn serve_gemini_code_assist(
        load_status: axum::http::StatusCode,
        load_body: Value,
        quota_status: axum::http::StatusCode,
        quota_body: Value,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<GeminiCodeAssistObservation>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observations_for_route = std::sync::Arc::clone(&observations);
        let app = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: bytes::Bytes| {
                let observations = std::sync::Arc::clone(&observations_for_route);
                let (operation, status, response_body) = match uri.path() {
                    "/v1internal:loadCodeAssist" => {
                        ("loadCodeAssist", load_status, load_body.clone())
                    }
                    "/v1internal:retrieveUserQuota" => {
                        ("retrieveUserQuota", quota_status, quota_body.clone())
                    }
                    _ => (
                        "unknown",
                        axum::http::StatusCode::NOT_FOUND,
                        json!({"error": "not found"}),
                    ),
                };
                async move {
                    observations
                        .lock()
                        .unwrap()
                        .push(gemini_observation(operation, &headers, &body));
                    gemini_test_response(status, response_body)
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), observations, server)
    }

    async fn serve_antigravity_code_assist(
        quota_body: Value,
        privacy_status: axum::http::StatusCode,
        privacy_body: Value,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<GeminiCodeAssistObservation>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observations_for_route = std::sync::Arc::clone(&observations);
        let app = axum::Router::new().fallback(axum::routing::post(
            move |uri: axum::http::Uri, headers: axum::http::HeaderMap, body: bytes::Bytes| {
                let observations = std::sync::Arc::clone(&observations_for_route);
                let (operation, status, response_body) = match uri.path() {
                    "/v1internal:loadCodeAssist" => (
                        "loadCodeAssist",
                        axum::http::StatusCode::OK,
                        json!({
                            "cloudaicompanionProject": {"id": "antigravity-project"},
                            "currentTier": {"name": "PRO"}
                        }),
                    ),
                    "/v1internal:retrieveUserQuota" => (
                        "retrieveUserQuota",
                        axum::http::StatusCode::OK,
                        quota_body.clone(),
                    ),
                    "/v1internal:fetchUserInfo" => {
                        ("fetchUserInfo", privacy_status, privacy_body.clone())
                    }
                    _ => (
                        "unknown",
                        axum::http::StatusCode::NOT_FOUND,
                        json!({"error": "not found"}),
                    ),
                };
                async move {
                    observations
                        .lock()
                        .unwrap()
                        .push(gemini_observation(operation, &headers, &body));
                    gemini_test_response(status, response_body)
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), observations, server)
    }

    fn gemini_observation(
        operation: &'static str,
        headers: &axum::http::HeaderMap,
        body: &[u8],
    ) -> GeminiCodeAssistObservation {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        GeminiCodeAssistObservation {
            operation,
            authorization: header("authorization").unwrap_or_default(),
            user_agent: header("user-agent").unwrap_or_default(),
            x_goog_api_client: header("x-goog-api-client"),
            client_metadata: header("client-metadata"),
            body: serde_json::from_slice(body).unwrap(),
        }
    }

    fn gemini_test_response(
        status: axum::http::StatusCode,
        body: Value,
    ) -> axum::response::Response {
        axum::http::Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    }

    fn gemini_code_assist_account(
        provider_type: ProviderType,
        raw: Value,
        profile: Option<Value>,
    ) -> Account {
        let mut account = imported_account(provider_type, raw);
        account.access_token = Some("gemini-code-assist-access".to_string());
        account.profile = profile;
        account
    }

    #[tokio::test]
    async fn quota_response_body_limit_is_enforced_before_json_parsing() {
        let (url, server) = serve_oversized_json_response().await;
        let failure = request_json(
            ProviderType::CodexOAuth,
            reqwest::Client::new().get(url),
            1_000,
        )
        .await
        .unwrap_err();

        assert_eq!(failure.status_code, 502);
        assert!(!failure.retryable);
        assert!(failure.message.contains("response body exceeds"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn chatgpt_probe_response_body_limit_is_enforced() {
        let (url, server) = serve_oversized_json_response().await;
        let mut response = reqwest::Client::new().get(url).send().await.unwrap();
        let status = read_chatgpt_probe_json(&mut response).await.unwrap_err();

        assert_eq!(status, ChatGptProbeStatus::ParseError);
        server.await.unwrap();
    }

    #[test]
    fn quota_upstream_errors_redact_reflected_credentials() {
        let failure = QuotaRefreshFailure::upstream(
            ProviderType::CodexOAuth,
            reqwest::StatusCode::BAD_GATEWAY,
            r#"{"access_token":"should-not-escape"}"#.to_string(),
            None,
            1_000,
        );

        assert!(!failure.message.contains("should-not-escape"));
        assert!(failure.message.contains("[REDACTED]"));
        assert_eq!(failure.upstream_status, Some(502));
        assert!(failure.partial_update.is_none());
    }

    #[test]
    fn trae_quota_projects_only_entitlement_usage_and_never_installs_routing_cooldown() {
        let now_ms = 1_700_000_000_000;
        let expires_at = now_ms + 86_400_000;
        let account = imported_account(ProviderType::TraeSolo, json!({}));
        let update = parse_trae_quota_update(
            &account,
            &json!({
                "Result": {
                    "user_entitlement_pack_list": [{
                        "uid": "must-not-be-projected",
                        "device_id": "must-not-be-projected",
                        "access_token": "must-not-be-projected",
                        "entitlement_base_info": {
                            "plan_name": "Trae Solo Pro",
                            "status": "active",
                            "period": "monthly",
                            "period_start": now_ms,
                            "period_end": expires_at,
                            "quota": {"credits_limit": 2_000}
                        },
                        "usage": {"credits_amount": 500}
                    }]
                }
            }),
            now_ms,
            300_000,
        )
        .unwrap();
        assert_eq!(update.subscription_level.as_deref(), Some("Trae Solo Pro"));
        assert_eq!(update.entitlement_status.as_deref(), Some("available"));
        assert_eq!(update.quota_percent, Some(25.0));
        assert!(update.rate_limited_until.is_none());
        assert!(update.clear_rate_limited_until_if.is_none());
        let quota = update.quota.unwrap();
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].used, Some(500.0));
        assert_eq!(quota.tiers[0].limit, Some(2_000.0));
        assert_eq!(quota.tiers[0].resets_at, Some(expires_at));
        let projection = quota.extra_usage.unwrap().to_string();
        assert!(projection.contains("Trae Solo Pro"));
        assert!(projection.contains("1500"));
        assert!(!projection.contains("must-not-be-projected"));
        assert!(!projection.contains("access_token"));
        assert!(!projection.contains("device_id"));
    }

    #[test]
    fn trae_quota_empty_or_non_authoritative_packs_are_unknown_errors_not_exhaustion() {
        let account = imported_account(ProviderType::TraeSolo, json!({}));
        for value in [
            json!({"data": {"user_entitlement_pack_list": []}}),
            json!({"user_entitlement_pack_list": [{
                "entitlement_base_info": {"quota": {"credits_limit": 0}},
                "usage": {"credits_amount": 0}
            }]}),
            json!({"user_entitlement_pack_list": [{
                "entitlement_base_info": {"quota": {"credits_limit": 100}}
            }]}),
            json!({"result": {}}),
        ] {
            let error = parse_trae_quota_update(&account, &value, 1_000, 300_000).unwrap_err();
            assert_eq!(error.status_code, 502);
            assert!(error.partial_update.is_none());
            assert!(!error.message.contains("exhausted"));
        }
    }

    #[test]
    fn codebuddy_quota_sorts_packages_normalizes_units_and_projects_only_safe_fields() {
        let now_ms = 1_700_000_000_000;
        let account = imported_account(ProviderType::CodeBuddyOAuth, json!({}));
        let update = parse_codebuddy_quota_update(
            &account,
            &json!({
                "code": 0,
                "data": {"Response": {"Data": {
                    "TotalCount": 2,
                    "TotalDosage": 350,
                    "Accounts": [
                        {
                            "PackageName": "Bonus Pack",
                            "SubProductCode": "bonus",
                            "CapacitySize": "250",
                            "CapacityRemain": 200,
                            "CapacityUnit": "credits",
                            "CycleEndTime": "2026-09-14 14:15:28",
                            "Uin": "sensitive-uin",
                            "AppId": "sensitive-app",
                            "AccountId": "sensitive-account",
                            "DealName": "sensitive-deal",
                            "ResourceId": "sensitive-resource",
                            "AccountAttributes": [{"payerUin": "sensitive-payer"}]
                        },
                        {
                            "PackageName": "Free Plan Subscription",
                            "SubProductCode": "free",
                            "CapacitySize": 100,
                            "CapacityRemain": 100,
                            "CapacityUnit": "credit",
                            "CycleEndTime": "2026-08-31 23:59:59"
                        }
                    ]
                }}}
            }),
            now_ms,
            300_000,
        )
        .unwrap();
        assert_eq!(
            update.subscription_level.as_deref(),
            Some("Free Plan Subscription")
        );
        assert_eq!(update.entitlement_status.as_deref(), Some("available"));
        assert!(update.rate_limited_until.is_none());
        assert!(update.clear_rate_limited_until_if.is_none());
        let quota = update.quota.unwrap();
        assert_eq!(quota.tiers.len(), 2);
        assert_eq!(
            quota.tiers[0].label.as_deref(),
            Some("Free Plan Subscription")
        );
        assert_eq!(quota.tiers[0].unit.as_deref(), Some("credits"));
        assert_eq!(quota.tiers[0].used, Some(0.0));
        assert_eq!(quota.tiers[0].limit, Some(100.0));
        assert_eq!(quota.tiers[1].used, Some(50.0));
        assert_eq!(quota.tiers[1].limit, Some(250.0));
        assert!(quota.tiers[0].resets_at < quota.tiers[1].resets_at);
        let projection = quota.extra_usage.unwrap().to_string();
        assert!(projection.contains("\"totalDosage\":350.0"));
        assert!(projection.contains("Free Plan Subscription"));
        for forbidden in [
            "sensitive-uin",
            "sensitive-app",
            "sensitive-account",
            "sensitive-deal",
            "sensitive-resource",
            "sensitive-payer",
            "payerUin",
            "ResourceId",
        ] {
            assert!(!projection.contains(forbidden), "{forbidden}: {projection}");
        }
    }

    #[test]
    fn codebuddy_quota_missing_or_inconsistent_authority_is_unknown_not_exhausted() {
        let account = imported_account(ProviderType::CodeBuddyOAuth, json!({}));
        for value in [
            json!({"data":{"Response":{"Data":{"TotalCount":0,"TotalDosage":0,"Accounts":[]}}}}),
            json!({"data":{"Response":{"Data":{"TotalCount":2,"TotalDosage":100,"Accounts":[{
                "PackageName":"Plan","CapacitySize":100,"CapacityRemain":100,"CapacityUnit":"credit","CycleEndTime":"2026-01-01 00:00:00"
            }]}}}}),
            json!({"data":{"Response":{"Data":{"TotalCount":1,"TotalDosage":100,"Accounts":[{
                "PackageName":"Plan","CapacitySize":100,"CapacityRemain":101,"CapacityUnit":"credit","CycleEndTime":"2026-01-01 00:00:00"
            }]}}}}),
            json!({"data":{"Response":{"Data":{"TotalCount":1,"TotalDosage":100,"Accounts":[{
                "PackageName":"Plan","CapacitySize":100,"CapacityRemain":50,"CapacityUnit":"points","CycleEndTime":"2026-01-01 00:00:00"
            }]}}}}),
            json!({"data":{"Response":{"Data":{"TotalCount":1,"TotalDosage":0,"Accounts":[{
                "PackageName":"Plan","CapacitySize":0,"CapacityRemain":0,"CapacityUnit":"credits","CycleEndTime":"2026-01-01 00:00:00"
            }]}}}}),
            json!({"data":{"Response":{"Data":{"TotalCount":1,"Accounts":[]}}}}),
        ] {
            let error = parse_codebuddy_quota_update(&account, &value, 1_000, 300_000).unwrap_err();
            assert_eq!(error.status_code, 502);
            assert!(error.partial_update.is_none());
            assert!(!error.message.contains("exhausted"));
        }
    }

    #[tokio::test]
    async fn codebuddy_quota_dispatch_uses_only_the_fixed_billing_override_and_identity() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let route_observations = std::sync::Arc::clone(&observations);
        let app = axum::Router::new().route(
            crate::domain::codebuddy::CODEBUDDY_RESOURCE_PATH,
            axum::routing::post(
                move |headers: axum::http::HeaderMap, body: bytes::Bytes| {
                    let observations = std::sync::Arc::clone(&route_observations);
                    async move {
                        observations.lock().unwrap().push(json!({
                            "authorization": headers.get("authorization").and_then(|value| value.to_str().ok()),
                            "uid": headers.get("x-user-id").and_then(|value| value.to_str().ok()),
                            "domain": headers.get("x-domain").and_then(|value| value.to_str().ok()),
                            "body": serde_json::from_slice::<Value>(&body).unwrap(),
                        }));
                        axum::Json(json!({
                            "code": 0,
                            "data": {"Response": {"Data": {
                                "TotalCount": 1,
                                "TotalDosage": 100,
                                "Accounts": [{
                                    "PackageName": "Intl Plan",
                                    "SubProductCode": "intl",
                                    "CapacitySize": 100,
                                    "CapacityRemain": 75,
                                    "CapacityUnit": "credit",
                                    "CycleEndTime": "2027-01-01 00:00:00"
                                }]
                            }}}
                        }))
                    }
                },
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut account = imported_account(
            ProviderType::CodeBuddyOAuth,
            json!({
                "testCodeBuddyBaseUrl": "http://127.0.0.1:1",
                "testCodeBuddyBillingBaseUrl": format!("http://{address}")
            }),
        );
        account.access_token = Some("codebuddy-billing-access".to_string());
        account.refresh_token = Some("codebuddy-billing-refresh".to_string());
        account.profile = Some(json!({
            "site": "intl",
            "domain": "www.codebuddy.ai",
            "uid": "billing-user",
            "enterpriseId": "",
            "clientVersion": crate::domain::codebuddy::CODEBUDDY_CLIENT_VERSION,
            "productPlatform": crate::domain::codebuddy::CODEBUDDY_PLATFORM
        }));
        let result = refresh_account_quota(
            &reqwest::Client::new(),
            &account,
            1_700_000_000_000,
            true,
            300_000,
            2_000,
        )
        .await
        .unwrap();
        let QuotaRefreshResult::Updated { update, .. } = result else {
            panic!("forced CodeBuddy quota refresh must execute")
        };
        assert_eq!(update.entitlement_status.as_deref(), Some("available"));
        assert_eq!(update.quota_percent, Some(25.0));
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0]["authorization"],
            "Bearer codebuddy-billing-access"
        );
        assert_eq!(observations[0]["uid"], "billing-user");
        assert_eq!(observations[0]["domain"], "www.codebuddy.ai");
        assert_eq!(observations[0]["body"]["ProductCode"], "p_tcaca");
        assert_eq!(observations[0]["body"]["Status"], json!([0, 3]));
        assert_eq!(observations[0]["body"]["PageSize"], 100);
        drop(observations);
        server.abort();
    }

    #[test]
    fn frozen_qoder_cli_oracle_quota_case_drives_the_native_parser() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../assets/contract/qoder-cli-oracle.json"
        ))
        .expect("Qoder CLI oracle fixture must be valid JSON");
        let quota_case = &fixture["quotaCase"];
        let now_ms = 1_700_000_000_000;
        let account = imported_account(ProviderType::QoderCosy, json!({}));
        let update =
            parse_qoder_quota_update(&account, &quota_case["input"], now_ms, 300_000).unwrap();
        let quota = update.quota.as_ref().unwrap();
        let mut bucket_ids = quota
            .tiers
            .iter()
            .map(|tier| tier.name.as_str())
            .collect::<Vec<_>>();
        bucket_ids.sort_unstable();
        assert_eq!(
            bucket_ids,
            quota_case["expectedBucketIds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap())
                .collect::<Vec<_>>()
        );
        let projection = quota.extra_usage.as_ref().unwrap();
        assert_eq!(projection["qoderQuota"]["availability"], "available");
        assert_eq!(projection["qoderQuota"]["exhausted"], false);
        if quota_case["informationalOnly"] == true {
            assert_eq!(update.rate_limited_until, None);
            assert_eq!(update.clear_rate_limited_until_if, None);
        }

        let mut missing_balance = quota_case["input"].clone();
        for field in ["userQuota", "addOnQuota"] {
            let bucket = missing_balance[field].as_object_mut().unwrap();
            bucket.remove("used");
            bucket.remove("remaining");
        }
        let missing = parse_qoder_quota_update(&account, &missing_balance, now_ms, 300_000)
            .unwrap()
            .quota
            .unwrap()
            .extra_usage
            .unwrap();
        assert_eq!(
            missing["qoderQuota"]["availability"],
            quota_case["missingBalanceState"]
        );
        assert_eq!(missing["qoderQuota"]["exhausted"], false);
    }

    #[test]
    fn qoder_quota_preserves_add_on_and_org_balances_before_limiting() {
        let now_ms = 1_700_000_000_000;
        let reset_at = now_ms + 3_600_000;
        let account = imported_account(ProviderType::QoderCosy, json!({}));
        let update = parse_qoder_quota_update(
            &account,
            &json!({
                "userType": "teams",
                "usageType": "credits",
                "isQuotaExceeded": true,
                "expiresAt": reset_at,
                "userQuota": {"total": 100, "used": 100, "remaining": 0, "percentage": 100},
                "add_on_quota": {"cap": 50, "used": 10, "remaining": 40},
                "sharedQuota": {"used": 25, "remaining": 75, "unit": "credits"}
            }),
            now_ms,
            300_000,
        )
        .unwrap();

        assert_eq!(update.rate_limited_until, None);
        assert_eq!(update.clear_rate_limited_until_if, Some(reset_at));
        assert_eq!(update.quota_percent, Some(54.0));
        let quota = update.quota.unwrap();
        assert_eq!(quota.tiers.len(), 3);
        assert_eq!(quota.tiers[0].name, "qoder_user");
        assert_eq!(quota.tiers[1].name, "qoder_add_on");
        assert_eq!(quota.tiers[1].limit, Some(50.0));
        assert_eq!(quota.tiers[2].name, "qoder_organization");
        assert_eq!(quota.tiers[2].limit, Some(100.0));
        let extra = quota.extra_usage.unwrap();
        assert_eq!(extra["qoderQuota"]["availability"], "available");
        assert_eq!(extra["qoderQuota"]["exhausted"], false);
        assert_eq!(
            extra["qoderQuota"]["buckets"]["qoder_add_on"]["source"],
            "add_on_quota"
        );
        assert_eq!(
            extra["qoderQuota"]["buckets"]["qoder_organization"]["source"],
            "sharedQuota"
        );
    }

    #[test]
    fn qoder_quota_limits_only_when_every_present_bucket_is_exhausted() {
        let now_ms = 1_700_000_000_000;
        let reset_at = now_ms + 3_600_000;
        let account = imported_account(ProviderType::QoderCosy, json!({}));
        let update = parse_qoder_quota_update(
            &account,
            &json!({
                "userType": "teams",
                "isQuotaExceeded": true,
                "expiresAt": reset_at,
                "userQuota": {"total": 100, "used": 100},
                "addOnQuota": {"total": 50, "used": 50, "remaining": 0},
                "orgResourcePackage": {"cap": 25, "used": 25, "remaining": 0}
            }),
            now_ms,
            300_000,
        )
        .unwrap();

        assert_eq!(update.rate_limited_until, Some(reset_at));
        assert_eq!(update.clear_rate_limited_until_if, None);
        assert_eq!(update.quota_percent, Some(100.0));
        assert_eq!(
            update.quota.unwrap().extra_usage.unwrap()["qoderQuota"]["exhausted"],
            true
        );
    }

    #[test]
    fn qoder_quota_missing_balance_and_personal_zero_are_unknown_not_exhausted() {
        let now_ms = 1_700_000_000_000;
        let reset_at = now_ms + 3_600_000;
        let account = imported_account(ProviderType::QoderCosy, json!({}));

        let incomplete = parse_qoder_quota_update(
            &account,
            &json!({
                "userType": "teams",
                "isQuotaExceeded": true,
                "expiresAt": reset_at,
                "userQuota": {"total": 100, "used": 100},
                "orgResourcePackage": {"available": true}
            }),
            now_ms,
            300_000,
        )
        .unwrap();
        assert_eq!(incomplete.rate_limited_until, None);
        assert_eq!(
            incomplete.quota.unwrap().extra_usage.unwrap()["qoderQuota"]["availability"],
            "unknown"
        );

        let personal_zero = parse_qoder_quota_update(
            &account,
            &json!({
                "userType": "personal_standard",
                "isQuotaExceeded": true,
                "expiresAt": 253402214400000_i64,
                "userQuota": {"total": 0, "used": 0, "remaining": 0, "percentage": 0}
            }),
            now_ms,
            300_000,
        )
        .unwrap();
        assert_eq!(personal_zero.rate_limited_until, None);
        let extra = personal_zero.quota.unwrap().extra_usage.unwrap();
        assert_eq!(extra["qoderQuota"]["personalZeroUnknown"], true);
        assert_eq!(extra["qoderQuota"]["availability"], "unknown");
    }

    #[test]
    fn qoder_quota_rejects_conflicting_aliases_and_invalid_numbers() {
        let account = imported_account(ProviderType::QoderCosy, json!({}));
        let aliases = parse_qoder_quota_update(
            &account,
            &json!({
                "addOnQuota": {"total": 1, "remaining": 1},
                "add_on_quota": {"total": 2, "remaining": 2}
            }),
            1_000,
            300_000,
        )
        .unwrap_err();
        assert!(aliases.message.contains("conflicting"));

        let negative = parse_qoder_quota_update(
            &account,
            &json!({"userQuota": {"total": -1, "remaining": 0}}),
            1_000,
            300_000,
        )
        .unwrap_err();
        assert!(negative.message.contains("non-negative"));
    }

    #[tokio::test]
    async fn gemini_cli_project_load_uses_official_identity_and_merges_tier() {
        let (base_url, observations, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            json!({
                "cloudaicompanionProject": {"id": "discovered-project"},
                "paidTier": {"id": "PAID_ID", "displayName": "Paid"},
                "currentTier": {"name": "STANDARD", "displayName": "Standard"}
            }),
            axum::http::StatusCode::OK,
            json!({"buckets": []}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::GeminiCli,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            Some(json!({
                "email": "owner@example.com",
                "opaque": {"keep": true}
            })),
        );

        let update =
            load_gemini_v1internal_project(&reqwest::Client::new(), &account, 1_000, 5_000)
                .await
                .unwrap();
        assert_eq!(update.subscription_level.as_deref(), Some("PAID_ID"));
        assert_eq!(update.capability_observations.len(), 1);
        assert_eq!(
            update.capability_observations[0].dimension,
            crate::domain::accounts::capability_evidence::PROJECT_PROVISIONING_DIMENSION
        );
        assert_eq!(
            update.capability_observations[0].state,
            crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Supported
        );
        let profile = update.profile.unwrap();
        assert_eq!(profile["projectId"], "discovered-project");
        assert_eq!(profile["tier"], "PAID_ID");
        assert_eq!(profile["subscriptionTier"], "PAID_ID");
        assert_eq!(profile["paidTier"]["displayName"], "Paid");
        assert_eq!(profile["selectedTier"]["displayName"], "Paid");
        assert_eq!(profile["selectedTierSource"], "paidTier");
        assert_eq!(profile["email"], "owner@example.com");
        assert_eq!(profile["opaque"], json!({"keep": true}));
        assert!(profile.get("postExchangeEnrichment").is_none());

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(observation.operation, "loadCodeAssist");
        assert_eq!(
            observation.authorization,
            "Bearer gemini-code-assist-access"
        );
        assert_eq!(
            observation.user_agent,
            crate::provider_identity::gemini_cli_user_agent()
        );
        assert_eq!(
            observation.x_goog_api_client.as_deref(),
            Some(crate::provider_identity::GEMINI_CLI_X_GOOG_API_CLIENT)
        );
        assert!(observation.client_metadata.is_none());
        assert_eq!(
            observation.body,
            json!({"metadata": {"ideType": "GEMINI_CLI", "pluginType": "GEMINI"}})
        );
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn gemini_quota_records_current_project_and_model_entitlement_evidence() {
        let (base_url, observations, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            json!({
                "cloudaicompanionProject": {"id": "entitled-project"},
                "currentTier": {"name": "STANDARD"}
            }),
            axum::http::StatusCode::OK,
            json!({
                "buckets": [{
                    "modelId": "gemini-2.5-pro",
                    "remainingFraction": 0.75,
                    "resetTime": "2026-08-13T00:00:00Z"
                }]
            }),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::GeminiCli,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            None,
        );

        let update = refresh_gemini_quota(
            &reqwest::Client::new(),
            &account,
            1_000,
            30_000,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(update.capability_observations.len(), 2);
        assert!(update.capability_observations.iter().any(|observation| {
            observation.dimension
                == crate::domain::accounts::capability_evidence::PROJECT_PROVISIONING_DIMENSION
                && observation.state
                    == crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Supported
        }));
        assert!(update.capability_observations.iter().any(|observation| {
            observation.dimension
                == crate::domain::accounts::capability_evidence::MODEL_ENTITLEMENT_DIMENSION
                && observation.state
                    == crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Supported
                && observation.expires_at_ms == 61_000
        }));
        assert_eq!(update.quota.as_ref().unwrap().tiers.len(), 1);
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].operation, "loadCodeAssist");
        assert_eq!(observations[1].operation, "retrieveUserQuota");
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn gemini_empty_model_quota_records_unknown_not_unsupported_entitlement() {
        let (base_url, _, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            json!({
                "cloudaicompanionProject": "empty-quota-project",
                "currentTier": "FREE"
            }),
            axum::http::StatusCode::OK,
            json!({"buckets": []}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::GeminiCli,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            None,
        );

        let update = refresh_gemini_quota(
            &reqwest::Client::new(),
            &account,
            1_000,
            30_000,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        let entitlement = update
            .capability_observations
            .iter()
            .find(|observation| {
                observation.dimension
                    == crate::domain::accounts::capability_evidence::MODEL_ENTITLEMENT_DIMENSION
            })
            .unwrap();
        assert_eq!(
            entitlement.state,
            crate::domain::accounts::capability_evidence::AccountCapabilityObservationState::Unknown
        );
        assert_eq!(
            entitlement.reason.as_deref(),
            Some("quota_has_no_model_buckets")
        );
        server.abort();
    }

    #[tokio::test]
    async fn antigravity_and_agy_project_load_use_antigravity_identity() {
        let (base_url, observations, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            json!({
                "cloudaicompanionProject": "agy-project",
                "currentTier": {"name": "PRO"}
            }),
            axum::http::StatusCode::OK,
            json!({"buckets": []}),
        )
        .await;
        for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
            let account = gemini_code_assist_account(
                provider_type,
                json!({
                    "testGeminiLoadCodeAssistUrl": format!(
                        "{base_url}/v1internal:loadCodeAssist"
                    )
                }),
                Some(json!({"displayName": provider_type.as_str()})),
            );
            let update =
                load_gemini_v1internal_project(&reqwest::Client::new(), &account, 1_000, 5_000)
                    .await
                    .unwrap();
            assert_eq!(update.subscription_level.as_deref(), Some("PRO"));
            let profile = update.profile.unwrap();
            assert_eq!(profile["projectId"], "agy-project");
            assert_eq!(profile["postExchangeEnrichment"], "project_loaded");
            assert_eq!(profile["displayName"], provider_type.as_str());
        }

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        for observation in observations.iter() {
            assert_eq!(
                observation.user_agent,
                crate::provider_identity::antigravity_user_agent()
            );
            assert!(observation.x_goog_api_client.is_none());
            assert_eq!(
                observation.client_metadata.as_deref(),
                Some(
                    crate::provider_identity::antigravity_client_metadata()
                        .to_string()
                        .as_str()
                )
            );
            assert_eq!(
                observation.body,
                json!({"metadata": crate::provider_identity::antigravity_client_metadata()})
            );
        }
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn antigravity_quota_emits_project_tier_families_capacity_and_read_only_privacy() {
        let (base_url, observations, server) = serve_antigravity_code_assist(
            json!({
                "buckets": [
                    {"modelId": "gemini-2.5-pro", "remainingFraction": 0.75},
                    {"modelId": "claude-sonnet-4-5", "remainingFraction": 0.25}
                ]
            }),
            axum::http::StatusCode::OK,
            json!({"userSettings": {}}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::AntigravityOAuth,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            None,
        );

        let update = refresh_antigravity_quota(
            &reqwest::Client::new(),
            &account,
            1_000,
            30_000,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let state_for = |dimension: &str| {
            update
                .capability_observations
                .iter()
                .find(|observation| {
                    observation.capability
                        == crate::domain::accounts::capability_evidence::ANTIGRAVITY_CODE_PLAN_CAPABILITY
                        && observation.dimension == dimension
                })
                .map(|observation| observation.state)
        };
        for dimension in [
            PROJECT_BOOTSTRAP_DIMENSION,
            TIER_ENTITLEMENT_DIMENSION,
            GEMINI_QUOTA_FAMILY_DIMENSION,
            CLAUDE_QUOTA_FAMILY_DIMENSION,
            MODEL_CAPACITY_DIMENSION,
            PRIVACY_DIMENSION,
        ] {
            assert_eq!(
                state_for(dimension),
                Some(AccountCapabilityObservationState::Supported),
                "{dimension} must be supported"
            );
        }
        let evidence_json = serde_json::to_string(&update.capability_observations).unwrap();
        assert!(!evidence_json.contains("antigravity-project"));

        let observations = observations.lock().unwrap();
        assert_eq!(
            observations
                .iter()
                .map(|observation| observation.operation)
                .collect::<Vec<_>>(),
            ["loadCodeAssist", "retrieveUserQuota", "fetchUserInfo"]
        );
        assert!(observations
            .iter()
            .all(|observation| observation.operation != "setUserSettings"));
        assert_eq!(
            observations[2].body,
            json!({"project": "antigravity-project"})
        );
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn antigravity_missing_family_and_non_model_capacity_remain_unknown() {
        let (base_url, _, server) = serve_antigravity_code_assist(
            json!({
                "buckets": [
                    {"modelId": "gemini-2.5-pro", "remainingFraction": 0.5},
                    {"remainingFraction": 0.0}
                ]
            }),
            axum::http::StatusCode::OK,
            json!({"userSettings": {}}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::AgyOAuth,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            None,
        );
        let update = refresh_antigravity_quota(
            &reqwest::Client::new(),
            &account,
            1_000,
            30_000,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let observation = |dimension: &str| {
            update
                .capability_observations
                .iter()
                .find(|observation| {
                    observation.capability
                        == crate::domain::accounts::capability_evidence::ANTIGRAVITY_CODE_PLAN_CAPABILITY
                        && observation.dimension == dimension
                })
                .unwrap()
        };
        assert_eq!(
            observation(CLAUDE_QUOTA_FAMILY_DIMENSION).state,
            AccountCapabilityObservationState::Unknown
        );
        assert_eq!(
            observation(CLAUDE_QUOTA_FAMILY_DIMENSION).reason.as_deref(),
            Some("quota_has_no_explicit_family_bucket")
        );
        assert_eq!(
            observation(MODEL_CAPACITY_DIMENSION).state,
            AccountCapabilityObservationState::Supported
        );
        server.abort();
    }

    #[tokio::test]
    async fn antigravity_privacy_negative_and_probe_failure_are_observations_not_mutations() {
        for (status, body, expected_state, expected_reason) in [
            (
                axum::http::StatusCode::OK,
                json!({"userSettings": {"telemetryEnabled": false}}),
                AccountCapabilityObservationState::Unsupported,
                "telemetry_setting_present",
            ),
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                json!({"error": {"message": "slow down"}}),
                AccountCapabilityObservationState::Unknown,
                "privacy_probe_rate_limited",
            ),
        ] {
            let (base_url, observations, server) = serve_antigravity_code_assist(
                json!({"buckets": [{
                    "modelId": "claude-sonnet-4-5",
                    "remainingFraction": 0.5
                }]}),
                status,
                body,
            )
            .await;
            let account = gemini_code_assist_account(
                ProviderType::AntigravityOAuth,
                json!({"testGeminiCodeAssistBaseUrl": base_url}),
                None,
            );
            let update = refresh_antigravity_quota(
                &reqwest::Client::new(),
                &account,
                1_000,
                30_000,
                Duration::from_secs(5),
            )
            .await
            .expect("privacy probing must not block quota refresh");
            let privacy = update
                .capability_observations
                .iter()
                .find(|observation| observation.dimension == PRIVACY_DIMENSION)
                .unwrap();
            assert_eq!(privacy.state, expected_state);
            assert_eq!(privacy.reason.as_deref(), Some(expected_reason));
            let observations = observations.lock().unwrap();
            assert_eq!(observations.len(), 3);
            assert!(observations
                .iter()
                .all(|observation| observation.operation != "setUserSettings"));
            drop(observations);
            server.abort();
        }
    }

    #[tokio::test]
    async fn project_load_without_project_keeps_enrichment_deferred() {
        let (base_url, _, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            json!({"currentTier": "free-tier"}),
            axum::http::StatusCode::OK,
            json!({"buckets": []}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::AntigravityOAuth,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            Some(json!({
                "postExchangeEnrichment": "project_and_tier_deferred_to_quota_refresh",
                "opaque": true
            })),
        );

        let failure =
            load_gemini_v1internal_project(&reqwest::Client::new(), &account, 1_000, 5_000)
                .await
                .unwrap_err();
        assert_eq!(failure.status_code, 502);
        assert!(failure.message.contains("no usable projectId"));
        assert!(failure.retryable);
        assert_eq!(
            failure.next_refresh_at,
            Some(1_000 + QUOTA_FAILURE_COOLDOWN_MS)
        );
        let partial = failure.partial_update.expect("tier partial update");
        assert_eq!(partial.subscription_level.as_deref(), Some("free-tier"));
        let profile = partial.profile.unwrap();
        assert!(profile.get("projectId").is_none());
        assert_eq!(
            profile["postExchangeEnrichment"],
            "project_and_tier_deferred_to_quota_refresh"
        );
        assert_eq!(profile["tier"], "free-tier");
        assert_eq!(profile["opaque"], true);
        server.abort();
    }

    #[tokio::test]
    async fn gemini_quota_failure_carries_completed_project_load_update() {
        let (base_url, observations, server) = serve_gemini_code_assist(
            axum::http::StatusCode::OK,
            json!({
                "cloudaicompanionProject": {"projectId": "partial-project"},
                "currentTier": {"name": "PRO"}
            }),
            axum::http::StatusCode::BAD_GATEWAY,
            json!({"error": {"message": "quota unavailable"}}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::GeminiCli,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            None,
        );

        let failure = refresh_gemini_quota(
            &reqwest::Client::new(),
            &account,
            1_000,
            30_000,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert_eq!(failure.upstream_status, Some(502));
        let partial = failure.partial_update.expect("project load update");
        assert_eq!(partial.subscription_level.as_deref(), Some("PRO"));
        assert_eq!(
            partial
                .profile
                .as_ref()
                .and_then(|profile| profile["projectId"].as_str()),
            Some("partial-project")
        );
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[1].operation, "retrieveUserQuota");
        assert_eq!(observations[1].body["project"], "partial-project");
        drop(observations);
        server.abort();
    }

    #[tokio::test]
    async fn project_load_exposes_upstream_unauthorized_status() {
        let (base_url, _, server) = serve_gemini_code_assist(
            axum::http::StatusCode::UNAUTHORIZED,
            json!({"error": {"message": "expired"}}),
            axum::http::StatusCode::OK,
            json!({"buckets": []}),
        )
        .await;
        let account = gemini_code_assist_account(
            ProviderType::GeminiCli,
            json!({"testGeminiCodeAssistBaseUrl": base_url}),
            None,
        );

        let failure =
            load_gemini_v1internal_project(&reqwest::Client::new(), &account, 1_000, 5_000)
                .await
                .unwrap_err();
        assert_eq!(failure.status_code, 400);
        assert_eq!(failure.upstream_status, Some(401));
        assert!(failure.partial_update.is_none());
        server.abort();
    }

    #[test]
    fn ollama_me_parse_is_display_only_and_keeps_subscription_window() {
        let body = json!({
            "Email": "xiechengqi01@gmail.com",
            "Name": "xiechengqi01",
            "Plan": "pro",
            "SubscriptionPeriodEnd": {
                "Time": "2026-07-25T04:49:24Z",
                "Valid": true
            }
        });

        let update = parse_ollama_me_update(
            &body,
            1_000,
            crate::domain::settings::ui_settings::default_oauth_quota_refresh_interval_ms(),
        );
        assert_eq!(update.email.as_deref(), Some("xiechengqi01@gmail.com"));
        assert_eq!(update.subscription_level.as_deref(), Some("ollama pro"));
        assert_eq!(update.quota_percent, None);
        let quota = update.quota.expect("quota");
        assert!(quota.success);
        assert!(quota.tiers.is_empty());
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.pointer("/subscriptionPeriodEnd"))
                .and_then(Value::as_str),
            Some("2026-07-25T04:49:24+00:00")
        );
    }

    #[test]
    fn codex_usage_maps_one_percent_window_without_scaling_bug() {
        let tiers = codex_tiers_from_rate_limit(Some(CodexRateLimit {
            allowed: None,
            limit_reached: None,
            primary_window: Some(CodexRateLimitWindow {
                used_percent: Some(1.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(9_000),
                reset_at: Some(1),
            }),
            secondary_window: None,
        }));

        assert_eq!(tiers[0].name, "five_hour");
        assert_eq!(tiers[0].utilization, Some(0.01));
    }

    #[test]
    fn codex_usage_parse_keeps_percent_as_account_percent() {
        let tiers = codex_tiers_from_rate_limit(Some(CodexRateLimit {
            allowed: None,
            limit_reached: None,
            primary_window: Some(CodexRateLimitWindow {
                used_percent: Some(42.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(9_000),
                reset_at: Some(1),
            }),
            secondary_window: None,
        }));
        assert_eq!(tiers[0].name, "five_hour");
        assert_eq!(tiers[0].utilization, Some(0.42));
        assert_eq!(tiers[0].resets_at, Some(1_000));
        let quota = AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Pro 20x".to_string()),
            tiers,
            extra_usage: None,
        };
        let update = update_from_quota(
            quota,
            Some("ChatGPT Pro 20x".to_string()),
            None,
            10_000,
            crate::domain::settings::ui_settings::default_oauth_quota_refresh_interval_ms(),
        );
        assert_eq!(update.quota_percent, Some(42.0));
        assert_eq!(
            update.quota_next_refresh_at,
            Some(
                10_000
                    + crate::domain::settings::ui_settings::default_oauth_quota_refresh_interval_ms(
                    )
            )
        );
    }

    #[test]
    fn codex_usage_keeps_seven_day_exhaustion_at_full_utilization() {
        let tiers = codex_tiers_from_rate_limit(Some(CodexRateLimit {
            allowed: None,
            limit_reached: None,
            primary_window: Some(CodexRateLimitWindow {
                used_percent: Some(4.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(8_657),
                reset_at: Some(1_700_000_000),
            }),
            secondary_window: Some(CodexRateLimitWindow {
                used_percent: Some(100.0),
                limit_window_seconds: Some(604_800),
                reset_after_seconds: Some(518_400),
                reset_at: Some(1_700_500_000),
            }),
        }));

        assert_eq!(
            tiers
                .iter()
                .find(|tier| tier.name == "seven_day")
                .and_then(|tier| tier.utilization),
            Some(1.0)
        );
    }

    #[test]
    fn codex_usage_corrects_remaining_percent_when_window_was_just_reset() {
        let tiers = codex_tiers_from_rate_limit(Some(CodexRateLimit {
            allowed: None,
            limit_reached: None,
            primary_window: Some(CodexRateLimitWindow {
                used_percent: Some(100.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(17_940),
                reset_at: Some(1_700_000_000),
            }),
            secondary_window: Some(CodexRateLimitWindow {
                used_percent: Some(36.0),
                limit_window_seconds: Some(604_800),
                reset_after_seconds: Some(518_400),
                reset_at: Some(1_700_500_000),
            }),
        }));

        assert_eq!(
            tiers
                .iter()
                .find(|tier| tier.name == "five_hour")
                .and_then(|tier| tier.utilization),
            Some(0.0)
        );
        assert_eq!(
            tiers
                .iter()
                .find(|tier| tier.name == "seven_day")
                .and_then(|tier| tier.utilization),
            Some(0.36)
        );
    }

    #[test]
    fn codex_usage_swaps_reversed_weekly_primary_window() {
        let tiers = codex_tiers_from_rate_limit(Some(CodexRateLimit {
            allowed: None,
            limit_reached: None,
            primary_window: Some(CodexRateLimitWindow {
                used_percent: Some(36.0),
                limit_window_seconds: Some(604_800),
                reset_after_seconds: Some(518_400),
                reset_at: Some(1_700_500_000),
            }),
            secondary_window: Some(CodexRateLimitWindow {
                used_percent: Some(4.0),
                limit_window_seconds: Some(18_000),
                reset_after_seconds: Some(8_657),
                reset_at: Some(1_700_000_000),
            }),
        }));

        assert_eq!(tiers[0].name, "five_hour");
        assert_eq!(tiers[0].utilization, Some(0.04));
        assert_eq!(tiers[1].name, "seven_day");
        assert_eq!(tiers[1].utilization, Some(0.36));
    }

    #[test]
    fn codex_review_quota_uses_only_explicit_review_fields() {
        let direct = json!({
            "code_review_rate_limit": {
                "primary_window": {
                    "used_percent": 25.0,
                    "limit_window_seconds": 18_000,
                    "reset_at": 1
                },
                "secondary_window": {
                    "used_percent": 75.0,
                    "limit_window_seconds": 604_800,
                    "reset_at": 2
                }
            }
        });
        let tiers = codex_review_tiers_from_usage(&direct);
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].name, "review_session");
        assert_eq!(tiers[0].utilization, Some(0.25));
        assert_eq!(tiers[1].name, "review_weekly");
        assert_eq!(tiers[1].utilization, Some(0.75));

        let by_id = json!({
            "rate_limits_by_limit_id": {
                "codex_review": {
                    "rate_limit": {
                        "primary_window": {"used_percent": 40.0}
                    }
                }
            }
        });
        assert_eq!(
            codex_review_tiers_from_usage(&by_id)[0].utilization,
            Some(0.4)
        );
    }

    #[test]
    fn codex_review_quota_rejects_fuzzy_additional_limit_ids() {
        let fuzzy = json!({
            "additional_rate_limits": [{
                "id": "pull_request_review_preview",
                "primary_window": {"used_percent": 90.0}
            }]
        });
        assert!(codex_review_tiers_from_usage(&fuzzy).is_empty());

        let exact = json!({
            "additional_rate_limits": [{
                "metered_feature": "review",
                "primary_window": {"used_percent": 10.0}
            }]
        });
        assert_eq!(
            codex_review_tiers_from_usage(&exact)[0].name,
            "review_session"
        );

        let exact_in_later_field = json!({
            "additional_rate_limits": [{
                "limit_name": "unrelated",
                "id": "code_review",
                "primary_window": {"used_percent": 15.0}
            }]
        });
        assert_eq!(
            codex_review_tiers_from_usage(&exact_in_later_field)[0].utilization,
            Some(0.15)
        );

        let malformed_direct_with_valid_fallback = json!({
            "code_review_rate_limit": "malformed",
            "rate_limits_by_limit_id": {
                "review": {
                    "primary_window": {"used_percent": 20.0}
                }
            }
        });
        assert_eq!(
            codex_review_tiers_from_usage(&malformed_direct_with_valid_fallback)[0].utilization,
            Some(0.2)
        );

        let empty_direct_with_valid_fallback = json!({
            "code_review_rate_limit": {"primary_window": {}},
            "rate_limits_by_limit_id": {
                "review": {
                    "primary_window": {"used_percent": 22.0}
                }
            }
        });
        assert_eq!(
            codex_review_tiers_from_usage(&empty_direct_with_valid_fallback)[0].utilization,
            Some(0.22)
        );

        let malformed_first_id_with_valid_later_id = json!({
            "rate_limits_by_limit_id": {
                "code_review": "malformed",
                "codex_review": {
                    "primary": {
                        "percent_used": 30.0,
                        "resets_at": 3
                    }
                }
            }
        });
        let tiers = codex_review_tiers_from_usage(&malformed_first_id_with_valid_later_id);
        assert_eq!(tiers[0].utilization, Some(0.3));
        assert_eq!(tiers[0].resets_at, Some(3_000));

        let malformed_first_additional_with_valid_later_entry = json!({
            "additional_rate_limits": [
                {"id": "review", "rate_limit": "malformed"},
                {
                    "id": "code_review",
                    "rate_limit": {
                        "secondary": {
                            "percent_used": 35.0,
                            "limit_window_seconds": 604_800,
                            "resetAt": 4
                        }
                    }
                }
            ]
        });
        let tiers =
            codex_review_tiers_from_usage(&malformed_first_additional_with_valid_later_entry);
        assert_eq!(tiers[0].name, "review_weekly");
        assert_eq!(tiers[0].utilization, Some(0.35));
        assert_eq!(tiers[0].resets_at, Some(4_000));

        let millisecond_reset = json!({
            "review_rate_limit": {
                "primary_window": {
                    "used_percent": 45.0,
                    "resetAt": 1_800_000_000_000_i64
                }
            }
        });
        assert_eq!(
            codex_review_tiers_from_usage(&millisecond_reset)[0].resets_at,
            Some(1_800_000_000_000)
        );
    }

    #[test]
    fn account_quota_percent_ignores_review_tiers() {
        let tiers = vec![
            AccountQuotaTier {
                name: "five_hour".to_string(),
                label: None,
                utilization: Some(0.2),
                used: None,
                limit: None,
                unit: Some("percent".to_string()),
                resets_at: None,
                ..Default::default()
            },
            AccountQuotaTier {
                name: "review_weekly".to_string(),
                label: None,
                utilization: Some(0.9),
                used: None,
                limit: None,
                unit: Some("percent".to_string()),
                resets_at: None,
                ..Default::default()
            },
        ];
        assert_eq!(quota_percent_from_tiers(&tiers), Some(20.0));
    }

    #[test]
    fn chatgpt_accounts_check_skips_expired_and_inactive_fallbacks() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let body = json!({
            "accounts": {
                "expired-business": {
                    "account": {
                        "id": "expired-business",
                        "plan_type": "self_serve_business_usage_based",
                        "is_default": true
                    },
                    "entitlement": {
                        "expires_at": "2026-03-26T14:55:16Z"
                    }
                },
                "suspended-pro": {
                    "account": {
                        "id": "suspended-pro",
                        "plan_type": "pro",
                        "status": "suspended"
                    },
                    "entitlement": {
                        "expires_at": "2026-08-01T00:00:00Z"
                    }
                },
                "active-plus": {
                    "account": {
                        "id": "active-plus",
                        "plan_type": "plus",
                        "is_default": false
                    },
                    "entitlement": {
                        "expires_at": "2026-08-20T00:00:00Z"
                    }
                }
            }
        });

        let lookup = parse_chatgpt_accounts_check_lookup(&body, None, now_ms).unwrap();

        assert_eq!(lookup.plan_type.as_deref(), Some("plus"));
        assert_eq!(
            lookup.expires_at.as_deref(),
            Some("2026-08-20T00:00:00+00:00")
        );
    }

    #[test]
    fn chatgpt_accounts_check_does_not_cross_fallback_for_trusted_workspace() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let body = json!({
            "accounts": {
                "expired-business": {
                    "account": {
                        "id": "expired-business",
                        "plan_type": "business"
                    },
                    "entitlement": {
                        "expires_at": "2026-03-26T14:55:16Z"
                    }
                },
                "active-plus": {
                    "account": {
                        "id": "active-plus",
                        "plan_type": "plus"
                    },
                    "entitlement": {
                        "expires_at": "2026-08-20T00:00:00Z"
                    }
                }
            }
        });

        assert!(
            parse_chatgpt_accounts_check_lookup(&body, Some("expired-business"), now_ms).is_none()
        );
        assert!(parse_chatgpt_accounts_check_lookup(&body, Some("missing"), now_ms).is_none());
    }

    #[test]
    fn codex_subscription_reconciliation_keeps_plus_over_expired_business() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let account_lookup = chatgpt_lookup_from_account(&json!({
            "account": {"plan_type": "self_serve_business_usage_based"},
            "entitlement": {"expires_at": "2026-03-26T14:55:16Z"}
        }));

        let resolution = reconcile_chatgpt_subscription(
            Some("plus"),
            Some(true),
            false,
            account_lookup,
            None,
            now_ms,
        );
        let subscription = resolution.subscription.unwrap();

        assert_eq!(subscription.plan_type.as_deref(), Some("plus"));
        assert_eq!(subscription.plan_label.as_deref(), Some("ChatGPT Plus"));
        assert!(subscription.expires_at.is_none());
        assert_eq!(
            resolution.discarded_reasons,
            vec!["accounts_check_plan_mismatch"]
        );
    }

    #[test]
    fn codex_subscription_reconciliation_matches_production_plus_evidence() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T07:28:58Z").unwrap();
        let accounts_check = json!({
            "accounts": {
                "expired-business": {
                    "account": {
                        "id": "expired-business",
                        "plan_type": "self_serve_business_usage_based",
                        "is_default": true
                    },
                    "entitlement": {
                        "expires_at": "2026-03-26T14:55:16Z"
                    }
                }
            }
        });
        let account_lookup = parse_chatgpt_accounts_check_lookup(&accounts_check, None, now_ms);
        assert!(account_lookup.is_none());

        let resolution = reconcile_chatgpt_subscription(
            Some("plus"),
            Some(true),
            false,
            account_lookup,
            None,
            now_ms,
        );
        let subscription = resolution.subscription.unwrap();

        assert_eq!(subscription.plan_type.as_deref(), Some("plus"));
        assert_eq!(subscription.plan_label.as_deref(), Some("ChatGPT Plus"));
        assert!(subscription.expires_at.is_none());
        assert!(subscription.expires_source.is_none());
        assert!(subscription.expires_kind.is_none());
    }

    #[test]
    fn codex_subscription_reconciliation_requires_trusted_workspace_for_expiry() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let account_lookup = chatgpt_lookup_from_account(&json!({
            "account": {"plan_type": "plus"},
            "entitlement": {"expires_at": "2026-08-20T00:00:00Z"}
        }));

        let resolution = reconcile_chatgpt_subscription(
            Some("plus"),
            Some(true),
            false,
            account_lookup,
            None,
            now_ms,
        );
        let subscription = resolution.subscription.unwrap();

        assert_eq!(subscription.plan_label.as_deref(), Some("ChatGPT Plus"));
        assert!(subscription.expires_at.is_none());
        assert!(subscription.expires_source.is_none());
        assert_eq!(
            resolution.discarded_reasons,
            vec!["accounts_check_untrusted_workspace_expiry"]
        );
    }

    #[test]
    fn codex_subscription_reconciliation_accepts_matching_trusted_expiry() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let account_lookup = chatgpt_lookup_from_account(&json!({
            "account": {"plan_type": "plus"},
            "entitlement": {"expires_at": "2026-08-20T00:00:00Z"}
        }));

        let resolution = reconcile_chatgpt_subscription(
            Some("plus"),
            Some(true),
            true,
            account_lookup,
            None,
            now_ms,
        );
        let subscription = resolution.subscription.unwrap();

        assert_eq!(
            subscription.expires_at.as_deref(),
            Some("2026-08-20T00:00:00+00:00")
        );
        assert_eq!(
            subscription.expires_source.as_deref(),
            Some("accounts_check_entitlement")
        );
        assert!(resolution.discarded_reasons.is_empty());
    }

    #[test]
    fn codex_subscription_reconciliation_drops_past_expiry_when_usage_is_available() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let subscription_lookup = parse_chatgpt_subscription_lookup(&json!({
            "plan_type": "plus",
            "active_until": "2026-07-01T00:00:00Z"
        }));

        let resolution = reconcile_chatgpt_subscription(
            Some("plus"),
            Some(true),
            true,
            None,
            subscription_lookup,
            now_ms,
        );
        let subscription = resolution.subscription.unwrap();

        assert_eq!(subscription.plan_label.as_deref(), Some("ChatGPT Plus"));
        assert!(subscription.expires_at.is_none());
        assert_eq!(
            resolution.discarded_reasons,
            vec!["subscriptions_expired_while_usage_available"]
        );
    }

    #[test]
    fn chatgpt_subscription_sources_only_exist_with_expiry() {
        let lookup = parse_chatgpt_subscription_lookup(&json!({"plan_type": "plus"})).unwrap();

        assert!(lookup.expires_at.is_none());
        assert!(lookup.expires_source.is_none());
        assert!(lookup.expires_kind.is_none());
    }

    #[test]
    fn legacy_codex_workspace_candidate_requires_consistent_identity_evidence() {
        let mut account = imported_account(ProviderType::CodexOAuth, json!({}));
        account.id = "workspace-1".to_string();
        account.profile = Some(json!({
            "accountId": "workspace-1",
            "chatgpt_account_id": "workspace-1"
        }));
        assert_eq!(
            legacy_codex_workspace_candidate(&account).as_deref(),
            Some("workspace-1")
        );

        account.profile = Some(json!({
            "accountId": "workspace-1",
            "chatgpt_account_id": "workspace-2"
        }));
        assert!(legacy_codex_workspace_candidate(&account).is_none());

        account.id = "local-import-id".to_string();
        account.profile = Some(json!({"chatgpt_account_id": "workspace-1"}));
        assert_eq!(
            legacy_codex_workspace_candidate(&account).as_deref(),
            Some("workspace-1")
        );
    }

    #[test]
    fn accounts_check_discovers_one_active_workspace_matching_usage_plan() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let body = json!({
            "accounts": {
                "old-business": {
                    "account": {"id": "old-business", "plan_type": "business"},
                    "entitlement": {"expires_at": "2026-03-01T00:00:00Z"}
                },
                "current-plus": {
                    "account": {"id": "current-plus", "plan_type": "plus"}
                },
                "current-free": {
                    "account": {"id": "current-free", "plan_type": "free"}
                }
            }
        });
        let lookup = parse_chatgpt_accounts_check_lookup(&body, None, now_ms);
        let probe = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::Success,
            http_status: Some(200),
            lookup,
            workspace_candidates: parse_chatgpt_workspace_candidates(&body, now_ms),
        };

        let candidate = unique_chatgpt_workspace_matching_usage(&probe, Some("plus")).unwrap();
        assert_eq!(candidate.workspace_id, "current-plus");
        assert_eq!(candidate.lookup.plan_type.as_deref(), Some("plus"));
        assert!(unique_chatgpt_workspace_matching_usage(&probe, Some("pro")).is_none());
    }

    #[test]
    fn authenticated_discovery_requires_matching_usage_plan() {
        let matching = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::Success,
            http_status: Some(200),
            lookup: parse_chatgpt_subscription_lookup(&json!({"plan_type": "pro"})),
            workspace_candidates: Vec::new(),
        };
        assert!(chatgpt_probe_matches_usage(&matching, Some("pro")));
        assert!(!chatgpt_probe_matches_usage(&matching, Some("plus")));
        assert!(!chatgpt_probe_matches_usage(&matching, None));

        let failed = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::HttpError,
            http_status: Some(403),
            lookup: matching.lookup.clone(),
            workspace_candidates: Vec::new(),
        };
        assert!(!chatgpt_probe_matches_usage(&failed, Some("pro")));
    }

    #[test]
    fn codex_subscription_finalize_exposes_workspace_and_probe_states() {
        let account = imported_account(ProviderType::CodexOAuth, json!({}));
        let lookup = parse_chatgpt_subscription_lookup(&json!({"plan_type": "pro"}));
        let success = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::Success,
            http_status: Some(200),
            lookup: lookup.clone(),
            workspace_candidates: Vec::new(),
        };
        let skipped = ChatGptSubscriptionProbe::skipped_no_trusted_workspace();

        let (subscription, snapshot) = finalize_codex_subscription(
            &account,
            lookup.clone(),
            None,
            Some("pro"),
            &success,
            &skipped,
            rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap(),
        );
        assert_eq!(
            subscription.unwrap().expiry_availability.as_deref(),
            Some("workspace_unverified")
        );
        assert!(snapshot.is_none());

        let trusted = crate::domain::accounts::store::TrustedCodexWorkspace {
            id: "workspace-1".to_string(),
            source: "verified_id_token".to_string(),
        };
        let failed = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::HttpError,
            http_status: Some(404),
            lookup: None,
            workspace_candidates: Vec::new(),
        };
        let (subscription, _) = finalize_codex_subscription(
            &account,
            lookup,
            Some(&trusted),
            Some("pro"),
            &success,
            &failed,
            rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap(),
        );
        assert_eq!(
            subscription.unwrap().expiry_availability.as_deref(),
            Some("probe_unavailable")
        );
    }

    #[test]
    fn codex_subscription_finalize_caches_only_same_workspace_and_plan() {
        let now_ms = rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap();
        let mut account = imported_account(ProviderType::CodexOAuth, json!({}));
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscriptionExpirySnapshot": {
                    "workspaceId": "workspace-1",
                    "planFamily": "pro",
                    "expiresAt": "2026-08-20T00:00:00Z",
                    "source": "subscriptions_active_until",
                    "kind": "subscription",
                    "observedAt": 123,
                    "stale": false
                }
            })),
            ..Default::default()
        });
        let trusted = crate::domain::accounts::store::TrustedCodexWorkspace {
            id: "workspace-1".to_string(),
            source: "verified_id_token".to_string(),
        };
        let failed = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::NetworkError,
            http_status: None,
            lookup: None,
            workspace_candidates: Vec::new(),
        };
        let lookup = parse_chatgpt_subscription_lookup(&json!({"plan_type": "pro"}));

        let (subscription, snapshot) = finalize_codex_subscription(
            &account,
            lookup.clone(),
            Some(&trusted),
            Some("pro"),
            &failed,
            &failed,
            now_ms,
        );
        let subscription = subscription.unwrap();
        assert_eq!(
            subscription.expires_at.as_deref(),
            Some("2026-08-20T00:00:00+00:00")
        );
        assert!(subscription.expiry_stale);
        assert_eq!(snapshot.unwrap()["stale"], true);

        let other_workspace = crate::domain::accounts::store::TrustedCodexWorkspace {
            id: "workspace-2".to_string(),
            source: "user_selected".to_string(),
        };
        let (subscription, snapshot) = finalize_codex_subscription(
            &account,
            lookup.clone(),
            Some(&other_workspace),
            Some("pro"),
            &failed,
            &failed,
            now_ms,
        );
        assert!(subscription.unwrap().expires_at.is_none());
        assert!(snapshot.is_none());

        let (subscription, snapshot) = finalize_codex_subscription(
            &account,
            lookup,
            Some(&trusted),
            Some("plus"),
            &failed,
            &failed,
            now_ms,
        );
        assert!(subscription.unwrap().expires_at.is_none());
        assert!(snapshot.is_none());
    }

    #[test]
    fn codex_subscription_finalize_persists_fresh_active_until() {
        let account = imported_account(ProviderType::CodexOAuth, json!({}));
        let trusted = crate::domain::accounts::store::TrustedCodexWorkspace {
            id: "workspace-1".to_string(),
            source: "verified_id_token".to_string(),
        };
        let lookup = parse_chatgpt_subscription_lookup(&json!({
            "plan_type": "pro",
            "active_until": "2026-08-20T00:00:00Z"
        }));
        let success = ChatGptSubscriptionProbe {
            status: ChatGptProbeStatus::Success,
            http_status: Some(200),
            lookup: lookup.clone(),
            workspace_candidates: Vec::new(),
        };
        let (subscription, snapshot) = finalize_codex_subscription(
            &account,
            lookup,
            Some(&trusted),
            Some("pro"),
            &success,
            &success,
            rfc3339_to_unix_ms("2026-07-20T00:00:00Z").unwrap(),
        );

        let subscription = subscription.unwrap();
        assert_eq!(
            subscription.expiry_availability.as_deref(),
            Some("available")
        );
        assert!(!subscription.expiry_stale);
        let snapshot = snapshot.unwrap();
        assert_eq!(snapshot["workspaceId"], "workspace-1");
        assert_eq!(snapshot["planFamily"], "pro");
        assert_eq!(snapshot["stale"], false);
    }

    #[test]
    fn claude_usage_windows_parse_known_and_unknown_tiers() {
        let body = json!({
            "plan": "claude_pro",
            "five_hour": {
                "utilization": 25.0,
                "resets_at": "2026-07-02T00:00:00Z"
            },
            "seven_day_omelette": {
                "utilization": 0.5
            },
            "new_window": {
                "utilization": 75.0
            },
            "extra_usage": {
                "is_enabled": true
            }
        });
        let account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        let subscription =
            resolve_claude_quota_subscription(&account, &body, None, None, 1_000).unwrap();
        let quota = parse_claude_quota(&body, Some(&subscription), 1_000);

        assert_eq!(quota.credential_message.as_deref(), Some("Claude Pro"));
        assert_eq!(quota.tiers[0].name, "five_hour");
        assert_eq!(quota.tiers[0].utilization, Some(0.25));
        assert_eq!(quota.tiers[1].name, "seven_day_opus");
        assert_eq!(quota.tiers[1].utilization, Some(0.5));
        assert!(quota.tiers.iter().any(|tier| tier.name == "new_window"));
        assert_eq!(
            quota.extra_usage.unwrap()["subscription"]["planType"],
            "claude_pro"
        );
    }

    #[test]
    fn claude_subscription_resolves_live_max_multipliers() {
        for (rate_limit_tier, expected_type, expected_label) in [
            ("default_claude_max_5x", "claude_max_5x", "Claude Max 5x"),
            ("default_claude_max_20x", "claude_max_20x", "Claude Max 20x"),
        ] {
            let account = imported_account(ProviderType::ClaudeOAuth, json!({}));
            let usage = json!({"plan": "claude_max"});
            let bootstrap = json!({
                "organizationType": "claude_max",
                "organizationRateLimitTier": rate_limit_tier,
            });
            let subscription =
                resolve_claude_quota_subscription(&account, &usage, None, Some(&bootstrap), 1_000)
                    .unwrap();
            let quota = parse_claude_quota(&usage, Some(&subscription), 1_000);
            let extra = quota.extra_usage.unwrap();

            assert_eq!(quota.credential_message.as_deref(), Some(expected_label));
            assert_eq!(extra["subscription"]["planType"], expected_type);
            assert_eq!(extra["subscription"]["planLabel"], expected_label);
            assert_eq!(extra["subscription"]["planObservedAt"], 1_000);
            assert_eq!(
                extra["subscriptionEvidence"]["source"],
                "bootstrap_rate_limit_tier"
            );
            assert_eq!(extra["subscriptionEvidence"]["stale"], false);
            assert_eq!(extra["warningCodes"], json!([]));
        }
    }

    #[test]
    fn claude_fable_pool_is_canonical_and_only_exposed_for_eligible_max_plans() {
        let usage = json!({
            "plan": "claude_max",
            "seven_day": {"utilization": 72.0},
            "seven_day_overage_included": {
                "utilization": 100.0,
                "resets_at": "2026-09-07T00:00:00Z"
            }
        });
        let account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        let bootstrap = json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_20x",
        });
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, None, Some(&bootstrap), 1_000)
                .unwrap();
        let quota = parse_claude_quota(&usage, Some(&subscription), 1_000);
        let fable = quota
            .tiers
            .iter()
            .find(|tier| tier.name == CLAUDE_FABLE_QUOTA_TIER)
            .unwrap();
        assert_eq!(fable.utilization, Some(1.0));
        assert_eq!(fable.scope.as_deref(), Some("model_family"));
        assert_eq!(
            fable.capacity_pool.as_deref(),
            Some(CLAUDE_FABLE_CAPACITY_POOL)
        );
        assert_eq!(
            fable.model_family.as_deref(),
            Some(CLAUDE_FABLE_MODEL_FAMILY)
        );
        assert_eq!(
            fable.relative_weekly_capacity,
            Some(CLAUDE_FABLE_RELATIVE_WEEKLY_CAPACITY)
        );

        let pro_usage = json!({
            "plan": "claude_pro",
            "seven_day_overage_included": {"utilization": 100.0}
        });
        let pro_subscription =
            resolve_claude_quota_subscription(&account, &pro_usage, None, None, 1_000).unwrap();
        let pro_quota = parse_claude_quota(&pro_usage, Some(&pro_subscription), 1_000);
        assert!(pro_quota
            .tiers
            .iter()
            .all(|tier| tier.name != CLAUDE_FABLE_QUOTA_TIER));
    }

    #[test]
    fn claude_subscription_keeps_generic_max_without_multiplier_evidence() {
        let account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        let usage = json!({"five_hour": {"utilization": 10.0}});
        let profile_lookup = ClaudeProfileLookup {
            organization_type: Some("claude_max".to_string()),
            rate_limit_tier: None,
            profile_overlay: None,
        };
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, Some(&profile_lookup), None, 1_000)
                .unwrap();

        assert_eq!(subscription.resolution.plan.plan_type(), "claude_max");
        assert_eq!(subscription.resolution.plan.label(), "Claude Max");
        assert!(!subscription.resolution.stale);
        assert!(!subscription.resolution.conflict);
    }

    #[test]
    fn claude_subscription_reports_incompatible_live_evidence() {
        let account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        let usage = json!({"tier": "claude_pro"});
        let bootstrap = json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_20x",
        });
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, None, Some(&bootstrap), 1_000)
                .unwrap();
        let quota = parse_claude_quota(&usage, Some(&subscription), 1_000);
        let extra = quota.extra_usage.unwrap();

        assert_eq!(quota.credential_message.as_deref(), Some("Claude Pro"));
        assert_eq!(extra["subscriptionEvidence"]["conflict"], true);
        assert_eq!(extra["warningCodes"], json!(["claude_plan_conflict"]));
    }

    #[test]
    fn claude_subscription_backfills_compatible_cached_multiplier_as_stale() {
        let mut account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        account.profile = Some(json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_5x",
            "organizationTypeObservedAt": 100,
            "organizationRateLimitTierObservedAt": 100,
        }));
        account.subscription_level = Some("Claude Max 5x".to_string());
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {
                    "planType": "claude_max_5x",
                    "planStale": false,
                    "planObservedAt": 100,
                }
            })),
            ..Default::default()
        });
        let usage = json!({"plan": "claude_max"});
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, None, None, 1_000).unwrap();
        let quota = parse_claude_quota(&usage, Some(&subscription), 1_000);
        let extra = quota.extra_usage.unwrap();

        assert_eq!(quota.credential_message.as_deref(), Some("Claude Max 5x"));
        assert_eq!(extra["subscription"]["planType"], "claude_max_5x");
        assert_eq!(extra["subscription"]["planStale"], true);
        assert_eq!(extra["subscription"]["planObservedAt"], 100);
        assert_eq!(
            extra["subscriptionEvidence"]["source"],
            "cached_subscription_level"
        );
    }

    #[test]
    fn claude_subscription_does_not_renew_stale_multiplier_observation() {
        let observed_at = 1_000;
        let now_ms = observed_at + 60_000;
        let mut account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        account.subscription_level = Some("Claude Max 20x".to_string());
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {
                    "planType": "claude_max_20x",
                    "planStale": true,
                    "planObservedAt": observed_at,
                }
            })),
            ..Default::default()
        });

        let usage = json!({"plan": "claude_max"});
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, None, None, now_ms).unwrap();
        let quota = parse_claude_quota(&usage, Some(&subscription), now_ms);
        let extra = quota.extra_usage.unwrap();

        assert_eq!(extra["subscription"]["planType"], "claude_max_20x");
        assert_eq!(extra["subscription"]["planStale"], true);
        assert_eq!(extra["subscription"]["planObservedAt"], observed_at);
        assert_eq!(extra["queriedAt"], now_ms);
    }

    #[test]
    fn claude_subscription_rejects_observation_from_a_different_cached_plan() {
        let mut account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        account.subscription_level = Some("Claude Max 20x".to_string());
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {
                    "planType": "claude_max_5x",
                    "planStale": false,
                    "planObservedAt": 900,
                }
            })),
            ..Default::default()
        });

        let usage = json!({"plan": "claude_max"});
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, None, None, 1_000).unwrap();

        assert_eq!(subscription.resolution.plan.plan_type(), "claude_max");
        assert_eq!(
            subscription.resolution.source,
            ClaudeSubscriptionSource::UsagePlan
        );
        assert!(!subscription.resolution.stale);
        assert_eq!(subscription.observed_at_ms, 1_000);
    }

    #[test]
    fn claude_subscription_drops_expired_cached_multiplier_to_live_generic_max() {
        let observed_at = 1_000;
        let now_ms = observed_at + CLAUDE_PLAN_CACHE_MAX_AGE_MS + 1;
        let mut account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        account.profile = Some(json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_5x",
            "organizationTypeObservedAt": observed_at,
            "organizationRateLimitTierObservedAt": observed_at,
        }));
        account.subscription_level = Some("Claude Max 5x".to_string());
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {
                    "planType": "claude_max_5x",
                    "planStale": true,
                    "planObservedAt": observed_at,
                }
            })),
            ..Default::default()
        });

        let usage = json!({"plan": "claude_max"});
        let subscription =
            resolve_claude_quota_subscription(&account, &usage, None, None, now_ms).unwrap();
        let quota = parse_claude_quota(&usage, Some(&subscription), now_ms);
        let extra = quota.extra_usage.unwrap();

        assert_eq!(quota.credential_message.as_deref(), Some("Claude Max"));
        assert_eq!(extra["subscription"]["planType"], "claude_max");
        assert_eq!(extra["subscription"]["planSource"], "usage_plan");
        assert_eq!(extra["subscription"]["planStale"], false);
        assert_eq!(extra["subscription"]["planObservedAt"], now_ms);
    }

    #[test]
    fn claude_subscription_clears_expired_multiplier_without_live_plan_evidence() {
        let observed_at = 1_000;
        let now_ms = observed_at + CLAUDE_PLAN_CACHE_MAX_AGE_MS + 1;
        let mut account = imported_account(ProviderType::ClaudeOAuth, json!({}));
        account.profile = Some(json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_20x",
            "organizationTypeObservedAt": observed_at,
            "organizationRateLimitTierObservedAt": observed_at,
        }));
        account.subscription_level = Some("Claude Max 20x".to_string());
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "subscription": {
                    "planType": "claude_max_20x",
                    "planStale": true,
                    "planObservedAt": observed_at,
                }
            })),
            ..Default::default()
        });
        let usage = json!({
            "five_hour": {"utilization": 10.0}
        });

        let subscription = resolve_claude_quota_subscription(&account, &usage, None, None, now_ms);
        assert!(subscription.is_none());
        let quota = parse_claude_quota(&usage, None, now_ms);
        let update = update_from_claude_quota(quota, account.profile.clone(), now_ms, 60_000);

        assert!(update.subscription_level.is_none());
        assert!(update.clear_subscription_level);
    }

    #[test]
    fn gemini_quota_groups_by_lowest_remaining_fraction() {
        let response = GeminiQuotaResponse {
            buckets: Some(vec![
                GeminiBucketInfo {
                    remaining_fraction: Some(0.75),
                    reset_time: Some("2026-07-02T00:00:00Z".to_string()),
                    model_id: Some("gemini-2.5-pro".to_string()),
                },
                GeminiBucketInfo {
                    remaining_fraction: Some(0.25),
                    reset_time: Some("2026-07-03T00:00:00Z".to_string()),
                    model_id: Some("gemini-2.5-pro".to_string()),
                },
            ]),
        };

        let quota = parse_gemini_quota(&response, None, json!({}), json!({}), 1_000);
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].name, "gemini_pro");
        assert_eq!(quota.tiers[0].utilization, Some(0.75));
    }

    #[test]
    fn copilot_usage_parses_paid_premium_quota_and_reset() {
        let account = imported_account(
            ProviderType::GitHubCopilot,
            json!({
                "copilot_plan": "individual",
                "quota_reset_date": "2026-07-31T00:00:00Z",
                "quota_snapshots": {
                    "premium_interactions": {
                        "entitlement": 100,
                        "remaining": 25,
                        "percent_remaining": 25,
                        "unlimited": false
                    }
                }
            }),
        );

        let quota = parse_copilot_usage_quota(account.raw.as_ref().unwrap(), 1_000).unwrap();

        assert_eq!(
            quota.credential_message.as_deref(),
            Some("Copilot Individual")
        );
        assert_eq!(quota.tiers[0].name, "premium");
        assert_eq!(quota.tiers[0].utilization, Some(0.75));
        assert_eq!(quota.tiers[0].used, Some(75.0));
        assert_eq!(quota.tiers[0].limit, Some(100.0));
        assert_eq!(
            quota.tiers[0].resets_at,
            dateish_to_unix_ms("2026-07-31T00:00:00Z")
        );
    }

    #[test]
    fn copilot_usage_preserves_unlimited_premium_quota() {
        let quota = parse_copilot_usage_quota(
            &json!({
                "copilot_plan": "business",
                "quota_reset_date": "2026-08-01",
                "quota_snapshots": {
                    "premium_interactions": {
                        "unlimited": true,
                        "entitlement": 9999,
                        "remaining": 1,
                        "percent_remaining": 0
                    }
                }
            }),
            1_000,
        )
        .unwrap();

        let tier = &quota.tiers[0];
        assert_eq!(
            quota.credential_message.as_deref(),
            Some("Copilot Business")
        );
        assert_eq!(tier.label.as_deref(), Some("Unlimited"));
        assert_eq!(tier.utilization, Some(0.0));
        assert_eq!(tier.used, Some(0.0));
        assert_eq!(tier.limit, None);
        assert_eq!(tier.resets_at, dateish_to_unix_ms("2026-08-01"));
    }

    #[test]
    fn copilot_usage_parses_free_monthly_remaining_and_clamps_overflow() {
        for (remaining, expected_used, expected_utilization) in
            [(20.0, 30.0, 0.6), (75.0, 0.0, 0.0)]
        {
            let quota = parse_copilot_usage_quota(
                &json!({
                    "copilot_plan": "free",
                    "monthly_quotas": {"premium_interactions": 50},
                    "limited_user_quotas": {"premium_interactions": remaining},
                    "limited_user_reset_date": "2026-08-15T12:00:00Z"
                }),
                1_000,
            )
            .unwrap();

            let tier = &quota.tiers[0];
            assert_eq!(quota.credential_message.as_deref(), Some("Copilot Free"));
            assert_eq!(tier.used, Some(expected_used));
            assert_eq!(tier.limit, Some(50.0));
            assert_eq!(tier.utilization, Some(expected_utilization));
            assert_eq!(tier.resets_at, dateish_to_unix_ms("2026-08-15T12:00:00Z"));
        }
    }

    #[tokio::test]
    async fn copilot_live_quota_uses_github_oauth_token_with_expired_subtoken() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/copilot_internal/user",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("token github-oauth-live-token")
                );
                assert_ne!(
                    headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok()),
                    Some("Bearer expired-copilot-subtoken")
                );
                axum::Json(json!({
                    "copilot_plan": "individual",
                    "quota_reset_date": "2026-08-31T00:00:00Z",
                    "quota_snapshots": {
                        "premium_interactions": {
                            "entitlement": 300,
                            "remaining": 240,
                            "unlimited": false
                        }
                    }
                }))
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut account = imported_account(
            ProviderType::GitHubCopilot,
            json!({
                "githubDomain": "github.com",
                "githubToken": "github-oauth-live-token",
                "copilotToken": {"token": "expired-copilot-subtoken"},
                "copilotUsage": {
                    "quota_snapshots": {
                        "premium_interactions": {"entitlement": 1, "remaining": 0}
                    }
                },
                "testCopilotUsageUrl": format!("http://{address}/copilot_internal/user")
            }),
        );
        account.access_token = Some("expired-copilot-subtoken".to_string());
        account.refresh_token = Some("github-oauth-live-token".to_string());
        account.expires_at = Some(1);

        let result = refresh_account_quota(
            &reqwest::Client::new(),
            &account,
            2_000,
            true,
            60_000,
            5_000,
        )
        .await
        .unwrap();
        let QuotaRefreshResult::Updated { update, .. } = result else {
            panic!("expected live Copilot quota update");
        };
        let quota = update.quota.unwrap();
        assert_eq!(quota.tiers[0].used, Some(60.0));
        assert_eq!(quota.tiers[0].limit, Some(300.0));
        assert_eq!(quota.tiers[0].utilization, Some(0.2));
        assert_eq!(update.capability_observations.len(), 1);
        assert_eq!(
            update.capability_observations[0].dimension,
            crate::domain::accounts::capability_evidence::PREMIUM_INTERACTIONS_DIMENSION
        );
        server.abort();
    }

    #[tokio::test]
    async fn copilot_live_quota_unauthorized_does_not_fallback_to_imported_snapshot() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/copilot_internal/user",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(json!({"message": "expired GitHub OAuth credential"})),
                )
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut account = imported_account(
            ProviderType::GitHubCopilot,
            json!({
                "githubToken": "github-oauth-rejected-token",
                "copilotUsage": {
                    "quota_snapshots": {
                        "premium_interactions": {"entitlement": 100, "remaining": 99}
                    }
                },
                "testCopilotUsageUrl": format!("http://{address}/copilot_internal/user")
            }),
        );
        account.refresh_token = Some("github-oauth-rejected-token".to_string());
        account.quota = Some(
            parse_copilot_usage_quota(
                account.raw.as_ref().unwrap().get("copilotUsage").unwrap(),
                1_000,
            )
            .unwrap(),
        );

        let error = refresh_account_quota(
            &reqwest::Client::new(),
            &account,
            2_000,
            true,
            60_000,
            5_000,
        )
        .await
        .unwrap_err();
        // Public quota errors normalize rejected credentials to 400, while
        // retaining the authoritative upstream status for same-account auth
        // recovery and diagnostics.
        assert_eq!(error.status_code, 400);
        assert_eq!(error.upstream_status, Some(401));
        assert!(error.partial_update.is_none());
        server.abort();
    }

    #[test]
    fn kiro_imported_snapshot_sums_active_trial_and_bonus_credits() {
        let account = imported_account(
            ProviderType::KiroOAuth,
            json!({
                "subscriptionInfo": {"subscriptionTitle": "Kiro Pro"},
                "nextDateReset": 1_774_000_000.0,
                "usageBreakdownList": [{
                    "currentUsageWithPrecision": 10.0,
                    "usageLimitWithPrecision": 100.0,
                    "freeTrialInfo": {
                        "freeTrialStatus": "ACTIVE",
                        "currentUsageWithPrecision": 2.0,
                        "usageLimitWithPrecision": 20.0
                    },
                    "bonuses": [{
                        "status": "ACTIVE",
                        "currentUsage": 3.0,
                        "usageLimit": 30.0
                    }]
                }],
                "overageConfiguration": {"overageEnabled": true}
            }),
        );

        let quota = parse_kiro_imported_quota(&account, 1_000).unwrap();

        assert_eq!(quota.credential_message.as_deref(), Some("Kiro Pro"));
        assert_eq!(quota.tiers[0].name, "kiro_agentic_requests");
        assert_eq!(quota.tiers[0].used, Some(15.0));
        assert_eq!(quota.tiers[0].limit, Some(150.0));
        assert_eq!(quota.tiers[0].utilization, Some(0.1));
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.get("overageEnabled"))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn kiro_imported_snapshot_accepts_snake_case_fields() {
        let account = imported_account(
            ProviderType::KiroOAuth,
            json!({
                "subscription_info": {"subscription_title": "Kiro Team"},
                "usage_breakdown_list": [{
                    "current_usage_with_precision": 4.0,
                    "usage_limit_with_precision": 40.0,
                    "next_date_reset": 1_774_000_000.0
                }]
            }),
        );

        let quota = parse_kiro_imported_quota(&account, 1_000).unwrap();

        assert_eq!(quota.credential_message.as_deref(), Some("Kiro Team"));
        assert_eq!(quota.tiers[0].used, Some(4.0));
        assert_eq!(quota.tiers[0].limit, Some(40.0));
        assert_eq!(quota.tiers[0].utilization, Some(0.1));
    }

    #[test]
    fn kiro_quota_uses_profile_region_and_rejects_unresolved_idc_identity() {
        let account: Account = serde_json::from_value(json!({
            "id": "kiro-cross-region-quota",
            "providerType": "kiro_oauth",
            "profile": {
                "authMethod": "idc",
                "authRegion": "eu-north-1",
                "runtimeRegion": "us-east-1",
                "apiRegion": "us-east-1",
                "profileArn": "arn:aws:codewhisperer:eu-central-1:123456789012:profile/org-profile",
                "profileProvenance": "list_available_profiles"
            },
            "raw": {"authMethod": "idc"}
        }))
        .unwrap();
        let runtime = kiro_quota_runtime_identity(&account).unwrap();
        assert_eq!(runtime.runtime_region, "eu-central-1");
        assert_eq!(
            runtime.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:eu-central-1:123456789012:profile/org-profile")
        );

        for profile in [
            json!({"authMethod": "idc", "runtimeRegion": "eu-central-1"}),
            json!({
                "authMethod": "idc",
                "profileArn": "arn:aws:codewhisperer:eu-central-1:610548660232:profile/VNECVYCYYAWN",
                "profileProvenance": "auth_method_default"
            }),
        ] {
            let unresolved: Account = serde_json::from_value(json!({
                "id": "kiro-unresolved-quota",
                "providerType": "kiro_oauth",
                "profile": profile,
                "raw": {"authMethod": "idc"}
            }))
            .unwrap();
            let error = kiro_quota_runtime_identity(&unresolved).unwrap_err();
            assert_eq!(error.status_code, 400);
            assert!(!error.retryable);
        }
    }

    #[test]
    fn cursor_imported_snapshot_parses_paid_plan_usage() {
        let account = imported_account(
            ProviderType::CursorOAuth,
            json!({
                "stripeStatus": {"membershipType": "pro_plus"},
                "currentPeriodUsage": {
                    "billingCycleEnd": 1_774_000_000_000i64,
                    "planUsage": {
                        "limit": 2000.0,
                        "used": 500.0,
                        "totalPercentUsed": 25.0
                    }
                }
            }),
        );

        let quota = parse_cursor_imported_quota(&account, 1_000).unwrap();

        assert_eq!(quota.credential_message.as_deref(), Some("Cursor Pro+"));
        assert_eq!(quota.tiers[0].name, "cursor_credits");
        assert_eq!(quota.tiers[0].utilization, Some(0.25));
        assert_eq!(quota.tiers[0].used, Some(5.0));
        assert_eq!(quota.tiers[0].limit, Some(20.0));
        assert_eq!(quota.tiers[0].unit.as_deref(), Some("USD"));
    }

    #[test]
    fn codex_banked_reset_snapshot_keeps_available_count_and_expiry() {
        let account = imported_account(
            ProviderType::CodexOAuth,
            json!({
                "codexBankedReset": {
                    "credits": [
                        {"id": "c1", "status": "available", "expiresAt": "2026-07-10T00:00:00Z"},
                        {"id": "c2", "status": "used", "expiresAt": "2026-07-05T00:00:00Z"}
                    ]
                }
            }),
        );

        let status = codex_banked_reset_status_snapshot(&account, 1_000);

        assert_eq!(
            status.get("availableCount").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            status.get("nextExpiresAt").and_then(Value::as_str),
            Some("2026-07-10T00:00:00.000Z")
        );
        assert!(status.get("readOnly").is_none());
        assert!(status.get("queriedAt").is_some_and(Value::is_null));
    }

    #[test]
    fn codex_banked_reset_snapshot_prefers_live_quota_cache_over_imported_raw() {
        let mut account = imported_account(
            ProviderType::CodexOAuth,
            json!({"codexBankedReset": {"availableCount": 99}}),
        );
        account.profile = Some(json!({
            "verifiedOpenAiClaims": {"chatgpt_account_id": "workspace-a"}
        }));
        account.quota = Some(AccountQuota {
            success: true,
            credential_message: None,
            tiers: Vec::new(),
            extra_usage: Some(json!({
                "bankedReset": {
                    "enabled": true,
                    "workspaceId": "workspace-a",
                    "availableCount": 2,
                    "credits": [],
                    "countSource": "usage",
                    "detailsSource": "unavailable",
                    "countFetchedAt": 123,
                    "detailsFetchedAt": null,
                    "detailsAvailable": false,
                    "detailsStale": false,
                    "detailsError": null,
                    "queriedAt": 123,
                    "source": "usage"
                }
            })),
        });

        let status = codex_banked_reset_status_snapshot(&account, 999_999);
        assert_eq!(status["availableCount"], 2);
        assert_eq!(status["queriedAt"], 123);
        assert_eq!(status["workspaceId"], "workspace-a");
    }

    #[test]
    fn codex_banked_reset_snapshot_rejects_cache_from_another_workspace() {
        let mut account = imported_account(
            ProviderType::CodexOAuth,
            json!({"accountId": "workspace-b"}),
        );
        account.profile = Some(json!({
            "verifiedOpenAiClaims": {"chatgpt_account_id": "workspace-b"}
        }));
        account.quota = Some(AccountQuota {
            success: true,
            extra_usage: Some(json!({
                "bankedReset": {
                    "enabled": true,
                    "workspaceId": "workspace-a",
                    "availableCount": 2,
                    "credits": [{"id": "a-credit", "status": "available"}],
                    "countSource": "details",
                    "detailsSource": "details",
                    "countFetchedAt": 123,
                    "detailsFetchedAt": 123,
                    "detailsAvailable": true,
                    "detailsStale": false,
                    "detailsError": null,
                    "queriedAt": 123,
                    "source": "upstream"
                }
            })),
            ..Default::default()
        });

        let status = codex_banked_reset_status_snapshot(&account, 999_999);
        assert!(status["availableCount"].is_null());
        assert_eq!(status["workspaceId"], "workspace-b");
        assert!(status["credits"].as_array().unwrap().is_empty());
    }

    #[test]
    fn codex_quota_uses_selected_verified_workspace_before_legacy_account_id() {
        let mut account = imported_account(
            ProviderType::CodexOAuth,
            json!({
                "accountId": "workspace-legacy"
            }),
        );
        account.profile = Some(json!({
            "accountId": "workspace-profile-default",
            "selectedChatgptAccountId": "workspace-selected",
            "verifiedOpenAiClaims": {
                "chatgpt_account_id": "workspace-profile-default"
            },
            "codexWorkspaceProvenance": {
                "workspaceId": "workspace-selected",
                "source": "authenticated_discovery",
                "verifiedAt": 123
            }
        }));

        assert_eq!(
            codex_account_id(&account).as_deref(),
            Some("workspace-selected")
        );
    }

    #[test]
    fn claude_bootstrap_profile_normalizes_only_operational_identity_fields() {
        let profile = normalize_claude_bootstrap_profile(
            &json!({
                "oauth_account": {
                    "account_uuid": "acct-1",
                    "account_email": "owner@example.com",
                    "organization_uuid": "org-1",
                    "organization_name": "Example",
                    "organization_type": "team",
                    "organization_rate_limit_tier": "tier-2",
                    "unexpected_secret": "do-not-copy"
                }
            }),
            1234,
        )
        .unwrap();

        assert_eq!(profile["accountUUID"], "acct-1");
        assert_eq!(profile["email"], "owner@example.com");
        assert_eq!(profile["organizationUUID"], "org-1");
        assert_eq!(profile["organizationRateLimitTier"], "tier-2");
        assert_eq!(profile["bootstrapRefreshedAt"], 1234);
        assert_eq!(profile["organizationTypeObservedAt"], 1234);
        assert_eq!(profile["organizationRateLimitTierObservedAt"], 1234);
        assert!(profile.get("unexpected_secret").is_none());

        let merged = merge_profile_overlay(
            Some(&json!({"providerType": "claude_oauth", "accountUUID": "old"})),
            Some(profile),
        )
        .unwrap();
        assert_eq!(merged["providerType"], "claude_oauth");
        assert_eq!(merged["accountUUID"], "acct-1");
    }

    #[test]
    fn claude_profile_enrichment_keeps_existing_fields_when_roles_fail() {
        let existing = json!({
            "email": "owner@example.com",
            "billingSource": "stripe",
            "organizationType": "claude_max",
            "claudeCliRoles": ["old-role"],
        });
        let merged = merge_claude_profile_enrichments(
            Some(&existing),
            None,
            Some(json!({
                "accountUUID": "acct-1",
                "bootstrapRefreshedAt": 200,
            })),
            None,
        )
        .unwrap();

        assert_eq!(merged["email"], "owner@example.com");
        assert_eq!(merged["billingSource"], "stripe");
        assert_eq!(merged["organizationType"], "claude_max");
        assert_eq!(merged["claudeCliRoles"], json!(["old-role"]));
        assert_eq!(merged["accountUUID"], "acct-1");
    }

    #[test]
    fn claude_profile_enrichment_keeps_roles_when_bootstrap_fails() {
        let existing = json!({
            "email": "owner@example.com",
            "billingSource": "stripe",
        });
        let merged = merge_claude_profile_enrichments(
            Some(&existing),
            Some(json!({
                "claudeCliRoles": ["max_user"],
                "rolesRefreshedAt": 200,
            })),
            None,
            None,
        )
        .unwrap();

        assert_eq!(merged["email"], "owner@example.com");
        assert_eq!(merged["billingSource"], "stripe");
        assert_eq!(merged["claudeCliRoles"], json!(["max_user"]));
        assert_eq!(merged["rolesRefreshedAt"], 200);
    }

    #[test]
    fn claude_profile_enrichment_accumulates_all_endpoint_overlays() {
        let existing = json!({
            "providerType": "claude_oauth",
            "organizationTypeObservedAt": 100,
            "organizationRateLimitTierObservedAt": 100,
        });
        let merged = merge_claude_profile_enrichments(
            Some(&existing),
            Some(json!({
                "claudeCliRoles": ["max_user"],
                "rolesRefreshedAt": 200,
            })),
            Some(json!({
                "accountUUID": "acct-1",
                "organizationType": "claude_max",
                "organizationTypeObservedAt": 300,
                "bootstrapRefreshedAt": 300,
            })),
            Some(json!({
                "organizationRateLimitTier": "default_claude_max_20x",
                "organizationRateLimitTierObservedAt": 400,
                "billingSource": "stripe",
                "profileRefreshedAt": 400,
            })),
        )
        .unwrap();

        assert_eq!(merged["providerType"], "claude_oauth");
        assert_eq!(merged["claudeCliRoles"], json!(["max_user"]));
        assert_eq!(merged["accountUUID"], "acct-1");
        assert_eq!(merged["organizationType"], "claude_max");
        assert_eq!(
            merged["organizationRateLimitTier"],
            "default_claude_max_20x"
        );
        assert_eq!(merged["billingSource"], "stripe");
        assert_eq!(merged["rolesRefreshedAt"], 200);
        assert_eq!(merged["organizationTypeObservedAt"], 300);
        assert_eq!(merged["organizationRateLimitTierObservedAt"], 400);
    }

    #[test]
    fn claude_profile_enrichment_does_not_refresh_unobserved_plan_evidence() {
        let existing = json!({
            "organizationType": "claude_max",
            "organizationRateLimitTier": "default_claude_max_5x",
            "organizationTypeObservedAt": 100,
            "organizationRateLimitTierObservedAt": 100,
        });
        let bootstrap = normalize_claude_bootstrap_profile(
            &json!({"oauth_account": {"account_uuid": "acct-1"}}),
            300,
        );
        let profile =
            parse_claude_profile_lookup(&json!({"organization": {"billing_type": "stripe"}}), 400)
                .and_then(|lookup| lookup.profile_overlay);
        let merged = merge_claude_profile_enrichments(
            Some(&existing),
            Some(json!({
                "claudeCliRoles": ["max_user"],
                "rolesRefreshedAt": 200,
            })),
            bootstrap,
            profile,
        )
        .unwrap();

        assert_eq!(merged["rolesRefreshedAt"], 200);
        assert_eq!(merged["bootstrapRefreshedAt"], 300);
        assert_eq!(merged["profileRefreshedAt"], 400);
        assert_eq!(merged["organizationTypeObservedAt"], 100);
        assert_eq!(merged["organizationRateLimitTierObservedAt"], 100);
    }

    #[test]
    fn claude_profile_lookup_keeps_plan_and_billing_source_independent() {
        let lookup = parse_claude_profile_lookup(
            &json!({
                "organization": {
                    "uuid": "org-1",
                    "name": "Example",
                    "organization_type": "team",
                    "rate_limit_tier": "tier-2",
                    "billing_type": "apple_subscription"
                }
            }),
            1_234,
        )
        .unwrap();

        assert_eq!(lookup.organization_type.as_deref(), Some("team"));
        assert_eq!(lookup.rate_limit_tier.as_deref(), Some("tier-2"));
        let profile = lookup.profile_overlay.unwrap();
        assert_eq!(profile["organizationUUID"], "org-1");
        assert_eq!(profile["organizationName"], "Example");
        assert_eq!(profile["billingSource"], "apple_subscription");
        assert_eq!(profile["profileRefreshedAt"], 1_234);
        assert_eq!(profile["organizationTypeObservedAt"], 1_234);
        assert_eq!(profile["organizationRateLimitTierObservedAt"], 1_234);
        assert!(profile.get("planType").is_none());
        assert!(profile.get("subscriptionExpiresAt").is_none());

        let unknown = parse_claude_profile_lookup(
            &json!({
                "organization": {"billing_type": "future_partner"}
            }),
            2_345,
        )
        .unwrap();
        let unknown = unknown.profile_overlay.unwrap();
        assert_eq!(unknown["billingSource"], "future_partner");
        assert_eq!(unknown["profileRefreshedAt"], 2_345);
        assert!(unknown.get("organizationTypeObservedAt").is_none());
    }

    #[test]
    fn grok_user_and_billing_normalize_legacy_plan_and_credit_metadata() {
        let user = json!({
            "email": "owner@example.com",
            "subscriptionTier": "GrokPro",
            "entitlementStatus": "active"
        });
        let billing = json!({
            "creditsRemaining": 25,
            "creditsLimit": 100,
            "billingPeriodEnd": "2026-08-01T00:00:00Z"
        });

        assert_eq!(grok_email(&user).as_deref(), Some("owner@example.com"));
        assert_eq!(grok_subscription_level(&user).as_deref(), Some("SuperGrok"));
        assert_eq!(grok_entitlement_status(&user).as_deref(), Some("active"));

        let weekly = grok_test_probe(billing.clone());
        let skipped = GrokProbe::skipped("not needed");
        let quota = grok_quota_from_probes(
            &user,
            GrokQuotaProbes {
                weekly: &weekly,
                monthly: &skipped,
                task_usage: &skipped,
                subscriptions: &skipped,
            },
            Some("GrokPro".to_string()),
            1_000,
            &[],
        );
        assert!(quota.success);
        assert_eq!(quota.credential_message.as_deref(), Some("SuperGrok"));
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].name, "grok_credits");
        assert_eq!(quota.tiers[0].used, Some(75.0));
        assert_eq!(quota.tiers[0].limit, Some(100.0));
        assert_eq!(quota.tiers[0].utilization, Some(0.75));
        assert_eq!(quota.tiers[0].resets_at, Some(1_785_542_400_000));
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.pointer("/subscription/expiryCapability"))
                .and_then(Value::as_str),
            Some("automatic_or_manual")
        );

        let observed_billing = json!({
            "config": {
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "end": "2026-08-01T00:00:00Z"
                },
                "monthlyLimit": {"val": 1000},
                "includedUsed": {"val": 275},
                "onDemandCap": {"val": 100},
                "onDemandUsed": {"val": 35},
                "prepaidBalance": {"val": 12.5}
            }
        });
        let tiers = grok_billing_tiers(&observed_billing, false);
        assert_eq!(tiers.len(), 2);
        assert_eq!(tiers[0].name, "grok_on_demand");
        assert_eq!(tiers[0].utilization, Some(0.35));
        assert_eq!(tiers[1].name, "grok_prepaid");
        assert_eq!(tiers[1].limit, Some(12.5));
        let monthly = grok_monthly_billing_tiers(&observed_billing);
        assert_eq!(monthly[0].used, Some(2.75));
        assert_eq!(monthly[0].limit, Some(10.0));
        assert_eq!(monthly[0].unit.as_deref(), Some("USD"));

        let billing_plan = json!({
            "config": {
                "subscriptionTier": "XPremiumPlus"
            }
        });
        assert_eq!(
            grok_subscription_level(&billing_plan).as_deref(),
            Some("XPremiumPlus")
        );

        let paid_zero_cap = json!({
            "config": {
                "subscriptionTier": "XPremiumPlus",
                "onDemandCap": {"val": 0},
                "onDemandUsed": {"val": 0}
            }
        });
        let weekly = grok_test_probe(paid_zero_cap);
        let skipped = GrokProbe::skipped("not needed");
        let quota = grok_quota_from_probes(
            &json!({}),
            GrokQuotaProbes {
                weekly: &weekly,
                monthly: &skipped,
                task_usage: &skipped,
                subscriptions: &skipped,
            },
            Some("XPremiumPlus".to_string()),
            1_000,
            &[],
        );
        assert!(quota.tiers.is_empty());
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.get("quotaStatus"))
                .and_then(Value::as_str),
            Some("valid_non_numeric")
        );
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.get("spendingLimitReached"))
                .and_then(Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn grok_spending_limit_is_a_successful_exhausted_quota_snapshot() {
        let spending_limited = GrokProbe {
            body: None,
            issue: None,
            spending_limited: true,
            status_code: Some(402),
            skipped_reason: None,
        };
        let skipped = GrokProbe::skipped("not needed");
        let quota = grok_quota_from_probes(
            &json!({"subscriptionTier": "SuperGrok"}),
            GrokQuotaProbes {
                weekly: &spending_limited,
                monthly: &skipped,
                task_usage: &skipped,
                subscriptions: &skipped,
            },
            Some("SuperGrok".to_string()),
            1_000,
            &[],
        );

        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].name, "grok_spending_limit");
        assert_eq!(quota.tiers[0].utilization, Some(1.0));

        let exhausted = json!({
            "config": {
                "onDemandCap": {"val": 0},
                "onDemandUsed": {"val": 0},
                "prepaidBalance": {"val": 0}
            }
        });
        assert!(grok_billing_reports_exhausted(&exhausted, false));
        let tiers = grok_billing_tiers(&exhausted, false);
        assert_eq!(tiers.len(), 1);
        assert_eq!(tiers[0].name, "grok_spending_limit");
        assert_eq!(tiers[0].utilization, Some(1.0));
        assert!(grok_billing_tiers(&exhausted, true).is_empty());
    }

    #[test]
    fn grok_media_entitlement_requires_paid_access_and_authoritative_quota() {
        let weekly = grok_test_probe(json!({
            "config": {
                "subscriptionTier": "SuperGrok",
                "creditUsagePercent": 12.5
            }
        }));
        let monthly = grok_test_probe(json!({"config": {}}));
        let skipped = GrokProbe::skipped("not needed");

        let observation = grok_media_entitlement_observation(
            &json!({"subscriptionTier": "SuperGrok"}),
            &weekly,
            &monthly,
            &skipped,
            1_000,
        );

        assert_eq!(observation.dimension, MEDIA_ENTITLEMENT_DIMENSION);
        assert_eq!(
            observation.state,
            AccountCapabilityObservationState::Supported
        );
        assert_eq!(
            observation.reason.as_deref(),
            Some("paid_access_with_authoritative_quota")
        );
    }

    #[test]
    fn grok_media_entitlement_rejects_complete_free_billing() {
        let weekly = grok_test_probe(json!({"config": {"subscriptionTier": "Free"}}));
        let monthly = grok_test_probe(json!({"config": {}}));
        let skipped = GrokProbe::skipped("not needed");

        let observation = grok_media_entitlement_observation(
            &json!({"subscriptionTier": "Free"}),
            &weekly,
            &monthly,
            &skipped,
            1_000,
        );

        assert_eq!(
            observation.state,
            AccountCapabilityObservationState::Unsupported
        );
        assert_eq!(
            observation.reason.as_deref(),
            Some("free_plan_with_complete_billing")
        );
    }

    #[test]
    fn grok_media_entitlement_rejects_forbidden_billing_probe() {
        let forbidden = GrokProbe {
            body: None,
            issue: Some(GrokProbeIssue {
                probe: "weekly_billing",
                message: "forbidden".to_string(),
                next_refresh_at: None,
            }),
            spending_limited: false,
            status_code: Some(403),
            skipped_reason: None,
        };
        let monthly = grok_test_probe(json!({"config": {}}));
        let skipped = GrokProbe::skipped("not needed");

        let observation = grok_media_entitlement_observation(
            &json!({"subscriptionTier": "SuperGrok"}),
            &forbidden,
            &monthly,
            &skipped,
            1_000,
        );

        assert_eq!(
            observation.state,
            AccountCapabilityObservationState::Unsupported
        );
        assert_eq!(
            observation.reason.as_deref(),
            Some("billing_probe_forbidden")
        );
    }

    #[test]
    fn grok_media_entitlement_is_unknown_for_partial_or_shape_drifted_probes() {
        let failed = grok_test_failed_probe("weekly_billing", "temporarily unavailable");
        let monthly = grok_test_probe(json!({"config": {}}));
        let skipped = GrokProbe::skipped("not needed");
        let partial =
            grok_media_entitlement_observation(&json!({}), &failed, &monthly, &skipped, 1_000);
        assert_eq!(partial.state, AccountCapabilityObservationState::Unknown);
        assert_eq!(partial.reason.as_deref(), Some("billing_probe_incomplete"));

        let weekly = grok_test_probe(json!({
            "config": {"subscriptionTier": "SuperGrok"}
        }));
        let drifted = grok_media_entitlement_observation(
            &json!({"subscriptionTier": "SuperGrok"}),
            &weekly,
            &monthly,
            &skipped,
            2_000,
        );
        assert_eq!(drifted.state, AccountCapabilityObservationState::Unknown);
        assert_eq!(
            drifted.reason.as_deref(),
            Some("paid_access_without_authoritative_quota")
        );
    }

    #[test]
    fn grok_media_entitlement_does_not_use_stale_account_plan_fields() {
        let account = Account {
            subscription_level: Some("SuperGrok".to_string()),
            entitlement_status: Some("active".to_string()),
            ..imported_account(ProviderType::GrokOAuth, json!({}))
        };
        let failed = grok_test_failed_probe("weekly_billing", "temporarily unavailable");
        let skipped = GrokProbe::skipped("not needed");

        let observation =
            grok_media_entitlement_observation(&json!({}), &failed, &skipped, &skipped, 1_000);

        assert_eq!(account.subscription_level.as_deref(), Some("SuperGrok"));
        assert_eq!(
            observation.state,
            AccountCapabilityObservationState::Unknown
        );
        assert_eq!(
            observation.reason.as_deref(),
            Some("billing_probe_incomplete")
        );
    }

    #[test]
    fn grok_billing_failure_preserves_the_previous_credit_tier() {
        let previous = AccountQuotaTier {
            name: "grok_credits".to_string(),
            label: Some("Credits".to_string()),
            utilization: Some(0.25),
            used: Some(25.0),
            limit: Some(100.0),
            unit: Some("credits".to_string()),
            resets_at: None,
            ..Default::default()
        };
        let failed = grok_test_failed_probe("weekly_billing", "billing temporarily unavailable");
        let skipped = GrokProbe::skipped("not needed");
        let quota = grok_quota_from_probes(
            &json!({"subscriptionTier": "SuperGrok"}),
            GrokQuotaProbes {
                weekly: &failed,
                monthly: &skipped,
                task_usage: &skipped,
                subscriptions: &skipped,
            },
            Some("SuperGrok".to_string()),
            1_000,
            std::slice::from_ref(&previous),
        );

        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].name, previous.name);
        assert_eq!(quota.tiers[0].utilization, previous.utilization);
        assert_eq!(quota.tiers[0].used, previous.used);
        assert_eq!(quota.tiers[0].limit, previous.limit);
        let extra = quota.extra_usage.as_ref().unwrap();
        assert_eq!(extra["quotaStatus"], "partial");
        assert_eq!(extra["staleTierNames"], json!(["grok_credits"]));
        assert!(extra["warnings"][0]
            .as_str()
            .is_some_and(|warning| warning.contains("billing temporarily unavailable")));
    }

    #[test]
    fn grok_parses_weekly_monthly_product_task_and_subscription_expiry() {
        let weekly = json!({
            "config": {
                "currentPeriod": {
                    "type": "WEEKLY",
                    "end": "2026-07-27T00:00:00Z"
                },
                "creditUsagePercent": 12.5,
                "productUsage": [{"product": "GrokBuild", "usagePercent": 25.0}],
                "weeklyCredits": {"total": {"val": 1000}, "remaining": {"val": 875}}
            }
        });
        let monthly = json!({
            "config": {
                "monthlyLimit": {"val": 15000},
                "used": {"val": 7500},
                "billingPeriodEnd": "2026-08-01T00:00:00Z"
            }
        });
        let tasks = json!({
            "frequentUsage": 2,
            "frequentLimit": 10,
            "occasionalUsage": 3,
            "occasionalLimit": 30
        });
        let subscriptions = json!({
            "subscriptions": [{
                "tier": "XPremium",
                "status": "SUBSCRIPTION_STATUS_ACTIVE",
                "expiresAt": "2026-12-31T00:00:00Z"
            }]
        });
        let weekly = grok_test_probe(weekly);
        let monthly = grok_test_probe(monthly);
        let tasks = grok_test_probe(tasks);
        let subscriptions = grok_test_probe(subscriptions);
        let quota = grok_quota_from_probes(
            &json!({"subscriptionTier": "XPremium"}),
            GrokQuotaProbes {
                weekly: &weekly,
                monthly: &monthly,
                task_usage: &tasks,
                subscriptions: &subscriptions,
            },
            Some("XPremium".to_string()),
            1_000,
            &[],
        );

        assert!(quota.success);
        let tier = |name: &str| quota.tiers.iter().find(|tier| tier.name == name).unwrap();
        assert_eq!(tier("grok_weekly").utilization, Some(0.125));
        assert_eq!(tier("grok_product_grokbuild").utilization, Some(0.25));
        assert_eq!(tier("grok_monthly").used, Some(75.0));
        assert_eq!(tier("grok_monthly").limit, Some(150.0));
        assert_eq!(tier("grok_monthly").unit.as_deref(), Some("USD"));
        assert_eq!(tier("grok_frequent").utilization, Some(0.2));
        assert_eq!(tier("grok_occasional").utilization, Some(0.1));
        let extra = quota.extra_usage.as_ref().unwrap();
        assert_eq!(extra["quotaStatus"], "valid_numeric");
        assert_eq!(
            extra["subscription"]["expiresAt"],
            "2026-12-31T00:00:00+00:00"
        );
        assert_eq!(extra["subscription"]["expiresKind"], "subscription");
    }

    #[test]
    fn grok_does_not_treat_billing_period_or_inactive_subscription_as_expiry() {
        let user = json!({"subscriptionTier": "XPremium"});
        let monthly = json!({
            "config": {
                "billingPeriodEnd": "2030-01-01T00:00:00Z"
            }
        });
        let inactive_subscriptions = grok_test_probe(json!({
            "subscriptions": [{
                "tier": "XPremium",
                "status": "SUBSCRIPTION_STATUS_CANCELED",
                "expiresAt": "2030-02-01T00:00:00Z"
            }]
        }));
        let subscription = grok_subscription_json(
            &user,
            None,
            Some(&monthly),
            &inactive_subscriptions,
            Some("XPremium".to_string()),
        );

        assert!(subscription["expiresAt"].is_null());
        assert_eq!(subscription["expiryAvailability"], "upstream_not_provided");

        let inactive = json!({
            "subscription": {
                "tier": "XPremium",
                "status": "SUBSCRIPTION_STATUS_INACTIVE",
                "expiresAt": "2030-03-01T00:00:00Z"
            }
        });
        assert!(grok_subscription_expiry_at(&inactive).is_none());

        let token_expiry = json!({
            "subscriptionTier": "XPremium",
            "expiresAt": "2030-04-01T00:00:00Z"
        });
        assert!(grok_subscription_expiry_at(&token_expiry).is_none());

        let active = json!({
            "subscription": {
                "tier": "XPremium",
                "status": "SUBSCRIPTION_STATUS_ACTIVE",
                "expiresAt": "2030-05-01T00:00:00Z"
            }
        });
        assert!(grok_subscription_expiry_at(&active).is_some());
    }

    fn grok_test_probe(body: Value) -> GrokProbe {
        GrokProbe {
            body: Some(body),
            issue: None,
            spending_limited: false,
            status_code: Some(200),
            skipped_reason: None,
        }
    }

    fn grok_test_failed_probe(probe: &'static str, message: &str) -> GrokProbe {
        GrokProbe {
            body: None,
            issue: Some(GrokProbeIssue {
                probe,
                message: message.to_string(),
                next_refresh_at: Some(120_000),
            }),
            spending_limited: false,
            status_code: Some(502),
            skipped_reason: None,
        }
    }

    #[test]
    fn codex_personal_credit_projection_uses_exact_decimal_strings() {
        for value in ["1", "0.01", "+000.00010"] {
            assert_eq!(decimal_string_is_positive(value), Some(true));
        }
        for value in ["0", "0.000", "-1", "-0.01"] {
            assert_eq!(decimal_string_is_positive(value), Some(false));
        }
        for value in ["", ".", "1e-3", "NaN", "1.2.3"] {
            assert_eq!(decimal_string_is_positive(value), None);
        }

        let credits: CodexPersonalCredits = serde_json::from_value(json!({
            "has_credits": true,
            "unlimited": false,
            "overage_limit_reached": false,
            "balance": "0000000000000000000000.000000000000000001",
            "approx_local_messages": [3, 4]
        }))
        .unwrap();
        let projection = codex_personal_credits_projection(&credits);
        assert_eq!(projection["available"], true);
        assert_eq!(
            projection["balance"],
            "0000000000000000000000.000000000000000001"
        );
        assert_eq!(projection["approxLocalMessages"], json!([3, 4]));

        let capped: CodexPersonalCredits = serde_json::from_value(json!({
            "has_credits": true,
            "overage_limit_reached": true,
            "balance": "10.00"
        }))
        .unwrap();
        assert_eq!(
            codex_personal_credits_projection(&capped)["available"],
            false
        );
    }

    #[tokio::test]
    async fn amazon_q_quota_uses_official_cli_operation_and_preserves_account_scope() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let observations_for_route = std::sync::Arc::clone(&observations);
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |headers: axum::http::HeaderMap, body: bytes::Bytes| {
                let observations = std::sync::Arc::clone(&observations_for_route);
                async move {
                    observations.lock().unwrap().push(json!({
                        "target": headers.get("x-amz-target").and_then(|value| value.to_str().ok()),
                        "authorization": headers.get("authorization").and_then(|value| value.to_str().ok()),
                        "userAgent": headers.get("user-agent").and_then(|value| value.to_str().ok()),
                        "body": serde_json::from_slice::<Value>(&body).unwrap(),
                    }));
                    axum::Json(json!({
                        "subscriptionInfo": {"subscriptionTitle": "Amazon Q Developer Pro"},
                        "usageBreakdownList": [{
                            "resourceType": "CREDITS",
                            "displayName": "Monthly requests",
                            "currentValue": 25,
                            "limitValue": 100,
                            "nextResetDate": 4_102_444_800_000_i64
                        }],
                        "overageConfiguration": {"overageEnabled": false}
                    }))
                }
            }),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let endpoint = format!("http://{address}/");
        let account: Account = serde_json::from_value(json!({
            "id": "amazon-q-quota-account",
            "providerType": "amazon_q_oauth",
            "accessToken": "amazon-q-quota-access",
            "refreshToken": "amazon-q-quota-refresh",
            "authIdentityGeneration": 7,
            "tokenRefreshGeneration": 3,
            "profile": {
                "accountId": "amazon-q-quota-subject",
                "runtimeRegion": "us-east-1",
                "authRegion": "us-east-1"
            },
            "raw": {"testAmazonQRuntimeUrl": endpoint}
        }))
        .unwrap();
        let update = refresh_amazon_q_quota(
            &reqwest::Client::new(),
            &account,
            1_700_000_000_000,
            300_000,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(
            update.subscription_level.as_deref(),
            Some("Amazon Q Developer Pro")
        );
        assert!(update.quota.as_ref().is_some_and(|quota| quota.success));
        assert_eq!(
            update
                .raw
                .as_ref()
                .and_then(|raw| raw.get("amazonQUsageLimits"))
                .and_then(|usage| usage.pointer("/subscriptionInfo/subscriptionTitle")),
            Some(&json!("Amazon Q Developer Pro"))
        );
        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(
            observations[0]["target"],
            crate::clients::oauth::amazon_q_runtime::AMAZON_Q_USAGE_TARGET
        );
        assert_eq!(
            observations[0]["authorization"],
            "Bearer amazon-q-quota-access"
        );
        assert_eq!(observations[0]["body"]["origin"], "CLI");
        assert!(observations[0]["userAgent"]
            .as_str()
            .is_some_and(|value| value.contains("AmazonQ-For-CLI")));
        drop(observations);
        server.abort();
    }

    fn imported_account(provider_type: ProviderType, raw: Value) -> Account {
        Account {
            id: "acct-imported".to_string(),
            provider_type,
            auth_identity_generation: 1,
            token_refresh_generation: 0,
            email: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: None,
            raw: Some(raw),
            subscription_level: None,
            entitlement_status: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
            capacity_pool_limits: Default::default(),
            capability_observations: Default::default(),
        }
    }
}
