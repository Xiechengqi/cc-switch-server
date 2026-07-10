use std::collections::BTreeMap;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::accounts::oauth::{
    build_authorize_url, build_cursor_poll_request, oauth_provider_spec,
    provider_login_request_shape_available, provider_token_exchange_available, OAuthAuthorizeFlow,
    OAuthHttpRequest, OAuthSupportStage,
};
use crate::domain::providers::model::ProviderType;

const LOGIN_SESSION_TTL_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Default)]
pub struct OAuthLoginStore {
    sessions: BTreeMap<String, OAuthLoginSession>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct OAuthLoginSession {
    session_id: String,
    provider_type: ProviderType,
    state: String,
    code_verifier: String,
    code_challenge: String,
    authorize_url: String,
    redirect_uri: Option<String>,
    flow: OAuthAuthorizeFlow,
    stage: OAuthSupportStage,
    created_at_ms: i64,
    expires_at_ms: i64,
    status: OAuthLoginStatus,
    authorization_code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OAuthSessionPollState {
    Pending,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthLoginStatus {
    Pending,
    TokenRequestPreviewed,
    TokenExchangeStarted,
    TokenExchanged,
    Expired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthLoginStart {
    pub provider_type: ProviderType,
    pub method: &'static str,
    pub session_id: String,
    pub state: String,
    pub authorize_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    pub code_challenge: String,
    pub code_challenge_method: &'static str,
    pub flow: OAuthAuthorizeFlow,
    pub status: OAuthLoginStatus,
    pub server_native_stage: OAuthSupportStage,
    pub expires_at_ms: i64,
    pub token_exchange_enabled: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthLoginFinish {
    pub provider_type: ProviderType,
    pub method: &'static str,
    pub session_id: String,
    pub state: String,
    pub flow: OAuthAuthorizeFlow,
    pub status: OAuthLoginStatus,
    pub token_exchange_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_request: Option<OAuthHttpRequest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_import_hint: Option<Value>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub enum OAuthLoginError {
    Unsupported(String),
    NotFound,
    Expired,
    StateMismatch,
    MissingCode,
    AlreadyConsumed,
    RequestShape(String),
}

impl std::fmt::Display for OAuthLoginError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(message) => formatter.write_str(message),
            Self::NotFound => formatter.write_str("oauth login session not found"),
            Self::Expired => formatter.write_str("oauth login session expired"),
            Self::StateMismatch => formatter.write_str("oauth login state does not match session"),
            Self::MissingCode => formatter.write_str("authorization code is required"),
            Self::AlreadyConsumed => formatter.write_str("oauth login session is already consumed"),
            Self::RequestShape(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for OAuthLoginError {}

impl OAuthLoginStore {
    pub fn start(
        &mut self,
        provider_type: ProviderType,
        redirect_uri: Option<String>,
        now_ms: i64,
    ) -> Result<OAuthLoginStart, OAuthLoginError> {
        self.cleanup_expired(now_ms);
        if !provider_login_request_shape_available(provider_type) {
            return Err(OAuthLoginError::Unsupported(format!(
                "{} browser login request shape is not available",
                provider_type.as_str()
            )));
        }
        let spec = oauth_provider_spec(provider_type).ok_or_else(|| {
            OAuthLoginError::Unsupported(format!(
                "{} does not have an oauth provider spec",
                provider_type.as_str()
            ))
        })?;
        let session_id = generate_base64url_token();
        let state = generate_base64url_token();
        let code_verifier = generate_pkce_verifier(provider_type);
        let code_challenge = generate_code_challenge(&code_verifier);
        let effective_redirect_uri = match spec.authorize_flow {
            OAuthAuthorizeFlow::CursorDeepControl => None,
            _ => redirect_uri.filter(|value| !value.trim().is_empty()),
        };
        let authorize_url = build_authorize_url(
            provider_type,
            effective_redirect_uri.as_deref(),
            Some(&code_challenge),
            &state,
        )
        .map_err(|error| OAuthLoginError::RequestShape(error.message))?;
        let expires_at_ms = now_ms.saturating_add(LOGIN_SESSION_TTL_MS);
        let session = OAuthLoginSession {
            session_id: session_id.clone(),
            provider_type,
            state: state.clone(),
            code_verifier,
            code_challenge: code_challenge.clone(),
            authorize_url: authorize_url.clone(),
            redirect_uri: effective_redirect_uri.clone(),
            flow: spec.authorize_flow,
            stage: spec.stage,
            created_at_ms: now_ms,
            expires_at_ms,
            status: OAuthLoginStatus::Pending,
            authorization_code: None,
        };
        self.sessions.insert(session_id.clone(), session);

        Ok(OAuthLoginStart {
            provider_type,
            method: "browser_oauth_request_shape",
            session_id,
            state,
            authorize_url,
            redirect_uri: effective_redirect_uri,
            code_challenge,
            code_challenge_method: "S256",
            flow: spec.authorize_flow,
            status: OAuthLoginStatus::Pending,
            server_native_stage: spec.stage,
            expires_at_ms,
            token_exchange_enabled: provider_token_exchange_available(provider_type),
            message: if provider_token_exchange_available(provider_type) {
                "login request shape created; token exchange/account import can be executed explicitly after callback validation".to_string()
            } else {
                "login request shape created; token exchange is disabled until provider-specific validation is complete".to_string()
            },
        })
    }

    pub fn finish(
        &mut self,
        session_id: Option<&str>,
        state: Option<&str>,
        code: Option<&str>,
        execute_token_exchange: bool,
        now_ms: i64,
    ) -> Result<OAuthLoginFinish, OAuthLoginError> {
        self.cleanup_expired(now_ms);
        let session_key = self.session_key(session_id, state)?;
        let session = self
            .sessions
            .get_mut(&session_key)
            .ok_or(OAuthLoginError::NotFound)?;
        if session.expires_at_ms <= now_ms {
            session.status = OAuthLoginStatus::Expired;
            return Err(OAuthLoginError::Expired);
        }
        if session.status == OAuthLoginStatus::TokenExchanged {
            return Err(OAuthLoginError::AlreadyConsumed);
        }
        if session.status == OAuthLoginStatus::TokenExchangeStarted {
            return Err(OAuthLoginError::AlreadyConsumed);
        }
        if execute_token_exchange && !provider_token_exchange_available(session.provider_type) {
            return Err(OAuthLoginError::Unsupported(format!(
                "{} token exchange is still preview-only",
                session.provider_type.as_str()
            )));
        }
        if let Some(state) = state {
            if session.state != state {
                return Err(OAuthLoginError::StateMismatch);
            }
        }

        if let Some(code) = code.map(str::trim).filter(|value| !value.is_empty()) {
            session.authorization_code = Some(code.to_string());
        }

        let token_request = match session.flow {
            OAuthAuthorizeFlow::AuthorizationCode | OAuthAuthorizeFlow::AuthorizationCodePkce => {
                let code = session
                    .authorization_code
                    .as_deref()
                    .ok_or(OAuthLoginError::MissingCode)?;
                let redirect_uri = session.redirect_uri.as_deref().ok_or_else(|| {
                    OAuthLoginError::RequestShape("redirect_uri is required".to_string())
                })?;
                let code_verifier =
                    matches!(session.flow, OAuthAuthorizeFlow::AuthorizationCodePkce)
                        .then_some(session.code_verifier.as_str());
                let (authorization_code, token_state) =
                    if session.provider_type == ProviderType::ClaudeOAuth {
                        super::oauth::parse_claude_authorization_code_input(code, &session.state)
                            .map_err(|error| OAuthLoginError::RequestShape(error.message))?
                    } else if session.provider_type == ProviderType::GrokOAuth {
                        super::oauth::parse_grok_authorization_code_input(code, &session.state)
                            .map_err(|error| OAuthLoginError::RequestShape(error.message))?
                    } else {
                        (code.to_string(), session.state.clone())
                    };
                Some(
                    super::oauth::build_authorization_code_request(
                        session.provider_type,
                        &authorization_code,
                        redirect_uri,
                        code_verifier,
                        &token_state,
                    )
                    .map_err(|error| OAuthLoginError::RequestShape(error.message))?,
                )
            }
            OAuthAuthorizeFlow::CursorDeepControl => Some(
                build_cursor_poll_request(&session.state, &session.code_verifier)
                    .map_err(|error| OAuthLoginError::RequestShape(error.message))?,
            ),
            OAuthAuthorizeFlow::Unsupported => return Err(OAuthLoginError::NotFound),
        };
        session.status = if execute_token_exchange {
            OAuthLoginStatus::TokenExchangeStarted
        } else {
            OAuthLoginStatus::TokenRequestPreviewed
        };

        Ok(OAuthLoginFinish {
            provider_type: session.provider_type,
            method: if execute_token_exchange {
                "token_exchange_request"
            } else {
                "token_exchange_request_preview"
            },
            session_id: session.session_id.clone(),
            state: session.state.clone(),
            flow: session.flow,
            status: session.status,
            token_exchange_enabled: provider_token_exchange_available(session.provider_type),
            token_request,
            account_import_hint: Some(serde_json::json!({
                "providerType": session.provider_type.as_str(),
                "nextStep": if provider_token_exchange_available(session.provider_type) {
                    "Send executeTokenExchange=true to exchange/poll provider credentials and import the account."
                } else {
                    "After provider-specific token exchange is enabled, the token response will be converted into an account import/update."
                },
            })),
            message: if execute_token_exchange {
                "authorization code was validated against the session; token exchange execution has started".to_string()
            } else if provider_token_exchange_available(session.provider_type) {
                "authorization code was validated against the session; send executeTokenExchange=true to exchange/poll and import the account".to_string()
            } else {
                "authorization code was validated against the session; token exchange request is preview-only for this provider".to_string()
            },
        })
    }

    pub fn mark_exchanged(&mut self, session_id: &str) -> Result<(), OAuthLoginError> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or(OAuthLoginError::NotFound)?;
        session.status = OAuthLoginStatus::TokenExchanged;
        Ok(())
    }

    pub fn mark_exchange_failed(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.get_mut(session_id) {
            if session.status == OAuthLoginStatus::TokenExchangeStarted {
                session.status = OAuthLoginStatus::TokenRequestPreviewed;
            }
        }
    }

    pub fn poll_state_by_oauth_state(
        &mut self,
        state: &str,
        now_ms: i64,
    ) -> Result<OAuthSessionPollState, OAuthLoginError> {
        self.cleanup_expired(now_ms);
        let session = self
            .sessions
            .values()
            .find(|session| session.state == state)
            .ok_or(OAuthLoginError::NotFound)?;
        if session.expires_at_ms <= now_ms {
            return Err(OAuthLoginError::Expired);
        }
        if session.status == OAuthLoginStatus::TokenExchanged {
            return Err(OAuthLoginError::AlreadyConsumed);
        }
        match session.flow {
            OAuthAuthorizeFlow::CursorDeepControl => {
                if matches!(
                    session.status,
                    OAuthLoginStatus::Pending | OAuthLoginStatus::TokenRequestPreviewed
                ) {
                    Ok(OAuthSessionPollState::Ready)
                } else {
                    Ok(OAuthSessionPollState::Pending)
                }
            }
            OAuthAuthorizeFlow::AuthorizationCode | OAuthAuthorizeFlow::AuthorizationCodePkce => {
                if session.status == OAuthLoginStatus::TokenRequestPreviewed
                    && session.authorization_code.is_some()
                {
                    Ok(OAuthSessionPollState::Ready)
                } else {
                    Ok(OAuthSessionPollState::Pending)
                }
            }
            OAuthAuthorizeFlow::Unsupported => Err(OAuthLoginError::Unsupported(
                "oauth login flow is unsupported".to_string(),
            )),
        }
    }

    fn session_key(
        &self,
        session_id: Option<&str>,
        state: Option<&str>,
    ) -> Result<String, OAuthLoginError> {
        if let Some(session_id) = session_id.map(str::trim).filter(|value| !value.is_empty()) {
            if let Some(state) = state.map(str::trim).filter(|value| !value.is_empty()) {
                let session = self
                    .sessions
                    .get(session_id)
                    .ok_or(OAuthLoginError::NotFound)?;
                if session.state != state {
                    return Err(OAuthLoginError::StateMismatch);
                }
            }
            return Ok(session_id.to_string());
        }
        let state = state
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or(OAuthLoginError::NotFound)?;
        self.sessions
            .iter()
            .find(|(_, session)| session.state == state)
            .map(|(session_id, _)| session_id.clone())
            .ok_or(OAuthLoginError::NotFound)
    }

    fn cleanup_expired(&mut self, now_ms: i64) {
        self.sessions
            .retain(|_, session| session.expires_at_ms > now_ms);
    }
}

fn generate_base64url_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_pkce_verifier(provider_type: ProviderType) -> String {
    if provider_type == ProviderType::GrokOAuth {
        let mut bytes = [0u8; 96];
        rand::thread_rng().fill_bytes(&mut bytes);
        return URL_SAFE_NO_PAD.encode(bytes);
    }
    generate_base64url_token()
}

fn generate_code_challenge(code_verifier: &str) -> String {
    let digest = Sha256::digest(code_verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static OAUTH_ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        name: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(name: &'static str, value: &'static str) -> Self {
            let previous = std::env::var(name).ok();
            std::env::set_var(name, value);
            Self { name, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.name, previous);
            } else {
                std::env::remove_var(self.name);
            }
        }
    }

    fn google_oauth_test_env() -> (MutexGuard<'static, ()>, Vec<EnvGuard>) {
        let guard = OAUTH_ENV_LOCK.lock().expect("oauth env lock");
        let vars = vec![
            EnvGuard::set("CC_SWITCH_SERVER_GEMINI_CLIENT_ID", "gemini-client-fixture"),
            EnvGuard::set(
                "CC_SWITCH_SERVER_GEMINI_CLIENT_SECRET",
                "gemini-secret-fixture",
            ),
            EnvGuard::set(
                "CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_ID",
                "antigravity-client-fixture",
            ),
            EnvGuard::set(
                "CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_SECRET",
                "antigravity-secret-fixture",
            ),
        ];
        (guard, vars)
    }

    #[test]
    fn codex_start_builds_pkce_session_with_explicit_exchange_available() {
        let mut store = OAuthLoginStore::default();
        let login = store
            .start(
                ProviderType::CodexOAuth,
                Some("http://localhost:15721/api/accounts/login/callback".to_string()),
                1_000,
            )
            .expect("login");

        assert_eq!(login.provider_type, ProviderType::CodexOAuth);
        assert_eq!(login.status, OAuthLoginStatus::Pending);
        assert!(login.token_exchange_enabled);
        assert!(login.authorize_url.contains("code_challenge_method=S256"));
        assert!(login.authorize_url.contains("originator=codex_cli_rs"));
        assert_eq!(login.expires_at_ms, 301_000);
    }

    #[test]
    fn finish_preview_does_not_consume_session() {
        let mut store = OAuthLoginStore::default();
        let login = store
            .start(
                ProviderType::ClaudeOAuth,
                Some("http://localhost:15721/api/accounts/login/callback".to_string()),
                1_000,
            )
            .expect("login");
        let finish = store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                false,
                2_000,
            )
            .expect("finish");

        assert_eq!(finish.status, OAuthLoginStatus::TokenRequestPreviewed);
        assert!(finish.token_exchange_enabled);
        let request = finish.token_request.expect("token request");
        assert_eq!(request.method, "POST");
        assert_eq!(request.body["grant_type"], "authorization_code");
        assert_eq!(request.body["code"], "auth-code");
        assert!(request.body.get("code_verifier").is_some());
        let second_preview = store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                false,
                2_001,
            )
            .expect("preview remains available");
        assert_eq!(
            second_preview.status,
            OAuthLoginStatus::TokenRequestPreviewed
        );
    }

    #[test]
    fn cursor_finish_returns_poll_request_shape_without_code() {
        let mut store = OAuthLoginStore::default();
        let login = store
            .start(ProviderType::CursorOAuth, None, 1_000)
            .expect("cursor login");
        let finish = store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                None,
                false,
                2_000,
            )
            .expect("finish");
        let request = finish.token_request.expect("poll request");

        assert_eq!(request.method, "GET");
        assert!(request.url.contains("https://api2.cursor.sh/auth/poll?"));
        assert!(request.url.contains("uuid="));
        assert!(request.url.contains("verifier="));
    }

    #[test]
    fn non_pkce_authorization_code_finish_omits_code_verifier() {
        let _env = google_oauth_test_env();
        let mut store = OAuthLoginStore::default();
        let login = store
            .start(
                ProviderType::GeminiCli,
                Some("http://localhost:15721/api/accounts/login/callback".to_string()),
                1_000,
            )
            .expect("gemini login");
        let finish = store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                false,
                2_000,
            )
            .expect("finish");
        let request = finish.token_request.expect("token request");

        assert_eq!(request.body["grant_type"], "authorization_code");
        assert_eq!(request.body["code"], "auth-code");
        assert!(request.body.get("code_verifier").is_none());
    }

    #[test]
    fn expired_sessions_are_rejected() {
        let mut store = OAuthLoginStore::default();
        let login = store
            .start(
                ProviderType::CodexOAuth,
                Some("http://localhost:15721/api/accounts/login/callback".to_string()),
                1_000,
            )
            .expect("login");
        let error = store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                false,
                302_000,
            )
            .expect_err("expired");

        assert!(matches!(
            error,
            OAuthLoginError::NotFound | OAuthLoginError::Expired
        ));
    }

