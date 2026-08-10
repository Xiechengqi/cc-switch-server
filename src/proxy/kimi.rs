use bytes::Bytes;
use serde_json::Value;

use crate::domain::accounts::store::AccountStore;
use crate::domain::kimi_cli::{device_identity_from_profile, KIMI_DEFAULT_MODEL};
use crate::domain::providers::model::ProviderType;
use crate::domain::providers::runtime::{ProviderRuntimePlan, RuntimeAuthRef, RuntimeModelPolicy};

use super::adapters::AdapterRequest;
use super::ProxyError;

pub const KIMI_MODEL_ALIASES: &[&str] = &[
    "kimi",
    "kimi-code",
    "kimi-for-coding",
    "haiku",
    "claude-haiku-4-5",
    "claude-haiku-4-5-20251001",
    "sonnet",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "opus",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "fable",
    "claude-fable-5",
    "kimi-k3",
    "k3",
];

pub(super) fn finalize_request(
    plan: &ProviderRuntimePlan,
    request: &mut AdapterRequest,
) -> Result<(), ProxyError> {
    let candidate = match &plan.model_policy {
        RuntimeModelPolicy::Single { upstream_model } => upstream_model.as_str(),
        RuntimeModelPolicy::Passthrough => request
            .actual_model
            .as_deref()
            .or(request.model.as_deref())
            .or(request.requested_model.as_deref())
            .ok_or_else(|| ProxyError::bad_request("Kimi request is missing model"))?,
    };
    let model = resolve_model(candidate)?;
    request.body = replace_model(&request.body, model)?;
    request.model = Some(model.to_string());
    request.actual_model = Some(model.to_string());
    request.actual_model_source = Some("kimi_model_allowlist".to_string());
    Ok(())
}

pub(super) fn finalize_account_identity(
    plan: &ProviderRuntimePlan,
    accounts: &AccountStore,
    headers: &mut Vec<(String, String)>,
) -> Result<(), ProxyError> {
    let (account_id, expected_generation) = match &plan.auth_ref {
        RuntimeAuthRef::ManagedAccount {
            account_id,
            expected_provider_type: ProviderType::KimiCode,
            auth_identity_generation,
        } => (account_id.as_str(), *auth_identity_generation),
        _ => {
            return Err(ProxyError::bad_request(
                "Kimi Provider must explicitly bind one kimi_code managed account",
            ))
        }
    };
    let account = accounts
        .accounts
        .iter()
        .find(|account| account.id == account_id && account.provider_type == ProviderType::KimiCode)
        .ok_or_else(|| ProxyError::bad_request("bound Kimi account does not exist"))?;
    if account.auth_identity_generation != expected_generation {
        return Err(ProxyError::conflict(
            "bound Kimi account identity changed; rebind the Provider",
        ));
    }
    let identity = device_identity_from_profile(account.profile.as_ref()).ok_or_else(|| {
        ProxyError::bad_request("bound Kimi account is missing its account-scoped device identity")
    })?;
    for (name, value) in identity.headers() {
        replace_header(headers, &name, value);
    }
    Ok(())
}

pub fn supported_models() -> Vec<String> {
    KIMI_MODEL_ALIASES
        .iter()
        .map(|model| (*model).to_string())
        .collect()
}

fn resolve_model(model: &str) -> Result<&'static str, ProxyError> {
    let model = model.trim().to_ascii_lowercase();
    match model.as_str() {
        "kimi-k3" | "k3" => return Ok("k3"),
        candidate if KIMI_MODEL_ALIASES.contains(&candidate) => return Ok(KIMI_DEFAULT_MODEL),
        _ => {}
    }
    Err(ProxyError::bad_request(format!(
        "unsupported Kimi Code model {model:?}; use kimi-for-coding or a registered alias"
    )))
}

fn replace_model(body: &Bytes, model: &str) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| ProxyError::bad_request(format!("invalid Kimi request JSON: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("Kimi request body must be an object"))?;
    object.insert("model".to_string(), Value::String(model.to_string()));
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode Kimi request: {error}")))
}

fn replace_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    headers.retain(|(candidate, _)| !candidate.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value));
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::domain::providers::model::AppKind;
    use crate::domain::providers::registry::{
        DriverId, OutboundIdentityPolicy, ProfileId, ProviderKey, UpstreamProtocol,
    };
    use crate::domain::providers::runtime::{RuntimeConfigurationState, RuntimeTransportPolicy};

    fn plan(policy: RuntimeModelPolicy) -> ProviderRuntimePlan {
        ProviderRuntimePlan {
            provider_key: ProviderKey::new(AppKind::Claude, "kimi").unwrap(),
            provider_revision: 1,
            profile_id: ProfileId::parse("claude.kimi_code").unwrap(),
            profile_schema_revision: 1,
            driver_id: DriverId::parse("oauth.kimi_code").unwrap(),
            driver_contract_revision: 1,
            endpoint: crate::domain::kimi_cli::KIMI_API_BASE_URL.to_string(),
            upstream_protocol: UpstreamProtocol::OpenAiChat,
            outbound_identity_policy: OutboundIdentityPolicy::ServerIdentity,
            auth_ref: RuntimeAuthRef::Missing,
            model_policy: policy,
            test_model: None,
            aws_region: None,
            media_policy: None,
            transport_policy: RuntimeTransportPolicy::default(),
            extra_headers: Vec::new(),
            driver_options: BTreeMap::new(),
            configuration_state: RuntimeConfigurationState::Ready,
            warnings: Vec::new(),
            runtime_fingerprint: "fixture".to_string(),
        }
    }

    #[test]
    fn passthrough_alias_is_normalized_and_unknown_model_fails_closed() {
        let mut request = AdapterRequest {
            body: Bytes::from_static(br#"{"model":"sonnet","messages":[]}"#),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("sonnet".to_string()),
            requested_model: Some("sonnet".to_string()),
            actual_model: Some("sonnet".to_string()),
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        finalize_request(&plan(RuntimeModelPolicy::Passthrough), &mut request).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap()["model"],
            json!(KIMI_DEFAULT_MODEL)
        );

        request.model = Some("unknown".to_string());
        request.actual_model = Some("unknown".to_string());
        assert!(finalize_request(&plan(RuntimeModelPolicy::Passthrough), &mut request).is_err());

        request.model = Some("kimi-k3".to_string());
        request.actual_model = Some("kimi-k3".to_string());
        finalize_request(&plan(RuntimeModelPolicy::Passthrough), &mut request).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&request.body).unwrap()["model"],
            json!("k3")
        );
    }
}
