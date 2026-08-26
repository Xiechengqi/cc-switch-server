use serde::Serialize;
use serde_json::Value;

pub const GROK_RESPONSES_INPUT_ITEM_TYPES: &[&str] = &[
    "message",
    "reasoning",
    "function_call",
    "function_call_output",
    "shell_call",
    "shell_call_output",
    "web_search_call",
    "file_search_call",
    "code_interpreter_call",
    "mcp_call",
    "custom_tool_call",
    "image_generation_call",
    "compaction",
];

pub const GROK_RESPONSES_TOP_LEVEL_TOOL_TYPES: &[&str] = &[
    "code_execution",
    "code_interpreter",
    "collections_search",
    "file_search",
    "function",
    "mcp",
    "shell",
    "web_search",
    "x_search",
];

const GROK_RESPONSES_ENDPOINT: &str = "/v1/responses";
const GROK_RESPONSES_PROTOCOL: &str = "grok_responses";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdditionalToolsPolicy {
    LosslessTopLevelMergeOrReject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownInputItemPolicy {
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsesProtocolCapability {
    pub endpoint: &'static str,
    pub provider_protocol: &'static str,
    pub accepted_input_item_types: &'static [&'static str],
    pub adapted_input_item_types: &'static [&'static str],
    pub accepted_top_level_tool_types: &'static [&'static str],
    pub additional_tools_policy: AdditionalToolsPolicy,
    pub unknown_input_item_policy: UnknownInputItemPolicy,
}

