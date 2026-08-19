use std::collections::BTreeSet;

use crate::domain::sharing::retired_fields::find_retired_share_field;
use crate::domain::sharing::shares::{Share, ShareBinding, SharePatchError, UpsertShareInput};

fn reject_retired_runtime_snapshot(
    snapshot: Option<&serde_json::Value>,
) -> Result<(), SharePatchError> {
    if let Some(snapshot) = snapshot {
        if let Some(field) = find_retired_share_field(snapshot) {
            return Err(SharePatchError::Invalid(format!(
                "retired Share field `{field}` is not accepted; use freeAccess/userGrants"
            )));
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

pub fn validate_and_normalize_upsert_input(
    input: &mut UpsertShareInput,
) -> Result<ShareBinding, SharePatchError> {
    reject_retired_runtime_snapshot(input.runtime_snapshot.as_ref())?;
    if input
        .banked_reset_expiry_lead_minutes
        .is_some_and(|minutes| {
            !(crate::domain::sharing::shares::MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES
                ..=crate::domain::sharing::shares::MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES)
                .contains(&minutes)
        })
    {
        return Err(SharePatchError::Invalid(format!(
            "bankedResetExpiryLeadMinutes must be between {} and {}",
            crate::domain::sharing::shares::MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES,
            crate::domain::sharing::shares::MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES
        )));
    }
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
    Ok(binding)
}

pub fn validate_share_import(share: &Share) -> Result<(), SharePatchError> {
    reject_retired_runtime_snapshot(share.runtime_snapshot.as_ref())?;
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
    if !(crate::domain::sharing::shares::MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES
        ..=crate::domain::sharing::shares::MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES)
        .contains(&share.banked_reset_expiry_lead_minutes)
    {
        return Err(SharePatchError::Invalid(format!(
            "bankedResetExpiryLeadMinutes must be between {} and {}",
            crate::domain::sharing::shares::MIN_BANKED_RESET_EXPIRY_LEAD_MINUTES,
            crate::domain::sharing::shares::MAX_BANKED_RESET_EXPIRY_LEAD_MINUTES
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{validate_and_normalize_upsert_input, validate_share_import};
    use crate::domain::sharing::shares::{Share, UpsertShareInput};

    #[test]
    fn upsert_validation_rejects_nested_retired_runtime_metadata() {
        let mut input: UpsertShareInput = serde_json::from_value(json!({
            "app": "codex",
            "providerId": "provider-1",
            "providerType": "codex",
            "runtimeSnapshot": {
                "details": [{"marketEmail": "retired@example.com"}]
            }
        }))
        .unwrap();
        let error = validate_and_normalize_upsert_input(&mut input).unwrap_err();
        assert!(error.to_string().contains("details[0].marketEmail"));
    }

    #[test]
    fn import_validation_rejects_nested_retired_runtime_metadata() {
        let share: Share = serde_json::from_value(json!({
            "id": "share-1",
            "app": "codex",
            "providerId": "provider-1",
            "providerType": "codex",
            "runtimeSnapshot": {
                "details": [{"marketEmail": "retired@example.com"}]
            }
        }))
        .unwrap();
        let error = validate_share_import(&share).unwrap_err();
        assert!(error.to_string().contains("details[0].marketEmail"));
    }
}
