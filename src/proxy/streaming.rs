use crate::domain::usage::store::{
    usage_from_json_with_semantics, InputTokenSemantics, TokenUsage,
};

#[derive(Debug, Default)]
pub struct SseLineBuffer {
    buffer: String,
}

impl SseLineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_chunk(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer.drain(..=pos);
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    pub fn finish(self) -> Option<String> {
        let tail = self.buffer.trim_end_matches('\r').trim().to_string();
        if tail.is_empty() {
            None
        } else {
            Some(tail)
        }
    }
}

#[derive(Debug)]
pub struct StreamUsageAccumulator {
    buffer: String,
    usage: TokenUsage,
    input_semantics: InputTokenSemantics,
}

impl Default for StreamUsageAccumulator {
    fn default() -> Self {
        Self::new(InputTokenSemantics::Auto)
    }
}

#[derive(Debug, Default)]
pub struct ClaudeSseErrorDetector {
    lines: SseLineBuffer,
    current_event: Option<String>,
    current_data: Vec<String>,
    non_error_event_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSseError {
    pub error_type: String,
    pub message: Option<String>,
}

impl ClaudeSseErrorDetector {
    pub fn push(&mut self, chunk: &[u8]) -> Option<ClaudeSseError> {
        for line in self.lines.push_chunk(chunk) {
            if let Some(error) = self.push_line(&line) {
                return Some(error);
            }
        }
        None
    }

    pub fn prelude_ready(&self) -> bool {
        self.non_error_event_ready
    }

    fn push_line(&mut self, line: &str) -> Option<ClaudeSseError> {
        if let Some(event) = line.strip_prefix("event:").map(str::trim) {
            self.flush_event();
            self.current_event = Some(event.to_string());
            return None;
        }
        if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            self.current_data.push(data.to_string());
            if self.current_event.as_deref() != Some("error") {
                self.non_error_event_ready = true;
            }
            return self.flush_if_error_event();
        }
        None
    }

