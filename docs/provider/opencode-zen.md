# OpenCode Zen 免 Key 反代

状态：协议对齐 zen-gate 单账号免费车道。离线 fixture 覆盖会话铸造、请求头、工具指纹和字段顺序。线上目录与真实推理为 `live_pending`。

范围是一条免费车道，不是 OpenCode Go，也不是付费 Zen API Key。一个 Provider Bundle 固定使用 `Authorization: Bearer public`，不建号池，不轮换出口。

## 上游

- Origin：`https://opencode.ai`
- Chat：`/inference/v1/chat/completions`
- Muse Spark：`/inference/v1/responses`
- `union-alpha`：`/inference/v1/messages`
- UA：`opencode/latest/2.1.3/cli`
- 头：`x-opencode-client: cli`，以及 `x-opencode-session`、`x-opencode-session-id`、`x-session-affinity`、`x-session-id`
- 不发送 `x-opencode-request`。不加 `prompt_cache_key` 或 `cache_control`。

Session 必须铸造。12 位十六进制前缀是 `~(ms<<12|seq)` 的低 48 位。网关只还原其中低 36 位毫秒。同一下游会话复用同一个 id，满 6 天重铸。哈希前缀会被上游 403。

请求必须带官方 `read` 与 `shell` 的原文描述和参数。调用方的同名工具改名为 `read_client`，响应和 SSE 增量再改回调用方名称。chat JSON 关闭 HTML 转义，顶层字段按 `model, messages, tools, stream, stream_options, max_completion_tokens` 排序。chat 在调用方没给上限时补 `max_completion_tokens: 32000`。Responses 缺省 `tool_choice` 才补 `auto`。`tool_choice: none` 本地直接 403。

429 与 403 `FreeTierError` 直接失败，不在同一模型上重试。没有 token，不做 401 刷新。非流式 chat 截断续写最多一次，只覆盖纯推理中断、只有推理的正常结束、以及 `finish_reason=length` 的回答截断。已经开始的 SSE 不再拼接第二次上游响应。

## Server

- Family：`family.opencode_zen`
- Profiles：`claude.opencode_zen`、`codex.opencode_zen`
- Driver：`special.opencode_zen`
- 凭据：无用户密钥。endpoint 固定，不允许覆盖。
- Gemini Surface 不开放。
- 不把 OpenCode 注册成第四个 App。
- 美元计量走现有 HTTP/SSE 生命周期。发出上游请求前准入，发送失败记未发送。用量从缓冲后的 JSON 或 SSE 读取。这只说明账本接线，不构成真实推理回执。

目录来自 `GET /zen/v1/models`。保留 id 含 `free` 的模型，以及 `union-alpha`、`space-bunny-free`。拉取失败时沿用上一份目录。生成请求在目录为空时先拉一次；冷启动失败不使用写死清单，请求会被拒绝。公开 `/v1/models` 和管理端发现共用这一份目录。能力字段只标已确认的流式和工具，不标未探测的图像。发现状态保持 `live_pending`：接线已完成，没有线上回执。

## 证据

只读参考：`/data/projects/proxy/OpenCode/zen-gate/internal/lane/upstream.go`。该仓库不是构建、测试或运行时输入。指纹版本变化时单独改本文件和 `src/proxy/opencode_zen.rs`，不在热路径里猜测。
