//! Inspect protocol control positions, never prompts, schemas or history.
use super::capabilities::ShareOperation as Op;
use crate::domain::usage::store::InferenceOperation;
use serde_json::Value;
use std::collections::BTreeSet;
pub fn binding_fingerprint(
    plan: &crate::domain::providers::runtime::ProviderRuntimePlan,
) -> String {
    use sha2::{Digest, Sha256};
    // RuntimeAuthRef has identity/generation references, never OAuth tokens.
    hex::encode(Sha256::digest(
        serde_json::to_vec(&serde_json::json!([
            plan.provider_key,
            plan.profile_id,
            plan.profile_schema_revision,
            plan.driver_id,
            plan.driver_contract_revision,
            plan.upstream_protocol,
            plan.coding_plan,
            plan.auth_ref,
            plan.model_policy,
            plan.media_policy,
            plan.driver_options,
            plan.endpoint,
            super::capabilities::CAPABILITY_CATALOG_VERSION,
            crate::domain::usage::value_pricing::RESOURCE_ESTIMATE_VERSION
        ]))
        .expect("serializable runtime identity"),
    ))
}
pub fn operations(body: &Value, operation: InferenceOperation) -> Vec<Op> {
    let mut out = BTreeSet::new();
    match operation {
        InferenceOperation::ImageGeneration => {
            out.insert(Op::ImageGenerate);
        }
        InferenceOperation::ImageEdit => {
            out.insert(Op::ImageEdit);
        }
        InferenceOperation::VideoGeneration => {
            out.insert(Op::VideoGenerate);
        }
        _ => {}
    }
    for path in [
        "/modalities",
        "/generationConfig/responseModalities",
        "/generation_config/response_modalities",
    ] {
        for mode in body
            .pointer(path)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            match mode.to_ascii_lowercase().as_str() {
                "image" => {
                    out.insert(Op::ImageGenerate);
                }
                "audio" => {
                    out.insert(Op::AudioGenerate);
                }
                "video" => {
                    out.insert(Op::VideoGenerate);
                }
                _ => {}
            }
        }
    }
    if body.get("audio").is_some_and(Value::is_object) {
        out.insert(Op::AudioGenerate);
    }
    for tool in body
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = tool.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(kind, "namespace" | "allowed_tools") {
            out.extend(operations(tool, InferenceOperation::Unknown));
            continue;
        }
        if matches!(kind, "function" | "custom") {
            continue;
        }
        let op = if kind == "image_generation" {
            match tool.get("action").and_then(Value::as_str) {
                Some("generate") => Some(Op::ImageGenerate),
                Some("edit") => Some(Op::ImageEdit),
                _ => {
                    out.insert(Op::ImageEdit);
                    Some(Op::ImageGenerate)
                }
            }
        } else if kind == "web_search"
            || kind.starts_with("web_search_")
            || [
                "googleSearch",
                "google_search",
                "googleSearchRetrieval",
                "google_search_retrieval",
            ]
            .iter()
            .any(|k| tool.get(*k).is_some())
        {
            Some(Op::WebSearch)
        } else if kind == "web_fetch"
            || kind.starts_with("web_fetch_")
            || ["urlContext", "url_context"]
                .iter()
                .any(|k| tool.get(*k).is_some())
        {
            Some(Op::WebFetch)
        } else if kind == "file_search"
            || kind.starts_with("file_search_")
            || ["fileSearch", "file_search"]
                .iter()
                .any(|key| tool.get(*key).is_some())
        {
            Some(Op::FileSearch)
        } else if (kind == "shell"
            && tool.pointer("/environment/type").and_then(Value::as_str) != Some("local"))
            || kind == "code_interpreter"
            || kind.starts_with("code_execution")
            || ["codeExecution", "code_execution"]
                .iter()
                .any(|k| tool.get(*k).is_some())
        {
            Some(Op::CodeExecution)
        } else if kind == "computer_use_preview" {
            Some(Op::Computer)
        } else if kind == "video_generation" {
            Some(Op::VideoGenerate)
        } else if kind == "mcp" {
            Some(Op::RemoteMcp)
        } else {
            None
        };
        // Anthropic computer/bash tools are client-executed: their names alone
        // must never be mistaken for provider-hosted execution.
        if let Some(op) = op {
            out.insert(op);
        }
        // Gemini's untagged tool object may contain multiple executable fields.
        // Each one needs permission; a permitted sibling cannot hide another.
        for (keys, operation) in [
            (
                &[
                    "googleSearch",
                    "google_search",
                    "googleSearchRetrieval",
                    "google_search_retrieval",
                ][..],
                Op::WebSearch,
            ),
            (&["urlContext", "url_context"][..], Op::WebFetch),
            (&["fileSearch", "file_search"][..], Op::FileSearch),
            (&["codeExecution", "code_execution"][..], Op::CodeExecution),
        ] {
            if keys.iter().any(|key| tool.get(*key).is_some()) {
                out.insert(operation);
            }
        }
    }
    if let Some(choice) = body.get("tool_choice").filter(|v| v.is_object()) {
        match choice.get("type").and_then(Value::as_str) {
            Some("function" | "custom" | "tool" | "auto" | "none" | "any" | "required") => {}
            Some("allowed_tools" | "namespace") => {
                out.extend(operations(choice, InferenceOperation::Unknown));
            }
            _ => out.extend(operations(
                &serde_json::json!({"tools":[choice]}),
                InferenceOperation::Unknown,
            )),
        }
    }
    if body.get("web_search_options").is_some_and(|v| !v.is_null())
        || body.get("search_enabled").and_then(Value::as_bool) == Some(true)
    {
        out.insert(Op::WebSearch);
    }
    out.into_iter().collect()
}

