use std::collections::{BTreeMap, BTreeSet};

use chrono::DateTime;

use crate::domain::providers::model::AppKind;
use crate::domain::sharing::router_contract::{ShareAppAccess, ShareAppSettings};
use crate::domain::sharing::shares::{
    Share, ShareAcl, ShareBinding, SharePatchError, UpsertShareInput,
};

fn policy_divergent(message: impl Into<String>) -> SharePatchError {
    SharePatchError::PolicyDivergent(message.into())
}

fn share_sale_pricing_is_eligible(for_sale: bool, free_access: bool) -> bool {
    for_sale && !free_access
}

fn normalize_emails(values: &[String], owner_email: Option<&str>) -> Vec<String> {
    let owner = owner_email.map(|value| value.trim().to_ascii_lowercase());
    values
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty() && owner.as_deref() != Some(value.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_market_access_mode(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" => "all".to_string(),
        _ => "selected".to_string(),
    }
}

fn normalize_for_sale(value: &str) -> (bool, bool) {
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => (false, true),
        "yes" | "true" | "1" | "share" => (true, false),
        _ => (false, false),
    }
}

fn parse_expiration(value: &str) -> Result<Option<i64>, SharePatchError> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        return Ok(Some(timestamp));
    }
    DateTime::parse_from_rfc3339(value)
        .map(|value| Some(value.timestamp_millis()))
        .map_err(|_| policy_divergent("Router appSettings contains an invalid expiresAt"))
}

fn validate_bindings(bindings: &[ShareBinding]) -> Result<(), SharePatchError> {
    if !(1..=3).contains(&bindings.len()) {
        return Err(SharePatchError::Invalid(
            "share must have between one and three bindings".into(),
        ));
    }
    let mut apps = BTreeSet::new();
    for binding in bindings {
        if !apps.insert(binding.app) {
            return Err(SharePatchError::Invalid(
                "share must have at most one binding per app".into(),
            ));
        }
        if binding.provider_id.trim().is_empty() {
            return Err(SharePatchError::Invalid(
                "share binding provider_id is required".into(),
            ));
        }
    }
    Ok(())
}

fn ensure_bound_keys<T>(
    values: &BTreeMap<AppKind, T>,
    apps: &BTreeSet<AppKind>,
    field: &str,
) -> Result<(), SharePatchError> {
    if values.keys().any(|app| !apps.contains(app)) {
        return Err(policy_divergent(format!(
            "Router {field} contains an app without a Share binding"
        )));
    }
    Ok(())
}

fn canonical_access(
    values: &BTreeMap<AppKind, ShareAppAccess>,
    owner_email: Option<&str>,
) -> Result<Option<ShareAppAccess>, SharePatchError> {
    let mut canonical = None;
    for value in values.values() {
        let normalized = ShareAppAccess {
            shared_with_emails: normalize_emails(&value.shared_with_emails, owner_email),
            market_access_mode: normalize_market_access_mode(&value.market_access_mode),
        };
        if canonical
            .as_ref()
            .is_some_and(|current: &ShareAppAccess| current != &normalized)
        {
            return Err(policy_divergent(
                "Router accessByApp entries must describe one global Share policy",
            ));
        }
        canonical = Some(normalized);
    }
    Ok(canonical)
}

fn canonical_settings(
    values: &BTreeMap<AppKind, ShareAppSettings>,
    owner_email: Option<&str>,
) -> Result<Option<ShareAppSettings>, SharePatchError> {
    let mut canonical = None;
    for value in values.values() {
        let normalized = ShareAppSettings {
            for_sale: match normalize_for_sale(&value.for_sale) {
                (false, true) => "Free".to_string(),
                (true, false) => "Yes".to_string(),
                _ => "No".to_string(),
            },
            market_access_mode: normalize_market_access_mode(&value.market_access_mode),
            shared_with_emails: normalize_emails(&value.shared_with_emails, owner_email),
            token_limit: value.token_limit,
            parallel_limit: value.parallel_limit,
            expires_at: value.expires_at.trim().to_string(),
        };
        if canonical
            .as_ref()
            .is_some_and(|current: &ShareAppSettings| current != &normalized)
        {
            return Err(policy_divergent(
                "Router appSettings entries must describe one global Share policy",
            ));
        }
        canonical = Some(normalized);
    }
    Ok(canonical)
}

fn canonical_price(pricing: &BTreeMap<AppKind, u16>) -> Result<Option<u16>, SharePatchError> {
    if pricing.values().any(|percent| !(1..=100).contains(percent)) {
        return Err(SharePatchError::Invalid(
            "share official price percent must be between 1 and 100".into(),
        ));
    }
    let mut values = pricing.values().copied();
    let first = values.next();
    if first.is_some() && values.any(|value| Some(value) != first) {
        return Err(policy_divergent(
            "Router per-app prices must describe one global Share price",
        ));
    }
    Ok(first)
}

