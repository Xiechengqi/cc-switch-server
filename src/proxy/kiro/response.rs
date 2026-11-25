use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub(super) struct StopReason {
    reason: Option<&'static str>,
    sequence: Option<String>,
}

impl StopReason {
    pub(super) fn observe(&mut self, payload: &Value) {
        if self.is_refusal() {
            return;
        }
        let payload = payload
            .get("metadataEvent")
            .or_else(|| payload.get("messageMetadataEvent"))
            .unwrap_or(payload);
        let reason = payload.get("stopReason").and_then(Value::as_str);
        self.reason = match reason.map(str::to_ascii_uppercase).as_deref() {
            Some("CONTENT_FILTERED" | "REFUSAL") => Some("refusal"),
            Some("MAX_TOKENS") => Some("max_tokens"),
            Some("MODEL_CONTEXT_WINDOW_EXCEEDED") => Some("model_context_window_exceeded"),
            Some("STOP_SEQUENCE") => Some("stop_sequence"),
            _ => self.reason,
        };
        if self.reason == Some("stop_sequence") {
            self.sequence = payload
                .get("stopSequence")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
    }

    pub(super) fn is_refusal(&self) -> bool {
        self.reason == Some("refusal")
    }

    pub(super) fn finish(&self, has_tools: bool) -> &'static str {
        self.reason
            .unwrap_or(if has_tools { "tool_use" } else { "end_turn" })
    }

    pub(super) fn sequence(&self) -> Option<&str> {
        (self.reason == Some("stop_sequence"))
            .then_some(self.sequence.as_deref())
            .flatten()
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.sequence.as_ref().map_or(0, String::capacity)
    }
}

/// Keep a possible tag suffix across AWS events. Event boundaries can occur
/// inside either tag, independently of HTTP chunk and UTF-8 boundaries.
#[derive(Debug, Default)]
pub(super) struct ThinkingParser {
    pending: String,
    inside: bool,
}

impl ThinkingParser {
    pub(super) fn push(&mut self, text: &str, finish: bool) -> Vec<(bool, String)> {
        self.pending.push_str(text);
        let mut output = Vec::new();
        loop {
            let tag = if self.inside {
                "</thinking>"
            } else {
                "<thinking>"
            };
            if let Some(index) = self.pending.find(tag) {
                if index > 0 {
                    output.push((self.inside, self.pending[..index].to_string()));
                }
                self.pending.drain(..index + tag.len());
                self.inside = !self.inside;
                continue;
            }
            let held = if finish {
                0
            } else {
                super::marker_suffix_len(&self.pending, tag)
            };
            let end = self.pending.len() - held;
            if end > 0 {
                output.push((self.inside, self.pending.drain(..end).collect()));
            }
            break;
        }
        output
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.pending.capacity()
    }
}
