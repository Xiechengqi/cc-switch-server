use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::domain::settings::config::ServerConfig;

const SESSIONS_FILE_NAME: &str = "web-auth-sessions.json";
const ACCESS_TTL_SECS: i64 = 60 * 60;
const REFRESH_TTL_SECS: i64 = 30 * 24 * 60 * 60;
const LOGIN_FAILURE_WINDOW_SECS: i64 = 10 * 60;
const LOGIN_FAILURE_LOCK_SECS: i64 = 10 * 60;
const LOGIN_FAILURE_LIMIT: usize = 8;
const LOCAL_ADMIN_EMAIL: &str = "local-admin@cc-switch.local";

static LOGIN_THROTTLE: OnceLock<Mutex<LoginThrottle>> = OnceLock::new();

#[derive(Debug)]
pub struct WebAuthStore {
    config_dir: PathBuf,
    sessions: Mutex<Vec<StoredSession>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethods {
    pub router_available: bool,
    pub password_configured: bool,
    pub setup_token_required: bool,
    pub initial_client_setup_required: bool,
    pub owner_email: Option<String>,
    pub methods: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordLoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: String,
    pub refresh_expires_at: String,
}

#[derive(Debug, Clone)]
pub struct WebPrincipal {
    pub user_email: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    id: String,
    access_token_hash: String,
    refresh_token_hash: String,
    access_expires_at: String,
    refresh_expires_at: String,
    created_at: String,
    last_used_at: String,
    revoked_at: Option<String>,
}

#[derive(Debug, Default)]
struct LoginThrottle {
    failures: Vec<DateTime<Utc>>,
    locked_until: Option<DateTime<Utc>>,
}

#[derive(Debug, thiserror::Error)]
pub enum WebAuthError {
    #[error("{0}")]
    Message(String),
}

impl WebAuthError {
    fn message(value: impl Into<String>) -> Self {
        Self::Message(value.into())
    }
}

impl WebAuthStore {
    pub fn load(config_dir: PathBuf) -> Self {
        let sessions = read_sessions_unlocked(&config_dir).unwrap_or_default();
        Self {
            config_dir,
            sessions: Mutex::new(sessions),
        }
    }

    pub fn setup_password(
        &self,
        config: &mut ServerConfig,
        password: &str,
    ) -> Result<PasswordLoginResponse, WebAuthError> {
        if is_password_configured(config) {
            return Err(WebAuthError::message("web password is already configured"));
        }
        validate_password(password)?;
        config
            .set_password(password)
            .map_err(|error| WebAuthError::message(error.to_string()))?;
        self.create_session()
    }

    pub fn login(
        &self,
        config: &ServerConfig,
        password: &str,
    ) -> Result<PasswordLoginResponse, WebAuthError> {
        if !is_password_configured(config) {
            return Err(WebAuthError::message("web password is not configured"));
        }
        check_password_login_allowed()?;
        if !config.verify_password(password) {
            record_password_login_failure();
            return Err(WebAuthError::message("invalid password"));
        }
        clear_password_login_failures();
        self.create_session()
    }

