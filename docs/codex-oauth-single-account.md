# Codex OAuth 单账号反代

本文描述 cc-switch-server 对 OpenAI/Codex OAuth 的生产边界、账号绑定、数据面行为和验收方式。目标是把 Provider Bundle 明确绑定的 ChatGPT OAuth 账号稳定暴露给 Codex/OpenAI-compatible 客户端；不提供账号池、轮询、权重调度、配额溢出或跨账号故障转移。

## 能力边界

- 文本入口包括 Route Key 下的 `POST /r/:routeKey/v1/responses`、`POST /r/:routeKey/v1/chat/completions` 及兼容别名。
- 原生 compact 入口包括 `POST /r/:routeKey/v1/responses/compact` 及兼容别名。
- Responses WebSocket 使用 `GET /r/:routeKey/v1/responses` 及兼容别名。
- Codex 专用 surface 包括 models manifest、alpha search、图片生成和图片编辑。
- 本地直连推理入口使用独立 inference token；经过 Router 签名验证的请求沿用 Router 身份。
- 每个 `codex_oauth` Provider Bundle 必须绑定一个明确的 `codex_oauth` Account。
- 一次请求从开始到结束只使用解析出的 Provider、账号和 workspace；任何错误都不能触发通用 Provider failover 或账号轮换。

单账号边界用于避免重复执行、重复计费、会话漂移，以及 OAuth token、workspace、WebSocket 连接或 prompt cache identity 在账号之间串用。

## 账号中心选择

`accounts.json` 持久化 `activeCodexOauthAccountId`。`GET /api/accounts` 的 `codexOauth` 字段和 Web 兼容命令 `auth_get_status` 的 `codex_oauth` 字段都返回以下状态：

| 状态 | 含义 | 账号中心 |
| --- | --- | --- |
| `unconfigured` | 没有 Codex OAuth 账号 | 无默认操作目标 |
| `ready` | 已解析出唯一活动账号 | quota 等独立账号操作使用该账号 |
| `needs_selection` | 存在多个账号但没有有效的显式选择 | 要求先选择账号中心操作目标 |

只有一个账号时，该账号自动成为账号中心操作目标，不要求额外写入选择。存在两个或更多账号时，管理员可在 Web 中选择账号中心目标，或调用：

```http
POST /api/accounts/codex/active
Content-Type: application/json

{"accountId":"<codex-oauth-account-id>"}
```

兼容命令 `auth_set_default_account` 的 Codex 分支执行同一操作。该选择不参与数据面路由，也不会改变任何 Provider Bundle 或 Share binding。

## 与 Provider 解耦

选择账号中心目标时，Server：

1. 验证目标账号存在且类型为 `codex_oauth`。
2. 只更新并持久化 `accounts.json` 中的 `activeCodexOauthAccountId`。
3. 不修改 `providers.json`、Provider revision、RuntimePlan 或 Share binding。
4. 不改变任何 Route Key 的执行身份。

数据面执行身份只能在 Provider Bundle 编辑器中通过账号绑定变更。Route Key 和 Share 请求始终使用各自 Bundle 已提交的账号，即使账号中心选择了另一个 Codex 账号也不会漂移。

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

1. 从 `/r/:routeKey` 解析唯一的 Codex Surface；Share 请求使用 Share 自身的不可变 binding。
2. 编译后的 RuntimePlan 必须与已提交的 Provider revision、类型和账号身份代际一致。
3. 对 `codex_oauth` Driver，解析 Provider Bundle 明确绑定的账号。
4. 检查账号登录、cooldown、quota 和并发状态，并获取该账号的 in-flight lease。
5. 对即将过期的 token 执行同账号 refresh，再物化 Authorization、workspace 和 Codex CLI identity headers。

Route Key 不存在、Surface 已禁用、绑定过期、账号需要重登、处于 cooldown、quota 耗尽或并发饱和时，请求直接失败。系统不会查询另一个 Codex Provider 或账号。账号中心是否选择 active account 不影响该判定。

