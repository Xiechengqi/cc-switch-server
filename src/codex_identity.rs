use std::env;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const DEFAULT_CODEX_ORIGINATOR: &str = "codex_cli_rs";
pub(crate) const DEFAULT_CODEX_VERSION: &str = "0.144.1";

const CODEX_VERSION_ENV: &str = "CC_SWITCH_CODEX_CLIENT_VERSION";
const CODEX_USER_AGENT_ENV: &str = "CC_SWITCH_CODEX_USER_AGENT";
const CODEX_VERSION_SYNC_DISABLED_ENV: &str = "CC_SWITCH_CODEX_CLI_VERSION_SYNC_DISABLED";
const CODEX_VERSION_SYNC_INTERVAL_HOURS_ENV: &str =
    "CC_SWITCH_CODEX_CLI_VERSION_SYNC_INTERVAL_HOURS";
const CODEX_RELEASES_LATEST_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CODEX_VERSION_CACHE_FILE: &str = "codex-cli-version-cache.json";
const CODEX_VERSION_CACHE_SCHEMA: u32 = 1;
const CODEX_VERSION_SYNC_BODY_LIMIT_BYTES: usize = 1024 * 1024;
const CODEX_VERSION_SYNC_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_CODEX_VERSION_SYNC_INTERVAL_HOURS: u64 = 12;
const MAX_CODEX_VERSION_SYNC_INTERVAL_HOURS: u64 = 720;
const MAX_ORIGINATOR_LEN: usize = 64;

static SYNCED_CODEX_VERSION: OnceLock<RwLock<Option<String>>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexVersionCache {
    schema_version: u32,
    version: String,
    synced_at_ms: i64,
    source: String,
}

const OFFICIAL_ORIGINATORS: &[&str] = &[
    "codex_cli_rs",
    "codex-tui",
    "codex_vscode",
    "codex_vscode_copilot",
    "codex_app",
    "codex_chatgpt_desktop",
    "codex_atlas",
    "codex_exec",
    "codex_sdk_ts",
];

pub(crate) fn configured_version() -> String {
    let explicit = env::var(CODEX_VERSION_ENV)
        .ok()
        .map(|value| value.trim().to_string());
    resolve_configured_version(explicit.as_deref(), synced_version().as_deref())
}

fn resolve_configured_version(explicit: Option<&str>, synced: Option<&str>) -> String {
    explicit
        .map(str::trim)
        .filter(|value| version_at_least(value, DEFAULT_CODEX_VERSION))
        .map(str::to_string)
        .or_else(|| {
            synced
                .map(str::trim)
                .filter(|value| stable_version_at_least(value, DEFAULT_CODEX_VERSION))
                .map(str::to_string)
        })
        .unwrap_or_else(|| DEFAULT_CODEX_VERSION.to_string())
}

