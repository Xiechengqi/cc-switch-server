use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::build_info::build_info;

#[derive(Debug, thiserror::Error)]
pub enum SelfUpdateError {
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    Forbidden(String),
}

pub const SERVICE_UNIT: &str = "cc-switch-server.service";
pub const SERVICE_NAME: &str = "cc-switch-server";
pub const BINARY_INSTALL_PATH: &str = "/usr/local/bin/cc-switch-server";
pub const BINARY_STAGING_PATH: &str = "/usr/local/bin/.cc-switch-server.new";
pub const BINARY_ROLLBACK_PATH: &str = "/usr/local/bin/cc-switch-server.bak";
pub const SERVICE_LOG_PATH: &str = "/var/log/cc-switch-server.log";
const GITHUB_LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/Xiechengqi/cc-switch-server/releases/tags/latest";
const GITHUB_REPO_API: &str = "https://api.github.com/repos/Xiechengqi/cc-switch-server";

#[derive(Debug, Deserialize)]
struct GithubLatestRelease {
    tag_name: String,
    target_commitish: String,
}

#[derive(Debug, Deserialize)]
struct GithubGitRef {
    object: GithubGitObject,
}

#[derive(Debug, Deserialize)]
struct GithubGitObject {
    sha: String,
    #[serde(rename = "type")]
    object_type: String,
}

#[derive(Debug, Deserialize)]
struct GithubAnnotatedTag {
    object: GithubGitObject,
}

pub fn release_binary_url() -> &'static str {
    let target = env!("CC_SWITCH_BUILD_TARGET");
    if target.contains("aarch64") || target.contains("arm64") {
        "https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-arm64"
    } else {
        "https://github.com/Xiechengqi/cc-switch-server/releases/download/latest/cc-switch-server-linux-amd64"
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManager {
    Service,
    Nohup,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub manager: ServiceManager,
    pub active: bool,
    pub unit_name: Option<&'static str>,
    pub active_state: Option<String>,
    pub unit_file_state: Option<String>,
}

pub fn service_cc_switch_started() -> bool {
    Command::new("systemctl")
        .args(["is-active", "--quiet", SERVICE_UNIT])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
        || Command::new("service")
            .args([SERVICE_NAME, "status"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
}

pub fn detect_service_status() -> ServiceStatus {
    let started = service_cc_switch_started();
    if started {
        ServiceStatus {
            manager: ServiceManager::Service,
            active: true,
            unit_name: Some(SERVICE_NAME),
            active_state: Some("started".into()),
            unit_file_state: None,
        }
    } else {
        nohup_status()
    }
}

fn nohup_status() -> ServiceStatus {
    ServiceStatus {
        manager: ServiceManager::Nohup,
        active: true,
        unit_name: None,
        active_state: Some("running".into()),
        unit_file_state: None,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatestReleaseMeta {
    pub binary_url: String,
    pub available: bool,
    pub commit_id: Option<String>,
    pub commit_short: Option<String>,
    pub update_available: bool,
    pub etag: Option<String>,
    pub content_length: Option<u64>,
    pub error: Option<String>,
}

pub async fn fetch_latest_release_meta(client: &reqwest::Client) -> LatestReleaseMeta {
    let url = release_binary_url();
    let local_commit_id = build_info().commit_id;
    let mut meta = LatestReleaseMeta {
        binary_url: url.to_string(),
        available: false,
        commit_id: None,
        commit_short: None,
        update_available: false,
        etag: None,
        content_length: None,
        error: None,
    };

    match fetch_latest_release_commit(client).await {
        Ok(commit_id) => {
            meta.commit_short = Some(commit_short_from_id(&commit_id));
            meta.commit_id = Some(commit_id);
        }
        Err(err) => {
            meta.error = Some(format!("fetch latest release commit failed: {err}"));
            return meta;
        }
    }

    match client
        .head(url)
        .timeout(Duration::from_secs(8))
        .send()
        .await
    {
        Ok(resp) => {
            if resp.status().is_success() || resp.status().is_redirection() {
                meta.available = true;
                if let Some(value) = resp.headers().get("etag") {
                    meta.etag = value.to_str().ok().map(str::to_string);
                }
                if let Some(value) = resp.headers().get("content-length") {
                    meta.content_length = value.to_str().ok().and_then(|v| v.trim().parse().ok());
                }
            } else {
                meta.error = Some(format!("binary probe HTTP {}", resp.status()));
            }
        }
        Err(err) => meta.error = Some(format!("binary probe failed: {err}")),
    }

    if meta.error.is_none() {
        meta.update_available = meta.available
            && meta
                .commit_id
                .as_deref()
                .is_some_and(|remote| !commits_equal(remote, local_commit_id));
    }

    meta
}

pub(crate) async fn fetch_latest_release_commit(
    client: &reqwest::Client,
) -> Result<String, String> {
    let release = fetch_latest_release(client).await?;
    if is_commit_sha(&release.target_commitish) {
        return Ok(normalize_commit_id(&release.target_commitish));
    }
    match resolve_release_tag_commit(client, &release.tag_name).await {
        Ok(commit_id) => Ok(commit_id),
        Err(tag_err) => resolve_branch_head_commit(client, &release.target_commitish)
            .await
            .map_err(|branch_err| format!("{tag_err}; {branch_err}")),
    }
}

async fn fetch_latest_release(client: &reqwest::Client) -> Result<GithubLatestRelease, String> {
    let response = client
        .get(GITHUB_LATEST_RELEASE_API)
        .header("User-Agent", "cc-switch-server/0.1 release-check")
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response
        .json::<GithubLatestRelease>()
        .await
        .map_err(|err| err.to_string())
}

async fn resolve_release_tag_commit(
    client: &reqwest::Client,
    tag_name: &str,
) -> Result<String, String> {
    let tag_name = tag_name.trim();
    if tag_name.is_empty() {
        return Err("latest release is missing tag name".into());
    }
    let url = format!("{GITHUB_REPO_API}/git/ref/tags/{tag_name}");
    let response = client
        .get(url)
        .header("User-Agent", "cc-switch-server/0.1 release-check")
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("resolve release tag HTTP {}", response.status()));
    }
    let git_ref = response
        .json::<GithubGitRef>()
        .await
        .map_err(|err| err.to_string())?;
    match git_ref.object.object_type.as_str() {
        "commit" => Ok(normalize_commit_id(&git_ref.object.sha)),
        "tag" => resolve_annotated_tag_commit(client, &git_ref.object.sha).await,
        other => Err(format!("unsupported git ref object type: {other}")),
    }
}

async fn resolve_annotated_tag_commit(
    client: &reqwest::Client,
    tag_object_sha: &str,
) -> Result<String, String> {
    let url = format!("{GITHUB_REPO_API}/git/tags/{tag_object_sha}");
    let response = client
        .get(url)
        .header("User-Agent", "cc-switch-server/0.1 release-check")
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("resolve annotated tag HTTP {}", response.status()));
    }
    let tag = response
        .json::<GithubAnnotatedTag>()
        .await
        .map_err(|err| err.to_string())?;
    if tag.object.object_type != "commit" {
        return Err(format!(
            "annotated tag does not point to commit: {}",
            tag.object.object_type
        ));
    }
    Ok(normalize_commit_id(&tag.object.sha))
}

async fn resolve_branch_head_commit(
    client: &reqwest::Client,
    branch: &str,
) -> Result<String, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("latest release is missing target commit".into());
    }
    let url = format!("{GITHUB_REPO_API}/commits/{branch}");
    let response = client
        .get(url)
        .header("User-Agent", "cc-switch-server/0.1 release-check")
        .header("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("resolve branch head HTTP {}", response.status()));
    }
    #[derive(Debug, Deserialize)]
    struct GithubCommitHead {
        sha: String,
    }
    let commit = response
        .json::<GithubCommitHead>()
        .await
        .map_err(|err| err.to_string())?;
    if commit.sha.trim().is_empty() {
        return Err("resolved branch head is empty".into());
    }
    Ok(normalize_commit_id(&commit.sha))
}

