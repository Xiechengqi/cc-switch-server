use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::sharing::shares::{Share, ShareAcl};
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ReplaceShareAclRequest {
    pub(in crate::api) acl: ShareAcl,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct PublicTokenMarket {
    pub(in crate::api) id: String,
    pub(in crate::api) display_name: String,
    pub(in crate::api) email: String,
    pub(in crate::api) subdomain: String,
    pub(in crate::api) public_base_url: Option<String>,
    pub(in crate::api) market_kind: String,
    pub(in crate::api) status: String,
    #[serde(default)]
    pub(in crate::api) scopes: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ListTokenMarketsResponse {
    #[serde(default)]
    pub(in crate::api) ok: bool,
    pub(in crate::api) markets: Vec<PublicTokenMarket>,
}
