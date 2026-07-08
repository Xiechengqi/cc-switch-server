use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::accounts::store::{
    Account, AccountQuota, AccountQuotaTier, AccountRefreshUpdate,
};
use crate::domain::providers::model::ProviderType;

pub const QUOTA_FAILURE_COOLDOWN_MS: i64 = 2 * 60 * 1000;

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CHATGPT_ACCOUNTS_CHECK_URL: &str =
    "https://chatgpt.com/backend-api/accounts/check/v4-2023-04-27";
const CHATGPT_SUBSCRIPTIONS_URL: &str = "https://chatgpt.com/backend-api/subscriptions";
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
            429 => 429,
            _ => 502,
        };
        let retryable = !matches!(upstream_status.as_u16(), 401 | 403);
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
                truncate(&body, 240)
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

    let update = match account.provider_type {
        ProviderType::CodexOAuth => {
            refresh_codex_quota(http, account, now_ms, success_cooldown_ms).await?
        }
        ProviderType::ClaudeOAuth => {
            refresh_claude_quota(http, account, now_ms, success_cooldown_ms).await?
        }
        ProviderType::GeminiCli => {
            refresh_gemini_quota(http, account, now_ms, success_cooldown_ms).await?
        }
        ProviderType::AntigravityOAuth | ProviderType::AgyOAuth => {
            refresh_antigravity_quota(http, account, now_ms, success_cooldown_ms).await?
        }
        ProviderType::GitHubCopilot
        | ProviderType::KiroOAuth
        | ProviderType::CursorOAuth
        | ProviderType::CursorApiKey => {
            refresh_imported_snapshot_quota(account, now_ms, success_cooldown_ms)?
        }
        ProviderType::OllamaCloud => {
            refresh_ollama_cloud_quota(http, account, now_ms, success_cooldown_ms).await?
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
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let account_id = codex_account_id(account);
    let mut request = http
        .get(CHATGPT_USAGE_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(USER_AGENT, "codex-cli")
        .header(ACCEPT, "application/json")
        .timeout(Duration::from_secs(15));
    if let Some(account_id) = account_id.as_deref() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    let body = request_json(account.provider_type, request, now_ms).await?;
    let usage: CodexUsageResponse = serde_json::from_value(body.clone())
        .map_err(|error| QuotaRefreshFailure::parse(account.provider_type, error, now_ms))?;

    let usage_plan_type = usage
        .plan_type
        .as_deref()
        .map(normalize_chatgpt_plan_type)
        .filter(|value| !value.is_empty());
    let usage_plan_label = usage_plan_type.as_deref().map(format_chatgpt_plan_label);
    let account_lookup =
        fetch_chatgpt_account_lookup(http, access_token, account_id.as_deref()).await;
    let subscription_lookup =
        fetch_chatgpt_subscription_lookup(http, access_token, account_id.as_deref()).await;
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
            "bankedReset": codex_banked_reset_status_from_account(account, now_ms),
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
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let usage_request = http
        .get(CLAUDE_USAGE_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(ACCEPT, "application/json")
        .header("accept-language", "*")
        .header(USER_AGENT, "claude-cli/2.1.2 (external, cli)")
        .header("x-app", "cli")
        .timeout(Duration::from_secs(10));
    let (body, plan_label) = tokio::join!(
        request_json(account.provider_type, usage_request, now_ms),
        fetch_claude_plan_label(http, access_token),
    );
    let body = body?;
    let quota = parse_claude_quota(&body, plan_label, now_ms);
    let subscription_level = quota.credential_message.clone();
    Ok(update_from_quota(
        quota,
        subscription_level,
        None,
        now_ms,
        success_cooldown_ms,
    ))
}

async fn refresh_gemini_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
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
        .timeout(Duration::from_secs(15));
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
        .timeout(Duration::from_secs(15));
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

async fn refresh_antigravity_quota(
    http: &reqwest::Client,
    account: &Account,
    now_ms: i64,
    success_cooldown_ms: i64,
) -> Result<AccountRefreshUpdate, QuotaRefreshFailure> {
    let access_token = required_access_token(account)?;
    let metadata = antigravity_code_assist_metadata();
    let load_request = http
        .post(GEMINI_LOAD_CODE_ASSIST_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header(CONTENT_TYPE, "application/json")
        .header("client-metadata", metadata.to_string())
        .json(&json!({ "metadata": metadata }))
        .timeout(Duration::from_secs(15));
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
        .timeout(Duration::from_secs(15));
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
        .timeout(Duration::from_secs(15));
    let body = request_json(account.provider_type, request, now_ms).await?;
    Ok(parse_ollama_me_update(&body, now_ms, success_cooldown_ms))
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

async fn fetch_claude_plan_label(http: &reqwest::Client, access_token: &str) -> Option<String> {
    let response = http
        .get(CLAUDE_PROFILE_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("anthropic-beta", "oauth-2025-04-20")
        .header(ACCEPT, "application/json")
        .header("accept-language", "*")
        .header(USER_AGENT, "claude-cli/2.1.2 (external, cli)")
        .header("x-app", "cli")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.json::<Value>().await.ok()?;
    body.pointer("/organization/organization_type")
        .and_then(Value::as_str)
        .map(format_claude_plan_label)
}

async fn fetch_chatgpt_account_lookup(
    http: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
) -> Option<ChatGptSubscriptionLookup> {
    let response = http
        .get(CHATGPT_ACCOUNTS_CHECK_URL)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("Origin", "https://chatgpt.com")
        .header("Referer", "https://chatgpt.com/")
        .header(ACCEPT, "application/json")
        .timeout(Duration::from_secs(15))
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
        .timeout(Duration::from_secs(15))
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
    account
        .profile
        .as_ref()
        .and_then(|value| string_at(value, CODEX_ACCOUNT_ID_POINTERS))
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
    "/organizationId",
    "/organization_id",
    "/account/id",
    "/account/account_id",
    "/account/chatgpt_account_id",
    "/account/organization_id",
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

fn codex_banked_reset_status_from_account(account: &Account, now_ms: i64) -> Option<Value> {
    let source = account
        .raw
        .as_ref()
        .and_then(|raw| {
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
        })
        .or_else(|| {
            account.quota.as_ref().and_then(|quota| {
                quota
                    .extra_usage
                    .as_ref()
                    .and_then(|extra| value_at(extra, &["/bankedReset", "/codexBankedReset"]))
            })
        })?;
    Some(normalize_codex_banked_reset(source, now_ms))
}

fn normalize_codex_banked_reset(source: Value, now_ms: i64) -> Value {
    let credits = value_at(
        &source,
        &["/credits", "/remainingCredits", "/remaining_credits"],
    )
    .and_then(|value| value.as_array().cloned())
    .or_else(|| source.as_array().cloned())
    .unwrap_or_default();
    let available_count = number_at(
        &source,
        &["/availableCount", "/available_count", "/available"],
    )
    .map(|value| value as i64)
    .unwrap_or_else(|| {
        credits
            .iter()
            .filter(|credit| {
                string_at(credit, &["/status"])
                    .as_deref()
                    .is_some_and(|status| status.eq_ignore_ascii_case("available"))
            })
            .count() as i64
    });
    let next_expires_at = credits
        .iter()
        .filter(|credit| {
            string_at(credit, &["/status"])
                .as_deref()
                .map(|status| status.eq_ignore_ascii_case("available"))
                .unwrap_or(true)
        })
        .filter_map(|credit| string_at(credit, &["/expiresAt", "/expires_at"]))
        .filter_map(|value| dateish_to_unix_ms(&value).map(|ms| (ms, value)))
        .min_by_key(|(ms, _)| *ms)
        .map(|(_, value)| value);
    json!({
        "readOnly": true,
        "source": "imported_snapshot",
        "availableCount": available_count,
        "nextExpiresAt": next_expires_at,
        "credits": credits,
        "raw": source,
        "queriedAt": now_ms,
    })
}

pub fn codex_banked_reset_status_snapshot(account: &Account, now_ms: i64) -> Value {
    codex_banked_reset_status_from_account(account, now_ms).unwrap_or_else(|| {
        json!({
            "enabled": false,
            "readOnly": true,
            "source": "imported_snapshot",
            "availableCount": 0,
            "credits": [],
            "queriedAt": now_ms,
        })
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

        let status = codex_banked_reset_status_from_account(&account, 1_000).unwrap();

        assert_eq!(
            status.get("availableCount").and_then(Value::as_i64),
            Some(1)
        );
        assert_eq!(
            status.get("nextExpiresAt").and_then(Value::as_str),
            Some("2026-07-10T00:00:00Z")
        );
        assert_eq!(status.get("readOnly").and_then(Value::as_bool), Some(true));
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
            scopes: Vec::new(),
            profile: None,
            raw: Some(raw),
            subscription_level: None,
            quota_percent: None,
            quota: None,
            quota_refreshed_at: None,
            quota_next_refresh_at: None,
            expires_at: None,
            last_refresh_error: None,
        }
    }
}
