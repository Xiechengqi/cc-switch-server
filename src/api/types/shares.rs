use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::sharing::retired_fields::find_retired_share_field;
use crate::domain::sharing::shares::Share;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) shares: Vec<Share>,
}

#[derive(Debug)]
pub(in crate::api) struct ImportSharesRequest {
    pub(in crate::api) shares: Vec<Share>,
}

impl<'de> Deserialize<'de> for ImportSharesRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct CanonicalImport {
            shares: Vec<Share>,
        }

        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(field) = find_retired_share_field(&value) {
            return Err(serde::de::Error::custom(format!(
                "retired Share field `{field}` is not accepted; use freeAccess/userGrants"
            )));
        }
        let shares = value
            .get("shares")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| serde::de::Error::custom("shares must be an array"))?;
        for share in shares {
            if !share.is_object() {
                return Err(serde::de::Error::custom("each Share must be an object"));
            }
        }
        let input: CanonicalImport =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            shares: input.shares,
        })
    }
}

#[cfg(test)]
mod import_shares_request_tests {
    use super::*;

    #[test]
    fn import_rejects_retired_share_fields_in_both_naming_styles() {
        for field in [
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
        ] {
            let mut share = serde_json::Map::new();
            share.insert(field.to_string(), serde_json::Value::Null);
            let value = serde_json::json!({"shares": [share]});
            let error = serde_json::from_value::<ImportSharesRequest>(value)
                .expect_err("retired Share field must be rejected");
            assert!(error.to_string().contains(field), "field={field}: {error}");
        }
    }

    #[test]
    fn import_rejects_retired_fields_nested_in_runtime_snapshot() {
        let value = serde_json::json!({
            "shares": [{
                "id": "share-nested",
                "runtimeSnapshot": {
                    "nested": [{"marketEmail": "retired@example.com"}]
                }
            }]
        });
        let error = serde_json::from_value::<ImportSharesRequest>(value)
            .expect_err("nested retired Share field must be rejected");
        assert!(error
            .to_string()
            .contains("runtimeSnapshot.nested[0].marketEmail"));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) imported: usize,
    pub(in crate::api) owner_normalized: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertShareResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) share: Share,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareBindingRequest {
    pub(in crate::api) provider_id: String,
    pub(in crate::api) provider_type: ProviderType,
    pub(in crate::api) expected_config_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareReuseCandidatesQuery {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareReuseCandidate {
    pub(in crate::api) share_id: String,
    pub(in crate::api) share_name: String,
    pub(in crate::api) subdomain: Option<String>,
    pub(in crate::api) apps: Vec<AppKind>,
    pub(in crate::api) config_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareReuseCandidatesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) candidates: Vec<ShareReuseCandidate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AddShareBindingRequest {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) expected_config_revision: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RemoveShareBindingRequest {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) provider_id: String,
    pub(in crate::api) expected_config_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct RemoveShareBindingResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) deleted_share: bool,
    pub(in crate::api) share: Option<Share>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareConnectInfoResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) share_id: String,
    pub(in crate::api) tunnel_url: String,
    pub(in crate::api) subdomain: String,
    pub(in crate::api) router_domain: String,
    pub(in crate::api) snippets: Vec<ShareConnectSnippet>,
    pub(in crate::api) note: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareConnectSnippet {
    pub(in crate::api) app: AppKind,
    pub(in crate::api) title: String,
    pub(in crate::api) env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareSubdomainRequest {
    pub(in crate::api) subdomain: String,
    #[serde(default)]
    pub(in crate::api) expected_config_revision: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareSubdomainResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) remote_claimed: bool,
    pub(in crate::api) share: Share,
}
