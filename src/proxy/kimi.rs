use bytes::Bytes;
use serde_json::{json, Map, Value};

use crate::domain::accounts::store::AccountStore;
use crate::domain::kimi_cli::{
    device_identity_from_profile, KIMI_DEFAULT_MODEL, KIMI_HIGHSPEED_MODEL,
};
use crate::domain::providers::model::{AppKind, ProviderType};
use crate::domain::providers::runtime::{ProviderRuntimePlan, RuntimeAuthRef, RuntimeModelPolicy};

use super::adapters::AdapterRequest;
use super::ProxyError;

pub const KIMI_MODEL_ALIASES: &[&str] = &[
    "kimi",
    "kimi-code",
    "kimi-for-coding",
    "kimi-for-coding-highspeed",
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
    request.body = normalize_request(&request.body, model, plan.provider_key.app)?;
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

pub fn catalog_models(wire_models: &[String]) -> Vec<String> {
    let has_default = wire_models.iter().any(|model| model == KIMI_DEFAULT_MODEL);
    let has_highspeed = wire_models
        .iter()
        .any(|model| model == KIMI_HIGHSPEED_MODEL);
    let has_k3 = wire_models.iter().any(|model| model == "k3");
    KIMI_MODEL_ALIASES
        .iter()
        .copied()
        .filter(|alias| match *alias {
            "kimi-k3" | "k3" => has_k3,
            KIMI_HIGHSPEED_MODEL => has_highspeed,
            _ => has_default,
        })
        .map(str::to_string)
        .collect()
}

fn resolve_model(model: &str) -> Result<&'static str, ProxyError> {
    let model = model.trim().to_ascii_lowercase();
    match model.as_str() {
        "kimi-k3" | "k3" => return Ok("k3"),
        KIMI_HIGHSPEED_MODEL => return Ok(KIMI_HIGHSPEED_MODEL),
        candidate if KIMI_MODEL_ALIASES.contains(&candidate) => return Ok(KIMI_DEFAULT_MODEL),
        _ => {}
    }
    Err(ProxyError::bad_request(format!(
        "unsupported Kimi Code model {model:?}; use kimi-for-coding or a registered alias"
    )))
}

fn normalize_request(body: &Bytes, model: &str, app: AppKind) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| ProxyError::bad_request(format!("invalid Kimi request JSON: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| ProxyError::bad_request("Kimi request body must be an object"))?;
    object.insert("model".to_string(), Value::String(model.to_string()));
    match app {
        AppKind::Claude => normalize_anthropic_request(object, model)?,
        AppKind::Codex | AppKind::Gemini => normalize_chat_request(object, model)?,
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode Kimi request: {error}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestedThinking {
    Unspecified,
    Disabled,
    Enabled(Option<String>),
}

fn requested_thinking(body: &Map<String, Value>) -> RequestedThinking {
    for value in [
        body.get("reasoning_effort"),
        body.get("reasoning").and_then(|value| value.get("effort")),
        body.get("thinking").and_then(|value| value.get("effort")),
        body.get("output_config")
            .and_then(|value| value.get("effort")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(effort) = normalized_effort(value) {
            return effort;
        }
    }

    let thinking = body.get("thinking").and_then(Value::as_object);
    if thinking
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("disabled"))
    {
        return RequestedThinking::Disabled;
    }
    if let Some(budget) = thinking
        .and_then(|value| value.get("budget_tokens"))
        .and_then(Value::as_u64)
    {
        return if budget == 0 {
            RequestedThinking::Disabled
        } else {
            RequestedThinking::Enabled(Some(budget_effort(budget).to_string()))
        };
    }
    if thinking
        .and_then(|value| value.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "enabled" | "adaptive" | "auto"
            )
        })
    {
        return RequestedThinking::Enabled(None);
    }
    RequestedThinking::Unspecified
}

fn normalized_effort(value: &Value) -> Option<RequestedThinking> {
    let effort = value.as_str()?.trim().to_ascii_lowercase();
    if effort.is_empty() {
        return None;
    }
    Some(match effort.as_str() {
        "none" | "off" | "disabled" => RequestedThinking::Disabled,
        "on" | "auto" | "enabled" => RequestedThinking::Enabled(None),
        _ => RequestedThinking::Enabled(Some(effort)),
    })
}

fn budget_effort(budget: u64) -> &'static str {
    match budget {
        0..=1_024 => "low",
        1_025..=24_576 => "high",
        _ => "max",
    }
}

