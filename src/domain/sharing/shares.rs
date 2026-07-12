use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::Context;
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::store::ProviderStore;
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareAppAccess, ShareAppSettings,
    ShareSettingsPatch,
};
use crate::domain::usage::store::UsageStore;
use crate::infra::time::now_ms;

const SHARES_FILE_NAME: &str = "shares.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStore {
    #[serde(default)]
    pub shares: Vec<Share>,
    #[serde(default)]
    pub router_registered: bool,
    #[serde(default)]
    pub last_router_error: Option<String>,
    #[serde(default)]
    pub last_router_heartbeat_ms: Option<u128>,
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
        let owner_email = input.owner_email.clone();
        let tunnel_subdomain = input
            .tunnel_subdomain
            .clone()
            .or_else(|| owner_email.as_deref().map(default_share_subdomain));

        let existing_id = input.id.clone().or_else(|| {
            self.shares
                .iter()
                .find(|item| item.app == app && item.provider_id == provider_id)
                .map(|item| item.id.clone())
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
        ) = preserved.unwrap_or((0, 0, Vec::new(), 0, None, None, None, None));
        let created_at_ms = if created_at_ms > 0 {
            created_at_ms
        } else {
            crate::infra::time::now_ms()
        };

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
        };

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

    pub fn delete(&mut self, share_id: &str) -> bool {
        let before = self.shares.len();
        self.shares.retain(|item| item.id != share_id);
        self.shares.len() != before
    }

    pub fn pause(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.enabled = false;
        share.status = "paused".to_string();
        Some(share.clone())
    }

    pub fn resume(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.enabled = true;
        share.status = "active".to_string();
        share.last_error = None;
        Some(share.clone())
    }

    pub fn reset_usage(&mut self, share_id: &str) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.tokens_used = 0;
        share.requests_count = 0;
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
        Some(share.clone())
    }

    pub fn validate_for_invocation(
        &mut self,
        share_id: &str,
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

        Ok(ShareInvocation {
            share_id: share.id.clone(),
            share_name: share
                .display_name
                .clone()
                .unwrap_or_else(|| share.id.clone()),
            parallel_limit: share.parallel_limit,
        })
    }

    pub fn record_invocation_result(&mut self, share_id: &str, tokens: u64) -> Option<Share> {
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

        Some(share.clone()).ok_or(ShareUpdateError::NotFound)
    }

    pub fn replace_acl(&mut self, share_id: &str, acl: ShareAcl) -> Option<Share> {
        let share = self.shares.iter_mut().find(|item| item.id == share_id)?;
        share.acl = acl;
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
                updated.push(share.clone());
            }
        }
        Ok(updated)
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
        Some(share.clone())
    }

    pub fn apply_settings_patch(
        &mut self,
        share_id: &str,
        patch: ShareSettingsPatch,
    ) -> Result<Share, SharePatchError> {
        let share = self
            .shares
            .iter_mut()
            .find(|item| item.id == share_id)
            .ok_or(SharePatchError::NotFound)?;

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
            share.for_sale = parse_router_bool(&for_sale);
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
        if let Some(auto_start) = patch.auto_start {
            share.auto_start = auto_start;
        }

        Ok(share.clone())
    }

    pub fn import_shares(&mut self, shares: Vec<Share>) -> usize {
        let mut imported = 0;
        for share in shares {
            if let Some(existing) = self.shares.iter_mut().find(|item| item.id == share.id) {
                *existing = share;
            } else {
                self.shares.push(share);
            }
            imported += 1;
        }
        imported
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
        share.status = status.to_string();
        share.enabled = status == "active";
        share.last_error = error;
        Some(share.clone())
    }

    pub fn restore_auto_start(&mut self) -> Vec<Share> {
        for share in self.shares.iter_mut().filter(|item| item.auto_start) {
            share.status = "active".to_string();
            share.enabled = true;
            share.last_error = None;
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

    pub fn mark_router_sync(
        &mut self,
        share_id: &str,
        router_url: Option<String>,
        result: Result<u128, String>,
    ) {
        let Some(share) = self.shares.iter_mut().find(|item| item.id == share_id) else {
            return;
        };
        match result {
            Ok(now) => {
                share.router_last_synced_at_ms = Some(now);
                share.router_last_sync_error = None;
                share.router_url = router_url;
            }
            Err(error) => {
                share.router_last_sync_error = Some(error);
            }
        }
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
}

impl ShareRejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "NotFound",
            Self::Inactive => "Inactive",
            Self::Expired => "Expired",
            Self::Exhausted => "Exhausted",
            Self::ParallelLimit => "ParallelLimit",
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

pub fn default_share_subdomain(owner_email: &str) -> String {
    let email_prefix = owner_email.split('@').next().unwrap_or("share");
    let prefix = email_prefix
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(5)
        .collect::<String>();
    let prefix = if prefix.is_empty() {
        "share".to_string()
    } else {
        prefix.to_ascii_lowercase()
    };
    let suffix: String = rand::thread_rng()
        .sample_iter(rand::distributions::Uniform::new_inclusive(b'a', b'z'))
        .take(5)
        .map(char::from)
        .collect();
    format!("{prefix}{suffix}")
}

fn default_share_status() -> String {
    "active".to_string()
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

fn parse_router_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "yes" | "true" | "1" | "share"
    )
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

pub fn normalize_share_subdomain(subdomain: &str) -> Result<String, &'static str> {
    let value = subdomain.trim().to_ascii_lowercase();
    if value.len() < 3 || value.len() > 63 {
        return Err("share subdomain must be 3-63 characters");
    }
    if value.starts_with('-') || value.ends_with('-') {
        return Err("share subdomain cannot start or end with '-'");
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err("share subdomain may only contain lowercase letters, digits, and '-'");
    }
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
        setting.for_sale = if parse_router_bool(&setting.for_sale) {
            "Yes".to_string()
        } else {
            "No".to_string()
        };
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
            .validate_for_invocation("expired", 1_000_000)
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
            .validate_for_invocation("paused", 1_000_000)
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
            .validate_for_invocation("limited", 1_000_000)
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
    fn upsert_share_generates_default_subdomain_from_owner_email() {
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
            })
            .unwrap();

        let subdomain = share.tunnel_subdomain.unwrap();
        assert!(subdomain.starts_with("abcde"));
        assert_eq!(subdomain.len(), 10);
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
            })
            .unwrap();

        store.mark_router_sync("s1", Some("https://router.example".to_string()), Ok(123));
        let share = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(share.router_last_synced_at_ms, Some(123));
        assert_eq!(share.router_url.as_deref(), Some("https://router.example"));
        assert_eq!(share.router_last_sync_error, None);

        store.mark_router_sync(
            "s1",
            Some("https://router.example".to_string()),
            Err("failed".to_string()),
        );
        let share = store.shares.iter().find(|share| share.id == "s1").unwrap();
        assert_eq!(share.router_last_synced_at_ms, Some(123));
        assert_eq!(share.router_last_sync_error.as_deref(), Some("failed"));
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
        assert!(store
            .bind_all_to_client_owner("client@example.com")
            .unwrap()
            .is_empty());
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
