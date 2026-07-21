use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::Context;
use chrono::{Datelike, TimeZone, Utc, Weekday};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::ProviderStore;
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, share_expires_at_rfc3339, ShareAppAccess,
    ShareAppSettings, ShareSettingsPatch, ShareTokenPeriod, ShareUserGrant, ShareUserPolicy,
    ShareUserUsage, ShareUserUsageBucket,
};
use crate::domain::usage::store::UsageStore;
use crate::infra::time::now_ms;

const SHARES_FILE_NAME: &str = "shares.json";

impl ShareUserUsage {
    pub fn tokens_for(&self, period: ShareTokenPeriod, now_ms: i64) -> u64 {
        match period {
            ShareTokenPeriod::Lifetime => self.lifetime.tokens_used,
            ShareTokenPeriod::Day => current_bucket_tokens(&self.day, utc_day_start_ms(now_ms)),
            ShareTokenPeriod::Week => current_bucket_tokens(&self.week, utc_week_start_ms(now_ms)),
            ShareTokenPeriod::CalendarMonth => {
                current_bucket_tokens(&self.calendar_month, utc_calendar_month_start_ms(now_ms))
            }
        }
    }

    fn record(&mut self, tokens: u64, now_ms: i64) {
        record_bucket(&mut self.lifetime, 0, tokens);
        record_bucket(&mut self.day, utc_day_start_ms(now_ms), tokens);
        record_bucket(&mut self.week, utc_week_start_ms(now_ms), tokens);
        record_bucket(
            &mut self.calendar_month,
            utc_calendar_month_start_ms(now_ms),
            tokens,
        );
    }
}

fn current_bucket_tokens(bucket: &ShareUserUsageBucket, expected_start_ms: i64) -> u64 {
    if bucket.started_at_ms == expected_start_ms {
        bucket.tokens_used
    } else {
        0
    }
}

fn record_bucket(bucket: &mut ShareUserUsageBucket, start_ms: i64, tokens: u64) {
    if bucket.started_at_ms != start_ms {
        *bucket = ShareUserUsageBucket {
            started_at_ms: start_ms,
            ..ShareUserUsageBucket::default()
        };
    }
    bucket.tokens_used = bucket.tokens_used.saturating_add(tokens);
    bucket.requests_count = bucket.requests_count.saturating_add(1);
}

fn utc_datetime(now_ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(now_ms)
        .single()
        .unwrap_or_else(Utc::now)
}

fn utc_day_start_ms(now_ms: i64) -> i64 {
    utc_datetime(now_ms)
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("UTC midnight is valid")
        .and_utc()
        .timestamp_millis()
}

