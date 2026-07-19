# Transform/Streaming Coverage Tracker

Date: 2026-07-19

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

## Claude cache usage hardening

| Area | Status | Server evidence |
| --- | --- | --- |
| Anthropic cache-exclusive input -> normalized four buckets | Covered | `explicit_input_semantics_normalize_to_same_four_buckets`, `parses_claude_message_start_usage` |
| OpenAI/Gemini cache-inclusive input -> normalized four buckets | Covered | `parses_cache_usage_shapes`, `parses_openai_stream_usage_line`, `parses_gemini_stream_usage_metadata` |
| Responses -> Anthropic cache creation, stream and non-stream | Covered | `responses_anthropic_usage_round_trip_preserves_cache_creation`, `stream_snapshots_convert_between_sse_formats` |
| Anthropic -> Responses/Chat inclusive input restoration | Covered | `responses_anthropic_usage_round_trip_preserves_cache_creation`, `response_snapshots_convert_anthropic_to_openai_responses_and_chat` |

## Stateful cross-protocol streaming

| Area | Status | Server evidence |
| --- | --- | --- |
| Generic SSE framing across arbitrary network chunks | Covered | `StreamEventTransformer`, `framing_is_stable_across_every_chunk_boundary_and_crlf` |
| Responses parallel function calls and output-index mapping | Covered | `responses_parallel_tools_preserve_packed_done_arguments` |
| Packed `function_call_arguments.done` fallback | Covered | `responses_parallel_tools_preserve_packed_done_arguments`, `responses_done_does_not_duplicate_streamed_arguments` |
| OpenAI Chat parallel tool lifecycle | Covered | `ChatAnthropicState` shares per-tool open/delta/stop state with the Responses bridge |
| EOF incomplete event handling | Covered | `eof_half_json_is_a_protocol_error` plus bounded protocol-error metric labels |

## Codex v2 protocol hardening

| Area | Status | Server evidence |
| --- | --- | --- |
| Responses Lite `additional_tools` and custom history | Covered | `responses_lite_additional_custom_tools_and_history_convert_to_chat` |
| Custom tool non-stream and stream response restoration | Covered | `chat_custom_tool_response_is_restored_to_responses_item`, `custom_tool_stream_bridge_restores_freeform_events_and_completed_output` |
| Built-in `tool_search` downgrade and collision rejection | Covered | `responses_tool_search_name_collision_is_rejected` |
| Responses wire required zero fields | Covered | `custom_tool_events_keep_required_zero_index`, `deltas_keep_required_zero_fields` |
| Invalid continuation message IDs | Covered | `normalize_codex_oauth_gates_reasoning_and_strips_invalid_message_ids` |

## Remaining X7 Work

| Area | Status | Notes |
| --- | --- | --- |
| Streaming tool-call delta reassembly, including parallel calls | Covered | Per-request Responses/Chat lifecycle state tracks open blocks, output-index mapping, argument deltas and packed done fallback. |
| SSE frame boundary slicing (half frame, multi-frame, CRLF) | Covered | `StreamEventTransformer` buffers every transformed protocol; `SseLineBuffer` remains for native DeepSeek parsing. |
| Full stop/finish reason matrix across OpenAI Responses, Chat, Anthropic, Gemini streams | Partial | Non-streaming matrix plus key stream finish_reason paths covered; remaining provider-specific finish reasons can be added as fixtures. |
| Image and file block matrix across all request directions | Partial | Responses->Chat and existing Anthropic/Gemini paths covered; expand to edge cases. |
| Desktop-only semantics | Not applicable | MCP, Skills, desktop profile/session UI behavior remain excluded by server product boundary. |
