pub(crate) const DEFAULT_REGION: &str = "us-east-1";

pub(crate) const STATIC_MODEL_IDS: &[&str] = &[
    "claude-haiku-4.5",
    "claude-opus-4.5",
    "claude-opus-4.6",
    "claude-opus-4.7",
    "claude-opus-4.8",
    "claude-sonnet-4.5",
    "claude-sonnet-4.6",
    "claude-sonnet-4.8",
];

pub(crate) fn normalize_region(raw: &str) -> Option<String> {
    let region = raw.trim();
    let bytes = region.as_bytes();
    if region.is_empty()
        || region.len() > 63
        || !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        return None;
    }
    Some(region.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_is_normalized_as_one_safe_dns_label() {
        assert_eq!(
            normalize_region(" US-EAST-1 ").as_deref(),
            Some("us-east-1")
        );
        for invalid in [
            "",
            "-us-east-1",
            "us-east-1-",
            "../us-east-1",
            "us.east-1",
            "us-east-1:443",
            "us-east-1\r\nx-bad",
        ] {
            assert_eq!(normalize_region(invalid), None, "region={invalid:?}");
        }
        assert_eq!(normalize_region(&"a".repeat(64)), None);
    }
}