models manifest、alpha search、Provider 网络测试、模型发现、Images、HTTP、SSE、WebSocket 和 WS 到 HTTP fallback 使用同一 Bundle 绑定账号规则。账号中心 quota refresh 可独立使用 active account；credential persistence degraded 时，需要 OAuth 凭据的出站操作在网络前返回 `503`。

## HTTP、SSE 与 Images

- Responses/Chat 请求先执行 Codex body sanitizer、model capability 和官方 CLI identity contract。最终出站契约始终覆盖为 `store=false`；除原生 compact 外，OpenAI OAuth Responses 上游始终使用 `stream=true`，不信任 Claude/Codex/Gemini 入站适配器或客户端传入的这两个字段。
- 客户端 `stream=true` 时继续得到协议转换后的 SSE；客户端 `stream=false` 时，Server 增量消费同一上游 SSE，在收到合法终止事件后聚合为单个 Responses JSON 文档。终止事件中的 usage 同时写入本地日志和 Router 同步；解析错误、缺终止事件、上游失败、超时或断流也各自写入一条终态日志并完成 Share/Provider outcome 收尾，不把“上游必须流”误报成“客户端请求了流”。
- FAST 完全由 Provider 的 `codexFastMode` 控制：客户端的 `service_tier`/`serviceTier` 不能开启或关闭它。推理等级仍由客户端选择，OpenAI `reasoning.effort`/`reasoning_effort`、Claude `output_config.effort`/`thinking.effort` 和 Gemini `generationConfig.thinkingConfig.thinkingLevel` 的显式值均记录为 requested effort，转换后的最终出站值记录为 effective effort；`low`、`medium`、`high`、`xhigh`、`max` 保持不变，仅把非 wire 别名 `ultra` 规范为 `max`。
- 首次 401 只允许对原账号强制 refresh 一次，再以同一 Provider、账号、workspace、session 和请求 body 重放。
- 第二次 401、429、403、5xx、网络错误或流内错误都不能切换 Provider/账号。
- 非流式 `response.failed` 和 SSE semantic failure 会保留 OpenAI 错误语义；已向下游提交业务输出后绝不透明重放完整生成。
- Responses Lite、custom/freeform tool、`tool_search`、usage 四桶和空 `response.completed.output` 恢复使用同一执行身份。
- Images generation/edit 使用固定 Codex bridge、身份头和 body 上限；401 重放后仍返回原始上游错误 body，不用另一个账号掩盖错误。
- models manifest 与 alpha search 只访问固定 ChatGPT Codex endpoint，并采用同账号一次 401 refresh 边界。

下游流式请求先创建 `usageState=pending` 的日志；终止后更新为 `observed`、`missing`、`parse_error` 或 `interrupted`，并递增 `usageRevision`。下游非流但上游被强制为 SSE 的请求直接创建一条同语义的终态日志。显式观测到的全零 usage 仍是 `observed`，与未收到 usage 严格区分。Router 只接受同一 `requestId` 的相同或更高 revision，避免迟到的 pending 覆盖终态。

### Images 兼容与资源边界

Codex OAuth 图片桥覆盖 Route Key 下的 `POST /v1/images/generations`、`POST /images/generations`、`POST /v1/images/edits` 和 `POST /images/edits`。它把 OpenAI Images 请求转换成同一绑定账号上的 Responses `image_generation` tool 调用；上游始终使用增量 SSE，generation 与 edit 分别回放为 `image_generation.*` 和 `image_edit.*` 事件。`n` 当前只接受 `1`，不会用未验证的多图语义制造重复生成或重复计费。

