use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::providers::model::ProviderType;

const ACCESS_TOKEN_KEYS: &[&str] = &["cursorAuth/accessToken", "cursorAuth/token"];
const MACHINE_ID_KEYS: &[&str] = &[
    "storage.serviceMachineId",
    "storage.machineId",
    "telemetry.machineId",
];
const MIN_CURSOR_TOKEN_LEN: usize = 50;
const CURSOR_IDE_VERSION_DB_KEY: &str = "cursorupdate.lastUpdatedAndShown.version";
const CURSOR_VERSION_CACHE_TTL: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorImportSource {
    IdeStateVscdb,
    CursorAgentAuthJson,
}

impl CursorImportSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::IdeStateVscdb => "ide_state_vscdb",
            Self::CursorAgentAuthJson => "cursor_agent_auth_json",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorLocalImport {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    pub source: CursorImportSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workos_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImportError {
    pub message: String,
}

impl CursorImportError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for CursorImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CursorImportError {}

#[derive(Debug, Clone, Default)]
struct ExtractedCursorTokens {
    access_token: Option<String>,
    machine_id: Option<String>,
}

#[derive(Debug, Clone)]
struct VscDbRow {
    key: String,
    value: String,
}

#[derive(Debug, Clone)]
struct CursorVersionCache {
    version: Option<String>,
    cached_at: SystemTime,
}

pub fn import_from_local_cursor() -> Result<CursorLocalImport, CursorImportError> {
    let mut errors = Vec::new();
    match try_ide_auth() {
        Ok(import) => return Ok(import),
        Err(error) => errors.push(error.message),
    }
    match try_agent_auth() {
        Ok(import) => return Ok(import),
        Err(error) => errors.push(error.message),
    }
    Err(CursorImportError::new(format!(
        "Cursor local import failed: {}",
        errors.join("; ")
    )))
}

pub fn upsert_input_from_cursor_local_import(
    import: CursorLocalImport,
    profile_raw: Option<Value>,
    now_ms: i64,
) -> UpsertAccountInput {
    let profile_email = profile_raw.as_ref().and_then(email_from_profile_value);
    let email = profile_email.or(import.email.clone());
    let account_id =
        stable_cursor_import_account_id(import.workos_user_id.as_deref(), &import.access_token);
    let mut profile = json!({
        "providerType": ProviderType::CursorOAuth.as_str(),
        "source": "cursor_local_import",
        "importSource": import.source.as_str(),
        "accountId": account_id,
        "email": email,
        "workosUserId": import.workos_user_id,
        "cursorServiceMachineId": import.machine_id,
        "path": import.path.as_ref().map(|path| path.display().to_string()),
    });
    if let Some(profile_raw) = profile_raw {
        profile["profileRaw"] = profile_raw;
    }
    UpsertAccountInput {
        id: Some(account_id),
        provider_type: ProviderType::CursorOAuth,
        email,
        access_token: Some(import.access_token.clone()),
        refresh_token: None,
        id_token: None,
        token_type: Some("Bearer".to_string()),
        api_key: None,
        scopes: Vec::new(),
        profile: Some(profile),
        raw: Some(json!({
            "source": "cursor_local_import",
            "importSource": import.source.as_str(),
            "importedAtMs": now_ms,
            "path": import.path.as_ref().map(|path| path.display().to_string()),
            "cursorServiceMachineId": import.machine_id,
            "workosUserId": import.workos_user_id,
            "hasRefreshToken": false,
        })),
        subscription_level: None,
        entitlement_status: None,
        quota_percent: None,
        quota: None,
        quota_refreshed_at: None,
        quota_next_refresh_at: None,
        expires_at: import.expires_at,
        rate_limited_until: None,
        last_refresh_error: None,
    }
}

pub fn cursor_workos_user_id_from_access_token(access_token: &str) -> Option<String> {
    let claims = decode_jwt_claims(access_token)?;
    string_at(&claims, &["/sub", "/user_id"])
}

pub fn cursor_account_id_from_stable_subject(subject: &str) -> Option<String> {
    let subject = subject.trim();
    if subject.is_empty() {
        return None;
    }
    Some(cursor_account_id_from_seed(subject))
}

pub fn detect_cursor_ide_version() -> Option<String> {
    {
        let cache = cursor_version_cache().lock().ok()?;
        if let Some(cache) = cache.as_ref() {
            if cache.cached_at.elapsed().ok()? < CURSOR_VERSION_CACHE_TTL {
                return cache.version.clone();
            }
        }
    }
    let version = detect_cursor_ide_version_uncached();
    if let Ok(mut cache) = cursor_version_cache().lock() {
        *cache = Some(CursorVersionCache {
            version: version.clone(),
            cached_at: SystemTime::now(),
        });
    }
    version
}

pub fn candidate_paths_for(
    platform: &str,
    home: &Path,
    appdata: Option<&Path>,
    local_appdata: Option<&Path>,
) -> Vec<PathBuf> {
    match platform {
        "macos" | "darwin" => vec![
            home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
            home.join(
                "Library/Application Support/Cursor - Insiders/User/globalStorage/state.vscdb",
            ),
        ],
        "linux" => vec![
            home.join(".config/Cursor/User/globalStorage/state.vscdb"),
            home.join(".config/cursor/User/globalStorage/state.vscdb"),
        ],
        "windows" | "win32" => {
            let mut paths = Vec::new();
            if let Some(appdata) = appdata {
                paths.push(appdata.join("Cursor/User/globalStorage/state.vscdb"));
                paths.push(appdata.join("Cursor - Insiders/User/globalStorage/state.vscdb"));
            }
            if let Some(local_appdata) = local_appdata {
                paths.push(local_appdata.join("Cursor/User/globalStorage/state.vscdb"));
                paths.push(local_appdata.join("Programs/Cursor/User/globalStorage/state.vscdb"));
            }
            paths
        }
        _ => Vec::new(),
    }
}

pub fn candidate_agent_auth_paths_for(
    platform: &str,
    home: &Path,
    appdata: Option<&Path>,
    local_appdata: Option<&Path>,
) -> Vec<PathBuf> {
    match platform {
        "macos" | "darwin" => vec![
            home.join(".config/cursor/auth.json"),
            home.join("Library/Application Support/cursor/auth.json"),
            home.join("Library/Application Support/Cursor/auth.json"),
        ],
        "linux" => vec![home.join(".config/cursor/auth.json")],
        "windows" | "win32" => {
            let mut paths = Vec::new();
            if let Some(appdata) = appdata {
                paths.push(appdata.join("cursor/auth.json"));
                paths.push(appdata.join("Cursor/auth.json"));
            }
            if let Some(local_appdata) = local_appdata {
                paths.push(local_appdata.join("cursor/auth.json"));
                paths.push(local_appdata.join("Cursor/auth.json"));
            }
            paths
        }
        _ => Vec::new(),
    }
}

fn cursor_agent_auth_path_override() -> Option<PathBuf> {
    env::var_os("CURSOR_AGENT_AUTH_PATH")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn try_ide_auth() -> Result<CursorLocalImport, CursorImportError> {
    let state_db_override = cursor_state_db_path_override();
    if cfg!(target_os = "linux") && state_db_override.is_none() && !linux_cursor_install_present() {
        return Err(CursorImportError::new(
            "Cursor IDE does not appear to be installed on this Linux host",
        ));
    }
    let home = home_dir()?;
    let appdata = env::var_os("APPDATA").map(PathBuf::from);
    let local_appdata = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let candidates = state_db_override.map(|path| vec![path]).unwrap_or_else(|| {
        candidate_paths_for(
            env::consts::OS,
            &home,
            appdata.as_deref(),
            local_appdata.as_deref(),
        )
    });
    if candidates.is_empty() {
        return Err(CursorImportError::new(format!(
            "Cursor IDE import is unsupported on {}",
            env::consts::OS
        )));
    }

    let mut missing = Vec::new();
    let mut read_errors = Vec::new();
    for path in candidates {
        if !path.exists() {
            missing.push(path.display().to_string());
            continue;
        }
        match read_ide_tokens_from_vscdb(&path).and_then(tokens_to_ide_import) {
            Ok(mut import) => {
                import.path = Some(path);
                return Ok(import);
            }
            Err(error) => read_errors.push(format!("{}: {}", path.display(), error.message)),
        }
    }
    if !read_errors.is_empty() {
        return Err(CursorImportError::new(read_errors.join("; ")));
    }
    Err(CursorImportError::new(format!(
        "Cursor IDE state.vscdb not found; checked {}",
        missing.join(", ")
    )))
}

fn try_agent_auth() -> Result<CursorLocalImport, CursorImportError> {
    let home = home_dir()?;
    let appdata = env::var_os("APPDATA").map(PathBuf::from);
    let local_appdata = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let paths = cursor_agent_auth_path_override()
        .map(|path| vec![path])
        .unwrap_or_else(|| {
            candidate_agent_auth_paths_for(
                env::consts::OS,
                &home,
                appdata.as_deref(),
                local_appdata.as_deref(),
            )
        });
    if paths.is_empty() {
        return Err(CursorImportError::new(format!(
            "cursor-agent auth.json import is unsupported on {}",
            env::consts::OS
        )));
    }
    let mut read_errors = Vec::new();
    let mut loaded = None;
    for path in paths {
        match fs::read_to_string(&path) {
            Ok(raw) => {
                loaded = Some((path, raw));
                break;
            }
            Err(error) => read_errors.push(format!("{}: {error}", path.display())),
        }
    }
    let Some((path, raw)) = loaded else {
        return Err(CursorImportError::new(format!(
            "cursor-agent auth.json not found or unreadable; checked {}",
            read_errors.join("; ")
        )));
    };
    let value: Value = serde_json::from_str(&raw).map_err(|error| {
        CursorImportError::new(format!(
            "cursor-agent auth.json is not valid JSON at {}: {error}",
            path.display()
        ))
    })?;
    let access_token = string_at(
        &value,
        &[
            "/accessToken",
            "/access_token",
            "/token",
            "/key",
            "/auth/accessToken",
            "/auth/access_token",
        ],
    )
    .ok_or_else(|| CursorImportError::new("cursor-agent auth.json has no accessToken"))?;
    validate_access_token(&access_token)?;
    let (workos_user_id, email, expires_at) = access_token_identity(&access_token);
    Ok(CursorLocalImport {
        access_token,
        machine_id: None,
        source: CursorImportSource::CursorAgentAuthJson,
        path: Some(path),
        workos_user_id,
        email,
        expires_at,
    })
}

fn read_ide_tokens_from_vscdb(path: &Path) -> Result<ExtractedCursorTokens, CursorImportError> {
    let conn = open_immutable_vscdb(path)?;
    let exact_rows = query_rows(
        &conn,
        "SELECT key, value FROM itemTable WHERE key IN (?1, ?2, ?3, ?4, ?5)",
        [
            ACCESS_TOKEN_KEYS[0],
            ACCESS_TOKEN_KEYS[1],
            MACHINE_ID_KEYS[0],
            MACHINE_ID_KEYS[1],
            MACHINE_ID_KEYS[2],
        ],
    )?;
    let mut tokens = extract_cursor_tokens_from_rows(&exact_rows);
    if tokens.access_token.is_none() || tokens.machine_id.is_none() {
        let fuzzy_rows = query_fuzzy_rows(&conn)?;
        tokens = fuzzy_extract_cursor_tokens_from_rows(&fuzzy_rows, tokens);
    }
    Ok(tokens)
}

fn query_rows(
    conn: &Connection,
    sql: &str,
    params: [&str; 5],
) -> Result<Vec<VscDbRow>, CursorImportError> {
    let mut stmt = conn.prepare(sql).map_err(|error| {
        CursorImportError::new(format!("prepare Cursor state query failed: {error}"))
    })?;
    let rows = stmt
        .query_map(params, |row| {
            Ok(VscDbRow {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })
        .map_err(|error| CursorImportError::new(format!("query Cursor state failed: {error}")))?;
    collect_rows(rows)
}

fn query_fuzzy_rows(conn: &Connection) -> Result<Vec<VscDbRow>, CursorImportError> {
    let mut stmt = conn
        .prepare(
            "SELECT key, value FROM itemTable \
             WHERE lower(key) LIKE '%accesstoken%' OR lower(key) LIKE '%machineid%'",
        )
        .map_err(|error| {
            CursorImportError::new(format!("prepare Cursor fuzzy query failed: {error}"))
        })?;
    let rows = stmt
        .query_map([], |row| {
            Ok(VscDbRow {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })
        .map_err(|error| {
            CursorImportError::new(format!("query Cursor fuzzy state failed: {error}"))
        })?;
    collect_rows(rows)
}

fn collect_rows(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<VscDbRow>>,
) -> Result<Vec<VscDbRow>, CursorImportError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|error| {
            CursorImportError::new(format!("read Cursor state row failed: {error}"))
        })?);
    }
    Ok(out)
}

fn extract_cursor_tokens_from_rows(rows: &[VscDbRow]) -> ExtractedCursorTokens {
    let mut tokens = ExtractedCursorTokens::default();
    for row in rows {
        if tokens.access_token.is_none() && ACCESS_TOKEN_KEYS.contains(&row.key.as_str()) {
            tokens.access_token = normalize_vscdb_value(&row.value);
        } else if tokens.machine_id.is_none() && MACHINE_ID_KEYS.contains(&row.key.as_str()) {
            tokens.machine_id = normalize_vscdb_value(&row.value);
        }
    }
    tokens
}

fn fuzzy_extract_cursor_tokens_from_rows(
    rows: &[VscDbRow],
    mut tokens: ExtractedCursorTokens,
) -> ExtractedCursorTokens {
    for row in rows {
        let key = row.key.to_ascii_lowercase();
        let value = normalize_vscdb_value(&row.value);
        if tokens.access_token.is_none() && key.contains("accesstoken") {
            tokens.access_token = value.clone();
        }
        if tokens.machine_id.is_none() && key.contains("machineid") {
            tokens.machine_id = value;
        }
    }
    tokens
}

fn tokens_to_ide_import(
    tokens: ExtractedCursorTokens,
) -> Result<CursorLocalImport, CursorImportError> {
    let access_token = tokens
        .access_token
        .ok_or_else(|| CursorImportError::new("Cursor state.vscdb has no access token"))?;
    validate_access_token(&access_token)?;
    let (workos_user_id, email, expires_at) = access_token_identity(&access_token);
    Ok(CursorLocalImport {
        access_token,
        machine_id: tokens.machine_id.filter(|value| !value.trim().is_empty()),
        source: CursorImportSource::IdeStateVscdb,
        path: None,
        workos_user_id,
        email,
        expires_at,
    })
}

fn normalize_vscdb_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(Value::String(inner)) = serde_json::from_str::<Value>(trimmed) {
        let inner = inner.trim();
        if !inner.is_empty() {
            return Some(inner.to_string());
        }
    }
    Some(trimmed.to_string())
}

fn validate_access_token(access_token: &str) -> Result<(), CursorImportError> {
    if access_token.trim().len() < MIN_CURSOR_TOKEN_LEN {
        return Err(CursorImportError::new(
            "Cursor access token is missing or appears too short",
        ));
    }
    Ok(())
}

fn access_token_identity(access_token: &str) -> (Option<String>, Option<String>, Option<i64>) {
    let Some(claims) = decode_jwt_claims(access_token) else {
        return (None, None, None);
    };
    (
        string_at(&claims, &["/sub", "/user_id"]),
        string_at(&claims, &["/email", "/preferred_username"]),
        integer_at(&claims, &["/exp"]).map(|seconds| seconds.saturating_mul(1000)),
    )
}

fn email_from_profile_value(value: &Value) -> Option<String> {
    string_at(
        value,
        &[
            "/email",
            "/email_address",
            "/user/email",
            "/profile/email",
            "/account/email",
            "/account/email_address",
        ],
    )
}

fn stable_cursor_import_account_id(workos_user_id: Option<&str>, access_token: &str) -> String {
    let seed = workos_user_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(access_token);
    cursor_account_id_from_seed(seed)
}

fn cursor_account_id_from_seed(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("cursor_{}", &hex[..24])
}

fn decode_jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&decoded).ok()
}