fn utc_week_start_ms(now_ms: i64) -> i64 {
    let now = utc_datetime(now_ms);
    let days = match now.weekday() {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    (now.date_naive() - chrono::Duration::days(days))
        .and_hms_opt(0, 0, 0)
        .expect("UTC week boundary is valid")
        .and_utc()
        .timestamp_millis()
}

fn utc_calendar_month_start_ms(now_ms: i64) -> i64 {
    let now = utc_datetime(now_ms);
    chrono::NaiveDate::from_ymd_opt(now.year(), now.month(), 1)
        .expect("UTC month boundary is valid")
        .and_hms_opt(0, 0, 0)
        .expect("UTC month midnight is valid")
        .and_utc()
        .timestamp_millis()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStore {
    #[serde(default)]
    pub shares: Vec<Share>,
    #[serde(default)]
    pub pending_router_deletes: Vec<ShareDeleteTombstone>,
    #[serde(default)]
    pub router_share_prune_marker: Option<RouterSharePruneMarker>,
    #[serde(default)]
    pub router_registered: bool,
    #[serde(default)]
    pub last_router_error: Option<String>,
    #[serde(default)]
    pub last_router_heartbeat_ms: Option<u128>,
    #[serde(default)]
    pub router_descriptor_sync_mode: RouterDescriptorSyncMode,
    #[serde(default)]
    pub router_descriptor_sync_diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouterDescriptorSyncMode {
    #[default]
    Unknown,
    Legacy,
    Strict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareDeleteTombstone {
    pub share_id: String,
    pub operation_id: String,
    pub created_at_ms: u128,
    #[serde(default)]
    pub router_api_base: Option<String>,
    #[serde(default)]
    pub installation_id: Option<String>,
    #[serde(default)]
    pub last_attempt_at_ms: Option<u128>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl ShareDeleteTombstone {
    pub fn has_legacy_router_target(&self) -> bool {
        self.router_api_base.is_none() && self.installation_id.is_none()
    }

    pub fn router_target_matches(&self, router_api_base: &str, installation_id: &str) -> bool {
        self.router_api_base.as_deref().is_some_and(|target| {
            normalize_router_api_base(target) == normalize_router_api_base(router_api_base)
        }) && self
            .installation_id
            .as_deref()
            .is_some_and(|target| target.trim() == installation_id.trim())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterSharePruneMarker {
    pub router_api_base: String,
    pub installation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub id: String,
    #[serde(default)]
    pub owner_email: Option<String>,
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_share_status")]
    pub status: String,
    #[serde(default)]
    pub subscription_level: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
    #[serde(default)]
    pub quota_percent: Option<f64>,
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    #[serde(default)]
    pub acl: ShareAcl,
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub parallel_limit: Option<u32>,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub requests_count: u64,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub created_at_ms: u128,
    #[serde(default)]
    pub for_sale: bool,
    #[serde(default)]
    pub free_access: bool,
    #[serde(default = "default_sale_market_kind")]
    pub sale_market_kind: String,
    #[serde(default)]
    pub access_by_app: BTreeMap<String, ShareAppAccess>,
    #[serde(default)]
    pub app_settings: BTreeMap<String, ShareAppSettings>,
    #[serde(default)]
    pub for_sale_official_price_percent_by_app: BTreeMap<String, u16>,
    #[serde(default)]
    pub official_price_percent: Option<f64>,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub bindings: Vec<ShareBinding>,
    #[serde(default)]
    pub binding_history: Vec<ShareBindingHistory>,
    #[serde(default)]
    pub runtime_snapshot: Option<Value>,
    #[serde(default)]
    pub market_grant: Option<ShareMarketGrantStatus>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub router_last_synced_at_ms: Option<u128>,
    #[serde(default)]
    pub router_last_sync_error: Option<String>,
    #[serde(default)]
    pub router_url: Option<String>,
    #[serde(default)]
    pub config_revision: u64,
    #[serde(default)]
    pub router_synced_revision: u64,
    #[serde(default)]
    pub descriptor_generation: u64,
    #[serde(default)]
    pub descriptor_fingerprint: Option<String>,
    #[serde(default)]
    pub router_synced_descriptor_generation: u64,
    #[serde(default)]
    pub router_synced_descriptor_fingerprint: Option<String>,
    #[serde(default)]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareAcl {
    #[serde(default)]
    pub shared_with_emails: Vec<String>,
    #[serde(default)]
    pub public_market_email: Option<String>,
    #[serde(default)]
    pub market_access_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBinding {
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareBindingHistory {
    pub app: AppKind,
    pub previous_provider_id: Option<String>,
    pub next_provider_id: Option<String>,
    pub changed_at_ms: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertShareInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub owner_email: Option<String>,
    pub app: AppKind,
    pub provider_id: String,
    pub provider_type: ProviderType,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub subscription_level: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
    #[serde(default)]
    pub quota_percent: Option<f64>,
    #[serde(default)]
    pub tunnel_subdomain: Option<String>,
    #[serde(default)]
    pub acl: Option<ShareAcl>,
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub parallel_limit: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub for_sale: Option<bool>,
    #[serde(default)]
    pub free_access: Option<bool>,
    #[serde(default)]
    pub sale_market_kind: Option<String>,
    #[serde(default)]
    pub access_by_app: BTreeMap<String, ShareAppAccess>,
    #[serde(default)]
    pub app_settings: BTreeMap<String, ShareAppSettings>,
    #[serde(default)]
    pub for_sale_official_price_percent_by_app: BTreeMap<String, u16>,
    #[serde(default)]
    pub official_price_percent: Option<f64>,
    #[serde(default)]
    pub auto_start: Option<bool>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub bindings: Vec<ShareBinding>,
    #[serde(default)]
    pub runtime_snapshot: Option<Value>,
    #[serde(default)]
    pub market_grant: Option<ShareMarketGrantStatus>,
    #[serde(default)]
    pub user_grants: BTreeMap<String, ShareUserGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareMarketGrantStatus {
    pub status: String,
    #[serde(default)]
    pub grant_id: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub updated_at_ms: Option<u128>,
}

impl ShareStore {
    pub fn load_or_default(config_dir: &Path) -> anyhow::Result<Self> {
        let path = shares_path(config_dir);
        if !path.exists() {
            return Ok(Self::default());
        }

        let content =
            fs::read_to_string(&path).with_context(|| format!("read shares {}", path.display()))?;
        serde_json::from_str(&content).with_context(|| format!("parse shares {}", path.display()))
    }

    pub fn save(&self, config_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all(config_dir)
            .with_context(|| format!("create config dir {}", config_dir.display()))?;
        let path = shares_path(config_dir);
        crate::infra::storage::write_json_pretty(&path, self)
            .with_context(|| format!("write shares {}", path.display()))
    }

    pub fn upsert(&mut self, mut input: UpsertShareInput) -> Result<Share, SharePatchError> {
        let _binding =
            crate::domain::sharing::invariants::validate_and_normalize_upsert_input(&mut input)?;
        let provider_id = input.provider_id.clone();
        let provider_type = input.provider_type;
        let app = input.app;
        let existing_id = input.id.clone().or_else(|| {
            self.shares
                .iter()
                .find(|item| item.app == app && item.provider_id == provider_id)
                .map(|item| item.id.clone())
        });
        let owner_email = input.owner_email.clone();
        let tunnel_subdomain = input.tunnel_subdomain.clone().or_else(|| {
            existing_id
                .as_deref()
                .and_then(|id| self.shares.iter().find(|share| share.id == id))
                .and_then(|share| share.tunnel_subdomain.clone())
                .or_else(|| Some(generate_unique_share_slug(&self.shares)))
        });

        if let Some(conflict) = self.shares.iter().find(|item| {
            item.app == app
                && item.provider_id == provider_id
                && existing_id.as_deref() != Some(item.id.as_str())
                && item.status != "deleted"
        }) {
            return Err(SharePatchError::Invalid(format!(
                "provider already has share {}",
                conflict.id
            )));
        }

        if let Some(subdomain) = tunnel_subdomain.as_deref() {
            if let Some(conflict) = self.shares.iter().find(|item| {
                item.tunnel_subdomain.as_deref() == Some(subdomain)
                    && existing_id.as_deref() != Some(item.id.as_str())
                    && item.status != "deleted"
            }) {
                return Err(SharePatchError::Invalid(format!(
                    "share subdomain is already used by {}",
                    conflict.id
                )));
            }
        }

        let preserved = existing_id
            .as_deref()
            .and_then(|id| self.shares.iter().find(|item| item.id == id))
            .map(|existing| {
                (
                    existing.tokens_used,
                    existing.requests_count,
                    existing.binding_history.clone(),
                    existing.created_at_ms,
                    existing.router_last_synced_at_ms,
                    existing.router_last_sync_error.clone(),
                    existing.router_url.clone(),
                    existing.last_error.clone(),
                    existing.config_revision,
                    existing.router_synced_revision,
                    existing.descriptor_generation,
                    existing.descriptor_fingerprint.clone(),
                    existing.router_synced_descriptor_generation,
                    existing.router_synced_descriptor_fingerprint.clone(),
                    existing.user_grants.clone(),
                )
            });

        let share_id = existing_id.unwrap_or_else(generate_share_id);
        let (
            tokens_used,
            requests_count,
            binding_history,
            created_at_ms,
            router_last_synced_at_ms,
            router_last_sync_error,
            router_url,
            last_error,
            config_revision,
            router_synced_revision,
            descriptor_generation,
            descriptor_fingerprint,
            router_synced_descriptor_generation,
            router_synced_descriptor_fingerprint,
            preserved_user_grants,
        ) = preserved.unwrap_or((
            0,
            0,
            Vec::new(),
            0,
            None,
            None,
            None,
            None,
            0,
            0,
            0,
            None,
            0,
            None,
            BTreeMap::new(),
        ));
        let created_at_ms = if created_at_ms > 0 {
            created_at_ms
        } else {
            crate::infra::time::now_ms()
        };
        let explicit_user_grants = (!input.user_grants.is_empty()).then_some(input.user_grants);

        let share = Share {
            id: share_id,
            owner_email,
            app,
            provider_id,
            provider_type,
            display_name: input.display_name,
            enabled: input.enabled.unwrap_or(true),
            status: input.status.unwrap_or_else(default_share_status),
            subscription_level: input.subscription_level,
            account_email: input.account_email,
            quota_percent: input.quota_percent,
            tunnel_subdomain,
            acl: input.acl.unwrap_or_default(),
            token_limit: input.token_limit,
            parallel_limit: input.parallel_limit,
            tokens_used,
            requests_count,
            expires_at: input.expires_at,
            created_at_ms,
            for_sale: input.for_sale.unwrap_or(false),
            free_access: input.free_access.unwrap_or(false),
            sale_market_kind: input
                .sale_market_kind
                .unwrap_or_else(default_sale_market_kind),
            access_by_app: input.access_by_app,
            app_settings: input.app_settings,
            for_sale_official_price_percent_by_app: input.for_sale_official_price_percent_by_app,
            official_price_percent: input.official_price_percent,
            auto_start: input.auto_start.unwrap_or(true),
            description: input.description,
            bindings: input.bindings,
            binding_history,
            runtime_snapshot: input.runtime_snapshot,
            market_grant: input.market_grant,
            last_error,
            router_last_synced_at_ms,
            router_last_sync_error,
            router_url,
            config_revision: config_revision.saturating_add(1).max(1),
            router_synced_revision,
            descriptor_generation,
            descriptor_fingerprint,
            router_synced_descriptor_generation,
            router_synced_descriptor_fingerprint,
            user_grants: preserved_user_grants.clone(),
        };

        let mut share = share;
        if let Some(user_grants) = explicit_user_grants.as_ref() {
            share.user_grants = normalize_user_grants(
                user_grants,
                &preserved_user_grants,
                share.owner_email.as_deref(),
            )?;
        }
        reconcile_user_grants(&mut share, explicit_user_grants.as_ref());
        share.router_last_sync_error = None;

        self.cancel_pending_router_delete(&share.id);
        if let Some(existing) = self.shares.iter_mut().find(|item| item.id == share.id) {
            *existing = share.clone();
        } else {
            self.shares.push(share.clone());
        }

        Ok(share)
    }

    pub fn get(&self, share_id: &str) -> Option<&Share> {
        self.shares.iter().find(|item| item.id == share_id)
    }

    pub fn share_ids_for_provider(&self, app: AppKind, provider_id: &str) -> Vec<String> {
        self.shares
            .iter()
            .filter(|item| item.app == app && item.provider_id == provider_id)
            .map(|item| item.id.clone())
            .collect()
    }

    pub fn delete(&mut self, share_id: &str) -> Option<ShareDeleteTombstone> {
        self.delete_with_router_target(share_id, None)
    }

    pub fn delete_for_router_target(
        &mut self,
        share_id: &str,
        router_api_base: &str,
        installation_id: &str,
    ) -> Option<ShareDeleteTombstone> {
        self.delete_with_router_target(share_id, Some((router_api_base, installation_id)))
    }

    fn delete_with_router_target(
        &mut self,
        share_id: &str,
        router_target: Option<(&str, &str)>,
    ) -> Option<ShareDeleteTombstone> {
        let before = self.shares.len();
        self.shares.retain(|item| item.id != share_id);
        if self.shares.len() == before {
            return None;
        }
        self.pending_router_deletes
            .retain(|pending| pending.share_id != share_id);
        let tombstone = ShareDeleteTombstone {
            share_id: share_id.to_string(),
            operation_id: generate_share_delete_operation_id(),
            created_at_ms: now_ms(),
            router_api_base: router_target
                .map(|(router_api_base, _)| normalize_router_api_base(router_api_base)),
            installation_id: router_target
                .map(|(_, installation_id)| installation_id.trim().to_string()),
            last_attempt_at_ms: None,
            last_error: None,
        };
        self.pending_router_deletes.push(tombstone.clone());
        Some(tombstone)
    }

    pub fn bind_legacy_router_delete_target(
        &mut self,
        operation_id: &str,
        router_api_base: &str,
        installation_id: &str,
    ) -> bool {
        let Some(tombstone) = self
            .pending_router_deletes
            .iter_mut()
            .find(|pending| pending.operation_id == operation_id)
        else {
            return false;
        };
        if tombstone.router_api_base.is_some() || tombstone.installation_id.is_some() {
            return false;
        }
        tombstone.router_api_base = Some(normalize_router_api_base(router_api_base));
        tombstone.installation_id = Some(installation_id.trim().to_string());
        true
    }

    pub fn pending_router_delete(
        &self,
        share_id: &str,
        operation_id: &str,
    ) -> Option<&ShareDeleteTombstone> {
        self.pending_router_deletes
            .iter()
            .find(|pending| pending.share_id == share_id && pending.operation_id == operation_id)
    }

    pub fn has_pending_router_delete_for_share(&self, share_id: &str) -> bool {
        self.pending_router_deletes
            .iter()
            .any(|pending| pending.share_id == share_id)
    }

    pub fn router_share_prune_applied_for(
        &self,
        router_api_base: &str,
        installation_id: &str,
    ) -> bool {
        self.router_share_prune_marker
            .as_ref()
            .is_some_and(|marker| {
                normalize_router_api_base(&marker.router_api_base)
                    == normalize_router_api_base(router_api_base)
                    && marker.installation_id.trim() == installation_id.trim()
            })
    }

    pub fn mark_router_share_prune_applied(
        &mut self,
        router_api_base: &str,
        installation_id: &str,
    ) -> bool {
        if self.router_share_prune_applied_for(router_api_base, installation_id) {
            return false;
        }
        self.router_share_prune_marker = Some(RouterSharePruneMarker {
            router_api_base: normalize_router_api_base(router_api_base),
            installation_id: installation_id.trim().to_string(),
        });
        true
    }

    pub fn complete_pending_router_delete(&mut self, operation_id: &str) -> bool {
        let before = self.pending_router_deletes.len();
        self.pending_router_deletes
            .retain(|pending| pending.operation_id != operation_id);
        self.pending_router_deletes.len() != before
    }

    pub fn mark_pending_router_delete_failure(
        &mut self,
        operation_id: &str,
        error: String,
    ) -> bool {
        let Some(pending) = self
            .pending_router_deletes
            .iter_mut()
            .find(|pending| pending.operation_id == operation_id)
        else {
            return false;
        };
        pending.last_attempt_at_ms = Some(now_ms());
        pending.last_error = Some(error);
        true
    }

    fn cancel_pending_router_delete(&mut self, share_id: &str) -> bool {
        let before = self.pending_router_deletes.len();
        self.pending_router_deletes
            .retain(|pending| pending.share_id != share_id);
        self.pending_router_deletes.len() != before
    }

    pub fn pause(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.enabled = false;
        share.status = "paused".to_string();
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn resume(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.enabled = true;
        share.status = "active".to_string();
        share.last_error = None;
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn reset_usage(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.tokens_used = 0;
        share.requests_count = 0;
        let reset_at = now_ms();
        for grant in share.user_grants.values_mut() {
            grant.usage = ShareUserUsage::default();
            grant.updated_at_ms = reset_at;
        }
        if share.status == "exhausted" {
            share.status = "paused".to_string();
            share.enabled = false;
        }
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.remove("usage");
                object.remove("lastRequest");
                object.insert("tokensUsed".to_string(), json!(share.tokens_used));
                object.insert("requestsCount".to_string(), json!(share.requests_count));
            }
        }
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn validate_for_invocation(
        &mut self,
        share_id: &str,
        user_email: Option<&str>,
        now_ms: i64,
    ) -> Result<ShareInvocation, ShareInvocationRejection> {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::NotFound,
                message: "Share not found on this cc-switch.".to_string(),
                status_changed: false,
            });
        };

        if !share.enabled || share.status != "active" {
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Inactive,
                message: format!(
                    "Share is not active (current status: {}). Start the share first.",
                    share.status
                ),
                status_changed: false,
            });
        }

        if share
            .expires_at
            .is_some_and(|expires_at| share_expired(expires_at, now_ms))
        {
            share.status = "expired".to_string();
            share.enabled = false;
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Expired,
                message: "Share has expired. Extend the share expiration or create a new share."
                    .to_string(),
                status_changed: true,
            });
        }

        if share
            .token_limit
            .is_some_and(|token_limit| share.tokens_used >= token_limit)
        {
            share.status = "exhausted".to_string();
            share.enabled = false;
            return Err(ShareInvocationRejection {
                reason: ShareRejectReason::Exhausted,
                message:
                    "Share token quota has been exhausted. Reset usage or increase the token limit."
                        .to_string(),
                status_changed: true,
            });
        }

        let normalized_user_email = user_email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let user_grant = normalized_user_email
            .as_deref()
            .and_then(|email| share.user_grants.get(email))
            .filter(|grant| grant.active);
        if let Some(grant) = user_grant {
            if grant
                .policy
                .expires_at
                .is_some_and(|expires_at| share_expired(expires_at, now_ms))
            {
                return Err(ShareInvocationRejection {
                    reason: ShareRejectReason::UserExpired,
                    message: "This user's Share access has expired.".to_string(),
                    status_changed: false,
                });
            }
            if grant.policy.token_limit.is_some_and(|limit| {
                grant.usage.tokens_for(grant.policy.token_period, now_ms) >= limit
            }) {
                return Err(ShareInvocationRejection {
                    reason: ShareRejectReason::UserExhausted,
                    message: "This user's Share token quota has been exhausted.".to_string(),
                    status_changed: false,
                });
            }
        }

        Ok(ShareInvocation {
            share_id: share.id.clone(),
            share_name: share
                .display_name
                .clone()
                .unwrap_or_else(|| share.id.clone()),
            parallel_limit: share.parallel_limit,
            user_email: normalized_user_email,
            user_parallel_limit: user_grant.and_then(|grant| grant.policy.parallel_limit),
        })
    }

    pub fn record_invocation_result(&mut self, share_id: &str, tokens: u64) -> Option<Share> {
        self.record_user_invocation_result(share_id, None, tokens, now_ms() as i64)
    }

    pub fn record_user_invocation_result(
        &mut self,
        share_id: &str,
        user_email: Option<&str>,
        tokens: u64,
        recorded_at_ms: i64,
    ) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.requests_count = share.requests_count.saturating_add(1);
        share.tokens_used = share.tokens_used.saturating_add(tokens);
        if share
            .token_limit
            .is_some_and(|token_limit| share.tokens_used >= token_limit)
        {
            share.status = "exhausted".to_string();
            share.enabled = false;
        }
        if let Some(email) = user_email
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase)
        {
            if let Some(grant) = share.user_grants.get_mut(&email) {
                grant.usage.record(tokens, recorded_at_ms);
                grant.updated_at_ms = now_ms();
            }
        }
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("tokensUsed".to_string(), json!(share.tokens_used));
                object.insert("requestsCount".to_string(), json!(share.requests_count));
                object.insert("shareStatus".to_string(), json!(share.status));
            }
        }
        Some(share.clone())
    }

    pub fn update_binding(
        &mut self,
        share_id: &str,
        binding: ShareBinding,
    ) -> Result<Share, ShareUpdateError> {
        if self.shares.iter().any(|item| {
            item.id != share_id
                && item.app == binding.app
                && item.provider_id == binding.provider_id
                && item.status != "deleted"
        }) {
            return Err(ShareUpdateError::ProviderAlreadyShared);
        }
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(ShareUpdateError::NotFound)?;
        if share.status != "paused" {
            return Err(ShareUpdateError::MustBePaused);
        }

        if binding.app != share.app {
            return Err(ShareUpdateError::InvalidApp);
        }

        if share.bindings.len() != 1 {
            share.bindings = vec![ShareBinding {
                app: share.app,
                provider_id: share.provider_id.clone(),
                provider_type: share.provider_type,
            }];
        }

        let previous_provider_id = share.bindings.first().map(|item| item.provider_id.clone());
        share.bindings = vec![binding.clone()];

        share.provider_id = binding.provider_id.clone();
        share.provider_type = binding.provider_type;

        share.binding_history.push(ShareBindingHistory {
            app: binding.app,
            previous_provider_id,
            next_provider_id: Some(binding.provider_id),
            changed_at_ms: now_ms(),
        });

        mark_share_config_pending(share);

        Some(share.clone()).ok_or(ShareUpdateError::NotFound)
    }

    pub fn replace_acl(&mut self, share_id: &str, acl: ShareAcl) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.acl = acl;
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn update_subdomain(
        &mut self,
        share_id: &str,
        subdomain: String,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let subdomain = normalize_share_subdomain(&subdomain)
            .map_err(|message| SharePatchError::Invalid(message.to_string()))?;
        share.tunnel_subdomain = Some(subdomain.clone());
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("subdomain".to_string(), json!(subdomain));
            }
        }
        mark_share_config_pending(share);
        Ok(share.clone())
    }

    pub fn bind_all_to_client_owner(
        &mut self,
        owner_email: &str,
    ) -> Result<Vec<Share>, SharePatchError> {
        let owner_email = normalize_verified_email(owner_email)?;
        let mut updated = Vec::new();
        for share in &mut self.shares {
            if bind_share_to_client_owner(share, &owner_email) {
                reconcile_user_grants(share, None);
                mark_share_config_pending(share);
                updated.push(share.clone());
            }
        }
        Ok(updated)
    }

    pub fn migrate_user_grants_from_acl(&mut self) -> Vec<Share> {
        let mut updated = Vec::new();
        for share in &mut self.shares {
            let previous = share.user_grants.clone();
            reconcile_user_grants(share, None);
            if share.user_grants != previous {
                mark_share_config_pending(share);
                updated.push(share.clone());
            }
        }
        updated
    }

    fn sync_owner_email_snapshot(share: &mut Share, owner_email: &str) {
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("ownerEmail".to_string(), json!(owner_email));
            }
        }
    }

    pub fn authorize_share_market(
        &mut self,
        share_id: &str,
        market_email: String,
        public_market_emails: &BTreeSet<String>,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let market_email = normalize_verified_email(&market_email)?;
        share.for_sale = true;
        share.sale_market_kind = "share".to_string();
        share.acl.market_access_mode = Some("selected".to_string());
        share
            .acl
            .shared_with_emails
            .retain(|email| !public_market_emails.contains(&email.trim().to_ascii_lowercase()));
        insert_email(&mut share.acl.shared_with_emails, market_email.clone());

        let mut app_keys = if share.bindings.is_empty() {
            vec![share.app.as_str().to_string()]
        } else {
            share
                .bindings
                .iter()
                .map(|binding| binding.app.as_str().to_string())
                .collect::<Vec<_>>()
        };
        app_keys.sort();
        app_keys.dedup();
        for app in app_keys {
            let access = share
                .access_by_app
                .entry(app.clone())
                .or_insert_with(|| ShareAppAccess {
                    shared_with_emails: Vec::new(),
                    market_access_mode: "selected".to_string(),
                });
            access
                .shared_with_emails
                .retain(|email| !public_market_emails.contains(&email.trim().to_ascii_lowercase()));
            insert_email(&mut access.shared_with_emails, market_email.clone());
            access.market_access_mode = "selected".to_string();

            let settings = share.app_settings.entry(app).or_default();
            settings.for_sale = "Yes".to_string();
            settings.sale_market_kind = "share".to_string();
            settings.market_access_mode = "selected".to_string();
            settings
                .shared_with_emails
                .retain(|email| !public_market_emails.contains(&email.trim().to_ascii_lowercase()));
            insert_email(&mut settings.shared_with_emails, market_email.clone());
        }
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                object.insert("forSale".to_string(), json!(share.for_sale));
                object.insert("saleMarketKind".to_string(), json!(share.sale_market_kind));
                object.insert("accessByApp".to_string(), json!(share.access_by_app));
                object.insert("appSettings".to_string(), json!(share.app_settings));
            }
        }
        mark_share_config_pending(share);
        Ok(share.clone())
    }

    pub fn update_market_grant(
        &mut self,
        share_id: &str,
        market_grant: Option<ShareMarketGrantStatus>,
    ) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.market_grant = market_grant;
        if let Some(snapshot) = share.runtime_snapshot.as_mut() {
            if let Some(object) = snapshot.as_object_mut() {
                match &share.market_grant {
                    Some(grant) => {
                        object.insert(
                            "marketGrant".to_string(),
                            serde_json::to_value(grant).expect("serialize share market grant"),
                        );
                    }
                    None => {
                        object.remove("marketGrant");
                    }
                }
            }
        }
        mark_share_config_pending(share);
        Some(share.clone())
    }

    pub fn apply_settings_patch(
        &mut self,
        share_id: &str,
        patch: ShareSettingsPatch,
    ) -> Result<Share, SharePatchError> {
        let index = self
            .shares
            .iter()
            .position(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let mut share = self.shares[index].clone();
        let pricing_was_explicit = patch.for_sale_official_price_percent_by_app.is_some();

        if let Some(owner_email) = patch.owner_email {
            let owner_email = normalize_optional_email(Some(owner_email))
                .ok_or_else(|| SharePatchError::Invalid("ownerEmail is empty".to_string()))?;
            if !share
                .owner_email
                .as_deref()
                .is_some_and(|current| current.eq_ignore_ascii_case(&owner_email))
            {
                return Err(SharePatchError::Invalid(
                    "share owner is managed by the client owner".to_string(),
                ));
            }
        }

        if let Some(description) = patch.description {
            share.description = description.map(|value| value.trim().to_string());
        }
        if let Some(for_sale) = patch.for_sale {
            apply_router_for_sale_patch(&mut share, &for_sale);
        }
        if let Some(sale_market_kind) = patch.sale_market_kind {
            share.sale_market_kind = normalize_non_empty(sale_market_kind, "token");
        }
        if let Some(market_access_mode) = patch.market_access_mode {
            share.acl.market_access_mode =
                Some(normalize_non_empty(market_access_mode, "selected"));
        }
        if let Some(shared_with_emails) = patch.shared_with_emails {
            share.acl.shared_with_emails =
                normalize_email_list(&shared_with_emails, share.owner_email.as_deref());
        }
        if let Some(access_by_app) = patch.access_by_app {
            share.access_by_app =
                normalize_access_by_app(access_by_app, share.owner_email.as_deref());
        }
        if let Some(app_settings) = patch.app_settings {
            share.app_settings = normalize_app_settings(app_settings, share.owner_email.as_deref());
        }
        if let Some(pricing) = patch.for_sale_official_price_percent_by_app {
            share.for_sale_official_price_percent_by_app = pricing;
        }
        if let Some(token_limit) = patch.token_limit {
            share.token_limit = (token_limit >= 0).then_some(token_limit as u64);
        }
        if let Some(parallel_limit) = patch.parallel_limit {
            share.parallel_limit = (parallel_limit >= 0).then_some(parallel_limit as u32);
        }
        if let Some(expires_at) = patch.expires_at {
            share.expires_at = parse_share_expiration(&expires_at)?;
        }
        let explicit_user_grants = patch.user_grants;
        if let Some(user_grants) = explicit_user_grants.as_ref() {
            share.user_grants = normalize_user_grants(
                user_grants,
                &share.user_grants,
                share.owner_email.as_deref(),
            )?;
        }
        if let Some(auto_start) = patch.auto_start {
            share.auto_start = auto_start;
        }

        reconcile_user_grants(&mut share, explicit_user_grants.as_ref());

        let pricing_eligible = share.for_sale
            && !share.free_access
            && share.sale_market_kind.trim().eq_ignore_ascii_case("token");
        if !pricing_eligible {
            if pricing_was_explicit && !share.for_sale_official_price_percent_by_app.is_empty() {
                return Err(SharePatchError::Invalid(
                    "share official price percent requires forSale=Yes and saleMarketKind=token"
                        .to_string(),
                ));
            }
            share.for_sale_official_price_percent_by_app.clear();
        }
        crate::domain::sharing::invariants::validate_share_import(&share)?;
        mark_share_config_pending(&mut share);
        self.shares[index] = share.clone();

        Ok(share)
    }

    pub fn canonicalize_primary_app_settings(
        &mut self,
        share_id: &str,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;
        let app = share.app.as_str().to_string();
        let market_access_mode = share
            .acl
            .market_access_mode
            .clone()
            .unwrap_or_else(|| "selected".to_string());
        let shared_with_emails = share.acl.shared_with_emails.clone();
        share.access_by_app.clear();
        share.access_by_app.insert(
            app.clone(),
            ShareAppAccess {
                shared_with_emails: shared_with_emails.clone(),
                market_access_mode: market_access_mode.clone(),
            },
        );
        share.app_settings.clear();
        share.app_settings.insert(
            app,
            ShareAppSettings {
                for_sale: share_router_for_sale_label(share),
                sale_market_kind: share.sale_market_kind.clone(),
                market_access_mode,
                shared_with_emails,
                token_limit: share.token_limit.map(|value| value as i64).unwrap_or(-1),
                parallel_limit: share.parallel_limit.map(i64::from).unwrap_or(-1),
                expires_at: share_expires_at_rfc3339(share.expires_at),
            },
        );
        Ok(share.clone())
    }

    pub fn import_shares(&mut self, shares: Vec<Share>) -> usize {
        let mut imported = 0;
        for mut share in shares {
            if self.cancel_pending_router_delete(&share.id) {
                mark_share_config_pending(&mut share);
            }
            if let Some(existing) = self.shares.iter_mut().find(|item| item.id == share.id) {
                *existing = share;
            } else {
                self.shares.push(share);
            }
            imported += 1;
        }
        imported
    }

    pub fn replace_shares(&mut self, mut shares: Vec<Share>) {
        for share in &mut shares {
            if self.cancel_pending_router_delete(&share.id) {
                mark_share_config_pending(share);
            }
        }
        self.shares = shares;
    }

    pub fn replace_configured_share(&mut self, candidate: Share) -> Result<Share, SharePatchError> {
        crate::domain::sharing::invariants::validate_share_import(&candidate)?;
        let index = self
            .shares
            .iter()
            .position(|share| share.id == candidate.id)
            .ok_or(SharePatchError::NotFound)?;
        if self.shares.iter().enumerate().any(|(other_index, share)| {
            other_index != index
                && share.status != "deleted"
                && share.app == candidate.app
                && share.provider_id == candidate.provider_id
        }) {
            return Err(SharePatchError::Invalid(
                "provider already has an active share".to_string(),
            ));
        }
        if let Some(subdomain) = candidate.tunnel_subdomain.as_deref() {
            if self.shares.iter().enumerate().any(|(other_index, share)| {
                other_index != index
                    && share.status != "deleted"
                    && share.tunnel_subdomain.as_deref() == Some(subdomain)
            }) {
                return Err(SharePatchError::Invalid(
                    "share subdomain is already in use".to_string(),
                ));
            }
        }
        self.cancel_pending_router_delete(&candidate.id);
        self.shares[index] = candidate.clone();
        Ok(candidate)
    }

    pub fn set_share_tunnel_status(
        &mut self,
        share_id: &str,
        status: &str,
        error: Option<String>,
    ) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        let enabled = status == "active";
        let changed =
            share.status != status || share.enabled != enabled || share.last_error != error;
        share.status = status.to_string();
        share.enabled = enabled;
        share.last_error = error;
        if changed {
            mark_share_config_pending(share);
        }
        Some(share.clone())
    }

    pub fn restore_auto_start(&mut self) -> Vec<Share> {
        for share in self.shares.iter_mut().filter(|item| item.auto_start) {
            let changed = share.status != "active" || !share.enabled || share.last_error.is_some();
            share.status = "active".to_string();
            share.enabled = true;
            share.last_error = None;
            if changed {
                mark_share_config_pending(share);
            }
        }
        self.shares.clone()
    }

    pub fn refresh_runtime_snapshots(
        &mut self,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<Share> {
        for share in &mut self.shares {
            share.runtime_snapshot = Some(runtime_snapshot_for_share(
                share, providers, accounts, usage,
            ));
        }
        self.shares.clone()
    }

    pub fn refresh_runtime_snapshots_for_providers(
        &mut self,
        provider_keys: &BTreeSet<(AppKind, String)>,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<String> {
        let mut updated_ids = Vec::new();
        for share in &mut self.shares {
            let uses_provider = if share.bindings.is_empty() {
                provider_keys.contains(&(share.app, share.provider_id.clone()))
            } else {
                share.bindings.iter().any(|binding| {
                    provider_keys.contains(&(binding.app, binding.provider_id.clone()))
                })
            };
            if !uses_provider {
                continue;
            }

            share.runtime_snapshot = Some(runtime_snapshot_for_share(
                share, providers, accounts, usage,
            ));
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        updated_ids
    }

    pub fn refresh_subscription_expiry_snapshots_for_providers(
        &mut self,
        provider_keys: &BTreeSet<(AppKind, String)>,
        providers: &ProviderStore,
        accounts: Option<&AccountStore>,
        usage: &UsageStore,
    ) -> Vec<String> {
        let mut updated_ids = Vec::new();
        for share in &mut self.shares {
            let uses_provider = if share.bindings.is_empty() {
                provider_keys.contains(&(share.app, share.provider_id.clone()))
            } else {
                share.bindings.iter().any(|binding| {
                    provider_keys.contains(&(binding.app, binding.provider_id.clone()))
                })
            };
            if !uses_provider {
                continue;
            }

            let next_snapshot = runtime_snapshot_for_share(share, providers, accounts, usage);
            if subscription_expiry_fingerprint(share.runtime_snapshot.as_ref())
                == subscription_expiry_fingerprint(Some(&next_snapshot))
            {
                continue;
            }
            share.runtime_snapshot = Some(next_snapshot);
            mark_share_config_pending(share);
            updated_ids.push(share.id.clone());
        }
        updated_ids
    }

    pub fn mark_router_sync(
        &mut self,
        share_id: &str,
        revision: u64,
        router_url: Option<String>,
        result: Result<u128, String>,
    ) {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return;
        };
        match result {
            Ok(now) => {
                share.router_synced_revision = share.router_synced_revision.max(revision);
                share.router_last_synced_at_ms = Some(now);
                if share.router_synced_revision >= share.config_revision {
                    share.router_last_sync_error = None;
                }
                share.router_url = router_url;
            }
            Err(error) => {
                if revision >= share.config_revision && share.router_synced_revision < revision {
                    share.router_last_sync_error = Some(error);
                }
            }
        }
    }

    pub fn prepare_descriptor_projection(
        &mut self,
        share_id: &str,
        fingerprint: String,
    ) -> Option<(u64, String)> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        let changed = share.descriptor_fingerprint.as_deref() != Some(fingerprint.as_str());
        if changed || share.descriptor_generation == 0 {
            share.descriptor_generation = share
                .descriptor_generation
                .max(share.router_synced_descriptor_generation)
                .saturating_add(1)
                .max(1);
            share.descriptor_fingerprint = Some(fingerprint);
            share.router_last_sync_error = None;
        }
        Some((
            share.descriptor_generation,
            share.descriptor_fingerprint.clone().unwrap_or_default(),
        ))
    }

    pub fn mark_router_descriptor_sync(
        &mut self,
        share_id: &str,
        generation: u64,
        fingerprint: &str,
        config_revision: u64,
        router_url: Option<String>,
        result: Result<u128, String>,
    ) -> bool {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return false;
        };
        let is_current = share.descriptor_generation == generation
            && share.descriptor_fingerprint.as_deref() == Some(fingerprint);
        match result {
            Ok(now) if is_current => {
                share.router_synced_descriptor_generation = generation;
                share.router_synced_descriptor_fingerprint = Some(fingerprint.to_string());
                share.router_synced_revision = share.router_synced_revision.max(config_revision);
                share.router_last_synced_at_ms = Some(now);
                share.router_last_sync_error = None;
                share.router_url = router_url;
                true
            }
            Ok(_) => false,
            Err(error) if is_current => {
                share.router_last_sync_error = Some(error);
                false
            }
            Err(_) => false,
        }
    }

    pub fn descriptor_projection_pending(&self, share: &Share) -> bool {
        share.descriptor_generation == 0
            || share.descriptor_fingerprint.is_none()
            || share.router_synced_descriptor_generation != share.descriptor_generation
            || share.router_synced_descriptor_fingerprint != share.descriptor_fingerprint
    }

    pub fn record_router_descriptor_sync_mode(
        &mut self,
        mode: RouterDescriptorSyncMode,
        diagnostic: Option<String>,
    ) {
        if self.router_descriptor_sync_mode != RouterDescriptorSyncMode::Strict
            || mode == RouterDescriptorSyncMode::Strict
        {
            self.router_descriptor_sync_mode = mode;
        }
        self.router_descriptor_sync_diagnostic = diagnostic;
    }
}

