use crate::domain::providers::model::AppKind;
use crate::domain::sharing::shares::{Share, ShareBinding, SharePatchError, UpsertShareInput};

fn app_key(app: AppKind) -> String {
    app.as_str().to_string()
}

fn share_sale_pricing_is_eligible(
    for_sale: bool,
    free_access: bool,
    sale_market_kind: &str,
) -> bool {
    for_sale && !free_access && sale_market_kind.trim().eq_ignore_ascii_case("token")
}

fn validate_sale_pricing(
    pricing: &std::collections::BTreeMap<String, u16>,
    app_name: &str,
) -> Result<(), SharePatchError> {
    if pricing.keys().any(|key| key != app_name) {
        return Err(SharePatchError::Invalid(
            "share for_sale_official_price_percent_by_app must only contain the share app".into(),
        ));
    }
    if pricing.values().any(|percent| !(1..=100).contains(percent)) {
        return Err(SharePatchError::Invalid(
            "share official price percent must be between 1 and 100".into(),
        ));
    }
    Ok(())
}

pub fn validate_and_normalize_upsert_input(
    input: &mut UpsertShareInput,
) -> Result<ShareBinding, SharePatchError> {
    let app = input.app;
    let app_name = app_key(app);

    let binding = if input.bindings.is_empty() {
        ShareBinding {
            app,
            provider_id: input.provider_id.clone(),
            provider_type: input.provider_type,
        }
    } else if input.bindings.len() == 1 {
        input.bindings[0].clone()
    } else {
        return Err(SharePatchError::Invalid(
            "share must have exactly one binding".into(),
        ));
    };

    if binding.app != app {
        return Err(SharePatchError::Invalid(
            "share binding app must match share.app".into(),
        ));
    }
    if binding.provider_id != input.provider_id {
        return Err(SharePatchError::Invalid(
            "share binding provider_id must match share.provider_id".into(),
        ));
    }

    input.bindings = vec![binding.clone()];
    input.access_by_app.retain(|key, _| key == &app_name);
    input.app_settings.retain(|key, _| key == &app_name);
    validate_sale_pricing(&input.for_sale_official_price_percent_by_app, &app_name)?;
    let pricing_eligible = share_sale_pricing_is_eligible(
        input.for_sale.unwrap_or(false),
        input.free_access.unwrap_or(false),
        input.sale_market_kind.as_deref().unwrap_or("token"),
    );
    if !pricing_eligible && !input.for_sale_official_price_percent_by_app.is_empty() {
        return Err(SharePatchError::Invalid(
            "share official price percent requires forSale=Yes and saleMarketKind=token".into(),
        ));
    }

    Ok(binding)
}

pub fn validate_share_import(share: &Share) -> Result<(), SharePatchError> {
    if share.bindings.len() != 1 {
        return Err(SharePatchError::Invalid(
            "share must have exactly one binding".into(),
        ));
    }
    let binding = &share.bindings[0];
    if binding.app != share.app {
        return Err(SharePatchError::Invalid(
            "share binding app must match share.app".into(),
        ));
    }
    if binding.provider_id != share.provider_id {
        return Err(SharePatchError::Invalid(
            "share binding provider_id must match share.provider_id".into(),
        ));
    }

    let app_name = app_key(share.app);
    if share.access_by_app.keys().any(|key| key != &app_name) {
        return Err(SharePatchError::Invalid(
            "share access_by_app must only contain the share app".into(),
        ));
    }
    if share.app_settings.keys().any(|key| key != &app_name) {
        return Err(SharePatchError::Invalid(
            "share app_settings must only contain the share app".into(),
        ));
    }
    validate_sale_pricing(&share.for_sale_official_price_percent_by_app, &app_name)?;
    if !share_sale_pricing_is_eligible(share.for_sale, share.free_access, &share.sale_market_kind)
        && !share.for_sale_official_price_percent_by_app.is_empty()
    {
        return Err(SharePatchError::Invalid(
            "share official price percent requires forSale=Yes and saleMarketKind=token".into(),
        ));
    }

    Ok(())
}
