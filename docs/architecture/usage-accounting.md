# Usage Token 口径

`cc-switch-server` 将 usage 统一成互不重叠的 fresh input、cache read、cache creation、output 四桶，再保留兼容字段供 router/market 使用。

- `rawInputTokens`：总输入量，即 fresh + cache read + cache creation；OpenAI/Gemini 的 inclusive input 原样进入这里，Anthropic 的 exclusive input 会补回 cache 两桶。
- `inputTokens`：归一后的 fresh input；router/market 字段名不变。
- `cacheReadTokens`：命中缓存的输入 token。
- `cacheCreationTokens`：写入缓存的输入 token。
- `outputTokens`：输出 token。
- `totalTokens`：优先使用上游 `total_tokens/totalTokenCount`；缺失时按 `rawInputTokens + outputTokens` 推导。

Server 只记录上述 Token 桶，不按模型价格计算或保存成本金额。

Kiro `meteringEvent` 的 credits 是独立的 Provider 计量，不是 Token：

- `creditUsage`：usage log 中可选的本次 credit 合计；上游明确报告 `0` 时保留为零，旧记录或未报告时为 `null`/缺失。
- Kiro 三种 text surface 的响应 usage 还会按需保留 `credit_usage`、`credit_unit`、`credit_unit_plural`；多次有效事件按增量求和，单位标签取最后一个非空值。
- 非有限、负数、单事件或累计超过 `1_000_000_000` 的值不进入计量。
- credits 不加入 `inputTokens`、`outputTokens`、cache token、`totalTokens`、Share token 用量或 token quota。

该口径来自脱敏 fixture 和对照实现证据；真实订阅账号的多事件语义仍为 `live_pending`，在完成 live 验收前不把 credits 用作扣费或配额决策。

Codex OAuth Images 在同一 usage log 额外记录独立的输出元数据，不把图片字节换算成 Token 或费用：

- `imageCount`：语义完成并成功渲染的图片数。
- `imageBytes`：base64 解码后的图片总字节数。
- `imageFormat`：`png`、`jpeg`、`webp`；多种格式时为 `mixed`。
- `imageWidth` / `imageHeight`：可识别时的实际像素尺寸。
- `imageSize`：实际 `WIDTHxHEIGHT`，无法识别时回落到上游 size；多种尺寸时为 `mixed`。

图片元数据只在语义 completed 后写入。partial image、失败、超时或客户端取消不会被统计为成功输出；Prometheus 对应暴露 `cc_switch_codex_images_requests_total`、`cc_switch_codex_images_output_total` 和 `cc_switch_codex_images_output_bytes_total`。

解析来源：

- Claude/Anthropic：支持 `message.usage`、`usage`、流式 `message_delta` 的 `usage` / `delta.usage`，并识别 `cache_read_input_tokens`、`cache_creation_input_tokens` 及 camelCase/cache alias。
- Codex/OpenAI：支持 `response.usage`、OpenAI Chat `stream_options.include_usage` 末尾 `usage`、`input_tokens_details` / `prompt_tokens_details` 下的 cached、cache creation、`cache_write_tokens` / `cached_creation_tokens` 别名；显式零值会保留。
- Gemini：支持非流式和流式 `usageMetadata`；流式场景按上游累计块覆盖，最终保留最新累计值。

会话关联：

- `sessionId` 会写入本地 usage log，并在 Router Share request log sync 时传给 router。
- 外层 `x-cc-switch-session-id` 优先于各 surface 自身候选；候选值最长 256 字节，且只接受字母、数字、`-`、`_`、`.`、`:`。
- Claude 从 session header、JSON 字符串形式的 `metadata.user_id.session_id`、legacy `_session_` 后缀和 `metadata.session_id/sessionId` 提取。
- Codex 优先从 body `prompt_cache_key` 提取，再读取 `x-session-affinity`、`x-client-request-id`、`session_id`、`x-session-id`、`x-codex-session-id`、`x-codex-window-id` 和 metadata/session 字段。
- Kiro 找不到有效 session 时生成 request-scoped anonymous ID；同一请求的 conversation、prompt-cache namespace 和 usage correlation 复用该 ID，不同请求不会共享固定 `anonymous` scope。

Stream 状态：

- `pending`：stream 请求已开始，尚未收到上游结束。
- `streaming`：已收到首个上游 chunk。
- `completed`：上游正常结束。
- `upstream_error`：上游 stream 过程中报错。
- `interrupted`：客户端在 stream 结束前断开。
- `failed`：Images 上游显式失败、协议错误或本地渲染失败。
- `timeout`：Images 首事件或事件后空闲超时；usage `statusCode` 为 504。
- `client_cancelled`：Images 下游 Body 被取消；usage `statusCode` 为 499。

Images 为穿过 Cloudflare 保持连接，可能在最终结果前提交 SSE comment 或 JSON 空白。因此 wire HTTP status 可能保持 200，终态应以完整 payload 和 usage log 的 `statusCode`/`streamStatus` 为准。

Grok OAuth 媒体与文本共用同一终态 Usage 记录模型，但媒体没有 token 语义：

- `requestKind` 固定为 `image` 或 `video`；`operation` 区分 `image_generation`、`image_edit`、`video_generation` 和 `video_status`。
- `usageState=not_applicable` 是正常终态，不计入 missing/parse/transform error，也不把 token 兼容字段当作真实零值。
- 视频创建记录保存 `mediaTaskId` 和 submitted/status；状态查询通过 durable task binding 写入 `parentRequestId`，关联原创建请求。
- 视频参数只保留有界的 `videoDurationSeconds`、`videoResolution`、`videoAspectRatio`；错误只保留有界摘要。prompt、图片原始字节、凭据和原始私有响应 URL 不进入 Usage。
- Router Share 同步合同 v5 携带上述字段。Router 普通请求列表排除签名的 `HealthProbe`，并可按 `requestKind=text|image|video` 在数据库查询前过滤和分页。

Cursor required/named tool semantic retry 最多有三个同绑定 attempt。当前客户端 success usage 使用最终被提交 attempt 的估算值；每次被丢弃的 attempt 通过 `cursor_tool_retry_total{rail,reason,attempt}` 和脱敏 warning 审计，不把早期 prose 暴露给客户端。该 usage 是估算值并持续设置 `usageEstimated=true`。在上游提供逐 attempt 可归属的权威 usage 前，不将丢弃 attempt 伪装为客户端 token usage；订阅侧的实际消耗可能高于最终响应中的估算值。