pub fn validate_and_normalize_upsert_input(
    input: &mut UpsertShareInput,
) -> Result<ShareBinding, SharePatchError> {
    let app = input.app;
    if input.bindings.is_empty() {
        input.bindings.push(ShareBinding {
            app,
            provider_id: input.provider_id.clone(),
            provider_type: input.provider_type,
        });
    }
    input.bindings.sort_by_key(|binding| binding.app);
    validate_bindings(&input.bindings)?;
    let binding = input
        .bindings
        .iter()
        .find(|binding| binding.app == app)
        .cloned()
        .ok_or_else(|| {
            SharePatchError::Invalid("share.app must identify one of the bindings".into())
        })?;
    if binding.provider_id != input.provider_id || binding.provider_type != input.provider_type {
        return Err(SharePatchError::Invalid(
            "share primary fields must match the binding selected by share.app".into(),
        ));
    }

    let apps = input
        .bindings
        .iter()
        .map(|binding| binding.app)
        .collect::<BTreeSet<_>>();
    ensure_bound_keys(&input.access_by_app, &apps, "accessByApp")?;
    ensure_bound_keys(&input.app_settings, &apps, "appSettings")?;
    ensure_bound_keys(
        &input.for_sale_official_price_percent_by_app,
        &apps,
        "forSaleOfficialPricePercentByApp",
    )?;

    let access = canonical_access(&input.access_by_app, input.owner_email.as_deref())?;
    let settings = canonical_settings(&input.app_settings, input.owner_email.as_deref())?;
    let price = canonical_price(&input.for_sale_official_price_percent_by_app)?;

    if let Some(access) = access {
        if let Some(acl) = input.acl.as_ref() {
            if normalize_emails(&acl.shared_with_emails, input.owner_email.as_deref())
                != access.shared_with_emails
                || normalize_market_access_mode(
                    acl.market_access_mode.as_deref().unwrap_or("selected"),
                ) != access.market_access_mode
            {
                return Err(policy_divergent(
                    "accessByApp disagrees with the global Share ACL",
                ));
            }
        } else {
            input.acl = Some(ShareAcl {
                shared_with_emails: access.shared_with_emails,
                public_market_email: None,
                market_access_mode: Some(access.market_access_mode),
            });
        }
    }

    if let Some(settings) = settings {
        let (for_sale, free_access) = normalize_for_sale(&settings.for_sale);
        let token_limit = (settings.token_limit >= 0).then_some(settings.token_limit as u64);
        let parallel_limit =
            (settings.parallel_limit >= 0).then_some(settings.parallel_limit as u32);
        let expires_at = parse_expiration(&settings.expires_at)?;
        let acl = input.acl.get_or_insert_with(ShareAcl::default);
        let setting_emails = settings.shared_with_emails;
        let setting_mode = settings.market_access_mode;
        if (!acl.shared_with_emails.is_empty()
            && normalize_emails(&acl.shared_with_emails, input.owner_email.as_deref())
                != setting_emails)
            || acl
                .market_access_mode
                .as_deref()
                .is_some_and(|mode| normalize_market_access_mode(mode) != setting_mode)
        {
            return Err(policy_divergent(
                "appSettings disagrees with the global Share policy",
            ));
        }
        acl.shared_with_emails = setting_emails;
        acl.market_access_mode = Some(setting_mode);
        input.for_sale.get_or_insert(for_sale);
        input.free_access.get_or_insert(free_access);
        if let Some(projected) = token_limit {
            input.token_limit.get_or_insert(projected);
        }
        input.parallel_limit = input.parallel_limit.or(parallel_limit);
        input.expires_at = input.expires_at.or(expires_at);
    }

    if let (Some(current), Some(projected)) = (input.official_price_percent, price) {
        if current != projected {
            return Err(policy_divergent(
                "per-app pricing disagrees with the global Share price",
            ));
        }
    }
    input.official_price_percent = input.official_price_percent.or(price);
    let pricing_eligible = share_sale_pricing_is_eligible(
        input.for_sale.unwrap_or(false),
        input.free_access.unwrap_or(false),
    );
    if !pricing_eligible && input.official_price_percent.is_some() {
        return Err(SharePatchError::Invalid(
            "share official price percent requires forSale=Yes".into(),
        ));
    }

    input.access_by_app.clear();
    input.app_settings.clear();
    input.for_sale_official_price_percent_by_app.clear();
    Ok(binding)
}

pub fn validate_share_import(share: &Share) -> Result<(), SharePatchError> {
    validate_bindings(&share.bindings)?;
    let binding = share
        .bindings
        .iter()
        .find(|binding| binding.app == share.app)
        .ok_or_else(|| {
            SharePatchError::Invalid("share.app must identify one of the bindings".into())
        })?;
    if binding.provider_id != share.provider_id || binding.provider_type != share.provider_type {
        return Err(SharePatchError::Invalid(
            "share primary fields must match the binding selected by share.app".into(),
        ));
    }
    if share
        .official_price_percent
        .is_some_and(|percent| !(1..=100).contains(&percent))
    {
        return Err(SharePatchError::Invalid(
            "share official price percent must be between 1 and 100".into(),
        ));
    }
    if !share_sale_pricing_is_eligible(share.for_sale, share.free_access)
        && share.official_price_percent.is_some()
    {
        return Err(SharePatchError::Invalid(
            "share official price percent requires forSale=Yes".into(),
        ));
    }
    Ok(())
}
