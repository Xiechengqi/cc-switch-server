use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::accounts::store::{Account, AccountQuotaTier, AccountStore};
use crate::domain::accounts::subscription_expiry::resolved_subscription_expiry;
use crate::domain::health;
use crate::domain::providers::model::{classify_provider, AppKind, ProviderType};
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::model_health::ShareModelHealthSummary;
use crate::domain::sharing::shares::{share_router_for_sale_label, Share, ShareMarketGrantStatus};
use crate::domain::usage::store::UsageStore;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareSettingsPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sale_market_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_access_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_with_emails: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_by_app: Option<BTreeMap<String, ShareAppAccess>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_settings: Option<BTreeMap<String, ShareAppSettings>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub for_sale_official_price_percent_by_app: Option<BTreeMap<String, u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_start: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_grants: Option<BTreeMap<String, ShareUserGrant>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ShareTokenPeriod {
    #[default]
    Lifetime,
    Day,
    Week,
    CalendarMonth,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub token_period: ShareTokenPeriod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsageBucket {
    #[serde(default)]
    pub started_at_ms: i64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserUsage {
    #[serde(default)]
    pub lifetime: ShareUserUsageBucket,
    #[serde(default)]
    pub day: ShareUserUsageBucket,
    #[serde(default)]
    pub week: ShareUserUsageBucket,
    #[serde(default)]
    pub calendar_month: ShareUserUsageBucket,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUserGrant {
    pub email: String,
    #[serde(default)]
    pub role: String,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub policy: ShareUserPolicy,
    #[serde(default)]
    pub usage: ShareUserUsage,
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub updated_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at_ms: Option<u128>,
    #[serde(default)]
    pub revision: u64,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDescriptor {
    pub share_id: String,
    pub share_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_email: Option<String>,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub access_by_app: BTreeMap<String, ShareAppAccess>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_settings: BTreeMap<String, ShareAppSettings>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub for_sale_official_price_percent_by_app: BTreeMap<String, u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub market_grant: Option<ShareMarketGrantStatus>,
    #[serde(default)]
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    pub subdomain: String,
    pub app_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bindings: BTreeMap<String, String>,
    #[serde(default)]
    pub token_limit: i64,
    #[serde(default = "default_parallel_limit")]
    pub parallel_limit: i64,
    #[serde(default)]
    pub tokens_used: i64,
    #[serde(default)]
    pub requests_count: i64,
    #[serde(default)]
    pub share_status: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: String,
    #[serde(default)]
    pub support: ShareSupport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<ShareUpstreamProvider>,
    #[serde(default)]
    pub app_runtimes: ShareAppRuntimes,
    #[serde(default)]
    pub app_providers: ShareAppProviders,
    #[serde(default)]
    pub app_availability: ShareAppAvailability,
    #[serde(default)]
    pub model_health: ShareModelHealthSummary,
    #[serde(default, skip_serializing_if = "is_false")]
    pub auto_start: bool,
    #[serde(default, skip_serializing_if = "is_zero_revision")]
    pub config_revision: u64,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
}

fn is_zero_revision(value: &u64) -> bool {
    *value == 0
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppAccess {
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppSettings {
    #[serde(default)]
    pub for_sale: String,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    #[serde(default = "default_market_access_mode")]
    pub market_access_mode: String,
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default)]
    pub token_limit: i64,
    #[serde(default = "default_parallel_limit")]
    pub parallel_limit: i64,
    #[serde(default)]
    pub expires_at: String,
}

impl Default for ShareAppSettings {
    fn default() -> Self {
        Self {
            for_sale: default_share_for_sale(),
            sale_market_kind: default_sale_market_kind(),
            market_access_mode: default_market_access_mode(),
            shared_with_emails: Vec::new(),
            token_limit: -1,
            parallel_limit: default_parallel_limit(),
            expires_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSupport {
    #[serde(default)]
    pub claude: bool,
    #[serde(default)]
    pub codex: bool,
    #[serde(default)]
    pub gemini: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuotaTier {
    #[serde(alias = "name")]
    pub label: String,
    pub utilization: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamQuota {
    pub status: String,
    #[serde(
        default,
        alias = "credentialMessage",
        skip_serializing_if = "Option::is_none"
    )]
    pub plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queried_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_period_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_until: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_limit_percent: Option<f64>,
    #[serde(default)]
    pub tiers: Vec<ShareUpstreamQuotaTier>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamProvider {
    pub kind: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_remaining_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ShareProviderHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareUpstreamModel {
    pub slot: String,
    pub actual_model: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppProvider {
    pub id: String,
    pub name: String,
    pub app: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_type: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_current: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub codex_image_generation_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_remaining_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota: Option<ShareUpstreamQuota>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ShareUpstreamModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ShareProviderHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppProviders {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude: Vec<ShareAppProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub codex: Vec<ShareAppProvider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gemini: Vec<ShareAppProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppRuntimes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<ShareUpstreamProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ShareUpstreamProvider>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAppAvailability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<ShareProviderAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<ShareProviderAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gemini: Option<ShareProviderAvailability>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderAvailability {
    pub app: String,
    pub provider_id: String,
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareProviderHealth {
    pub healthy: bool,
    pub requests: u64,
    pub successes: u64,
    pub failures: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success_rate: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_latency_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status_code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_request_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareSyncOperation {
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<ShareDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareRequestLogEntry {
    pub request_id: String,
    pub share_id: String,
    pub share_name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub app_type: String,
    pub model: String,
    pub request_model: String,
    pub request_agent: String,
    pub requested_model: String,
    pub actual_model: String,
    pub actual_model_source: String,
    pub status_code: u16,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_token_ms: Option<u64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    pub is_streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_country_iso3: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    pub created_at: i64,
    #[serde(default)]
    pub is_health_check: bool,
}

pub fn descriptor_for_share(share: &Share, providers: &ProviderStore) -> ShareDescriptor {
    descriptor_for_share_with_usage(share, providers, None)
}

pub fn descriptor_for_share_with_usage(
    share: &Share,
    providers: &ProviderStore,
    usage: Option<&UsageStore>,
) -> ShareDescriptor {
    descriptor_for_share_with_accounts_and_usage(share, providers, None, usage)
}

pub fn descriptor_for_share_with_accounts_and_usage(
    share: &Share,
    providers: &ProviderStore,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
) -> ShareDescriptor {
    let mut bindings = BTreeMap::new();
    if share.bindings.is_empty() {
        bindings.insert(app_key(share.app).to_string(), share.provider_id.clone());
    } else {
        for binding in &share.bindings {
            bindings.insert(
                app_key(binding.app).to_string(),
                binding.provider_id.clone(),
            );
        }
    }

    let mut support = ShareSupport::default();
    for app in bindings.keys() {
        match app.as_str() {
            "claude" => support.claude = true,
            "codex" => support.codex = true,
            "gemini" => support.gemini = true,
            _ => {}
        }
    }

    let shared_with_emails = share.acl.shared_with_emails.clone();
    let market_access_mode = share.acl.market_access_mode.clone().unwrap_or_else(|| {
        if share.acl.public_market_email.is_some() {
            "selected".to_string()
        } else if shared_with_emails.is_empty() {
            "all".to_string()
        } else {
            "selected".to_string()
        }
    });
    let mut access_by_app = BTreeMap::new();
    let mut app_settings = BTreeMap::new();
    for app in bindings.keys() {
        let app_access = share
            .access_by_app
            .get(app)
            .cloned()
            .unwrap_or_else(|| ShareAppAccess {
                shared_with_emails: shared_with_emails.clone(),
                market_access_mode: market_access_mode.clone(),
            });
        access_by_app.insert(app.clone(), app_access);

        let app_setting =
            share
                .app_settings
                .get(app)
                .cloned()
                .unwrap_or_else(|| ShareAppSettings {
                    for_sale: share_router_for_sale_label(share),
                    sale_market_kind: share.sale_market_kind.clone(),
                    market_access_mode: market_access_mode.clone(),
                    shared_with_emails: shared_with_emails.clone(),
                    token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
                    parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(-1),
                    expires_at: share_expires_at_rfc3339(share.expires_at),
                });
        app_settings.insert(app.clone(), app_setting);
    }

    let mut app_runtimes = ShareAppRuntimes::default();
    let mut app_providers = ShareAppProviders::default();
    let mut app_availability = ShareAppAvailability::default();
    let mut primary_upstream = None;
    for (app, provider_id) in &bindings {
        if let Some(provider) = providers
            .providers
            .iter()
            .find(|item| app_key(item.app) == app && item.provider.id == *provider_id)
        {
            let upstream = upstream_provider(app, provider, share, accounts, usage);
            let availability = provider_availability(app, provider, share, accounts, usage);
            if app.as_str() == app_key(share.app) {
                primary_upstream = Some(upstream.clone());
            }
            match app.as_str() {
                "claude" => {
                    app_runtimes.claude = Some(upstream.clone());
                    app_providers
                        .claude
                        .push(app_provider(app, provider, share, accounts, usage, true));
                    app_availability.claude = Some(availability);
                }
                "codex" => {
                    app_runtimes.codex = Some(upstream.clone());
                    app_providers
                        .codex
                        .push(app_provider(app, provider, share, accounts, usage, true));
                    app_availability.codex = Some(availability);
                }
                "gemini" => {
                    app_runtimes.gemini = Some(upstream.clone());
                    app_providers
                        .gemini
                        .push(app_provider(app, provider, share, accounts, usage, true));
                    app_availability.gemini = Some(availability);
                }
                _ => {}
            }
        }
    }
    let model_health =
        crate::domain::sharing::model_health::summary_for_share(share, providers, accounts, usage);

    ShareDescriptor {
        share_id: share.id.clone(),
        share_name: share
            .display_name
            .clone()
            .unwrap_or_else(|| share.id.clone()),
        owner_email: share.owner_email.clone(),
        shared_with_emails,
        market_access_mode,
        access_by_app,
        app_settings,
        for_sale_official_price_percent_by_app: share
            .for_sale_official_price_percent_by_app
            .clone(),
        description: share.description.clone(),
        market_grant: share.market_grant.clone(),
        for_sale: share_router_for_sale_label(share),
        sale_market_kind: share.sale_market_kind.clone(),
        subdomain: share
            .tunnel_subdomain
            .clone()
            .unwrap_or_else(|| share.id.replace('_', "-")),
        app_type: app_key(share.app).to_string(),
        provider_id: Some(share.provider_id.clone()),
        bindings,
        token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
        parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(-1),
        tokens_used: share.tokens_used as i64,
        requests_count: share.requests_count as i64,
        share_status: share.status.clone(),
        created_at: share_created_at_rfc3339(share),
        expires_at: share_expires_at_rfc3339(share.expires_at),
        support,
        upstream_provider: primary_upstream,
        app_runtimes,
        app_providers,
        app_availability,
        model_health,
        auto_start: share.auto_start,
        config_revision: share.config_revision,
        user_grants: share.user_grants.clone(),
    }
}

fn app_key(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "claude",
        AppKind::Codex => "codex",
        AppKind::Gemini => "gemini",
    }
}

fn upstream_provider(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
) -> ShareUpstreamProvider {
    let health = usage.map(|usage| provider_health(provider, usage));
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    let resolved_provider_type = classify_provider(provider.app, &provider.provider);
    let provider_type_id = resolved_provider_type.as_str().to_string();
    ShareUpstreamProvider {
        kind: provider_type_id.clone(),
        app: app.to_string(),
        provider_name: Some(provider.provider.name.clone()),
        provider_type: Some(provider_type_id.clone()),
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
        quota: account.and_then(upstream_quota_from_account),
        api_url: provider_api_url(provider),
        models: provider_models(provider),
        health,
        available,
    }
}

fn app_provider(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
    is_current: bool,
) -> ShareAppProvider {
    let health = usage.map(|usage| provider_health(provider, usage));
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    let resolved_provider_type = classify_provider(provider.app, &provider.provider);
    let provider_type_id = resolved_provider_type.as_str().to_string();
    ShareAppProvider {
        id: provider.provider.id.clone(),
        name: provider.provider.name.clone(),
        app: app.to_string(),
        kind: Some(provider_type_id.clone()),
        provider_type: Some(provider_type_id),
        is_current,
        enabled: true,
        codex_image_generation_enabled: provider
            .provider
            .meta
            .as_ref()
            .and_then(|meta| meta.codex_image_generation_enabled)
            .unwrap_or(false),
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
        quota: account.and_then(upstream_quota_from_account),
        api_url: provider_api_url(provider),
        models: provider_models(provider),
        health,
        available,
    }
}

fn provider_availability(
    app: &str,
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
    usage: Option<&UsageStore>,
) -> ShareProviderAvailability {
    let health = usage.map(|usage| health::provider_health(provider, usage));
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available =
        health.as_ref().map(|health| health.healthy).unwrap_or(true) && quota_blocked != Some(true);
    let reason = if quota_blocked == Some(true) {
        Some("quota blocked".to_string())
    } else {
        health.as_ref().and_then(|health| health.reason.clone())
    };
    ShareProviderAvailability {
        app: app.to_string(),
        provider_id: provider.provider.id.clone(),
        available,
        reason,
        quota_blocked,
        last_status_code: health.as_ref().and_then(|health| health.last_status_code),
        success_rate: health.as_ref().and_then(|health| health.success_rate),
        avg_latency_ms: health.as_ref().and_then(|health| health.avg_latency_ms),
    }
}

fn provider_health(provider: &StoredProvider, usage: &UsageStore) -> ShareProviderHealth {
    let health = health::provider_health(provider, usage);
    ShareProviderHealth {
        healthy: health.healthy,
        requests: health.requests,
        successes: health.successes,
        failures: health.failures,
        success_rate: health.success_rate,
        avg_latency_ms: health.avg_latency_ms,
        last_status_code: health.last_status_code,
        last_request_at_ms: health.last_request_at_ms,
        reason: health.reason,
    }
}

fn quota_blocked_percent(quota_percent: Option<f64>) -> Option<bool> {
    quota_percent.map(|quota_percent| quota_percent >= 100.0)
}

#[derive(Debug, Clone, Default)]
struct ShareAccountContext {
    account_email: Option<String>,
    subscription_level: Option<String>,
    subscription_expires_at: Option<String>,
    subscription_remaining_ms: Option<i64>,
    quota_percent: Option<f64>,
}

fn account_context_for_share(
    provider: &StoredProvider,
    share: &Share,
    accounts: Option<&AccountStore>,
) -> ShareAccountContext {
    let account = accounts.and_then(|accounts| account_for_provider(accounts, provider));
    ShareAccountContext {
        account_email: account
            .and_then(|account| account.email.clone())
            .or_else(|| share.account_email.clone()),
        subscription_level: account
            .and_then(|account| account.subscription_level.clone())
            .or_else(|| share.subscription_level.clone()),
        subscription_expires_at: account.and_then(account_subscription_expires_at),
        subscription_remaining_ms: account.and_then(account_subscription_remaining_ms),
        quota_percent: account
            .and_then(|account| account.quota_percent)
            .or(share.quota_percent),
    }
}

fn account_for_provider<'a>(
    accounts: &'a AccountStore,
    provider: &StoredProvider,
) -> Option<&'a Account> {
    let account_id = provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref());
    accounts.find_for_provider(provider.provider_type, account_id)
}

fn account_subscription_expires_at(account: &Account) -> Option<String> {
    let expires_at_ms = resolved_subscription_expiry(account).expires_at_ms?;
    Utc.timestamp_millis_opt(expires_at_ms)
        .single()
        .map(|value| value.to_rfc3339())
}

fn account_subscription_remaining_ms(account: &Account) -> Option<i64> {
    resolved_subscription_expiry(account)
        .expires_at_ms
        .map(|expires_at_ms| {
            expires_at_ms
                .saturating_sub(crate::infra::time::now_ms().min(i64::MAX as u128) as i64)
                .max(0)
        })
}

fn upstream_quota_from_account(account: &Account) -> Option<ShareUpstreamQuota> {
    let subscription_period_end = account_subscription_expires_at(account);
    let Some(quota) = account.quota.as_ref() else {
        return subscription_period_end.map(|subscription_period_end| ShareUpstreamQuota {
            status: "ok".to_string(),
            plan: account.subscription_level.clone(),
            queried_at: None,
            subscription_period_end: Some(subscription_period_end),
            availability: Some("available".to_string()),
            blocked_until: None,
            blocked_reason: None,
            blocked_scope: None,
            dispatch_limit_percent: None,
            tiers: Vec::new(),
        });
    };
    if quota.tiers.is_empty() && !quota.success && subscription_period_end.is_none() {
        return None;
    }
    let plan = quota
        .credential_message
        .clone()
        .or_else(|| account.subscription_level.clone());
    Some(ShareUpstreamQuota {
        status: if quota.success {
            "ok".to_string()
        } else {
            "failed".to_string()
        },
        plan,
        queried_at: account.quota_refreshed_at,
        subscription_period_end,
        availability: Some("available".to_string()),
        blocked_until: None,
        blocked_reason: None,
        blocked_scope: None,
        dispatch_limit_percent: None,
        tiers: quota
            .tiers
            .iter()
            .map(share_upstream_quota_tier_from_account)
            .collect(),
    })
}

fn share_upstream_quota_tier_from_account(tier: &AccountQuotaTier) -> ShareUpstreamQuotaTier {
    ShareUpstreamQuotaTier {
        label: share_quota_tier_label(&tier.name),
        utilization: utilization_percent_for_router_share(tier.utilization),
        resets_at: tier.resets_at.and_then(unix_ms_to_rfc3339),
        used: tier.used,
        limit: tier.limit,
        unit: tier.unit.clone(),
    }
}

fn share_quota_tier_label(name: &str) -> String {
    match name {
        "five_hour" => "5h".to_string(),
        "seven_day" => "1w".to_string(),
        "30_day" => "30d".to_string(),
        "seven_day_opus" => "7d Opus".to_string(),
        "seven_day_omelette" => "7d Opus".to_string(),
        "seven_day_sonnet" => "7d Sonnet".to_string(),
        "premium" => "premium".to_string(),
        "kiro_agentic_requests" => "Kiro".to_string(),
        other => other.replace('_', " "),
    }
}

fn utilization_percent_for_router_share(value: Option<f64>) -> f64 {
    let Some(value) = value else {
        return 0.0;
    };
    if !value.is_finite() {
        return 0.0;
    }
    if value <= 1.0 {
        (value * 100.0).clamp(0.0, 100.0)
    } else {
        value.clamp(0.0, 100.0)
    }
}

fn unix_ms_to_rfc3339(ms: i64) -> Option<String> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|value| value.to_rfc3339())
}

const UNLIMITED_SHARE_EXPIRES_AT: &str = "2099-12-31T23:59:59Z";

fn share_timestamp_to_rfc3339(value: i64) -> Option<String> {
    if value <= 0 {
        return None;
    }
    let ms = if value < 10_000_000_000 {
        value.saturating_mul(1000)
    } else {
        value
    };
    unix_ms_to_rfc3339(ms)
}

pub(crate) fn share_expires_at_rfc3339(expires_at: Option<i64>) -> String {
    expires_at
        .and_then(share_timestamp_to_rfc3339)
        .unwrap_or_else(|| UNLIMITED_SHARE_EXPIRES_AT.to_string())
}

fn share_created_at_rfc3339(share: &Share) -> String {
    if share.created_at_ms > 0 {
        return unix_ms_to_rfc3339(share.created_at_ms as i64)
            .unwrap_or_else(|| Utc::now().to_rfc3339());
    }
    share
        .binding_history
        .iter()
        .map(|entry| entry.changed_at_ms)
        .min()
        .and_then(|value| unix_ms_to_rfc3339(value as i64))
        .unwrap_or_else(|| Utc::now().to_rfc3339())
}

fn provider_models(provider: &StoredProvider) -> Vec<ShareUpstreamModel> {
    let settings = &provider.provider.settings_config;
    if let Some(model) = single_upstream_model_from_settings(settings) {
        return vec![ShareUpstreamModel {
            slot: "model".to_string(),
            actual_model: model,
        }];
    }

    if provider.app == AppKind::Codex {
        if let Some(model) = codex_model_from_settings(settings) {
            return vec![ShareUpstreamModel {
                slot: "model".to_string(),
                actual_model: model,
            }];
        }
    }

    let mut models = Vec::new();
    if let Some(mapping) = settings.get("modelMapping").and_then(Value::as_object) {
        for (slot, value) in mapping {
            if is_model_mapping_metadata_key(slot) {
                continue;
            }
            if let Some(actual_model) = value.as_str().filter(|model| !model.trim().is_empty()) {
                models.push(ShareUpstreamModel {
                    slot: slot.clone(),
                    actual_model: actual_model.to_string(),
                });
            }
        }
    }
    if let Some(values) = settings.get("models").and_then(Value::as_array) {
        for value in values {
            let model = value.as_str().or_else(|| {
                value
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| value.get("name").and_then(Value::as_str))
            });
            if let Some(model) = model.filter(|model| !model.trim().is_empty()) {
                models.push(ShareUpstreamModel {
                    slot: "available".to_string(),
                    actual_model: model.to_string(),
                });
            }
        }
    }
    models
}

fn is_model_mapping_metadata_key(key: &str) -> bool {
    matches!(
        key,
        "mode" | "type" | "upstreamModel" | "upstream_model" | "model"
    )
}

fn single_upstream_model_from_settings(settings: &Value) -> Option<String> {
    let mapping = settings.get("modelMapping")?;
    let mode = mapping
        .get("mode")
        .or_else(|| mapping.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mode != "single" {
        return None;
    }
    mapping
        .get("upstreamModel")
        .or_else(|| mapping.get("upstream_model"))
        .or_else(|| mapping.get("model"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn codex_model_from_settings(settings: &Value) -> Option<String> {
    settings
        .get("model")
        .and_then(Value::as_str)
        .or_else(|| settings.pointer("/config/model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .or_else(|| {
            settings
                .get("config")
                .and_then(Value::as_str)
                .and_then(extract_codex_toml_model)
                .map(str::to_string)
        })
}

fn extract_codex_toml_model(config: &str) -> Option<&str> {
    for line in config.lines() {
        let trimmed = line.split('#').next().unwrap_or(line).trim();
        for marker in ["model = \"", "model = '"] {
            let Some(rest) = trimmed.strip_prefix(marker) else {
                continue;
            };
            let quote = marker.chars().last()?;
            let Some(end) = rest.find(quote) else {
                continue;
            };
            let value = rest[..end].trim();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

fn extract_codex_toml_base_url(config: &str) -> Option<&str> {
    for marker in ["base_url = \"", "base_url = '"] {
        let Some(start) = config.find(marker) else {
            continue;
        };
        let quote = marker.chars().last()?;
        let rest = &config[start + marker.len()..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        let value = rest[..end].trim();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn normalize_api_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn provider_api_url(provider: &StoredProvider) -> Option<String> {
    let settings = &provider.provider.settings_config;
    let env = settings.get("env");
    let app_env_keys: &[&str] = match provider.app {
        AppKind::Claude => &["ANTHROPIC_BASE_URL", "BASE_URL"],
        AppKind::Codex => &["OPENAI_BASE_URL", "CODEX_BASE_URL", "BASE_URL", "base_url"],
        AppKind::Gemini => &["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "BASE_URL"],
    };
    for key in app_env_keys {
        if let Some(url) = settings
            .pointer(&format!("/env/{key}"))
            .and_then(Value::as_str)
            .or_else(|| settings.get(*key).and_then(Value::as_str))
        {
            if let Some(url) = normalize_api_url(url) {
                return Some(url);
            }
        }
    }
    if let Some(url) = [
        "/env/ANTHROPIC_BASE_URL",
        "/env/OPENAI_BASE_URL",
        "/env/GOOGLE_GEMINI_BASE_URL",
        "/env/GEMINI_BASE_URL",
    ]
    .into_iter()
    .find_map(|pointer| settings.pointer(pointer).and_then(Value::as_str))
    .and_then(normalize_api_url)
    {
        return Some(url);
    }
    if provider.app == AppKind::Codex {
        if let Some(url) = settings
            .get("config")
            .and_then(Value::as_str)
            .and_then(extract_codex_toml_base_url)
            .and_then(normalize_api_url)
        {
            return Some(url);
        }
    }
    env.and_then(|value| value.get("BASE_URL"))
        .and_then(Value::as_str)
        .and_then(normalize_api_url)
        .or_else(|| provider_type_default_api_url(provider.provider_type))
}

fn provider_type_default_api_url(provider_type: ProviderType) -> Option<String> {
    let url = match provider_type {
        ProviderType::Nvidia => "https://integrate.api.nvidia.com/v1",
        ProviderType::DeepSeekApi => "https://api.deepseek.com",
        ProviderType::OpenRouter => "https://openrouter.ai/api",
        ProviderType::OllamaCloud => "https://ollama.com",
        _ => return None,
    };
    Some(url.to_string())
}

fn default_market_access_mode() -> String {
    "selected".to_string()
}

fn default_sale_market_kind() -> String {
    "token".to_string()
}

fn default_parallel_limit() -> i64 {
    -1
}

fn default_share_for_sale() -> String {
    "No".to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::{AccountQuota, AccountQuotaTier, AccountStore};
    use crate::domain::providers::model::{Provider, ProviderType};
    use crate::domain::sharing::shares::{ShareAcl, ShareBinding};
    use crate::domain::usage::store::{UsageLog, UsageLogContext, UsageModelMetadata};

    #[test]
    fn descriptor_maps_free_for_sale_label() {
        let mut share = test_share(ProviderType::OllamaCloud, None);
        share.free_access = true;
        share.for_sale = false;
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        assert_eq!(descriptor.for_sale, "Free");
        for settings in descriptor.app_settings.values() {
            assert_eq!(settings.for_sale, "Free");
        }
    }

    #[test]
    fn descriptor_maps_unlimited_expiry_to_shared_permanent_constant() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        assert_eq!(descriptor.expires_at, UNLIMITED_SHARE_EXPIRES_AT);
    }

    #[test]
    fn descriptor_maps_unlimited_parallel_limit_to_negative_one() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);

        assert_eq!(descriptor.parallel_limit, -1);
        for settings in descriptor.app_settings.values() {
            assert_eq!(settings.parallel_limit, -1);
        }
    }

    #[test]
    fn descriptor_omits_quota_percent_when_share_has_no_percent() {
        let share = test_share(ProviderType::OllamaCloud, None);
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::OllamaCloud)],
        };
        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let value = serde_json::to_value(&descriptor).unwrap();
        let provider = &value["appProviders"]["codex"][0];

        assert_eq!(provider["accountEmail"], "owner@example.com");
        assert_eq!(provider["subscriptionLevel"], "pro");
        assert!(provider.get("quotaPercent").is_none());
        assert!(provider.get("quotaBlocked").is_none());
    }

    #[test]
    fn descriptor_uses_account_quota_over_manual_share_fields() {
        let mut share = test_share(ProviderType::CodexOAuth, Some(5.0));
        share.account_email = Some("share-owner@example.com".to_string());
        share.subscription_level = Some("manual".to_string());
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::CodexOAuth)],
        };
        let accounts = AccountStore {
            accounts: vec![test_account(ProviderType::CodexOAuth)],
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(
            provider.account_email.as_deref(),
            Some("account@example.com")
        );
        assert_eq!(
            provider.subscription_level.as_deref(),
            Some("ChatGPT Pro 20x")
        );
        assert_eq!(
            provider.subscription_expires_at.as_deref(),
            Some("2026-07-25T04:49:24+00:00")
        );
        assert_eq!(provider.quota_percent, Some(42.0));
        assert_eq!(provider.quota_blocked, Some(false));
    }

    #[test]
    fn descriptor_uses_manual_account_billing_expiry_without_changing_share_expiry() {
        let share = test_share(ProviderType::ClaudeOAuth, Some(5.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::ClaudeOAuth)],
        };
        let mut account = test_account(ProviderType::ClaudeOAuth);
        account.quota = None;
        account.manual_subscription_expires_at_ms = Some(1_786_924_800_000);
        account.manual_subscription_expiry_updated_at_ms = Some(1_784_000_000_000);
        let accounts = AccountStore {
            accounts: vec![account],
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(
            provider.subscription_expires_at.as_deref(),
            Some("2026-08-17T00:00:00+00:00")
        );
        assert!(provider.subscription_remaining_ms.is_some());
        assert_eq!(provider.quota.as_ref().unwrap().status, "ok");
        assert_eq!(
            provider
                .quota
                .as_ref()
                .and_then(|quota| quota.subscription_period_end.as_deref()),
            Some("2026-08-17T00:00:00+00:00")
        );
        assert_eq!(descriptor.expires_at, UNLIMITED_SHARE_EXPIRES_AT);
    }

    #[test]
    fn descriptor_derives_recurring_account_expiry_for_router_metadata() {
        use crate::domain::accounts::subscription_expiry::{
            resolved_subscription_expiry, SubscriptionExpiryCadence, SubscriptionExpiryRuleDraft,
        };

        let share = test_share(ProviderType::ClaudeOAuth, Some(5.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::ClaudeOAuth)],
        };
        let mut account = test_account(ProviderType::ClaudeOAuth);
        account.quota = None;
        account.manual_subscription_expiry_rule = Some(
            SubscriptionExpiryRuleDraft {
                cadence: SubscriptionExpiryCadence::Monthly,
                month: None,
                day: 10,
                time_zone: "Asia/Shanghai".to_string(),
            }
            .into_rule(1_784_000_000_000)
            .unwrap(),
        );
        let expected = Utc
            .timestamp_millis_opt(
                resolved_subscription_expiry(&account)
                    .expires_at_ms
                    .unwrap(),
            )
            .single()
            .unwrap()
            .to_rfc3339();
        let accounts = AccountStore {
            accounts: vec![account],
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert_eq!(
            provider.subscription_expires_at.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            provider
                .quota
                .as_ref()
                .and_then(|quota| quota.subscription_period_end.as_deref()),
            Some(expected.as_str())
        );
        assert_eq!(descriptor.expires_at, UNLIMITED_SHARE_EXPIRES_AT);
    }

    #[test]
    fn descriptor_maps_codex_quota_tiers_for_router_share_card() {
        let share = test_share(ProviderType::CodexOAuth, Some(1.0));
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::CodexOAuth)],
        };
        let mut account = test_account(ProviderType::CodexOAuth);
        account.quota = Some(AccountQuota {
            success: true,
            credential_message: Some("ChatGPT Plus".to_string()),
            tiers: vec![AccountQuotaTier {
                name: "five_hour".to_string(),
                utilization: Some(0.01),
                resets_at: Some(1_700_000_000_000),
                ..Default::default()
            }],
            extra_usage: None,
        });
        let accounts = AccountStore {
            accounts: vec![account],
        };

        let descriptor =
            descriptor_for_share_with_accounts_and_usage(&share, &providers, Some(&accounts), None);
        let runtime = descriptor.app_runtimes.codex.expect("codex runtime");
        let quota = runtime.quota.expect("quota payload");

        assert_eq!(quota.tiers[0].label, "5h");
        assert_eq!(quota.tiers[0].utilization, 1.0);
    }

    #[test]
    fn descriptor_includes_market_grant_when_present() {
        let mut share = test_share(ProviderType::Codex, Some(42.0));
        share.market_grant = Some(ShareMarketGrantStatus {
            status: "applied".to_string(),
            grant_id: Some("grant-1".to_string()),
            last_error: None,
            updated_at_ms: Some(123),
        });
        let providers = ProviderStore {
            providers: vec![test_provider(ProviderType::Codex)],
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let value = serde_json::to_value(&descriptor).unwrap();

        assert_eq!(value["marketGrant"]["status"], "applied");
        assert_eq!(value["marketGrant"]["grantId"], "grant-1");
    }

    #[test]
    fn descriptor_maps_recent_provider_failure_to_availability() {
        let share = test_share(ProviderType::Codex, Some(42.0));
        let provider = test_provider(ProviderType::Codex);
        let mut log = UsageLog::new(
            AppKind::Codex,
            provider.provider.id.clone(),
            provider.provider.name.clone(),
            ProviderType::Codex,
            500,
            250,
            UsageModelMetadata::default(),
            Default::default(),
        );
        log.created_at_ms = crate::infra::time::now_ms();
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let availability = descriptor.app_availability.codex.unwrap();
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert!(!availability.available);
        assert_eq!(availability.last_status_code, Some(500));
        assert_eq!(provider.quota_percent, Some(42.0));
        assert_eq!(provider.health.as_ref().unwrap().failures, 1);
    }

    #[test]
    fn descriptor_includes_share_model_health_from_health_check_usage() {
        let share = test_share(ProviderType::Codex, Some(42.0));
        let provider = test_provider(ProviderType::Codex);
        let mut log = UsageLog::new(
            AppKind::Codex,
            provider.provider.id.clone(),
            provider.provider.name.clone(),
            ProviderType::Codex,
            200,
            250,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                requested_model: Some("gpt-5.5".to_string()),
                actual_model: None,
                actual_model_source: None,
                pricing_model: None,
            },
            Default::default(),
        );
        log.apply_context(UsageLogContext {
            share_id: Some(share.id.clone()),
            share_name: share.display_name.clone(),
            is_health_check: true,
            is_streaming: true,
            stream_status: Some("completed".to_string()),
            ..UsageLogContext::default()
        });
        let usage = UsageStore {
            logs: vec![log],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider],
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let result = descriptor.model_health.codex.first().unwrap();

        assert_eq!(result.requested_model, "gpt-5.5");
        assert_eq!(result.actual_model, "glm-5.2");
        assert_eq!(result.status, "success");
        assert_eq!(result.source, "cc-switch-health-check");
    }

    #[test]
    fn descriptor_marks_quota_blocked_without_confusing_missing_percent() {
        let share = test_share(ProviderType::Codex, Some(100.0));
        let provider = test_provider(ProviderType::Codex);
        let providers = ProviderStore {
            providers: vec![provider],
        };
        let usage = UsageStore::default();

        let descriptor = descriptor_for_share_with_usage(&share, &providers, Some(&usage));
        let availability = descriptor.app_availability.codex.unwrap();
        let provider = descriptor.app_providers.codex.first().unwrap();

        assert!(!availability.available);
        assert_eq!(availability.quota_blocked, Some(true));
        assert_eq!(provider.quota_percent, Some(100.0));
        assert_eq!(provider.quota_blocked, Some(true));
    }

    #[test]
    fn descriptor_maps_nvidia_codex_api_url_and_single_model() {
        let share = test_share(ProviderType::Nvidia, None);
        let providers = ProviderStore {
            providers: vec![StoredProvider {
                app: AppKind::Codex,
                provider: Provider {
                    id: "p1".to_string(),
                    name: "Nvidia".to_string(),
                    settings_config: json!({
                        "config": "model_provider = \"custom\"\nmodel = \"moonshotai/kimi-k2.5\"\n\n[model_providers.custom]\nname = \"nvidia\"\nbase_url = \"https://integrate.api.nvidia.com/v1\"\n",
                        "modelMapping": {
                            "mode": "single",
                            "upstreamModel": "moonshotai/kimi-k2.5"
                        }
                    }),
                    category: None,
                    meta: None,
                    extra: Default::default(),
                },
                provider_type: ProviderType::Nvidia,
                provider_type_id: ProviderType::Nvidia.as_str().to_string(),
            }],
        };

        let descriptor = descriptor_for_share_with_usage(&share, &providers, None);
        let provider = descriptor.app_providers.codex.first().unwrap();
        let runtime = descriptor.app_runtimes.codex.as_ref().unwrap();

        assert_eq!(
            provider.api_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert_eq!(
            runtime.api_url.as_deref(),
            Some("https://integrate.api.nvidia.com/v1")
        );
        assert_eq!(provider.models.len(), 1);
        assert_eq!(provider.models[0].actual_model, "moonshotai/kimi-k2.5");
        assert_eq!(runtime.models.len(), 1);
        assert_eq!(runtime.models[0].actual_model, "moonshotai/kimi-k2.5");
    }

    fn test_provider(provider_type: ProviderType) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "p1".to_string(),
                name: "provider 1".to_string(),
                settings_config: json!({
                    "env": {
                        "OPENAI_BASE_URL": "https://upstream.example/v1"
                    },
                    "modelMapping": {
                        "upstreamModel": "glm-5.2",
                        "gpt-5.5": "glm-5.2"
                    },
                    "models": ["glm-5.2"]
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
        }
    }

    fn test_share(provider_type: ProviderType, quota_percent: Option<f64>) -> Share {
        Share {
            id: "share-1".to_string(),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: "p1".to_string(),
            provider_type,
            display_name: Some("codex share".to_string()),
            enabled: true,
            status: "active".to_string(),
            subscription_level: Some("pro".to_string()),
            account_email: Some("owner@example.com".to_string()),
            quota_percent,
            tunnel_subdomain: Some("codex-share".to_string()),
            acl: ShareAcl::default(),
            token_limit: None,
            parallel_limit: None,
            tokens_used: 0,
            requests_count: 0,
            expires_at: None,
            created_at_ms: 0,
            for_sale: false,
            free_access: false,
            sale_market_kind: "token".to_string(),
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            auto_start: false,
            description: None,
            bindings: vec![ShareBinding {
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type,
            }],
            binding_history: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
            last_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
            config_revision: 0,
            router_synced_revision: 0,
            user_grants: BTreeMap::new(),
        }
    }

    fn test_account(provider_type: ProviderType) -> Account {
        Account {
            id: "acct-1".to_string(),
            provider_type,
            email: Some("account@example.com".to_string()),
            access_token: Some("access".to_string()),
            refresh_token: None,
            id_token: None,
            token_type: Some("Bearer".to_string()),
            api_key: None,
            extra_headers: Default::default(),
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: Some("ChatGPT Pro 20x".to_string()),
            entitlement_status: None,
            quota_percent: Some(42.0),
            quota: Some(AccountQuota {
                success: true,
                credential_message: Some("ChatGPT Pro 20x".to_string()),
                tiers: Vec::new(),
                extra_usage: Some(json!({
                    "subscription": {
                        "expiresAt": "2026-07-25T04:49:24+00:00"
                    }
                })),
            }),
            quota_refreshed_at: Some(1_000),
            quota_next_refresh_at: Some(2_000),
            expires_at: None,
            manual_subscription_expires_at_ms: None,
            manual_subscription_expiry_updated_at_ms: None,
            manual_subscription_expiry_rule: None,
            rate_limited_until: None,
            last_refresh_error: None,
            refresh_consecutive_failures: 0,
            needs_relogin: false,
        }
    }
}
