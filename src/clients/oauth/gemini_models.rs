use std::collections::BTreeMap;

use serde_json::Value;

use crate::domain::accounts::store::Account;
use crate::domain::providers::model::ProviderType;

pub const GEMINI_CODE_ASSIST_MODELS_SOURCE_URL: &str =
    "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota";

#[derive(Debug, Clone, PartialEq)]
pub struct GeminiCodeAssistModelDescriptor {
    pub model_id: String,
    pub display_name: Option<String>,
    pub remaining_fraction: Option<f64>,
    pub reset_time: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GeminiCodeAssistModelCatalog {
    pub descriptors: Vec<GeminiCodeAssistModelDescriptor>,
    pub source: String,
    pub stale: bool,
    pub fetched_at_ms: Option<i64>,
}

pub fn model_catalog_from_account(
    account: &Account,
    now_ms: i64,
    max_age_ms: i64,
    force_stale: bool,
) -> GeminiCodeAssistModelCatalog {
    if account.provider_type != ProviderType::GeminiCli {
        return GeminiCodeAssistModelCatalog {
            descriptors: Vec::new(),
            source: "provider_type_mismatch".to_string(),
            stale: true,
            fetched_at_ms: account.quota_refreshed_at,
        };
    }

    let descriptors = account
        .quota
        .as_ref()
        .and_then(|quota| quota.extra_usage.as_ref())
        .map(parse_model_descriptors)
        .unwrap_or_default();
    let fetched_at_ms = account.quota_refreshed_at;
    let age_stale = fetched_at_ms
        .is_none_or(|fetched_at| max_age_ms <= 0 || now_ms.saturating_sub(fetched_at) > max_age_ms);
    let stale = force_stale || age_stale || account.last_refresh_error.is_some();
    let source = if descriptors.is_empty() {
        "authenticated_quota_empty"
    } else if stale {
        "same_account_cached_retrieve_user_quota"
    } else {
        "authenticated_retrieve_user_quota"
    };

    GeminiCodeAssistModelCatalog {
        descriptors,
        source: source.to_string(),
        stale,
        fetched_at_ms,
    }
}

fn parse_model_descriptors(extra_usage: &Value) -> Vec<GeminiCodeAssistModelDescriptor> {
    let mut descriptors = BTreeMap::<String, GeminiCodeAssistModelDescriptor>::new();
    for pointer in [
        "/retrieveUserQuota/buckets",
        "/raw/retrieveUserQuota/buckets",
        "/buckets",
    ] {
        let Some(buckets) = extra_usage.pointer(pointer).and_then(Value::as_array) else {
            continue;
        };
        for bucket in buckets {
            let Some(model_id) = bucket
                .get("modelId")
                .or_else(|| bucket.get("model_id"))
                .or_else(|| bucket.get("id"))
                .and_then(Value::as_str)
                .and_then(normalize_model_id)
            else {
                continue;
            };
            let display_name = bucket
                .get("displayName")
                .or_else(|| bucket.get("display_name"))
                .or_else(|| bucket.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            let remaining_fraction = bucket
                .get("remainingFraction")
                .or_else(|| bucket.get("remaining_fraction"))
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite() && (0.0..=1.0).contains(value));
            let reset_time = bucket
                .get("resetTime")
                .or_else(|| bucket.get("reset_time"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            descriptors
                .entry(model_id.clone())
                .and_modify(|existing| {
                    if existing.display_name.is_none() {
                        existing.display_name = display_name.clone();
                    }
                    if remaining_fraction.is_some() {
                        existing.remaining_fraction = remaining_fraction;
                    }
                    if reset_time.is_some() {
                        existing.reset_time = reset_time.clone();
                    }
                })
                .or_insert(GeminiCodeAssistModelDescriptor {
                    model_id,
                    display_name,
                    remaining_fraction,
                    reset_time,
                });
        }
    }
    descriptors.into_values().collect()
}

fn normalize_model_id(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix("models/").unwrap_or(value.trim());
    if value.is_empty()
        || value.len() > 256
        || !value.starts_with("gemini-")
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn account(extra_usage: Value, refreshed_at: Option<i64>) -> Account {
        serde_json::from_value(json!({
            "id": "gemini-account",
            "providerType": "gemini_cli",
            "quota": {
                "success": true,
                "extraUsage": extra_usage
            },
            "quotaRefreshedAt": refreshed_at
        }))
        .expect("fixture account")
    }

    #[test]
    fn catalog_uses_exact_quota_model_ids_and_deduplicates() {
        let account = account(
            json!({
                "retrieveUserQuota": {
                    "buckets": [
                        {"modelId":"models/gemini-3.1-pro", "remainingFraction":0.75, "resetTime":"2026-09-01T00:00:00Z"},
                        {"modelId":"gemini-3.1-pro", "displayName":"Gemini 3.1 Pro", "remainingFraction":0.5},
                        {"modelId":"claude-sonnet-4-6", "remainingFraction":1.0},
                        {"modelId":"gemini invalid", "remainingFraction":1.0}
                    ]
                }
            }),
            Some(1_000),
        );

        let catalog = model_catalog_from_account(&account, 1_500, 10_000, false);
        assert_eq!(catalog.source, "authenticated_retrieve_user_quota");
        assert!(!catalog.stale);
        assert_eq!(catalog.descriptors.len(), 1);
        assert_eq!(catalog.descriptors[0].model_id, "gemini-3.1-pro");
        assert_eq!(
            catalog.descriptors[0].display_name.as_deref(),
            Some("Gemini 3.1 Pro")
        );
        assert_eq!(catalog.descriptors[0].remaining_fraction, Some(0.5));
    }

    #[test]
    fn refresh_failure_only_exposes_same_account_catalog_as_stale() {
        let mut account = account(
            json!({
                "retrieveUserQuota": {
                    "buckets": [{"modelId":"gemini-3.1-flash", "remainingFraction":0.0}]
                }
            }),
            Some(1_000),
        );
        account.last_refresh_error = Some("redacted upstream failure".to_string());

        let catalog = model_catalog_from_account(&account, 1_500, 10_000, true);
        assert!(catalog.stale);
        assert_eq!(catalog.source, "same_account_cached_retrieve_user_quota");
        assert_eq!(catalog.descriptors[0].remaining_fraction, Some(0.0));
    }

    #[test]
    fn wrong_provider_type_never_reuses_quota_catalog() {
        let mut account = account(
            json!({
                "retrieveUserQuota": {
                    "buckets": [{"modelId":"gemini-3.1-pro"}]
                }
            }),
            Some(1_000),
        );
        account.provider_type = ProviderType::AntigravityOAuth;

        let catalog = model_catalog_from_account(&account, 1_500, 10_000, false);
        assert!(catalog.descriptors.is_empty());
        assert_eq!(catalog.source, "provider_type_mismatch");
        assert!(catalog.stale);
    }
}