fn string_at(value: &Value, pointers: &[&str]) -> Option<String> {
    pointers.iter().find_map(|pointer| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn integer_at(value: &Value, pointers: &[&str]) -> Option<i64> {
    pointers.iter().find_map(|pointer| {
        value.pointer(pointer).and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
        })
    })
}

fn home_dir() -> Result<PathBuf, CursorImportError> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| CursorImportError::new("HOME/USERPROFILE is not set"))
}

fn linux_cursor_install_present() -> bool {
    if which_cursor_present() {
        return true;
    }
    home_dir()
        .ok()
        .map(|home| {
            home.join(".local/share/applications/cursor.desktop")
                .exists()
        })
        .unwrap_or(false)
}

fn which_cursor_present() -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&path).any(|dir| {
        let candidate = dir.join("cursor");
        candidate.exists() && candidate.is_file()
    })
}

fn detect_cursor_ide_version_uncached() -> Option<String> {
    let path = cursor_state_db_path_override().or_else(|| {
        let home = home_dir().ok()?;
        let appdata = env::var_os("APPDATA").map(PathBuf::from);
        let local_appdata = env::var_os("LOCALAPPDATA").map(PathBuf::from);
        candidate_paths_for(
            env::consts::OS,
            &home,
            appdata.as_deref(),
            local_appdata.as_deref(),
        )
        .into_iter()
        .find(|path| path.exists())
    })?;
    read_cursor_version_from_vscdb(&path).ok().flatten()
}

