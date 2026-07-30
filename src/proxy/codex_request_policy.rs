use bytes::Bytes;
use serde_json::Value;

use crate::domain::providers::store::StoredProvider;

use super::codex_models::{self, CapabilitySupport};
use super::ProxyError;

const PRIORITY_SERVICE_TIER: &str = "priority";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CodexRequestIntent {
    pub requested_reasoning_effort: Option<String>,
    pub client_service_tier: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CodexRequestPolicyMetadata {
    pub requested_reasoning_effort: Option<String>,
    pub effective_reasoning_effort: Option<String>,
    pub client_service_tier: Option<String>,
    pub effective_service_tier: Option<String>,
    pub service_tier_decision: Option<String>,
}

pub(crate) fn extract_intent_from_bytes(body: &[u8]) -> CodexRequestIntent {
    serde_json::from_slice::<Value>(body)
        .ok()
        .as_ref()
        .map(extract_intent)
        .unwrap_or_default()
}

pub(crate) fn extract_intent(body: &Value) -> CodexRequestIntent {
    CodexRequestIntent {
        requested_reasoning_effort: reasoning_effort(body),
        client_service_tier: service_tier(body),
    }
}

pub(crate) fn apply_to_bytes(
    body: &Bytes,
    provider: &StoredProvider,
    final_model: Option<&str>,
    fast_mode_enabled: bool,
    intent: &CodexRequestIntent,
) -> Result<(Bytes, CodexRequestPolicyMetadata), ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("invalid Codex OAuth request JSON: {error}"))
    })?;
    let metadata = apply(&mut value, provider, final_model, fast_mode_enabled, intent)?;
    let body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode Codex OAuth request: {error}")))?;
    Ok((body, metadata))
}

pub(crate) fn apply(
    body: &mut Value,
    provider: &StoredProvider,
    final_model: Option<&str>,
    fast_mode_enabled: bool,
    intent: &CodexRequestIntent,
) -> Result<CodexRequestPolicyMetadata, ProxyError> {
    if !body.is_object() {
        return Err(ProxyError::bad_request(
            "Codex OAuth request body must be an object",
        ));
    }

    let transformed_effort = reasoning_effort(body);
    let object = body
        .as_object_mut()
        .expect("Codex OAuth request body was validated as an object");
    let requested_reasoning_effort = intent.requested_reasoning_effort.clone();
    let effective_reasoning_effort = transformed_effort
        .or_else(|| requested_reasoning_effort.clone())
        .map(|effort| {
            codex_models::normalize_reasoning_effort(final_model.unwrap_or_default(), &effort)
        });

    object.remove("reasoning_effort");
    if let Some(effort) = effective_reasoning_effort.as_ref() {
        if !object.get("reasoning").is_some_and(Value::is_object) {
            object.insert(
                "reasoning".to_string(),
                Value::Object(serde_json::Map::new()),
            );
        }
        if let Some(reasoning) = object.get_mut("reasoning").and_then(Value::as_object_mut) {
            reasoning.insert("effort".to_string(), Value::String(effort.clone()));
        }
    }

    object.remove("service_tier");
    object.remove("serviceTier");
    let (effective_service_tier, service_tier_decision) = if !fast_mode_enabled {
        (None, "server_disabled")
    } else if let Some(model) = final_model.filter(|model| !model.trim().is_empty()) {
        match codex_models::service_tier_support(provider, model, PRIORITY_SERVICE_TIER) {
            CapabilitySupport::Supported => {
                object.insert(
                    "service_tier".to_string(),
                    Value::String(PRIORITY_SERVICE_TIER.to_string()),
                );
                (
                    Some(PRIORITY_SERVICE_TIER.to_string()),
                    "server_forced_priority",
                )
            }
            CapabilitySupport::Unsupported => (None, "model_unsupported"),
            CapabilitySupport::Unknown => (None, "capability_unknown"),
        }
    } else {
        (None, "capability_unknown")
    };

    Ok(CodexRequestPolicyMetadata {
        requested_reasoning_effort,
        effective_reasoning_effort,
        client_service_tier: intent.client_service_tier.clone(),
        effective_service_tier,
        service_tier_decision: Some(service_tier_decision.to_string()),
    })
}

fn normalized_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn reasoning_effort(body: &Value) -> Option<String> {
    normalized_string(body.pointer("/reasoning/effort"))
        .or_else(|| normalized_string(body.get("reasoning_effort")))
        .or_else(|| normalized_string(body.pointer("/output_config/effort")))
        .or_else(|| normalized_string(body.pointer("/thinking/effort")))
        .or_else(|| {
            normalized_string(body.pointer("/generationConfig/thinkingConfig/thinkingLevel"))
        })
}