/// Unknown hosted tool types cannot acquire permission merely by passing through
/// a newer upstream schema. Known client-executed tools remain unrestricted.
pub fn invalid_control(body: &Value) -> Option<&'static str> {
    for path in [
        "/modalities",
        "/generationConfig/responseModalities",
        "/generation_config/response_modalities",
    ] {
        if let Some(modes) = body.pointer(path).filter(|v| !v.is_null()) {
            let Some(modes) = modes.as_array() else {
                return Some("invalid output modalities");
            };
            if modes.iter().any(|mode| {
                mode.as_str().is_none_or(|mode| {
                    !matches!(
                        mode.to_ascii_lowercase().as_str(),
                        "text" | "image" | "audio" | "video"
                    )
                })
            }) {
                return Some("unsupported output modality");
            }
        }
    }
    if let Some(choice) = body.get("tool_choice").filter(|v| v.is_object()) {
        // Anthropic chooses a client tool by name; OpenAI chooses by type.
        if choice.get("type").and_then(Value::as_str) != Some("tool")
            && !matches!(
                choice.get("type").and_then(Value::as_str),
                Some(
                    "auto" | "none" | "any" | "required" | "allowed_tools" | "function" | "custom"
                )
            )
        {
            if let Some(reason) = invalid_control(&serde_json::json!({"tools":[choice]})) {
                return Some(reason);
            }
        }
        if let Some(tools) = choice.get("tools") {
            if let Some(reason) = invalid_control(&serde_json::json!({"tools":tools})) {
                return Some(reason);
            }
        }
    }
    if let Some(tools) = body.get("tools").filter(|v| !v.is_null()) {
        let Some(tools) = tools.as_array() else {
            return Some("invalid tools");
        };
        for tool in tools {
            let raw_tool = tool;
            let Some(tool) = tool.as_object() else {
                return Some("invalid tool");
            };
            let kind = tool.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "namespace" {
                if let Some(reason) = invalid_control(raw_tool) {
                    return Some(reason);
                }
            } else if !kind.is_empty()
                && !matches!(
                    kind,
                    "function"
                        | "custom"
                        | "image_generation"
                        | "web_search"
                        | "web_fetch"
                        | "file_search"
                        | "code_interpreter"
                        | "computer_use_preview"
                        | "mcp"
                        | "video_generation"
                        | "local_shell"
                        | "shell"
                        | "apply_patch"
                )
                && ![
                    "web_search_",
                    "web_fetch_",
                    "file_search_",
                    "code_execution",
                    "computer_",
                    "bash_",
                    "text_editor_",
                ]
                .iter()
                .any(|prefix| kind.starts_with(prefix))
            {
                return Some("unsupported tool type");
            } else if kind.is_empty() {
                // Gemini tools are untagged objects and may combine fields.
                // A known sibling must not authorize an unknown hosted tool.
                let known = [
                    "name",
                    "description",
                    "input_schema",
                    "inputSchema",
                    "cache_control",
                    "strict",
                    "allowed_callers",
                    "input_examples",
                    "defer_loading",
                    "eager_input_streaming",
                    "functionDeclarations",
                    "function_declarations",
                    "googleSearch",
                    "google_search",
                    "googleSearchRetrieval",
                    "google_search_retrieval",
                    "urlContext",
                    "url_context",
                    "fileSearch",
                    "file_search",
                    "codeExecution",
                    "code_execution",
                ];
                if tool.is_empty() || tool.keys().any(|key| !known.contains(&key.as_str())) {
                    return Some("unsupported tool declaration");
                }
            }
        }
    }
    None
}