pub const fn grok_responses_capability() -> ResponsesProtocolCapability {
    ResponsesProtocolCapability {
        endpoint: GROK_RESPONSES_ENDPOINT,
        provider_protocol: GROK_RESPONSES_PROTOCOL,
        accepted_input_item_types: GROK_RESPONSES_INPUT_ITEM_TYPES,
        adapted_input_item_types: &["additional_tools"],
        accepted_top_level_tool_types: GROK_RESPONSES_TOP_LEVEL_TOOL_TYPES,
        additional_tools_policy: AdditionalToolsPolicy::LosslessTopLevelMergeOrReject,
        unknown_input_item_policy: UnknownInputItemPolicy::Reject,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformFidelity {
    Lossless,
    DeclaredLossy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformActionKind {
    Preserve,
    Map,
    Drop,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformAction {
    pub action: TransformActionKind,
    pub input_index: usize,
    pub item_type: &'static str,
    pub tool_count: usize,
    pub merged_tool_count: usize,
    pub deduplicated_tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformPlan {
    pub endpoint: &'static str,
    pub provider_protocol: &'static str,
    pub fidelity: TransformFidelity,
    pub preserved_input_items: usize,
    pub actions: Vec<TransformAction>,
}

impl TransformPlan {
    pub fn unchanged() -> Self {
        Self {
            endpoint: GROK_RESPONSES_ENDPOINT,
            provider_protocol: GROK_RESPONSES_PROTOCOL,
            fidelity: TransformFidelity::Lossless,
            preserved_input_items: 0,
            actions: Vec::new(),
        }
    }

    pub fn mapped_input_items(&self) -> usize {
        self.actions
            .iter()
            .filter(|action| action.action == TransformActionKind::Map)
            .count()
    }

    pub fn merged_tools(&self) -> usize {
        self.actions
            .iter()
            .map(|action| action.merged_tool_count)
            .sum()
    }

    pub fn deduplicated_tools(&self) -> usize {
        self.actions
            .iter()
            .map(|action| action.deduplicated_tool_count)
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolIncompatibilityReason {
    UnsupportedInputItem,
    InvalidAdditionalToolsItem,
    InvalidAdditionalTool,
    UnsupportedAdditionalTool,
    ConflictingToolDefinition,
    InvalidTopLevelTools,
}

impl ProtocolIncompatibilityReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedInputItem => "unsupported_input_item",
            Self::InvalidAdditionalToolsItem => "invalid_additional_tools_item",
            Self::InvalidAdditionalTool => "invalid_additional_tool",
            Self::UnsupportedAdditionalTool => "unsupported_additional_tool",
            Self::ConflictingToolDefinition => "conflicting_tool_definition",
            Self::InvalidTopLevelTools => "invalid_top_level_tools",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolCompatibilityError {
    pub reason: ProtocolIncompatibilityReason,
    pub input_index: usize,
    pub tool_index: Option<usize>,
}

impl ProtocolCompatibilityError {
    fn new(
        reason: ProtocolIncompatibilityReason,
        input_index: usize,
        tool_index: Option<usize>,
    ) -> Self {
        Self {
            reason,
            input_index,
            tool_index,
        }
    }

    pub fn client_message(self) -> String {
        match (self.reason, self.tool_index) {
            (ProtocolIncompatibilityReason::UnsupportedInputItem, _) => format!(
                "Grok Responses input[{}] uses an unsupported item type",
                self.input_index
            ),
            (ProtocolIncompatibilityReason::InvalidAdditionalToolsItem, _) => format!(
                "Grok Responses input[{}] additional_tools must contain a tools array and an optional developer or user role",
                self.input_index
            ),
            (ProtocolIncompatibilityReason::InvalidAdditionalTool, Some(tool_index)) => format!(
                "Grok Responses input[{}].tools[{tool_index}] is not a valid tool declaration",
                self.input_index
            ),
            (ProtocolIncompatibilityReason::UnsupportedAdditionalTool, Some(tool_index)) => format!(
                "Grok Responses input[{}].tools[{tool_index}] cannot be mapped losslessly to a Grok tool",
                self.input_index
            ),
            (ProtocolIncompatibilityReason::ConflictingToolDefinition, Some(tool_index)) => format!(
                "Grok Responses input[{}].tools[{tool_index}] conflicts with another tool declaration",
                self.input_index
            ),
            (ProtocolIncompatibilityReason::InvalidTopLevelTools, _) => {
                "Grok Responses top-level tools must be an array before additional_tools can be mapped"
                    .to_string()
            }
            _ => "Grok Responses request is not protocol-compatible".to_string(),
        }
    }

    pub fn rejection_action(self) -> TransformAction {
        TransformAction {
            action: TransformActionKind::Reject,
            input_index: self.input_index,
            item_type: if self.reason == ProtocolIncompatibilityReason::UnsupportedInputItem {
                "unknown"
            } else {
                "additional_tools"
            },
            tool_count: usize::from(self.tool_index.is_some()),
            merged_tool_count: 0,
            deduplicated_tool_count: 0,
            reason: Some(self.reason.as_str()),
        }
    }
}

impl std::fmt::Display for ProtocolCompatibilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.client_message())
    }
}

impl std::error::Error for ProtocolCompatibilityError {}

pub(crate) fn normalize_grok_responses_request(
    input: &Value,
) -> Result<(Value, TransformPlan), ProtocolCompatibilityError> {
    let mut output = input.clone();
    let mut plan = TransformPlan::unchanged();
    let Some(input_items) = input.get("input").and_then(Value::as_array) else {
        return Ok((output, plan));
    };

    let mut retained_input = Vec::with_capacity(input_items.len());
    let mut merged_tools: Option<Vec<Value>> = None;
    for (input_index, item) in input_items.iter().enumerate() {
        let item_type = match item.get("type") {
            None => {
                plan.preserved_input_items = plan.preserved_input_items.saturating_add(1);
                retained_input.push(item.clone());
                continue;
            }
            Some(Value::String(item_type)) => item_type.as_str(),
            Some(_) => {
                return Err(ProtocolCompatibilityError::new(
                    ProtocolIncompatibilityReason::UnsupportedInputItem,
                    input_index,
                    None,
                ))
            }
        };
        if GROK_RESPONSES_INPUT_ITEM_TYPES.contains(&item_type) {
            plan.preserved_input_items = plan.preserved_input_items.saturating_add(1);
            retained_input.push(item.clone());
            continue;
        }
        if item_type != "additional_tools" {
            return Err(ProtocolCompatibilityError::new(
                ProtocolIncompatibilityReason::UnsupportedInputItem,
                input_index,
                None,
            ));
        }

        let object = item.as_object().ok_or_else(|| {
            ProtocolCompatibilityError::new(
                ProtocolIncompatibilityReason::InvalidAdditionalToolsItem,
                input_index,
                None,
            )
        })?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "type" | "role" | "tools"))
            || object.get("role").is_some_and(|role| {
                role.as_str()
                    .map(str::trim)
                    .is_none_or(|role| !matches!(role, "developer" | "user"))
            })
        {
            return Err(ProtocolCompatibilityError::new(
                ProtocolIncompatibilityReason::InvalidAdditionalToolsItem,
                input_index,
                None,
            ));
        }
        let tools = object
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProtocolCompatibilityError::new(
                    ProtocolIncompatibilityReason::InvalidAdditionalToolsItem,
                    input_index,
                    None,
                )
            })?;
        if object.get("role").and_then(Value::as_str).map(str::trim) == Some("user") {
            plan.fidelity = TransformFidelity::DeclaredLossy;
        }
        if merged_tools.is_none() {
            merged_tools = Some(match input.get("tools") {
                None => Vec::new(),
                Some(Value::Array(tools)) => tools.clone(),
                Some(_) => {
                    return Err(ProtocolCompatibilityError::new(
                        ProtocolIncompatibilityReason::InvalidTopLevelTools,
                        input_index,
                        None,
                    ))
                }
            });
        }

        let target = merged_tools
            .as_mut()
            .expect("additional_tools initializes the target tool list");
        let mut merged_tool_count = 0_usize;
        let mut deduplicated_tool_count = 0_usize;
        for (tool_index, tool) in tools.iter().enumerate() {
            validate_additional_tool(tool, input_index, tool_index)?;
            if target.iter().any(|existing| existing == tool) {
                deduplicated_tool_count = deduplicated_tool_count.saturating_add(1);
                continue;
            }
            let identity = tool_identity(tool).ok_or_else(|| {
                ProtocolCompatibilityError::new(
                    ProtocolIncompatibilityReason::InvalidAdditionalTool,
                    input_index,
                    Some(tool_index),
                )
            })?;
            if target
                .iter()
                .filter_map(tool_identity)
                .any(|existing| existing == identity)
            {
                return Err(ProtocolCompatibilityError::new(
                    ProtocolIncompatibilityReason::ConflictingToolDefinition,
                    input_index,
                    Some(tool_index),
                ));
            }
            target.push(tool.clone());
            merged_tool_count = merged_tool_count.saturating_add(1);
        }
        plan.actions.push(TransformAction {
            action: TransformActionKind::Map,
            input_index,
            item_type: "additional_tools",
            tool_count: tools.len(),
            merged_tool_count,
            deduplicated_tool_count,
            reason: None,
        });
    }

    if let Some(tools) = merged_tools {
        let object = output
            .as_object_mut()
            .expect("an input array can only be read from a JSON object");
        object.insert("input".to_string(), Value::Array(retained_input));
        if !tools.is_empty() || input.get("tools").is_some() {
            object.insert("tools".to_string(), Value::Array(tools));
        }
    }
    Ok((output, plan))
}

