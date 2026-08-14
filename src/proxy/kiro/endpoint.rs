use serde_json::Value;

use super::{next_uuid_like, KiroAccountData};
use crate::proxy::ProxyError;

pub(super) const FALLBACK_IDE_VERSION: &str = "0.9.2";
const FALLBACK_CLI_VERSION: &str = "1.19.0";
const SYSTEM_VERSION: &str = "macos";
const NODE_VERSION: &str = "22.22.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum EndpointKind {
    Ide,
    Cli,
}

impl EndpointKind {
    pub(super) fn from_account(account: &KiroAccountData) -> Self {
        match account.endpoint.as_deref().map(str::trim) {
            Some(value) if value.eq_ignore_ascii_case("cli") => Self::Cli,
            _ => Self::Ide,
        }
    }
}

#[derive(Debug)]
pub(super) struct PreparedEndpoint {
    pub(super) url: String,
    pub(super) host: String,
    pub(super) headers: Vec<(&'static str, String)>,
    pub(super) body: Value,
}

pub(super) fn prepare(
    kind: EndpointKind,
    account: &KiroAccountData,
    access_token: &str,
    body: Value,
    ide_version: &str,
) -> PreparedEndpoint {
    let region = if account.api_region.trim().is_empty() {
        "us-east-1"
    } else {
        account.api_region.trim()
    };
    let host = format!("q.{region}.amazonaws.com");
    let machine_id = account
        .machine_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown");
    let version = account
        .client_version
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(ide_version)
        .trim();
    let version = if version.is_empty() {
        FALLBACK_IDE_VERSION
    } else {
        version
    };
    let (url, mut headers, body) = match kind {
        EndpointKind::Ide => {
            let headers = vec![
                ("content-type", "application/json".to_string()),
                ("x-amzn-codewhisperer-optout", "true".to_string()),
                ("x-amzn-kiro-agent-mode", "vibe".to_string()),
                (
                    "x-amz-user-agent",
                    format!("aws-sdk-js/1.0.34 KiroIDE-{version}-{machine_id}"),
                ),
                (
                    "user-agent",
                    format!(
                        "aws-sdk-js/1.0.34 ua/2.1 os/{SYSTEM_VERSION} lang/js md/nodejs#{NODE_VERSION} api/codewhispererstreaming#1.0.34 m/E KiroIDE-{version}-{machine_id}"
                    ),
                ),
            ];
            (
                format!("https://{host}/generateAssistantResponse"),
                headers,
                body,
            )
        }
        EndpointKind::Cli => {
            let cli_version = account
                .cli_version
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(FALLBACK_CLI_VERSION);
            let headers = vec![
                ("content-type", "application/x-amz-json-1.0".to_string()),
                (
                    "x-amz-target",
                    "AmazonCodeWhispererStreamingService.GenerateAssistantResponse".to_string(),
                ),
                ("x-amzn-codewhisperer-optout", "false".to_string()),
                (
                    "x-amz-user-agent",
                    format!(
                        "aws-sdk-rust/1.3.15 ua/2.1 api/codewhispererstreaming/0.1.14474 os/{SYSTEM_VERSION} lang/rust/1.92.0 m/F app/AmazonQ-For-CLI"
                    ),
                ),
                (
                    "user-agent",
                    format!(
                        "aws-sdk-rust/1.3.15 ua/2.1 api/codewhispererstreaming/0.1.14474 os/{SYSTEM_VERSION} lang/rust/1.92.0 md/appVersion-{cli_version} app/AmazonQ-For-CLI"
                    ),
                ),
            ];
            (format!("https://{host}/"), headers, cli_body(body))
        }
    };

    headers.extend([
        ("connection", "close".to_string()),
        ("host", host.clone()),
        ("amz-sdk-invocation-id", next_uuid_like("kiro-invocation")),
        ("amz-sdk-request", "attempt=1; max=3".to_string()),
        ("authorization", format!("Bearer {access_token}")),
    ]);
    if let Some(token_type) = token_type_header(account) {
        headers.push(("tokentype", token_type.to_string()));
    }
    PreparedEndpoint {
        url,
        host,
        headers,
        body,
    }
}

fn token_type_header(account: &KiroAccountData) -> Option<&'static str> {
    let method = account.auth_method.as_deref()?.trim().to_ascii_lowercase();
    match method.as_str() {
        "api_key" | "api-key" | "apikey" => Some("API_KEY"),
        "external_idp" | "external-idp" | "externalidp" => Some("EXTERNAL_IDP"),
        _ => None,
    }
}

fn cli_body(mut body: Value) -> Value {
    replace_origin(&mut body);
    if let Some(state) = body
        .get_mut("conversationState")
        .and_then(Value::as_object_mut)
    {
        state.remove("agentContinuationId");
        if let Some(history) = state.get_mut("history").and_then(Value::as_array_mut) {
            for message in history {
                if let Some(user) = message
                    .get_mut("userInputMessage")
                    .and_then(Value::as_object_mut)
                {
                    user.remove("modelId");
                }
            }
        }
    }
    body
}