    pub fn refresh(&self, refresh_token: &str) -> Result<PasswordLoginResponse, WebAuthError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| WebAuthError::message("web auth session lock poisoned"))?;
        let now = Utc::now();
        let refresh_hash = hash_token(refresh_token.trim());
        let Some(session) = sessions.iter_mut().find(|session| {
            session.revoked_at.is_none() && session.refresh_token_hash == refresh_hash
        }) else {
            return Err(WebAuthError::message("refresh session not found"));
        };
        if parse_time(&session.refresh_expires_at)? < now {
            return Err(WebAuthError::message("refresh session expired"));
        }
        let access_token = generate_secret(48);
        let refresh_token = generate_secret(64);
        let access_expires_at = now + Duration::seconds(ACCESS_TTL_SECS);
        let refresh_expires_at = now + Duration::seconds(REFRESH_TTL_SECS);
        session.access_token_hash = hash_token(&access_token);
        session.refresh_token_hash = hash_token(&refresh_token);
        session.access_expires_at = access_expires_at.to_rfc3339();
        session.refresh_expires_at = refresh_expires_at.to_rfc3339();
        session.last_used_at = now.to_rfc3339();
        self.persist(&sessions)?;
        Ok(PasswordLoginResponse {
            access_token,
            refresh_token,
            expires_at: access_expires_at.to_rfc3339(),
            refresh_expires_at: refresh_expires_at.to_rfc3339(),
        })
    }

    pub fn logout(&self, access_token: &str) -> Result<(), WebAuthError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| WebAuthError::message("web auth session lock poisoned"))?;
        let access_hash = hash_token(access_token.trim());
        let now = Utc::now().to_rfc3339();
        for session in sessions.iter_mut() {
            if session.access_token_hash == access_hash {
                session.revoked_at = Some(now.clone());
            }
        }
        self.persist(&sessions)
    }

    pub fn change_password(
        &self,
        config: &mut ServerConfig,
        current_password: &str,
        next_password: &str,
    ) -> Result<(), WebAuthError> {
        config
            .change_password(current_password, next_password)
            .map_err(|error| WebAuthError::message(error.to_string()))?;
        self.revoke_all_sessions()
    }

    pub fn authenticate_access_token(
        &self,
        access_token: &str,
    ) -> Result<Option<WebPrincipal>, WebAuthError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| WebAuthError::message("web auth session lock poisoned"))?;
        let access_hash = hash_token(access_token.trim());
        let now = Utc::now();
        let mut matched = false;
        for session in sessions.iter_mut() {
            if session.revoked_at.is_some() || session.access_token_hash != access_hash {
                continue;
            }
            if parse_time(&session.access_expires_at)? < now {
                continue;
            }
            session.last_used_at = now.to_rfc3339();
            matched = true;
            break;
        }
        if matched {
            self.persist(&sessions)?;
            return Ok(Some(WebPrincipal {
                user_email: LOCAL_ADMIN_EMAIL.to_string(),
                role: "admin".to_string(),
            }));
        }
        Ok(None)
    }

    pub fn revoke_all_sessions(&self) -> Result<(), WebAuthError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| WebAuthError::message("web auth session lock poisoned"))?;
        let now = Utc::now().to_rfc3339();
        for session in sessions.iter_mut() {
            session.revoked_at = Some(now.clone());
        }
        self.persist(&sessions)
    }

    fn create_session(&self) -> Result<PasswordLoginResponse, WebAuthError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| WebAuthError::message("web auth session lock poisoned"))?;
        let now = Utc::now();
        let access_token = generate_secret(48);
        let refresh_token = generate_secret(64);
        let access_expires_at = now + Duration::seconds(ACCESS_TTL_SECS);
        let refresh_expires_at = now + Duration::seconds(REFRESH_TTL_SECS);
        sessions.push(StoredSession {
            id: generate_secret(16),
            access_token_hash: hash_token(&access_token),
            refresh_token_hash: hash_token(&refresh_token),
            access_expires_at: access_expires_at.to_rfc3339(),
            refresh_expires_at: refresh_expires_at.to_rfc3339(),
            created_at: now.to_rfc3339(),
            last_used_at: now.to_rfc3339(),
            revoked_at: None,
        });
        prune_sessions(&mut sessions)?;
        self.persist(&sessions)?;
        Ok(PasswordLoginResponse {
            access_token,
            refresh_token,
            expires_at: access_expires_at.to_rfc3339(),
            refresh_expires_at: refresh_expires_at.to_rfc3339(),
        })
    }

    fn persist(&self, sessions: &[StoredSession]) -> Result<(), WebAuthError> {
        write_sessions_unlocked(&self.config_dir, sessions)
    }
}

pub fn auth_methods(config: &ServerConfig) -> AuthMethods {
    let router_available = router_available(config);
    let password_configured = is_password_configured(config);
    let mut methods = Vec::new();
    if router_available {
        methods.push("email");
        methods.push("apiToken");
    }
    if password_configured {
        methods.push("password");
    } else {
        methods.push("passwordSetup");
    }
    AuthMethods {
        router_available,
        password_configured,
        setup_token_required: false,
        initial_client_setup_required: false,
        owner_email: config
            .owner
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        methods,
    }
}

