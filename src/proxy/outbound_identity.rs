use crate::domain::providers::registry::{
    ManagedIdentityFamily, ManagedVersionFamily, OutboundIdentityPolicy,
};
use crate::domain::providers::runtime::ProviderRuntimePlan;

use super::ProxyError;

pub(super) fn finalize_headers(
    plan: &ProviderRuntimePlan,
    headers: &mut Vec<(String, String)>,
) -> Result<(), ProxyError> {
    match plan.outbound_identity_policy {
        OutboundIdentityPolicy::ServerIdentity => {
            set_user_agent(headers, crate::provider_identity::server_user_agent());
        }
        OutboundIdentityPolicy::Omit => remove_header(headers, "user-agent"),
        OutboundIdentityPolicy::CustomOverride => {
            let user_agent = plan
                .driver_options
                .get("customUserAgent")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(crate::provider_identity::server_user_agent);
            crate::domain::providers::runtime::validate_custom_user_agent(&user_agent)
                .map_err(|error| ProxyError::bad_request(error.to_string()))?;
            set_user_agent(headers, user_agent);
        }
        OutboundIdentityPolicy::ManagedIdentity { family } => {
            finalize_managed_identity(family, headers);
        }
        OutboundIdentityPolicy::ManagedVersion {
            family: ManagedVersionFamily::Antigravity,
        } => {
            set_user_agent(headers, crate::provider_identity::antigravity_user_agent());
            replace_header(
                headers,
                "client-metadata",
                crate::provider_identity::antigravity_client_metadata().to_string(),
            );
        }
        OutboundIdentityPolicy::LegacyFrozen => {}
    }
    Ok(())
}

fn finalize_managed_identity(family: ManagedIdentityFamily, headers: &mut Vec<(String, String)>) {
    match family {
        ManagedIdentityFamily::ClaudeCode => {
            set_user_agent(headers, crate::domain::claude_cli::claude_cli_user_agent());
        }
        ManagedIdentityFamily::CodexCli => {
            crate::codex_identity::finalize_owned_headers(headers);
        }
        ManagedIdentityFamily::GrokCli => {
            set_user_agent(
                headers,
                crate::domain::grok_cli::GROK_CLI_USER_AGENT.to_string(),
            );
        }
        ManagedIdentityFamily::Kiro => {
            set_user_agent(headers, "aws-sdk-js/1.0.34 KiroIDE-2.3.0".to_string())
        }
        ManagedIdentityFamily::Cursor => set_user_agent(headers, "connect-es/1.6.1".to_string()),
        ManagedIdentityFamily::Copilot => set_user_agent(
            headers,
            crate::clients::oauth::copilot_device::COPILOT_USER_AGENT.to_string(),
        ),
        ManagedIdentityFamily::Deepseek => {
            set_user_agent(headers, "DeepSeek/2.0.4 Android/35".to_string())
        }
    }
}

fn set_user_agent(headers: &mut Vec<(String, String)>, value: String) {
    replace_header(headers, "user-agent", value);
}

fn remove_header(headers: &mut Vec<(String, String)>, name: &str) {
    headers.retain(|(candidate, _)| !candidate.eq_ignore_ascii_case(name));
}

