use serde_json::Value;

/// JSON field names owned by the retired v1 Token Market/Share-sale
/// contract.  They are kept in one compatibility boundary so every write
/// surface (migration, REST, invoke and domain mutation) applies the same
/// fail-closed rule.
pub(crate) const RETIRED_SHARE_FIELDS: &[&str] = &[
    "acl",
    "forSale",
    "for_sale",
    "officialPricePercent",
    "official_price_percent",
    "forSaleOfficialPricePercentByApp",
    "for_sale_official_price_percent_by_app",
    "sharedWithEmails",
    "shared_with_emails",
    "marketAccessMode",
    "market_access_mode",
    "accessByApp",
    "access_by_app",
    "appSettings",
    "app_settings",
    "publicMarketEmail",
    "public_market_email",
    "marketEmail",
    "market_email",
    "marketSubdomain",
    "market_subdomain",
    "marketUrl",
    "market_url",
    "marketId",
    "market_id",
    "saleMarketKind",
    "sale_market_kind",
];

/// Return the first retired field found anywhere in a JSON payload.
///
/// Share `runtimeSnapshot` is intentionally a forward-compatible raw JSON
/// value.  Checking only the Share object's top-level keys therefore allows
/// an old Market/ACL field to be smuggled back into persistent state inside a
/// nested object.  The returned path is diagnostic only; callers must reject
/// the complete payload rather than attempting to strip user input.
pub(crate) fn find_retired_share_field(value: &Value) -> Option<String> {
    fn walk(value: &Value, path: &mut Vec<String>) -> Option<String> {
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    path.push(key.clone());
                    if RETIRED_SHARE_FIELDS.contains(&key.as_str()) {
                        let found = format_json_path(path);
                        path.pop();
                        return Some(found);
                    }
                    if let Some(found) = walk(child, path) {
                        path.pop();
                        return Some(found);
                    }
                    path.pop();
                }
                None
            }
            Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    path.push(format!("[{index}]"));
                    if let Some(found) = walk(child, path) {
                        path.pop();
                        return Some(found);
                    }
                    path.pop();
                }
                None
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => None,
        }
    }

    fn format_json_path(path: &[String]) -> String {
        let mut formatted = String::new();
        for segment in path {
            if segment.starts_with('[') {
                formatted.push_str(segment);
            } else {
                if !formatted.is_empty() {
                    formatted.push('.');
                }
                formatted.push_str(segment);
            }
        }
        formatted
    }

    walk(value, &mut Vec::new())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::find_retired_share_field;

    #[test]
    fn finds_retired_fields_at_any_nesting_depth() {
        let value = json!({
            "shares": [{
                "runtimeSnapshot": {
                    "nested": [{"provider": {"marketEmail": "retired@example.com"}}]
                }
            }]
        });
        assert_eq!(
            find_retired_share_field(&value).as_deref(),
            Some("shares[0].runtimeSnapshot.nested[0].provider.marketEmail")
        );
    }

    #[test]
    fn accepts_canonical_nested_runtime_metadata() {
        let value = json!({
            "runtimeSnapshot": {
                "providerId": "provider-1",
                "nested": [{"keep": true}]
            }
        });
        assert_eq!(find_retired_share_field(&value), None);
    }
}
