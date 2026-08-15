use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use crate::domain::accounts::store::{
    claude_account_uuid, effective_codex_workspace_id, verified_openai_subject, AccountStore,
};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::has_authoritative_subscription_oauth_binding;
use crate::domain::providers::store::{ProviderStore, StoredProvider};

use super::shares::{Share, ShareStore};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SubscriptionIdentityKey {
    ClaudeOAuth {
        account_uuid: String,
    },
    CodexOAuth {
        subject: String,
        workspace_id: String,
    },
}

impl SubscriptionIdentityKey {
    pub fn fingerprint(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"cc-switch-subscription-identity-v1\0");
        match self {
            Self::ClaudeOAuth { account_uuid } => {
                digest.update(b"claude_oauth\0");
                digest.update(account_uuid.as_bytes());
            }
            Self::CodexOAuth {
                subject,
                workspace_id,
            } => {
                digest.update(b"codex_oauth\0");
                digest.update(subject.as_bytes());
                digest.update(b"\0");
                digest.update(workspace_id.as_bytes());
            }
        }
        hex::encode(&digest.finalize()[..12])
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SubscriptionBindingError {
    #[error("share not found: {0}")]
    ShareNotFound(String),
    #[error("share {share_id} must have between one and three valid bindings")]
    InvalidShareBinding { share_id: String },
    #[error("share {share_id} Provider is missing: {app}/{provider_id}")]
    ProviderMissing {
        share_id: String,
        app: &'static str,
        provider_id: String,
    },
    #[error("share {share_id} Provider type does not match its binding")]
    ProviderTypeMismatch { share_id: String },
    #[error("shared OAuth Provider {app}/{provider_id} requires an explicit account binding")]
    AccountBindingMissing {
        app: &'static str,
        provider_id: String,
    },
    #[error("shared OAuth Provider {app}/{provider_id} must use managed-account authentication")]
    ManagedAuthRequired {
        app: &'static str,
        provider_id: String,
    },
    #[error("shared OAuth Provider {app}/{provider_id} references a missing account")]
    AccountMissing {
        app: &'static str,
        provider_id: String,
    },
    #[error("shared OAuth Provider {app}/{provider_id} references an incompatible account")]
    AccountTypeMismatch {
        app: &'static str,
        provider_id: String,
    },
    #[error("shared OAuth Provider {app}/{provider_id} account identity is stale")]
    StaleIdentity {
        app: &'static str,
        provider_id: String,
    },
    #[error("shared OAuth Provider {app}/{provider_id} has no verified subscription identity")]
    UnverifiedIdentity {
        app: &'static str,
        provider_id: String,
    },
    #[error(
        "subscription identity {fingerprint} is already reserved by Share {existing_share_id}"
    )]
    IdentityConflict {
        fingerprint: String,
        existing_share_id: String,
        conflicting_share_id: String,
    },
    #[error("Provider {app}/{provider_id} is referenced by Share {share_id}; its account binding is immutable")]
    SharedProviderIdentityChanged {
        app: &'static str,
        provider_id: String,
        share_id: String,
    },
    #[error("account {account_id} is referenced by Share {share_id}; its subscription identity is immutable")]
    SharedAccountIdentityChanged {
        account_id: String,
        share_id: String,
    },
}

impl SubscriptionBindingError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::IdentityConflict { .. } => "cc_switch_subscription_identity_conflict",
            Self::SharedProviderIdentityChanged { .. } => {
                "cc_switch_provider_account_binding_in_use"
            }
            Self::SharedAccountIdentityChanged { .. } => "cc_switch_account_identity_in_use",
            Self::UnverifiedIdentity { .. } => "cc_switch_subscription_identity_unverified",
            Self::StaleIdentity { .. } => "cc_switch_provider_account_identity_stale",
            Self::ManagedAuthRequired { .. } => "cc_switch_share_oauth_managed_auth_required",
            _ => "cc_switch_share_binding_integrity_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexWorkspaceRebindError {
    #[error("share not found: {0}")]
    ShareNotFound(String),
    #[error("Codex account not found: {0}")]
    AccountNotFound(String),
    #[error(
        "Codex workspace is reserved by Share(s) {}; pause the Share and submit shareId with expectedConfigRevision",
        .share_ids.join(", ")
    )]
    AccountInUse { share_ids: Vec<String> },
    #[error("Share revision conflict: expected {expected_revision}, current {current_revision}")]
    RevisionConflict {
        expected_revision: u64,
        current_revision: u64,
    },
    #[error("share must be paused before changing its Codex workspace")]
    MustBePaused,
    #[error("Share does not bind the requested Codex account")]
    AccountBindingMismatch,
    #[error("{0}")]
    InvalidWorkspace(String),
    #[error(transparent)]
    SubscriptionBinding(#[from] SubscriptionBindingError),
}

