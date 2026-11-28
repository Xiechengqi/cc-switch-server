//! Kimi-owned source controls, parsed before generic format bridges can lose
//! type information or invent a budget. No recursive business-tree inspection.
use super::{object_field, Map, ProxyError, RequestedThinking, Value};
use crate::domain::providers::model::AppKind;

pub(in crate::proxy) struct SourceControls {
    pub(super) thinking: RequestedThinking,
    output_limit: Option<u64>,
    k3_aliases: bool,
}

impl SourceControls {
    pub(in crate::proxy) fn from_request(app: AppKind, input: &Value) -> Result<Self, ProxyError> {
        Self::from_object(
            app,
            input
                .as_object()
                .ok_or_else(|| ProxyError::bad_request("Kimi source body must be an object"))?,
        )
    }

    pub(super) fn from_object(
        app: AppKind,
        input: &Map<String, Value>,
    ) -> Result<Self, ProxyError> {
        let mut controls = Self {
            thinking: RequestedThinking::Unspecified,
            output_limit: None,
            k3_aliases: input
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|model| {
                    let model = model.trim();
                    model.eq_ignore_ascii_case("k3") || model.eq_ignore_ascii_case("kimi-k3")
                }),
        };
        if let Some(effort) = input.get("reasoning_effort") {
            controls.merge(effort_control(effort)?)?;
        }
        for field in ["reasoning", "thinking", "output_config"] {
            let Some(value) = input.get(field) else {
                continue;
            };
            let object = value.as_object().ok_or_else(|| {
                ProxyError::bad_request(format!("Kimi {field} must be an object"))
            })?;
            if let Some(effort) = object.get("effort") {
                controls.merge(effort_control(effort)?)?;
            }
            if field == "thinking" {
                if let Some(kind) = object.get("type") {
                    let kind = kind.as_str().map(str::trim).ok_or_else(|| {
                        ProxyError::bad_request("Kimi thinking.type must be a string")
                    })?;
                    let requested = match kind.to_ascii_lowercase().as_str() {
                        "disabled" => RequestedThinking::Disabled,
                        "enabled" | "adaptive" | "auto" => RequestedThinking::Enabled(None),
                        _ => return Err(ProxyError::bad_request("unsupported Kimi thinking.type")),
                    };
                    controls.merge(requested)?;
                }
                if let Some(budget) = object.get("budget_tokens") {
                    controls.budget(budget, false)?;
                }
            }
        }
        for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
            if let Some(limit) = input.get(field) {
                controls.limit(limit)?;
            }
        }
        if app == AppKind::Gemini {
            for field in ["generationConfig", "generation_config"] {
                let Some(config) = input.get(field) else {
                    continue;
                };
                let config = config.as_object().ok_or_else(|| {
                    ProxyError::bad_request("Kimi Gemini generationConfig must be an object")
                })?;
                for field in ["maxOutputTokens", "max_output_tokens"] {
                    if let Some(limit) = config.get(field) {
                        controls.limit(limit)?;
                    }
                }
                for field in ["thinkingConfig", "thinking_config"] {
                    let Some(thinking) = config.get(field) else {
                        continue;
                    };
                    let thinking = thinking.as_object().ok_or_else(|| {
                        ProxyError::bad_request("Kimi Gemini thinkingConfig must be an object")
                    })?;
                    for field in ["thinkingLevel", "thinking_level"] {
                        if let Some(level) = thinking.get(field) {
                            controls.merge(effort_control(level)?)?;
                        }
                    }
                    for field in ["thinkingBudget", "thinking_budget"] {
                        if let Some(budget) = thinking.get(field) {
                            controls.budget(budget, true)?;
                        }
                    }
                }
            }
        }
        Ok(controls)
    }

    fn budget(&mut self, budget: &Value, dynamic: bool) -> Result<(), ProxyError> {
        let budget = budget
            .as_i64()
            .ok_or_else(|| ProxyError::bad_request("Kimi thinking budget must be an integer"))?;
        match budget {
            0 => self.merge(RequestedThinking::Disabled),
            -1 if dynamic => self.merge(RequestedThinking::Enabled(None)),
            1.. => Err(ProxyError::bad_request(
                "Kimi does not support an exact thinking token budget; select an effort level",
            )),
            _ => Err(ProxyError::bad_request("unsupported Kimi thinking budget")),
        }
    }

    fn limit(&mut self, limit: &Value) -> Result<(), ProxyError> {
        let limit = limit.as_u64().filter(|limit| *limit > 0).ok_or_else(|| {
            ProxyError::bad_request("Kimi output token limit must be a positive integer")
        })?;
        if self.output_limit.is_some_and(|old| old != limit) {
            return Err(ProxyError::bad_request(
                "conflicting Kimi output token limit aliases",
            ));
        }
        self.output_limit = Some(limit);
        Ok(())
    }

    fn merge(&mut self, requested: RequestedThinking) -> Result<(), ProxyError> {
        use RequestedThinking::*;
        match (&mut self.thinking, requested) {
            (Unspecified, next) => self.thinking = next,
            (Disabled, Disabled) | (Enabled(_), Enabled(None)) => {}
            (Enabled(old @ None), Enabled(Some(next))) => *old = Some(next),
            (Enabled(Some(old)), Enabled(Some(next)))
                if *old == next || self.k3_aliases && effort_class(old) == effort_class(&next) => {}
            _ => {
                return Err(ProxyError::bad_request(
                    "conflicting Kimi thinking controls",
                ))
            }
        }
        Ok(())
    }

    pub(super) fn diagnostic_effort(&self) -> Option<&str> {
        match &self.thinking {
            RequestedThinking::Unspecified => None,
            RequestedThinking::Disabled => Some("none"),
            RequestedThinking::Enabled(None) => Some("enabled"),
            RequestedThinking::Enabled(Some(effort)) => Some(effort),
        }
    }

    pub(in crate::proxy) fn apply(&self, output: &mut Value, app: AppKind) {
        let object = output
            .as_object_mut()
            .expect("format bridge produces object");
        // Only control envelopes are touched. Schema/default/tool values remain opaque.
        object.remove("reasoning_effort");
        object.remove("reasoning");
        if let Some(config) = object
            .get_mut("output_config")
            .and_then(Value::as_object_mut)
        {
            config.remove("effort");
        }
        if let Some(thinking) = object.get_mut("thinking").and_then(Value::as_object_mut) {
            for field in ["type", "effort", "budget_tokens"] {
                thinking.remove(field);
            }
        }
        match &self.thinking {
            RequestedThinking::Unspecified => {}
            RequestedThinking::Disabled => {
                object_field(object, "thinking")
                    .insert("type".into(), Value::String("disabled".into()));
            }
            RequestedThinking::Enabled(effort) => {
                let thinking = object_field(object, "thinking");
                thinking.insert("type".into(), Value::String("enabled".into()));
                if let Some(effort) = effort {
                    thinking.insert("effort".into(), Value::String(effort.clone()));
                }
            }
        }
        if let Some(limit) = self.output_limit {
            for field in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
                object.remove(field);
            }
            object.insert(
                if app == AppKind::Claude {
                    "max_tokens"
                } else {
                    "max_completion_tokens"
                }
                .into(),
                Value::from(limit),
            );
        }
    }
}

