//! Explicit opt-in, whole-answer tool echo recovery. No quote repair, prose
//! stripping, partial execution or inference from examples is permitted.
use super::{ProxyError, QoderOutputContract, MAX_QODER_CHAT_TOOL_CALLS};
use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{json, Map, Value};

const LIMIT: usize = 128 * 1024;
const MARKER: &str = "[assistant 请求调用工具]";

#[derive(Debug, Default)]
pub(super) struct TextTools {
    text: String,
    bypass: bool,
}

impl TextTools {
    pub(super) fn retained_bytes(&self) -> usize {
        self.text.capacity()
    }
    pub(super) fn projected_bytes(&self, value: &Value) -> usize {
        if self.bypass {
            return self.retained_bytes();
        }
        let projected = self.text.capacity().saturating_add(
            value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .map_or(0, str::len),
        );
        // Only a finish event parses the whole echo. Account conservatively for
        // tiny nested JSON collections, decoded arguments and output copies;
        // byte-size alone substantially undercounts their resident memory.
        if value
            .pointer("/choices/0/finish_reason")
            .is_some_and(|v| !v.is_null())
        {
            projected.saturating_mul(256).saturating_add(64 * 1024)
        } else {
            projected.saturating_mul(2).saturating_add(128)
        }
    }

    pub(super) fn observe(
        &mut self,
        value: &mut Value,
        contract: Option<&QoderOutputContract>,
    ) -> Result<(), ProxyError> {
        let Some(contract) = contract.filter(|contract| contract.recover_text_tools) else {
            return Ok(());
        };
        if self.bypass {
            return Ok(());
        }
        if value
            .pointer("/choices/0/delta/tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| !calls.is_empty())
        {
            if !self.text.trim().is_empty() {
                return Err(invalid());
            }
            self.bypass = true;
            return Ok(());
        }
        if let Some(text) = value
            .pointer("/choices/0/delta/content")
            .and_then(Value::as_str)
        {
            if self.text.len().saturating_add(text.len()) > LIMIT {
                return Err(invalid());
            }
            self.text.push_str(text);
            value["choices"][0]["delta"]
                .as_object_mut()
                .expect("canonical delta")
                .remove("content");
        }
        let trimmed = self.text.trim_start();
        let possible = [MARKER, "<tool_call>", "<|DSML|calls>", "<｜DSML｜calls>"]
            .iter()
            .any(|prefix| prefix.starts_with(trimmed) || trimmed.starts_with(prefix));
        let finish = value
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str);
        if !possible {
            value["choices"][0]["delta"]["content"] = Value::String(std::mem::take(&mut self.text));
            self.bypass = true;
        } else if let Some(finish) = finish {
            if trimmed.is_empty() {
                return Ok(());
            }
            if finish != "stop" {
                return Err(invalid());
            }
            let calls = parse(trimmed, contract)?;
            value["choices"][0]["delta"]["tool_calls"] = Value::Array(calls);
            value["choices"][0]["finish_reason"] = json!("tool_calls");
            self.text.clear();
            self.bypass = true;
        }
        Ok(())
    }
}

fn invalid() -> ProxyError {
    ProxyError::bad_gateway("Qoder text tool echo is incomplete, ambiguous or unauthorized")
}

// Value's ordinary deserializer is last-key-wins. Tool echoes are executable
// data, so duplicate keys (including nested argument keys) are ambiguous.
struct UniqueValue(Value);

impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueValue;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|value| UniqueValue(Value::Number(value)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(value.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueValue(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                    let UniqueValue(value) = map.next_value()?;
                    values.insert(key, value);
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

fn unique_json(text: &str) -> Result<Value, ProxyError> {
    serde_json::from_str::<UniqueValue>(text)
        .map(|value| value.0)
        .map_err(|_| invalid())
}

fn echo_object(value: Value) -> Result<(String, Value), ProxyError> {
    let object = value.as_object().ok_or_else(invalid)?;
    if object.len() != 2 {
        return Err(invalid());
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let args = match object.get("arguments") {
        Some(Value::String(args)) => unique_json(args)?,
        Some(args) => args.clone(),
        None => return Err(invalid()),
    };
    Ok((name.to_string(), args))
}

fn call(
    name: &str,
    arguments: Value,
    index: usize,
    contract: &QoderOutputContract,
) -> Result<Value, ProxyError> {
    if index >= MAX_QODER_CHAT_TOOL_CALLS
        || !contract.allowed_names.contains(name)
        || !arguments.is_object()
    {
        return Err(invalid());
    }
    let arguments = serde_json::to_string(&arguments).map_err(|_| invalid())?;
    Ok(
        json!({"index":index,"id":format!("call_qoder_echo_{}",crate::domain::qoder::random_qoder_uuid()),"type":"function","function":{"name":name,"arguments":arguments}}),
    )
}

fn parse(text: &str, contract: &QoderOutputContract) -> Result<Vec<Value>, ProxyError> {
    let mut rest = text.strip_prefix(MARKER).unwrap_or(text).trim();
    let mut calls = Vec::new();
    if rest.starts_with('[') {
        let values = unique_json(rest)?;
        for value in values.as_array().ok_or_else(invalid)? {
            let (name, args) = echo_object(value.clone())?;
            calls.push(call(&name, args, calls.len(), contract)?);
        }
    } else if rest.starts_with("<tool_call>") {
        while let Some(inner) = rest.strip_prefix("<tool_call>") {
            let (body, tail) = inner.split_once("</tool_call>").ok_or_else(invalid)?;
            let body = body.trim();
            let (name, args) = if body.starts_with('{') {
                echo_object(unique_json(body)?)?
            } else {
                let start = body.find('{').ok_or_else(invalid)?;
                let name = body[..start].trim();
                (name.to_string(), unique_json(&body[start..])?)
            };
            calls.push(call(&name, args, calls.len(), contract)?);
            rest = tail.trim();
        }
        if !rest.is_empty() {
            return Err(invalid());
        }
    } else {
        let token = if rest.starts_with("<|DSML|calls>") {
            "|DSML|"
        } else {
            "｜DSML｜"
        };
        rest = rest
            .strip_prefix(&format!("<{token}calls>"))
            .and_then(|s| s.strip_suffix(&format!("</{token}calls>")))
            .ok_or_else(invalid)?
            .trim();
        let invoke = format!("<{token}invoke name=\"");
        while let Some(inner) = rest.strip_prefix(&invoke) {
            let (name, inner) = inner.split_once("\">").ok_or_else(invalid)?;
            let (mut params, tail) = inner
                .split_once(&format!("</{token}invoke>"))
                .ok_or_else(invalid)?;
            let mut arguments = Map::new();
            let parameter = format!("<{token}parameter name=\"");
            params = params.trim();
            while let Some(inner) = params.strip_prefix(&parameter) {
                let (key, inner) = inner.split_once("\">").ok_or_else(invalid)?;
                let (value, tail) = inner
                    .split_once(&format!("</{token}parameter>"))
                    .ok_or_else(invalid)?;
                if key.is_empty()
                    || key.contains(['"', '<', '>'])
                    || arguments.insert(key.to_string(), json!(value)).is_some()
                {
                    return Err(invalid());
                }
                params = tail.trim();
            }
            if !params.is_empty() {
                return Err(invalid());
            }
            calls.push(call(name, Value::Object(arguments), calls.len(), contract)?);
            rest = tail.trim();
        }
        if !rest.is_empty() {
            return Err(invalid());
        }
    }
    if calls.is_empty() {
        return Err(invalid());
    }
    Ok(calls)
}
