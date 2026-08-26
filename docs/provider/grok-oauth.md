# Grok OAuth 单账号反代

本文描述 cc-switch-server 对 Grok/xAI OAuth 的生产边界、信任模型、数据面流程和验收方式。目标是把一个 Grok Provider 明确绑定的单个 OAuth 账号稳定暴露为 OpenAI Responses 兼容入口；不提供账号池、轮询、权重调度、配额溢出或跨 Provider 故障转移。

## 能力边界

- 文本入口使用 Router Share URL 下的 `POST /v1/responses` 和 `POST /v1/chat/completions`。
- Responses WebSocket 使用同一 Share URL 下的 `GET /v1/responses`。
- 媒体入口包括图片生成/编辑和视频生成/状态查询；这些能力按账号 fail closed。
- 模型目录通过同一 Share URL 下的 `GET /v1/models` 返回，并附带 Grok catalog 的来源和新鲜度。
- Models、Responses HTTP/WS、Chat 和媒体入口都要求 Router 签名验证且必须携带 Share 身份。Server 不接受本地推理 token，也不提供 Provider 专属公开路径。
- 每个 `grok_oauth` Provider 必须绑定一个明确的 `grok_oauth` Account。
- 同一个生成请求只允许使用该 Provider 的绑定账号；任何错误都不能触发账号轮换或通用 Provider failover。

单账号边界用于避免重复执行、重复计费、会话漂移，以及 OAuth token、conversation id 或 turn 在账号之间串用。

## OAuth 与身份信任边界

支持浏览器 PKCE、device code 和显式粘贴 `~/.grok/auth.json` 三种导入方式。Server 不自动扫描或写入宿主机用户目录。

- 授权请求使用固定 loopback callback `http://127.0.0.1:56121/callback`、96 字节 PKCE verifier、nonce，以及 `openid profile email offline_access grok-cli:access api:access conversations:read conversations:write workspaces:read workspaces:write` scopes。
- Device start/poll 使用与数据面相同的 Grok CLI version，并发送 `x-grok-client-surface: ui`；poll 与普通 code exchange/refresh 复用受策略约束的 token URL。
- Device start 的 2xx 响应只有在 `device_code`、`user_code` 和 `verification_uri` 去除首尾空白后均非空时才被接受；空的 `verification_uri_complete` 会被省略。缺失或为零的 poll interval/expires-in 分别按 5 秒/30 分钟归一化，并限制在 5 分钟/30 分钟上限内。
- 新登录、device flow 和 auth.json 导入都必须提供已签名 ID token。Server 通过 xAI OIDC discovery/JWKS 严格校验 ES256、EC P-256 JWK、`kid`、issuer、audience、expiry/nbf 和登录 nonce。
- 本地账号 ID 从已验证的 `sub` 稳定派生。email、display name、token 文本和未验证 profile 字段都不是 principal。
- Refresh 可不返回新 ID token，但仅限账号已保存 verified subject；若返回新 ID token则必须重新验签，且 subject 必须与原账号一致。
- Device flow 完成结果中的 token 只保留到账号成功 durable 写入；写入成功后立即删除 flow 与 principal binding，写入失败则保留完成结果供同一 principal 重试。
- 同一 verified subject 重新登录时更新 token 和已验证 claims，但保留此前持久化的 Grok capability evidence。
- Discovery、JWKS、authorize 和 token endpoint 在生产构建中固定为经审计的 `auth.x.ai` HTTPS URL。测试构建可使用 loopback；生产环境变量和 Provider 都不能注入其他 OAuth、WebSocket 或 model-catalog host。
- 所有 OAuth/OIDC JSON 都按累计响应体大小读取：discovery 上限 64 KiB、device 上限 256 KiB、JWKS 和通用 token/profile 上限 1 MiB；超限响应在解析前 fail closed。

账号 token 继续由共享的加密 `accounts.json` 存储处理。控制面响应只暴露凭据是否存在和脱敏状态，不返回 access、refresh 或 ID token。

## Provider 与账号固定

一次 Grok Share 请求按以下顺序解析执行身份：

1. 校验 Router ingress 签名，根据 Share binding 的应用协议 Surface 解析一个 Provider Bundle。
2. 编译后的 RuntimePlan 必须绑定 `oauth.grok_responses` Driver。
3. 从 Provider 的 managed-account binding 解析唯一账号。
4. 检查账号登录、cooldown、配额和并发状态，并获取该账号的 in-flight lease。
5. 对即将过期的 token 执行同账号 refresh，然后物化 Authorization 和 CLI identity。