fn bind_share_to_client_owner(share: &mut Share, owner_email: &str) -> bool {
    if share
        .owner_email
        .as_deref()
        .is_some_and(|current| current == owner_email)
    {
        return false;
    }
    let previous_owner = share
        .owner_email
        .as_deref()
        .and_then(|email| normalize_verified_email(email).ok())
        .filter(|email| !email.eq_ignore_ascii_case(owner_email));
    if let Some(previous_owner) = previous_owner {
        insert_email(&mut share.acl.shared_with_emails, previous_owner.clone());
        for access in share.access_by_app.values_mut() {
            insert_email(&mut access.shared_with_emails, previous_owner.clone());
        }
        for settings in share.app_settings.values_mut() {
            insert_email(&mut settings.shared_with_emails, previous_owner.clone());
        }
    }
    share.owner_email = Some(owner_email.to_string());
    share.acl.shared_with_emails =
        normalize_email_list(&share.acl.shared_with_emails, Some(owner_email));
    share.access_by_app = normalize_access_by_app(share.access_by_app.clone(), Some(owner_email));
    share.app_settings = normalize_app_settings(share.app_settings.clone(), Some(owner_email));
    ShareStore::sync_owner_email_snapshot(share, owner_email);
    true
}

fn runtime_snapshot_for_share(
    share: &Share,
    providers: &ProviderStore,
    accounts: Option<&AccountStore>,
    usage: &UsageStore,
) -> Value {
    let descriptor =
        descriptor_for_share_with_accounts_and_usage(share, providers, accounts, Some(usage));
    let provider = providers
        .providers
        .iter()
        .find(|item| item.app == share.app && item.provider.id == share.provider_id);
    let health = provider.map(|item| crate::domain::health::provider_health(item, usage));
    let last_request = usage
        .logs
        .iter()
        .filter(|log| log.provider_id == share.provider_id && log.app == share.app)
        .max_by_key(|log| log.created_at_ms);

    json!({
        "shareId": share.id,
        "app": share.app,
        "providerId": share.provider_id,
        "providerType": share.provider_type,
        "providerName": provider.map(|item| item.provider.name.clone()),
        "accountEmail": descriptor.upstream_provider.as_ref().and_then(|item| item.account_email.clone()).or_else(|| share.account_email.clone()),
        "subscriptionLevel": descriptor.upstream_provider.as_ref().and_then(|item| item.subscription_level.clone()).or_else(|| share.subscription_level.clone()),
        "subscriptionExpiresAt": descriptor.upstream_provider.as_ref().and_then(|item| item.subscription_expires_at.clone()),
        "subscriptionRemainingMs": descriptor.upstream_provider.as_ref().and_then(|item| item.subscription_remaining_ms),
        "quotaPercent": descriptor.upstream_provider.as_ref().and_then(|item| item.quota_percent).or(share.quota_percent),
        "tokensUsed": share.tokens_used,
        "requestsCount": share.requests_count,
        "health": health,
        "lastRequest": last_request,
        "marketGrant": share.market_grant,
        "upstreamProvider": descriptor.upstream_provider,
        "appRuntimes": descriptor.app_runtimes,
        "appProviders": descriptor.app_providers,
        "appAvailability": descriptor.app_availability,
        "modelHealth": descriptor.model_health,
        "updatedAtMs": now_ms(),
    })
}

