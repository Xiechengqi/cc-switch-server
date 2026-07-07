use crate::domain::providers::model::AppKind;
use crate::domain::sharing::shares::{Share, ShareBinding, SharePatchError, UpsertShareInput};

fn app_key(app: AppKind) -> String {
    app.as_str().to_string()
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
    input
        .for_sale_official_price_percent_by_app
        .retain(|key, _| key == &app_name);

    Ok(binding)
}

pub fn validate_share_import(share: &Share) -> Result<(), SharePatchError> {
    if share.bindings.len() > 1 {
        return Err(SharePatchError::Invalid(
            "share must have at most one binding".into(),
        ));
    }
    if let Some(binding) = share.bindings.first() {
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
    } else if share.provider_id.trim().is_empty() {
        return Err(SharePatchError::Invalid(
            "share provider_id is required".into(),
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
    if share
        .for_sale_official_price_percent_by_app
        .keys()
        .any(|key| key != &app_name)
    {
        return Err(SharePatchError::Invalid(
            "share for_sale_official_price_percent_by_app must only contain the share app".into(),
        ));
    }

    Ok(())
}
