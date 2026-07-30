use crate::domain::providers::model::AppKind;
use crate::domain::sharing::shares::{Share, ShareBinding, SharePatchError, UpsertShareInput};

fn app_key(app: AppKind) -> String {
    app.as_str().to_string()
}

fn share_sale_pricing_is_eligible(for_sale: bool, free_access: bool) -> bool {
    for_sale && !free_access
}

fn validate_sale_pricing(
    pricing: &std::collections::BTreeMap<String, u16>,
    app_names: &std::collections::BTreeSet<String>,
) -> Result<(), SharePatchError> {
    if pricing.keys().any(|key| !app_names.contains(key)) {
        return Err(SharePatchError::Invalid(
            "share pricing contains an app without a binding".into(),
        ));
    }
    if pricing.values().any(|percent| !(1..=100).contains(percent)) {
        return Err(SharePatchError::Invalid(
            "share official price percent must be between 1 and 100".into(),
        ));
    }
    let mut values = pricing.values();
    if let Some(first) = values.next() {
        if values.any(|value| value != first) {
            return Err(SharePatchError::Invalid(
                "share official price percent must be shared by every bound app".into(),
            ));
        }
    }
    Ok(())
}

fn validate_bindings(bindings: &[ShareBinding]) -> Result<(), SharePatchError> {
    if !(1..=3).contains(&bindings.len()) {
        return Err(SharePatchError::Invalid(
            "share must have between one and three bindings".into(),
        ));
    }
    let mut apps = std::collections::BTreeSet::new();
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

    let app_names = input
        .bindings
        .iter()
        .map(|binding| app_key(binding.app))
        .collect::<std::collections::BTreeSet<_>>();
    input.access_by_app.retain(|key, _| app_names.contains(key));
    input.app_settings.retain(|key, _| app_names.contains(key));
    validate_sale_pricing(&input.for_sale_official_price_percent_by_app, &app_names)?;
    let pricing_eligible = share_sale_pricing_is_eligible(
        input.for_sale.unwrap_or(false),
        input.free_access.unwrap_or(false),
    );
    if !pricing_eligible && !input.for_sale_official_price_percent_by_app.is_empty() {
        return Err(SharePatchError::Invalid(
            "share official price percent requires forSale=Yes".into(),
        ));
    }

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

    let app_names = share
        .bindings
        .iter()
        .map(|binding| app_key(binding.app))
        .collect::<std::collections::BTreeSet<_>>();
    if share
        .access_by_app
        .keys()
        .any(|key| !app_names.contains(key))
    {
        return Err(SharePatchError::Invalid(
            "share access_by_app contains an app without a binding".into(),
        ));
    }
    if share
        .app_settings
        .keys()
        .any(|key| !app_names.contains(key))
    {
        return Err(SharePatchError::Invalid(
            "share app_settings contains an app without a binding".into(),
        ));
    }
    validate_sale_pricing(&share.for_sale_official_price_percent_by_app, &app_names)?;
    if !share_sale_pricing_is_eligible(share.for_sale, share.free_access)
        && !share.for_sale_official_price_percent_by_app.is_empty()
    {
        return Err(SharePatchError::Invalid(
            "share official price percent requires forSale=Yes".into(),
        ));
    }

    Ok(())
}