Provider 不存在、绑定缺失、账号不可用、并发饱和或处于 cooldown 时请求直接失败。即使仓库中存在第二个健康的 Grok Provider 或账号，也不会成为候选。

旧配置只要存在有效的 Grok account binding，该 binding 也是 authoritative managed OAuth 身份；残留的 `OPENAI_API_KEY` 不能把请求降级成静态凭据，也不能关闭 CLI endpoint/header contract。Grok inference endpoint 对新 profile 和 legacy compatibility plan 都固定为官方 `https://api.x.ai/v1`，配置中的 base URL override 会被忽略并产生 runtime warning。

## HTTP 与 SSE

HTTP 和 SSE 共用同一份 Grok request contract：

- OpenAI Chat Completions 先无损规范化为 Responses，请求上游固定使用 Grok CLI `/v1/responses`；非流式和 SSE 再恢复为 Chat contract。Grok 数据面不再向上游发送 `/v1/chat/completions`。
- Provider 的 single-model policy 先决定候选上游模型，默认 `grok-4.5`；随后由 Grok contract 对候选别名做最终规范化，例如 `grok-composer` 变为 `grok-composer-2.5-fast`。
- 出站使用 `Authorization: Bearer`、`x-xai-token-auth`、`x-grok-client-identifier`、`x-grok-client-version`、`x-authenticateresponse`、Grok CLI User-Agent 和稳定的 `x-grok-conv-id`。
- 账号 `extraHeaders` 不能覆盖 Authorization、CLI identity、conversation/cache identity、turn、accept/content-type 或 hop-by-hop header；发现冲突配置时请求 fail closed，而不是静默采用账号值。
- Responses body 会清理不受支持的字段，并校验 reasoning、tool 和 `encrypted_content` 形状。Codex Responses Lite 的 `additional_tools` 接受可选的规范 `role=developer`，随后把工具提升到 xAI 顶层 tools；完全相同的声明会去重，同名不同定义、其他 role、未知字段或无法无损映射的工具仍在本地 `422` fail closed。
- Claude Messages 的客户端 function tools 会转换为 xAI Responses 的扁平声明（顶层 `name` / `description` / `parameters`），Anthropic hosted web search 会转换为 xAI `web_search`，不会把 Chat Completions 专用的嵌套 `function` 对象发给 xAI。
- 普通 Responses HTTP/WS body 的 `prompt_cache_key` 由 Server 强制绑定到隔离后的 conversation id，客户端值不能覆盖；compact 请求只保留会话 header，body 必须省略 `prompt_cache_key`。
- 首次 401 允许对原账号强制 refresh 一次，再用新 Authorization 重放原请求；第二次 401 直接返回并只冷却原账号。
- 429、403、5xx、网络错误或流内错误都不能触发跨 Provider/账号重放。
- SSE 已向下游提交业务事件后不会透明重放完整请求。
- first-event deadline 只由完整业务/终态事件满足；SSE comment、ping、空 data、生命周期事件和部分 JSON 字节不续命。content 与 reasoning delta 分开做有界重复输出保护，HTTP/SSE、WebSocket 和 WS→HTTP fallback 使用同一阈值合同，触发后终止当前请求且不切换账号或 Provider。

OAuth 凭据发生轮换时，Server 先原子持久化候选账号快照，再发布内存状态。若 durable write 失败，新 token 会保留在内存并由后台退避重试，但 Grok 新数据面请求和 WebSocket 会返回 `503`，`/ready` 同时进入 degraded，避免重启后继续使用未持久化的旋转凭据。

## Hosted Search 响应语义

Grok Responses 的 hosted `web_search_call` 和 `custom_tool_call`/`x_search` 是服务端工具，不是需要客户端执行的普通 function tool：

