use std::collections::BTreeMap;

use serde_json::Value;

use super::ProxyError;

#[derive(Debug, Default)]
struct Candidate {
    finished: bool,
    output: bool,
    refusal: bool,
}

/// Shared by native streaming, first-event admission and non-stream aggregation.
/// Usage, role-only shells and opaque signatures are not generated output.
#[derive(Debug, Default)]
pub(super) struct GeminiTerminal {
    candidates: BTreeMap<i64, Candidate>,
    blocked: bool,
}

impl GeminiTerminal {
    pub(super) fn retained_bytes(&self) -> usize {
        self.candidates.len().saturating_mul(96)
    }

    pub(super) fn observe(&mut self, response: &Value) -> Result<(), ProxyError> {
        let object = response.as_object().ok_or_else(|| {
            ProxyError::bad_gateway("Gemini v1internal response must be a JSON object")
        })?;
        self.blocked |= response
            .pointer("/promptFeedback/blockReason")
            .or_else(|| response.pointer("/prompt_feedback/block_reason"))
            .and_then(Value::as_str)
            .is_some_and(|reason| {
                !reason.trim().is_empty() && reason.trim() != "BLOCK_REASON_UNSPECIFIED"
            });
        if let Some(candidates) = object.get("candidates") {
            let candidates = candidates.as_array().ok_or_else(|| {
                ProxyError::bad_gateway("Gemini v1internal candidates must be an array")
            })?;
            for (position, candidate) in candidates.iter().enumerate() {
                if !candidate.is_object() {
                    return Err(ProxyError::bad_gateway(
                        "Gemini v1internal candidate must be a JSON object",
                    ));
                }
                let index = candidate
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or(position as i64);
                if !self.candidates.contains_key(&index) && self.candidates.len() >= 128 {
                    return Err(ProxyError::bad_gateway(
                        "Gemini response exceeds 128 candidates",
                    ));
                }
                let state = self.candidates.entry(index).or_default();
                state.output |= candidate_has_output(candidate);
                if let Some(reason) = candidate
                    .get("finishReason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty() && *reason != "FINISH_REASON_UNSPECIFIED")
                {
                    if matches!(
                        reason,
                        "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" | "MALFORMED_RESPONSE"
                    ) {
                        return Err(ProxyError::bad_gateway(format!(
                            "Gemini generation failed with {reason}"
                        )));
                    }
                    state.finished = true;
                    state.refusal |= matches!(
                        reason,
                        "SAFETY"
                            | "RECITATION"
                            | "BLOCKLIST"
                            | "PROHIBITED_CONTENT"
                            | "SPII"
                            | "IMAGE_SAFETY"
                    );
                }
            }
        }
        Ok(())
    }

    pub(super) fn ready(&self) -> bool {
        self.blocked
            || self
                .candidates
                .values()
                .any(|candidate| candidate.output || candidate.refusal)
    }

    pub(super) fn validate(&self) -> Result<(), ProxyError> {
        if self.candidates.is_empty() && self.blocked {
            return Ok(());
        }
        if !self.candidates.is_empty()
            && self.candidates.values().all(|candidate| candidate.finished)
            && self.ready()
        {
            return Ok(());
        }
        Err(ProxyError::bad_gateway("Gemini v1internal stream ended without generated output and terminal candidate finishReason values or blocked prompt feedback"))
    }
}

fn nonempty(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty())
}

fn candidate_has_output(candidate: &Value) -> bool {
    if [
        "/groundingMetadata/groundingChunks",
        "/grounding_metadata/grounding_chunks",
    ]
    .iter()
    .any(|pointer| {
        candidate
            .pointer(pointer)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    }) {
        return true;
    }
    candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                nonempty(part.get("text"))
                    || nonempty(
                        part.pointer("/functionCall/name")
                            .or_else(|| part.pointer("/function_call/name")),
                    )
                    || nonempty(
                        part.pointer("/inlineData/data")
                            .or_else(|| part.pointer("/inline_data/data")),
                    )
                    || nonempty(
                        part.pointer("/fileData/fileUri")
                            .or_else(|| part.pointer("/file_data/file_uri")),
                    )
                    || nonempty(
                        part.pointer("/executableCode/code")
                            .or_else(|| part.pointer("/executable_code/code")),
                    )
                    || nonempty(
                        part.pointer("/codeExecutionResult/output")
                            .or_else(|| part.pointer("/code_execution_result/output")),
                    )
                    || nonempty(
                        part.pointer("/codeExecutionResult/outcome")
                            .or_else(|| part.pointer("/code_execution_result/outcome")),
                    )
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn gemini_native_shells_signatures_and_abnormal_terminals_are_not_success() {
        for payload in [
            json!({"usageMetadata":{"totalTokenCount":2}}),
            json!({"candidates":[{"content":{"role":"model","parts":[{"thoughtSignature":"opaque"}]},"finishReason":"STOP"}]}),
            json!({"candidates":[{"finishReason":"STOP"}]}),
        ] {
            let mut state = GeminiTerminal::default();
            state.observe(&payload).unwrap();
            assert!(!state.ready());
            assert!(state.validate().is_err());
        }
        for reason in [
            "MALFORMED_FUNCTION_CALL",
            "UNEXPECTED_TOOL_CALL",
            "MALFORMED_RESPONSE",
        ] {
            assert!(GeminiTerminal::default()
                .observe(&json!({"candidates":[{"finishReason":reason}]}))
                .is_err());
        }
    }

    #[test]
    fn gemini_native_thought_tools_media_grounding_and_refusals_are_output() {
        for candidate in [
            json!({"content":{"parts":[{"thought":true,"text":"reasoning"}]}}),
            json!({"content":{"parts":[{"functionCall":{"name":"run","args":{}}}]}}),
            json!({"content":{"parts":[{"inlineData":{"data":"AA=="}}]}}),
            json!({"groundingMetadata":{"groundingChunks":[{"web":{"uri":"https://example.test"}}]}}),
            json!({"finishReason":"SAFETY"}),
        ] {
            let mut candidate = candidate;
            let mut state = GeminiTerminal::default();
            state
                .observe(&json!({"candidates":[candidate.clone()]}))
                .unwrap();
            assert!(state.ready());
            if candidate.get("finishReason").is_none() {
                candidate["finishReason"] = json!("STOP");
            }
            state.observe(&json!({"candidates":[candidate]})).unwrap();
            state.validate().unwrap();
        }
        let mut blocked = GeminiTerminal::default();
        blocked
            .observe(&json!({"promptFeedback":{"blockReason":"SAFETY"}}))
            .unwrap();
        blocked.validate().unwrap();
    }
}