fn is_commit_sha(value: &str) -> bool {
    let trimmed = value.trim();
    let len = trimmed.len();
    (7..=40).contains(&len) && trimmed.chars().all(|c| c.is_ascii_hexdigit())
}

fn normalize_commit_id(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn commit_short_from_id(commit_id: &str) -> String {
    let normalized = normalize_commit_id(commit_id);
    if normalized.len() <= 12 {
        normalized
    } else {
        normalized[..12].to_string()
    }
}

pub(crate) fn commits_equal(left: &str, right: &str) -> bool {
    let left = normalize_commit_id(left);
    let right = normalize_commit_id(right);
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let short_len = left.len().min(right.len()).min(12);
    left[..short_len] == right[..short_len]
}

pub fn backup_installed_binary() -> Result<(), SelfUpdateError> {
    let install = std::path::Path::new(BINARY_INSTALL_PATH);
    if !install.exists() {
        return Ok(());
    }
    std::fs::copy(install, BINARY_ROLLBACK_PATH).map_err(|err| {
        SelfUpdateError::Internal(format!(
            "backup {BINARY_INSTALL_PATH} to {BINARY_ROLLBACK_PATH} failed: {err}"
        ))
    })?;
    std::fs::File::open(BINARY_ROLLBACK_PATH)
        .and_then(|file| file.sync_all())
        .map_err(|err| SelfUpdateError::Internal(format!("sync rollback backup failed: {err}")))
}

pub async fn fetch_release_checksum(client: &reqwest::Client) -> Result<String, SelfUpdateError> {
    let url = format!("{}.sha256", release_binary_url());
    let response = client
        .get(&url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("checksum request failed: {err}")))?;
    if !response.status().is_success() {
        return Err(SelfUpdateError::Internal(format!(
            "checksum HTTP {}",
            response.status()
        )));
    }
    let body = response
        .text()
        .await
        .map_err(|err| SelfUpdateError::Internal(format!("read checksum failed: {err}")))?;
    parse_release_checksum(&body)
}