fn subscription_expiry_fingerprint(snapshot: Option<&Value>) -> Vec<String> {
    fn collect(value: &Value, path: &str, entries: &mut Vec<String>) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    let child_path = format!("{path}/{key}");
                    if matches!(
                        key.as_str(),
                        "subscriptionExpiresAt" | "subscriptionPeriodEnd"
                    ) {
                        entries.push(format!("{child_path}={value}"));
                    } else {
                        collect(value, &child_path, entries);
                    }
                }
            }
            Value::Array(values) => {
                for (index, value) in values.iter().enumerate() {
                    collect(value, &format!("{path}/{index}"), entries);
                }
            }
            _ => {}
        }
    }

    let mut entries = Vec::new();
    if let Some(snapshot) = snapshot {
        collect(snapshot, "", &mut entries);
    }
    entries.sort();
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareUpdateError {
    NotFound,
    MustBePaused,
    InvalidApp,
    ProviderAlreadyShared,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInvocation {
    pub share_id: String,
    pub share_name: String,
    pub parallel_limit: Option<u32>,
    pub user_email: Option<String>,
    pub user_parallel_limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareInvocationRejection {
    pub reason: ShareRejectReason,
    pub message: String,
    pub status_changed: bool,
}

impl ShareInvocationRejection {
    pub fn formatted_message(&self) -> String {
        format!("{} [{}]", self.message, self.reason.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareRejectReason {
    NotFound,
    Inactive,
    Expired,
    Exhausted,
    ParallelLimit,
    UserExpired,
    UserExhausted,
    UserParallelLimit,
}

impl ShareRejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::Inactive => "Inactive",
            Self::Expired => "Expired",
            Self::Exhausted => "Exhausted",
            Self::ParallelLimit => "ParallelLimit",
            Self::UserExpired => "UserExpired",
            Self::UserExhausted => "UserExhausted",
            Self::UserParallelLimit => "UserParallelLimit",
        }
    }
}

impl std::fmt::Display for ShareUpdateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("share not found"),
            Self::MustBePaused => {
                formatter.write_str("share must be paused before updating binding")
            }
            Self::InvalidApp => formatter.write_str("share binding app must match share.app"),
            Self::ProviderAlreadyShared => {
                formatter.write_str("provider already has an active share")
            }
        }
    }
}

impl std::error::Error for ShareUpdateError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharePatchError {
    NotFound,
    Invalid(String),
}

impl std::fmt::Display for SharePatchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => formatter.write_str("share not found"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for SharePatchError {}

pub fn shares_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(SHARES_FILE_NAME)
}

fn generate_share_id() -> String {
    let mut bytes = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut bytes);
    let suffix: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("share-{suffix}")
}

fn generate_share_delete_operation_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn normalize_router_api_base(router_api_base: &str) -> String {
    router_api_base.trim().trim_end_matches('/').to_string()
}

fn generate_unique_share_slug(shares: &[Share]) -> String {
    for attempt in 0..crate::domain::subdomain_suggest::SUGGEST_MAX_ATTEMPTS {
        let candidate =
            crate::domain::subdomain_suggest::generate_candidate(&mut rand::thread_rng(), attempt);
        if !shares.iter().any(|share| {
            share.status != "deleted"
                && share.tunnel_subdomain.as_deref() == Some(candidate.as_str())
        }) {
            return candidate;
        }
    }
    crate::domain::subdomain_suggest::generate_share_slug(&mut rand::thread_rng())
}

fn default_share_status() -> String {
    "active".to_string()
}

fn mark_share_config_pending(share: &mut Share) {
    share.config_revision = share.config_revision.saturating_add(1).max(1);
    share.router_last_sync_error = None;
}

fn default_sale_market_kind() -> String {
    "token".to_string()
}

fn share_expired(expires_at: i64, now_ms: i64) -> bool {
    let expires_at_ms = if expires_at > 0 && expires_at < 10_000_000_000 {
        expires_at.saturating_mul(1000)
    } else {
        expires_at
    };
    now_ms > expires_at_ms
}

fn parse_share_expiration(value: &str) -> Result<Option<i64>, SharePatchError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        return Ok(Some(timestamp));
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|value| Some(value.timestamp_millis()))
        .map_err(|_| SharePatchError::Invalid("expiresAt must be a timestamp or RFC3339".into()))
}

pub(crate) fn share_router_for_sale_label(share: &Share) -> String {
    if share.free_access {
        "Free".to_string()
    } else if share.for_sale {
        "Yes".to_string()
    } else {
        "No".to_string()
    }
}

pub(crate) fn apply_router_for_sale_patch(share: &mut Share, value: &str) {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => {
            share.free_access = true;
            share.for_sale = false;
        }
        "yes" | "true" | "1" | "share" => {
            share.free_access = false;
            share.for_sale = true;
        }
        _ => {
            share.free_access = false;
            share.for_sale = false;
        }
    }
}

pub(crate) fn normalize_router_for_sale_setting(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => "Free".to_string(),
        "yes" | "true" | "1" | "share" => "Yes".to_string(),
        _ => "No".to_string(),
    }
}

fn normalize_non_empty(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_ascii_lowercase()
    }
}

fn normalize_optional_email(value: Option<String>) -> Option<String> {
    value
        .map(|email| email.trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty())
}