pub fn router_available(config: &ServerConfig) -> bool {
    config
        .router
        .url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && config
            .client
            .tunnel_subdomain
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

pub fn is_password_configured(config: &ServerConfig) -> bool {
    config
        .auth
        .password_hash
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn validate_password(password: &str) -> Result<(), WebAuthError> {
    if password.chars().count() < 8 {
        return Err(WebAuthError::message(
            "web password must be at least 8 characters",
        ));
    }
    Ok(())
}

fn check_password_login_allowed() -> Result<(), WebAuthError> {
    let now = Utc::now();
    let throttle = LOGIN_THROTTLE.get_or_init(|| Mutex::new(LoginThrottle::default()));
    let mut guard = throttle
        .lock()
        .map_err(|_| WebAuthError::message("web password throttle lock poisoned"))?;
    if let Some(locked_until) = guard.locked_until {
        if locked_until > now {
            return Err(WebAuthError::message("too many password attempts"));
        }
        guard.locked_until = None;
    }
    guard
        .failures
        .retain(|time| *time + Duration::seconds(LOGIN_FAILURE_WINDOW_SECS) >= now);
    Ok(())
}

fn record_password_login_failure() {
    let now = Utc::now();
    let throttle = LOGIN_THROTTLE.get_or_init(|| Mutex::new(LoginThrottle::default()));
    let Ok(mut guard) = throttle.lock() else {
        return;
    };
    guard
        .failures
        .retain(|time| *time + Duration::seconds(LOGIN_FAILURE_WINDOW_SECS) >= now);
    guard.failures.push(now);
    if guard.failures.len() >= LOGIN_FAILURE_LIMIT {
        guard.locked_until = Some(now + Duration::seconds(LOGIN_FAILURE_LOCK_SECS));
        guard.failures.clear();
    }
}

fn clear_password_login_failures() {
    let throttle = LOGIN_THROTTLE.get_or_init(|| Mutex::new(LoginThrottle::default()));
    if let Ok(mut guard) = throttle.lock() {
        guard.failures.clear();
        guard.locked_until = None;
    }
}

fn read_sessions_unlocked(config_dir: &Path) -> Result<Vec<StoredSession>, WebAuthError> {
    let path = sessions_path(config_dir);
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).map_err(|error| {
        WebAuthError::message(format!("read web auth sessions failed: {error}"))
    })?;
    serde_json::from_str(&raw)
        .map_err(|error| WebAuthError::message(format!("parse web auth sessions failed: {error}")))
}

fn write_sessions_unlocked(
    config_dir: &Path,
    sessions: &[StoredSession],
) -> Result<(), WebAuthError> {
    let path = sessions_path(config_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            WebAuthError::message(format!("create web auth sessions dir failed: {error}"))
        })?;
    }
    let raw = serde_json::to_string(sessions).map_err(|error| {
        WebAuthError::message(format!("serialize web auth sessions failed: {error}"))
    })?;
    fs::write(&path, raw)
        .map_err(|error| WebAuthError::message(format!("write web auth sessions failed: {error}")))
}

fn sessions_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SESSIONS_FILE_NAME)
}

fn prune_sessions(sessions: &mut Vec<StoredSession>) -> Result<(), WebAuthError> {
    let now = Utc::now();
    sessions.retain(|session| {
        session.revoked_at.is_none()
            && parse_time(&session.refresh_expires_at)
                .map(|expires| expires >= now)
                .unwrap_or(false)
    });
    Ok(())
}

fn parse_time(value: &str) -> Result<DateTime<Utc>, WebAuthError> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|error| WebAuthError::message(format!("parse web auth timestamp failed: {error}")))
}

fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn generate_secret(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::settings::config::{
        AuthConfig, ClientConfig, OwnerConfig, RouterConfig, ServerConfig,
    };

    fn test_config(password: Option<&str>) -> ServerConfig {
        let mut config = ServerConfig {
            auth: AuthConfig::default(),
            owner: OwnerConfig {
                email: Some("owner@example.com".to_string()),
                ..OwnerConfig::default()
            },
            router: RouterConfig {
                url: Some("https://router.example.com".to_string()),
                domain: Some("router.example.com".to_string()),
                ..RouterConfig::default()
            },
            client: ClientConfig {
                tunnel_subdomain: Some("owner".to_string()),
                ..ClientConfig::default()
            },
            upstream_proxy: Default::default(),
        };
        if let Some(password) = password {
            config.set_password(password).expect("hash password");
        }
        config
    }

    #[test]
    fn auth_methods_include_router_and_password_when_configured() {
        let methods = auth_methods(&test_config(Some("password123")));
        assert!(methods.router_available);
        assert!(methods.password_configured);
        assert!(methods.methods.contains(&"email"));
        assert!(methods.methods.contains(&"apiToken"));
        assert!(methods.methods.contains(&"password"));
        assert_eq!(methods.owner_email.as_deref(), Some("owner@example.com"));
    }

    #[test]
    fn password_login_and_refresh_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "cc-switch-server-web-auth-test-{}",
            generate_secret(8)
        ));
        fs::create_dir_all(&dir).expect("tempdir");
        let store = WebAuthStore::load(dir.clone());
        let mut config = test_config(None);
        let created = store
            .setup_password(&mut config, "password123")
            .expect("setup");
        let refreshed = store.refresh(&created.refresh_token).expect("refresh");
        assert_ne!(created.access_token, refreshed.access_token);
        assert!(store
            .authenticate_access_token(&refreshed.access_token)
            .expect("auth")
            .is_some());
        let _ = fs::remove_dir_all(dir);
    }
}