fn cursor_state_db_path_override() -> Option<PathBuf> {
    env::var_os("CURSOR_STATE_DB_PATH")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
}

fn read_cursor_version_from_vscdb(path: &Path) -> Result<Option<String>, CursorImportError> {
    let conn = open_immutable_vscdb(path)?;
    let mut stmt = conn
        .prepare("SELECT value FROM itemTable WHERE key = ?1")
        .map_err(|error| {
            CursorImportError::new(format!("prepare Cursor version query failed: {error}"))
        })?;
    let mut rows = stmt
        .query([CURSOR_IDE_VERSION_DB_KEY])
        .map_err(|error| CursorImportError::new(format!("query Cursor version failed: {error}")))?;
    let Some(row) = rows.next().map_err(|error| {
        CursorImportError::new(format!("read Cursor version row failed: {error}"))
    })?
    else {
        return Ok(None);
    };
    let value: String = row.get(0).map_err(|error| {
        CursorImportError::new(format!("read Cursor version value failed: {error}"))
    })?;
    Ok(normalize_vscdb_value(&value))
}

fn open_immutable_vscdb(path: &Path) -> Result<Connection, CursorImportError> {
    let uri = immutable_sqlite_uri(path);
    Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| {
        let message = error.to_string();
        if message.to_ascii_lowercase().contains("busy")
            || message.to_ascii_lowercase().contains("locked")
        {
            CursorImportError::new(format!(
                "open Cursor state.vscdb failed: {error}; Cursor IDE may be running, close it and retry or use cursor-agent auth.json"
            ))
        } else {
            CursorImportError::new(format!("open Cursor state.vscdb failed: {error}"))
        }
    })
}

