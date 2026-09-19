use std::collections::BTreeMap;

use anyhow::{ensure, Context};
use serde_json::Value;

use crate::domain::accounts::store::AccountStore;
use crate::domain::providers::store::ProviderStore;
use crate::domain::sharing::shares::ShareStore;

pub(super) fn encrypted_provider_payloads(
    root: Option<&Value>,
    providers: &ProviderStore,
) -> anyhow::Result<BTreeMap<(String, String), Value>> {
    if providers.providers.is_empty() {
        return Ok(BTreeMap::new());
    }
    let root = root.context("providers.json is missing")?;
    ensure!(
        root.get("format").and_then(Value::as_str) == Some("cc-switch-provider-store"),
        "non-empty legacy Provider S1 store must be migrated to encrypted S2 before SQLite shadow import"
    );
    let records = root
        .get("records")
        .and_then(Value::as_object)
        .context("Provider S2 records are missing")?;
    let mut output = BTreeMap::new();
    for (app, values) in records {
        let values = values
            .as_object()
            .context("Provider S2 app records must be an object")?;
        for (provider_id, payload) in values {
            output.insert((app.clone(), provider_id.clone()), payload.clone());
        }
    }
    Ok(output)
}

pub(super) fn encrypted_account_payloads(
    root: Option<&Value>,
    accounts: &AccountStore,
) -> anyhow::Result<BTreeMap<(String, String), Value>> {
    if accounts.accounts.is_empty() {
        return Ok(BTreeMap::new());
    }
    let values = root
        .and_then(|root| root.get("accounts"))
        .and_then(Value::as_array)
        .context("accounts.json entries are missing")?;
    let mut output = BTreeMap::new();
    for payload in values {
        let provider_type = json_string(payload, &["providerType", "provider_type"])?;
        let id = json_string(payload, &["id"])?;
        ensure!(
            output
                .insert((provider_type, id), payload.clone())
                .is_none(),
            "duplicate Account source key"
        );
    }
    Ok(output)
}

pub(super) fn share_payloads(
    root: Option<&Value>,
    shares: &ShareStore,
) -> anyhow::Result<BTreeMap<String, Value>> {
    if shares.shares.is_empty() {
        return Ok(BTreeMap::new());
    }
    let values = root
        .and_then(|root| root.get("shares"))
        .and_then(Value::as_array)
        .context("shares.json entries are missing")?;
    let mut output = BTreeMap::new();
    for payload in values {
        let id = json_string(payload, &["id"])?;
        ensure!(
            output.insert(id, payload.clone()).is_none(),
            "duplicate Share source key"
        );
    }
    Ok(output)
}

pub(super) fn ensure_account_payload_secrets_are_encrypted(value: &Value) -> anyhow::Result<()> {
    fn visit(value: &Value, parent: Option<&str>) -> anyhow::Result<()> {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    let compact = key
                        .chars()
                        .filter(|character| character.is_ascii_alphanumeric())
                        .map(|character| character.to_ascii_lowercase())
                        .collect::<String>();
                    let secret = matches!(
                        compact.as_str(),
                        "token"
                            | "key"
                            | "secret"
                            | "authorization"
                            | "proxyauthorization"
                            | "cookie"
                            | "password"
                            | "sessiontoken"
                            | "githubtoken"
                            | "copilottoken"
                            | "devicecode"
                            | "usercode"
                            | "codeverifier"
                            | "authorizationcode"
                            | "clientassertion"
                            | "machinetoken"
                            | "securityoauthtoken"
                            | "personaltoken"
                    ) || [
                        "accesstoken",
                        "refreshtoken",
                        "idtoken",
                        "apikey",
                        "clientsecret",
                        "kiroapikey",
                        "secretaccesskey",
                        "privatekey",
                        "signingkey",
                    ]
                    .iter()
                    .any(|suffix| compact.ends_with(suffix))
                        || parent.is_some_and(|parent| {
                            parent
                                .chars()
                                .filter(|character| character.is_ascii_alphanumeric())
                                .map(|character| character.to_ascii_lowercase())
                                .collect::<String>()
                                == "extraheaders"
                        });
                    if secret {
                        if let Some(text) = value.as_str().filter(|text| !text.is_empty()) {
                            ensure!(
                                text.starts_with("ccenc:v1:") || text.starts_with("ccenc:v2:"),
                                "refusing to place plaintext Account credentials in SQLite"
                            );
                        }
                    }
                    visit(value, Some(key))?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    visit(value, parent)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    visit(value, None)
}

fn json_string(value: &Value, keys: &[&str]) -> anyhow::Result<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
        .context("required JSON identity field is missing")
}
