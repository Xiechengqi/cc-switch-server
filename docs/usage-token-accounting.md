# Usage Token 口径

`cc-switch-server` 在 usage log 中保留 raw 与 billed 两套输入 token 口径，避免 Codex/OpenAI cached token 与实际计费混淆。

- `rawInputTokens`：上游返回的原始输入 token，通常等于 `inputTokens`。
- `billedInputTokens`：用于 input price 计费的输入 token。当前规则是 `inputTokens - cacheReadTokens`，无 cache 信息时等于 `inputTokens`。
- `inputTokens`：兼容 router/market 现有字段，保留原始输入 token。
- `cacheReadTokens`：命中缓存的输入 token。
- `cacheCreationTokens`：写入缓存的输入 token。
- `outputTokens`：输出 token。
- `totalTokens`：优先使用上游 `total_tokens/totalTokenCount`；缺失时按 `inputTokens + outputTokens` 或 cache-only 字段推导。

Cost 计算使用 `billedInputTokens` 计算 input cost，使用 `cacheReadTokens/cacheCreationTokens` 分别计算缓存读写成本。

解析来源：

- Claude/Anthropic：支持 `message.usage`、`usage`、流式 `message_delta` 的 `usage` / `delta.usage`，并识别 `cache_read_input_tokens`、`cache_creation_input_tokens` 及 camelCase/cache alias。
- Codex/OpenAI：支持 `response.usage`、OpenAI Chat `stream_options.include_usage` 产生的末尾 `usage` block、`input_tokens_details.cached_tokens` 和 `prompt_tokens_details.cached_tokens`。
- Gemini：支持非流式和流式 `usageMetadata`；流式场景按上游累计块覆盖，最终保留最新累计值。

会话关联：

- `sessionId` 会写入本地 usage log，并在 direct share request log sync 时传给 router。
- Claude 从 session header、`metadata.user_id` 的 `_session_` 后缀和 `metadata.session_id/sessionId` 提取。
- Codex 从 `session_id`、`x-session-id`、`x-codex-session-id`、`x-client-request-id`、`x-codex-window-id` 和 metadata 提取。

Stream 状态：

- `pending`：stream 请求已开始，尚未收到上游结束。
- `streaming`：已收到首个上游 chunk。
- `completed`：上游正常结束。
- `upstream_error`：上游 stream 过程中报错。
- `interrupted`：客户端在 stream 结束前断开。