fn replace_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    remove_header(headers, name);
    headers.push((name.to_string(), value));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::providers::model::AppKind;
    use crate::domain::providers::registry::{DriverId, ProfileId, ProviderKey, UpstreamProtocol};
    use crate::domain::providers::runtime::{
        RuntimeAuthRef, RuntimeConfigurationState, RuntimeModelPolicy, RuntimeTransportPolicy,
    };

    fn plan(policy: OutboundIdentityPolicy) -> ProviderRuntimePlan {
        ProviderRuntimePlan {
            provider_key: ProviderKey::new(AppKind::Codex, "provider").unwrap(),
            provider_revision: 1,
            profile_id: ProfileId::parse("codex.custom_http").unwrap(),
            profile_schema_revision: 1,
            driver_id: DriverId::parse("http.openai_responses").unwrap(),
            driver_contract_revision: 2,
            endpoint: "https://example.test".to_string(),
            upstream_protocol: UpstreamProtocol::OpenAiResponses,
            outbound_identity_policy: policy,
            auth_ref: RuntimeAuthRef::Missing,
            model_policy: RuntimeModelPolicy::Passthrough,
            media_policy: None,
            transport_policy: RuntimeTransportPolicy::default(),
            extra_headers: Vec::new(),
            driver_options: BTreeMap::new(),
            configuration_state: RuntimeConfigurationState::Ready,
            warnings: Vec::new(),
            runtime_fingerprint: "fixture".to_string(),
        }
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    #[test]
    fn custom_override_wins_and_empty_custom_uses_server_identity() {
        let mut custom = plan(OutboundIdentityPolicy::CustomOverride);
        custom
            .driver_options
            .insert("customUserAgent".to_string(), json!("custom-agent/1"));
        let mut headers = vec![("User-Agent".to_string(), "incoming/1".to_string())];
        finalize_headers(&custom, &mut headers).unwrap();
        assert_eq!(header(&headers, "user-agent"), Some("custom-agent/1"));

        let mut headers = Vec::new();
        finalize_headers(&plan(OutboundIdentityPolicy::CustomOverride), &mut headers).unwrap();
        assert_eq!(
            header(&headers, "user-agent"),
            Some(crate::provider_identity::server_user_agent().as_str())
        );
    }

    #[test]
    fn server_and_omit_policies_replace_or_remove_user_agent() {
        let mut headers = vec![("user-agent".to_string(), "incoming/1".to_string())];
        finalize_headers(&plan(OutboundIdentityPolicy::ServerIdentity), &mut headers).unwrap();
        assert_eq!(
            header(&headers, "user-agent"),
            Some(crate::provider_identity::server_user_agent().as_str())
        );

        finalize_headers(&plan(OutboundIdentityPolicy::Omit), &mut headers).unwrap();
        assert_eq!(header(&headers, "user-agent"), None);
    }

    #[test]
    fn managed_codex_repairs_the_full_identity_tuple() {
        let mut headers = vec![
            ("user-agent".to_string(), "curl/8".to_string()),
            ("originator".to_string(), "attacker".to_string()),
            ("version".to_string(), "0.1.0".to_string()),
        ];
        finalize_headers(
            &plan(OutboundIdentityPolicy::ManagedIdentity {
                family: ManagedIdentityFamily::CodexCli,
            }),
            &mut headers,
        )
        .unwrap();
        assert_eq!(header(&headers, "originator"), Some("codex_cli_rs"));
        assert!(header(&headers, "user-agent")
            .is_some_and(|value| { value.starts_with("codex_cli_rs/") }));
    }

    #[test]
    fn fixed_managed_identities_replace_untrusted_user_agents() {
        let cases = [
            (
                ManagedIdentityFamily::ClaudeCode,
                crate::domain::claude_cli::claude_cli_user_agent(),
            ),
            (
                ManagedIdentityFamily::GrokCli,
                crate::domain::grok_cli::GROK_CLI_USER_AGENT.to_string(),
            ),
            (
                ManagedIdentityFamily::Kiro,
                "aws-sdk-js/1.0.34 KiroIDE-2.3.0".to_string(),
            ),
            (
                ManagedIdentityFamily::Cursor,
                "connect-es/1.6.1".to_string(),
            ),
            (
                ManagedIdentityFamily::Copilot,
                crate::clients::oauth::copilot_device::COPILOT_USER_AGENT.to_string(),
            ),
            (
                ManagedIdentityFamily::Deepseek,
                "DeepSeek/2.0.4 Android/35".to_string(),
            ),
        ];

        for (family, expected) in cases {
            let mut headers = vec![
                ("User-Agent".to_string(), "untrusted/1".to_string()),
                ("user-agent".to_string(), "duplicate/2".to_string()),
            ];
            finalize_headers(
                &plan(OutboundIdentityPolicy::ManagedIdentity { family }),
                &mut headers,
            )
            .unwrap();
            assert_eq!(header(&headers, "user-agent"), Some(expected.as_str()));
            assert_eq!(
                headers
                    .iter()
                    .filter(|(name, _)| name.eq_ignore_ascii_case("user-agent"))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn antigravity_identity_replaces_the_full_managed_tuple() {
        let mut headers = vec![
            ("User-Agent".to_string(), "untrusted/1".to_string()),
            ("client-metadata".to_string(), "untrusted".to_string()),
        ];
        finalize_headers(
            &plan(OutboundIdentityPolicy::ManagedVersion {
                family: ManagedVersionFamily::Antigravity,
            }),
            &mut headers,
        )
        .unwrap();

        assert_eq!(
            header(&headers, "user-agent"),
            Some(crate::provider_identity::antigravity_user_agent().as_str())
        );
        assert_eq!(
            header(&headers, "client-metadata"),
            Some(
                crate::provider_identity::antigravity_client_metadata()
                    .to_string()
                    .as_str()
            )
        );
    }

    #[test]
    fn legacy_identity_leaves_existing_headers_untouched() {
        let mut headers = vec![("User-Agent".to_string(), "legacy/1".to_string())];
        finalize_headers(&plan(OutboundIdentityPolicy::LegacyFrozen), &mut headers).unwrap();
        assert_eq!(
            headers,
            vec![("User-Agent".to_string(), "legacy/1".to_string())]
        );
    }
}
