use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct LoginRequest {
    #[serde(default = "default_password_method")]
    pub(in crate::api) method: String,
    #[serde(default)]
    pub(in crate::api) password: String,
    #[serde(default)]
    pub(in crate::api) api_token: Option<String>,
    #[serde(default)]
    pub(in crate::api) email: Option<String>,
    #[serde(default)]
    pub(in crate::api) code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ChangePasswordRequest {
    pub(in crate::api) new_password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ChangePasswordResponse {
    pub(in crate::api) ok: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct EmailLoginCodeRequest {
    pub(in crate::api) email: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct EmailLoginVerifyCodeRequest {
    pub(in crate::api) email: String,
    pub(in crate::api) code: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct WebPasswordRequest {
    pub(in crate::api) password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct WebSessionRefreshRequest {
    pub(in crate::api) refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct WebPasswordChangeRequest {
    pub(in crate::api) current_password: String,
    pub(in crate::api) new_password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct LoginResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) token: String,
    pub(in crate::api) token_type: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct ApiTokenResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) api_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct AuthMeResponse {
    pub(in crate::api) ok: bool,
    pub(in crate::api) owner_email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::api) struct EventQuery {
    #[serde(default)]
    pub(in crate::api) token: Option<String>,
}

fn default_password_method() -> String {
    "password".to_string()
}