- 输入模型只接受 `gpt-image-*`；`gpt-image-2-2k` 和 `gpt-image-2-4k` 会规范化为 `gpt-image-2` 及对应方向尺寸。
- `response_format` 支持 `b64_json` 和 `url`；同时校验 `size`、`quality`、`background`、`output_format`、`moderation`、`input_fidelity`、`output_compression`、`partial_images` 和 `stream` 的类型、范围及组合关系。
- edit 支持 JSON image URL/data URI 和 multipart `image`/`image[]`/`mask`。每张输入图最多 20 MiB、所有输入图和 mask 合计最多 32 MiB、input image 最多 16 张、mask 最多 1 张；MIME 必须与 PNG/JPEG/GIF/WebP/BMP/AVIF 签名一致。为容纳 32 MiB 图片经 base64 后的膨胀和 multipart 元数据，Codex Images HTTP decoded envelope 上限为 48 MiB；图片解码后的聚合上限仍为 32 MiB。共享 Images 路由上的 Grok media wire/decoded body 继续限制为 32 MiB。
- 远程图片只允许 HTTP(S)，逐次校验 redirect、DNS 全部解析结果和最终 MIME/signature，阻止私网、loopback、保留地址、协议降级与 DNS rebinding，并使用 10 秒单图、30 秒批次和 4 路并发上限。
- 单张解码后输出最多 48 MiB，Codex Images 成功流累计最多 72 MiB，以容纳 48 MiB 图片的 base64 膨胀和 SSE/JSON 元数据。输出必须是合法 base64，且实际 PNG/JPEG/WebP 签名必须与上游声明格式一致；能识别的图片会记录实际宽高，无法识别尺寸时仍保留上游 size 元数据。

下游 `stream=true` 会立即发送 `: connected`，空闲期间每 15 秒发送 `: keepalive`；partial、completed 和 error 按 Images SSE 事件输出。`stream=false` 仍保持一个合法 JSON 文档：先发送 JSON 合法空白，并每 15 秒发送空白心跳，最终再写入 JSON 对象。这样 Cloudflare 可以持续看到源站字节，而 Server 不需要把整个生成结果缓冲到内存后才响应。

首个有效上游事件使用 `STREAM_FIRST_BYTE_TIMEOUT_MS`，之后使用 `STREAM_IDLE_TIMEOUT_MS`。`response.failed`、`response.incomplete`、cancel/error 事件、缺失终止事件的 EOF、超限、网络错误和超时都不会记为 Provider 成功。客户端中断会记为 HTTP `499`/`client_cancelled`，并在 usage 与 Share 终态记账完成后归还账号与 Share in-flight lease。`CC_SWITCH_PROXY_SEMANTIC_GUARD_ENABLED=0` 只回滚普通 Responses 的语义提交门禁，不关闭图片传输的最小 lifecycle/terminal 检查。

由于首个空白或 SSE comment 已提交 HTTP headers，后续流内失败不能再把 wire status 从 `200` 改成 `502/504`：stream 模式返回 `event: error`，JSON 模式返回标准 error JSON；本地 usage log 的 `statusCode`、`streamStatus` 和 `errorMessage` 记录真实终态。调用方不能只看初始 HTTP 200 判断图片成功，必须消费完整 Body 并验证 completed/data 或 error。

### `response_format=url`

URL 模式不会返回伪造或上游私有 URL。Server 将解码后的图片放入持久化 capability store，并返回同源 `/v1/images/files/<256-bit-token>`：