fn synced_version() -> Option<String> {
    SYNCED_CODEX_VERSION
        .get_or_init(|| RwLock::new(None))
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn install_synced_version(version: Option<String>) {
    *SYNCED_CODEX_VERSION
        .get_or_init(|| RwLock::new(None))
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = version;
}

pub(crate) fn initialize_synced_version_cache(config_dir: &Path) -> anyhow::Result<()> {
    match read_version_cache(&version_cache_path(config_dir)) {
        Ok(cache) => {
            install_synced_version(cache.map(|cache| cache.version));
            Ok(())
        }
        Err(error) => {
            install_synced_version(None);
            Err(error)
        }
    }
}

pub(crate) fn version_sync_disabled() -> bool {
    env::var(CODEX_VERSION_SYNC_DISABLED_ENV)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

pub(crate) fn version_sync_interval() -> Duration {
    let hours = env::var(CODEX_VERSION_SYNC_INTERVAL_HOURS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CODEX_VERSION_SYNC_INTERVAL_HOURS)
        .min(MAX_CODEX_VERSION_SYNC_INTERVAL_HOURS);
    Duration::from_secs(hours.saturating_mul(60 * 60))
}

pub(crate) async fn sync_latest_version(
    http: &reqwest::Client,
    config_dir: &Path,
) -> anyhow::Result<String> {
    sync_latest_version_from_url(http, config_dir, CODEX_RELEASES_LATEST_URL).await
}

async fn sync_latest_version_from_url(
    http: &reqwest::Client,
    config_dir: &Path,
    url: &str,
) -> anyhow::Result<String> {
    let mut response = http
        .get(url)
        .header("accept", "application/vnd.github+json")
        .header("user-agent", "cc-switch-server")
        .timeout(CODEX_VERSION_SYNC_TIMEOUT)
        .send()
        .await
        .context("request latest OpenAI Codex release")?;
    let status = response.status();
    let body = crate::infra::http::read_response_body_limited(
        &mut response,
        CODEX_VERSION_SYNC_BODY_LIMIT_BYTES,
    )
    .await
    .context("read latest OpenAI Codex release")?;
    if !status.is_success() {
        anyhow::bail!(
            "latest OpenAI Codex release returned HTTP {}",
            status.as_u16()
        );
    }
    let fetched = parse_latest_release_version(&body)?;
    let current = read_version_cache(&version_cache_path(config_dir))
        .ok()
        .flatten()
        .map(|cache| cache.version)
        .filter(|version| stable_version_at_least(version, DEFAULT_CODEX_VERSION));
    let effective = [
        Some(DEFAULT_CODEX_VERSION.to_string()),
        current,
        Some(fetched),
    ]
    .into_iter()
    .flatten()
    .max_by_key(|version| parse_stable_version(version).expect("validated stable version"))
    .expect("built-in Codex version is always present");
    let cache = CodexVersionCache {
        schema_version: CODEX_VERSION_CACHE_SCHEMA,
        version: effective.clone(),
        synced_at_ms: crate::infra::time::now_ms().min(i64::MAX as u128) as i64,
        source: "github_latest_release".to_string(),
    };
    let path = version_cache_path(config_dir);
    tokio::task::spawn_blocking(move || crate::infra::storage::write_json_pretty(&path, &cache))
        .await
        .context("join Codex version cache writer")??;
    install_synced_version(Some(effective.clone()));
    Ok(effective)
}

fn parse_latest_release_version(body: &[u8]) -> anyhow::Result<String> {
    let payload: Value = serde_json::from_slice(body).context("parse latest Codex release JSON")?;
    ["name", "tag_name"]
        .into_iter()
        .filter_map(|field| payload.get(field).and_then(Value::as_str))
        .find_map(extract_stable_release_version)
        .context("latest OpenAI Codex release did not contain a stable semantic version")
}

fn version_cache_path(config_dir: &Path) -> PathBuf {
    config_dir.join(CODEX_VERSION_CACHE_FILE)
}

fn read_version_cache(path: &Path) -> anyhow::Result<Option<CodexVersionCache>> {
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let cache: CodexVersionCache =
        serde_json::from_slice(&content).with_context(|| format!("parse {}", path.display()))?;
    if cache.schema_version != CODEX_VERSION_CACHE_SCHEMA
        || cache.source != "github_latest_release"
        || !stable_version_at_least(&cache.version, DEFAULT_CODEX_VERSION)
    {
        return Ok(None);
    }
    Ok(Some(cache))
}

fn extract_stable_release_version(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let candidate = raw
        .strip_prefix("rust-")
        .unwrap_or(raw)
        .strip_prefix('v')
        .unwrap_or(raw.strip_prefix("rust-").unwrap_or(raw));
    parse_stable_version(candidate).map(|_| candidate.to_string())
}

fn stable_version_at_least(candidate: &str, minimum: &str) -> bool {
    parse_stable_version(candidate)
        .zip(parse_stable_version(minimum))
        .is_some_and(|(candidate, minimum)| candidate >= minimum)
}

fn parse_stable_version(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim();
    if value.is_empty()
        || value.contains(['-', '+'])
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_digit() || byte == b'.'))
    {
        return None;
    }
    let mut parts = value.split('.');
    let version = (
        parse_stable_version_component(parts.next()?)?,
        parse_stable_version_component(parts.next()?)?,
        parse_stable_version_component(parts.next()?)?,
    );
    parts.next().is_none().then_some(version)
}

fn parse_stable_version_component(value: &str) -> Option<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    value.parse().ok()
}

pub(crate) fn default_user_agent() -> String {
    canonical_auth_identity().1
}