fn parse_release_checksum(body: &str) -> Result<String, SelfUpdateError> {
    let checksum = body.split_whitespace().next().unwrap_or_default();
    if checksum.len() != 64 || !checksum.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(SelfUpdateError::Internal(
            "release checksum is missing or invalid".into(),
        ));
    }
    Ok(checksum.to_ascii_lowercase())
}

pub fn is_containerized() -> bool {
    if std::env::var("CC_SWITCH_SERVER_ALLOW_CONTAINER_SELF_UPDATE").as_deref() == Ok("1") {
        return false;
    }
    std::path::Path::new("/.dockerenv").exists()
        || std::fs::read_to_string("/proc/1/cgroup")
            .map(|value| {
                value.contains("/docker/")
                    || value.contains("/kubepods/")
                    || value.contains("/containerd/")
            })
            .unwrap_or(false)
}

pub fn ensure_binary_writable() -> Result<(), SelfUpdateError> {
    use std::os::unix::fs::PermissionsExt;
    if is_containerized() {
        return Err(SelfUpdateError::Forbidden(
            "self-update is disabled in containers; deploy a new image instead".into(),
        ));
    }
    let staging_parent = std::path::Path::new(BINARY_STAGING_PATH)
        .parent()
        .ok_or_else(|| SelfUpdateError::Internal("staging path has no parent".into()))?;
    let staging_parent_meta = std::fs::metadata(staging_parent).map_err(|err| {
        SelfUpdateError::Internal(format!("stat {} failed: {err}", staging_parent.display()))
    })?;
    if !staging_parent_meta.is_dir() {
        return Err(SelfUpdateError::Forbidden(format!(
            "{} is not a directory",
            staging_parent.display()
        )));
    }
    let install_parent = std::path::Path::new(BINARY_INSTALL_PATH)
        .parent()
        .ok_or_else(|| SelfUpdateError::Internal("install path has no parent".into()))?;
    std::fs::create_dir_all(install_parent).map_err(|err| {
        SelfUpdateError::Internal(format!(
            "ensure install dir {} failed: {err}",
            install_parent.display()
        ))
    })?;
    let probe = install_parent.join(format!(
        ".cc-switch-server-write-probe-{}",
        std::process::id()
    ));
    std::fs::write(&probe, b"").map_err(|err| {
        SelfUpdateError::Forbidden(format!(
            "install directory {} is not writable: {err}",
            install_parent.display()
        ))
    })?;
    let _ = std::fs::remove_file(probe);
    let metadata = match std::fs::metadata(BINARY_INSTALL_PATH) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(SelfUpdateError::Internal(format!(
                "stat {BINARY_INSTALL_PATH} failed: {err}"
            )));
        }
    };
    let mode = metadata.permissions().mode();
    if mode & 0o200 == 0 {
        return Err(SelfUpdateError::Forbidden(format!(
            "binary at {BINARY_INSTALL_PATH} is not writable by this process"
        )));
    }
    Ok(())
}

pub fn rollback_available() -> bool {
    std::path::Path::new(BINARY_ROLLBACK_PATH).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commits_equal_matches_full_and_short_prefix() {
        let full = "aabbccddeeff00112233445566778899aabbccdd";
        let short = "aabbccddeeff";
        assert!(commits_equal(full, short));
        assert!(commits_equal(short, full));
        assert!(commits_equal(full, full));
    }

    #[test]
    fn commits_equal_rejects_different_commits() {
        assert!(!commits_equal(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        ));
    }

    #[test]
    fn release_checksum_parser_accepts_sha256sum_format() {
        let hash = "a".repeat(64);
        assert_eq!(
            parse_release_checksum(&format!("{hash}  cc-switch-server-linux-amd64\n")).unwrap(),
            hash
        );
        assert!(parse_release_checksum("not-a-checksum").is_err());
    }

    #[test]
    fn commit_short_from_id_uses_twelve_chars() {
        assert_eq!(
            commit_short_from_id("AABBCCDDEEFF001122334455"),
            "aabbccddeeff"
        );
    }

    #[test]
    fn is_commit_sha_accepts_hex_and_rejects_branch_names() {
        assert!(is_commit_sha("1584084a73cf3cc4c8ffec9260ac9a2e2e4f1419"));
        assert!(is_commit_sha("1584084"));
        assert!(!is_commit_sha("main"));
        assert!(!is_commit_sha("latest"));
    }
}