- token 具有 256-bit 随机熵，GET/HEAD 不再要求 inference token；token 本身就是短期访问能力。
- TTL 为 1 小时，最多 128 项、合计 256 MiB；达到上限会淘汰最旧项。默认目录是 `<config-dir>/image-capabilities`，也可用 `CC_SWITCH_IMAGE_STORE_DIR` 指定绝对路径或相对 config dir 的路径。过期、孤立、元数据损坏、长度或 SHA-256 不匹配的条目会在启动、读写或命中校验时清理。
- payload 先以 `0600` 数据文件原子落盘，随后才提交带长度、MIME、SHA-256 和过期时间的元数据；所有读写通过同一锁文件串行化。因此同一目录中的 URL 可跨进程重启继续使用，也可由挂载该目录的其他副本读取。
- 多副本共享目录必须支持跨进程 advisory file lock、同一文件系统内的 atomic rename 和目录同步，并让所有副本以兼容的文件权限访问。无法满足这些条件时，每个副本使用独立 store，并对生成请求及后续 GET/HEAD 配置粘性回源。
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
- 允许 `/v1/images/files/<token>` 的匿名 GET/HEAD 到达共享同一 `CC_SWITCH_IMAGE_STORE_DIR` 的任一 Server 副本；副本没有共享目录时才要求回到生成实例。不要把 inference bearer 追加到生成 URL，也不要缓存 capability 响应。
- 确认 WAF、请求体和上传规则允许 Codex Images 的 48 MiB HTTP envelope（解码图片聚合仍为 32 MiB）；真实验证首块、15 秒以上生成、URL 下载和客户端取消。

Cloudflare 524 是否消失、Worker 是否实际 flush 小 chunk、订阅账号是否拥有 image entitlement，仍是外部部署/真实 OAuth gate，离线测试不能替代。

`/metrics` 暴露 capability insert/hit/miss/expiry/corruption/eviction、当前条目与字节数，以及 Responses/Images/Grok 图片传输的首字节、心跳次数和最大静默时间。共享目录下的 size gauge 是各进程最近一次扫描到的 store 快照，不是跨副本聚合值。

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

- 0/1/2 个 Codex OAuth 账号分别得到 `unconfigured`、自动 `ready`、`needs_selection`，这些状态只约束账号中心操作。
- `needs_selection` 不阻断具有完整账号绑定的 Route Key 或 Share 数据面。
- 选择账号中心目标后，Provider、Share、revision 和 RuntimePlan 均保持不变；进程重启后账号中心选择保持。
- Route Key 和 Share 都只使用各自 Bundle 的绑定账号；并发饱和、cooldown、quota 耗尽及第二次 401 时其他 Provider/账号上游请求数为零。
- HTTP、SSE、Images、WS 握手和 WS 到 HTTP fallback 的首次 401 都只刷新同一账号一次。
- Claude/Codex/Gemini 经 OpenAI OAuth 转出的普通 Responses 最终出站均为 `store=false`、`stream=true`；客户端非流请求得到单个 JSON，成功终止 SSE usage 记录为 `observed` 而不是零值占位，失败聚合则按 `missing`、`parse_error` 或 `interrupted` 记录并保留已明确观测到的 usage。
- Router 收到 pending 后可由更高 `usageRevision` 的终态覆盖；低 revision 重放不能回退状态，显式 observed zero 与 missing/parse error/interrupted 在 API 和 UI 中保持可区分。
- WS 只在 `response.create` 成功发送前 transport fallback；发送后的 read/close/首事件超时不重放。
- overflow compact 默认关闭；开启后仅 HTTP Responses/Chat 和 Responses SSE 重试一次，摘要 usage 独立记录，摘要失败使用省略标记，超大最新 item 与 WebSocket lifecycle 不重放。
- credential persistence degraded 时 `/ready` 为 503 且所有 Codex OAuth credentialed surface 零上游请求。

真实 OAuth、订阅权限、图片能力和长连接生产可用性仍需要按 [`real-acceptance-runbook.md`](real-acceptance-runbook.md) 使用专用测试账号取证；离线测试不能替代真实上游验收。

## 非目标

- 不实现多账号池、轮询、权重、session affinity 选账号或按负载选择账号。
- 不实现 quota spillover、cooldown 换号、401 换号或跨 Provider 自动故障转移。
- 不允许用 Provider endpoint override、环境变量或请求 header 改写 OAuth/JWKS/ChatGPT 生产 trust boundary。
- 不保证上游账号自身的订阅、风控、地域和模型 entitlement。