fn canonical_effort(model: &str, effort: Option<&str>) -> Result<Option<String>, ProxyError> {
    if model != "k3" {
        return Ok(effort.map(str::to_string));
    }
    let effort = effort.unwrap_or("max").trim().to_ascii_lowercase();
    let canonical = match effort.as_str() {
        "minimal" | "low" => "low",
        "medium" | "high" => "high",
        "xhigh" | "max" => "max",
        _ => {
            return Err(ProxyError::bad_request(format!(
                "unsupported Kimi K3 reasoning effort {effort:?}; use low, high, or max"
            )))
        }
    };
    Ok(Some(canonical.to_string()))
}

fn normalize_chat_request(body: &mut Map<String, Value>, model: &str) -> Result<(), ProxyError> {
    if !body.contains_key("max_completion_tokens") {
        if let Some(max_tokens) = body.get("max_tokens").cloned() {
            body.insert("max_completion_tokens".to_string(), max_tokens);
        }
    }
    body.remove("max_tokens");

    let requested = requested_thinking(body);
    body.remove("reasoning_effort");
    body.remove("reasoning");
    body.remove("output_config");
    match requested {
        RequestedThinking::Disabled => {
            let thinking = object_field(body, "thinking");
            thinking.insert("type".to_string(), json!("disabled"));
            thinking.remove("effort");
        }
        RequestedThinking::Enabled(effort) => {
            let effort = canonical_effort(model, effort.as_deref())?;
            let thinking = object_field(body, "thinking");
            thinking.insert("type".to_string(), json!("enabled"));
            if let Some(effort) = effort {
                thinking.insert("effort".to_string(), Value::String(effort));
            } else {
                thinking.remove("effort");
            }
            thinking.insert("keep".to_string(), json!("all"));
            backfill_chat_reasoning(body);
        }
        RequestedThinking::Unspecified if model == "k3" => {
            let thinking = object_field(body, "thinking");
            thinking.insert("type".to_string(), json!("enabled"));
            thinking.insert("effort".to_string(), json!("max"));
            thinking.insert("keep".to_string(), json!("all"));
            backfill_chat_reasoning(body);
        }
        RequestedThinking::Unspecified => {
            if body
                .get("thinking")
                .and_then(Value::as_object)
                .and_then(|thinking| thinking.get("type"))
                .and_then(Value::as_str)
                .is_some_and(|kind| !kind.eq_ignore_ascii_case("disabled"))
            {
                object_field(body, "thinking").insert("keep".to_string(), json!("all"));
                backfill_chat_reasoning(body);
            }
        }
    }
    Ok(())
}

fn normalize_anthropic_request(
    body: &mut Map<String, Value>,
    model: &str,
) -> Result<(), ProxyError> {
    let requested = requested_thinking(body);
    body.remove("reasoning_effort");
    body.remove("reasoning");
    match requested {
        RequestedThinking::Disabled => {
            let thinking = object_field(body, "thinking");
            thinking.insert("type".to_string(), json!("disabled"));
            thinking.remove("effort");
            thinking.remove("budget_tokens");
            body.remove("output_config");
            remove_clear_thinking_edit(body);
        }
        RequestedThinking::Enabled(effort) => {
            let effort = canonical_effort(model, effort.as_deref())?;
            let thinking = object_field(body, "thinking");
            thinking.insert("type".to_string(), json!("enabled"));
            thinking.remove("effort");
            thinking.remove("budget_tokens");
            match effort {
                Some(effort) => {
                    object_field(body, "output_config")
                        .insert("effort".to_string(), Value::String(effort));
                }
                None => {
                    body.remove("output_config");
                }
            }
            add_clear_thinking_edit(body);
        }
        RequestedThinking::Unspecified if model == "k3" => {
            object_field(body, "thinking").insert("type".to_string(), json!("enabled"));
            object_field(body, "output_config").insert("effort".to_string(), json!("max"));
            add_clear_thinking_edit(body);
        }
        RequestedThinking::Unspecified => {
            let kind = body
                .get("thinking")
                .and_then(Value::as_object)
                .and_then(|thinking| thinking.get("type"))
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase);
            match kind.as_deref() {
                Some("adaptive" | "auto") => {
                    object_field(body, "thinking").insert("type".to_string(), json!("enabled"));
                    add_clear_thinking_edit(body);
                }
                Some("enabled") => add_clear_thinking_edit(body),
                Some("disabled") => remove_clear_thinking_edit(body),
                _ => {}
            }
        }
    }
    Ok(())
}

