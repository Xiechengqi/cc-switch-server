# Codex OAuth 单账号反代

本文描述 cc-switch-server 对 OpenAI/Codex OAuth 的生产边界、账号选择、数据面行为和验收方式。目标是把一个明确选定的 ChatGPT OAuth 账号稳定暴露给 Codex/OpenAI-compatible 客户端；不提供账号池、轮询、权重调度、配额溢出或跨账号故障转移。

## 能力边界

- 文本入口包括 `POST /v1/responses`、`POST /v1/chat/completions` 及兼容别名。
- 原生 compact 入口包括 `POST /v1/responses/compact` 及兼容别名。
- Responses WebSocket 使用 `GET /v1/responses` 及兼容别名。
- Codex 专用 surface 包括 models manifest、alpha search、图片生成和图片编辑。
- 本地直连推理入口使用独立 inference token；经过 Router 签名验证的请求沿用 Router 身份。
- 每个 `codex_oauth` Provider 必须绑定当前活动的 `codex_oauth` Account。
- 一次请求从开始到结束只使用解析出的 Provider、账号和 workspace；任何错误都不能触发通用 Provider failover 或账号轮换。

单账号边界用于避免重复执行、重复计费、会话漂移，以及 OAuth token、workspace、WebSocket 连接或 prompt cache identity 在账号之间串用。

## 活动账号状态

`accounts.json` 持久化 `activeCodexOauthAccountId`。`GET /api/accounts` 的 `codexOauth` 字段和 Web 兼容命令 `auth_get_status` 的 `codex_oauth` 字段都返回以下状态：

| 状态 | 含义 | 数据面 |
| --- | --- | --- |
| `unconfigured` | 没有 Codex OAuth 账号 | 拒绝 Codex OAuth 出站 |
| `ready` | 已解析出唯一活动账号 | 仅允许该账号出站 |
| `needs_selection` | 存在多个账号但没有有效的显式选择 | 拒绝所有 Codex OAuth 出站 |

只有一个账号时，该账号自动成为活动账号，不要求额外写入选择。存在两个或更多账号时，管理员必须在 Web 中选择“当前反代账号”，或调用：

```http
POST /api/accounts/codex/active
Content-Type: application/json

{"accountId":"<codex-oauth-account-id>"}
```

兼容命令 `auth_set_default_account` 的 Codex 分支执行同一操作；这里的“default”不表示候选顺序，而是系统唯一的当前反代账号。

## 原子 Provider 重绑

选择活动账号时，Server 会在一个协调事务中：

1. 锁定所有 Codex OAuth 账号的 refresh、Provider commit 和引用变更。
2. 验证目标账号存在且类型为 `codex_oauth`。
3. 将所有未被 Share 占用的 Codex OAuth Provider 重绑到目标账号及其当前 `authIdentityGeneration`。
4. 增加发生绑定变化的 Provider revision，重建并校验 RuntimePlan。
5. 通过带 commit marker 的事务提交 `accounts.json`、`providers.json` 和引用图。
6. 发布同一代内存快照；启动和后续写入前会恢复已提交但尚未应用完整的事务。

如果任一非删除 Share 仍引用需要切换身份的 Codex OAuth Provider，选择操作返回冲突并列出 Share，不进行部分重绑。管理员必须先按 Share 的 paused/revision-CAS 规则处理该绑定。Share 不是活动账号规则的例外：引用非活动账号的 Codex 请求同样不能出站。

## OpenAI 信任边界

- Device OAuth 与官方 CLI PKCE OAuth 共用 Server 登录状态机；远程管理保留官方 `http://localhost:1455/auth/callback`，只接受完整 callback URL 回传。
- OpenAI JWKS 固定为 `https://auth.openai.com/.well-known/jwks.json`，issuer 固定为 `https://auth.openai.com`。
- ID token audience 固定为官方 Codex client ID；access token audience 固定为 `https://api.openai.com/v1`。
- ID/access JWT 只接受 RS256，并校验 `kid`、issuer、audience、expiry 和 nbf；未知 `kid` 会刷新 JWKS 缓存后再判定。
- 身份合并必须同时得到非空 verified subject 和 verified `chatgpt_account_id`。subject 是本地 principal，workspace 只用于上游 `chatgpt-account-id`，二者不能互相替代。
- 同 subject 重新登录复用稳定派生的本地账号 ID；refresh 返回不同 subject 时 fail closed。
- authorize、token、quota、models、alpha search、inference、Images 和 WebSocket endpoint 固定为经审计的 OpenAI/ChatGPT 生产 origin，Provider 配置不能把 OAuth 凭据导向自定义 host。
- Server 不自动读取或写入宿主机用户的 `~/.codex/auth.json`。

