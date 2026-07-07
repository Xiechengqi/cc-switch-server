# Transform/Streaming Coverage Tracker

Date: 2026-07-07

This tracker follows Phase X / X7. It records server-side test coverage against the desktop transform and streaming suites without importing desktop-only behavior.

## Current Gate

| Item | Value |
| --- | --- |
| Desktop baseline | 253 tests (`transform_codex_chat` 55, `transform_responses` 61, `transform` 59, `transform_gemini` 26, `streaming` 52) |
| Server target | 85% / 216 tests |
| Gate | `node scripts/audit/audit-transform-coverage.mjs --check` from `scripts/static-checks.sh` |

## First Batch Covered

| Area | Status | Server evidence |
| --- | --- | --- |
| Codex Responses request -> Chat multimodal input | Covered | `openai_responses_to_chat_preserves_multimodal_request_fields` |
| Codex Responses function call/output request items -> Chat messages | Covered | `openai_responses_to_chat_maps_function_call_and_output_items` |
| Codex Responses function tools/tool_choice -> Chat tools/tool_choice | Covered | `openai_responses_to_chat_maps_function_tools_and_tool_choice` |
| Responses response `incomplete` -> Chat `finish_reason=length` | Covered | `openai_responses_response_to_chat_maps_incomplete_status_to_length` |
| Anthropic stop reason matrix -> OpenAI Chat/Gemini finish reasons | Covered | `response_finish_reason_matrix_maps_across_protocols` |
| Ollama Codex `xhigh` reasoning clamp | Covered | Adapter fixtures added by X2 |

## Second Batch Covered

| Area | Status | Server evidence |
| --- | --- | --- |
| OpenAI Chat tool-call streaming deltas -> Anthropic tool_use events | Covered | `streaming_tool_call_deltas_map_to_anthropic_tool_use_events` |
| OpenAI Responses function-call streaming -> Anthropic tool_use events | Covered | `responses_streaming_function_call_maps_to_anthropic_tool_events` |
| Gemini `functionCall` streaming -> Anthropic tool_use events | Covered | `gemini_streaming_function_call_maps_to_anthropic_tool_events` |
| Anthropic tool_use streaming -> OpenAI Chat/Responses events | Covered | `anthropic_streaming_tool_use_maps_to_openai_chat_tool_call_deltas`, `anthropic_streaming_tool_use_maps_to_openai_responses_events` |
| Stream finish reason matrix additions | Covered | `anthropic_streaming_tool_stop_maps_to_openai_finish_reasons`, `stream_finish_reason_matrix_maps_to_downstream_protocols` |
| SSE CRLF multi-frame chunk parsing | Covered | `adapter_stream_transform_handles_crlf_multi_frame_sse_chunks` |

## Third Batch Covered (Phase Y)

| Area | Status | Server evidence |
| --- | --- | --- |
| Anthropic/OpenAI/Gemini stop & finish reason fixtures | Covered | `anthropic_response_maps_*`, `anthropic_stream_*`, `openai_chat_stream_*` |
| SSE line buffer across chunk boundaries | Covered | `sse_line_buffer_*`, `deepseek_stream_fixture_emits_claude_sse_across_chunk_boundaries` |
| DeepSeek Account Claude protocol bridge | Covered | `proxy/deepseek.rs`, `forward_claude_deepseek`, `clients/deepseek/client.rs` mock upstream test |
| Bedrock converse tool_use + inferenceConfig | Covered | `bedrock_converse_body_maps_tool_use_and_inference_config_from_anthropic` |

## Remaining X7 Work

| Area | Status | Notes |
| --- | --- | --- |
| Streaming tool-call delta reassembly, including parallel calls | Partial | Protocol-level tool-call deltas now covered in both directions; true cross-chunk reassembly still requires stateful stream buffering above the stateless adapter transform hook. |
| SSE frame boundary slicing (half frame, multi-frame, CRLF) | Covered (Y2) | `SseLineBuffer` + DeepSeek stream fixture; adapter CRLF multi-frame test retained |
| Full stop/finish reason matrix across OpenAI Responses, Chat, Anthropic, Gemini streams | Partial | Non-streaming matrix plus key stream finish_reason paths covered; remaining provider-specific finish reasons can be added as fixtures. |
| Image and file block matrix across all request directions | Partial | Responses->Chat and existing Anthropic/Gemini paths covered; expand to edge cases. |
| Desktop-only semantics | Not applicable | MCP, Skills, desktop profile/session UI behavior remain excluded by server product boundary. |
