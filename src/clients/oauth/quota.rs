use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::clients::oauth::codex_reset_credits::{
    codex_authenticated_get, fetch_reset_credit_details, merge_reset_credit_snapshot,
    normalize_imported_snapshot, parse_usage_available_count,
};
use crate::clients::oauth::kiro_device::{
    default_profile_arn, fetch_usage_limits, machine_id_from_refresh_token, quota_from_usage_limits,
};
use crate::domain::accounts::store::{
    Account, AccountQuota, AccountQuotaTier, AccountRefreshUpdate,
};
use crate::domain::claude_cli::claude_cli_user_agent;
use crate::domain::providers::model::ProviderType;

pub const QUOTA_FAILURE_COOLDOWN_MS: i64 = 2 * 60 * 1000;

fn quota_request_timeout(timeout_ms: i64) -> Duration {
    Duration::from_millis(timeout_ms.clamp(1_000, 120_000) as u64)
}

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CLAUDE_BOOTSTRAP_URL: &str = "https://api.anthropic.com/api/claude_cli/bootstrap";
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CHATGPT_ACCOUNTS_CHECK_URL: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
const CHATGPT_SUBSCRIPTIONS_URL: &str = "https://chatgpt.com/backend-api/subscriptions";
const GROK_USER_URL: &str = "https://cli-chat-proxy.grok.com/v1/user?include=subscription";
const GROK_BILLING_CREDITS_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
const GEMINI_LOAD_CODE_ASSIST_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist";
const GEMINI_RETRIEVE_USER_QUOTA_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";
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
    pub message: String,
    pub retryable: bool,
    pub next_refresh_at: Option<i64>,
}

