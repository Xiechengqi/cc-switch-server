# Usage Token 口径

`cc-switch-server` 将 usage 统一成互不重叠的 fresh input、cache read、cache creation、output 四桶，再保留兼容字段供 router/market 使用。

- `rawInputTokens`：总输入量，即 fresh + cache read + cache creation；OpenAI/Gemini 的 inclusive input 原样进入这里，Anthropic 的 exclusive input 会补回 cache 两桶。
- `inputTokens`：归一后的 fresh input；router/market 字段名不变。
- `cacheReadTokens`：命中缓存的输入 token。
- `cacheCreationTokens`：写入缓存的输入 token。
- `outputTokens`：输出 token。
- `totalTokens`：优先使用上游 `total_tokens/totalTokenCount`；缺失时按 `rawInputTokens + outputTokens` 推导。

Server 只记录上述 Token 桶，不按模型价格计算或保存成本金额。

解析来源：

- Claude/Anthropic：支持 `message.usage`、`usage`、流式 `message_delta` 的 `usage` / `delta.usage`，并识别 `cache_read_input_tokens`、`cache_creation_input_tokens` 及 camelCase/cache alias。
- Codex/OpenAI：支持 `response.usage`、OpenAI Chat `stream_options.include_usage` 末尾 `usage`、`input_tokens_details` / `prompt_tokens_details` 下的 cached、cache creation、`cache_write_tokens` / `cached_creation_tokens` 别名；显式零值会保留。
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