- 流式和非流式桥都输出 Anthropic `server_tool_use`，随后输出匹配的 `web_search_tool_result` 或 `x_search_tool_result`。
- 事件缺失 `output_index` 时按 item id 关联 added/done/input 事件，避免把并行搜索结果附着到错误 content block。
- `url_citation` annotation 转为 `citations_delta` 或非流式 citation content，保留 URL、标题和可用的文本位置。
- `usage.server_tool_use.web_search_requests` 记录 hosted search 总数，`x_search_requests` 单独记录 X 搜索数。
- Hosted search 已由上游完成，不把 Anthropic `stop_reason` 改成 `tool_use`；只有需要下游执行的普通 function call 才使用该终态。

该转换只改变当前 Grok 响应的协议表示，不触发第二次搜索、不重放请求，也不进入 Provider/账号 failover。

## Turn 与会话

`x-grok-turn-idx` 是纯下游输入，不是 Server 状态：

- 只接受不超过 20 位、可解析为十进制 `u64` 的 header 值。
- 缺失、负数、带符号、空白、溢出或含非数字字符时完全省略上游 header，不返回客户端参数错误。
- Server 不生成、不缓存、不自增，也不从请求次数推断 turn。
- 同账号 401 重放和 WebSocket 到 HTTP fallback 复用请求开始时解析出的同一个 optional turn。

普通模型使用客户端 `x-grok-conv-id`、`x-session-id` 或请求 session metadata；缺失时生成随机 conversation id。租户/Share 场景会进行命名空间隔离。每个新的下游 HTTP `grok-composer-*` 请求都会忽略客户端 session 并生成新的 conversation id，避免错误复用 composer 会话；该请求内部的同账号 401 重放必须原样复用已经生成的 id。WebSocket 的 conversation id 在握手时确定，并在该下游连接上的所有串行 `response.create` lifecycle 以及每次可能发生的 HTTP fallback 中保持不变；需要新的 composer conversation 时必须建立新的下游 WebSocket。

## WebSocket 与 HTTP Fallback

Grok Responses WebSocket 使用固定 `wss://api.x.ai/v1/responses`，并复用 Provider、账号、会话、turn 和 in-flight lease。

- 握手首次 401 时只强刷绑定账号一次，重新物化 Authorization 后重连。
- connect/timeout、握手 5xx、stale socket、首业务事件前 send/read 错误和 close 1009 可回退到同账号 HTTP/SSE。
- 握手 400/401/403/429、已提交业务事件后的错误以及 idle timeout 不进行传输 fallback。
- fallback 请求保持原 Provider、账号、conversation id、turn 和 single-model policy。
- 裸 Responses body、nested `response.create.response` 和 flat `response.create` 使用同一清洗合同：删除 WS 不适用的 stream/background 字段、强制 `store=true`，并在带 `previous_response_id` 时删除重复 instructions。
- `websocket` capability 未验证时在连接上游前返回 `503`。
- 握手 HTTP 响应与普通 HTTP/媒体响应一样更新 entitlement header evidence，并把 403/5xx cooldown 只记录到绑定账号。

生产中 WebSocket 上游不可配置。仅 `cfg(test)` 的 Driver option `testGrokWebsocketUrl` 可指向 loopback mock。

## 媒体能力

图片生成、图片编辑和视频生成分别对应 `image_generation`、`image_edit`、`video_generation` capability。默认都不开放：

1. 运维可通过 `CC_SWITCH_GROK_OAUTH_CAPABILITIES` 显式启用一个待验证能力。
2. 成功的真实上游响应会把 `supported` evidence 持久化到绑定账号。
3. 后续可移除显式开关，由持久化 evidence 继续开放能力。

媒体首次 401 同样只允许原账号强刷一次。视频创建成功后，request id 的 durable binding 写入 `grok-media-tasks.json`，固定 Provider、账号、`authIdentityGeneration`、Share、runtime、用户命名空间、TTL 和 `upstreamPlane`；它不是账号调度机制。schema v1 的历史任务只有一个 direct-XAI endpoint，因此精确迁移为 `xai` plane；未知 schema/plane fail closed。状态查询只读取创建时的 binding，Provider 重绑、身份代际、runtime 或 plane 变化均返回 `409`，重启后不会丢失有效绑定。

媒体请求复用文本/WS 的 CLI identity family；客户端显式提供的 `x-grok-conv-id` 在 Share/user 边界内做同样的命名空间隔离。媒体 POST 的 wire body 和逐层 gzip/deflate 解码结果都使用 32 MiB 硬上限，避免 Axum 默认 2 MiB 误拒合法图片，同时阻止压缩膨胀绕过内存边界。媒体上游 wire response 和逐层解压结果使用 64 MiB 硬上限，避免 base64 图片响应形成无界内存读取。视频状态 request id 只接受 1-128 字节 ASCII 字母、数字、`-` 和 `_`，禁止把路径、query 或 fragment 注入固定上游 URL。