fn object_field<'a>(body: &'a mut Map<String, Value>, field: &str) -> &'a mut Map<String, Value> {
    if !body.get(field).is_some_and(Value::is_object) {
        body.insert(field.to_string(), Value::Object(Map::new()));
    }
    body.get_mut(field)
        .and_then(Value::as_object_mut)
        .expect("object field was initialized")
}

fn add_clear_thinking_edit(body: &mut Map<String, Value>) {
    let context = object_field(body, "context_management");
    let mut edits = context
        .remove("edits")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    edits
        .retain(|edit| edit.get("type").and_then(Value::as_str) != Some("clear_thinking_20251015"));
    edits.insert(0, json!({"type": "clear_thinking_20251015", "keep": "all"}));
    context.insert("edits".to_string(), Value::Array(edits));
}

fn remove_clear_thinking_edit(body: &mut Map<String, Value>) {
    let Some(context) = body
        .get_mut("context_management")
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let Some(edits) = context.get_mut("edits").and_then(Value::as_array_mut) else {
        return;
    };
    edits
        .retain(|edit| edit.get("type").and_then(Value::as_str) != Some("clear_thinking_20251015"));
}

fn backfill_chat_reasoning(body: &mut Map<String, Value>) {
    let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    let mut latest_reasoning = None::<String>;
    for message in messages {
        let Some(message) = message.as_object_mut() else {
            continue;
        };
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }
        if let Some(reasoning) = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            latest_reasoning = Some(reasoning.to_string());
        }
        let has_tool_calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty());
        let missing_reasoning = message
            .get("reasoning_content")
            .and_then(Value::as_str)
            .is_none_or(|value| value.trim().is_empty());
        if !has_tool_calls || !missing_reasoning {
            continue;
        }
        let fallback = latest_reasoning
            .clone()
            .or_else(|| visible_message_text(message))
            .unwrap_or_else(|| "[reasoning unavailable]".to_string());
        message.insert("reasoning_content".to_string(), Value::String(fallback));
    }
}

