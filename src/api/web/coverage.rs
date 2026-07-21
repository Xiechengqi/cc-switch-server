use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::providers::model::{AppKind, Provider, ProviderType};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCoverage {
    pub generated_from: Value,
    pub provider_types: Vec<ProviderTypeCoverage>,
    pub presets: PresetCoverage,
    #[serde(default)]
    pub fixtures: ProviderFixtureCoverage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTypeCoverage {
    pub id: String,
    pub label: String,
    pub apps: Vec<String>,
    pub required: bool,
    pub present_in_source: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetCoverage {
    pub claude: Vec<PresetSummary>,
    pub codex: Vec<PresetSummary>,
    pub gemini: Vec<PresetSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<crate::domain::providers::registry::ProfileId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_schema_revision: Option<u32>,
    pub provider_type: Option<String>,
    #[serde(default)]
    pub api_format: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFixtureCoverage {
    #[serde(default)]
    pub claude: Vec<ProviderFixture>,
    #[serde(default)]
    pub codex: Vec<ProviderFixture>,
    #[serde(default)]
    pub gemini: Vec<ProviderFixture>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFixture {
    pub app: AppKind,
    pub name: String,
    pub expected_provider_type: ProviderType,
    pub provider: Provider,
}

impl ProviderCoverage {
    pub fn load_embedded() -> anyhow::Result<Self> {
        let raw = include_str!("../../../assets/contract/provider-coverage.json");
        Ok(serde_json::from_str(raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::classify_provider;

    const REQUIRED_PROVIDER_TYPES: &[&str] = &[
        "claude",
        "claude_auth",
        "claude_oauth",
        "codex",
        "codex_oauth",
        "gemini",
        "gemini_cli",
        "openrouter",
        "github_copilot",
        "deepseek_account",
        "kiro_oauth",
        "cursor_oauth",
        "cursor_apikey",
        "antigravity_oauth",
        "agy_oauth",
        "ollama_cloud",
    ];

    const SERVER_COMPAT_PROVIDER_TYPES: &[&str] = &["aws_bedrock", "nvidia", "deepseek_api"];

    #[test]
    fn embedded_coverage_contains_required_provider_types() {
        let coverage = ProviderCoverage::load_embedded().unwrap();
        for required in REQUIRED_PROVIDER_TYPES {
            assert!(
                coverage
                    .provider_types
                    .iter()
                    .any(|item| item.id == *required && item.required && item.present_in_source),
                "missing required provider type: {required}"
            );
        }
    }

    #[test]
    fn embedded_coverage_contains_server_compatibility_provider_types() {
        let coverage = ProviderCoverage::load_embedded().unwrap();
        for expected in SERVER_COMPAT_PROVIDER_TYPES {
            assert!(
                coverage
                    .provider_types
                    .iter()
                    .any(|item| item.id == *expected && !item.required),
                "missing server compatibility provider type: {expected}"
            );
        }
    }

    #[test]
    fn embedded_coverage_contains_all_core_app_presets() {
        let coverage = ProviderCoverage::load_embedded().unwrap();
        assert!(
            !coverage.presets.claude.is_empty(),
            "missing Claude presets"
        );
        assert!(!coverage.presets.codex.is_empty(), "missing Codex presets");
        assert!(
            !coverage.presets.gemini.is_empty(),
            "missing Gemini presets"
        );
    }

    #[test]
    fn embedded_provider_fixtures_match_classification() {
        let coverage = ProviderCoverage::load_embedded().unwrap();
        let fixtures = coverage
            .fixtures
            .claude
            .iter()
            .chain(coverage.fixtures.codex.iter())
            .chain(coverage.fixtures.gemini.iter());

        for fixture in fixtures {
            let actual = classify_provider(fixture.app, &fixture.provider);
            assert_eq!(
                actual, fixture.expected_provider_type,
                "fixture {:?} / {} classified incorrectly",
                fixture.app, fixture.name
            );
        }
    }

    #[test]
    fn exported_provider_structures_cover_required_fields() {
        let raw = include_str!("../../../assets/contract/provider-fixtures/structures.json");
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let files = value
            .get("files")
            .and_then(serde_json::Value::as_array)
            .expect("structures files must be an array");
        assert!(files.len() >= 20, "provider structures export regressed");

        for field in [
            "settingsConfig",
            "meta",
            "models",
            "modelMapping",
            "authBinding",
            "codexConfig",
            "geminiConfig",
        ] {
            assert!(
                files.iter().any(|file| {
                    file.pointer(&format!("/coveredFields/{field}"))
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                }),
                "provider structures do not cover {field}"
            );
        }
    }
}
