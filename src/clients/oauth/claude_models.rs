use crate::domain::claude_cli::CLAUDE_WIRE_PROFILE;

pub const CLAUDE_MODEL_CATALOG_CAPTURED_AT_MS: i64 = 1_787_702_400_000;
pub const CLAUDE_MODEL_IDS: &[&str] = &[
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-sonnet-5",
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeModelCatalog {
    pub models: Vec<String>,
    pub source: &'static str,
    pub stale: bool,
    pub fetched_at_ms: i64,
    pub wire_profile_id: &'static str,
    pub claude_code_version: &'static str,
}

pub fn static_claude_model_catalog() -> ClaudeModelCatalog {
    ClaudeModelCatalog {
        models: CLAUDE_MODEL_IDS
            .iter()
            .map(|model| (*model).to_string())
            .collect(),
        source: "claude_code_wire_profile",
        stale: false,
        fetched_at_ms: CLAUDE_MODEL_CATALOG_CAPTURED_AT_MS,
        wire_profile_id: CLAUDE_WIRE_PROFILE.id,
        claude_code_version: CLAUDE_WIRE_PROFILE.claude_code_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE_PROFILE_JSON: &str =
        include_str!("../../../assets/contract/claude-oauth-wire-profile.json");

    #[test]
    fn catalog_is_versioned_with_the_active_wire_profile() {
        let catalog = static_claude_model_catalog();
        assert_eq!(catalog.wire_profile_id, CLAUDE_WIRE_PROFILE.id);
        assert_eq!(
            catalog.claude_code_version,
            CLAUDE_WIRE_PROFILE.claude_code_version
        );
        assert_eq!(
            catalog.models,
            vec![
                "claude-fable-5",
                "claude-opus-5",
                "claude-opus-4-8",
                "claude-sonnet-5",
                "claude-opus-4-6",
                "claude-sonnet-4-6",
                "claude-haiku-4-5-20251001"
            ]
        );
        assert!(!catalog.stale);
    }

    #[test]
    fn catalog_and_runtime_versions_match_the_wire_profile_asset() {
        let profile: serde_json::Value = serde_json::from_str(WIRE_PROFILE_JSON).unwrap();
        let models = profile["modelCatalog"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model.as_str().unwrap())
            .collect::<Vec<_>>();
        let captured_at_ms =
            chrono::DateTime::parse_from_rfc3339(profile["capturedAt"].as_str().unwrap())
                .unwrap()
                .timestamp_millis();

        assert_eq!(profile["profileId"], CLAUDE_WIRE_PROFILE.id);
        assert_eq!(
            profile["versions"]["claudeCode"],
            CLAUDE_WIRE_PROFILE.claude_code_version
        );
        assert_eq!(
            profile["versions"]["stainlessSdk"],
            CLAUDE_WIRE_PROFILE.stainless_package_version
        );
        assert_eq!(
            profile["versions"]["node"],
            CLAUDE_WIRE_PROFILE.node_version
        );
        assert_eq!(
            profile["versions"]["axios"],
            CLAUDE_WIRE_PROFILE.axios_version
        );
        assert_eq!(
            profile["cch"]["seedHex"],
            format!("{:016x}", CLAUDE_WIRE_PROFILE.cch_seed)
        );
        assert_eq!(models.as_slice(), CLAUDE_MODEL_IDS);
        assert_eq!(
            profile["modelCatalog"]["source"],
            static_claude_model_catalog().source
        );
        assert_eq!(profile["modelCatalog"]["stale"], false);
        assert_eq!(captured_at_ms, CLAUDE_MODEL_CATALOG_CAPTURED_AT_MS);
    }
}