fn visible_message_text(message: &Map<String, Value>) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => non_empty(text),
        Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            non_empty(&text)
        }
        _ => None,
    }
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
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

    fn plan_for(app: AppKind, policy: RuntimeModelPolicy) -> ProviderRuntimePlan {
        ProviderRuntimePlan {
            provider_key: ProviderKey::new(app, "kimi").unwrap(),
            provider_revision: 1,
            profile_id: ProfileId::parse("claude.kimi_code").unwrap(),
            profile_schema_revision: 1,
            driver_id: DriverId::parse("oauth.kimi_code").unwrap(),
            driver_contract_revision: 1,
            endpoint: crate::domain::kimi_cli::KIMI_API_BASE_URL.to_string(),
            upstream_protocol: UpstreamProtocol::Special,
            outbound_identity_policy: OutboundIdentityPolicy::ServerIdentity,
            auth_ref: RuntimeAuthRef::Missing,
            model_policy: policy,
            coding_plan: None,
            test_model: None,
            probe_policy_fingerprint: "fixture".to_string(),
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

    fn plan(policy: RuntimeModelPolicy) -> ProviderRuntimePlan {
        plan_for(AppKind::Codex, policy)
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

    #[test]
    fn k3_chat_normalizes_effort_tokens_and_tool_history() {
        let mut request = AdapterRequest {
            body: Bytes::from_static(
                br#"{
                    "model":"k3",
                    "max_tokens":4096,
                    "reasoning":{"effort":"xhigh","summary":"keep"},
                    "thinking":{"vendor":"keep"},
                    "messages":[
                        {"role":"assistant","content":"first","reasoning_content":"signed prior","tool_calls":[{"id":"one"}]},
                        {"role":"assistant","content":"second","tool_calls":[{"id":"two"}]}
                    ]
                }"#,
            ),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("k3".to_string()),
            requested_model: Some("k3".to_string()),
            actual_model: Some("k3".to_string()),
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };

        finalize_request(&plan(RuntimeModelPolicy::Passthrough), &mut request).unwrap();
        let body = serde_json::from_slice::<Value>(&request.body).unwrap();
        assert_eq!(body["max_completion_tokens"], json!(4096));
        assert!(body.get("max_tokens").is_none());
        assert!(body.get("reasoning").is_none());
        assert_eq!(body["thinking"]["type"], json!("enabled"));
        assert_eq!(body["thinking"]["effort"], json!("max"));
        assert_eq!(body["thinking"]["keep"], json!("all"));
        assert_eq!(body["thinking"]["vendor"], json!("keep"));
        assert_eq!(
            body["messages"][0]["reasoning_content"],
            json!("signed prior")
        );
        assert_eq!(
            body["messages"][1]["reasoning_content"],
            json!("signed prior")
        );
    }

    #[test]
    fn k3_defaults_to_max_and_rejects_unknown_effort() {
        let mut request = AdapterRequest {
            body: Bytes::from_static(br#"{"model":"k3","messages":[]}"#),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("k3".to_string()),
            requested_model: Some("k3".to_string()),
            actual_model: Some("k3".to_string()),
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        finalize_request(&plan(RuntimeModelPolicy::Passthrough), &mut request).unwrap();
        let body = serde_json::from_slice::<Value>(&request.body).unwrap();
        assert_eq!(body["thinking"]["effort"], json!("max"));

        request.body =
            Bytes::from_static(br#"{"model":"k3","reasoning_effort":"ultra","messages":[]}"#);
        request.model = Some("k3".to_string());
        request.actual_model = Some("k3".to_string());
        let error =
            finalize_request(&plan(RuntimeModelPolicy::Passthrough), &mut request).unwrap_err();
        assert_eq!(error.status, reqwest::StatusCode::BAD_REQUEST);
        assert!(error.message.contains("low, high, or max"));
    }

    #[test]
    fn anthropic_thinking_preserves_caller_context_and_disabling_removes_only_owned_edit() {
        let mut request = AdapterRequest {
            body: Bytes::from_static(
                br#"{
                    "model":"k3",
                    "thinking":{"type":"enabled","budget_tokens":30000,"vendor":"keep"},
                    "output_config":{"vendor":"keep"},
                    "context_management":{"vendor":"keep","edits":[
                        {"type":"caller_edit","value":1},
                        {"type":"clear_thinking_20251015","keep":"last"}
                    ]},
                    "messages":[]
                }"#,
            ),
            upstream_endpoint: None,
            upstream_headers: Vec::new(),
            model: Some("k3".to_string()),
            requested_model: Some("k3".to_string()),
            actual_model: Some("k3".to_string()),
            actual_model_source: None,
            gemini_action: None,
            stream_requested: false,
            upstream_stream_requested: false,
            custom_tool_names: Default::default(),
            responses_tool_context: Default::default(),
            claude_tool_name_map: Default::default(),
        };
        let claude_plan = plan_for(AppKind::Claude, RuntimeModelPolicy::Passthrough);
        finalize_request(&claude_plan, &mut request).unwrap();
        let body = serde_json::from_slice::<Value>(&request.body).unwrap();
        assert_eq!(body["thinking"], json!({"type":"enabled","vendor":"keep"}));
        assert_eq!(
            body["output_config"],
            json!({"vendor":"keep","effort":"max"})
        );
        assert_eq!(body["context_management"]["vendor"], json!("keep"));
        assert_eq!(
            body["context_management"]["edits"],
            json!([
                {"type":"clear_thinking_20251015","keep":"all"},
                {"type":"caller_edit","value":1}
            ])
        );

        request.body = Bytes::from_static(
            br#"{
                "model":"k3",
                "thinking":{"type":"disabled"},
                "context_management":{"vendor":"keep","edits":[
                    {"type":"caller_edit"},
                    {"type":"clear_thinking_20251015","keep":"all"}
                ]},
                "messages":[]
            }"#,
        );
        finalize_request(&claude_plan, &mut request).unwrap();
        let body = serde_json::from_slice::<Value>(&request.body).unwrap();
        assert_eq!(body["thinking"], json!({"type":"disabled"}));
        assert_eq!(body["context_management"]["vendor"], json!("keep"));
        assert_eq!(
            body["context_management"]["edits"],
            json!([{"type":"caller_edit"}])
        );
    }
}