fn service_tier(body: &Value) -> Option<String> {
    normalized_string(body.get("service_tier"))
        .or_else(|| normalized_string(body.get("serviceTier")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::providers::model::{AppKind, Provider, ProviderMeta, ProviderType};
    use crate::domain::providers::store::{ProviderResourceMetadata, StoredProvider};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn provider() -> StoredProvider {
        StoredProvider {
            app: AppKind::Codex,
            provider: Provider {
                id: "codex".to_string(),
                name: "Codex".to_string(),
                settings_config: json!({}),
                category: None,
                meta: Some(ProviderMeta::default()),
                extra: BTreeMap::new(),
            },
            provider_type: ProviderType::CodexOAuth,
            provider_type_id: ProviderType::CodexOAuth.as_str().to_string(),
            resource: ProviderResourceMetadata::default(),
        }
    }

    #[test]
    fn server_disabled_removes_every_client_tier() {
        for tier in ["priority", "fast", "default", "flex", "scale"] {
            let mut body = json!({
                "model": "gpt-5.6-sol",
                "service_tier": tier,
                "reasoning": {"effort": "high"}
            });
            let intent = extract_intent(&body);
            let metadata =
                apply(&mut body, &provider(), Some("gpt-5.6-sol"), false, &intent).unwrap();

            assert!(body.get("service_tier").is_none());
            assert_eq!(metadata.client_service_tier.as_deref(), Some(tier));
            assert_eq!(metadata.effective_service_tier, None);
            assert_eq!(
                metadata.service_tier_decision.as_deref(),
                Some("server_disabled")
            );
        }
    }

    #[test]
    fn server_enabled_forces_priority_for_supported_model() {
        let mut body = json!({"model": "gpt-5.4", "service_tier": "default"});
        let intent = extract_intent(&body);
        let metadata = apply(&mut body, &provider(), Some("gpt-5.4"), true, &intent).unwrap();

        assert_eq!(body["service_tier"], "priority");
        assert_eq!(metadata.client_service_tier.as_deref(), Some("default"));
        assert_eq!(metadata.effective_service_tier.as_deref(), Some("priority"));
        assert_eq!(
            metadata.service_tier_decision.as_deref(),
            Some("server_forced_priority")
        );
    }

    #[test]
    fn server_enabled_omits_priority_for_unsupported_and_unknown_models() {
        for (model, decision) in [
            ("gpt-5.4-mini", "model_unsupported"),
            ("future-model", "capability_unknown"),
        ] {
            let mut body = json!({"model": model, "service_tier": "priority"});
            let intent = extract_intent(&body);
            let metadata = apply(&mut body, &provider(), Some(model), true, &intent).unwrap();

            assert!(body.get("service_tier").is_none());
            assert_eq!(metadata.service_tier_decision.as_deref(), Some(decision));
        }
    }

    #[test]
    fn reasoning_prefers_nested_and_preserves_wire_values() {
        let mut body = json!({
            "model": "gpt-5.5",
            "reasoning": {"effort": "MAX"},
            "reasoning_effort": "low"
        });
        let intent = extract_intent(&body);
        let metadata = apply(&mut body, &provider(), Some("gpt-5.5"), false, &intent).unwrap();

        assert_eq!(metadata.requested_reasoning_effort.as_deref(), Some("max"));
        assert_eq!(metadata.effective_reasoning_effort.as_deref(), Some("max"));
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("max")));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn invalid_primary_alias_falls_back_to_valid_secondary_alias() {
        let mut body = json!({
            "model": "gpt-5.5",
            "reasoning": {"effort": null},
            "reasoning_effort": "high",
            "service_tier": false,
            "serviceTier": "default"
        });
        let intent = extract_intent(&body);
        let metadata = apply(&mut body, &provider(), Some("gpt-5.5"), false, &intent).unwrap();

        assert_eq!(metadata.requested_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(metadata.effective_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(metadata.client_service_tier.as_deref(), Some("default"));
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("high")));
        assert!(body.get("service_tier").is_none());
        assert!(body.get("serviceTier").is_none());
    }

    #[test]
    fn direct_ultra_is_recorded_and_sent_as_max() {
        let mut body = json!({"model": "gpt-5.6-sol", "reasoning": {"effort": "ultra"}});
        let intent = extract_intent(&body);
        let metadata = apply(&mut body, &provider(), Some("gpt-5.6-sol"), false, &intent).unwrap();

        assert_eq!(
            metadata.requested_reasoning_effort.as_deref(),
            Some("ultra")
        );
        assert_eq!(metadata.effective_reasoning_effort.as_deref(), Some("max"));
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("max")));
    }

    #[test]
    fn translated_protocol_intent_preserves_explicit_effort() {
        for (body, expected) in [
            (json!({"output_config": {"effort": "MAX"}}), "max"),
            (json!({"thinking": {"effort": "high"}}), "high"),
            (
                json!({
                    "generationConfig": {
                        "thinkingConfig": {"thinkingLevel": "LOW"}
                    }
                }),
                "low",
            ),
        ] {
            assert_eq!(
                extract_intent(&body).requested_reasoning_effort.as_deref(),
                Some(expected)
            );
        }
    }
}
