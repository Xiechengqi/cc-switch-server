//! Managed Codex Responses negotiation. Never used by models/search or generic
//! OpenAI-compatible providers. Values cannot suppress the root usage ledger.
use axum::http::HeaderMap;
use sha2::{Digest, Sha256};

use super::ProxyError;

pub(super) const EXCLUDE_HEADER: &str = "x-codex-exclude";
const MAX_EXCLUDE_BYTES: usize = 1024;

pub(super) fn response_exclusions(client: &HeaderMap) -> Result<Option<String>, ProxyError> {
    let mut bytes = 0usize;
    let mut tools = false;
    let mut attribution = false;
    for value in client.get_all(EXCLUDE_HEADER) {
        bytes = bytes
            .saturating_add(value.as_bytes().len())
            .saturating_add(1);
        if bytes > MAX_EXCLUDE_BYTES {
            return Err(ProxyError::bad_request(
                "X-Codex-Exclude exceeds 1024 bytes",
            ));
        }
        let value = value
            .to_str()
            .map_err(|_| ProxyError::bad_request("invalid X-Codex-Exclude header"))?;
        for token in value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            match token {
                "tools" => tools = true,
                "usage.attribution" => attribution = true,
                _ => {
                    return Err(ProxyError::bad_request(
                        "X-Codex-Exclude only supports tools and usage.attribution",
                    ))
                }
            }
        }
    }
    Ok(match (tools, attribution) {
        (true, true) => Some("tools,usage.attribution".to_string()),
        (true, false) => Some("tools".to_string()),
        (false, true) => Some("usage.attribution".to_string()),
        (false, false) => None,
    })
}

fn is_negotiation(name: &str) -> bool {
    matches!(
        name,
        "x-codex-turn-state"
            | "x-codex-beta-features"
            | "x-responsesapi-include-timing-metrics"
            | "x-oai-attestation"
            | EXCLUDE_HEADER
    )
}

fn is_diagnostic(name: &str) -> bool {
    matches!(name, "x-client-request-id" | "x-codex-turn-metadata")
}

fn hash_part(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

// The prefix retains the fixed tenant, binding, endpoint, credentials and CLI
// identity. The suffix isolates negotiated options. Neither contains raw IDs,
// credentials, attestation or metadata; a bounded pool can compare scopes.
pub(super) fn websocket_cache_key(scope: &[&str], headers: &[(String, String)]) -> String {
    let mut base = Sha256::new();
    let mut negotiated = Sha256::new();
    hash_part(&mut base, "codex-responses-ws-v1");
    for value in scope {
        hash_part(&mut base, value);
    }
    // The transport inserts into HeaderMap: a later case-insensitive value
    // replaces an earlier value. Hash exactly that effective view, including
    // duplicate-header precedence, rather than sorting all duplicate values.
    let canonical: std::collections::BTreeMap<_, _> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value))
        .collect();
    for (name, value) in canonical {
        if is_diagnostic(&name) {
            continue;
        }
        let digest = if is_negotiation(&name) {
            &mut negotiated
        } else {
            &mut base
        };
        hash_part(digest, &name);
        hash_part(digest, value);
    }
    format!(
        "{}:{}",
        hex::encode(base.finalize()),
        hex::encode(negotiated.finalize())
    )
}

// Fixed inline storage is charged by the active socket owner and cache indexes.
// This is only a recent-completion conflict guard, never a history replay store.
#[derive(Default)]
pub(super) struct CompletedResponseIds {
    digests: [[u8; 32]; 32],
    count: usize,
    next: usize,
}

impl CompletedResponseIds {
    pub(super) fn contains(&self, id: &str) -> bool {
        if id.is_empty() || id.len() > 512 {
            return false;
        }
        let digest: [u8; 32] = Sha256::digest(id.as_bytes()).into();
        self.digests[..self.count].contains(&digest)
    }

    pub(super) fn remember(&mut self, id: &str) {
        if id.trim().is_empty() || id.len() > 512 || self.contains(id) {
            return;
        }
        self.digests[self.next] = Sha256::digest(id.as_bytes()).into();
        self.next = (self.next + 1) % self.digests.len();
        self.count = (self.count + 1).min(self.digests.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn codex_r6_exclusions_are_bounded_canonical_and_preserve_billing_fields() {
        let mut headers = HeaderMap::new();
        headers.append(
            EXCLUDE_HEADER,
            HeaderValue::from_static(" usage.attribution, tools "),
        );
        headers.append(EXCLUDE_HEADER, HeaderValue::from_static("tools,, "));
        assert_eq!(
            response_exclusions(&headers).unwrap().as_deref(),
            Some("tools,usage.attribution")
        );
        for value in ["usage", "usage.input_tokens", "output", "Tools", "unknown"] {
            headers.insert(EXCLUDE_HEADER, HeaderValue::from_str(value).unwrap());
            assert!(response_exclusions(&headers).is_err(), "{value}");
        }
        for value in ["", " ", ", ,"] {
            headers.insert(EXCLUDE_HEADER, HeaderValue::from_str(value).unwrap());
            assert_eq!(response_exclusions(&headers).unwrap(), None);
        }
        headers.insert(
            EXCLUDE_HEADER,
            HeaderValue::from_str(&" ".repeat(1024)).unwrap(),
        );
        assert!(response_exclusions(&headers).is_err());
        headers.insert(EXCLUDE_HEADER, HeaderValue::from_bytes(b"\xff").unwrap());
        assert!(response_exclusions(&headers).is_err());
    }

    #[test]
    fn codex_r6_identity_is_canonical_but_negotiation_and_tenants_are_separate() {
        let headers = vec![
            ("Authorization".into(), "Bearer secret".into()),
            ("X-Codex-Beta-Features".into(), "a".into()),
            ("x-client-request-id".into(), "trace1".into()),
        ];
        let key = websocket_cache_key(&["share-a", "user-a"], &headers);
        let reordered = vec![
            ("x-client-request-id".into(), "trace2".into()),
            ("x-codex-beta-features".into(), "a".into()),
            ("authorization".into(), "Bearer secret".into()),
        ];
        assert_eq!(key, websocket_cache_key(&["share-a", "user-a"], &reordered));
        let mut duplicates = reordered.clone();
        duplicates.insert(
            0,
            (
                "X-Codex-Beta-Features".into(),
                "ignored-earlier-value".into(),
            ),
        );
        assert_eq!(
            key,
            websocket_cache_key(&["share-a", "user-a"], &duplicates)
        );
        duplicates.push(("X-Codex-Beta-Features".into(), "last-value".into()));
        assert_ne!(
            key,
            websocket_cache_key(&["share-a", "user-a"], &duplicates)
        );
        let mut changed = reordered;
        changed[1].1 = "b".into();
        let other = websocket_cache_key(&["share-a", "user-a"], &changed);
        assert_eq!(
            key.split_once(':').unwrap().0,
            other.split_once(':').unwrap().0
        );
        assert_ne!(key, other);
        assert_ne!(
            key.split_once(':').unwrap().0,
            websocket_cache_key(&["share-b", "user-a"], &headers)
                .split_once(':')
                .unwrap()
                .0
        );
        assert!(!key.contains("secret"));
        let mut ring = CompletedResponseIds::default();
        for id in 0..33 {
            ring.remember(&format!("resp-{id}"));
        }
        assert!(!ring.contains("resp-0"));
        assert!(ring.contains("resp-32"));
        assert!(!ring.contains(" resp-32 "));
    }
}
