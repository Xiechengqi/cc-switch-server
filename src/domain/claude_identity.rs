//! Bounded decoding of public Claude Code metadata; never an authentication proof.
use serde::Deserialize;

#[derive(Deserialize)]
pub struct ClaudeMetadataIdentity {
    pub device_id: String,
    pub account_uuid: String,
    pub session_id: String,
    #[serde(default)]
    pub parent_session_id: Option<String>,
}

pub fn valid_session_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b':' | b'.'))
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, ch)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                ch == b'-'
            } else {
                ch.is_ascii_hexdigit()
            }
        })
}

pub fn parse_metadata_identity(value: &str) -> Option<ClaudeMetadataIdentity> {
    if value.len() > 2048 {
        return None;
    }
    let identity: ClaudeMetadataIdentity = serde_json::from_str(value).ok()?;
    (identity.device_id.len() == 64
        && identity
            .device_id
            .bytes()
            .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
        && (identity.account_uuid.is_empty() || valid_uuid(&identity.account_uuid))
        && valid_uuid(&identity.session_id)
        && identity.parent_session_id.as_deref().is_none_or(valid_uuid))
    .then_some(identity)
}

pub fn session_from_metadata_user_id(value: &str) -> Option<String> {
    if value.len() > 2048 {
        return None;
    }
    if let Some(identity) = parse_metadata_identity(value) {
        return Some(identity.session_id);
    }
    // Older integrations encoded only the public session field. This is a
    // correlation hint, not enough on its own to classify a native CLI.
    if let Ok(identity) = serde_json::from_str::<serde_json::Value>(value) {
        if ["device_id", "account_uuid", "parent_session_id"]
            .iter()
            .any(|key| identity.get(*key).is_some())
        {
            return None;
        }
        if let Some(session) = identity
            .get("session_id")
            .and_then(serde_json::Value::as_str)
        {
            return valid_session_identifier(session).then(|| session.to_string());
        }
    }
    let value = value.trim();
    if value.len() > 512
        || !value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b':' | b'.'))
    {
        return None;
    }
    let session = value.split_once("_session_")?.1;
    valid_session_identifier(session).then(|| session.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modern_and_legacy_metadata_are_bounded_and_validated() {
        let id = serde_json::json!({"device_id":"f".repeat(64),"account_uuid":"", "session_id":"11111111-2222-4333-8444-555555555555"}).to_string();
        assert_eq!(
            session_from_metadata_user_id(&id).as_deref(),
            Some("11111111-2222-4333-8444-555555555555")
        );
        assert_eq!(
            session_from_metadata_user_id("user_a_account__session_old").as_deref(),
            Some("old")
        );
        for invalid in [
            id.replace("11111111-", "invalid-"),
            id.replace(&"f".repeat(64), "device"),
            "x".repeat(2049),
            "user_x_session_".into(),
        ] {
            assert!(session_from_metadata_user_id(&invalid).is_none());
        }
    }
}
