# Transform/Streaming Coverage Tracker

Date: 2026-07-07

This tracker follows Phase X / X7. It records server-side test coverage against the desktop transform and streaming suites without importing desktop-only behavior.

## Current Gate

| Item | Value |
| --- | --- |
| Desktop baseline | 253 tests (`transform_codex_chat` 55, `transform_responses` 61, `transform` 59, `transform_gemini` 26, `streaming` 52) |
| Server target | 75% / 190 tests |
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

## Remaining X7 Work

| Area | Status | Notes |
| --- | --- | --- |
| Streaming tool-call delta reassembly, including parallel calls | To port | Highest remaining proxy correctness risk; should land in X7 second batch. |
| SSE frame boundary slicing (half frame, multi-frame, CRLF) | To port | Use server `streaming.rs` fixture macros where possible. |
| Full stop/finish reason matrix across OpenAI Responses, Chat, Anthropic, Gemini streams | Partial | Non-streaming matrix started in first batch. |
| Image and file block matrix across all request directions | Partial | Responses->Chat and existing Anthropic/Gemini paths covered; expand to edge cases. |
| Desktop-only semantics | Not applicable | MCP, Skills, desktop profile/session UI behavior remain excluded by server product boundary. |
