//! Explicit product/site imports. External exports are input formats, never
//! runtime configuration; only reviewed credential/profile fields survive.
use serde_json::{json, Value};

use super::store::UpsertAccountInput;
use crate::domain::codebuddy::{buddy_account_id, buddy_product_policy, CodeBuddySite};
use crate::domain::providers::model::ProviderType;

pub fn normalize_buddy_import(
    product: ProviderType,
    site: CodeBuddySite,
    payload: &Value,
) -> Result<Vec<UpsertAccountInput>, String> {
    let policy = buddy_product_policy(product, site)?;
    if serde_json::to_vec(payload)
        .map_err(|_| "invalid credentials JSON")?
        .len()
        > 1024 * 1024
    {
        return Err("credential import exceeds 1 MiB".to_string());
    }
    fn encrypted(value: &Value) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key("$wbEncrypted") || object.values().any(encrypted)
            }
            Value::Array(values) => values.iter().any(encrypted),
            _ => false,
        }
    }
    if encrypted(payload) {
        return Err(
            "desktop $wbEncrypted credentials cannot be decrypted by Server; use native login"
                .to_string(),
        );
    }
    let validate_scope = |value: &Value| -> Result<(), String> {
        for key in ["providerType", "provider_type"] {
            if value
                .get(key)
                .is_some_and(|value| value.as_str() != Some(product.as_str()))
            {
                return Err("imported product does not match the explicit providerType".to_string());
            }
        }
        if value.get("site").is_some_and(|value| {
            value
                .as_str()
                .and_then(|value| CodeBuddySite::parse(value).ok())
                != Some(site)
        }) {
            return Err("imported site does not match the explicit site".to_string());
        }
        for key in ["productPlatform", "product_platform", "platform"] {
            if value
                .get(key)
                .is_some_and(|value| value.as_str() != Some(policy.auth_platform))
            {
                return Err(
                    "imported platform does not match the explicit product and site".to_string(),
                );
            }
        }
        Ok(())
    };
    validate_scope(payload)?;
    let entries: Vec<&Value> = if let Some(values) = payload.as_array() {
        values.iter().collect()
    } else if let Some(values) = payload.get("accounts").and_then(Value::as_array) {
        values.iter().collect()
    } else {
        vec![payload]
    };
    if entries.is_empty() || entries.len() > 32 {
        return Err("import requires 1 to 32 accounts".to_string());
    }
    let mut identities = std::collections::BTreeSet::new();
    entries
        .into_iter()
        .map(|entry| {
            if !entry.is_object() {
                return Err("credential entry must be an object".to_string());
            }
            let auth = entry.get("auth").unwrap_or(entry);
            let profile = entry
                .get("account")
                .or_else(|| entry.get("profile"))
                .unwrap_or(entry);
            for value in [entry, auth, profile] {
                validate_scope(value)?;
            }
            let field = |value: &Value, keys: &[&str]| -> Option<String> {
                keys.iter()
                    .find_map(|key| value.get(*key).and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            };
            let access = field(auth, &["accessToken", "access_token"])
                .ok_or("import is missing accessToken")?;
            let refresh = field(auth, &["refreshToken", "refresh_token"])
                .ok_or("import is missing refreshToken")?;
            let uid =
                field(profile, &["uid", "userId", "user_id"]).ok_or("import is missing uid")?;
            let domain = field(auth, &["domain"])
                .or_else(|| field(profile, &["domain"]))
                .ok_or("import is missing domain")?;
            let domain = site
                .canonical_token_domain(&domain)
                .ok_or("import domain is outside the explicit site")?;
            let enterprise = field(profile, &["enterpriseId", "enterprise_id", "enterpriseID"])
                .unwrap_or_default();
            if !enterprise.is_empty() {
                return Err("enterprise accounts are not enabled".to_string());
            }
            let expires = auth
                .get("expiresAt")
                .or_else(|| auth.get("expires_at"))
                .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
                .filter(|value| *value > 0)
                .map(|value| {
                    if value < 10_000_000_000 {
                        value.saturating_mul(1000)
                    } else {
                        value
                    }
                });
            let input: UpsertAccountInput = serde_json::from_value(json!({
                "id":buddy_account_id(product, site, &uid, &enterprise)?,
                "providerType":product.as_str(),"accessToken":access,"refreshToken":refresh,
                "tokenType":"Bearer","expiresAt":expires,
                "email":field(profile, &["email"]),
                "profile":{"site":site,"domain":domain,"uid":uid,"enterpriseId":enterprise,
                    "name":field(profile, &["name", "nickname"]).unwrap_or_default(),
                    "email":field(profile, &["email"]).unwrap_or_default(),
                    "productPlatform":policy.auth_platform},
                "raw":{"source":"explicit_buddy_import"}
            }))
            .map_err(|_| "invalid normalized credential input")?;
            if !identities.insert(input.id.clone()) {
                return Err("duplicate account identity in import payload".to_string());
            }
            Ok(input)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codebuddy_workbuddy_imports_require_product_and_site_and_drop_untrusted_fields() {
        for product in [ProviderType::CodeBuddyOAuth, ProviderType::WorkBuddyOAuth] {
            let payload = json!({"account":{"uid":"same-user","email":"test@example.com"},
                "auth":{"accessToken":"access","refreshToken":"refresh","domain":"www.workbuddy.ai","expiresAt":2000000000},
                "input":"never-copy", "extraHeaders":{"X-Domain":"evil.test"}});
            let inputs = normalize_buddy_import(product, CodeBuddySite::Intl, &payload).unwrap();
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0].provider_type, product);
            assert!(inputs[0].extra_headers.is_none());
            assert_eq!(inputs[0].expires_at, Some(2000000000000));
            assert!(!inputs[0]
                .raw
                .as_ref()
                .unwrap()
                .to_string()
                .contains("never-copy"));
            assert!(normalize_buddy_import(product, CodeBuddySite::Cn, &payload).is_err());
        }
        assert!(normalize_buddy_import(
            ProviderType::WorkBuddyOAuth,
            CodeBuddySite::Intl,
            &json!({"$wbEncrypted":true})
        )
        .is_err());
        assert!(normalize_buddy_import(
            ProviderType::WorkBuddyOAuth,
            CodeBuddySite::Intl,
            &json!([])
        )
        .is_err());
    }
}