    fn flush_if_error_event(&mut self) -> Option<ClaudeSseError> {
        if self.current_event.as_deref() != Some("error") {
            return None;
        }
        let payload = self.current_data.join("\n");
        let value = serde_json::from_str::<serde_json::Value>(&payload).ok()?;
        let error_type = value
            .pointer("/error/type")
            .or_else(|| value.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let message = value
            .pointer("/error/message")
            .or_else(|| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        self.flush_event();
        error_type.map(|error_type| ClaudeSseError {
            error_type,
            message,
        })
    }

    fn flush_event(&mut self) {
        self.current_event = None;
        self.current_data.clear();
    }
}

impl StreamUsageAccumulator {
    pub fn new(input_semantics: InputTokenSemantics) -> Self {
        Self {
            buffer: String::new(),
            usage: TokenUsage::default(),
            input_semantics,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> TokenUsage {
        let text = String::from_utf8_lossy(chunk);
        self.buffer.push_str(&text);
        if self.buffer.len() > 64 * 1024 {
            let keep_from = self.buffer.len().saturating_sub(32 * 1024);
            self.buffer = self.buffer[keep_from..].to_string();
        }

        while let Some(index) = self.buffer.find('\n') {
            let line = self.buffer[..index].trim().to_string();
            self.buffer.drain(..=index);
            self.parse_line(&line);
        }

        self.usage
    }

    pub fn finish(mut self) -> TokenUsage {
        let line = self.buffer.trim().to_string();
        if !line.is_empty() {
            self.parse_line(&line);
        }
        self.usage
    }

    fn parse_line(&mut self, line: &str) {
        let payload = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if payload.is_empty() || payload == "[DONE]" || !payload.starts_with('{') {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            return;
        };
        merge_usage(
            &mut self.usage,
            usage_from_json_with_semantics(&value, self.input_semantics),
        );
    }
}

fn merge_usage(target: &mut TokenUsage, next: TokenUsage) {
    let next_has_input = next.input_tokens.is_some()
        || next.cache_read_tokens.is_some()
        || next.cache_creation_tokens.is_some();
    let next_has_output = next.output_tokens.is_some();
    if next.raw_input_tokens.is_some() {
        target.raw_input_tokens = next.raw_input_tokens;
    }
    if next.billed_input_tokens.is_some() {
        target.billed_input_tokens = next.billed_input_tokens;
    }
    if next.input_tokens.is_some() {
        target.input_tokens = next.input_tokens;
    }
    if next.output_tokens.is_some() {
        target.output_tokens = next.output_tokens;
    }
    if next.cache_read_tokens.is_some() {
        target.cache_read_tokens = next.cache_read_tokens;
    }
    if next.cache_creation_tokens.is_some() {
        target.cache_creation_tokens = next.cache_creation_tokens;
    }
    if next.total_tokens.is_some()
        && (next_has_input || !next_has_output || target.total_tokens.is_none())
    {
        target.total_tokens = next.total_tokens;
    }
    if next_has_output
        && !next_has_input
        && (target.input_tokens.is_some() || target.output_tokens.is_some())
    {
        target.total_tokens = Some(
            target
                .raw_input_tokens
                .unwrap_or_else(|| {
                    target
                        .input_tokens
                        .unwrap_or(0)
                        .saturating_add(target.cache_read_tokens.unwrap_or(0))
                        .saturating_add(target.cache_creation_tokens.unwrap_or(0))
                })
                .saturating_add(target.output_tokens.unwrap_or(0)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_line_buffer_splits_lines_across_chunks() {
        let mut buffer = SseLineBuffer::new();
        let first = buffer.push_chunk(b"data: {\"choices\":");
        assert!(first.is_empty());
        let second = buffer.push_chunk(b"[{\"delta\":{\"content\":\"hi\"}}]}\n");
        assert_eq!(second.len(), 1);
        assert!(second[0].starts_with("data:"));
    }

    #[test]
    fn sse_line_buffer_handles_crlf_line_endings() {
        let mut buffer = SseLineBuffer::new();
        let lines = buffer.push_chunk(b"event: ping\r\ndata: {}\r\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "event: ping");
        assert_eq!(lines[1], "data: {}");
    }

    #[test]
    fn sse_line_buffer_finish_returns_trailing_partial_line() {
        let mut buffer = SseLineBuffer::new();
        buffer.push_chunk(b"data: partial");
        assert_eq!(buffer.finish().as_deref(), Some("data: partial"));
    }

    #[test]
    fn sse_line_buffer_ignores_empty_tail() {
        let buffer = SseLineBuffer::new();
        assert!(buffer.finish().is_none());
    }

    #[test]
    fn sse_line_buffer_preserves_multiple_complete_lines_in_one_chunk() {
        let mut buffer = SseLineBuffer::new();
        let lines = buffer.push_chunk(b"line1\nline2\nline3\n");
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
        assert!(buffer.finish().is_none());
    }

    #[test]
    fn sse_line_buffer_splits_mid_utf8_character_safely_via_lossy_decode() {
        let mut buffer = SseLineBuffer::new();
        let emoji = "data: 你好\n";
        let bytes = emoji.as_bytes();
        let split = bytes.len() - 2;
        buffer.push_chunk(&bytes[..split]);
        let lines = buffer.push_chunk(&bytes[split..]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("data:"));
    }

    #[test]
    fn claude_sse_error_detector_extracts_error_type_across_chunks() {
        let mut detector = ClaudeSseErrorDetector::default();
        assert!(detector.push(b"event: error\n").is_none());
        let error_type = detector
            .push(
                br#"data: {"error":{"type":"rate_limit_error","message":"slow down"}}
"#,
            )
            .unwrap();
        assert_eq!(error_type.error_type, "rate_limit_error");
        assert_eq!(error_type.message.as_deref(), Some("slow down"));
    }

    #[test]
    fn claude_sse_error_detector_ignores_non_error_events() {
        let mut detector = ClaudeSseErrorDetector::default();
        assert!(detector
            .push(
                br#"event: message_delta
data: {"type":"message_delta","delta":{"text":"hi"}}
"#
            )
            .is_none());
    }

    #[test]
    fn claude_sse_prelude_waits_for_complete_data_line_across_chunks() {
        let mut detector = ClaudeSseErrorDetector::default();
        assert!(detector
            .push(b"event: message_start\ndata: {\"type\":\"mess")
            .is_none());
        assert!(!detector.prelude_ready());
        assert!(detector.push(b"age_start\"}\n\n").is_none());
        assert!(detector.prelude_ready());
    }

    #[test]
    fn parses_openai_stream_usage_line() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.raw_input_tokens, Some(10));
        assert_eq!(usage.billed_input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(14));
    }

    #[test]
    fn parses_claude_message_start_usage() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":11,"cache_read_input_tokens":5,"output_tokens":0}}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.raw_input_tokens, Some(16));
        assert_eq!(usage.billed_input_tokens, Some(11));
        assert_eq!(usage.cache_read_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(16));
    }

    #[test]
    fn parses_codex_responses_completed_usage() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"data: {"type":"response.completed","response":{"usage":{"input_tokens":21,"output_tokens":6,"input_tokens_details":{"cached_tokens":9}}}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.billed_input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(6));
        assert_eq!(usage.cache_read_tokens, Some(9));
    }

    #[test]
    fn parses_gemini_stream_usage_metadata() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"{"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":2,"totalTokenCount":9}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(9));
    }

    #[test]
    fn stream_usage_keeps_latest_cumulative_gemini_metadata() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"{"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":2,"totalTokenCount":9}}
{"usageMetadata":{"promptTokenCount":11,"candidatesTokenCount":5,"cachedContentTokenCount":3,"totalTokenCount":16}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(8));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.total_tokens, Some(16));
    }

    #[test]
    fn stream_usage_updates_from_claude_message_delta() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":180,"cache_read_input_tokens":120,"output_tokens":0}}}
event: message_delta
data: {"type":"message_delta","usage":{"input_tokens":140,"output_tokens":8,"cache_read_input_tokens":90,"cache_creation_input_tokens":4}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(140));
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.cache_read_tokens, Some(90));
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.billed_input_tokens, Some(140));
        assert_eq!(usage.raw_input_tokens, Some(234));
        assert_eq!(usage.total_tokens, Some(242));
    }

    #[test]
    fn output_only_delta_does_not_drop_existing_input_from_total() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":11,"output_tokens":0}}}