    #[test]
    fn codex_exchange_mode_marks_session_in_progress_and_can_complete() {
        let mut store = OAuthLoginStore::default();
        let login = store
            .start(
                ProviderType::CodexOAuth,
                Some("http://localhost:15721/api/accounts/login/callback".to_string()),
                1_000,
            )
            .expect("login");

        let finish = store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                true,
                2_000,
            )
            .expect("begin exchange");
        assert_eq!(finish.status, OAuthLoginStatus::TokenExchangeStarted);
        assert!(finish.token_exchange_enabled);
        assert!(store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                true,
                2_001,
            )
            .is_err());

        store.mark_exchange_failed(&login.session_id);
        assert!(store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                true,
                2_002,
            )
            .is_ok());
        store
            .mark_exchanged(&login.session_id)
            .expect("mark exchanged");
        assert!(store
            .finish(
                Some(&login.session_id),
                Some(&login.state),
                Some("auth-code"),
                false,
                2_003,
            )
            .is_err());
    }

    #[test]
    fn account_import_exchange_is_available_for_supported_login_providers() {
        let _env = google_oauth_test_env();
        for provider_type in [
            ProviderType::CodexOAuth,
            ProviderType::ClaudeOAuth,
            ProviderType::GeminiCli,
            ProviderType::CursorOAuth,
            ProviderType::AntigravityOAuth,
            ProviderType::AgyOAuth,
        ] {
            let mut store = OAuthLoginStore::default();
            let login = store
                .start(
                    provider_type,
                    Some("http://localhost:15721/api/accounts/login/callback".to_string()),
                    1_000,
                )
                .expect("login");
            assert!(login.token_exchange_enabled);
        }
    }
}