pub(crate) fn canonical_auth_identity() -> (String, String) {
    let version = configured_version();
    let fallback_ua = default_user_agent_for_version(&version);
    let candidate = env::var(CODEX_USER_AGENT_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let (originator, user_agent) = candidate
        .as_deref()
        .and_then(pair_user_agent)
        .unwrap_or_else(|| (DEFAULT_CODEX_ORIGINATOR.to_string(), fallback_ua));
    let user_agent = set_user_agent_version(&user_agent, &version).unwrap_or(user_agent);
    (originator, user_agent)
}

fn set_user_agent_version(user_agent: &str, version: &str) -> Option<String> {
    let slash = user_agent.find('/')?;
    let rest = &user_agent[slash + 1..];
    let version_end = rest
        .find(|ch: char| ch.is_ascii_whitespace() || ch == '(')
        .unwrap_or(rest.len());
    Some(format!(
        "{}{}{}",
        &user_agent[..slash + 1],
        version,
        &rest[version_end..]
    ))
}

pub(crate) fn finalize_headers(headers: &mut Vec<(&'static str, String)>) {
    let configured_version = configured_version();
    let candidate = header_value(headers, "user-agent")
        .map(str::to_string)
        .unwrap_or_else(default_user_agent);
    let (originator, user_agent) = pair_user_agent(&candidate).unwrap_or_else(|| {
        (
            DEFAULT_CODEX_ORIGINATOR.to_string(),
            default_user_agent_for_version(&configured_version),
        )
    });
    let version = header_value(headers, "version")
        .filter(|value| version_at_least(value, DEFAULT_CODEX_VERSION))
        .map(str::to_string)
        .unwrap_or(configured_version);
    let user_agent = set_user_agent_version(&user_agent, &version).unwrap_or(user_agent);

    replace_or_push(headers, "user-agent", user_agent);
    replace_or_push(headers, "originator", originator);
    replace_or_push(headers, "version", version);
}

pub(crate) fn finalize_owned_headers(headers: &mut Vec<(String, String)>) {
    let configured_version = configured_version();
    let candidate = owned_header_value(headers, "user-agent")
        .map(str::to_string)
        .unwrap_or_else(default_user_agent);
    let (originator, user_agent) = pair_user_agent(&candidate).unwrap_or_else(|| {
        (
            DEFAULT_CODEX_ORIGINATOR.to_string(),
            default_user_agent_for_version(&configured_version),
        )
    });
    let version = owned_header_value(headers, "version")
        .filter(|value| version_at_least(value, DEFAULT_CODEX_VERSION))
        .map(str::to_string)
        .unwrap_or(configured_version);
    let user_agent = set_user_agent_version(&user_agent, &version).unwrap_or(user_agent);

    replace_or_push_owned(headers, "user-agent", user_agent);
    replace_or_push_owned(headers, "originator", originator);
    replace_or_push_owned(headers, "version", version);
}

fn default_user_agent_for_version(version: &str) -> String {
    format!("codex_cli_rs/{version} (Ubuntu 22.04.0; x86_64) xterm-256color")
}

fn header_value<'a>(headers: &'a [(&'static str, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn replace_or_push(headers: &mut Vec<(&'static str, String)>, name: &'static str, value: String) {
    headers.retain(|(candidate, _)| !candidate.eq_ignore_ascii_case(name));
    headers.push((name, value));
}

fn owned_header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
}

fn replace_or_push_owned(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    headers.retain(|(candidate, _)| !candidate.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value));
}

fn pair_user_agent(user_agent: &str) -> Option<(String, String)> {
    let user_agent = user_agent.trim();
    let slash = user_agent.find('/')?;
    if slash == 0 {
        return None;
    }
    let leading = user_agent[..slash].trim();
    if sane_originator(leading) && official_originator(leading) {
        let originator = canonical_originator(leading);
        return Some((
            originator.clone(),
            format!("{originator}{}", &user_agent[slash..]),
        ));
    }

    let trailer = trailer_originator(user_agent)?;
    if trailer.contains('/') || !sane_originator(trailer) || !official_originator(trailer) {
        return None;
    }
    let originator = canonical_originator(trailer);
    Some((
        originator.clone(),
        format!("{originator}{}", &user_agent[slash..]),
    ))
}

fn trailer_originator(user_agent: &str) -> Option<&str> {
    let open = user_agent.rfind('(')?;
    let rest = &user_agent[open + 1..];
    let close = rest.find(')')?;
    if !rest[close + 1..].trim().is_empty() {
        return None;
    }
    let inside = rest[..close].trim();
    let name = inside
        .split_once(';')
        .map_or(inside, |(name, _)| name)
        .trim();
    (!name.is_empty()).then_some(name)
}

fn official_originator(originator: &str) -> bool {
    let lower = originator.trim().to_ascii_lowercase();
    OFFICIAL_ORIGINATORS.contains(&lower.as_str()) || originator.starts_with("Codex ")
}

fn canonical_originator(originator: &str) -> String {
    let lower = originator.trim().to_ascii_lowercase();
    if OFFICIAL_ORIGINATORS.contains(&lower.as_str()) {
        lower
    } else {
        originator.trim().to_string()
    }
}