impl QuotaRefreshFailure {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status_code: 400,
            message: message.into(),
            retryable: false,
            next_refresh_at: None,
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
            message: format!(
                "{} quota request failed: upstream HTTP {}: {}",
                provider_type.as_str(),
                upstream_status.as_u16(),
                truncate(&crate::logging::redact_sensitive_text(&body), 240)
            ),
            retryable,
            next_refresh_at,
        }
    }

    fn network(provider_type: ProviderType, error: reqwest::Error, now_ms: i64) -> Self {
        Self {
            status_code: 502,
            message: format!("{} quota request failed: {error}", provider_type.as_str()),
            retryable: true,
            next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        }
    }

    fn parse(provider_type: ProviderType, error: impl std::fmt::Display, now_ms: i64) -> Self {
        Self {
            status_code: 502,
            message: format!(
                "{} quota response is not valid JSON: {error}",
                provider_type.as_str()
            ),
            retryable: false,
            next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
        }
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
        ProviderType::GrokOAuth => {
            refresh_grok_quota(http, account, now_ms, success_cooldown_ms, request_timeout).await?
        }
        ProviderType::GitHubCopilot | ProviderType::CursorOAuth | ProviderType::CursorApiKey => {
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

    Ok(QuotaRefreshResult::Updated {
        update,
        message: "quota refreshed from upstream provider".to_string(),
    })
}

async fn refresh_codex_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let workspace_id = codex_account_id(account);
    let usage_request = codex_authenticated_get(
        http,
        &format!("{CHATGPT_USAGE_URL}?supports_rewardless_invites=true"),
        access_token,
        workspace_id.as_deref(),
        request_timeout,
    );
    let (body, reset_credit_details) = tokio::join!(
        request_json(account.provider_type, usage_request, now_ms),
        fetch_reset_credit_details(http, access_token, workspace_id.as_deref(), request_timeout,),
    );
    let body = body?;
    let usage: CodexUsageResponse = serde_json::from_value(body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let previous_reset_credits = codex_banked_reset_status_from_account(account);
    let reset_credits = merge_reset_credit_snapshot(
        parse_usage_available_count(&body),
        reset_credit_details,
        previous_reset_credits.as_ref(),
        workspace_id.as_deref(),
        now_ms,
    );

    let usage_plan_type = usage
        .plan_type
        .as_deref()
        .map(normalize_chatgpt_plan_type)
        .filter(|value| !value.is_empty());
    let usage_plan_label = usage_plan_type.as_deref().map(format_chatgpt_plan_label);
    let account_lookup =
        fetch_chatgpt_account_lookup(http, access_token, workspace_id.as_deref(), request_timeout)
            .await;
    let subscription_lookup = fetch_chatgpt_subscription_lookup(
        http,
        access_token,
        workspace_id.as_deref(),
        request_timeout,
    )
    .await;
    let subscription = merge_subscription_lookup(account_lookup, subscription_lookup);
    let subscription_level = subscription
        .as_ref()
        .and_then(|item| item.plan_label.clone())
        .or_else(|| usage_plan_label.clone());
    let subscription_json = subscription.as_ref().map(|item| {
        json!({
            "planType": item.plan_type,
            "planLabel": item.plan_label,
            "expiresAt": item.expires_at,
            "expiresSource": item.expires_source,
            "expiresKind": item.expires_kind,
        })
    });

    let tiers = codex_tiers_from_rate_limit(usage.rate_limit);

    let quota = AccountQuota {
        success: true,
        credential_message: subscription_level.clone(),
        tiers,
        extra_usage: Some(json!({
            "raw": body,
            "subscription": subscription_json,
            "bankedReset": reset_credits,
            "queriedAt": now_ms,
        })),
    };
    Ok(update_from_quota(
        quota,
        subscription_level,
        None,
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
        .header(USER_AGENT, claude_cli_user_agent())
        .header("x-app", "cli")
        .timeout(request_timeout);
    let (body, profile_lookup, bootstrap_profile) = tokio::join!(
        request_json(account.provider_type, usage_request, now_ms),
        fetch_claude_profile_lookup(http, access_token, request_timeout),
        fetch_claude_bootstrap_profile_with_timeout(http, access_token, request_timeout, now_ms,),
    );
    let body = body?;
    let plan_label = profile_lookup
        .as_ref()
        .and_then(|lookup| lookup.plan_label.clone());
    let quota = parse_claude_quota(&body, plan_label, now_ms);
    let subscription_level = quota.credential_message.clone();
    let bootstrap_merged = merge_profile_overlay(account.profile.as_ref(), bootstrap_profile);
    let existing = bootstrap_merged.as_ref().or(account.profile.as_ref());
    let profile = merge_profile_overlay(
        existing,
        profile_lookup.and_then(|lookup| lookup.profile_overlay),
    )
    .or(bootstrap_merged);
    Ok(update_from_quota(
        quota,
        subscription_level,
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
    let body = match response.json::<Value>().await {
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

async fn refresh_gemini_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let load_request = http
        .post(GEMINI_LOAD_CODE_ASSIST_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .json(&json!({
            "metadata": {
                "ideType": "GEMINI_CLI",
                "pluginType": "GEMINI"
            }
        }))
        .timeout(request_timeout);
    let load_body = request_json(account.provider_type, load_request, now_ms).await?;
    let load: GeminiLoadCodeAssistResponse = serde_json::from_value(load_body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let project_id = load
        .cloudaicompanion_project
        .as_ref()
        .and_then(extract_project_id);
    let plan_label = load
        .current_tier
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut quota_body = json!({});
    if let Some(project_id) = project_id.as_deref() {
        quota_body["project"] = Value::String(project_id.to_string());
    }
    let quota_request = http
        .post(GEMINI_RETRIEVE_USER_QUOTA_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .json(&quota_body)
        .timeout(request_timeout);
    let body = request_json(account.provider_type, quota_request, now_ms).await?;
    let quota_response: GeminiQuotaResponse = serde_json::from_value(body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let quota = parse_gemini_quota(&quota_response, plan_label, load_body, body, now_ms);
    let subscription_level = quota.credential_message.clone();
    Ok(update_from_quota(
        quota,
        subscription_level,
        None,
        now_ms,
        success_cooldown_ms,
    ))
}

async fn refresh_grok_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
    request_timeout: Duration,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let user_probe = grok_probe_json(
        http,
        account,
        GROK_USER_URL,
        access_token,
        request_timeout,
        now_ms,
    )
    .await?;
    let billing_probe = grok_probe_json(
        http,
        account,
        GROK_BILLING_CREDITS_URL,
        access_token,
        request_timeout,
        now_ms,
    )
    .await;

    let (billing_body, billing_spending_limited, billing_error) = match billing_probe {
        Ok(body) => (Some(body), false, None),
        Err(error) if error.status_code == 402 => (None, true, None),
        Err(error) => (None, false, Some(error)),
    };
    let subscription_level = grok_subscription_level(&user_probe)
        .or_else(|| {
            billing_body
                .as_ref()
                .and_then(|billing| grok_subscription_level(billing))
        })
        .or_else(|| grok_access_plan(&user_probe, billing_body.as_ref()))
        .or_else(|| account.subscription_level.clone())
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
        billing_body.as_ref(),
        subscription_level.clone(),
        now_ms,
        billing_spending_limited,
        &previous_billing_tiers,
        billing_error.as_ref().map(|error| error.message.as_str()),
    );
    let profile = merge_profile_overlay(
        account.profile.as_ref(),
        Some(grok_profile_from_user_probe(
            &user_probe,
            billing_body.as_ref(),
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
    update.email = grok_email(&user_probe);
    update.entitlement_status = grok_entitlement_status(&user_probe);
    if let Some(error) = billing_error {
        update.last_refresh_error = Some(error.message);
        if let Some(next_refresh_at) = error.next_refresh_at {
            update.quota_next_refresh_at = Some(next_refresh_at);
        }
    }
    Ok(update)
}

async fn grok_probe_json(
    http: &reqwest::Client,
    account: &Account,
    url: &str,
    access_token: &str,
    request_timeout: Duration,
    now_ms: i64,
) -> Result<Value, QuotaRefreshFailure> {
    let mut request = http
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(ACCEPT, "application/json")
        .header(
            USER_AGENT,
            "grok-pager/0.2.93 grok-shell/0.2.93 (linux; x86_64)",
        )
        .header("x-xai-token-auth", "xai-grok-cli")
        .header("x-grok-client-identifier", "grok-pager")
        .header("x-grok-client-version", "0.2.93")
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
    request_json(account.provider_type, request, now_ms).await
}

fn grok_quota_from_probes(
    user: &Value,
    billing: Option<&Value>,
    subscription_level: Option<String>,
    now_ms: i64,
    billing_spending_limited: bool,
    previous_billing_tiers: &[AccountQuotaTier],
    billing_error: Option<&str>,
) -> AccountQuota {
    let subscription_access = grok_subscription_level(user)
        .or_else(|| billing.and_then(grok_subscription_level))
        .is_some_and(|tier| {
            !matches!(tier.to_ascii_lowercase().as_str(), "free" | "none" | "null")
        });
    let spending_limited = user
        .pointer("/spendingLimitReached")
        .or_else(|| user.pointer("/spending_limit_reached"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || billing_spending_limited
        || billing
            .is_some_and(|billing| grok_billing_reports_exhausted(billing, subscription_access));
    let mut tiers = billing
        .map(|billing| grok_billing_tiers(billing, subscription_access))
        .unwrap_or_default();
    if spending_limited && tiers.is_empty() {
        tiers.push(AccountQuotaTier {
            name: "grok_spending_limit".to_string(),
            utilization: Some(1.0),
            resets_at: Some(now_ms.saturating_add(60 * 60_000)),
            ..Default::default()
        });
    } else if billing.is_none() && !spending_limited {
        tiers.extend_from_slice(previous_billing_tiers);
    }
    AccountQuota {
        success: true,
        credential_message: subscription_level.clone(),
        tiers,
        extra_usage: Some(json!({
            "provider": "grok",
            "user": user,
            "billing": billing,
            "billingError": billing_error,
            "spendingLimitReached": spending_limited,
            "subscription": {
                "planType": subscription_level.clone(),
                "planLabel": subscription_level,
                "expiresAt": Value::Null,
                "expiryCapability": "research_pending",
            },
            "queriedAt": now_ms,
        })),
    }
}

fn grok_billing_tiers(body: &Value, subscription_access: bool) -> Vec<AccountQuotaTier> {
    let resets_at = grok_timestamp_at(
        body,
        &[
            "/config/billingPeriodEnd",
            "/config/billing_period_end",
            "/config/currentPeriod/end",
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
    );
    let monthly_limit = grok_number_at(
        body,
        &[
            "/config/monthlyLimit",
            "/config/monthly_limit",
            "/monthlyLimit",
            "/monthly_limit",
        ],
    );
    let included_used = grok_number_at(
        body,
        &[
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
    if let Some(limit) = monthly_limit.filter(|value| *value > 0.0) {
        let used = included_used.unwrap_or(0.0).max(0.0);
        tiers.push(grok_credit_tier("grok_monthly", used, limit, resets_at));
    }
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
            utilization,
            used: inferred_used,
            limit,
            unit: Some("credits".to_string()),
            resets_at,
        }
    })
}

fn grok_credit_tier(name: &str, used: f64, limit: f64, resets_at: Option<i64>) -> AccountQuotaTier {
    AccountQuotaTier {
        name: name.to_string(),
        utilization: (limit > 0.0).then(|| (used / limit).clamp(0.0, 1.0)),
        used: Some(used),
        limit: Some(limit),
        unit: Some("credits".to_string()),
        resets_at,
    }
}

fn grok_number_at(value: &Value, pointers: &[&str]) -> Option<f64> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        let value = value.get("val").unwrap_or(value);
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|value| value as f64))
            .or_else(|| value.as_u64().map(|value| value as f64))
            .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
            .filter(|value| value.is_finite())
    })
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
    let access_token = required_access_token(account)?;
    let metadata = antigravity_code_assist_metadata();
    let load_request = http
        .post(GEMINI_LOAD_CODE_ASSIST_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .header("client-metadata", metadata.to_string())
        .json(&json!({ "metadata": metadata }))
        .timeout(request_timeout);
    let load_body = request_json(account.provider_type, load_request, now_ms).await?;
    let load: GeminiLoadCodeAssistResponse = serde_json::from_value(load_body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let project_id = load
        .cloudaicompanion_project
        .as_ref()
        .and_then(extract_project_id)
        .or_else(|| {
            account
                .profile
                .as_ref()
                .and_then(|value| string_at(value, &["/projectId", "/project_id"]))
        });
    let plan_label = load
        .current_tier
        .as_ref()
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let mut quota_body = json!({});
    if let Some(project_id) = project_id.as_deref() {
        quota_body["project"] = Value::String(project_id.to_string());
    }
    let quota_request = http
        .post(GEMINI_RETRIEVE_USER_QUOTA_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .json(&quota_body)
        .timeout(request_timeout);
    let body = request_json(account.provider_type, quota_request, now_ms).await?;
    let quota_response: GeminiQuotaResponse = serde_json::from_value(body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;
    let quota = parse_gemini_quota(&quota_response, plan_label, load_body, body, now_ms);
    let subscription_level = quota.credential_message.clone();
    Ok(update_from_quota(
        quota,
        subscription_level,
        None,
        now_ms,
        success_cooldown_ms,
    ))
}

fn refresh_imported_snapshot_quota(
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let quota = match account.provider_type {
        ProviderType::GitHubCopilot => parse_copilot_imported_quota(account, now_ms),
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
    Ok(update_from_quota(
        quota,
        subscription_level,
        None,
        now_ms,
        success_cooldown_ms,
    ))
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
    let api_region = raw
        .and_then(|value| string_at(value, &["/apiRegion", "/api_region"]))
        .or_else(|| profile.and_then(|value| string_at(value, &["/apiRegion", "/api_region"])))
        .or_else(|| profile.and_then(region_from_profile_value))
        .unwrap_or_else(|| "us-east-1".to_string());
    let profile_arn = profile
        .and_then(|value| string_at(value, &["/profileArn", "/profile_arn"]))
        .or_else(|| raw.and_then(|value| string_at(value, &["/resolvedProfileArn", "/profileArn"])))
        .unwrap_or_else(|| default_profile_arn(raw.unwrap_or(&Value::Null), &api_region));
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
            &api_region,
            &profile_arn,
            &machine_id,
            access_token,
            kiro_quota_token_type(account),
        ),
    )
    .await
    .map_err(|_| QuotaRefreshFailure {
        status_code: 504,
        message: "kiro_oauth quota request timed out".to_string(),
        retryable: true,
        next_refresh_at: Some(now_ms.saturating_add(QUOTA_FAILURE_COOLDOWN_MS)),
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
    Ok(
        update_from_quota(quota, subscription_level, None, now_ms, success_cooldown_ms)
            .with_raw(raw),
    )
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

fn region_from_profile_value(value: &Value) -> Option<String> {
    let arn = string_at(value, &["/profileArn", "/profile_arn"])?;
    let mut parts = arn.split(':');
    (parts.next() == Some("arn")).then_some(())?;
    (parts.next() == Some("aws")).then_some(())?;
    (parts.next() == Some("codewhisperer")).then_some(())?;
    parts.next().map(str::to_string)
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
    let response = request
        .send()
        .await
        .map_err(|error| QuotaRefreshFailure::network(provider_type, error, now_ms))?;
    let status = response.status();
    let retry_after = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body = response
        .text()
        .await
        .map_err(|error| QuotaRefreshFailure::network(provider_type, error, now_ms))?;
    if !status.is_success() {
        return Err(QuotaRefreshFailure::upstream(
            provider_type,
            status,
            body,
            retry_after,
            now_ms,
        ));
    }
    serde_json::from_str(&body)
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

fn quota_percent_from_tiers(tiers: &[AccountQuotaTier]) -> Option<f64> {
    tiers
        .iter()
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
                utilization: Some(utilization),
                used: None,
                limit: None,
                unit: Some("percent".to_string()),
                resets_at: window.reset_at.map(|value| value.saturating_mul(1000)),
            });
        }
    }
    sort_codex_quota_tiers(&mut tiers);
    tiers
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

/// Align with desktop `CodexRateWindowNormalizer`: session (5h) first, weekly (7d) second.
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

/// `/wham/usage` reports consumed quota on a 0..100 percent scale (desktop keeps the
/// raw value). Do not treat `(0, 1]` as a 0..1 fraction or `1.0` becomes 100%.
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

fn parse_claude_quota(body: &Value, plan_label: Option<String>, now_ms: i64) -> AccountQuota {
    const KNOWN_TIERS: &[&str] = &[
        "five_hour",
        "seven_day",
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
    AccountQuota {
        success: true,
        credential_message: plan_label,
        tiers,
        extra_usage: Some(json!({
            "raw": body,
            "extraUsage": body.get("extra_usage"),
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
    tiers.push(AccountQuotaTier {
        name: normalize_claude_tier_name(name).to_string(),
        utilization: Some(percent_to_fraction(utilization)),
        used: None,
        limit: None,
        unit: Some("percent".to_string()),
        resets_at: window.resets_at.as_deref().and_then(rfc3339_to_unix_ms),
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
            utilization: Some((1.0 - remaining).clamp(0.0, 1.0)),
            used: None,
            limit: None,
            unit: Some("percent".to_string()),
            resets_at: reset_time.as_deref().and_then(rfc3339_to_unix_ms),
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

fn parse_copilot_imported_quota(
    account: &Account,
    now_ms: i64,
) -> Result<AccountQuota, QuotaRefreshFailure> {
    let snapshot = require_imported_snapshot(
        account,
        &[
            "/copilotUsage",
            "/copilot_usage",
            "/usage",
            "/quota",
            "/billingOrQuotaSnapshot",
            "",
        ],
        "Copilot usage response",
    )?;
    let usage = serde_json::from_value::<CopilotImportedUsage>(snapshot.clone()).ok();
    let premium = usage
        .as_ref()
        .and_then(|usage| usage.quota_snapshots.as_ref())
        .and_then(|snapshots| snapshots.premium_interactions.clone())
        .or_else(|| {
            value_at(
                &snapshot,
                &[
                    "/quota_snapshots/premium_interactions",
                    "/quotaSnapshots/premiumInteractions",
                    "/premium_interactions",
                    "/premiumInteractions",
                ],
            )
            .and_then(|value| serde_json::from_value::<CopilotQuotaDetail>(value).ok())
        })
        .ok_or_else(|| {
            QuotaRefreshFailure::bad_request(
                "github_copilot raw snapshot is missing premium_interactions quota",
            )
        })?;
    let plan = usage
        .as_ref()
        .and_then(|usage| usage.copilot_plan.clone())
        .or_else(|| string_at(&snapshot, &["/copilotPlan", "/copilot_plan", "/plan"]));
    let reset = usage
        .as_ref()
        .and_then(|usage| usage.quota_reset_date.clone())
        .or_else(|| {
            string_at(
                &snapshot,
                &["/quotaResetDate", "/quota_reset_date", "/resetAt"],
            )
        });
    let utilization = if premium.unlimited.unwrap_or(false) {
        0.0
    } else if let Some(percent_remaining) = premium.percent_remaining {
        percent_to_fraction(100.0 - percent_remaining)
    } else {
        match (premium.entitlement, premium.remaining) {
            (Some(entitlement), Some(remaining)) if entitlement > 0.0 => {
                ((entitlement - remaining).max(0.0) / entitlement).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    };
    let used = match (premium.entitlement, premium.remaining) {
        (Some(entitlement), Some(remaining)) => Some((entitlement - remaining).max(0.0)),
        _ => None,
    };
    Ok(AccountQuota {
        success: true,
        credential_message: plan.as_deref().map(format_copilot_plan_label),
        tiers: vec![AccountQuotaTier {
            name: "premium".to_string(),
            utilization: Some(utilization),
            used,
            limit: premium.entitlement,
            unit: Some("premium_interactions".to_string()),
            resets_at: reset.as_deref().and_then(dateish_to_unix_ms),
        }],
        extra_usage: Some(json!({
            "raw": snapshot,
            "source": "imported_snapshot",
            "queriedAt": now_ms,
        })),
    })
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
    let usage: KiroImportedUsageLimitsResponse =
        serde_json::from_value(snapshot.clone()).map_err(|error| {
            QuotaRefreshFailure::bad_request(format!(
                "kiro_oauth imported usage limits snapshot is invalid: {error}"
            ))
        })?;
    let current_usage = usage.current_usage();
    let usage_limit = usage.usage_limit();
    let utilization = if usage_limit > 0.0 {
        (current_usage / usage_limit).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Ok(AccountQuota {
        success: true,
        credential_message: usage
            .subscription_title()
            .map(str::to_string)
            .or_else(|| Some("Kiro OAuth".to_string())),
        tiers: vec![AccountQuotaTier {
            name: "kiro_agentic_requests".to_string(),
            utilization: Some(utilization),
            used: Some(current_usage),
            limit: Some(usage_limit),
            unit: Some("credits".to_string()),
            resets_at: usage
                .next_reset_timestamp()
                .and_then(timestamp_number_to_unix_ms),
        }],
        extra_usage: Some(json!({
            "raw": snapshot,
            "source": "imported_snapshot",
            "overageEnabled": usage.overage_enabled(),
            "queriedAt": now_ms,
        })),
    })
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
        "/membershipType",
        "/membership_type",
        "/subscription/planLabel",
        "/plan",
    ];
    let plan = account
        .raw
        .as_ref()
        .and_then(|raw| string_at(raw, &plan_paths))
        .or_else(|| string_at(&snapshot, &plan_paths))
        .map(|value| format_cursor_membership_label(&value))
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
        let used = number_at(plan_usage, &["/used"]).or_else(|| {
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
            utilization: Some(utilization),
            used,
            limit,
            unit,
            resets_at,
        }],
        extra_usage: Some(json!({
            "raw": snapshot,
            "source": "imported_snapshot",
            "queriedAt": now_ms,
        })),
    })
}

#[derive(Debug, Clone, PartialEq)]
struct ClaudeProfileLookup {
    plan_label: Option<String>,
    profile_overlay: Option<Value>,
}

async fn fetch_claude_profile_lookup(
    http: &reqwest::Client,
    access_token: &str,
    request_timeout: Duration,
) -> Option<ClaudeProfileLookup> {
    let response = http
        .get(CLAUDE_PROFILE_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(ACCEPT, "application/json")
        .header("accept-language", "*")
        .header(USER_AGENT, claude_cli_user_agent())
        .header("x-app", "cli")
        .timeout(request_timeout)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.json::<Value>().await.ok()?;
    parse_claude_profile_lookup(&body)
}

fn parse_claude_profile_lookup(body: &Value) -> Option<ClaudeProfileLookup> {
    let organization = body.get("organization")?.as_object()?;
    let organization_type = organization
        .get("organization_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let plan_label = organization_type.map(format_claude_plan_label);
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
    (plan_label.is_some() || !overlay.is_empty()).then(|| ClaudeProfileLookup {
        plan_label,
        profile_overlay: (!overlay.is_empty()).then_some(Value::Object(overlay)),
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

async fn fetch_chatgpt_account_lookup(
    http: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    request_timeout: Duration,
) -> Option<ChatGptSubscriptionLookup> {
    let response = http
        .get(CHATGPT_ACCOUNTS_CHECK_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .header(ACCEPT, "application/json")
        .timeout(request_timeout)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.json::<Value>().await.ok()?;
    parse_chatgpt_accounts_check_lookup(&body, account_id)
}

async fn fetch_chatgpt_subscription_lookup(
    http: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
    request_timeout: Duration,
) -> Option<ChatGptSubscriptionLookup> {
    let account_id = account_id
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let response = http
        .get(CHATGPT_SUBSCRIPTIONS_URL)
        .query(&[("account_id", account_id)])
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .header(ACCEPT, "application/json")
        .timeout(request_timeout)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.json::<Value>().await.ok()?;
    parse_chatgpt_subscription_lookup(&body)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ChatGptSubscriptionLookup {
    plan_type: Option<String>,
    plan_label: Option<String>,
    expires_at: Option<String>,
    expires_source: Option<String>,
    expires_kind: Option<String>,
}

fn parse_chatgpt_accounts_check_lookup(
    body: &Value,
    account_id: Option<&str>,
) -> Option<ChatGptSubscriptionLookup> {
    let accounts = body.get("accounts")?.as_object()?;
    let account_id = account_id.map(str::trim).filter(|value| !value.is_empty());

    if let Some(account_id) = account_id {
        if let Some(account) = accounts.get(account_id) {
            if let Some(lookup) = chatgpt_lookup_from_account(account) {
                return Some(lookup);
            }
        }
        for account in accounts.values() {
            if chatgpt_account_matches_id(account, account_id) {
                if let Some(lookup) = chatgpt_lookup_from_account(account) {
                    return Some(lookup);
                }
            }
        }
    }

    let mut default_candidate = None;
    let mut paid_candidate = None;
    let mut any_candidate = None;
    for account in accounts.values() {
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
    Some(ChatGptSubscriptionLookup {
        plan_type,
        plan_label,
        expires_at,
        expires_source: Some("subscriptions_active_until".to_string()),
        expires_kind: Some("subscription".to_string()),
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
    Some(ChatGptSubscriptionLookup {
        plan_type,
        plan_label,
        expires_at,
        expires_source: Some("accounts_check_entitlement".to_string()),
        expires_kind: Some("subscription".to_string()),
    })
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

fn codex_account_id(account: &Account) -> Option<String> {
    crate::domain::accounts::store::effective_codex_workspace_id(account)
        .or_else(|| {
            account
                .profile
                .as_ref()
                .and_then(|value| string_at(value, CODEX_ACCOUNT_ID_POINTERS))
        })
        .or_else(|| {
            account
                .raw
                .as_ref()
                .and_then(|value| string_at(value, CODEX_ACCOUNT_ID_POINTERS))
        })
}

const CODEX_ACCOUNT_ID_POINTERS: &[&str] = &[
    "/accountId",
    "/account_id",
    "/chatgptAccountId",
    "/chatgpt_account_id",
    "/openai_auth/chatgpt_account_id",
    "/openaiAuth/chatgptAccountId",
    "/verifiedOpenAiClaims/chatgpt_account_id",
    "/verifiedOpenAiClaims/chatgptAccountId",
    "/organizationId",
    "/organization_id",
    "/account/id",
    "/account/account_id",
    "/account/chatgpt_account_id",
    "/account/organization_id",
    "/raw/chatgpt_account_id",
    "/raw/openai_auth/chatgpt_account_id",
];

#[derive(Debug, Deserialize)]
struct CodexUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<CodexRateLimit>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimit {
    primary_window: Option<CodexRateLimitWindow>,
    secondary_window: Option<CodexRateLimitWindow>,
}

#[derive(Debug, Deserialize)]
struct CodexRateLimitWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<i64>,
    reset_after_seconds: Option<i64>,
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
    remaining: Option<f64>,
    #[serde(default, alias = "percentRemaining")]
    percent_remaining: Option<f64>,
    #[serde(default)]
    unlimited: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportedUsageLimitsResponse {
    #[serde(default, alias = "next_date_reset")]
    next_date_reset: Option<f64>,
    #[serde(default, alias = "subscription_info")]
    subscription_info: Option<KiroImportedSubscriptionInfo>,
    #[serde(default, alias = "usage_breakdown_list")]
    usage_breakdown_list: Vec<KiroImportedUsageBreakdown>,
    #[serde(default, alias = "overage_configuration")]
    overage_configuration: Option<KiroImportedOverageConfiguration>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportedSubscriptionInfo {
    #[serde(default, alias = "subscription_title")]
    subscription_title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportedOverageConfiguration {
    #[serde(default, alias = "overage_enabled")]
    overage_enabled: Option<bool>,
    #[serde(default, alias = "overage_status")]
    overage_status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportedUsageBreakdown {
    #[serde(default, alias = "current_usage_with_precision")]
    current_usage_with_precision: f64,
    #[serde(default)]
    bonuses: Vec<KiroImportedBonus>,
    #[serde(default, alias = "free_trial_info")]
    free_trial_info: Option<KiroImportedFreeTrialInfo>,
    #[serde(default, alias = "next_date_reset")]
    next_date_reset: Option<f64>,
    #[serde(default, alias = "usage_limit_with_precision")]
    usage_limit_with_precision: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportedBonus {
    #[serde(default, alias = "current_usage")]
    current_usage: f64,
    #[serde(default, alias = "usage_limit")]
    usage_limit: f64,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KiroImportedFreeTrialInfo {
    #[serde(default, alias = "current_usage_with_precision")]
    current_usage_with_precision: f64,
    #[serde(default, alias = "free_trial_status")]
    free_trial_status: Option<String>,
    #[serde(default, alias = "usage_limit_with_precision")]
    usage_limit_with_precision: f64,
}

impl KiroImportedBonus {
    fn is_active(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|status| status.eq_ignore_ascii_case("ACTIVE"))
    }
}

impl KiroImportedFreeTrialInfo {
    fn is_active(&self) -> bool {
        self.free_trial_status
            .as_deref()
            .is_some_and(|status| status.eq_ignore_ascii_case("ACTIVE"))
    }
}

impl KiroImportedUsageLimitsResponse {
    fn subscription_title(&self) -> Option<&str> {
        self.subscription_info
            .as_ref()
            .and_then(|info| info.subscription_title.as_deref())
    }

    fn overage_enabled(&self) -> Option<bool> {
        let config = self.overage_configuration.as_ref()?;
        config.overage_enabled.or_else(|| {
            config
                .overage_status
                .as_deref()
                .map(|status| status.eq_ignore_ascii_case("ENABLED"))
        })
    }

    fn primary_breakdown(&self) -> Option<&KiroImportedUsageBreakdown> {
        self.usage_breakdown_list.first()
    }

    fn current_usage(&self) -> f64 {
        let Some(breakdown) = self.primary_breakdown() else {
            return 0.0;
        };
        let mut total = breakdown.current_usage_with_precision;
        if let Some(trial) = &breakdown.free_trial_info {
            if trial.is_active() {
                total += trial.current_usage_with_precision;
            }
        }
        for bonus in &breakdown.bonuses {
            if bonus.is_active() {
                total += bonus.current_usage;
            }
        }
        total
    }

    fn usage_limit(&self) -> f64 {
        let Some(breakdown) = self.primary_breakdown() else {
            return 0.0;
        };
        let mut total = breakdown.usage_limit_with_precision;
        if let Some(trial) = &breakdown.free_trial_info {
            if trial.is_active() {
                total += trial.usage_limit_with_precision;
            }
        }
        for bonus in &breakdown.bonuses {
            if bonus.is_active() {
                total += bonus.usage_limit;
            }
        }
        total
    }

    fn next_reset_timestamp(&self) -> Option<f64> {
        self.primary_breakdown()
            .and_then(|breakdown| breakdown.next_date_reset)
            .or(self.next_date_reset)
    }
}

fn format_claude_plan_label(org_type: &str) -> String {
    match org_type {
        "claude_pro" => "Claude Pro".to_string(),
        "claude_max" => "Claude Max".to_string(),
        "claude_free" => "Claude Free".to_string(),
        "claude_team" => "Claude Team".to_string(),
        "claude_enterprise" => "Claude Enterprise".to_string(),
        other => other.to_string(),
    }
}

fn normalize_claude_tier_name(name: &str) -> &str {
    match name {
        "seven_day_omelette" => "seven_day_opus",
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
        "business" | "self_serve_business_usage_based" => "ChatGPT Business".to_string(),
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
        Value::String(value) => Some(value.clone()),
        Value::Object(object) => object
            .get("id")
            .or_else(|| object.get("projectId"))
            .and_then(Value::as_str)
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

fn antigravity_code_assist_metadata() -> Value {
    let platform = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            2
        } else {
            1
        }
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "aarch64") {
            4
        } else {
            3
        }
    } else if cfg!(target_os = "windows") {
        5
    } else {
        0
    };
    json!({ "ideType": 9, "platform": platform, "pluginType": 2 })
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

fn format_cursor_membership_label(membership_type: &str) -> String {
    match membership_type.trim().to_ascii_lowercase().as_str() {
        "free" => "Cursor Free".to_string(),
        "pro" => "Cursor Pro".to_string(),
        "pro_plus" | "pro+" => "Cursor Pro+".to_string(),
        "ultra" => "Cursor Ultra".to_string(),
        other if !other.is_empty() => format!("Cursor {other}"),
        _ => "Cursor".to_string(),
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
    fn claude_usage_windows_parse_known_and_unknown_tiers() {
        let quota = parse_claude_quota(
            &json!({
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
            }),
            Some("Claude Pro".to_string()),
            1_000,
        );

        assert_eq!(quota.credential_message.as_deref(), Some("Claude Pro"));
        assert_eq!(quota.tiers[0].name, "five_hour");
        assert_eq!(quota.tiers[0].utilization, Some(0.25));
        assert_eq!(quota.tiers[1].name, "seven_day_opus");
        assert_eq!(quota.tiers[1].utilization, Some(0.5));
        assert!(quota.tiers.iter().any(|tier| tier.name == "new_window"));
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
    fn copilot_imported_snapshot_parses_premium_quota() {
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

        let quota = parse_copilot_imported_quota(&account, 1_000).unwrap();

        assert_eq!(
            quota.credential_message.as_deref(),
            Some("Copilot Individual")
        );
        assert_eq!(quota.tiers[0].name, "premium");
        assert_eq!(quota.tiers[0].utilization, Some(0.75));
        assert_eq!(quota.tiers[0].used, Some(75.0));
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
        account.profile = Some(json!({"accountId": "workspace-a"}));
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
        account.profile = Some(json!({"accountId": "workspace-b"}));
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
                "chatgpt_account_id": "workspace-profile-default",
                "organizations": [
                    {"id": "workspace-selected", "name": "Selected"}
                ]
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
    fn claude_profile_lookup_keeps_plan_and_billing_source_independent() {
        let lookup = parse_claude_profile_lookup(&json!({
            "organization": {
                "uuid": "org-1",
                "name": "Example",
                "organization_type": "team",
                "rate_limit_tier": "tier-2",
                "billing_type": "apple_subscription"
            }
        }))
        .unwrap();

        assert_eq!(lookup.plan_label.as_deref(), Some("team"));
        let profile = lookup.profile_overlay.unwrap();
        assert_eq!(profile["organizationUUID"], "org-1");
        assert_eq!(profile["organizationName"], "Example");
        assert_eq!(profile["billingSource"], "apple_subscription");
        assert!(profile.get("planType").is_none());
        assert!(profile.get("subscriptionExpiresAt").is_none());

        let unknown = parse_claude_profile_lookup(&json!({
            "organization": {"billing_type": "future_partner"}
        }))
        .unwrap();
        assert_eq!(
            unknown.profile_overlay.unwrap()["billingSource"],
            "future_partner"
        );
    }

    #[test]
    fn grok_user_and_billing_normalize_account_and_credit_metadata() {
        let user = json!({
            "email": "owner@example.com",
            "subscriptionTier": "SuperGrok",
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

        let quota = grok_quota_from_probes(
            &user,
            Some(&billing),
            Some("SuperGrok".to_string()),
            1_000,
            false,
            &[],
            None,
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
            Some("research_pending")
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
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0].name, "grok_monthly");
        assert_eq!(tiers[0].used, Some(275.0));
        assert_eq!(tiers[0].limit, Some(1000.0));
        assert_eq!(tiers[1].name, "grok_on_demand");
        assert_eq!(tiers[1].utilization, Some(0.35));
        assert_eq!(tiers[2].name, "grok_prepaid");
        assert_eq!(tiers[2].limit, Some(12.5));

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
        let quota = grok_quota_from_probes(
            &json!({}),
            Some(&paid_zero_cap),
            Some("XPremiumPlus".to_string()),
            1_000,
            false,
            &[],
            None,
        );
        assert!(quota.tiers.is_empty());
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
        let quota = grok_quota_from_probes(
            &json!({"subscriptionTier": "SuperGrok"}),
            None,
            Some("SuperGrok".to_string()),
            1_000,
            true,
            &[],
            None,
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
    fn grok_billing_failure_preserves_the_previous_credit_tier() {
        let previous = AccountQuotaTier {
            name: "grok_credits".to_string(),
            utilization: Some(0.25),
            used: Some(25.0),
            limit: Some(100.0),
            unit: Some("credits".to_string()),
            resets_at: None,
        };
        let quota = grok_quota_from_probes(
            &json!({"subscriptionTier": "SuperGrok"}),
            None,
            Some("SuperGrok".to_string()),
            1_000,
            false,
            std::slice::from_ref(&previous),
            Some("billing temporarily unavailable"),
        );

        assert!(quota.success);
        assert_eq!(quota.tiers.len(), 1);
        assert_eq!(quota.tiers[0].name, previous.name);
        assert_eq!(quota.tiers[0].utilization, previous.utilization);
        assert_eq!(quota.tiers[0].used, previous.used);
        assert_eq!(quota.tiers[0].limit, previous.limit);
        assert_eq!(
            quota
                .extra_usage
                .as_ref()
                .and_then(|value| value.get("billingError"))
                .and_then(Value::as_str),
            Some("billing temporarily unavailable")
        );
    }

    fn imported_account(provider_type: ProviderType, raw: Value) -> Account {
        Account {
            id: "acct-imported".to_string(),
            provider_type,
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
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        }
    }
}
