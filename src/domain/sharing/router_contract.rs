use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::accounts::store::{Account, AccountStore};
use crate::domain::health;
use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::sharing::model_health::ShareModelHealthSummary;
use crate::domain::sharing::shares::{Share, ShareMarketGrantStatus};
use crate::domain::usage::store::UsageStore;
use crate::infra::time::now_ms;

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
    #[serde(default)]
    pub is_current: bool,
    #[serde(default)]
    pub enabled: bool,
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
                    for_sale: if share.for_sale { "Yes" } else { "No" }.to_string(),
                    sale_market_kind: share.sale_market_kind.clone(),
                    market_access_mode: market_access_mode.clone(),
                    shared_with_emails: shared_with_emails.clone(),
                    token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
                    parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(3),
                    expires_at: share
                        .expires_at
                        .map(|value| value.to_string())
                        .unwrap_or_default(),
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
        for_sale: if share.for_sale { "Yes" } else { "No" }.to_string(),
        sale_market_kind: share.sale_market_kind.clone(),
        subdomain: share
            .tunnel_subdomain
            .clone()
            .unwrap_or_else(|| share.id.replace('_', "-")),
        app_type: app_key(share.app).to_string(),
        provider_id: Some(share.provider_id.clone()),
        bindings,
        token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
        parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(3),
        tokens_used: share.tokens_used as i64,
        requests_count: share.requests_count as i64,
        share_status: share.status.clone(),
        created_at: now_ms().to_string(),
        expires_at: share
            .expires_at
            .map(|value| value.to_string())
            .unwrap_or_default(),
        support,
        upstream_provider: primary_upstream,
        app_runtimes,
        app_providers,
        app_availability,
        model_health,
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
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    ShareUpstreamProvider {
        kind: provider.provider_type_id.clone(),
        app: app.to_string(),
        provider_name: Some(provider.provider.name.clone()),
        provider_type: Some(provider.provider_type_id.clone()),
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
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
    let account_context = account_context_for_share(provider, share, accounts);
    let quota_blocked = quota_blocked_percent(account_context.quota_percent);
    let available = health
        .as_ref()
        .map(|health| health.healthy && quota_blocked != Some(true));
    ShareAppProvider {
        id: provider.provider.id.clone(),
        name: provider.provider.name.clone(),
        app: app.to_string(),
        kind: Some(provider.provider_type_id.clone()),
        provider_type: Some(provider.provider_type_id.clone()),
        is_current,
        enabled: true,
        account_email: account_context.account_email,
        subscription_level: account_context.subscription_level,
        subscription_expires_at: account_context.subscription_expires_at,
        subscription_remaining_ms: account_context.subscription_remaining_ms,
        quota_percent: account_context.quota_percent,
        quota_blocked,
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
    account
        .quota
        .as_ref()
        .and_then(|quota| quota.extra_usage.as_ref())
        .and_then(subscription_expires_at_from_extra)
}

fn account_subscription_remaining_ms(account: &Account) -> Option<i64> {
    account
        .quota
        .as_ref()
        .and_then(|quota| quota.extra_usage.as_ref())
        .and_then(subscription_remaining_ms_from_extra)
}

fn subscription_expires_at_from_extra(value: &Value) -> Option<String> {
    [
        "/subscriptionPeriodEnd",
        "/subscription/expiresAt",
        "/subscription/expires_at",
        "/raw/SubscriptionPeriodEnd/Time",
        "/raw/subscriptionPeriodEnd/time",
    ]
    .iter()
    .find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn subscription_remaining_ms_from_extra(value: &Value) -> Option<i64> {
    value
        .pointer("/subscriptionRemainingMs")
        .and_then(Value::as_i64)
}

fn provider_models(provider: &StoredProvider) -> Vec<ShareUpstreamModel> {
    let mut models = Vec::new();
    if let Some(upstream_model) = provider
        .provider
        .settings_config
        .pointer("/modelMapping/upstreamModel")
        .and_then(serde_json::Value::as_str)
        .filter(|model| !model.trim().is_empty())
    {
        models.push(ShareUpstreamModel {
            slot: "default".to_string(),
            actual_model: upstream_model.to_string(),
        });
    }
    if let Some(mapping) = provider
        .provider
        .settings_config
        .get("modelMapping")
        .and_then(serde_json::Value::as_object)
    {
        for (slot, value) in mapping {
            if slot == "upstreamModel" {
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
    if let Some(values) = provider
        .provider
        .settings_config
        .get("models")
        .and_then(serde_json::Value::as_array)
    {
        for value in values {
            let model = value.as_str().or_else(|| {
                value
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| value.get("name").and_then(serde_json::Value::as_str))
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

fn provider_api_url(provider: &StoredProvider) -> Option<String> {
    let env = provider.provider.settings_config.get("env");
    [
        "/env/ANTHROPIC_BASE_URL",
        "/env/OPENAI_BASE_URL",
        "/env/GEMINI_BASE_URL",
        "/ANTHROPIC_BASE_URL",
        "/OPENAI_BASE_URL",
        "/GEMINI_BASE_URL",
    ]
    .into_iter()
    .find_map(|pointer| {
        provider
            .provider
            .settings_config
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    })
    .or_else(|| {
        env.and_then(|value| value.get("BASE_URL"))
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn default_market_access_mode() -> String {
    "selected".to_string()
}

fn default_sale_market_kind() -> String {
    "token".to_string()
}

fn default_parallel_limit() -> i64 {
    3
}

fn default_share_for_sale() -> String {
    "No".to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::accounts::store::{AccountQuota, AccountStore};
    use crate::domain::providers::model::{Provider, ProviderType};
    use crate::domain::sharing::shares::{ShareAcl, ShareBinding};
    use crate::domain::usage::store::{UsageLog, UsageLogContext, UsageModelMetadata};

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
            for_sale: false,
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
            scopes: Vec::new(),
            profile: None,
            raw: None,
            subscription_level: Some("ChatGPT Pro 20x".to_string()),
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
            last_refresh_error: None,
        }
    }
}