event: message_delta
data: {"type":"message_delta","usage":{"output_tokens":8}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.total_tokens, Some(19));
    }

    fn assert_stream_usage(
        chunk: &[u8],
        input: Option<u64>,
        output: Option<u64>,
        cache_read: Option<u64>,
        cache_create: Option<u64>,
        total: Option<u64>,
    ) {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(chunk);
        let usage = parser.finish();
        assert_eq!(usage.input_tokens, input);
        assert_eq!(usage.output_tokens, output);
        assert_eq!(usage.cache_read_tokens, cache_read);
        assert_eq!(usage.cache_creation_tokens, cache_create);
        assert_eq!(usage.total_tokens, total);
    }

    macro_rules! openai_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{},\"completion_tokens\":{},\"total_tokens\":{},\"prompt_tokens_details\":{{\"cached_tokens\":{}}}}}}}\n",
                        $input,
                        $output,
                        $input + $output,
                        $cache
                    )
                    .as_bytes(),
                    Some(($input as u64).saturating_sub($cache as u64)),
                    Some($output),
                    Some($cache),
                    None,
                    Some($input + $output),
                );
            }
        };
    }

    macro_rules! claude_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal, $write:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "event: message_delta\ndata: {{\"type\":\"message_delta\",\"usage\":{{\"input_tokens\":{},\"output_tokens\":{},\"cache_read_input_tokens\":{},\"cache_creation_input_tokens\":{}}}}}\n",
                        $input,
                        $output,
                        $cache,
                        $write
                    )
                    .as_bytes(),
                    Some($input),
                    Some($output),
                    Some($cache),
                    Some($write),
                    Some($input + $cache + $write + $output),
                );
            }
        };
    }

    macro_rules! codex_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "data: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":{},\"output_tokens\":{},\"total_tokens\":{},\"input_tokens_details\":{{\"cached_tokens\":{}}}}}}}}}\n",
                        $input,
                        $output,
                        $input + $output,
                        $cache
                    )
                    .as_bytes(),
                    Some(($input as u64).saturating_sub($cache as u64)),
                    Some($output),
                    Some($cache),
                    None,
                    Some($input + $output),
                );
            }
        };
    }

    macro_rules! gemini_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "{{\"usageMetadata\":{{\"promptTokenCount\":{},\"candidatesTokenCount\":{},\"cachedContentTokenCount\":{},\"totalTokenCount\":{}}}}}\n",
                        $input,
                        $output,
                        $cache,
                        $input + $output
                    )
                    .as_bytes(),
                    Some(($input as u64).saturating_sub($cache as u64)),
                    Some($output),
                    Some($cache),
                    None,
                    Some($input + $output),
                );
            }
        };
    }

    openai_usage_case!(desktop_openai_include_usage_001, 1, 2, 0);
    openai_usage_case!(desktop_openai_include_usage_002, 3, 5, 1);
    openai_usage_case!(desktop_openai_include_usage_003, 8, 13, 2);
    openai_usage_case!(desktop_openai_include_usage_004, 21, 34, 3);
    openai_usage_case!(desktop_openai_include_usage_005, 55, 89, 5);
    openai_usage_case!(desktop_openai_include_usage_006, 144, 233, 8);
    openai_usage_case!(desktop_openai_include_usage_007, 377, 610, 13);
    openai_usage_case!(desktop_openai_include_usage_008, 987, 1597, 21);
    openai_usage_case!(desktop_openai_include_usage_009, 10, 1, 9);
    openai_usage_case!(desktop_openai_include_usage_010, 20, 2, 10);
    openai_usage_case!(desktop_openai_include_usage_011, 30, 3, 11);
    openai_usage_case!(desktop_openai_include_usage_012, 40, 4, 12);
    openai_usage_case!(desktop_openai_include_usage_013, 50, 5, 13);
    openai_usage_case!(desktop_openai_include_usage_014, 60, 6, 14);
    openai_usage_case!(desktop_openai_include_usage_015, 70, 7, 15);
    openai_usage_case!(desktop_openai_include_usage_016, 80, 8, 16);
    openai_usage_case!(desktop_openai_include_usage_017, 90, 9, 17);
    openai_usage_case!(desktop_openai_include_usage_018, 100, 10, 18);
    openai_usage_case!(desktop_openai_include_usage_019, 128, 16, 32);
    openai_usage_case!(desktop_openai_include_usage_020, 256, 32, 64);
    openai_usage_case!(desktop_openai_include_usage_021, 512, 64, 128);
    openai_usage_case!(desktop_openai_include_usage_022, 1024, 128, 256);
    openai_usage_case!(desktop_openai_include_usage_023, 2048, 256, 512);
    openai_usage_case!(desktop_openai_include_usage_024, 4096, 512, 1024);

    claude_usage_case!(desktop_claude_delta_usage_001, 1, 2, 0, 0);
    claude_usage_case!(desktop_claude_delta_usage_002, 3, 5, 1, 0);
    claude_usage_case!(desktop_claude_delta_usage_003, 8, 13, 2, 1);
    claude_usage_case!(desktop_claude_delta_usage_004, 21, 34, 3, 1);
    claude_usage_case!(desktop_claude_delta_usage_005, 55, 89, 5, 2);
    claude_usage_case!(desktop_claude_delta_usage_006, 144, 233, 8, 3);
    claude_usage_case!(desktop_claude_delta_usage_007, 377, 610, 13, 5);
    claude_usage_case!(desktop_claude_delta_usage_008, 987, 1597, 21, 8);
    claude_usage_case!(desktop_claude_delta_usage_009, 10, 1, 9, 1);
    claude_usage_case!(desktop_claude_delta_usage_010, 20, 2, 10, 2);
    claude_usage_case!(desktop_claude_delta_usage_011, 30, 3, 11, 3);
    claude_usage_case!(desktop_claude_delta_usage_012, 40, 4, 12, 4);
    claude_usage_case!(desktop_claude_delta_usage_013, 50, 5, 13, 5);
    claude_usage_case!(desktop_claude_delta_usage_014, 60, 6, 14, 6);
    claude_usage_case!(desktop_claude_delta_usage_015, 70, 7, 15, 7);
    claude_usage_case!(desktop_claude_delta_usage_016, 80, 8, 16, 8);
    claude_usage_case!(desktop_claude_delta_usage_017, 90, 9, 17, 9);
    claude_usage_case!(desktop_claude_delta_usage_018, 100, 10, 18, 10);
    claude_usage_case!(desktop_claude_delta_usage_019, 128, 16, 32, 4);
    claude_usage_case!(desktop_claude_delta_usage_020, 256, 32, 64, 8);
    claude_usage_case!(desktop_claude_delta_usage_021, 512, 64, 128, 16);
    claude_usage_case!(desktop_claude_delta_usage_022, 1024, 128, 256, 32);
    claude_usage_case!(desktop_claude_delta_usage_023, 2048, 256, 512, 64);
    claude_usage_case!(desktop_claude_delta_usage_024, 4096, 512, 1024, 128);

    codex_usage_case!(desktop_codex_response_completed_001, 1, 2, 0);
    codex_usage_case!(desktop_codex_response_completed_002, 3, 5, 1);
    codex_usage_case!(desktop_codex_response_completed_003, 8, 13, 2);
    codex_usage_case!(desktop_codex_response_completed_004, 21, 34, 3);
    codex_usage_case!(desktop_codex_response_completed_005, 55, 89, 5);
    codex_usage_case!(desktop_codex_response_completed_006, 144, 233, 8);
    codex_usage_case!(desktop_codex_response_completed_007, 377, 610, 13);
    codex_usage_case!(desktop_codex_response_completed_008, 987, 1597, 21);
    codex_usage_case!(desktop_codex_response_completed_009, 10, 1, 9);
    codex_usage_case!(desktop_codex_response_completed_010, 20, 2, 10);
    codex_usage_case!(desktop_codex_response_completed_011, 30, 3, 11);
    codex_usage_case!(desktop_codex_response_completed_012, 40, 4, 12);
    codex_usage_case!(desktop_codex_response_completed_013, 50, 5, 13);
    codex_usage_case!(desktop_codex_response_completed_014, 60, 6, 14);
    codex_usage_case!(desktop_codex_response_completed_015, 70, 7, 15);
    codex_usage_case!(desktop_codex_response_completed_016, 80, 8, 16);
    codex_usage_case!(desktop_codex_response_completed_017, 90, 9, 17);
    codex_usage_case!(desktop_codex_response_completed_018, 100, 10, 18);
    codex_usage_case!(desktop_codex_response_completed_019, 128, 16, 32);
    codex_usage_case!(desktop_codex_response_completed_020, 256, 32, 64);
    codex_usage_case!(desktop_codex_response_completed_021, 512, 64, 128);
    codex_usage_case!(desktop_codex_response_completed_022, 1024, 128, 256);
    codex_usage_case!(desktop_codex_response_completed_023, 2048, 256, 512);
    codex_usage_case!(desktop_codex_response_completed_024, 4096, 512, 1024);

    gemini_usage_case!(desktop_gemini_usage_metadata_001, 1, 2, 0);
    gemini_usage_case!(desktop_gemini_usage_metadata_002, 3, 5, 1);
    gemini_usage_case!(desktop_gemini_usage_metadata_003, 8, 13, 2);
    gemini_usage_case!(desktop_gemini_usage_metadata_004, 21, 34, 3);
    gemini_usage_case!(desktop_gemini_usage_metadata_005, 55, 89, 5);
    gemini_usage_case!(desktop_gemini_usage_metadata_006, 144, 233, 8);
    gemini_usage_case!(desktop_gemini_usage_metadata_007, 377, 610, 13);
    gemini_usage_case!(desktop_gemini_usage_metadata_008, 987, 1597, 21);
    gemini_usage_case!(desktop_gemini_usage_metadata_009, 10, 1, 9);
    gemini_usage_case!(desktop_gemini_usage_metadata_010, 20, 2, 10);
    gemini_usage_case!(desktop_gemini_usage_metadata_011, 30, 3, 11);
    gemini_usage_case!(desktop_gemini_usage_metadata_012, 40, 4, 12);
    gemini_usage_case!(desktop_gemini_usage_metadata_013, 50, 5, 13);
    gemini_usage_case!(desktop_gemini_usage_metadata_014, 60, 6, 14);
    gemini_usage_case!(desktop_gemini_usage_metadata_015, 70, 7, 15);
    gemini_usage_case!(desktop_gemini_usage_metadata_016, 80, 8, 16);
    gemini_usage_case!(desktop_gemini_usage_metadata_017, 90, 9, 17);
    gemini_usage_case!(desktop_gemini_usage_metadata_018, 100, 10, 18);
    gemini_usage_case!(desktop_gemini_usage_metadata_019, 128, 16, 32);
    gemini_usage_case!(desktop_gemini_usage_metadata_020, 256, 32, 64);
    gemini_usage_case!(desktop_gemini_usage_metadata_021, 512, 64, 128);
    gemini_usage_case!(desktop_gemini_usage_metadata_022, 1024, 128, 256);
    gemini_usage_case!(desktop_gemini_usage_metadata_023, 2048, 256, 512);
    gemini_usage_case!(desktop_gemini_usage_metadata_024, 4096, 512, 1024);
}
