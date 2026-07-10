use super::*;

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::settings::config::{
    PayoutAddressType, PayoutNetwork, PayoutProfile, PayoutProfileState, PayoutProfileSyncStatus,
    PayoutToken, PayoutVerificationStatus, PAYOUT_PROFILE_SCHEMA_VERSION,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::api) struct SavePayoutProfileInput {
    address: String,
    token: PayoutToken,
    networks: Vec<PayoutNetwork>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AdminPayoutProfileResponse {
    schema_version: u32,
    revision: i64,
    configured: bool,
    owner_email: Option<String>,
    installation_id: Option<String>,
    profile: Option<PayoutProfile>,
    updated_at: Option<String>,
    sync: PayoutProfileSyncStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublicPayoutProfileResponse {
    schema_version: u32,
    revision: i64,
    configured: bool,
    owner_email: Option<String>,
    installation_id: Option<String>,
    profile: Option<PayoutProfile>,
    updated_at: Option<String>,
}

pub(in crate::api) async fn get_payout_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AdminPayoutProfileResponse>, ApiError> {
    require_session(&state, &headers).await?;
    Ok(Json(admin_response(&state.config_snapshot().await)))
}

pub(in crate::api) async fn save_payout_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
    Json(input): Json<SavePayoutProfileInput>,
) -> Result<Json<AdminPayoutProfileResponse>, ApiError> {
    require_session(&state, &headers).await?;
    let profile = PayoutProfile {
        address_type: PayoutAddressType::Evm,
        address: input.address,
        token: input.token,
        networks: input.networks,
        verification_status: PayoutVerificationStatus::SelfDeclared,
    }
    .validate_and_normalize()
    .map_err(ApiError::bad_request)?;
    state
        .update_owner_payout_profile(profile)
        .await
        .map_err(ApiError::internal)?;
    reconcile_without_rollback(state.clone()).await;
    Ok(Json(admin_response(&state.config_snapshot().await)))
}

pub(in crate::api) async fn clear_payout_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Json<AdminPayoutProfileResponse>, ApiError> {
    require_session(&state, &headers).await?;
    state
        .clear_owner_payout_profile()
        .await
        .map_err(ApiError::internal)?;
    reconcile_without_rollback(state.clone()).await;
    Ok(Json(admin_response(&state.config_snapshot().await)))
}

pub(in crate::api) async fn public_payout_profile(
    State(state): State<ServerState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let config = state.config_snapshot().await;
    let payout = &config.owner.payout_profile;
    let response = PublicPayoutProfileResponse {
        schema_version: PAYOUT_PROFILE_SCHEMA_VERSION,
        revision: payout.revision,
        configured: payout.profile.is_some(),
        owner_email: config.owner.email.clone(),
        installation_id: config
            .router
            .identity
            .as_ref()
            .map(|identity| identity.installation_id.clone()),
        profile: payout.profile.clone(),
        updated_at: payout_updated_at(payout),
    };
    cached_public_json(&headers, &response).map_err(ApiError::internal)
}

async fn reconcile_without_rollback(state: ServerState) {
    if let Err(error) = crate::state::reconcile_payout_profile_to_router(state).await {
        tracing::warn!(error = %error, "router payout profile sync failed; local configuration remains active");
    }
}

fn admin_response(
    config: &crate::domain::settings::config::ServerConfig,
) -> AdminPayoutProfileResponse {
    let payout = &config.owner.payout_profile;
    AdminPayoutProfileResponse {
        schema_version: PAYOUT_PROFILE_SCHEMA_VERSION,
        revision: payout.revision,
        configured: payout.profile.is_some(),
        owner_email: config.owner.email.clone(),
        installation_id: config
            .router
            .identity
            .as_ref()
            .map(|identity| identity.installation_id.clone()),
        profile: payout.profile.clone(),
        updated_at: payout_updated_at(payout),
        sync: config.owner.payout_profile_sync.clone(),
    }
}

fn payout_updated_at(state: &PayoutProfileState) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(state.updated_at_ms).map(|value| value.to_rfc3339())
}

fn cached_public_json<T: Serialize>(headers: &HeaderMap, value: &T) -> anyhow::Result<Response> {
    let body = serde_json::to_vec(value)?;
    let etag = format!("\"{}\"", hex::encode(Sha256::digest(&body)));
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == etag))
    {
        return Ok(Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, etag)
            .header(header::CACHE_CONTROL, "public, max-age=60")
            .body(Body::empty())?);
    }
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "public, max-age=60")
        .header(header::ETAG, etag)
        .body(Body::from(body))?)
}
