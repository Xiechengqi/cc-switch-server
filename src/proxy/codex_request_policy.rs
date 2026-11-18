use bytes::Bytes;
use serde_json::Value;

use crate::domain::providers::model::CodexServiceTier;
use crate::domain::providers::store::StoredProvider;

use super::codex_models::{self, CapabilitySupport};
use super::ProxyError;

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
        requested_reasoning_effort: last_configuration_effort(body)
            .or_else(|| reasoning_effort(body)),
        client_service_tier: service_tier(body),
    }
}

pub(crate) fn apply_to_bytes(
    body: &Bytes,
    provider: &StoredProvider,
    final_model: Option<&str>,
    service_tier: impl Into<CodexServiceTier>,
    intent: &CodexRequestIntent,
) -> Result<(Bytes, CodexRequestPolicyMetadata), ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body).map_err(|error| {
        ProxyError::bad_request(format!("invalid Codex OAuth request JSON: {error}"))
    })?;
    let metadata = apply(&mut value, provider, final_model, service_tier, intent)?;
    let body = serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode Codex OAuth request: {error}")))?;
    Ok((body, metadata))
}

pub(crate) fn apply(
    body: &mut Value,
    provider: &StoredProvider,
    final_model: Option<&str>,
    service_tier: impl Into<CodexServiceTier>,
    intent: &CodexRequestIntent,
) -> Result<CodexRequestPolicyMetadata, ProxyError> {
    if !body.is_object() {
        return Err(ProxyError::bad_request(
            "Codex OAuth request body must be an object",
        ));
    }

    normalize_output_budget(body);
    validate_output_budget(body)?;
    let model = final_model.unwrap_or_default();
    let capability = codex_models::resolved_capability_for_model(provider, model);
    let transformed_effort = reasoning_effort(body);
    let has_updates = body
        .get("input")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("configuration_update")
            })
        });
    if has_updates
        && capability
            .as_ref()
            .and_then(|c| c.supports_reasoning_effort_updates)
            == Some(false)
    {
        return Err(ProxyError::bad_request(
            "selected Codex model does not support configuration_update",
        ));
    }
    let normalize_effort = |effort: String| -> Result<String, ProxyError> {
        let normalized = codex_models::normalize_reasoning_effort(model, &effort);
        // New native models advertise an explicit effort/update contract. Legacy
        // snapshots keep their existing wire behavior until an account manifest
        // advertises this capability.
        if let Some(c) = capability
            .as_ref()
            .filter(|c| c.supports_reasoning_effort_updates == Some(true))
        {
            if c.reasoning_efforts
                .as_ref()
                .is_some_and(|levels| !levels.contains(&normalized))
            {
                return Err(ProxyError::bad_request(format!(
                    "reasoning effort '{normalized}' is unsupported by model '{model}'"
                )));
            }
        }
        Ok(normalized)
    };
    let requested_reasoning_effort = intent.requested_reasoning_effort.clone();
    let initial_effort = transformed_effort
        .or_else(|| {
            (!has_updates)
                .then(|| requested_reasoning_effort.clone())
                .flatten()
        })
        .map(&normalize_effort)
        .transpose()?;
    let mut effective_reasoning_effort = initial_effort.clone();
    let object = body.as_object_mut().expect("validated object");
    object.remove("reasoning_effort");
    if let Some(effort) = initial_effort {
        if !object.get("reasoning").is_some_and(Value::is_object) {
            object.insert("reasoning".to_string(), serde_json::json!({}));
        }
        object["reasoning"]["effort"] = Value::String(effort);
    }
    if let Some(items) = object.get_mut("input").and_then(Value::as_array_mut) {
        for item in items {
            if item.get("type").and_then(Value::as_str) != Some("configuration_update") {
                continue;
            }
            // Service tier belongs to the administrator for the whole
            // generation, including native mid-history configuration items.
            let update = item.as_object_mut().expect("configuration update object");
            update.remove("service_tier");
            update.remove("serviceTier");
            if let Some(effort) = reasoning_effort(item) {
                let effort = normalize_effort(effort)?;
                if !item.get("reasoning").is_some_and(Value::is_object) {
                    item["reasoning"] = serde_json::json!({});
                }
                item["reasoning"]["effort"] = Value::String(effort.clone());
                item.as_object_mut()
                    .expect("update object")
                    .remove("reasoning_effort");
                effective_reasoning_effort = Some(effort);
            }
        }
    }

    object.remove("service_tier");
    object.remove("serviceTier");
    let tier = service_tier.into();
    let (effective_service_tier, service_tier_decision) = if tier == CodexServiceTier::Standard {
        (None, "server_disabled")
    } else {
        match codex_models::service_tier_support(provider, model, tier.as_str()) {
            CapabilitySupport::Supported => {
                object.insert(
                    "service_tier".to_string(),
                    Value::String(tier.as_str().to_string()),
                );
                (
                    Some(tier.as_str().to_string()),
                    if tier == CodexServiceTier::Ultrafast {
                        "server_forced_ultrafast"
                    } else {
                        "server_forced_priority"
                    },
                )
            }
            support if tier == CodexServiceTier::Ultrafast => {
                return Err(ProxyError::bad_request(format!(
                    "Codex ultrafast capability is {support:?} for model '{model}'"
                )));
            }
            CapabilitySupport::Unsupported => (None, "model_unsupported"),
            _ => (None, "capability_unknown"),
        }
    };

    Ok(CodexRequestPolicyMetadata {
        requested_reasoning_effort,
        effective_reasoning_effort,
        client_service_tier: intent.client_service_tier.clone(),
        effective_service_tier,
        service_tier_decision: Some(service_tier_decision.to_string()),
    })
}