/// Models with a dedicated generated-media default need permission even when
/// output modalities are omitted. Match documented model identifiers, not prompt text.
pub fn model_operations(model: &str) -> Vec<Op> {
    let model = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    if model.starts_with("gpt-image-")
        || model.starts_with("imagen-")
        || model.starts_with("gemini-2.5-flash-image")
        || model.starts_with("gemini-3-pro-image")
        || model.starts_with("gemini-3.1-flash-image")
        || model == "gemini-2.0-flash-preview-image-generation"
    {
        vec![Op::ImageGenerate]
    } else {
        vec![]
    }
}

pub fn model_operations_for_request(
    model: &str,
    plan: Option<&crate::domain::usage::value_pricing::ResourcePlan>,
) -> Vec<Op> {
    let gemini = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
        .starts_with("gemini-");
    if gemini && plan.is_some_and(|p| p.text_output_only) {
        return vec![];
    }
    let mut ops = model_operations(model);
    if gemini && ops.contains(&Op::ImageGenerate) && plan.is_some_and(|p| p.gemini_image_input) {
        ops.push(Op::ImageEdit);
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn capability_intent_understanding_inputs_and_client_tool_names_never_grant_generation() {
        let body = json!({"input":[{"type":"input_image","image_url":"data:image/png;base64,a"},{"type":"input_audio","input_audio":{"data":"a","format":"wav"}},{"type":"input_file","file_url":"https://example.test/file.pdf"}],"tools":[{"type":"function","name":"image_generation","parameters":{"properties":{"audio":{"type":"string"}}}},{"type":"namespace","name":"web_search"},{"type":"computer_20250124","name":"computer"},{"type":"bash_20250124","name":"bash"}]});
        assert!(operations(&body, InferenceOperation::Responses).is_empty());
        assert_eq!(
            operations(
                &json!({"tools":[{"type":"image_generation","action":"edit"}]}),
                InferenceOperation::Responses
            ),
            vec![Op::ImageEdit]
        );
    }
    #[test]
    fn capability_intent_native_and_embedded_routes_resolve_same_operation() {
        assert_eq!(
            operations(&json!({}), InferenceOperation::ImageGeneration),
            vec![Op::ImageGenerate]
        );
        assert_eq!(
            operations(
                &json!({"generationConfig":{"responseModalities":["TEXT","IMAGE"]}}),
                InferenceOperation::GeminiGenerateContent
            ),
            vec![Op::ImageGenerate]
        );
        assert_eq!(
            operations(
                &json!({"modalities":["text","audio"],"tools":[{"type":"web_search_preview"},{"type":"function","name":"image_generation"}]}),
                InferenceOperation::ChatCompletions
            ),
            vec![Op::AudioGenerate, Op::WebSearch]
        );
        assert_eq!(
            operations(
                &json!({"tools":[{"type":"computer_use_preview"}]}),
                InferenceOperation::Responses
            ),
            vec![Op::Computer]
        );
        assert!(operations(
            &json!({"input":[{"type":"image_generation_call"}],"prompt":"web_search"}),
            InferenceOperation::Responses
        )
        .is_empty());
    }
    #[test]
    fn capability_auto_image_tools_need_both_actions_and_unknown_controls_fail_closed() {
        assert_eq!(
            operations(
                &json!({"tools":[{"type":"image_generation"}]}),
                InferenceOperation::Responses
            ),
            vec![Op::ImageGenerate, Op::ImageEdit]
        );
        assert_eq!(
            operations(
                &json!({"tools":[{"type":"image_generation","action":"generate"}]}),
                InferenceOperation::Responses
            ),
            vec![Op::ImageGenerate]
        );
        assert_eq!(
            operations(
                &json!({"tools":[{"type":"namespace","name":"client","tools":[{"type":"function","name":"web_search"}]}]}),
                InferenceOperation::Responses
            ),
            vec![]
        );
        for body in [
            json!({"tools":[{"type":"future_paid_operation"}]}),
            json!({"tools":[{"googleMaps":{}}]}),
            json!({"tools":[{"functionDeclarations":[],"googleMaps":{}}]}),
            json!({"tools":[{"name":"custom","futureHostedTool":{}}]}),
            json!({"modalities":["text","future_output"]}),
            json!({"generationConfig":{"responseModalities":["IMAGE",7]}}),
            json!({"tool_choice":{"type":"future_paid_operation"}}),
            json!({"tool_choice":{"type":"allowed_tools","tools":[{"type":"future_paid_operation"}]}}),
        ] {
            assert!(crate::domain::usage::value_pricing::resource_request_plan(
                &body,
                InferenceOperation::Responses
            )
            .invalid_reason
            .is_some());
        }
        assert!(invalid_control(&json!({"tools":[{"functionDeclarations":[{"name":"custom","parameters":{"properties":{"googleMaps":{"type":"object"}}}}]}]})).is_none());
        assert_eq!(
            operations(
                &json!({"tools":[{"googleSearchRetrieval":{}}]}),
                InferenceOperation::GeminiGenerateContent
            ),
            vec![Op::WebSearch]
        );
    }
    #[test]
    fn capability_combined_gemini_tools_require_every_hosted_operation() {
        let tools = json!([{
            "functionDeclarations": [{"name":"custom","parameters":{"properties":{"image_generation":{"type":"string"}}}}],
            "googleSearch": {}, "urlContext": {}, "fileSearch": {}, "codeExecution": {}
        }]);
        let expected = vec![
            Op::WebSearch,
            Op::WebFetch,
            Op::FileSearch,
            Op::CodeExecution,
        ];
        let body = json!({"tools":tools});
        assert!(invalid_control(&body).is_none());
        assert_eq!(
            operations(&body, InferenceOperation::GeminiGenerateContent),
            expected
        );
        assert_eq!(
            operations(
                &json!({"tool_choice":{"type":"allowed_tools","tools":tools}}),
                InferenceOperation::Responses
            ),
            expected
        );
        assert!(operations(
            &json!({"tool_choice":{"type":"function","function":{"name":"image_generation"}}}),
            InferenceOperation::Responses
        )
        .is_empty());
    }
    #[test]
    fn capability_gemini_image_understanding_with_text_output_stays_base_but_editing_needs_permission(
    ) {
        use crate::domain::usage::value_pricing::resource_request_plan;
        let mut body = json!({"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"image/png","data":"fixture"}}]}],"generationConfig":{"responseModalities":["TEXT"]}});
        let text = resource_request_plan(&body, InferenceOperation::GeminiGenerateContent);
        assert!(text.operations.is_empty());
        assert!(model_operations_for_request("gemini-2.5-flash-image", Some(&text)).is_empty());
        body["generationConfig"]["responseModalities"] = json!(["TEXT", "IMAGE"]);
        let edit = resource_request_plan(&body, InferenceOperation::GeminiGenerateContent);
        assert!(edit.operations.contains(&Op::ImageGenerate));
        assert!(edit.operations.contains(&Op::ImageEdit));
        body.as_object_mut().unwrap().remove("generationConfig");
        let implicit = resource_request_plan(&body, InferenceOperation::GeminiGenerateContent);
        assert_eq!(
            model_operations_for_request("gemini-2.5-flash-image", Some(&implicit)),
            vec![Op::ImageGenerate, Op::ImageEdit]
        );
    }
    #[test]
    fn capability_resource_parameters_reject_negative_or_overflow_without_changing_requested_quantity(
    ) {
        for body in [
            json!({"n":0}),
            json!({"n":-1}),
            json!({"n":101}),
            json!({"n":1.5}),
            json!({"max_output_tokens":-1}),
        ] {
            assert!(crate::domain::usage::value_pricing::resource_request_plan(
                &body,
                InferenceOperation::ImageGeneration
            )
            .invalid_reason
            .is_some());
        }
        assert!(crate::domain::usage::value_pricing::resource_request_plan(
            &json!({"n":2,"size":"1024x1024"}),
            InferenceOperation::ImageGeneration
        )
        .invalid_reason
        .is_none());
    }
}