账号 token 使用共享的加密 `accounts.json` 持久化。控制面只返回凭据存在性、脱敏身份、状态和 quota，不返回 access token、refresh token、ID token、extra headers、profile 或 raw 上游载荷。

## Provider 与账号固定

一次直连请求按以下顺序解析执行身份：

1. 解析 Codex 当前 Provider；显式 `x-cc-provider-id` 只表示调用方主动固定一个 Provider。
2. 编译后的 RuntimePlan 必须与已提交的 Provider revision、类型和账号身份代际一致。
3. 对 `codex_oauth` Driver，绑定账号必须等于当前活动账号。
4. 检查账号登录、cooldown、quota 和并发状态，并获取该账号的 in-flight lease。
5. 对即将过期的 token 执行同账号 refresh，再物化 Authorization、workspace 和 Codex CLI identity headers。

当前/显式 Provider 不可用、活动账号未选择、绑定过期、账号需要重登、处于 cooldown、quota 耗尽或并发饱和时，请求直接失败。系统不会查询另一个 Codex Provider 或账号。Share 请求额外固定其不可变 Provider binding，但仍必须通过活动账号门禁。

models manifest、alpha search、Provider 网络测试、模型发现、quota refresh、Images、HTTP、SSE、WebSocket 和 WS 到 HTTP fallback 使用同一活动账号规则。credential persistence degraded 时，这些需要 OAuth 凭据的出站操作在网络前返回 `503`。

## HTTP、SSE 与 Images

- Responses/Chat 请求先执行 Codex body sanitizer、model capability 和官方 CLI identity contract。
- 首次 401 只允许对原账号强制 refresh 一次，再以同一 Provider、账号、workspace、session 和请求 body 重放。
- 第二次 401、429、403、5xx、网络错误或流内错误都不能切换 Provider/账号。
- 非流式 `response.failed` 和 SSE semantic failure 会保留 OpenAI 错误语义；已向下游提交业务输出后绝不透明重放完整生成。
- Responses Lite、custom/freeform tool、`tool_search`、usage 四桶和空 `response.completed.output` 恢复使用同一执行身份。
- Images generation/edit 使用固定 Codex bridge、身份头和 body 上限；401 重放后仍返回原始上游错误 body，不用另一个账号掩盖错误。
- models manifest 与 alpha search 只访问固定 ChatGPT Codex endpoint，并采用同账号一次 401 refresh 边界。

### Images 兼容与资源边界

Codex OAuth 图片桥覆盖 `POST /v1/images/generations`、`POST /images/generations`、`POST /v1/images/edits` 和 `POST /images/edits`。它把 OpenAI Images 请求转换成同一活动账号上的 Responses `image_generation` tool 调用；上游始终使用增量 SSE，generation 与 edit 分别回放为 `image_generation.*` 和 `image_edit.*` 事件。`n` 当前只接受 `1`，不会用未验证的多图语义制造重复生成或重复计费。

- 输入模型只接受 `gpt-image-*`；`gpt-image-2-2k` 和 `gpt-image-2-4k` 会规范化为 `gpt-image-2` 及对应方向尺寸。
- `response_format` 支持 `b64_json` 和 `url`；同时校验 `size`、`quality`、`background`、`output_format`、`moderation`、`input_fidelity`、`output_compression`、`partial_images` 和 `stream` 的类型、范围及组合关系。
- edit 支持 JSON image URL/data URI 和 multipart `image`/`image[]`/`mask`。每张输入图最多 20 MiB、所有输入图和 mask 合计最多 32 MiB、input image 最多 16 张、mask 最多 1 张；MIME 必须与 PNG/JPEG/GIF/WebP/BMP/AVIF 签名一致。为容纳 32 MiB 图片经 base64 后的膨胀和 multipart 元数据，Codex Images HTTP decoded envelope 上限为 48 MiB；图片解码后的聚合上限仍为 32 MiB。共享 Images 路由上的 Grok media wire/decoded body 继续限制为 32 MiB。
- 远程图片只允许 HTTP(S)，逐次校验 redirect、DNS 全部解析结果和最终 MIME/signature，阻止私网、loopback、保留地址、协议降级与 DNS rebinding，并使用 10 秒单图、30 秒批次和 4 路并发上限。
- 单张解码后输出最多 48 MiB，Codex Images 成功流累计最多 72 MiB，以容纳 48 MiB 图片的 base64 膨胀和 SSE/JSON 元数据。输出必须是合法 base64，且实际 PNG/JPEG/WebP 签名必须与上游声明格式一致；能识别的图片会记录实际宽高，无法识别尺寸时仍保留上游 size 元数据。