Grok 图片是 direct-XAI 实验能力，不代表 Build OAuth 官方保证。图片 multipart edit 最多接受 3 张图，每张都复用统一 image primitive 校验非空、大小、允许 MIME 和 magic bytes；声明 MIME 不匹配、未知签名、超数量以及未经验证的 `mask`/`quality`/`size`/`style` 均在出站前返回 4xx，不再静默丢弃。图片请求强制 `Accept-Encoding: identity`。成功响应拿到 headers 后，SSE 立即提交 `: connected` 并按完整事件边界转发，JSON 先提交合法空白、完整缓冲并校验一个 JSON 文档；两种模式空闲时都每 15 秒发送心跳，且逐块执行 64 MiB 上限。首个 comment/空白提交后 wire status 固定为 `200`，后续读失败、超限或 JSON 无效只能返回流内 error，不能透明换 Provider 或替换 HTTP 状态；Provider/Share 终态记账仍使用实际结果，客户端必须消费完整 Body。

视频创建在出站前执行本地 DTO 校验：模型、prompt、1..15 秒 duration、aspect ratio、720p/1080p、reference 数量/互斥和 reference 最高 720p 均有稳定 4xx；`video`、`output`、`storage_options` 在透明代理模式明确拒绝。本版本维持 direct-XAI `Xai` plane，不启用无真实 entitlement/evidence 支持的 Build→XAI fallback。阶段 5 本地 worker、upload callback 和媒体归档评审结论为 **no-go**：透明代理无需持有本地媒体任务或资产，后续只有上游强制 upload callback 或产品明确要求归档时才独立立项。

## 模型目录

Share URL 下的 `GET /v1/models` 使用该 Share 的 Codex Surface 绑定账号 access token 请求固定的 Grok CLI models endpoint：

- 缓存按账号隔离，默认 TTL 为 300 秒，可通过 `CC_SWITCH_GROK_MODELS_TTL_SECONDS` 在 1 秒到 24 小时范围内调整。
- 支持 ETag/304；成功目录记录抓取时间。
- 上游失败且有缓存时返回 last-known-good，并标记 `stale=true`。
- 没有可用缓存时返回静态 `grok-4.5` fallback。
- 上游目录响应体上限为 1 MiB，超限按上游失败处理。
- entry 支持纯字符串以及 `id`、`model`、`modelId`、`name`、`_meta.model`、`_meta.modelId`，按该优先级选取标识；`hidden=true` 或 `_meta.hidden=true` 不对外发布。
- 顶层元数据 `source`、`stale`、`fetchedAtMs` 用于区分 upstream、fresh cache、304、last-known-good 和 static fallback。

模型目录降级不会绕过 single-model policy，也不会选择另一个 Grok 账号。credential persistence degraded 时不会访问上游目录，只返回明确来源的静态 fallback；刷新前已 degraded 和本次 refresh 因旋转 token 落盘失败而刚进入 degraded 都执行同一零上游门禁。生成数据面仍返回 `503`。

Share models 和管理端 Provider 模型发现都只接受已提交 RuntimePlan 中 driver 为 `oauth.grok_responses` 的 `ManagedAccount` 引用，并要求 Provider revision、账号类型和 `authIdentityGeneration` 全部匹配。Provider 未绑定账号、仅配置 legacy API key、绑定缺失/类型错误、RuntimePlan 过期或账号身份代际变化时，只返回 `static_fallback`，不会刷新任意账号或访问 models 上游。

不存在不带 Share 身份的公共模型列表；未签名或签名但无 Share 的 `/v1/models` 请求分别返回 `401` 或 `403`。

## 重放矩阵