fn validate_additional_tool(
    tool: &Value,
    input_index: usize,
    tool_index: usize,
) -> Result<(), ProtocolCompatibilityError> {
    let Some(object) = tool.as_object() else {
        return Err(ProtocolCompatibilityError::new(
            ProtocolIncompatibilityReason::InvalidAdditionalTool,
            input_index,
            Some(tool_index),
        ));
    };
    let Some(tool_type) = object.get("type").and_then(Value::as_str) else {
        return Err(ProtocolCompatibilityError::new(
            ProtocolIncompatibilityReason::InvalidAdditionalTool,
            input_index,
            Some(tool_index),
        ));
    };
    if !GROK_RESPONSES_TOP_LEVEL_TOOL_TYPES.contains(&tool_type) {
        return Err(ProtocolCompatibilityError::new(
            ProtocolIncompatibilityReason::UnsupportedAdditionalTool,
            input_index,
            Some(tool_index),
        ));
    }
    if tool_type == "function"
        && object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .is_none_or(str::is_empty)
    {
        return Err(ProtocolCompatibilityError::new(
            ProtocolIncompatibilityReason::InvalidAdditionalTool,
            input_index,
            Some(tool_index),
        ));
    }
    Ok(())
}

fn tool_identity(tool: &Value) -> Option<String> {
    let tool_type = tool.get("type")?.as_str()?;
    if tool_type == "function" {
        let name = tool.get("name")?.as_str()?.trim();
        return (!name.is_empty()).then(|| format!("function:{name}"));
    }
    if tool_type == "mcp" {
        let endpoint = tool
            .get("server_label")
            .or_else(|| tool.get("server_url"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("<default>");
        return Some(format!("mcp:{endpoint}"));
    }
    Some(tool_type.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn additional_tools_are_losslessly_merged_and_deduplicated() {
        let input = json!({
            "model": "grok-4.6",
            "futureTopLevelField": {"preserved": true},
            "tools": [{"type": "function", "name": "lookup", "parameters": {"type": "object"}}],
            "input": [
                {"type": "message", "role": "user", "content": "hello", "futureItemField": 1},
                {"type": "additional_tools", "role": "developer", "tools": [
                    {"type": "function", "name": "lookup", "parameters": {"type": "object"}},
                    {"type": "function", "name": "write", "parameters": {"type": "object"}},
                    {"type": "web_search"}
                ]},
                {"type": "reasoning", "summary": []}
            ]
        });

        let (output, plan) = normalize_grok_responses_request(&input).unwrap();

        assert_eq!(output["futureTopLevelField"], json!({"preserved": true}));
        assert_eq!(output["input"].as_array().unwrap().len(), 2);
        assert_eq!(output["input"][0]["futureItemField"], 1);
        assert_eq!(output["tools"].as_array().unwrap().len(), 3);
        assert_eq!(plan.fidelity, TransformFidelity::Lossless);
        assert_eq!(plan.preserved_input_items, 2);
        assert_eq!(plan.mapped_input_items(), 1);
        assert_eq!(plan.merged_tools(), 2);
        assert_eq!(plan.deduplicated_tools(), 1);
    }

    #[test]
    fn conflicting_tool_definitions_are_rejected_without_mutating_input() {
        let input = json!({
            "tools": [{"type": "function", "name": "lookup", "description": "first"}],
            "input": [{"type": "additional_tools", "tools": [
                {"type": "function", "name": "lookup", "description": "second"}
            ]}]
        });
        let original = input.clone();

        let error = normalize_grok_responses_request(&input).unwrap_err();

        assert_eq!(input, original);
        assert_eq!(
            error.reason,
            ProtocolIncompatibilityReason::ConflictingToolDefinition
        );
        assert_eq!(error.input_index, 0);
        assert_eq!(error.tool_index, Some(0));
        assert!(!error.client_message().contains("lookup"));
    }

    #[test]
    fn unsupported_and_malformed_additional_tools_are_rejected() {
        for (input, reason) in [
            (
                json!({"input": [{"type": "additional_tools", "tools": [
                    {"type": "future_tool", "name": "exec"}
                ]}]}),
                ProtocolIncompatibilityReason::UnsupportedAdditionalTool,
            ),
            (
                json!({"input": [{"type": "additional_tools", "tools": "invalid"}]}),
                ProtocolIncompatibilityReason::InvalidAdditionalToolsItem,
            ),
            (
                json!({"input": [{"type": "additional_tools", "tools": [
                    {"type": "function"}
                ]}]}),
                ProtocolIncompatibilityReason::InvalidAdditionalTool,
            ),
            (
                json!({"input": [{"type": "additional_tools", "tools": [], "unexpected": true}]}),
                ProtocolIncompatibilityReason::InvalidAdditionalToolsItem,
            ),
            (
                json!({"input": [{"type": "additional_tools", "role": 1, "tools": []}]}),
                ProtocolIncompatibilityReason::InvalidAdditionalToolsItem,
            ),
        ] {
            assert_eq!(
                normalize_grok_responses_request(&input).unwrap_err().reason,
                reason
            );
        }

        let (_, plan) = normalize_grok_responses_request(&json!({
            "input": [{"type": "additional_tools", "role": "user", "tools": []}]
        }))
        .unwrap();
        assert_eq!(plan.fidelity, TransformFidelity::DeclaredLossy);
    }

    #[test]
    fn unknown_input_items_are_rejected_while_supported_items_are_preserved() {
        let supported = GROK_RESPONSES_INPUT_ITEM_TYPES
            .iter()
            .map(|item_type| json!({"type": item_type, "future": true}))
            .collect::<Vec<_>>();
        let input = json!({"input": supported});
        let (output, plan) = normalize_grok_responses_request(&input).unwrap();
        assert_eq!(output, input);
        assert_eq!(
            plan.preserved_input_items,
            GROK_RESPONSES_INPUT_ITEM_TYPES.len()
        );

        let error = normalize_grok_responses_request(
            &json!({"input": [{"type": "future_secret_item", "secret": "do-not-log"}]}),
        )
        .unwrap_err();
        assert_eq!(
            error.reason,
            ProtocolIncompatibilityReason::UnsupportedInputItem
        );
        assert!(!error.client_message().contains("future_secret_item"));
        assert!(!error.client_message().contains("do-not-log"));

        let malformed =
            normalize_grok_responses_request(&json!({"input": [{"type": 7}]})).unwrap_err();
        assert_eq!(
            malformed.reason,
            ProtocolIncompatibilityReason::UnsupportedInputItem
        );
    }

    #[test]
    fn string_input_and_unknown_top_level_fields_remain_byte_semantically_intact() {
        let input = json!({
            "model": "grok-4.6",
            "input": "hello",
            "future": [1, 2, 3]
        });
        let (output, plan) = normalize_grok_responses_request(&input).unwrap();
        assert_eq!(output, input);
        assert!(plan.actions.is_empty());
    }
}