下游 `stream=true` 会立即发送 `: connected`，空闲期间每 15 秒发送 `: keepalive`；partial、completed 和 error 按 Images SSE 事件输出。`stream=false` 仍保持一个合法 JSON 文档：先发送 JSON 合法空白，并每 15 秒发送空白心跳，最终再写入 JSON 对象。这样 Cloudflare 可以持续看到源站字节，而 Server 不需要把整个生成结果缓冲到内存后才响应。

首个有效上游事件使用 `STREAM_FIRST_BYTE_TIMEOUT_MS`，之后使用 `STREAM_IDLE_TIMEOUT_MS`。`response.failed`、`response.incomplete`、cancel/error 事件、缺失终止事件的 EOF、超限、网络错误和超时都不会记为 Provider 成功。客户端中断会记为 HTTP `499`/`client_cancelled`，并在 Body 释放时归还账号与 Share in-flight lease。

由于首个空白或 SSE comment 已提交 HTTP headers，后续流内失败不能再把 wire status 从 `200` 改成 `502/504`：stream 模式返回 `event: error`，JSON 模式返回标准 error JSON；本地 usage log 的 `statusCode`、`streamStatus` 和 `errorMessage` 记录真实终态。调用方不能只看初始 HTTP 200 判断图片成功，必须消费完整 Body 并验证 completed/data 或 error。

### `response_format=url`

URL 模式不会返回伪造或上游私有 URL。Server 将解码后的图片放入进程内 capability store，并返回同源 `/v1/images/files/<256-bit-token>`：

- token 具有 256-bit 随机熵，GET/HEAD 不再要求 inference token；token 本身就是短期访问能力。
- TTL 为 1 小时，最多 128 项、合计 256 MiB；达到上限会淘汰最旧项，进程重启会使 URL 不可用。多副本部署必须让生成请求及其后续文件 GET/HEAD 粘性回到同一实例；当前实现没有共享对象存储，无法保证跨实例 URL 可用。
- 下载响应带 `private, no-store`、`nosniff`、准确 Content-Type/Length，不应被 Cloudflare Cache、浏览器或下游代理持久化。
- public origin 优先使用 `CC_SWITCH_IMAGE_PUBLIC_BASE_URL`，其次使用 Router 已验证的 Client tunnel host，再按 Host/forwarded headers 推导。环境变量必须是无 path/query/fragment 的 HTTP(S) origin。

### Cloudflare Proxy

Cloudflare DNS 橙云或 Tunnel 直接保留公开 Host 时可自动推导 URL。Cloudflare Worker 把请求转发到不同源站 host 时，应显式配置：

```bash
CC_SWITCH_IMAGE_PUBLIC_BASE_URL=https://api.example.com
```

Worker 必须把上游 Body 当作 `ReadableStream` 原样返回，例如 `new Response(upstream.body, { status, headers })`；不能调用 `.text()`、`.json()`、`.arrayBuffer()`，也不能先聚合完整响应。还必须：

- 透传或正确生成公开 Host、`CF-Visitor`/`X-Forwarded-Proto`，不要覆盖 Server 的 `Cache-Control: no-store` 和 `X-Accel-Buffering: no`。
- 对 Images SSE/JSON 关闭 Worker 自定义缓冲、响应重写和 cache；允许至少每 15 秒一个很小的 chunk 立即流向客户端。
- 允许 `/v1/images/files/<token>` 的匿名 GET/HEAD 到达同一 Server 实例，不把 inference bearer 追加到生成 URL，也不缓存 capability 响应。
- 确认 WAF、请求体和上传规则允许 Codex Images 的 48 MiB HTTP envelope（解码图片聚合仍为 32 MiB）；真实验证首块、15 秒以上生成、URL 下载和客户端取消。

Cloudflare 524 是否消失、Worker 是否实际 flush 小 chunk、订阅账号是否拥有 image entitlement，仍是外部部署/真实 OAuth gate，离线测试不能替代。

## WebSocket 与 HTTP Fallback

Codex Responses WebSocket 使用有界连接池，pool key 包含进程、Provider/runtime、session、upstream URL、credential generation 和 workspace headers。

- 同一下游连接中的每个 `response.create` 串行执行。
- connect/timeout、握手 5xx、stale cached socket，以及成功发送 `response.create` 前的 send 错误可以回退到 HTTP/SSE。
- `response.create` 一旦成功发送，上游 read/close/首事件超时（包括 close 1009）都只终止当前生命周期，不进行 transport fallback；握手 400/401/403/429 和 idle timeout 同样不回退。
- fallback 复用原 `ProviderExecution`、账号、workspace、session、request body 和 in-flight lease，不重新进入 Router。
- 首次握手/HTTP 401 只强刷原账号一次；不会借 fallback 获得额外的 refresh 或 Provider failover 次数。
- `response.create` 一旦成功发送到上游，后续失败只终止当前 lifecycle，不重放整个请求。
- `codexWebsocketEnabled=false` 可关闭 WS，但不影响 POST Responses HTTP/SSE。

