use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::api::error::ApiError;
use crate::clients::oauth::quota::{
    refresh_account_quota, QuotaRefreshFailure, QuotaRefreshResult,
};
use crate::clients::oauth::refresh::account_needs_native_refresh;
use crate::clients::oauth::refresh::execute_native_account_refresh;
use crate::domain::accounts::store::AccountRefreshUpdate;
use crate::domain::providers::model::AppKind;
use crate::domain::providers::store::StoredProvider;
use crate::domain::sharing::router_contract::{
    descriptor_for_share_with_accounts_and_usage, ShareAppAvailability, ShareAppProviders,
    ShareAppRuntimes, ShareDescriptor, ShareRequestLogEntry, ShareSettingsPatch, ShareSupport,
};
use crate::domain::sharing::shares::Share;
use crate::domain::sharing::shares::ShareBinding;
use crate::state::ServerState;

use super::{
    clamp_u128_to_u64, now_ms, parse_app_kind, APPLY_SHARE_SETTINGS_PATH, REFRESH_SHARE_USAGE_PATH,
};

mod ctl;
mod share_router;

pub(crate) use ctl::{control_apply_share_settings, control_refresh_share_usage};
pub use ctl::{control_signature, refresh_share_usage_items, ControlRefreshShareUsageItem};
pub(crate) use share_router::{
    share_router_health, share_router_model_health, share_router_request_logs, share_router_runtime,
};