/// Preserve an explicit Responses budget; otherwise translate the legacy alias
/// with the same precedence as Chat Completions. The upstream minimum is 16.
pub(crate) fn normalize_output_budget(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let budget = object
        .get("max_output_tokens")
        .or_else(|| object.get("max_completion_tokens"))
        .or_else(|| object.get("max_tokens"))
        .cloned();
    if let Some(mut budget) = budget {
        if let Some(value) = budget.as_u64().filter(|value| *value > 0) {
            budget = Value::from(value.max(16));
        }
        object.insert("max_output_tokens".to_string(), budget);
    }
    object.remove("max_tokens");
    object.remove("max_completion_tokens");
}

pub(crate) fn validate_output_budget(body: &Value) -> Result<(), ProxyError> {
    if body
        .get("max_output_tokens")
        .is_some_and(|value| value.as_u64().is_none_or(|value| value < 16))
    {
        return Err(ProxyError::bad_request(
            "max_output_tokens must be a positive integer (minimum 16)",
        ));
    }
    Ok(())
}

fn last_configuration_effort(body: &Value) -> Option<String> {
    body.get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("configuration_update"))
        .filter_map(reasoning_effort)
        .next_back()
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
    fn native_reasoning_updates_keep_the_timeline_and_report_the_last_effort() {
        let mut body = serde_json::json!({
            "model": "gpt-6.2-sol", "reasoning": {"effort": "high", "summary": "auto"},
            "input": [
                {"role": "user", "content": "first"},
                {"type": "configuration_update", "reasoning": {"effort": "LOW", "summary": "detailed"}, "tools": [], "service_tier": "ultrafast"},
                {"role": "user", "content": "second"},
                {"type": "configuration_update", "reasoning": {"effort": "ultra"}}
            ]
        });
        let original = body.clone();
        let intent = extract_intent(&body);
        let metadata = apply(&mut body, &provider(), Some("gpt-6.2-sol"), false, &intent).unwrap();
        assert_eq!(
            metadata.requested_reasoning_effort.as_deref(),
            Some("ultra")
        );
        assert_eq!(metadata.effective_reasoning_effort.as_deref(), Some("max"));
        assert_eq!(body["reasoning"], original["reasoning"]);
        assert_eq!(body["input"][0], original["input"][0]);
        assert_eq!(body["input"][1]["reasoning"]["effort"], "low");
        assert!(body["input"][1].get("service_tier").is_none());
        assert_eq!(body["input"][1]["reasoning"]["summary"], "detailed");
        assert_eq!(body["input"][1]["tools"], original["input"][1]["tools"]);
        assert_eq!(body["input"][2], original["input"][2]);
        assert_eq!(body["input"][3]["reasoning"]["effort"], "max");
    }

    #[test]
    fn updates_without_initial_effort_do_not_apply_to_earlier_messages() {
        let mut body = serde_json::json!({"input": [
            {"role": "user", "content": "first"},
            {"type": "configuration_update", "reasoning": {"effort": "high"}}
        ]});
        let intent = extract_intent(&body);
        let metadata = apply(
            &mut body,
            &provider(),
            Some("gpt-6.2-astra"),
            false,
            &intent,
        )
        .unwrap();
        assert!(body.get("reasoning").is_none());
        assert_eq!(metadata.effective_reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn new_models_reject_unsupported_effort_and_ultrafast_never_silently_downgrades() {
        let mut unsupported = serde_json::json!({"reasoning": {"effort": "none"}});
        assert!(apply(
            &mut unsupported,
            &provider(),
            Some("gpt-6.2-sol"),
            false,
            &CodexRequestIntent::default()
        )
        .is_err());
        for model in ["gpt-6.2-astra", "unknown-model"] {
            assert!(apply(
                &mut serde_json::json!({}),
                &provider(),
                Some(model),
                CodexServiceTier::Ultrafast,
                &CodexRequestIntent::default()
            )
            .is_err());
        }
        let mut body = serde_json::json!({"service_tier": "priority"});
        let intent = extract_intent(&body);
        let metadata = apply(
            &mut body,
            &provider(),
            Some("gpt-6.2-sol"),
            CodexServiceTier::Ultrafast,
            &intent,
        )
        .unwrap();
        assert_eq!(body["service_tier"], "ultrafast");
        assert_eq!(
            metadata.effective_service_tier.as_deref(),
            Some("ultrafast")
        );
    }

    #[test]
    fn budgets_preserve_precedence_enforce_minimum_and_reject_invalid_values() {
        for (mut body, expected) in [
            (serde_json::json!({"max_tokens": 1}), 16),
            (
                serde_json::json!({"max_tokens": 4096, "max_completion_tokens": 128}),
                128,
            ),
            (
                serde_json::json!({"max_tokens": 4096, "max_output_tokens": 1024}),
                1024,
            ),
        ] {
            apply(
                &mut body,
                &provider(),
                Some("gpt-6.2-sol"),
                false,
                &CodexRequestIntent::default(),
            )
            .unwrap();
            assert_eq!(body["max_output_tokens"], expected);
            assert!(body.get("max_tokens").is_none());
            assert!(body.get("max_completion_tokens").is_none());
        }
        for value in [
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!("128"),
            Value::Null,
        ] {
            assert!(apply(
                &mut serde_json::json!({"max_output_tokens": value}),
                &provider(),
                None,
                false,
                &CodexRequestIntent::default()
            )
            .is_err());
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