fn replace_origin(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                if key == "origin" && child.as_str() == Some("AI_EDITOR") {
                    *child = Value::String("KIRO_CLI".to_string());
                } else {
                    replace_origin(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(replace_origin),
        _ => {}
    }
}

pub(super) fn profile_arn(account: &KiroAccountData) -> Result<Option<String>, ProxyError> {
    if let Some(profile_arn) = account
        .profile_arn
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(Some(profile_arn.to_string()));
    }
    let auth_method = account
        .auth_method
        .as_deref()
        .or(account.provider.as_deref())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match auth_method.as_str() {
        "api_key" | "api-key" | "apikey" => Ok(None),
        "social" | "google" | "github" => Ok(Some(
            crate::domain::providers::kiro::SOCIAL_PROFILE_ARN.to_string(),
        )),
        "builder-id" | "builder_id" | "builderid" | "builder" | "" => Ok(Some(
            crate::domain::providers::kiro::BUILDER_ID_PROFILE_ARN.to_string(),
        )),
        _ => Err(ProxyError::bad_request(format!(
            "kiro {auth_method} account lacks a resolved profile ARN"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIRE_PROTOCOL_JSON: &str =
        include_str!("../../../assets/contract/kiro-wire-protocol.json");

    fn account(endpoint: &str) -> KiroAccountData {
        KiroAccountData {
            account_id: "account".to_string(),
            email: None,
            refresh_token: "refresh".to_string(),
            profile_arn: Some("arn:profile".to_string()),
            auth_region: "us-east-1".to_string(),
            api_region: "us-east-1".to_string(),
            machine_id: Some("machine".to_string()),
            client_id: None,
            client_secret: None,
            client_secret_expires_at: None,
            start_url: None,
            auth_method: Some("api_key".to_string()),
            provider: None,
            endpoint: Some(endpoint.to_string()),
            client_version: None,
            cli_version: None,
            authenticated_at: 0,
        }
    }

    #[test]
    fn cli_uses_rust_identity_and_cleans_complete_history() {
        let prepared = prepare(
            EndpointKind::Cli,
            &account("cli"),
            "token",
            serde_json::json!({
                "conversationState": {
                    "agentContinuationId": "remove",
                    "currentMessage": {"userInputMessage": {"origin": "AI_EDITOR", "modelId": "current"}},
                    "history": [
                        {"userInputMessage": {"origin": "AI_EDITOR", "modelId": "history"}}
                    ]
                }
            }),
            "9.9.9",
        );
        assert_eq!(prepared.url, "https://q.us-east-1.amazonaws.com/");
        assert!(prepared
            .headers
            .iter()
            .any(|(name, value)| { *name == "user-agent" && value.contains("aws-sdk-rust") }));
        assert!(!prepared
            .headers
            .iter()
            .any(|(name, _)| { *name == "x-amzn-kiro-agent-mode" }));
        assert!(prepared
            .body
            .pointer("/conversationState/agentContinuationId")
            .is_none());
        assert_eq!(
            prepared
                .body
                .pointer("/conversationState/currentMessage/userInputMessage/modelId"),
            Some(&serde_json::json!("current"))
        );
        assert!(prepared
            .body
            .pointer("/conversationState/history/0/userInputMessage/modelId")
            .is_none());
        assert_eq!(
            prepared
                .body
                .pointer("/conversationState/history/0/userInputMessage/origin"),
            Some(&serde_json::json!("KIRO_CLI"))
        );
    }

    #[test]
    fn endpoint_shapes_match_the_wire_protocol_fixture() {
        let fixture: Value = serde_json::from_str(WIRE_PROTOCOL_JSON).unwrap();
        let ide = prepare(
            EndpointKind::Ide,
            &account("ide"),
            "token",
            serde_json::json!({}),
            FALLBACK_IDE_VERSION,
        );
        let cli = prepare(
            EndpointKind::Cli,
            &account("cli"),
            "token",
            serde_json::json!({}),
            FALLBACK_IDE_VERSION,
        );

        assert_eq!(fixture["endpoints"]["defaultRegion"], "us-east-1");
        assert_eq!(fixture["endpoints"]["productionOverrideAllowed"], false);
        assert_eq!(
            ide.url,
            "https://q.us-east-1.amazonaws.com/generateAssistantResponse"
        );
        assert_eq!(cli.url, "https://q.us-east-1.amazonaws.com/");
        assert_eq!(
            fixture["endpoints"]["ide"]["contentType"],
            ide.headers
                .iter()
                .find(|(name, _)| *name == "content-type")
                .map(|(_, value)| value.as_str())
                .unwrap()
        );
        assert_eq!(
            fixture["endpoints"]["cli"]["xAmzTarget"],
            cli.headers
                .iter()
                .find(|(name, _)| *name == "x-amz-target")
                .map(|(_, value)| value.as_str())
                .unwrap()
        );
    }
}