fn immutable_sqlite_uri(path: &Path) -> String {
    format!("file:{}?immutable=1", sqlite_uri_path(path))
}

fn sqlite_uri_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let mut out = String::with_capacity(raw.len());
    for byte in raw.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'_' | b'-' | b'~' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn cursor_version_cache() -> &'static Mutex<Option<CursorVersionCache>> {
    static CACHE: OnceLock<Mutex<Option<CursorVersionCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_json_encoded_string_values() {
        assert_eq!(
            normalize_vscdb_value("\"token-value\"").as_deref(),
            Some("token-value")
        );
        assert_eq!(normalize_vscdb_value("plain").as_deref(), Some("plain"));
    }

    #[test]
    fn extracts_exact_and_fuzzy_tokens() {
        let rows = vec![
            VscDbRow {
                key: "cursorAuth/accessToken".to_string(),
                value: "\"access-token\"".to_string(),
            },
            VscDbRow {
                key: "storage.serviceMachineId".to_string(),
                value: "machine".to_string(),
            },
        ];
        let tokens = extract_cursor_tokens_from_rows(&rows);
        assert_eq!(tokens.access_token.as_deref(), Some("access-token"));
        assert_eq!(tokens.machine_id.as_deref(), Some("machine"));

        let fuzzy = fuzzy_extract_cursor_tokens_from_rows(
            &[VscDbRow {
                key: "cursorAuth/newAccessTokenKey".to_string(),
                value: "fuzzy-token".to_string(),
            }],
            ExtractedCursorTokens::default(),
        );
        assert_eq!(fuzzy.access_token.as_deref(), Some("fuzzy-token"));
    }

    #[test]
    fn candidate_paths_include_macos_insiders() {
        let paths = candidate_paths_for("macos", Path::new("/Users/a"), None, None);
        assert_eq!(paths.len(), 2);
        assert!(paths[1].display().to_string().contains("Cursor - Insiders"));
    }

    #[test]
    fn agent_auth_paths_cover_desktop_platforms() {
        let macos = candidate_agent_auth_paths_for("macos", Path::new("/Users/a"), None, None);
        assert!(macos.iter().any(|path| path
            .display()
            .to_string()
            .contains("Application Support/cursor")));

        let windows = candidate_agent_auth_paths_for(
            "windows",
            Path::new("C:/Users/a"),
            Some(Path::new("C:/Users/a/AppData/Roaming")),
            Some(Path::new("C:/Users/a/AppData/Local")),
        );
        assert!(windows.iter().any(|path| path
            .display()
            .to_string()
            .contains("Roaming/cursor/auth.json")));
        assert!(windows.iter().any(|path| path
            .display()
            .to_string()
            .contains("Local/Cursor/auth.json")));
    }

    #[test]
    fn local_import_builds_cursor_oauth_upsert() {
        let import = CursorLocalImport {
            access_token: "x".repeat(64),
            machine_id: Some("machine".to_string()),
            source: CursorImportSource::IdeStateVscdb,
            path: Some(PathBuf::from("/tmp/state.vscdb")),
            workos_user_id: Some("user".to_string()),
            email: Some("a@example.com".to_string()),
            expires_at: Some(1_000),
        };
        let upsert = upsert_input_from_cursor_local_import(import, None, 2_000);
        assert_eq!(upsert.provider_type, ProviderType::CursorOAuth);
        assert!(upsert.id.unwrap().starts_with("cursor_"));
        assert_eq!(upsert.email.as_deref(), Some("a@example.com"));
        assert!(upsert.refresh_token.is_none());
    }

    #[test]
    fn stable_subject_account_id_matches_local_import_id() {
        let import = CursorLocalImport {
            access_token: "x".repeat(64),
            machine_id: None,
            source: CursorImportSource::CursorAgentAuthJson,
            path: None,
            workos_user_id: Some("workos-subject".to_string()),
            email: None,
            expires_at: None,
        };
        let upsert = upsert_input_from_cursor_local_import(import, None, 2_000);
        assert_eq!(
            upsert.id,
            cursor_account_id_from_stable_subject("workos-subject")
        );
    }

    #[test]
    fn immutable_sqlite_uri_escapes_path_for_uri_mode() {
        assert_eq!(
            immutable_sqlite_uri(Path::new("/tmp/Cursor State/state.vscdb")),
            "file:/tmp/Cursor%20State/state.vscdb?immutable=1"
        );
    }
}