fn sane_originator(originator: &str) -> bool {
    !originator.is_empty()
        && originator.len() <= MAX_ORIGINATOR_LEN
        && originator
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

fn version_at_least(candidate: &str, minimum: &str) -> bool {
    parse_version(candidate)
        .zip(parse_version(minimum))
        .is_some_and(|(candidate, minimum)| candidate >= minimum)
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.trim().trim_start_matches('v');
    let numeric = value.split(['-', '+']).next()?;
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_release_versions_are_strict_and_never_downgrade_builtin() {
        assert_eq!(
            extract_stable_release_version("rust-v0.145.2"),
            Some("0.145.2".to_string())
        );
        assert!(extract_stable_release_version("0.145.2-alpha.1").is_none());
        assert!(extract_stable_release_version("0.145.2+build.1").is_none());
        assert!(extract_stable_release_version("0.145").is_none());
        assert_eq!(
            parse_latest_release_version(
                br#"{"name":"0.145.3-alpha.1","tag_name":"rust-v0.145.2"}"#,
            )
            .unwrap(),
            "0.145.2"
        );
        assert_eq!(
            resolve_configured_version(None, Some("0.100.0")),
            DEFAULT_CODEX_VERSION
        );
    }

    #[test]
    fn explicit_version_precedes_synced_cache() {
        assert_eq!(
            resolve_configured_version(Some("0.146.0"), Some("0.147.0")),
            "0.146.0"
        );
        assert_eq!(resolve_configured_version(None, Some("0.147.0")), "0.147.0");
        assert_eq!(
            resolve_configured_version(Some("0.144.0"), Some("0.147.0")),
            "0.147.0"
        );
    }

    #[test]
    fn version_cache_accepts_only_valid_last_success_records() {
        let directory = std::env::temp_dir().join(format!(
            "cc-switch-codex-version-cache-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = version_cache_path(&directory);
        crate::infra::storage::write_json_pretty(
            &path,
            &CodexVersionCache {
                schema_version: CODEX_VERSION_CACHE_SCHEMA,
                version: "0.145.0".to_string(),
                synced_at_ms: 1,
                source: "github_latest_release".to_string(),
            },
        )
        .unwrap();
        assert_eq!(
            read_version_cache(&path).unwrap().unwrap().version,
            "0.145.0"
        );

        crate::infra::storage::write_json_pretty(
            &path,
            &CodexVersionCache {
                schema_version: CODEX_VERSION_CACHE_SCHEMA,
                version: "0.999.0-alpha.1".to_string(),
                synced_at_ms: 2,
                source: "github_latest_release".to_string(),
            },
        )
        .unwrap();
        assert!(read_version_cache(&path).unwrap().is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pairs_official_identity_and_recovers_override_trailer() {
        assert_eq!(
            pair_user_agent("CODEX_CLI_RS/0.144.1 (Ubuntu; x86_64) xterm"),
            Some((
                "codex_cli_rs".to_string(),
                "codex_cli_rs/0.144.1 (Ubuntu; x86_64) xterm".to_string()
            ))
        );
        assert_eq!(
            pair_user_agent("cccc/0.144.1 (Ubuntu 22.04; x86_64) xterm (codex-tui; 0.144.1)"),
            Some((
                "codex-tui".to_string(),
                "codex-tui/0.144.1 (Ubuntu 22.04; x86_64) xterm (codex-tui; 0.144.1)".to_string()
            ))
        );
    }

    #[test]
    fn rejects_third_party_and_malformed_identity() {
        assert!(pair_user_agent("luna/1.0.0").is_none());
        assert!(pair_user_agent("codex_cli_rs_evil/1.0.0").is_none());
        assert!(pair_user_agent("Codex \u{1}evil/1.0.0").is_none());
        assert!(pair_user_agent("curl").is_none());
    }

    #[test]
    fn canonical_auth_identity_pairs_originator_and_current_version_without_version_header() {
        let (originator, user_agent) = canonical_auth_identity();
        assert_eq!(originator, DEFAULT_CODEX_ORIGINATOR);
        assert!(user_agent.starts_with(&format!("{DEFAULT_CODEX_ORIGINATOR}/")));
        assert!(user_agent.contains(&configured_version()));
    }

    #[test]
    fn finalizer_repairs_pair_and_version_or_falls_back() {
        let mut headers = vec![
            ("originator", "codex_cli_rs".to_string()),
            ("version", "0.125.0".to_string()),
            (
                "user-agent",
                "codex-tui/0.140.2 (Mac OS X; arm64) iTerm".to_string(),
            ),
        ];
        finalize_headers(&mut headers);
        assert_eq!(header_value(&headers, "originator"), Some("codex-tui"));
        assert_eq!(
            header_value(&headers, "version"),
            Some(DEFAULT_CODEX_VERSION)
        );

        replace_or_push(&mut headers, "user-agent", "PostmanRuntime/7".to_string());
        finalize_headers(&mut headers);
        assert_eq!(header_value(&headers, "originator"), Some("codex_cli_rs"));
        assert!(header_value(&headers, "user-agent")
            .is_some_and(|value| value.starts_with("codex_cli_rs/0.144.1")));
    }
}
