use crate::domain::providers::model::AppKind;
use crate::domain::sharing::shares::{Share, ShareAcl, ShareMarketGrantStatus};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) shares: Vec<Share>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportSharesRequest {
    pub(in crate::api) shares: Vec<Share>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ImportSharesResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) imported: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpsertShareResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) share: Share,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ShareConnectInfoResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) share_id: String,
    pub(in crate::api) direct_url: String,
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
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareSubdomainResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) remote_claimed: bool,
    pub(in crate::api) share: Share,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ReplaceShareAclRequest {
    pub(in crate::api) acl: ShareAcl,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct UpdateShareMarketGrantRequest {
    pub(in crate::api) market_grant: Option<ShareMarketGrantStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PublicShareMarket {
    pub(in crate::api) id: String,
    pub(in crate::api) display_name: String,
    pub(in crate::api) email: String,
    pub(in crate::api) subdomain: String,
    public_base_url: Option<String>,
    pub(in crate::api) market_kind: String,
    pub(in crate::api) status: String,
    #[serde(default)]
    pub(in crate::api) scopes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListShareMarketsResponse {
    #[serde(default)]
    pub(in crate::api) ok: bool,
    pub(in crate::api) markets: Vec<PublicShareMarket>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AuthorizeShareMarketRequest {
    pub(in crate::api) market_email: String,
}