impl CodexWorkspaceRebindError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ShareNotFound(_) => "cc_switch_share_not_found",
            Self::AccountNotFound(_) => "cc_switch_codex_account_not_found",
            Self::AccountInUse { .. } => "cc_switch_codex_workspace_in_use",
            Self::RevisionConflict { .. } => "cc_switch_share_revision_conflict",
            Self::MustBePaused => "cc_switch_share_must_be_paused",
            Self::AccountBindingMismatch => "cc_switch_share_binding_integrity_failed",
            Self::InvalidWorkspace(_) => "cc_switch_codex_workspace_invalid",
            Self::SubscriptionBinding(error) => error.code(),
        }
    }
}

pub fn validate_subscription_reference_graph(
    providers: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> Result<BTreeMap<SubscriptionIdentityKey, String>, SubscriptionBindingError> {
    let (owners, issues) = collect_subscription_reference_graph(providers, accounts, shares);
    match issues.into_iter().next() {
        Some(issue) => Err(issue.error),
        None => Ok(owners),
    }
}

pub fn subscription_reference_graph_errors(
    providers: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> Vec<SubscriptionBindingError> {
    collect_subscription_reference_graph(providers, accounts, shares)
        .1
        .into_iter()
        .map(|issue| issue.error)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SubscriptionGraphIssue {
    share_id: String,
    error: SubscriptionBindingError,
}

fn collect_subscription_reference_graph(
    providers: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> (
    BTreeMap<SubscriptionIdentityKey, String>,
    Vec<SubscriptionGraphIssue>,
) {
    let mut owners = BTreeMap::<SubscriptionIdentityKey, String>::new();
    let mut errors = Vec::new();
    for share in shares
        .shares
        .iter()
        .filter(|share| share.status != "deleted")
    {
        let identities = match resolve_share_subscription_identities(share, providers, accounts) {
            Ok(identities) => identities,
            Err(error) => {
                errors.push(SubscriptionGraphIssue {
                    share_id: share.id.clone(),
                    error,
                });
                subscription_identity_reservations_for_share(share, providers, accounts)
            }
        };
        for identity in identities {
            owners.entry(identity).or_insert_with(|| share.id.clone());
        }
    }
    (owners, errors)
}

pub fn validate_share_subscription_binding(
    share_id: &str,
    providers: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> Result<Option<SubscriptionIdentityKey>, SubscriptionBindingError> {
    let share = shares
        .shares
        .iter()
        .find(|share| share.id == share_id)
        .ok_or_else(|| SubscriptionBindingError::ShareNotFound(share_id.to_string()))?;
    Ok(
        resolve_share_subscription_identities(share, providers, accounts)?
            .into_iter()
            .next(),
    )
}

pub fn validate_shared_provider_bindings_unchanged(
    current: &ProviderStore,
    candidate: &ProviderStore,
    _accounts: &AccountStore,
    shares: &ShareStore,
) -> Result<(), SubscriptionBindingError> {
    for share in shares
        .shares
        .iter()
        .filter(|share| share.status != "deleted")
    {
        let mut provider_keys = std::iter::once((share.app, share.provider_id.as_str()))
            .chain(
                share
                    .bindings
                    .iter()
                    .map(|binding| (binding.app, binding.provider_id.as_str())),
            )
            .collect::<Vec<_>>();
        provider_keys.sort();
        provider_keys.dedup();

        for (app, provider_id) in provider_keys {
            let current_provider = find_provider(current, app, provider_id);
            let candidate_provider = find_provider(candidate, app, provider_id);
            let changed = match (current_provider, candidate_provider) {
                (Some(current_provider), Some(candidate_provider)) => {
                    provider_binding_snapshot(current_provider)
                        != provider_binding_snapshot(candidate_provider)
                }
                (Some(_), None) => true,
                (None, _) => false,
            };
            if changed {
                return Err(SubscriptionBindingError::SharedProviderIdentityChanged {
                    app: app.as_str(),
                    provider_id: provider_id.to_string(),
                    share_id: share.id.clone(),
                });
            }
        }
    }
    Ok(())
}

pub fn validate_ordinary_subscription_reference_change(
    current: &ProviderStore,
    candidate: &ProviderStore,
    accounts: &AccountStore,
    shares: &ShareStore,
) -> Result<(), SubscriptionBindingError> {
    validate_shared_provider_bindings_unchanged(current, candidate, accounts, shares)?;
    validate_subscription_reference_graph_transition(
        current, accounts, shares, candidate, accounts, shares,
    )
}

pub fn validate_ordinary_account_subscription_change(
    providers: &ProviderStore,
    current_accounts: &AccountStore,
    candidate_accounts: &AccountStore,
    shares: &ShareStore,
) -> Result<(), SubscriptionBindingError> {
    for share in shares
        .shares
        .iter()
        .filter(|share| share.status != "deleted")
    {
        let mut provider_keys = std::iter::once((share.app, share.provider_id.as_str()))
            .chain(
                share
                    .bindings
                    .iter()
                    .map(|binding| (binding.app, binding.provider_id.as_str())),
            )
            .collect::<Vec<_>>();
        provider_keys.sort();
        provider_keys.dedup();

        for (app, provider_id) in provider_keys {
            let Some(provider) = find_provider(providers, app, provider_id) else {
                continue;
            };
            let Some(account_id) = provider_account_id(provider) else {
                continue;
            };
            let Some(current) = current_accounts
                .accounts
                .iter()
                .find(|account| account.id == account_id)
            else {
                // Adding a previously missing account may repair a historical Share.
                continue;
            };
            let Some(candidate) = candidate_accounts
                .accounts
                .iter()
                .find(|account| account.id == account_id)
            else {
                return Err(SubscriptionBindingError::SharedAccountIdentityChanged {
                    account_id: account_id.to_string(),
                    share_id: share.id.clone(),
                });
            };
            let current_identity = subscription_identity_reservation(provider, current_accounts);
            let candidate_identity =
                subscription_identity_reservation(provider, candidate_accounts);
            if current.provider_type != candidate.provider_type
                || current.auth_identity_generation != candidate.auth_identity_generation
                || current_identity != candidate_identity
            {
                return Err(SubscriptionBindingError::SharedAccountIdentityChanged {
                    account_id: account_id.to_string(),
                    share_id: share.id.clone(),
                });
            }
        }
    }

    validate_subscription_reference_graph_transition(
        providers,
        current_accounts,
        shares,
        providers,
        candidate_accounts,
        shares,
    )
}

pub fn validate_subscription_reference_graph_transition(
    current_providers: &ProviderStore,
    current_accounts: &AccountStore,
    current_shares: &ShareStore,
    candidate_providers: &ProviderStore,
    candidate_accounts: &AccountStore,
    candidate_shares: &ShareStore,
) -> Result<(), SubscriptionBindingError> {
    let current_issues =
        collect_subscription_reference_graph(current_providers, current_accounts, current_shares).1;
    let candidate_issues = collect_subscription_reference_graph(
        candidate_providers,
        candidate_accounts,
        candidate_shares,
    )
    .1;

    let mut unmatched_current = current_issues
        .iter()
        .filter(|issue| {
            !matches!(
                &issue.error,
                SubscriptionBindingError::IdentityConflict { .. }
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    for candidate_issue in candidate_issues.iter().filter(|issue| {
        !matches!(
            &issue.error,
            SubscriptionBindingError::IdentityConflict { .. }
        )
    }) {
        let Some(index) = unmatched_current
            .iter()
            .position(|current_issue| current_issue == candidate_issue)
        else {
            return Err(candidate_issue.error.clone());
        };
        unmatched_current.swap_remove(index);
    }

    let current_conflicts = conflict_participants_by_fingerprint(&current_issues);
    let candidate_conflicts = conflict_participants_by_fingerprint(&candidate_issues);
    for (fingerprint, candidate_participants) in candidate_conflicts {
        let unchanged_or_reduced =
            current_conflicts
                .get(&fingerprint)
                .is_some_and(|current_participants| {
                    candidate_participants.is_subset(current_participants)
                });
        if unchanged_or_reduced {
            continue;
        }
        if let Some(error) = candidate_issues
            .iter()
            .find_map(|issue| match &issue.error {
                SubscriptionBindingError::IdentityConflict {
                    fingerprint: issue_fingerprint,
                    ..
                } if issue_fingerprint == &fingerprint => Some(issue.error.clone()),
                _ => None,
            })
        {
            return Err(error);
        }
    }
    Ok(())
}

fn conflict_participants_by_fingerprint(
    issues: &[SubscriptionGraphIssue],
) -> BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut conflicts = BTreeMap::<String, std::collections::BTreeSet<String>>::new();
    for issue in issues {
        let SubscriptionBindingError::IdentityConflict {
            fingerprint,
            existing_share_id,
            conflicting_share_id,
        } = &issue.error
        else {
            continue;
        };
        let participants = conflicts.entry(fingerprint.clone()).or_default();
        participants.insert(existing_share_id.clone());
        participants.insert(conflicting_share_id.clone());
    }
    conflicts
}

pub fn share_ids_for_account(
    account_id: &str,
    providers: &ProviderStore,
    shares: &ShareStore,
) -> Vec<String> {
    let mut ids = Vec::new();
    for share in shares
        .shares
        .iter()
        .filter(|share| share.status != "deleted")
    {
        let referenced = share
            .bindings
            .iter()
            .map(|binding| (binding.app, binding.provider_id.as_str()))
            .filter_map(|(app, provider_id)| find_provider(providers, app, provider_id))
            .any(|provider| provider_account_id(provider) == Some(account_id));
        if referenced {
            ids.push(share.id.clone());
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

fn resolve_share_subscription_identities(
    share: &Share,
    providers: &ProviderStore,
    accounts: &AccountStore,
) -> Result<Vec<SubscriptionIdentityKey>, SubscriptionBindingError> {
    crate::domain::sharing::invariants::validate_share_import(share).map_err(|_| {
        SubscriptionBindingError::InvalidShareBinding {
            share_id: share.id.clone(),
        }
    })?;
    let mut identities = Vec::new();
    for binding in &share.bindings {
        let provider =
            find_provider(providers, binding.app, &binding.provider_id).ok_or_else(|| {
                SubscriptionBindingError::ProviderMissing {
                    share_id: share.id.clone(),
                    app: binding.app.as_str(),
                    provider_id: binding.provider_id.clone(),
                }
            })?;
        if provider.provider_type != binding.provider_type {
            return Err(SubscriptionBindingError::ProviderTypeMismatch {
                share_id: share.id.clone(),
            });
        }
        if is_subscription_oauth(provider.provider_type) {
            if let Some(identity) = resolve_provider_subscription_identity(provider, accounts)? {
                identities.push(identity);
            }
        }
    }
    identities.sort();
    identities.dedup();
    Ok(identities)
}

fn is_subscription_oauth(provider_type: ProviderType) -> bool {
    matches!(
        provider_type,
        ProviderType::ClaudeOAuth | ProviderType::CodexOAuth
    )
}

fn subscription_identity_reservations_for_share(
    share: &Share,
    providers: &ProviderStore,
    accounts: &AccountStore,
) -> Vec<SubscriptionIdentityKey> {
    let mut identities = share
        .bindings
        .iter()
        .map(|binding| (binding.app, binding.provider_id.as_str()))
        .filter_map(|(app, provider_id)| find_provider(providers, app, provider_id))
        .filter_map(|provider| subscription_identity_reservation(provider, accounts))
        .collect::<Vec<_>>();
    identities.sort();
    identities.dedup();
    identities
}

fn subscription_identity_reservation(
    provider: &StoredProvider,
    accounts: &AccountStore,
) -> Option<SubscriptionIdentityKey> {
    if !matches!(
        provider.provider_type,
        ProviderType::ClaudeOAuth | ProviderType::CodexOAuth
    ) {
        return None;
    }
    let account_id = provider_account_id(provider)?;
    let account = accounts.accounts.iter().find(|account| {
        account.id == account_id && account.provider_type == provider.provider_type
    })?;
    match provider.provider_type {
        ProviderType::ClaudeOAuth => claude_account_uuid(account)
            .map(|account_uuid| SubscriptionIdentityKey::ClaudeOAuth { account_uuid }),
        ProviderType::CodexOAuth => verified_openai_subject(account)
            .zip(effective_codex_workspace_id(account))
            .map(
                |(subject, workspace_id)| SubscriptionIdentityKey::CodexOAuth {
                    subject,
                    workspace_id,
                },
            ),
        _ => None,
    }
}

pub fn resolve_provider_subscription_identity(
    provider: &StoredProvider,
    accounts: &AccountStore,
) -> Result<Option<SubscriptionIdentityKey>, SubscriptionBindingError> {
    if !matches!(
        provider.provider_type,
        ProviderType::ClaudeOAuth | ProviderType::CodexOAuth
    ) {
        return Ok(None);
    }
    let app = provider.app.as_str();
    let provider_id = provider.provider.id.clone();
    let account_id = provider_account_id(provider).ok_or_else(|| {
        SubscriptionBindingError::AccountBindingMissing {
            app,
            provider_id: provider_id.clone(),
        }
    })?;
    if !has_authoritative_subscription_oauth_binding(provider) {
        return Err(SubscriptionBindingError::ManagedAuthRequired { app, provider_id });
    }
    let account = accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| SubscriptionBindingError::AccountMissing {
            app,
            provider_id: provider_id.clone(),
        })?;
    if account.provider_type != provider.provider_type {
        return Err(SubscriptionBindingError::AccountTypeMismatch { app, provider_id });
    }
    if provider_auth_identity_generation(provider) != Some(account.auth_identity_generation) {
        return Err(SubscriptionBindingError::StaleIdentity { app, provider_id });
    }

    let identity = match provider.provider_type {
        ProviderType::ClaudeOAuth => claude_account_uuid(account)
            .map(|account_uuid| SubscriptionIdentityKey::ClaudeOAuth { account_uuid }),
        ProviderType::CodexOAuth => verified_openai_subject(account)
            .zip(effective_codex_workspace_id(account))
            .map(
                |(subject, workspace_id)| SubscriptionIdentityKey::CodexOAuth {
                    subject,
                    workspace_id,
                },
            ),
        _ => None,
    };
    identity
        .map(Some)
        .ok_or(SubscriptionBindingError::UnverifiedIdentity { app, provider_id })
}

fn find_provider<'a>(
    providers: &'a ProviderStore,
    app: AppKind,
    provider_id: &str,
) -> Option<&'a StoredProvider> {
    providers
        .providers
        .iter()
        .find(|provider| provider.app == app && provider.provider.id == provider_id)
}

fn provider_binding_snapshot(
    provider: &StoredProvider,
) -> (ProviderType, Option<&str>, Option<u64>) {
    (
        provider.provider_type,
        provider_account_id(provider),
        provider_auth_identity_generation(provider),
    )
}

fn provider_account_id(provider: &StoredProvider) -> Option<&str> {
    provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.account_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn provider_auth_identity_generation(provider: &StoredProvider) -> Option<u64> {
    provider
        .provider
        .meta
        .as_ref()
        .and_then(|meta| meta.auth_binding.as_ref())
        .and_then(|binding| binding.auth_identity_generation)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::domain::accounts::store::{Account, AccountStore};
    use crate::domain::providers::model::{AuthBinding, Provider, ProviderMeta};
    use crate::domain::providers::registry::ProfileId;
    use crate::domain::providers::store::{ProviderResourceMetadata, StoredProvider};
    use crate::domain::sharing::shares::{ShareBinding, UpsertShareInput};

    use super::*;

    fn account(id: &str, subject: &str, workspace: &str) -> Account {
        Account {
            id: id.to_string(),
            provider_type: ProviderType::CodexOAuth,
            auth_identity_generation: 1,
            token_refresh_generation: 0,
            email: None,
            access_token: None,
            refresh_token: None,
            id_token: None,
            token_type: None,
            api_key: None,
            extra_headers: BTreeMap::new(),
            scopes: Vec::new(),
            profile: Some(json!({
                "verifiedOpenAiClaims": {
                    "subject": subject,
                    "chatgpt_account_id": workspace
                },
                "workspaces": [{"id": workspace, "name": workspace}]
            })),
            raw: None,
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
            capability_observations: Default::default(),
        }
    }

    fn provider(id: &str, account_id: &str) -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta {
                    auth_binding: Some(AuthBinding {
                        source: Some("account".to_string()),
                        auth_provider: Some("codex_oauth".to_string()),
                        account_id: Some(account_id.to_string()),
                        auth_identity_generation: Some(1),
                    }),
                    ..ProviderMeta::default()
                }),
                extra: BTreeMap::new(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: "codex_oauth".to_string(),
            resource: ProviderResourceMetadata::default(),
        }
    }

    fn share(id: &str, provider_id: &str) -> Share {
        let mut store = ShareStore::default();
        let input = UpsertShareInput {
            id: Some(id.to_string()),
            owner_email: Some("owner@example.com".to_string()),
            app: AppKind::Codex,
            provider_id: provider_id.to_string(),
            provider_type: ProviderType::CodexOAuth,
            display_name: None,
            enabled: Some(true),
            status: Some("active".to_string()),
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
            access_by_app: BTreeMap::new(),
            app_settings: BTreeMap::new(),
            for_sale_official_price_percent_by_app: BTreeMap::new(),
            official_price_percent: None,
            allow_personal_credits: None,
            auto_consume_banked_reset: None,
            banked_reset_expiry_lead_minutes: None,
            previous_response_cache_enabled: None,
            auto_start: None,
            description: None,
            enabled_apps: None,
            bindings: vec![ShareBinding {
                app: AppKind::Codex,
                provider_id: provider_id.to_string(),
                provider_type: ProviderType::CodexOAuth,
            }],
            runtime_snapshot: None,
            user_grants: BTreeMap::new(),
        };
        store.upsert(input).unwrap()
    }

    fn claude_account(id: &str, account_uuid: &str) -> Account {
        let mut account = account(id, "unused-codex-subject", "unused-codex-workspace");
        account.provider_type = ProviderType::ClaudeOAuth;
        account.profile = Some(json!({"accountUUID": account_uuid}));
        account
    }

    fn claude_provider(id: &str, account_id: &str) -> StoredProvider {
        let mut provider = provider(id, account_id);
        provider.app = AppKind::Claude;
        provider.provider_type = ProviderType::ClaudeOAuth;
        provider.provider_type_id = ProviderType::ClaudeOAuth.as_str().to_string();
        if let Some(binding) = provider
            .provider
            .meta
            .as_mut()
            .and_then(|meta| meta.auth_binding.as_mut())
        {
            binding.auth_provider = Some(ProviderType::ClaudeOAuth.as_str().to_string());
        }
        provider
    }

    fn claude_share(id: &str, provider_id: &str) -> Share {
        let mut share = share(id, provider_id);
        share.app = AppKind::Claude;
        share.provider_type = ProviderType::ClaudeOAuth;
        share.bindings = vec![ShareBinding {
            app: AppKind::Claude,
            provider_id: provider_id.to_string(),
            provider_type: ProviderType::ClaudeOAuth,
        }];
        share
    }

    #[test]
    fn duplicate_logical_subscription_can_back_multiple_share_urls() {
        let accounts = AccountStore {
            accounts: vec![
                account("account-a", "subject-1", "workspace-1"),
                account("account-b", "subject-1", "workspace-1"),
            ],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![
                provider("provider-a", "account-a"),
                provider("provider-b", "account-b"),
            ],
            ..ProviderStore::default()
        };
        let shares = ShareStore {
            shares: vec![
                share("share-a", "provider-a"),
                share("share-b", "provider-b"),
            ],
            ..ShareStore::default()
        };

        let graph = validate_subscription_reference_graph(&providers, &accounts, &shares).unwrap();
        assert_eq!(graph.len(), 1);
        assert_eq!(graph.values().next().map(String::as_str), Some("share-a"));
    }

    #[test]
    fn different_workspaces_are_distinct_subscription_identities() {
        let accounts = AccountStore {
            accounts: vec![
                account("account-a", "subject-1", "workspace-1"),
                account("account-b", "subject-1", "workspace-2"),
            ],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![
                provider("provider-a", "account-a"),
                provider("provider-b", "account-b"),
            ],
            ..ProviderStore::default()
        };
        let shares = ShareStore {
            shares: vec![
                share("share-a", "provider-a"),
                share("share-b", "provider-b"),
            ],
            ..ShareStore::default()
        };

        assert_eq!(
            validate_subscription_reference_graph(&providers, &accounts, &shares)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn claude_account_uuid_can_back_multiple_share_urls() {
        let accounts = AccountStore {
            accounts: vec![
                claude_account("account-a", "CLAUDE-ACCOUNT-UUID"),
                claude_account("account-b", "claude-account-uuid"),
            ],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![
                claude_provider("provider-a", "account-a"),
                claude_provider("provider-b", "account-b"),
            ],
            ..ProviderStore::default()
        };
        let shares = ShareStore {
            shares: vec![
                claude_share("share-a", "provider-a"),
                claude_share("share-b", "provider-b"),
            ],
            ..ShareStore::default()
        };

        assert_eq!(
            validate_subscription_reference_graph(&providers, &accounts, &shares)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn missing_or_stale_strong_identity_fails_closed() {
        let mut unverified = account("account-a", "", "workspace-1");
        unverified.profile = Some(json!({
            "verifiedOpenAiClaims": {"chatgpt_account_id": "workspace-1"},
            "workspaces": [{"id": "workspace-1", "name": "workspace-1"}]
        }));
        let accounts = AccountStore {
            accounts: vec![unverified],
            ..Default::default()
        };
        let mut providers = ProviderStore {
            providers: vec![provider("provider-a", "account-a")],
            ..ProviderStore::default()
        };
        let shares = ShareStore {
            shares: vec![share("share-a", "provider-a")],
            ..ShareStore::default()
        };

        let error =
            validate_subscription_reference_graph(&providers, &accounts, &shares).unwrap_err();
        assert_eq!(error.code(), "cc_switch_subscription_identity_unverified");

        providers.providers[0]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .auth_identity_generation = Some(0);
        let error =
            validate_subscription_reference_graph(&providers, &accounts, &shares).unwrap_err();
        assert_eq!(error.code(), "cc_switch_provider_account_identity_stale");
    }

    #[test]
    fn oauth_share_rejects_a_static_credential_profile_even_with_an_account_binding() {
        let accounts = AccountStore {
            accounts: vec![account("account-a", "subject-1", "workspace-1")],
            ..Default::default()
        };
        let mut incompatible = provider("provider-a", "account-a");
        incompatible.resource.profile_id = Some(ProfileId::parse("codex.openai_api_key").unwrap());

        let error = resolve_provider_subscription_identity(&incompatible, &accounts).unwrap_err();

        assert_eq!(error.code(), "cc_switch_share_oauth_managed_auth_required");
    }

    #[test]
    fn share_without_bindings_is_rejected_in_fresh_schema() {
        let mut static_provider = provider("provider-a", "unused-account");
        static_provider.provider_type = ProviderType::Codex;
        static_provider.provider_type_id = ProviderType::Codex.as_str().to_string();
        static_provider.resource.profile_id =
            Some(ProfileId::parse("codex.openai_api_key").unwrap());
        static_provider.provider.meta.as_mut().unwrap().auth_binding = None;
        let providers = ProviderStore {
            providers: vec![static_provider],
            ..ProviderStore::default()
        };
        let mut legacy = share("share-a", "provider-a");
        legacy.provider_type = ProviderType::Codex;
        legacy.bindings.clear();
        let shares = ShareStore {
            shares: vec![legacy],
            ..ShareStore::default()
        };
        let accounts = AccountStore::default();

        let error = validate_share_subscription_binding("share-a", &providers, &accounts, &shares)
            .unwrap_err();
        assert!(matches!(
            error,
            SubscriptionBindingError::InvalidShareBinding { .. }
        ));
    }

    #[test]
    fn subscription_oauth_share_without_bindings_remains_fail_closed() {
        let accounts = AccountStore {
            accounts: vec![account("account-a", "subject-1", "workspace-1")],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider("provider-a", "account-a")],
            ..ProviderStore::default()
        };
        let mut malformed = share("share-a", "provider-a");
        malformed.bindings.clear();
        let shares = ShareStore {
            shares: vec![malformed],
            ..ShareStore::default()
        };

        let error = validate_share_subscription_binding("share-a", &providers, &accounts, &shares)
            .unwrap_err();
        assert_eq!(error.code(), "cc_switch_share_binding_integrity_failed");
        assert!(matches!(
            error,
            SubscriptionBindingError::InvalidShareBinding { .. }
        ));
    }

    #[test]
    fn every_non_deleted_share_can_share_one_capacity_identity() {
        let accounts = AccountStore {
            accounts: vec![
                account("account-a", "subject-1", "workspace-1"),
                account("account-b", "subject-1", "workspace-1"),
            ],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![
                provider("provider-a", "account-a"),
                provider("provider-b", "account-b"),
            ],
            ..ProviderStore::default()
        };
        let first = share("share-a", "provider-a");
        let mut second = share("share-b", "provider-b");

        for status in ["active", "paused", "expired", "stopped"] {
            second.status = status.to_string();
            let shares = ShareStore {
                shares: vec![first.clone(), second.clone()],
                ..ShareStore::default()
            };
            assert_eq!(
                validate_subscription_reference_graph(&providers, &accounts, &shares)
                    .unwrap()
                    .len(),
                1,
                "status {status} must preserve the shared identity"
            );
        }

        second.status = "deleted".to_string();
        let shares = ShareStore {
            shares: vec![first, second],
            ..ShareStore::default()
        };
        assert_eq!(
            validate_subscription_reference_graph(&providers, &accounts, &shares)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn historical_graph_error_allows_only_non_worsening_provider_changes() {
        let accounts = AccountStore {
            accounts: vec![account("account-a", "subject-1", "workspace-1")],
            ..Default::default()
        };
        let mut providers = ProviderStore {
            providers: vec![provider("provider-a", "account-a")],
            ..ProviderStore::default()
        };
        providers.providers[0]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .auth_identity_generation = Some(0);
        let shares = ShareStore {
            shares: vec![share("share-a", "provider-a")],
            ..ShareStore::default()
        };

        let mut metadata_only = providers.clone();
        metadata_only.providers[0].provider.name = "renamed".to_string();
        validate_ordinary_subscription_reference_change(
            &providers,
            &metadata_only,
            &accounts,
            &shares,
        )
        .unwrap();

        let mut rebound = providers.clone();
        rebound.providers[0]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .account_id = Some("other-account".to_string());
        let error = validate_ordinary_subscription_reference_change(
            &providers, &rebound, &accounts, &shares,
        )
        .unwrap_err();
        assert_eq!(error.code(), "cc_switch_provider_account_binding_in_use");
    }

    #[test]
    fn malformed_historical_share_still_pins_its_primary_provider() {
        let accounts = AccountStore {
            accounts: vec![account("account-a", "subject-a", "workspace-a")],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider("provider-a", "account-a")],
            ..ProviderStore::default()
        };
        let mut malformed = share("share-a", "provider-a");
        malformed.bindings.clear();
        let shares = ShareStore {
            shares: vec![malformed],
            ..ShareStore::default()
        };
        let mut removed = providers.clone();
        removed.providers.clear();

        let error = validate_ordinary_subscription_reference_change(
            &providers, &removed, &accounts, &shares,
        )
        .unwrap_err();
        assert_eq!(error.code(), "cc_switch_provider_account_binding_in_use");
    }

    #[test]
    fn historical_graph_errors_can_be_repaired_one_share_at_a_time() {
        let accounts = AccountStore {
            accounts: vec![
                account("account-a", "subject-a", "workspace-a"),
                account("account-b", "subject-b", "workspace-b"),
            ],
            ..Default::default()
        };
        let mut providers = ProviderStore {
            providers: vec![
                provider("provider-a", "account-a"),
                provider("provider-b", "account-b"),
            ],
            ..ProviderStore::default()
        };
        for provider in &mut providers.providers {
            provider
                .provider
                .meta
                .as_mut()
                .unwrap()
                .auth_binding
                .as_mut()
                .unwrap()
                .auth_identity_generation = Some(0);
        }
        let shares = ShareStore {
            shares: vec![
                share("share-a", "provider-a"),
                share("share-b", "provider-b"),
            ],
            ..ShareStore::default()
        };

        let mut repaired = shares.clone();
        repaired
            .shares
            .iter_mut()
            .find(|share| share.id == "share-a")
            .unwrap()
            .status = "deleted".to_string();
        validate_subscription_reference_graph_transition(
            &providers, &accounts, &shares, &providers, &accounts, &repaired,
        )
        .unwrap();

        let mut replaced = repaired;
        replaced.shares.push(share("share-c", "provider-a"));
        let error = validate_subscription_reference_graph_transition(
            &providers, &accounts, &shares, &providers, &accounts, &replaced,
        )
        .unwrap_err();
        assert_eq!(error.code(), "cc_switch_provider_account_identity_stale");
    }

    #[test]
    fn ordinary_account_write_cannot_move_a_reserved_subscription_identity() {
        let accounts = AccountStore {
            accounts: vec![account("account-a", "subject-a", "workspace-a")],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![provider("provider-a", "account-a")],
            ..ProviderStore::default()
        };
        let shares = ShareStore {
            shares: vec![share("share-a", "provider-a")],
            ..ShareStore::default()
        };
        let mut candidate = accounts.clone();
        candidate.accounts[0].auth_identity_generation = 2;
        candidate.accounts[0].profile = Some(json!({
            "verifiedOpenAiClaims": {
                "subject": "subject-b",
                "chatgpt_account_id": "workspace-b",
                "organizations": [{"id": "workspace-b"}]
            },
            "selectedChatgptAccountId": "workspace-b"
        }));

        let error = validate_ordinary_account_subscription_change(
            &providers, &accounts, &candidate, &shares,
        )
        .unwrap_err();
        assert_eq!(error.code(), "cc_switch_account_identity_in_use");

        let mut token_rotation = accounts.clone();
        token_rotation.accounts[0].access_token = Some("rotated-token".to_string());
        token_rotation.accounts[0].token_refresh_generation = 1;
        validate_ordinary_account_subscription_change(
            &providers,
            &accounts,
            &token_rotation,
            &shares,
        )
        .unwrap();
    }

    #[test]
    fn stale_share_does_not_block_an_independent_url_for_same_identity() {
        let accounts = AccountStore {
            accounts: vec![
                account("account-a", "shared-subject", "shared-workspace"),
                account("account-b", "shared-subject", "shared-workspace"),
            ],
            ..Default::default()
        };
        let mut providers = ProviderStore {
            providers: vec![
                provider("provider-a", "account-a"),
                provider("provider-b", "account-b"),
            ],
            ..ProviderStore::default()
        };
        providers.providers[0]
            .provider
            .meta
            .as_mut()
            .unwrap()
            .auth_binding
            .as_mut()
            .unwrap()
            .auth_identity_generation = Some(0);
        let current = ShareStore {
            shares: vec![share("share-a", "provider-a")],
            ..ShareStore::default()
        };
        let mut candidate = current.clone();
        candidate.shares.push(share("share-b", "provider-b"));

        validate_subscription_reference_graph_transition(
            &providers, &accounts, &current, &providers, &accounts, &candidate,
        )
        .unwrap();
        assert!(
            validate_share_subscription_binding("share-b", &providers, &accounts, &candidate)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn adding_and_removing_urls_for_one_identity_is_allowed() {
        let accounts = AccountStore {
            accounts: vec![
                account("account-a", "subject", "workspace"),
                account("account-b", "subject", "workspace"),
                account("account-c", "subject", "workspace"),
                account("account-d", "subject", "workspace"),
            ],
            ..Default::default()
        };
        let providers = ProviderStore {
            providers: vec![
                provider("provider-a", "account-a"),
                provider("provider-b", "account-b"),
                provider("provider-c", "account-c"),
                provider("provider-d", "account-d"),
            ],
            ..ProviderStore::default()
        };
        let shares = ShareStore {
            shares: vec![
                share("share-a", "provider-a"),
                share("share-b", "provider-b"),
                share("share-c", "provider-c"),
            ],
            ..ShareStore::default()
        };

        let mut reduced = shares.clone();
        reduced
            .shares
            .iter_mut()
            .find(|share| share.id == "share-a")
            .unwrap()
            .status = "deleted".to_string();
        validate_subscription_reference_graph_transition(
            &providers, &accounts, &shares, &providers, &accounts, &reduced,
        )
        .unwrap();

        reduced.shares.push(share("share-d", "provider-d"));
        validate_subscription_reference_graph_transition(
            &providers, &accounts, &shares, &providers, &accounts, &reduced,
        )
        .unwrap();
    }
}