## Context Overflow 自动压缩

`CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT=1` 可启用保守的 context overflow 恢复，默认关闭。它只作用于 Codex OAuth 的 HTTP Responses、Chat 和 Responses SSE，不作用于其他 Driver，也不替代显式 `/responses/compact`。

当首次响应为 HTTP 400，或在任何业务输出提交前出现 `response.failed` 且错误为 `context_length_exceeded` 时：

1. 保留第一个 system/developer item 和最近约 200 KiB 的上下文。
2. 最多把较早的 512 KiB 文本送到同一 Provider、同一账号做低 reasoning 摘要；单 item 文本最多取 8 KiB。
3. 用摘要替换早期上下文并修复 orphan tool call/output；摘要失败时改用明确的省略标记。
4. 使用原 URL、鉴权、workspace、session、模型和 `ProviderExecution` 重试原请求一次。

自动压缩至少要求 4 个 input items，摘要超时为 120 秒。最新单个 input item 超过约 200 KiB 时保守跳过，因为只压缩旧上下文通常无法恢复该请求。一次顶层请求最多压缩一次，内部摘要不会递归触发 compact，也不会调用顶层 Router。已经提交业务输出的 SSE 不压缩、不回放。

WebSocket 生命周期及其 HTTP fallback 不在自动压缩范围内：握手或传输错误仍按上一节的 transport fallback 规则处理，但上游接受 `response.create` 后的语义失败不会自动 compact 或重放。需要自动恢复时应使用 POST Responses/Chat 或 Responses SSE；也可由客户端显式调用 `/responses/compact`。

内部摘要是一次真实模型调用，其 usage 单独记录为 `dataSource=codex_overflow_compact_summary`；最终重试继续记录原请求 usage。运维应把该额外 token 消耗纳入容量和审计评估。

## OAuth 刷新与持久化

OAuth refresh 在账号单飞锁内完成，并在发布新 token 前持久化完整候选 `accounts.json`。如果旋转 token 已到达提交点但 durable write 失败，Server 保留内存中的新 token、进入 credential persistence degraded，并后台指数退避重试。

- `/ready` 在 degraded 时返回 `503`。
- 新 Codex OAuth 数据面、quota、models、alpha search 和 Provider test 不再出站。
- 旧的成功重试不能清除更新一代持久化失败。
- `refresh_token_reused` 立即隔离账号；不会通过切换账号继续请求。

## 验收清单

- 0/1/2 个 Codex OAuth 账号分别得到 `unconfigured`、自动 `ready`、`needs_selection`。
- `needs_selection` 时 HTTP、SSE、WS、Images、models、alpha search、quota 和 Provider test 的上游请求数均为零。
- 选择活动账号后，所有未共享 Codex OAuth Provider 原子重绑并增加 revision；进程重启后选择保持。
- Share 冲突返回 409，`accounts.json`、`providers.json` 和 `shares.json` 不出现部分提交。
- current Provider 和显式 Provider 都只能使用活动账号；并发饱和、cooldown、quota 耗尽及第二次 401 时其他 Provider/账号上游请求数为零。
- HTTP、SSE、Images、WS 握手和 WS 到 HTTP fallback 的首次 401 都只刷新同一账号一次。
- WS 只在 `response.create` 成功发送前 transport fallback；发送后的 read/close/首事件超时不重放。
- overflow compact 默认关闭；开启后仅 HTTP Responses/Chat 和 Responses SSE 重试一次，摘要 usage 独立记录，摘要失败使用省略标记，超大最新 item 与 WebSocket lifecycle 不重放。
- credential persistence degraded 时 `/ready` 为 503 且所有 Codex OAuth credentialed surface 零上游请求。

真实 OAuth、订阅权限、图片能力和长连接生产可用性仍需要按 [`real-acceptance-runbook.md`](real-acceptance-runbook.md) 使用专用测试账号取证；离线测试不能替代真实上游验收。

## 非目标

- 不实现多账号池、轮询、权重、session affinity 选账号或按负载选择账号。
- 不实现 quota spillover、cooldown 换号、401 换号或跨 Provider 自动故障转移。
- 不允许用 Provider endpoint override、环境变量或请求 header 改写 OAuth/JWKS/ChatGPT 生产 trust boundary。
- 不保证上游账号自身的订阅、风控、地域和模型 entitlement。
