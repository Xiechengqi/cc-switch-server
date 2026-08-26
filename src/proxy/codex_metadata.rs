use axum::http::HeaderMap;
use bytes::Bytes;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::ProxyError;

const TURN_METADATA_HEADER: &str = "x-codex-turn-metadata";
const MAX_TURN_METADATA_BYTES: usize = 32 * 1024;
const WORKSPACE_PLACEHOLDER_PREFIX: &str = "/workspace/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexMetadataScope {
    pub account_id: String,
    pub auth_identity_generation: u64,
    pub workspace_id: String,
    pub runtime_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataDecision {
    Unchanged,
    Scrubbed,
    DroppedMalformed,
    DroppedOversize,
}

impl MetadataDecision {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Scrubbed => "scrubbed",
            Self::DroppedMalformed => "dropped_malformed",
            Self::DroppedOversize => "dropped_oversize",
        }
    }
}

pub(crate) fn scrub_body_bytes(
    body: &Bytes,
    scope: &CodexMetadataScope,
) -> Result<(Bytes, MetadataDecision), ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| ProxyError::bad_request(format!("invalid Codex body: {error}")))?;
    let decision = scrub_body_value(&mut value, scope);
    if decision == MetadataDecision::Unchanged {
        return Ok((body.clone(), MetadataDecision::Unchanged));
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map(|body| (body, decision))
        .map_err(|error| ProxyError::bad_request(format!("encode scrubbed Codex body: {error}")))
}

pub(crate) fn scrub_body_value(body: &mut Value, scope: &CodexMetadataScope) -> MetadataDecision {
    if scrub_client_metadata_values(body, scope) {
        MetadataDecision::Scrubbed
    } else {
        MetadataDecision::Unchanged
    }
}

fn scrub_client_metadata_values(value: &mut Value, scope: &CodexMetadataScope) -> bool {
    match value {
        Value::Object(object) => {
            let mut changed = object
                .get_mut("client_metadata")
                .is_some_and(|metadata| scrub_metadata_value(metadata, scope));
            for (key, child) in object.iter_mut() {
                if key != "client_metadata" {
                    changed |= scrub_client_metadata_values(child, scope);
                }
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            scrub_client_metadata_values(item, scope) || changed
        }),
        _ => false,
    }
}

pub(crate) fn scrub_owned_headers(
    headers: &mut Vec<(String, String)>,
    scope: &CodexMetadataScope,
) -> MetadataDecision {
    let matching_headers = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(TURN_METADATA_HEADER))
        .count();
    let Some(raw) = headers
        .iter()
        .rfind(|(name, _)| name.eq_ignore_ascii_case(TURN_METADATA_HEADER))
        .map(|(_, value)| value.clone())
    else {
        return MetadataDecision::Unchanged;
    };
    if raw.len() > MAX_TURN_METADATA_BYTES {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(TURN_METADATA_HEADER));
        return MetadataDecision::DroppedOversize;
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(raw.as_bytes()) else {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(TURN_METADATA_HEADER));
        return MetadataDecision::DroppedMalformed;
    };
    let scrubbed = scrub_metadata_value(&mut value, scope);
    if !scrubbed && matching_headers == 1 {
        return MetadataDecision::Unchanged;
    }
    let Ok(encoded) = serde_json::to_string(&value) else {
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(TURN_METADATA_HEADER));
        return MetadataDecision::DroppedMalformed;
    };
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case(TURN_METADATA_HEADER));
    headers.push((TURN_METADATA_HEADER.to_string(), encoded));
    MetadataDecision::Scrubbed
}

pub(crate) fn scrub_header_map(
    headers: &mut HeaderMap,
    scope: &CodexMetadataScope,
) -> MetadataDecision {
    let had_metadata = headers.contains_key(TURN_METADATA_HEADER);
    let mut owned = headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect::<Vec<_>>();
    let mut decision = scrub_owned_headers(&mut owned, scope);
    if had_metadata
        && !owned
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case(TURN_METADATA_HEADER))
        && decision == MetadataDecision::Unchanged
    {
        decision = MetadataDecision::DroppedMalformed;
    }
    if decision != MetadataDecision::Unchanged {
        headers.remove(TURN_METADATA_HEADER);
        if let Some((_, value)) = owned
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(TURN_METADATA_HEADER))
        {
            if let Ok(value) = value.parse() {
                headers.insert(TURN_METADATA_HEADER, value);
            }
        }
    }
    decision
}

fn scrub_metadata_value(metadata: &mut Value, scope: &CodexMetadataScope) -> bool {
    match metadata {
        Value::Object(object) => {
            let mut changed = false;
            if let Some(workspaces) = object.get_mut("workspaces").and_then(Value::as_object_mut) {
                changed |= scrub_workspaces(workspaces, scope);
            }
            for value in object.values_mut() {
                changed |= scrub_metadata_value(value, scope);
            }
            changed
        }
        Value::Array(items) => items.iter_mut().fold(false, |changed, item| {
            scrub_metadata_value(item, scope) || changed
        }),
        _ => false,
    }
}

fn scrub_workspaces(workspaces: &mut Map<String, Value>, scope: &CodexMetadataScope) -> bool {
    let original = std::mem::take(workspaces);
    let mut changed = false;
    for (path, mut entry) in original {
        let placeholder = if is_workspace_placeholder(&path) {
            path
        } else {
            changed = true;
            workspace_placeholder(scope, &path)
        };
        if let Some(entry) = entry.as_object_mut() {
            for key in [
                "associated_remote_urls",
                "associatedRemoteUrls",
                "remote_urls",
            ] {
                changed |= entry.remove(key).is_some();
            }
            if entry
                .get("latest_git_commit_hash")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
            {
                let replacement = commit_placeholder(scope, &placeholder);
                if entry.get("latest_git_commit_hash").and_then(Value::as_str)
                    != Some(replacement.as_str())
                {
                    entry.insert(
                        "latest_git_commit_hash".to_string(),
                        Value::String(replacement),
                    );
                    changed = true;
                }
            }
        }
        workspaces.insert(placeholder, entry);
    }
    changed
}