fn normalize_verified_email(email: &str) -> Result<String, SharePatchError> {
    let email = email.trim().to_ascii_lowercase();
    if email.is_empty()
        || email.len() > 254
        || email.chars().any(char::is_whitespace)
        || email.matches('@').count() != 1
    {
        return Err(SharePatchError::Invalid(
            "ownerEmail format is invalid".to_string(),
        ));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(SharePatchError::Invalid(
            "ownerEmail format is invalid".to_string(),
        ));
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
    {
        return Err(SharePatchError::Invalid(
            "ownerEmail format is invalid".to_string(),
        ));
    }
    Ok(email)
}

fn default_user_policy(share: &Share) -> ShareUserPolicy {
    ShareUserPolicy {
        parallel_limit: share.parallel_limit,
        token_limit: share.token_limit,
        token_period: ShareTokenPeriod::Lifetime,
        expires_at: share.expires_at,
    }
}

fn new_user_grant(share: &Share, email: String, role: &str) -> ShareUserGrant {
    let now = now_ms();
    ShareUserGrant {
        email,
        role: role.to_string(),
        active: true,
        policy: default_user_policy(share),
        usage: Default::default(),
        created_at_ms: now,
        updated_at_ms: now,
        revoked_at_ms: None,
        revision: 1,
    }
}

fn normalize_user_grants(
    incoming: &BTreeMap<String, ShareUserGrant>,
    existing: &BTreeMap<String, ShareUserGrant>,
    owner_email: Option<&str>,
) -> Result<BTreeMap<String, ShareUserGrant>, SharePatchError> {
    let now = now_ms();
    let owner = owner_email.map(|value| value.trim().to_ascii_lowercase());
    let mut normalized = existing.clone();
    for grant in normalized.values_mut() {
        if grant.role != "owner" && grant.active {
            grant.active = false;
            grant.revoked_at_ms = Some(now);
            grant.updated_at_ms = now;
            grant.revision = grant.revision.saturating_add(1).max(1);
        }
    }

    for (key, incoming_grant) in incoming {
        let email = normalize_verified_email(if incoming_grant.email.trim().is_empty() {
            key
        } else {
            &incoming_grant.email
        })?;
        if owner.as_deref() == Some(email.as_str()) && incoming_grant.role != "owner" {
            return Err(SharePatchError::Invalid(
                "share owner cannot also be a ShareTo user".to_string(),
            ));
        }
        if incoming_grant.policy.parallel_limit == Some(0)
            || incoming_grant.policy.token_limit == Some(0)
        {
            return Err(SharePatchError::Invalid(
                "user limits must be positive or unlimited".to_string(),
            ));
        }
        let previous = existing.get(&email);
        let mut grant = incoming_grant.clone();
        grant.email = email.clone();
        grant.role = if owner.as_deref() == Some(email.as_str()) {
            "owner".to_string()
        } else {
            "shareto".to_string()
        };
        grant.active = true;
        grant.usage = previous.map(|item| item.usage.clone()).unwrap_or_default();
        grant.created_at_ms = previous
            .map(|item| item.created_at_ms)
            .filter(|value| *value > 0)
            .unwrap_or(now);
        grant.updated_at_ms = now;
        grant.revoked_at_ms = None;
        grant.revision = previous
            .map(|item| item.revision.saturating_add(1))
            .unwrap_or(1)
            .max(1);
        normalized.insert(email, grant);
    }
    Ok(normalized)
}

fn reconcile_user_grants(
    share: &mut Share,
    explicit_user_grants: Option<&BTreeMap<String, ShareUserGrant>>,
) {
    let policy_template = default_user_policy(share);
    let owner = share
        .owner_email
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    if let Some(owner) = owner.as_ref() {
        let now = now_ms();
        let grant = share
            .user_grants
            .entry(owner.clone())
            .or_insert_with(|| ShareUserGrant {
                email: owner.clone(),
                role: "owner".to_string(),
                active: true,
                policy: policy_template.clone(),
                usage: Default::default(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                revoked_at_ms: None,
                revision: 1,
            });
        if grant.email != *owner
            || grant.role != "owner"
            || !grant.active
            || grant.revoked_at_ms.is_some()
        {
            grant.updated_at_ms = now;
            grant.revision = grant.revision.saturating_add(1).max(1);
        }
        grant.email = owner.clone();
        grant.role = "owner".to_string();
        grant.active = true;
        grant.revoked_at_ms = None;

        for (email, grant) in &mut share.user_grants {
            if email != owner && grant.role == "owner" {
                grant.role = "shareto".to_string();
                grant.updated_at_ms = now;
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
        }
    }

    if explicit_user_grants.is_none() {
        let desired_emails = share_acl_emails(share);
        let now = now_ms();
        for email in &desired_emails {
            if let Some(grant) = share.user_grants.get_mut(email) {
                if grant.role == "shareto" && !grant.active {
                    grant.active = true;
                    grant.revoked_at_ms = None;
                    grant.updated_at_ms = now;
                    grant.revision = grant.revision.saturating_add(1).max(1);
                }
            } else {
                share.user_grants.insert(
                    email.clone(),
                    new_user_grant(share, email.clone(), "shareto"),
                );
            }
        }
        for grant in share
            .user_grants
            .values_mut()
            .filter(|grant| grant.role == "shareto" && grant.active)
        {
            if !desired_emails.contains(&grant.email) {
                grant.active = false;
                grant.revoked_at_ms = Some(now);
                grant.updated_at_ms = now;
                grant.revision = grant.revision.saturating_add(1).max(1);
            }
        }
    } else {
        let previous_direct = share
            .user_grants
            .values()
            .filter(|grant| grant.role == "shareto")
            .map(|grant| grant.email.clone())
            .collect::<BTreeSet<_>>();
        share
            .acl
            .shared_with_emails
            .retain(|email| !previous_direct.contains(&email.trim().to_ascii_lowercase()));
        for access in share.access_by_app.values_mut() {
            access
                .shared_with_emails
                .retain(|email| !previous_direct.contains(&email.trim().to_ascii_lowercase()));
        }
        for settings in share.app_settings.values_mut() {
            settings
                .shared_with_emails
                .retain(|email| !previous_direct.contains(&email.trim().to_ascii_lowercase()));
        }
        for grant in share
            .user_grants
            .values()
            .filter(|grant| grant.active && grant.role == "shareto")
        {
            insert_email(&mut share.acl.shared_with_emails, grant.email.clone());
            for access in share.access_by_app.values_mut() {
                insert_email(&mut access.shared_with_emails, grant.email.clone());
            }
            for settings in share.app_settings.values_mut() {
                insert_email(&mut settings.shared_with_emails, grant.email.clone());
            }
        }
        let desired_emails = share_acl_emails(share);
        for email in desired_emails {
            if share
                .user_grants
                .get(&email)
                .is_some_and(|grant| grant.active)
            {
                continue;
            }
            if let Some(grant) = share.user_grants.get_mut(&email) {
                grant.active = true;
                grant.revoked_at_ms = None;
                grant.updated_at_ms = now_ms();
                grant.revision = grant.revision.saturating_add(1).max(1);
            } else {
                share
                    .user_grants
                    .insert(email.clone(), new_user_grant(share, email, "shareto"));
            }
        }
    }
}

fn share_acl_emails(share: &Share) -> BTreeSet<String> {
    let owner = share
        .owner_email
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase);
    share
        .acl
        .shared_with_emails
        .iter()
        .chain(
            share
                .access_by_app
                .values()
                .flat_map(|access| access.shared_with_emails.iter()),
        )
        .chain(
            share
                .app_settings
                .values()
                .flat_map(|settings| settings.shared_with_emails.iter()),
        )
        .map(|email| email.trim().to_ascii_lowercase())
        .filter(|email| !email.is_empty() && owner.as_deref() != Some(email.as_str()))
        .collect()
}

pub fn normalize_share_subdomain(subdomain: &str) -> Result<String, &'static str> {
    let value = subdomain.trim().to_ascii_lowercase();
    crate::domain::router::ShareSlug::parse(&value)
        .map_err(|_| "share slug must be 6-30 lowercase DNS characters without '--'")?;
    Ok(value)
}

fn insert_email(emails: &mut Vec<String>, email: String) {
    let Some(email) = normalize_optional_email(Some(email)) else {
        return;
    };
    if !emails.iter().any(|item| item.eq_ignore_ascii_case(&email)) {
        emails.push(email);
    }
}

fn normalize_email_list(values: &[String], owner_email: Option<&str>) -> Vec<String> {
    let owner = owner_email.map(|value| value.trim().to_ascii_lowercase());
    let mut seen = BTreeSet::new();
    let mut emails = Vec::new();
    for value in values {
        let email = value.trim().to_ascii_lowercase();
        if email.is_empty() || owner.as_deref() == Some(email.as_str()) {
            continue;
        }
        if seen.insert(email.clone()) {
            emails.push(email);
        }
    }
    emails
}

fn normalize_access_by_app(
    access_by_app: BTreeMap<String, ShareAppAccess>,
    owner_email: Option<&str>,
) -> BTreeMap<String, ShareAppAccess> {
    let mut normalized = BTreeMap::new();
    for (app, mut access) in access_by_app {
        let app = app.trim().to_ascii_lowercase();
        if app.is_empty() {
            continue;
        }
        access.market_access_mode = normalize_non_empty(access.market_access_mode, "selected");
        access.shared_with_emails = normalize_email_list(&access.shared_with_emails, owner_email);
        normalized.insert(app, access);
    }
    normalized
}

fn normalize_app_settings(
    app_settings: BTreeMap<String, ShareAppSettings>,
    owner_email: Option<&str>,
) -> BTreeMap<String, ShareAppSettings> {
    let mut normalized = BTreeMap::new();
    for (app, mut setting) in app_settings {
        let app = app.trim().to_ascii_lowercase();
        if app.is_empty() {
            continue;
        }
        setting.for_sale = normalize_router_for_sale_setting(&setting.for_sale);
        setting.sale_market_kind = normalize_non_empty(setting.sale_market_kind, "token");
        setting.market_access_mode = normalize_non_empty(setting.market_access_mode, "selected");
        setting.shared_with_emails = normalize_email_list(&setting.shared_with_emails, owner_email);
        normalized.insert(app, setting);
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_share_store_defaults_router_delete_outbox() {
        let store: ShareStore = serde_json::from_str(r#"{"shares":[]}"#).unwrap();
        assert!(store.pending_router_deletes.is_empty());
        assert!(store.router_share_prune_marker.is_none());

        let tombstone: ShareDeleteTombstone = serde_json::from_str(
            r#"{"shareId":"legacy","operationId":"legacy-op","createdAtMs":1}"#,
        )
        .unwrap();
        assert!(tombstone.has_legacy_router_target());
    }

    #[test]
    fn account_metadata_refresh_follows_effective_share_bindings() {
        let mut input = codex_share_input("share-multi-app");
        input.provider_id = "codex-managed".to_string();
        input.provider_type = ProviderType::CodexOAuth;
        input.bindings = vec![ShareBinding {
            app: AppKind::Codex,
            provider_id: "codex-managed".to_string(),
            provider_type: ProviderType::CodexOAuth,
        }];
        let mut store = ShareStore::default();
        let original = store.upsert(input).unwrap();
        let provider_keys = BTreeSet::from([(AppKind::Codex, "codex-managed".to_string())]);

        let updated = store.refresh_runtime_snapshots_for_providers(
            &provider_keys,
            &ProviderStore::default(),
            None,
            &UsageStore::default(),
        );

        assert_eq!(updated, vec!["share-multi-app"]);
        assert!(store.get("share-multi-app").unwrap().config_revision > original.config_revision);
    }

    #[test]
    fn subscription_expiry_fingerprint_ignores_volatile_remaining_time() {
        let first = json!({
            "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
            "subscriptionRemainingMs": 1000,
            "updatedAtMs": 10,
            "upstreamProvider": {
                "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
                "quota": {"subscriptionPeriodEnd": "2026-08-10T23:59:59Z"}
            }
        });
        let same_expiry = json!({
            "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
            "subscriptionRemainingMs": 1,
            "updatedAtMs": 20,
            "upstreamProvider": {
                "subscriptionExpiresAt": "2026-08-10T23:59:59Z",
                "quota": {"subscriptionPeriodEnd": "2026-08-10T23:59:59Z"}
            }
        });
        let next_period = json!({
            "subscriptionExpiresAt": "2026-09-10T23:59:59Z",
            "subscriptionRemainingMs": 1000,
            "updatedAtMs": 30,
            "upstreamProvider": {
                "subscriptionExpiresAt": "2026-09-10T23:59:59Z",
                "quota": {"subscriptionPeriodEnd": "2026-09-10T23:59:59Z"}
            }
        });

        assert_eq!(
            subscription_expiry_fingerprint(Some(&first)),
            subscription_expiry_fingerprint(Some(&same_expiry))
        );
        assert_ne!(
            subscription_expiry_fingerprint(Some(&first)),
            subscription_expiry_fingerprint(Some(&next_period))
        );
    }

    #[test]
    fn router_delete_tombstone_normalizes_bound_target() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("share-targeted")).unwrap();

        let tombstone = store
            .delete_for_router_target(
                "share-targeted",
                " https://router.example.test/api/// ",
                " installation-a ",
            )
            .unwrap();

        assert_eq!(
            tombstone.router_api_base.as_deref(),
            Some("https://router.example.test/api")
        );
        assert_eq!(tombstone.installation_id.as_deref(), Some("installation-a"));
        assert!(
            tombstone.router_target_matches("https://router.example.test/api/", "installation-a")
        );
    }

    #[test]
    fn router_share_prune_marker_normalizes_target_and_changes_with_installation() {
        let mut store = ShareStore::default();
        assert!(store.mark_router_share_prune_applied(
            " https://router.example.test/api/// ",
            " installation-a "
        ));
        assert!(store
            .router_share_prune_applied_for("https://router.example.test/api", "installation-a"));
        assert!(!store
            .mark_router_share_prune_applied("https://router.example.test/api/", "installation-a"));

        assert!(!store
            .router_share_prune_applied_for("https://router.example.test/api", "installation-b"));
        assert!(store
            .mark_router_share_prune_applied("https://router.example.test/api", "installation-b"));
    }

    #[test]
    fn recreating_same_share_id_cancels_pending_router_delete() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("share-recreated")).unwrap();
        let tombstone = store.delete("share-recreated").unwrap();
        assert!(store
            .pending_router_delete("share-recreated", &tombstone.operation_id)
            .is_some());

        let recreated = store.upsert(codex_share_input("share-recreated")).unwrap();

        assert!(store.pending_router_deletes.is_empty());
        assert!(recreated.config_revision > recreated.router_synced_revision);
    }

    fn codex_share_input(id: &str) -> UpsertShareInput {
        UpsertShareInput {
            id: Some(id.to_string()),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: "p1".to_string(),
            provider_type: ProviderType::Codex,
            display_name: None,
            enabled: None,
            status: None,
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: None,
            acl: None,
            token_limit: None,
            parallel_limit: None,
            expires_at: None,
            for_sale: Some(true),
            free_access: None,
            sale_market_kind: Some("share".to_string()),
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            auto_start: None,
            description: None,
            bindings: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
            user_grants: BTreeMap::new(),
        }
    }

    fn codex_app_settings(emails: Vec<&str>) -> BTreeMap<String, ShareAppSettings> {
        let mut app_settings = BTreeMap::new();
        app_settings.insert(
            "codex".to_string(),
            ShareAppSettings {
                for_sale: "Yes".to_string(),
                sale_market_kind: "share".to_string(),
                market_access_mode: "selected".to_string(),
                shared_with_emails: emails.into_iter().map(str::to_string).collect(),
                token_limit: 5000,
                parallel_limit: 2,
                expires_at: "1893456000".to_string(),
            },
        );
        app_settings
    }

    fn test_timestamp_ms(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid UTC test timestamp")
            .timestamp_millis()
    }

    #[test]
    fn new_owner_and_shareto_grants_snapshot_total_share_limits() {
        let expires_at = test_timestamp_ms(2030, 1, 1, 0, 0);
        let mut input = codex_share_input("grant-defaults");
        input.token_limit = Some(50_000);
        input.parallel_limit = Some(7);
        input.expires_at = Some(expires_at);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["User@Example.com".to_string()],
            ..ShareAcl::default()
        });

        let share = ShareStore::default().upsert(input).unwrap();
        let expected = ShareUserPolicy {
            parallel_limit: Some(7),
            token_limit: Some(50_000),
            token_period: ShareTokenPeriod::Lifetime,
            expires_at: Some(expires_at),
        };

        assert_eq!(share.user_grants["owner@example.com"].policy, expected);
        assert_eq!(share.user_grants["user@example.com"].policy, expected);
        assert_eq!(share.user_grants["owner@example.com"].role, "owner");
        assert_eq!(share.user_grants["user@example.com"].role, "shareto");
    }

    #[test]
    fn user_usage_uses_utc_day_week_and_calendar_month_boundaries() {
        let sunday = test_timestamp_ms(2026, 7, 19, 23, 59);
        let monday = test_timestamp_ms(2026, 7, 20, 0, 0);
        let january_end = test_timestamp_ms(2027, 1, 31, 23, 59);
        let february = test_timestamp_ms(2027, 2, 1, 0, 0);
        let mut usage = ShareUserUsage::default();

        usage.record(11, sunday);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Lifetime, monday), 11);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Day, sunday), 11);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Day, monday), 0);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Week, sunday), 11);
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Week, monday), 0);

        usage.record(13, january_end);
        assert_eq!(
            usage.tokens_for(ShareTokenPeriod::CalendarMonth, january_end),
            13
        );
        assert_eq!(
            usage.tokens_for(ShareTokenPeriod::CalendarMonth, february),
            0
        );
        assert_eq!(usage.tokens_for(ShareTokenPeriod::Lifetime, february), 24);
    }

    #[test]
    fn user_quota_isolated_from_other_users_and_total_quota_remains_authoritative() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("user-quota");
        input.token_limit = Some(100);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec![
                "alice@example.com".to_string(),
                "bob@example.com".to_string(),
            ],
            ..ShareAcl::default()
        });
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        store
            .shares
            .first_mut()
            .unwrap()
            .user_grants
            .get_mut("alice@example.com")
            .unwrap()
            .policy = ShareUserPolicy {
            token_limit: Some(5),
            token_period: ShareTokenPeriod::Day,
            ..ShareUserPolicy::default()
        };

        store.record_user_invocation_result("user-quota", Some("alice@example.com"), 5, now);
        assert_eq!(
            store
                .validate_for_invocation("user-quota", Some("alice@example.com"), now)
                .unwrap_err()
                .reason,
            ShareRejectReason::UserExhausted
        );
        assert!(store
            .validate_for_invocation("user-quota", Some("bob@example.com"), now)
            .is_ok());
        assert_eq!(store.get("user-quota").unwrap().status, "active");

        let reset = store.reset_usage("user-quota").unwrap();
        assert_eq!(
            reset.user_grants["alice@example.com"]
                .usage
                .tokens_for(ShareTokenPeriod::Lifetime, now),
            0
        );
        assert!(store
            .validate_for_invocation("user-quota", Some("alice@example.com"), now)
            .is_ok());

        store.record_user_invocation_result("user-quota", Some("bob@example.com"), 95, now);
        store.record_user_invocation_result("user-quota", Some("alice@example.com"), 5, now);
        let rejection = store
            .validate_for_invocation("user-quota", Some("bob@example.com"), now)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Inactive);
        assert_eq!(store.get("user-quota").unwrap().status, "exhausted");
    }

    #[test]
    fn app_scoped_acl_grants_revoke_and_restore_policy_history() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("market-user")).unwrap();

        let added = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(added.user_grants["buyer@example.com"].active);
        store.record_user_invocation_result("market-user", Some("buyer@example.com"), 17, now);

        let revoked = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(Vec::new())),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(!revoked.user_grants["buyer@example.com"].active);
        assert_eq!(
            revoked.user_grants["buyer@example.com"]
                .usage
                .tokens_for(ShareTokenPeriod::Lifetime, now),
            17
        );

        let restored = store
            .apply_settings_patch(
                "market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert!(restored.user_grants["buyer@example.com"].active);
        assert_eq!(
            restored.user_grants["buyer@example.com"]
                .usage
                .tokens_for(ShareTokenPeriod::Lifetime, now),
            17
        );
    }

    #[test]
    fn invocation_completion_records_usage_after_grant_is_revoked() {
        let now = test_timestamp_ms(2026, 7, 19, 12, 0);
        let mut input = codex_share_input("revoked-inflight-user");
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["user@example.com".to_string()],
            ..ShareAcl::default()
        });
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        store
            .apply_settings_patch(
                "revoked-inflight-user",
                ShareSettingsPatch {
                    shared_with_emails: Some(Vec::new()),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        store.record_user_invocation_result(
            "revoked-inflight-user",
            Some("user@example.com"),
            23,
            now,
        );

        let grant = &store.get("revoked-inflight-user").unwrap().user_grants["user@example.com"];
        assert!(!grant.active);
        assert_eq!(grant.usage.tokens_for(ShareTokenPeriod::Lifetime, now), 23);
    }

    #[test]
    fn explicit_user_policies_fill_new_acl_users_with_default_policy() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(codex_share_input("explicit-market-user"))
            .unwrap();
        let owner_grant = share.user_grants["owner@example.com"].clone();

        let updated = store
            .apply_settings_patch(
                "explicit-market-user",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
                    user_grants: Some(BTreeMap::from([(
                        "owner@example.com".to_string(),
                        owner_grant,
                    )])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert!(updated.user_grants["buyer@example.com"].active);
        assert_eq!(
            updated.user_grants["buyer@example.com"].policy,
            default_user_policy(&updated)
        );
    }

    #[test]
    fn apply_settings_patch_persists_free_for_sale_mode() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(codex_share_input("share-free"))
            .expect("upsert");
        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    for_sale: Some("Free".to_string()),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("apply free");
        assert!(updated.free_access);
        assert!(!updated.for_sale);
        assert_eq!(share_router_for_sale_label(&updated), "Free");
    }

    #[test]
    fn apply_settings_patch_persists_valid_token_market_pricing() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("share-priced");
        input.sale_market_kind = Some("token".to_string());
        let share = store.upsert(input).expect("upsert");

        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    for_sale_official_price_percent_by_app: Some(BTreeMap::from([(
                        "codex".to_string(),
                        80,
                    )])),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("apply pricing");

        assert_eq!(
            updated.for_sale_official_price_percent_by_app,
            BTreeMap::from([("codex".to_string(), 80)])
        );
    }

    #[test]
    fn apply_settings_patch_rejects_invalid_pricing_without_partial_mutation() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("share-invalid-price");
        input.sale_market_kind = Some("token".to_string());
        let share = store.upsert(input).expect("upsert");

        for pricing in [
            BTreeMap::from([("codex".to_string(), 0)]),
            BTreeMap::from([("codex".to_string(), 101)]),
            BTreeMap::from([("claude".to_string(), 80)]),
        ] {
            let result = store.apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    description: Some(Some("must not persist".to_string())),
                    for_sale_official_price_percent_by_app: Some(pricing),
                    ..ShareSettingsPatch::default()
                },
            );
            assert!(matches!(result, Err(SharePatchError::Invalid(_))));
            let stored = store.get(&share.id).expect("stored share");
            assert_eq!(stored.description, None);
            assert!(stored.for_sale_official_price_percent_by_app.is_empty());
        }
    }

    #[test]
    fn sale_mode_transition_clears_pricing_and_rejects_contradictory_payload() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("share-price-transition");
        input.sale_market_kind = Some("token".to_string());
        input.for_sale_official_price_percent_by_app = BTreeMap::from([("codex".to_string(), 75)]);
        let share = store.upsert(input).expect("upsert");

        let rejected = store.apply_settings_patch(
            &share.id,
            ShareSettingsPatch {
                for_sale: Some("No".to_string()),
                for_sale_official_price_percent_by_app: Some(BTreeMap::from([(
                    "codex".to_string(),
                    75,
                )])),
                ..ShareSettingsPatch::default()
            },
        );
        assert!(matches!(rejected, Err(SharePatchError::Invalid(_))));
        assert_eq!(
            store
                .get(&share.id)
                .expect("stored share")
                .for_sale_official_price_percent_by_app
                .get("codex"),
            Some(&75)
        );

        let updated = store
            .apply_settings_patch(
                &share.id,
                ShareSettingsPatch {
                    sale_market_kind: Some("share".to_string()),
                    ..ShareSettingsPatch::default()
                },
            )
            .expect("switch market kind");
        assert!(updated.for_sale_official_price_percent_by_app.is_empty());
    }

    #[test]
    fn upsert_share_defaults_binding_and_status() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: Some("share".to_string()),
                enabled: None,
                status: None,
                subscription_level: Some("pro".to_string()),
                account_email: Some("owner@example.com".to_string()),
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: Some(1000),
                parallel_limit: Some(2),
                expires_at: None,
                for_sale: Some(true),
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: Some(80.0),
                auto_start: Some(true),
                description: Some("test".to_string()),
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        assert_eq!(share.status, "active");
        assert!(share.enabled);
        assert_eq!(share.bindings.len(), 1);
        assert_eq!(share.bindings[0].provider_id, "p1");
        assert_eq!(share.token_limit, Some(1000));
        assert!(share.for_sale);
    }

    #[test]
    fn rejects_second_share_for_same_provider_instance() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("s1")).unwrap();
        let error = store.upsert(codex_share_input("s2")).unwrap_err();
        assert!(error.to_string().contains("provider already has share"));
    }

    #[test]
    fn allows_multiple_instances_of_same_provider_type() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("s1")).unwrap();
        let mut second = codex_share_input("s2");
        second.provider_id = "p2".to_string();
        let share = store.upsert(second).unwrap();
        assert_eq!(share.provider_type, ProviderType::Codex);
        assert_eq!(store.shares.len(), 2);
    }

    #[test]
    fn validates_share_invocation_lifecycle_limits_and_counters() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("expired")).unwrap();
        store.shares[0].expires_at = Some(999);
        let rejection = store
            .validate_for_invocation("expired", None, 1_000_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Expired);
        assert_eq!(
            rejection.formatted_message(),
            "Share has expired. Extend the share expiration or create a new share. [Expired]"
        );
        assert_eq!(store.shares[0].status, "expired");
        assert!(!store.shares[0].enabled);

        let mut paused = codex_share_input("paused");
        paused.provider_id = "p-paused".to_string();
        let _ = store.upsert(paused).unwrap();
        store.pause("paused").unwrap();
        let rejection = store
            .validate_for_invocation("paused", None, 1_000_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Inactive);
        assert!(rejection.formatted_message().contains("[Inactive]"));

        let mut limited_input = codex_share_input("limited");
        limited_input.provider_id = "p-limited".to_string();
        let _ = store.upsert(limited_input).unwrap();
        {
            let limited = store
                .shares
                .iter_mut()
                .find(|share| share.id == "limited")
                .unwrap();
            limited.token_limit = Some(10);
            limited.tokens_used = 10;
        }
        let rejection = store
            .validate_for_invocation("limited", None, 1_000_000)
            .unwrap_err();
        assert_eq!(rejection.reason, ShareRejectReason::Exhausted);
        assert_eq!(
            store
                .shares
                .iter()
                .find(|share| share.id == "limited")
                .unwrap()
                .status,
            "exhausted"
        );

        let mut record = codex_share_input("record");
        record.provider_id = "p-record".to_string();
        let _ = store.upsert(record).unwrap();
        store
            .shares
            .iter_mut()
            .find(|share| share.id == "record")
            .unwrap()
            .token_limit = Some(10);
        store.record_invocation_result("record", 4).unwrap();
        let recorded = store.record_invocation_result("record", 6).unwrap();
        assert_eq!(recorded.tokens_used, 10);
        assert_eq!(recorded.requests_count, 2);
        assert_eq!(recorded.status, "exhausted");

        let reset = store.reset_usage("record").unwrap();
        assert_eq!(reset.tokens_used, 0);
        assert_eq!(reset.requests_count, 0);
        assert_eq!(reset.status, "paused");
    }

    #[test]
    fn upsert_share_generates_default_slug_from_shared_generator() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("abc.def@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let subdomain = share.tunnel_subdomain.unwrap();
        assert!(crate::domain::router::ShareSlug::parse(&subdomain).is_ok());
    }

    #[test]
    fn updates_binding_only_when_paused() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: None,
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let error = store
            .update_binding(
                "s1",
                ShareBinding {
                    app: AppKind::Codex,
                    provider_id: "p2".to_string(),
                    provider_type: ProviderType::OpenRouter,
                },
            )
            .unwrap_err();
        assert_eq!(error, ShareUpdateError::MustBePaused);

        store.pause("s1").unwrap();
        let share = store
            .update_binding(
                "s1",
                ShareBinding {
                    app: AppKind::Codex,
                    provider_id: "p2".to_string(),
                    provider_type: ProviderType::OpenRouter,
                },
            )
            .unwrap();

        assert_eq!(share.provider_id, "p2");
        assert_eq!(share.provider_type, ProviderType::OpenRouter);
        assert_eq!(share.binding_history.len(), 1);
    }

    #[test]
    fn imports_and_replaces_acl() {
        let mut store = ShareStore::default();
        let share = Share {
            id: "s1".to_string(),
            owner_email: None,
            app: AppKind::Claude,
            provider_id: "p1".to_string(),
            provider_type: ProviderType::Claude,
            display_name: None,
            enabled: true,
            status: "active".to_string(),
            subscription_level: None,
            account_email: None,
            quota_percent: None,
            tunnel_subdomain: None,
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
            bindings: Vec::new(),
            binding_history: Vec::new(),
            runtime_snapshot: None,
            market_grant: None,
            last_error: None,
            router_last_synced_at_ms: None,
            router_last_sync_error: None,
            router_url: None,
            config_revision: 0,
            router_synced_revision: 0,
            descriptor_generation: 0,
            descriptor_fingerprint: None,
            router_synced_descriptor_generation: 0,
            router_synced_descriptor_fingerprint: None,
            user_grants: BTreeMap::new(),
        };
        assert_eq!(store.import_shares(vec![share]), 1);
        let updated = store
            .replace_acl(
                "s1",
                ShareAcl {
                    shared_with_emails: vec!["user@example.com".to_string()],
                    public_market_email: Some("market@example.com".to_string()),
                    market_access_mode: Some("selected".to_string()),
                },
            )
            .unwrap();

        assert_eq!(updated.acl.shared_with_emails, vec!["user@example.com"]);
        assert_eq!(
            updated.acl.public_market_email.as_deref(),
            Some("market@example.com")
        );
    }

    #[test]
    fn applies_share_market_app_settings_patch() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: Some(true),
                free_access: None,
                sale_market_kind: Some("share".to_string()),
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let mut app_settings = BTreeMap::new();
        app_settings.insert(
            "codex".to_string(),
            ShareAppSettings {
                for_sale: "Yes".to_string(),
                sale_market_kind: "share".to_string(),
                market_access_mode: "selected".to_string(),
                shared_with_emails: vec![
                    "buyer@example.com".to_string(),
                    "OWNER@example.com".to_string(),
                    "buyer@example.com".to_string(),
                ],
                token_limit: 5000,
                parallel_limit: 2,
                expires_at: "1893456000".to_string(),
            },
        );

        let share = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(app_settings),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        let setting = share.app_settings.get("codex").unwrap();
        assert_eq!(setting.sale_market_kind, "share");
        assert_eq!(setting.shared_with_emails, vec!["buyer@example.com"]);

        let descriptor = crate::domain::sharing::router_contract::descriptor_for_share(
            &share,
            &ProviderStore::default(),
        );
        assert_eq!(
            descriptor
                .app_settings
                .get("codex")
                .unwrap()
                .sale_market_kind,
            "share"
        );
    }

    #[test]
    fn authorize_share_market_marks_app_settings_for_sale() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: Some(json!({"shareId": "s1"})),
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();
        let stored = store
            .shares
            .iter_mut()
            .find(|share| share.id == "s1")
            .unwrap();
        stored.acl.shared_with_emails = vec![
            "old-market@example.com".to_string(),
            "user@example.com".to_string(),
        ];
        stored.access_by_app.insert(
            "codex".to_string(),
            ShareAppAccess {
                shared_with_emails: vec![
                    "old-market@example.com".to_string(),
                    "buyer@example.com".to_string(),
                ],
                market_access_mode: "all".to_string(),
            },
        );
        stored.app_settings.insert(
            "codex".to_string(),
            ShareAppSettings {
                for_sale: "No".to_string(),
                sale_market_kind: "token".to_string(),
                market_access_mode: "all".to_string(),
                shared_with_emails: vec![
                    "old-market@example.com".to_string(),
                    "buyer@example.com".to_string(),
                ],
                token_limit: 5000,
                parallel_limit: 2,
                expires_at: "1893456000".to_string(),
            },
        );

        let mut public_markets = BTreeSet::new();
        public_markets.insert("old-market@example.com".to_string());
        let share = store
            .authorize_share_market("s1", "market@example.com".to_string(), &public_markets)
            .unwrap();

        assert!(share.for_sale);
        assert_eq!(share.sale_market_kind, "share");
        assert_eq!(
            share.acl.shared_with_emails,
            vec!["user@example.com", "market@example.com"]
        );
        let access = share.access_by_app.get("codex").unwrap();
        assert_eq!(access.market_access_mode, "selected");
        assert_eq!(
            access.shared_with_emails,
            vec!["buyer@example.com", "market@example.com"]
        );
        let settings = share.app_settings.get("codex").unwrap();
        assert_eq!(settings.for_sale, "Yes");
        assert_eq!(settings.sale_market_kind, "share");
        assert_eq!(settings.market_access_mode, "selected");
        assert_eq!(
            settings.shared_with_emails,
            vec!["buyer@example.com", "market@example.com"]
        );
        assert_eq!(
            share
                .runtime_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pointer("/appSettings/codex/forSale"))
                .and_then(Value::as_str),
            Some("Yes")
        );
    }

    #[test]
    fn mark_router_sync_records_success_and_failure_details() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: None,
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let revision = store.get("s1").unwrap().config_revision;
        store.mark_router_sync(
            "s1",
            revision,
            Some("https://router.example".to_string()),
            Ok(123),
        );
        let share = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(share.router_last_synced_at_ms, Some(123));
        assert_eq!(share.router_url.as_deref(), Some("https://router.example"));
        assert_eq!(share.router_last_sync_error, None);

        store.mark_router_sync(
            "s1",
            revision,
            Some("https://router.example".to_string()),
            Err("failed".to_string()),
        );
        let share = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(share.router_last_synced_at_ms, Some(123));
        assert_eq!(share.router_last_sync_error, None);

        let newer = store.pause("s1").unwrap();
        store.mark_router_sync(
            "s1",
            newer.config_revision,
            Some("https://router.example".to_string()),
            Err("failed".to_string()),
        );
        assert_eq!(
            store.get("s1").unwrap().router_last_sync_error.as_deref(),
            Some("failed")
        );
    }

    #[test]
    fn descriptor_projection_generation_and_ack_are_monotonic() {
        let mut store = ShareStore::default();
        store.upsert(codex_share_input("projection-order")).unwrap();

        let (first_generation, first_fingerprint) = store
            .prepare_descriptor_projection("projection-order", "a".repeat(64))
            .unwrap();
        assert_eq!(first_generation, 1);
        assert!(store.mark_router_descriptor_sync(
            "projection-order",
            first_generation,
            &first_fingerprint,
            1,
            Some("https://router.example".to_string()),
            Ok(100),
        ));

        let (second_generation, second_fingerprint) = store
            .prepare_descriptor_projection("projection-order", "b".repeat(64))
            .unwrap();
        assert_eq!(second_generation, 2);
        assert!(!store.mark_router_descriptor_sync(
            "projection-order",
            first_generation,
            &first_fingerprint,
            1,
            Some("https://router.example".to_string()),
            Ok(200),
        ));
        let share = store.get("projection-order").unwrap();
        assert_eq!(share.router_synced_descriptor_generation, first_generation);
        assert!(store.descriptor_projection_pending(share));

        assert!(store.mark_router_descriptor_sync(
            "projection-order",
            second_generation,
            &second_fingerprint,
            1,
            Some("https://router.example".to_string()),
            Ok(300),
        ));
        assert!(!store.descriptor_projection_pending(store.get("projection-order").unwrap()));
    }

    #[test]
    fn share_market_grant_status_is_persisted_for_web_display() {
        let mut store = ShareStore::default();
        let share = store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: None,
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: Some(ShareMarketGrantStatus {
                    status: "pending".to_string(),
                    grant_id: Some("grant-1".to_string()),
                    last_error: None,
                    updated_at_ms: Some(123),
                }),
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        assert_eq!(
            share
                .market_grant
                .as_ref()
                .map(|grant| grant.status.as_str()),
            Some("pending")
        );

        let providers = ProviderStore::default();
        let usage = UsageStore::default();
        let snapshot = runtime_snapshot_for_share(&share, &providers, None, &usage);

        assert_eq!(
            snapshot
                .pointer("/marketGrant/status")
                .and_then(Value::as_str),
            Some("pending")
        );
        assert_eq!(
            snapshot
                .pointer("/marketGrant/grantId")
                .and_then(Value::as_str),
            Some("grant-1")
        );
    }

    #[test]
    fn runtime_snapshot_includes_model_health_summary() {
        let mut store = ShareStore::default();
        let share = store.upsert(codex_share_input("s1")).unwrap();
        let providers = ProviderStore::default();
        let usage = UsageStore::default();

        let snapshot = runtime_snapshot_for_share(&share, &providers, None, &usage);

        assert!(snapshot.pointer("/modelHealth/codex").is_some());
        assert_eq!(
            snapshot
                .pointer("/modelHealth/codex")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(0)
        );
    }

    #[test]
    fn updates_market_grant_and_keeps_existing_snapshot_consistent() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: None,
                app: AppKind::Codex,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Codex,
                display_name: None,
                enabled: None,
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: None,
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: Some(json!({
                    "shareId": "s1",
                    "marketGrant": {"status": "pending"}
                })),
                market_grant: Some(ShareMarketGrantStatus {
                    status: "pending".to_string(),
                    grant_id: None,
                    last_error: None,
                    updated_at_ms: Some(100),
                }),
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let updated = store
            .update_market_grant(
                "s1",
                Some(ShareMarketGrantStatus {
                    status: "applied".to_string(),
                    grant_id: Some("grant-2".to_string()),
                    last_error: None,
                    updated_at_ms: Some(200),
                }),
            )
            .unwrap();
        assert_eq!(
            updated
                .market_grant
                .as_ref()
                .map(|grant| grant.status.as_str()),
            Some("applied")
        );
        assert_eq!(
            updated
                .runtime_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pointer("/marketGrant/grantId"))
                .and_then(Value::as_str),
            Some("grant-2")
        );

        let cleared = store.update_market_grant("s1", None).unwrap();
        assert!(cleared.market_grant.is_none());
        assert!(cleared
            .runtime_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.get("marketGrant"))
            .is_none());
    }

    #[test]
    fn share_market_grant_app_settings_add_and_revoke_target_buyer() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("s1")).unwrap();

        let added = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec![
                        "buyer@example.com",
                        "BUYER@example.com",
                        "owner@example.com",
                    ])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            added
                .app_settings
                .get("codex")
                .map(|settings| settings.shared_with_emails.clone()),
            Some(vec!["buyer@example.com".to_string()])
        );

        let revoked = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(Vec::new())),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        assert_eq!(
            revoked
                .app_settings
                .get("codex")
                .map(|settings| settings.shared_with_emails.clone()),
            Some(Vec::new())
        );
    }

    #[test]
    fn share_market_grant_duplicate_status_update_is_idempotent_for_display() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("s1")).unwrap();

        let first = ShareMarketGrantStatus {
            status: "applied".to_string(),
            grant_id: Some("grant-1".to_string()),
            last_error: None,
            updated_at_ms: Some(100),
        };
        store
            .update_market_grant("s1", Some(first.clone()))
            .unwrap();
        let second = store
            .update_market_grant("s1", Some(first.clone()))
            .unwrap();

        assert_eq!(
            second
                .market_grant
                .as_ref()
                .map(|grant| grant.status.as_str()),
            Some(first.status.as_str())
        );
        assert_eq!(
            second
                .market_grant
                .as_ref()
                .and_then(|grant| grant.grant_id.as_deref()),
            first.grant_id.as_deref()
        );
        assert_eq!(
            second
                .runtime_snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.pointer("/marketGrant/status"))
                .and_then(Value::as_str),
            None
        );
    }

    #[test]
    fn share_market_grant_rejected_error_is_display_only() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("s1")).unwrap();
        store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer@example.com"])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        let errored = store
            .update_market_grant(
                "s1",
                Some(ShareMarketGrantStatus {
                    status: "error".to_string(),
                    grant_id: Some("edit-1".to_string()),
                    last_error: Some("invalid patch".to_string()),
                    updated_at_ms: Some(200),
                }),
            )
            .unwrap();

        assert_eq!(
            errored
                .app_settings
                .get("codex")
                .map(|settings| settings.shared_with_emails.clone()),
            Some(vec!["buyer@example.com".to_string()])
        );
        assert_eq!(
            errored
                .market_grant
                .as_ref()
                .and_then(|grant| grant.last_error.as_deref()),
            Some("invalid patch")
        );
    }

    #[test]
    fn sequential_share_market_patches_are_deterministic() {
        let mut store = ShareStore::default();
        let _ = store.upsert(codex_share_input("s1")).unwrap();

        store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec!["buyer-a@example.com"])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();
        let final_share = store
            .apply_settings_patch(
                "s1",
                ShareSettingsPatch {
                    app_settings: Some(codex_app_settings(vec![
                        "buyer-b@example.com",
                        "buyer-b@example.com",
                        "OWNER@example.com",
                    ])),
                    ..ShareSettingsPatch::default()
                },
            )
            .unwrap();

        assert_eq!(
            final_share
                .app_settings
                .get("codex")
                .map(|settings| settings.shared_with_emails.clone()),
            Some(vec!["buyer-b@example.com".to_string()])
        );
    }

    #[test]
    fn bind_owner_renormalizes_acl() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: None,
                enabled: Some(true),
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: Some(ShareAcl {
                    shared_with_emails: vec![
                        "owner@example.com".to_string(),
                        "buyer@example.com".to_string(),
                    ],
                    public_market_email: None,
                    market_access_mode: Some("selected".to_string()),
                }),
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let updated = store
            .bind_all_to_client_owner("new-owner@example.com")
            .unwrap();
        let updated = &updated[0];
        assert_eq!(
            updated.owner_email.as_deref(),
            Some("new-owner@example.com")
        );
        assert_eq!(
            updated.acl.shared_with_emails,
            vec![
                "owner@example.com".to_string(),
                "buyer@example.com".to_string(),
            ]
        );
    }

    #[test]
    fn bind_all_to_client_owner_preserves_previous_owner_access_and_is_idempotent() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("owner-bind");
        input.owner_email = Some("previous@example.com".to_string());
        input.access_by_app.insert(
            "codex".to_string(),
            ShareAppAccess {
                shared_with_emails: vec!["buyer@example.com".to_string()],
                market_access_mode: "selected".to_string(),
            },
        );
        input.app_settings = codex_app_settings(vec!["buyer@example.com"]);
        store.upsert(input).unwrap();

        let updated = store
            .bind_all_to_client_owner("client@example.com")
            .unwrap();
        assert_eq!(updated.len(), 1);
        let share = store.get("owner-bind").unwrap();
        assert_eq!(share.owner_email.as_deref(), Some("client@example.com"));
        assert!(share
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "previous@example.com"));
        assert!(share.access_by_app["codex"]
            .shared_with_emails
            .iter()
            .any(|email| email == "previous@example.com"));
        assert!(share.app_settings["codex"]
            .shared_with_emails
            .iter()
            .any(|email| email == "previous@example.com"));
        assert_eq!(
            share
                .user_grants
                .values()
                .filter(|grant| grant.role == "owner")
                .count(),
            1
        );
        assert_eq!(share.user_grants["client@example.com"].role, "owner");
        let previous_owner = &share.user_grants["previous@example.com"];
        assert_eq!(previous_owner.role, "shareto");
        assert!(previous_owner.active);
        let revision = share.config_revision;
        assert!(store
            .bind_all_to_client_owner("client@example.com")
            .unwrap()
            .is_empty());
        assert!(store.migrate_user_grants_from_acl().is_empty());
        assert_eq!(store.get("owner-bind").unwrap().config_revision, revision);
    }

    #[test]
    fn old_share_json_migrates_user_grants_once_and_marks_router_sync_pending() {
        let expires_at = test_timestamp_ms(2030, 1, 1, 0, 0);
        let mut input = codex_share_input("legacy-user-grants");
        input.token_limit = Some(50_000);
        input.parallel_limit = Some(7);
        input.expires_at = Some(expires_at);
        input.acl = Some(ShareAcl {
            shared_with_emails: vec!["user@example.com".to_string()],
            ..ShareAcl::default()
        });
        let mut store = ShareStore::default();
        store.upsert(input).unwrap();
        let mut value = serde_json::to_value(store).unwrap();
        value["shares"][0]
            .as_object_mut()
            .unwrap()
            .remove("userGrants");
        let mut loaded: ShareStore = serde_json::from_value(value).unwrap();
        let previous_revision = loaded.shares[0].config_revision;

        let migrated = loaded.migrate_user_grants_from_acl();

        assert_eq!(migrated.len(), 1);
        let share = loaded.get("legacy-user-grants").unwrap();
        assert_eq!(share.config_revision, previous_revision + 1);
        assert_eq!(share.router_synced_revision, 0);
        let expected = ShareUserPolicy {
            parallel_limit: Some(7),
            token_limit: Some(50_000),
            token_period: ShareTokenPeriod::Lifetime,
            expires_at: Some(expires_at),
        };
        assert_eq!(share.user_grants["owner@example.com"].policy, expected);
        assert_eq!(share.user_grants["user@example.com"].policy, expected);
        assert!(loaded.migrate_user_grants_from_acl().is_empty());
        assert_eq!(
            loaded.get("legacy-user-grants").unwrap().config_revision,
            previous_revision + 1
        );
    }

    #[test]
    fn grant_migration_repairs_stale_owner_role_without_rebinding_owner() {
        let mut store = ShareStore::default();
        let mut input = codex_share_input("stale-owner-role");
        input.owner_email = Some("previous@example.com".to_string());
        store.upsert(input).unwrap();

        let share = store.shares.first_mut().unwrap();
        share.owner_email = Some("client@example.com".to_string());
        insert_email(
            &mut share.acl.shared_with_emails,
            "previous@example.com".to_string(),
        );
        let previous_revision = share.config_revision;

        let migrated = store.migrate_user_grants_from_acl();

        assert_eq!(migrated.len(), 1);
        let share = store.get("stale-owner-role").unwrap();
        assert_eq!(share.config_revision, previous_revision + 1);
        assert_eq!(
            share
                .user_grants
                .values()
                .filter(|grant| grant.role == "owner")
                .count(),
            1
        );
        assert_eq!(share.user_grants["client@example.com"].role, "owner");
        assert_eq!(share.user_grants["previous@example.com"].role, "shareto");
        assert!(share.user_grants["previous@example.com"].active);
        assert!(store.migrate_user_grants_from_acl().is_empty());
    }

    #[test]
    fn bind_all_to_client_owner_discards_invalid_previous_owner() {
        let mut store = ShareStore::default();
        store
            .upsert(codex_share_input("invalid-owner-bind"))
            .unwrap();
        store.shares[0].owner_email = Some("invalid-owner".to_string());
        store
            .bind_all_to_client_owner("client@example.com")
            .unwrap();
        let share = store.get("invalid-owner-bind").unwrap();
        assert_eq!(share.owner_email.as_deref(), Some("client@example.com"));
        assert!(!share
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "invalid-owner"));
    }

    #[test]
    fn bind_owner_updates_all_shares() {
        let mut store = ShareStore::default();
        let mut first = codex_share_input("s1");
        first.acl = Some(ShareAcl {
            shared_with_emails: vec![
                "new-owner@example.com".to_string(),
                "buyer@example.com".to_string(),
            ],
            public_market_email: None,
            market_access_mode: Some("selected".to_string()),
        });
        first.app_settings = codex_app_settings(vec!["new-owner@example.com", "buyer@example.com"]);
        let _ = store.upsert(first).unwrap();
        let mut second = codex_share_input("s2");
        second.provider_id = "p2".to_string();
        let _ = store.upsert(second).unwrap();
        let mut other = codex_share_input("s3");
        other.provider_id = "p3".to_string();
        other.owner_email = Some("other@example.com".to_string());
        let _ = store.upsert(other).unwrap();

        let updated = store
            .bind_all_to_client_owner("New-Owner@Example.com")
            .unwrap();

        assert_eq!(updated.len(), 3);
        assert_eq!(
            store
                .shares
                .iter()
                .filter(|share| share.owner_email.as_deref() == Some("new-owner@example.com"))
                .count(),
            3
        );
        assert_eq!(
            store
                .shares
                .iter()
                .find(|share| share.id == "s3")
                .and_then(|share| share.owner_email.as_deref()),
            Some("new-owner@example.com")
        );
        let first = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(
            first.acl.shared_with_emails,
            vec![
                "buyer@example.com".to_string(),
                "owner@example.com".to_string()
            ]
        );
        assert_eq!(
            first
                .app_settings
                .get("codex")
                .map(|settings| settings.shared_with_emails.clone()),
            Some(vec![
                "buyer@example.com".to_string(),
                "owner@example.com".to_string()
            ])
        );
    }

    #[test]
    fn binding_owner_demotes_previous_owner() {
        let mut store = ShareStore::default();
        store
            .upsert(UpsertShareInput {
                id: Some("s1".to_string()),
                owner_email: Some("owner@example.com".to_string()),
                app: AppKind::Claude,
                provider_id: "p1".to_string(),
                provider_type: ProviderType::Claude,
                display_name: None,
                enabled: Some(true),
                status: None,
                subscription_level: None,
                account_email: None,
                quota_percent: None,
                tunnel_subdomain: None,
                acl: Some(ShareAcl {
                    shared_with_emails: vec!["buyer@example.com".to_string()],
                    public_market_email: None,
                    market_access_mode: Some("selected".to_string()),
                }),
                token_limit: None,
                parallel_limit: None,
                expires_at: None,
                for_sale: None,
                free_access: None,
                sale_market_kind: None,
                access_by_app: BTreeMap::new(),
                app_settings: BTreeMap::new(),
                for_sale_official_price_percent_by_app: BTreeMap::new(),
                official_price_percent: None,
                auto_start: None,
                description: None,
                bindings: Vec::new(),
                runtime_snapshot: None,
                market_grant: None,
                user_grants: BTreeMap::new(),
            })
            .unwrap();

        let updated = store.bind_all_to_client_owner("buyer@example.com").unwrap();
        let updated = &updated[0];
        assert_eq!(updated.owner_email.as_deref(), Some("buyer@example.com"));
        assert!(updated
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "owner@example.com"));
        assert!(!updated
            .acl
            .shared_with_emails
            .iter()
            .any(|email| email == "buyer@example.com"));
    }
}
