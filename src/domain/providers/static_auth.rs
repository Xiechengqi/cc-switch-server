//! One credential-placement contract for forwarding, diagnostics and sharing.
//! Contains no credentials and never probes an upstream or selects a fallback.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use serde::Serialize;
use serde_json::Value;

use super::registry::{AuthScheme, UpstreamProtocol};
use super::runtime::validate_custom_auth_header_name;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticAuthCarrier {
    pub location: &'static str,
    pub name: String,
    pub prefix: &'static str,
}

impl StaticAuthCarrier {
    /// Preserve the v1 credential-source framing and existing capacity pool ids.
    pub fn identity_tuple(&self) -> (&str, &str, &str) {
        (self.location, &self.name, self.prefix)
    }
}

pub fn static_auth_carrier(
    protocol: UpstreamProtocol,
    scheme: AuthScheme,
    options: &BTreeMap<String, Value>,
) -> anyhow::Result<Option<StaticAuthCarrier>> {
    let field = || {
        options
            .get("apiKeyField")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
    };
    let (location, name, prefix) = match scheme {
        AuthScheme::None => return Ok(None),
        AuthScheme::Bearer => ("header", "authorization".to_string(), "Bearer "),
        AuthScheme::ApiKey => (
            "header",
            if protocol == UpstreamProtocol::GeminiNative {
                "x-goog-api-key"
            } else {
                "x-api-key"
            }
            .to_string(),
            "",
        ),
        AuthScheme::CustomHeader => (
            "header",
            validate_custom_auth_header_name(
                field().context("custom_header auth requires apiKeyField")?,
            )?,
            "",
        ),
        AuthScheme::Query => ("query", field().unwrap_or("key").to_string(), ""),
        AuthScheme::OAuth | AuthScheme::AwsSigV4 => bail!(
            "static Provider cannot materialize {:?} authentication",
            scheme
        ),
    };
    Ok(Some(StaticAuthCarrier {
        location,
        name,
        prefix,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn carriers_are_explicit_and_preserve_identity_framing() {
        use AuthScheme::*;
        use UpstreamProtocol::*;
        for (protocol, scheme, expected) in [
            (OpenAiChat, Bearer, ("header", "authorization", "Bearer ")),
            (
                OpenAiResponses,
                Bearer,
                ("header", "authorization", "Bearer "),
            ),
            (AnthropicMessages, ApiKey, ("header", "x-api-key", "")),
            (OpenAiChat, ApiKey, ("header", "x-api-key", "")),
            (GeminiNative, ApiKey, ("header", "x-goog-api-key", "")),
            (GeminiNative, Query, ("query", "key", "")),
        ] {
            let carrier = static_auth_carrier(protocol, scheme, &BTreeMap::new())
                .unwrap()
                .unwrap();
            assert_eq!(carrier.identity_tuple(), expected);
            assert_eq!(
                serde_json::to_vec(&carrier.identity_tuple()).unwrap(),
                serde_json::to_vec(&expected).unwrap()
            );
        }
        let options = BTreeMap::from([("apiKeyField".into(), Value::from(" X-Custom-Key "))]);
        assert_eq!(
            static_auth_carrier(OpenAiChat, CustomHeader, &options)
                .unwrap()
                .unwrap()
                .name,
            "x-custom-key"
        );
        assert!(static_auth_carrier(OpenAiChat, CustomHeader, &BTreeMap::new()).is_err());
        let forbidden = BTreeMap::from([("apiKeyField".into(), Value::from("Host"))]);
        assert!(static_auth_carrier(OpenAiChat, CustomHeader, &forbidden).is_err());
        assert!(static_auth_carrier(OpenAiChat, OAuth, &BTreeMap::new()).is_err());
    }
}
