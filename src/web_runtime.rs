use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

static CONTRACT: OnceLock<WebRuntimeContract> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRuntimeContract {
    pub version: u32,
    pub product: String,
    pub ui_automation_allowed: bool,
    #[serde(default)]
    pub retained_features: Vec<WebRuntimeFeature>,
    #[serde(default)]
    pub hidden_features: Vec<WebRuntimeFeature>,
    #[serde(default)]
    pub excluded_features: Vec<WebRuntimeFeature>,
    #[serde(default)]
    pub commands: Vec<WebRuntimeCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRuntimeFeature {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRuntimeCommand {
    pub name: String,
    pub support: WebRuntimeCommandSupport,
    pub implemented: bool,
    pub feature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WebRuntimeCommandSupport {
    Native,
    Shim,
    Excluded,
}

pub fn contract() -> &'static WebRuntimeContract {
    CONTRACT.get_or_init(|| {
        serde_json::from_str(include_str!("../docs/web-runtime-contract.json"))
            .expect("docs/web-runtime-contract.json is valid")
    })
}

pub fn command(name: &str) -> Option<&'static WebRuntimeCommand> {
    contract()
        .commands
        .iter()
        .find(|command| command.name == name)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn contract_disables_ui_automation() {
        assert!(!contract().ui_automation_allowed);
    }

    #[test]
    fn contract_has_unique_features_and_commands() {
        let contract = contract();
        let mut features = BTreeSet::new();
        for feature in contract
            .retained_features
            .iter()
            .chain(contract.hidden_features.iter())
            .chain(contract.excluded_features.iter())
        {
            assert!(
                features.insert(feature.id.as_str()),
                "duplicate feature {}",
                feature.id
            );
        }

        let mut commands = BTreeSet::new();
        for command in &contract.commands {
            assert!(
                commands.insert(command.name.as_str()),
                "duplicate command {}",
                command.name
            );
            assert!(
                features.contains(command.feature.as_str()),
                "command {} references unknown feature {}",
                command.name,
                command.feature
            );
            if command.support == WebRuntimeCommandSupport::Excluded {
                assert!(
                    !command.implemented,
                    "excluded command cannot be implemented"
                );
            }
        }
    }
}