fn effort_class(effort: &str) -> u8 {
    match effort {
        "minimal" | "low" => 1,
        "medium" | "high" => 2,
        "xhigh" | "max" => 3,
        _ => 0,
    }
}

fn effort_control(value: &Value) -> Result<RequestedThinking, ProxyError> {
    let effort = value
        .as_str()
        .map(str::trim)
        .ok_or_else(|| ProxyError::bad_request("Kimi effort must be a string"))?
        .to_ascii_lowercase();
    match effort.as_str() {
        "none" | "off" | "disabled" => Ok(RequestedThinking::Disabled),
        "on" | "auto" | "enabled" => Ok(RequestedThinking::Enabled(None)),
        "minimal" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Ok(RequestedThinking::Enabled(Some(effort)))
        }
        _ => Err(ProxyError::bad_request(
            "unsupported Kimi effort; use low, high, or max",
        )),
    }
}

// Optional diagnostics skip message/tool/history trees instead of allocating
// a second complete request or response document. Full validation still runs
// on the original source inside the format adapter.
pub(super) fn diagnostic_fields(body: &[u8]) -> Option<Map<String, Value>> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        model: Option<Value>,
        reasoning_effort: Option<Value>,
        reasoning: Option<Value>,
        thinking: Option<Value>,
        output_config: Option<Value>,
        max_tokens: Option<Value>,
        max_completion_tokens: Option<Value>,
        max_output_tokens: Option<Value>,
        #[serde(rename = "generationConfig")]
        gemini_config: Option<Value>,
        generation_config: Option<Value>,
    }
    let fields: Envelope = serde_json::from_slice(body).ok()?;
    Some(
        [
            ("model", fields.model),
            ("reasoning_effort", fields.reasoning_effort),
            ("reasoning", fields.reasoning),
            ("thinking", fields.thinking),
            ("output_config", fields.output_config),
            ("max_tokens", fields.max_tokens),
            ("max_completion_tokens", fields.max_completion_tokens),
            ("max_output_tokens", fields.max_output_tokens),
            ("generationConfig", fields.gemini_config),
            ("generation_config", fields.generation_config),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| (key.into(), value)))
        .collect(),
    )
}
