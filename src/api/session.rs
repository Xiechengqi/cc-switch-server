use axum::http::HeaderMap;
use rand::RngCore;

use crate::api::error::ApiError;
use crate::state::ServerState;

pub(crate) async fn require_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    require_web_admin_session(state, headers).await.map(|_| ())
}

pub(crate) async fn require_web_admin_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<WebAdminPrincipal, ApiError> {
    resolve_web_admin_principal(state, headers)
        .await?
        .ok_or_else(|| ApiError::unauthorized("missing or invalid bearer token"))
}

pub(crate) async fn resolve_web_admin_principal(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<Option<WebAdminPrincipal>, ApiError> {
    if let Some(user_email) = router_web_user_email(headers) {
        return Ok(Some(WebAdminPrincipal {
            user_email,
            role: router_web_role(headers),
        }));
    }

    let Some(token) = bearer_token(headers) else {
        return Ok(None);
    };

    if let Ok(Some(principal)) = state.web_auth.authenticate_access_token(token) {
        return Ok(Some(WebAdminPrincipal {
            user_email: principal.user_email,
            role: principal.role,
        }));
    }

    if state
        .sessions
        .read()
        .await
        .iter()
        .any(|session| session.token == token)
    {
        let config = state.config.read().await;
        return Ok(Some(WebAdminPrincipal {
            user_email: config
                .owner
                .email
                .clone()
                .unwrap_or_else(|| "local-admin@cc-switch.local".to_string()),
            role: "admin".to_string(),
        }));
    }

    let config = state.config.read().await;
    if config.verify_api_token(token) {
        return Ok(Some(WebAdminPrincipal {
            user_email: config
                .owner
                .email
                .clone()
                .unwrap_or_else(|| "local-admin@cc-switch.local".to_string()),
            role: "admin".to_string(),
        }));
    }

    Ok(None)
}

pub(crate) fn is_router_delegated_session(headers: &HeaderMap) -> bool {
    router_web_user_email(headers).is_some()
}

pub(crate) async fn require_local_server_owner_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<WebAdminPrincipal, ApiError> {
    if is_router_delegated_session(headers) {
        return Err(ApiError::forbidden(
            "only local server owner can change this setting",
        ));
    }
    require_web_admin_session(state, headers).await
}

pub(crate) fn router_web_user_email(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-cc-switch-web-user-email")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
}

pub(crate) fn router_web_role(headers: &HeaderMap) -> String {
    headers
        .get("x-cc-switch-web-role")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("owner")
        .to_string()
}

#[derive(Debug, Clone)]
pub(crate) struct WebAdminPrincipal {
    user_email: String,
    role: String,
}

impl WebAdminPrincipal {
    pub(crate) fn user_email(&self) -> &str {
        &self.user_email
    }

    #[allow(dead_code)]
    pub(crate) fn role(&self) -> &str {
        &self.role
    }
}

pub(crate) async fn require_event_session(
    state: &ServerState,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    if resolve_web_admin_principal(state, headers).await?.is_some() {
        return Ok(());
    }
    if let Some(token) = bearer_token(headers) {
        if state
            .web_auth
            .authenticate_access_token(token)
            .ok()
            .flatten()
            .is_some()
        {
            return Ok(());
        }
        let config = state.config.read().await;
        if config.verify_api_token(token) {
            return Ok(());
        }
        return require_session_token(state, token).await;
    }
    Err(ApiError::unauthorized("missing bearer token"))
}

pub(crate) async fn require_session_token(
    state: &ServerState,
    token: &str,
) -> Result<(), ApiError> {
    if state
        .sessions
        .read()
        .await
        .iter()
        .any(|session| session.token == token)
    {
        Ok(())
    } else {
        Err(ApiError::unauthorized("invalid bearer token"))
    }
}

pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

pub(crate) fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::bearer_token;

    #[test]
    fn bearer_token_accepts_only_bearer_authorization() {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token-1"),
        );
        assert_eq!(bearer_token(&headers), Some("token-1"));

        headers.insert(
            axum::http::header::AUTHORIZATION,
            HeaderValue::from_static("Basic token-1"),
        );
        assert_eq!(bearer_token(&headers), None);
    }
}
