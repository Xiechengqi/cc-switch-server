use bytes::Bytes;
use serde_json::Value;

use super::ProxyError;

pub(crate) const ROUTING_HINT_HEADER: &str = "x-codex-routing-hint";
const ROUTING_HINT_ENV: &str = "CC_SWITCH_CODEX_ROUTING_HINT_ENABLED";

pub(crate) fn finalize_body(body: &Bytes) -> Result<Bytes, ProxyError> {
    let mut value = serde_json::from_slice::<Value>(body)
        .map_err(|error| ProxyError::bad_request(format!("invalid Codex HTTP body: {error}")))?;
    if value.get("type").and_then(Value::as_str) == Some("response.create") {
        value
            .as_object_mut()
            .expect("checked object field")
            .remove("type");
    }
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| ProxyError::bad_request(format!("encode Codex HTTP body: {error}")))
}

pub(crate) fn routing_hint(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let model = value.get("model")?.as_str()?.trim();
    if model.is_empty()
        || model.len() > 128
        || model
            .chars()
            .any(|character| character == ';' || character == '=' || character.is_control())
    {
        return None;
    }
    let mut hint = model.to_string();
    if value
        .get("service_tier")
        .and_then(Value::as_str)
        .is_some_and(|tier| tier.eq_ignore_ascii_case("priority"))
    {
        hint.push_str(";tier=priority");
    }
    Some(hint)
}

pub(crate) fn routing_hint_enabled(
    driver_options: &std::collections::BTreeMap<String, Value>,
) -> bool {
    let environment_value = std::env::var(ROUTING_HINT_ENV).ok();
    routing_hint_enabled_from(driver_options, environment_value.as_deref())
}

fn routing_hint_enabled_from(
    driver_options: &std::collections::BTreeMap<String, Value>,
    environment_value: Option<&str>,
) -> bool {
    driver_options
        .get("codexRoutingHintEnabled")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            environment_value.is_some_and(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
        })
}

pub(crate) fn finalize_routing_hint(
    headers: &mut Vec<(String, String)>,
    body: &[u8],
    enabled: bool,
) {
    headers.retain(|(name, _)| !name.eq_ignore_ascii_case(ROUTING_HINT_HEADER));
    if !enabled {
        crate::metrics::record_codex_routing_hint("disabled");
        return;
    }
    if let Some(hint) = routing_hint(body) {
        headers.push((ROUTING_HINT_HEADER.to_string(), hint));
        crate::metrics::record_codex_routing_hint("emitted");
    } else {
        crate::metrics::record_codex_routing_hint("invalid_model");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strips_only_the_http_response_create_envelope() {
        let body = Bytes::from_static(
            br#"{"type":"response.create","input":[{"type":"message","content":[{"type":"input_text"}]}]}"#,
        );
        let value: Value = serde_json::from_slice(&finalize_body(&body).unwrap()).unwrap();
        assert!(value.get("type").is_none());
        assert_eq!(value.pointer("/input/0/type"), Some(&json!("message")));
        assert_eq!(
            value.pointer("/input/0/content/0/type"),
            Some(&json!("input_text"))
        );

        let other = Bytes::from_static(br#"{"type":"other"}"#);
        assert_eq!(finalize_body(&other).unwrap(), other);
    }

    #[test]
    fn routing_hint_uses_final_model_and_only_verified_priority_tier() {
        assert_eq!(
            routing_hint(br#"{"model":"gpt-5.5","service_tier":"priority"}"#).as_deref(),
            Some("gpt-5.5;tier=priority")
        );
        assert_eq!(
            routing_hint(br#"{"model":"gpt-5.5","service_tier":"flex"}"#).as_deref(),
            Some("gpt-5.5")
        );
        assert!(routing_hint(br#"{"model":"bad;model"}"#).is_none());
    }

    #[test]
    fn routing_hint_is_server_owned_and_disabled_by_default_contract() {
        let mut headers = vec![
            (ROUTING_HINT_HEADER.to_string(), "client-forged".to_string()),
            ("x-safe".to_string(), "keep".to_string()),
        ];
        finalize_routing_hint(&mut headers, br#"{"model":"gpt-5.5"}"#, false);
        assert_eq!(headers, vec![("x-safe".to_string(), "keep".to_string())]);

        finalize_routing_hint(
            &mut headers,
            br#"{"model":"gpt-5.5","service_tier":"priority"}"#,
            true,
        );
        assert!(headers.iter().any(|(name, value)| {
            name == ROUTING_HINT_HEADER && value == "gpt-5.5;tier=priority"
        }));
    }

    #[test]
    fn routing_hint_provider_option_overrides_environment_default() {
        let mut options = std::collections::BTreeMap::new();
        assert!(!routing_hint_enabled_from(&options, None));
        assert!(routing_hint_enabled_from(&options, Some("yes")));

        options.insert("codexRoutingHintEnabled".to_string(), json!(false));
        assert!(!routing_hint_enabled_from(&options, Some("true")));

        options.insert("codexRoutingHintEnabled".to_string(), json!(true));
        assert!(routing_hint_enabled_from(&options, Some("false")));
    }
}