fn workspace_placeholder(scope: &CodexMetadataScope, path: &str) -> String {
    let digest = scoped_digest(scope, "workspace-path", path);
    format!(
        "{WORKSPACE_PLACEHOLDER_PREFIX}{}",
        hex::encode(&digest[..8])
    )
}

fn commit_placeholder(scope: &CodexMetadataScope, placeholder: &str) -> String {
    hex::encode(&scoped_digest(scope, "git-commit", placeholder)[..20])
}

fn scoped_digest(scope: &CodexMetadataScope, domain: &str, value: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for component in [
        "cc-switch-server:codex-metadata:v1",
        domain,
        scope.account_id.as_str(),
        &scope.auth_identity_generation.to_string(),
        scope.workspace_id.as_str(),
        scope.runtime_fingerprint.as_str(),
        value,
    ] {
        hasher.update((component.len() as u64).to_be_bytes());
        hasher.update(component.as_bytes());
    }
    hasher.finalize().into()
}

fn is_workspace_placeholder(path: &str) -> bool {
    path.strip_prefix(WORKSPACE_PLACEHOLDER_PREFIX)
        .is_some_and(|suffix| {
            suffix.len() == 16 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scope(account: &str) -> CodexMetadataScope {
        CodexMetadataScope {
            account_id: account.to_string(),
            auth_identity_generation: 7,
            workspace_id: "workspace-a".to_string(),
            runtime_fingerprint: "runtime-a".to_string(),
        }
    }

    #[test]
    fn scrubs_paths_remotes_and_commits_stably_without_changing_shape() {
        let original_path = "/Users/alice/私有/project";
        let original_remote = "git@github.com:alice/private.git";
        let original_commit = "0123456789abcdef0123456789abcdef01234567";
        let mut value = json!({
            "workspaces": {
                (original_path): {
                    "associated_remote_urls": {"origin": original_remote},
                    "latest_git_commit_hash": original_commit,
                    "has_changes": true
                }
            },
            "thread_id": "unchanged"
        });
        assert!(scrub_metadata_value(&mut value, &scope("account-a")));
        let once = value.clone();
        assert!(!scrub_metadata_value(&mut value, &scope("account-a")));
        assert_eq!(value, once);
        let encoded = value.to_string();
        assert!(!encoded.contains(original_path));
        assert!(!encoded.contains(original_remote));
        assert!(!encoded.contains(original_commit));
        let (path, entry) = value["workspaces"]
            .as_object()
            .unwrap()
            .iter()
            .next()
            .unwrap();
        assert!(is_workspace_placeholder(path));
        assert_eq!(entry["latest_git_commit_hash"].as_str().unwrap().len(), 40);
        assert_eq!(entry["has_changes"], true);
    }

    #[test]
    fn placeholders_are_domain_separated_by_account() {
        assert_ne!(
            workspace_placeholder(&scope("account-a"), "/home/alice/project"),
            workspace_placeholder(&scope("account-b"), "/home/alice/project")
        );
    }

    #[test]
    fn malformed_and_oversized_optional_headers_are_dropped() {
        let mut malformed = vec![(TURN_METADATA_HEADER.to_string(), "not-json".to_string())];
        assert_eq!(
            scrub_owned_headers(&mut malformed, &scope("account-a")),
            MetadataDecision::DroppedMalformed
        );
        assert!(malformed.is_empty());

        let mut oversized = vec![(
            TURN_METADATA_HEADER.to_string(),
            "x".repeat(MAX_TURN_METADATA_BYTES + 1),
        )];
        assert_eq!(
            scrub_owned_headers(&mut oversized, &scope("account-a")),
            MetadataDecision::DroppedOversize
        );
        assert!(oversized.is_empty());
    }

    #[test]
    fn duplicate_headers_collapse_without_leaking_an_earlier_value() {
        let sensitive_path = "/Users/alice/private";
        let mut headers = vec![
            (
                TURN_METADATA_HEADER.to_string(),
                json!({"workspaces": {(sensitive_path): {"has_changes": true}}}).to_string(),
            ),
            (
                TURN_METADATA_HEADER.to_string(),
                json!({"thread_id": "safe"}).to_string(),
            ),
        ];

        assert_eq!(
            scrub_owned_headers(&mut headers, &scope("account-a")),
            MetadataDecision::Scrubbed
        );
        assert_eq!(headers.len(), 1);
        assert!(!headers[0].1.contains(sensitive_path));
        assert_eq!(
            serde_json::from_str::<Value>(&headers[0].1).unwrap(),
            json!({"thread_id": "safe"})
        );
    }

    #[test]
    fn scrubs_workspace_metadata_nested_inside_arrays() {
        let sensitive_path = "C:\\Users\\alice\\private";
        let mut value = json!({
            "nested": [{"workspaces": {(sensitive_path): {"has_changes": true}}}]
        });

        assert!(scrub_metadata_value(&mut value, &scope("account-a")));
        assert!(!value.to_string().contains(sensitive_path));
        let path = value
            .pointer("/nested/0/workspaces")
            .and_then(Value::as_object)
            .and_then(|workspaces| workspaces.keys().next())
            .unwrap();
        assert!(is_workspace_placeholder(path));
    }
}