| 场景 | 允许动作 | Provider/账号 |
| --- | --- | --- |
| HTTP JSON/SSE 首次 401 | 强制 refresh 后重放 1 次 | 原 Provider、原账号 |
| 媒体首次 401 | 强制 refresh 后重放 1 次 | 原 Provider、原账号 |
| WS 握手首次 401 | 强制 refresh 后重连 1 次 | 原 Provider、原账号 |
| WS 首业务事件前的受支持传输错误 | HTTP/SSE fallback 1 次 | 原 Provider、原账号 |
| 第二次 401、403、429、5xx | 返回并记录原账号状态 | 永不切换 |
| SSE/WS 已提交业务事件后中断 | 终止当前流 | 永不重放 |
| capability 未验证 | 上游零请求，返回 503 | 永不切换 |
| credential persistence degraded | 上游零请求，返回 503 | 永不切换 |

## 运行配置与观测

| 变量 | 作用 |
| --- | --- |
| `CC_SWITCH_SERVER_XAI_CLIENT_ID` | 覆盖 xAI public client id；通常留空 |
| `CC_SWITCH_GROK_CLI_VERSION` | CLI version，默认 `0.2.111` |
| `CC_SWITCH_GROK_CLI_USER_AGENT` | 完整 CLI User-Agent 覆盖 |
| `CC_SWITCH_GROK_MODELS_TTL_SECONDS` | 模型目录缓存 TTL，默认 300 秒，范围 1-86400 秒 |
| `CC_SWITCH_GROK_OAUTH_CAPABILITIES` | 逗号分隔的 capability bootstrap，或 `all` |

重点监控：

- `/ready` 和 `cc_switch_credential_persistence_degraded`：OAuth rotation 是否已 durable。
- `cc_switch_provider_outcome_total`：固定 Provider 的 success、401、429、5xx 和 network outcome。
- `cc_switch_forward_retry_total`：同账号 auth/transport retry。
- `cc_switch_codex_websocket_fallback_total`：Responses WS fallback 的 source/result。
- `cc_switch_grok_cli_version_gate_total`：上游 CLI version gate。
- `cc_switch_grok_model_catalog_total{source}`：目录来源与降级频率。
- 账号 quota/cooldown、in-flight/max 和 warm-refresh 指标。

日志和 evidence 只能记录 Provider id、脱敏账号、状态码、request id、模型、catalog source 和时间，不能记录 token、raw OAuth/JWKS 响应或完整上游错误体。

## 真实账号验收

先确认待测 Share 的 Grok Provider Bundle 只绑定待测账号，再运行：

```bash
CC_SWITCH_SHARE_URL='https://share.example.com' \
ROUTER_API_TOKEN='<router-user-token>' \
node scripts/smoke/grok-oauth-real.mjs
```

可选变量：

- `CC_SWITCH_GROK_MODEL`：覆盖默认 `grok-4.5`。
- `CC_SWITCH_GROK_MEDIA_SMOKE=1`：额外执行一次短图片生成；运行前必须已显式 bootstrap 或持久化 `image_generation` evidence。
- `CC_SWITCH_REAL_TIMEOUT_MS`：单请求超时，范围 1 秒到 5 分钟。
- `EVIDENCE_FILE=/tmp/...json`：写入脱敏结果摘要。

脚本依次通过同一个 Share URL 检查 models 元数据、Responses JSON 和 Responses SSE，并对两个 Responses 请求携带固定 session id 与合法 `x-grok-turn-idx`。缺少 Share URL 或 Router token，或者变量仍为占位符时，脚本输出 `SKIP` 并退出 0；这只表示真实验收未运行。

401 强刷、WS handshake/fallback、429/cooldown、version gate 和“不跨 Provider”需要受控上游故障或抓包环境，不能由正常成功 smoke 证明，按 `docs/acceptance/real-acceptance-runbook.md` 单独留证。

## 非目标与剩余风险

- 不实现多账号调度、轮询、权重、健康 failover 或 quota spillover。
- 不实现 grok.com Web cookie 反代，不迁移 Grok Web、Tauri、Skill、MCP 或 Desktop 行为。
- 不允许生产配置任意 OAuth、WebSocket、models 或 inference upstream。
- 本地 mock 测试不能证明真实 xAI OAuth、订阅权限、模型、媒体、WebSocket 和限流语义可用。
- Capability evidence 证明某账号曾成功使用能力，不保证其订阅未来始终保有该能力；真实 403/429 和 entitlement 变化仍需告警与人工处理。
- persistence degraded 期间若宿主机在重试成功前崩溃，内存中的最新旋转 token 可能丢失；fail-closed readiness 和数据面门禁只能缩短风险窗口，不能替代可靠磁盘。
