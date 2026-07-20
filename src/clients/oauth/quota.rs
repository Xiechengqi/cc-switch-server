use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
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
use crate::domain::grok_cli::{
    GROK_CLI_CLIENT_IDENTIFIER, GROK_CLI_MONTHLY_BILLING_URL, GROK_CLI_TOKEN_AUTH,
    GROK_CLI_USER_AGENT, GROK_CLI_USER_URL, GROK_CLI_VERSION, GROK_CLI_WEEKLY_BILLING_URL,
    GROK_SUBSCRIPTIONS_URL, GROK_TASK_USAGE_URL,
};
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

    let tiers = codex_tiers_from_rate_limit(usage.rate_limit);

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
            let status_code = error.status_code;
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
            .header(USER_AGENT, GROK_CLI_USER_AGENT)
            .header("x-xai-token-auth", GROK_CLI_TOKEN_AUTH)
            .header("x-grok-client-identifier", GROK_CLI_CLIENT_IDENTIFIER)
            .header("x-grok-client-version", GROK_CLI_VERSION)
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
                label: None,
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
        label: None,
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
            label: None,
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
            label: None,
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
            label: None,
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
            label: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatGptProbeStatus {
    Success,
    NotProvided,
    SkippedNoTrustedWorkspace,
    HttpError,
    NetworkError,
    ParseError,
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
    let response = match http
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
    let body = match response.json::<Value>().await {
        Ok(body) => body,
        Err(error) => {
            tracing::debug!(error = %error, "ChatGPT accounts/check response was invalid JSON");
            return ChatGptSubscriptionProbe {
                status: ChatGptProbeStatus::ParseError,
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
    let response = match http
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
    let body = match response.json::<Value>().await {
        Ok(body) => body,
        Err(error) => {
            tracing::debug!(error = %error, "ChatGPT subscriptions response was invalid JSON");
            return ChatGptSubscriptionProbe {
                status: ChatGptProbeStatus::ParseError,
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
    let mut profile = account.profile.clone();
    crate::domain::accounts::store::set_verified_openai_claims(&mut profile, Some(claims));
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
    #[serde(default)]
    allowed: Option<bool>,
    #[serde(default)]
    limit_reached: Option<bool>,
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
            Some("SuperGrok".to_string()),
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
    fn grok_billing_failure_preserves_the_previous_credit_tier() {
        let previous = AccountQuotaTier {
            name: "grok_credits".to_string(),
            label: Some("Credits".to_string()),
            utilization: Some(0.25),
            used: Some(25.0),
            limit: Some(100.0),
            unit: Some("credits".to_string()),
            resets_at: None,
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
