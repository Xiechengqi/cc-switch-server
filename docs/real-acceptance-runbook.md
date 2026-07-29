# 真实验收运行手册

本手册只记录变量名、最小权限、执行顺序和脱敏规则，不保存真实 token、账号、OAuth raw response 或 provider secret。

## 安全边界

- 真实密钥只放在 shell 环境或 `/tmp/cc-switch-server-real.env` 这类私有临时文件中。
- 仓库内只允许提交 `.env.example` 的占位符。
- 记录验收结果时只写 URL、token prefix、状态码、requestId、脱敏 email 和时间；不要写 token 明文、refresh token、raw provider response。
- 真实 provider 测试使用短 prompt、固定模型、固定 expected status，不跑大输入、不压测。
- OAuth 能力必须等真实账号 non-stream/stream、refresh、错误路径都回归后才能把 capability 从 `manual_token_store` 切到 NativeOAuth。

## 环境文件

复制占位文件到临时路径：

```bash
cp .env.example /tmp/cc-switch-server-real.env
chmod 600 /tmp/cc-switch-server-real.env
```

填入真实值后加载：

```bash
set -a
source /tmp/cc-switch-server-real.env
set +a
```

加载后先做脱敏自检：

```bash
scripts/smoke/real-acceptance-env-check.sh
STRICT=1 scripts/smoke/real-acceptance-env-check.sh
```

## 推荐顺序

静态验证（不编译、不部署、不启动服务）：

```bash
scripts/static-checks.sh
```

完整本地验证（会运行 `cargo check/test` 并通过 `cargo run` 启动本地 server）：

```bash
scripts/audit/validate-local.sh
scripts/smoke/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

真实 router/market/provider 输入齐备后：

```bash
STRICT=1 scripts/smoke/real-acceptance-env-check.sh
RUN_PROBES=1 STREAM_PROBE=1 scripts/smoke/direct-market-diagnostics.sh
scripts/smoke/router-market-smoke.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
scripts/smoke/oauth-readiness-check.sh
node scripts/smoke/grok-oauth-real.mjs
RUN_REAL=1 scripts/release-readiness.sh
```

## Provider Store Migration Acceptance

Do not run the write steps against a production data directory until the Server
has been stopped and a complete directory backup exists.

1. Run `cc-switch-server --config-dir "$CONFIG_DIR" config migrate-provider-store`
   while the service is running. It must be read-only and report S1/S2 format,
   blocker count, key source, and RuntimePlan parity without changing
   `providers.json` or creating `accounts.key`.
2. For an eligible S1 fixture, stop the Server and run the same command with
   `--apply`. Confirm the guarded S2 file contains no known plaintext Provider
   credential, every Provider recompiles to the same RuntimePlan, and the S1
   snapshot remains under `provider-migrations/s1-to-s2/`.
3. Attempt `--apply`, `--rollback`, and `--cleanup-snapshot` while another Server
   owns the data-directory lock. Each write action must fail before changing a
   live file.
4. Stop the Server and run `--rollback`; confirm the exact S1 bytes are restored
   and the previous bridge binary can parse them. Re-apply S2 before continuing
   forward acceptance.
5. Stage an S2 backup with a wrong/missing root key. Restore must fail before live
   replacement. With the matching `accounts.key` or
   `CC_SWITCH_SERVER_ACCOUNTS_ENCRYPTION_KEY`, restore must decrypt credentials
   and compile every Provider RuntimePlan before replacement.
6. Do not run `--cleanup-snapshot` or remove compatibility readers until
   `assets/contract/provider-compatibility-window.json` records two stable bridge
   releases and at least 14 observation days.

Record only format, counts, blocker codes, key source category, short reference
fingerprint, and pass/fail state. Never record an envelope, root key, or plaintext
credential.

## 必需变量

### server 基础

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `SERVER_URL` | 被测 server base URL | 可完整记录 |
| `CC_SWITCH_SERVER_TOKEN` | server 登录 bearer token | 不记录明文，只记录是否存在 |
| `CC_SWITCH_BASE_URL` | OAuth 真实账号 smoke 使用的 server base URL | 可完整记录 |
| `CC_SWITCH_INFERENCE_TOKEN` | 真实推理入口 token | 不记录明文，只记录是否存在 |

### router/market public probe

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `ROUTER_BASE_URL` | 真实 router base | 可完整记录 |
| `ROUTER_API_TOKEN` | direct public share URL 和 market API URL 的调用 token | 只记录 prefix |
| `ROUTER_API_TOKEN_HEADER` | `Authorization`、`x-api-key` 或 `x-goog-api-key` | 可完整记录 |
| `DIRECT_SHARE_URL` | share tunnel public URL，不带 `/v1/responses` | 可完整记录 |
| `SHARE_ID` | server 本地 share id | 可完整记录 |
| `MARKET_API_URL` | market API base，不带 `/v1/responses` | 可完整记录 |
| `MARKET_API_TOKEN` | market 专用用户 API key，可选 | 不记录明文 |
| `MARKET_API_TOKEN_HEADER` | market 专用 header，可选 | 可完整记录 |
| `PROBE_MODEL` | 低成本 probe 模型，默认 `probe` | 可完整记录 |
| `STREAM_PROBE` | `1` 时执行短 stream probe | 可完整记录 |
| `REQUIRE_STREAM_USAGE` | `1` 时 stream 摘要必须看到 usage 字段才算通过 | 可完整记录 |

### provider 和 OAuth

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `CLAUDE_PROVIDER_TOKEN` | Claude app/provider 真实低成本回归 | 不记录明文 |
| `CODEX_PROVIDER_TOKEN` | Codex app/provider 真实低成本回归 | 不记录明文 |
| `GEMINI_PROVIDER_TOKEN` | Gemini app/provider 真实低成本回归 | 不记录明文 |
| `CODEX_OAUTH_TEST_ACCOUNT` | Codex OAuth Plus/Pro 测试账号 | 记录脱敏 email |
| `CLAUDE_OAUTH_TEST_ACCOUNT` | Claude OAuth 测试账号 | 记录脱敏 email |
| `GEMINI_OAUTH_TEST_ACCOUNT` | Gemini OAuth/CLI 测试账号 | 记录脱敏 email |
| `GROK_OAUTH_TEST_ACCOUNT` | Grok/xAI OAuth 测试账号 | 记录脱敏 email |
| `CURSOR_OAUTH_TEST_ACCOUNT` | Cursor OAuth 测试账号 | 记录脱敏 email |
| `ANTIGRAVITY_OAUTH_TEST_ACCOUNT` | Antigravity/Agy OAuth 测试账号 | 记录脱敏 email |
| `CODEX_OAUTH_REFRESH_TOKEN_FIXTURE` | Codex OAuth 手动导入 refresh token fixture | 不记录明文 |
| `CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE` | Claude OAuth 手动导入 refresh token fixture | 不记录明文 |
| `GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE` | Gemini OAuth/CLI 手动导入 refresh token fixture | 不记录明文 |
| `GROK_OAUTH_REFRESH_TOKEN_FIXTURE` | Grok OAuth 手动导入 refresh token fixture | 不记录明文 |
| `GROK_OAUTH_AUTH_JSON_FIXTURE` | 显式粘贴的 Grok auth.json fixture | 不记录内容或路径中的账号信息 |
| `GROK_OAUTH_CALLBACK_URL` | Grok 固定 loopback callback，默认 `http://127.0.0.1:56121/callback` | 可完整记录 |
| `CC_SWITCH_GROK_PROVIDER_ID` | 只绑定待测账号的 Grok Provider id | 可完整记录 |
| `CC_SWITCH_GROK_MODEL` | Grok 单模型验收值，默认 `grok-4.5` | 可完整记录 |
| `CC_SWITCH_GROK_MEDIA_SMOKE` | `1` 时额外运行短图片生成 | 可完整记录 |
| `CURSOR_OAUTH_REFRESH_TOKEN_FIXTURE` | Cursor OAuth 手动导入 refresh token fixture | 不记录明文 |
| `ANTIGRAVITY_OAUTH_REFRESH_TOKEN_FIXTURE` | Antigravity/Agy OAuth 手动导入 refresh token fixture | 不记录明文 |
| `CURSOR_API_KEY_FIXTURE` | Cursor API Key 真实验收 fixture | 不记录明文 |
| `GITHUB_COPILOT_TEST_ACCOUNT` | GitHub Copilot device flow 测试账号 | 记录脱敏 email/账号名 |
| `GITHUB_COPILOT_GITHUB_DOMAIN` | GitHub 或 GHES 域名，默认 `github.com` | 可完整记录 |
| `GITHUB_COPILOT_TOKEN_FIXTURE` | Copilot/GitHub 已导入 token fixture | 不记录明文 |
| `KIRO_TEST_ACCOUNT` | Kiro/AWS Builder ID 测试账号 | 记录脱敏 email |
| `KIRO_REGION` | Kiro device flow region，默认 `us-east-1` | 可完整记录 |
| `KIRO_START_URL` | Kiro/AWS SSO start URL | 可完整记录 |
| `KIRO_REFRESH_TOKEN_FIXTURE` | Kiro 已导入 refresh token fixture | 不记录明文 |
| `AWS_REGION` | Bedrock region | 可完整记录 |
| `AWS_ACCESS_KEY_ID` | Bedrock AKSK access key | 只记录是否存在 |
| `AWS_SECRET_ACCESS_KEY` | Bedrock AKSK secret key | 不记录明文，只记录是否存在 |
| `AWS_SESSION_TOKEN` | Bedrock 临时 session token，可选 | 不记录明文，只记录是否存在 |
| `BEDROCK_MODEL_ID` | Bedrock Claude model id | 可完整记录 |

OAuth refresh fixture 的最小验收顺序：

1. 用私有 env 中的 refresh token fixture 导入账号，确认账号页显示 `ready` 或 `expires soon`，且不泄漏 token。
2. 执行账号手动 refresh，记录状态码、脱敏账号，并确认 token 轮换成功后 `lastRefreshError` 为空；profile/quota enrichment 通过独立 quota refresh 验收。
3. 绑定 provider 到该账号，清空或过期 access token 后发起本地 share 短请求，确认 proxy 转发前自动 refresh。
4. 再跑同一 provider 的 non-stream 和 stream 短请求，记录 requestId、status、actualModel、usage 摘要。
5. direct/market 入口只记录 URL、状态码、requestId 和脱敏账号，不记录 provider raw response。

Codex OAuth 专项补充：

1. Device Code 的 start/poll/cancel 必须绑定发起登录的管理员主体和 device-code 有效期，另一管理员不能 poll/cancel。同一 `device_code` 并发 poll 时只允许一个上游 exchange，其余返回 pending；完成后重复 poll 返回同一账号结果，cancel 后必须失效。
2. 新登录和 refresh 的 ID/access token 必须通过 OpenAI JWKS 的 RS256、issuer、各自 audience、expiry/nbf 校验；合并身份必须同时含非空 `subject` 和 `chatgpt_account_id`，冲突或缺失任一字段均 fail closed。轮换 `kid` 时应刷新缓存，未知 `kid` 必须拒绝。
3. 同一 refresh token 导入第二个账号必须拒绝；模拟 `refresh_token_reused` 时账号应立即进入 relogin，不等待普通 invalid-grant 阈值。
4. 抓包确认 HTTP、WebSocket、Images 的 `originator` 与 User-Agent family 匹配，`version` 不低于 `0.144.0`；默认应为 `0.144.1`。
5. 本地账号 ID 必须由已验证 user subject 稳定派生，同 subject 重登应原子复用旧记录；workspace 只能选择 token claims 中的 organization/Account-ID，不能作为本地 principal。修改后出站 `ChatGPT-Account-Id` 应随选择变化，伪造 ID 必须被控制面拒绝。
6. Responses Lite 请求应覆盖 `additional_tools`、custom tool call/output continuation、tool_search forced choice 和同名冲突错误；Chat 上游回程应恢复 custom item，stream 完成事件包含非空 output。
7. SSE 与 WebSocket 分别模拟空 `response.completed.response.output`，确认按 `output_index` 重建；已有非空 output 不覆盖，第二个 response 不得串入前一轮状态。
8. provider 的 `codexWebsocketEnabled=false` 应使 GET WS 返回 503，并保持 POST Responses SSE 可用；恢复开关后再跑 text/binary WS 与 Windows reset 场景。
9. GPT-5.6 Sol/Terra 接受 `ultra`，Luna 将 `ultra` 降为 `max`，旧 GPT 将 `max/ultra` 降为 `xhigh`；`/v1/models` 应返回 Sol/Terra/Luna。
10. usage fixture 同时覆盖 nested `cache_write_tokens`、cache read、cache creation 显式零值和 Anthropic exclusive input，核对 fresh/read/write/output 四桶与总 Token。
11. `/v1/images/generations` 使用短图片 prompt 验证既有 Codex bridge、身份头和账号冷却；不要把已有 Images 路由误报为未实现。
12. server 不应自动读取或写入运行主机用户的 `~/.codex/auth.json`；只测试显式登录/导入。TLS/JA3 只有在 rustls 请求出现可重复的上游拒绝证据时才开启专项评估。
13. 从配置中的非 loopback HTTPS Client URL 发起 CLI OAuth，确认授权请求仍使用 `http://localhost:1455/auth/callback`。浏览器本地回调失败后提交完整地址栏 URL 应完成同一管理员主体的会话；裸 code、`127.0.0.1`、错误端口/path、重复 state、过期/取消会话、另一管理员会话、非同源页面、未配置的 host 和远程 HTTP Client URL 都必须拒绝。另以 `0.0.0.0` 或 `::` 启动 Server，确认携带伪造 `Host: 127.0.0.1` 的远程请求仍被拒绝；只有 Server 实际绑定 loopback 时才允许本机例外。Device OAuth 同时保持可用。
14. Provider 中伪造 OAuth authorize/token、quota 或 inference endpoint 后保存/转发必须被固定 endpoint policy 拒绝或覆盖，OAuth token 不得发往自定义 host；managed OAuth Provider 缺少显式账号绑定时必须拒绝保存，不能隐式选同类型第一个账号。
15. `GET /api/accounts`、账号 upsert/refresh/quota 响应及兼容 invoke 响应不得包含 access/refresh/id token、API key、extra headers、profile、raw 或 refresh error 原文；只允许 `has*`/状态/配额/脱敏身份字段。
16. HTTP non-stream、SSE、Images、image-tool 去除后的二次请求、WebSocket handshake 与 WS→HTTP fallback 分别模拟首次 401：同一账号只强刷一次并重物化 Authorization/workspace header；仍为 401 时直接返回并只记录原账号 cooldown。current Provider、显式 `x-cc-provider-id` 和 Share binding 都不得跨 Provider/账号；为其他 Provider/账号设置 mock 后断言其上游请求数为零。
17. 分别以 0、1、2 个 Codex OAuth 账号启动，确认 `GET /api/accounts`/`auth_get_status` 返回 `unconfigured`、自动 `ready`、`needs_selection`。`needs_selection` 时 HTTP/SSE/WS/Images/models/alpha-search/quota/Provider test 均应零上游请求。调用 `POST /api/accounts/codex/active` 后，所有未共享 Codex OAuth Provider 必须原子重绑到目标账号并增加 revision，重启后选择保持；另创建引用旧账号 Provider 的 Share，确认切换返回 409 且三个 store 无部分变化。把活动账号置于 cooldown、quota 耗尽和并发上限时请求应直接失败，其他账号/Provider 请求数始终为零，SSE/WS 结束或断连后原账号 inflight 必须归零。
18. 同一 Codex session 连续两个 WS response 应只建立一个上游连接；更换 Provider/runtime/workspace/credential 必须生成新 pool key。用 `CC_SWITCH_CODEX_WS_CACHE_MAX_CONNECTIONS`、`CC_SWITCH_CODEX_WS_CACHE_IDLE_MS`、`CC_SWITCH_CODEX_WS_CACHE_MAX_AGE_MS` 缩短参数验证 capacity/idle TTL/max age，并验证 `codexWebsocketEnabled=false` 的禁用行为。
19. 分别模拟 WS connect refused/timeout、握手 5xx、stale cached socket 和发送 `response.create` 前的 send failure，确认仅这些阶段可通过同账号 HTTP/SSE 回退；握手 400/401/403/429 不得作为传输 fallback。成功发送 `response.create` 后再模拟 read failure、close 1009 和缩短 `STREAM_FIRST_BYTE_TIMEOUT_MS` 导致的首事件超时，均应只终止流且不重放；缩短 `STREAM_IDLE_TIMEOUT_MS` 验证首业务事件后的空闲超时同样不重放。`cc_switch_codex_websocket_fallback_total{source,result}` 与 cache/retry 指标应对应增加。
20. 保持 `CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT` 未设置或为 `0`，模拟 HTTP 400 和 SSE `response.failed/context_length_exceeded`，确认无内部摘要和重放。设置为 `1` 后确认同一 Provider/账号按“原请求 → 摘要 → 压缩后请求”最多执行一次，摘要 usage 的 `dataSource=codex_overflow_compact_summary`；摘要失败应使用省略标记继续一次，压缩后再次 overflow 直接返回。首个业务事件前的 SSE overflow 可压缩，已提交业务事件后的 overflow 必须保持原错误且绝不重放。

Grok OAuth 单账号专项补充：

1. 待测 `grok_oauth` Provider 必须显式绑定一个账号。另建一个绑定不同账号的 Grok Provider 作为负向候选；无论 HTTP、SSE、媒体、WebSocket、429、5xx 或 refresh 失败，负向 Provider/账号的请求数都必须保持为零。
2. 浏览器 PKCE、device start/poll/cancel 和显式 auth.json 导入分别验收。新登录、device 和 auth.json 缺失 ID token、`alg=none`、伪造签名、错误 issuer/audience/nonce、过期/nbf 或未知 `kid` 均必须 fail closed；不得把 email、display name 或未验证 token payload 当作 principal。
3. Refresh 不返回新 ID token 时，只允许已有 verified subject 的账号继续；返回新 ID token 时必须重新验签并拒绝 subject 变化。轮换后的 token 必须先 durable commit，落盘失败期间 `/ready`、HTTP/SSE、媒体和 WS 都应以 degraded/503 阻止新 Grok 流量。
4. 授权请求必须包含 `workspaces:read`、`workspaces:write` 和既有 Grok CLI/conversation scopes。Device start/poll 必须携带 `x-grok-client-version` 与 `x-grok-client-surface: ui`；生产 code exchange、device poll、refresh、OIDC discovery 和 JWKS 必须使用固定的官方 `auth.x.ai` HTTPS URL。生产环境变量或 Provider 伪造 OAuth、WS、models 或 inference host 必须无效或被固定覆盖。
5. 抓包确认 HTTP/SSE、媒体、WS handshake 和 WS→HTTP fallback 使用同一 CLI identity family，默认 version/User-Agent 为 `0.2.111`，并携带一致的 `Authorization`、`x-xai-token-auth`、client identifier/version 和 conversation id。给绑定账号配置同名 `extraHeaders` 必须在零上游请求下 fail closed；legacy Provider 即使残留 `OPENAI_API_KEY` 也必须继续使用 managed token 和 CLI endpoint，配置 inference base URL 必须被固定策略忽略。
6. 对 Responses JSON 和 SSE 发送固定 `x-session-id` 与合法十进制 `x-grok-turn-idx`，确认上游值完全一致。缺失、负数、带符号、空白、非数字、超过 20 位和 `u64` 溢出都应在上游完全省略；Server 不生成、不缓存、不递增 turn，也不因非法值返回 4xx。
7. HTTP non-stream 和 SSE 分别模拟首次 401，确认只强刷绑定账号一次，重放使用新 Authorization，同时 conversation id、turn、Provider id 和 model 不变。第二次 401 直接返回并只冷却原账号，不进入通用 Provider failover。
8. `websocket` 未验证时 GET Responses WS 必须在零上游请求下返回 503。显式 bootstrap 后模拟握手首次 401，再模拟首事件前 close 1009 或 connect/5xx，确认强刷及 HTTP/SSE fallback 始终复用原 Provider、账号、conversation id、turn、single-model policy 和 in-flight lease；握手 400/401/403/429 与首业务事件后的错误不得 fallback。分别发送裸 body、nested 和 flat `response.create`，确认三者统一删除 stream/background、强制 `store=true`，且 continuation 不重复发送 instructions；握手 403/5xx 必须更新绑定账号 cooldown，entitlement headers 必须持久化。
9. `image_generation`、`image_edit`、`video_generation` 未验证时必须各自 fail closed。仅在 `CC_SWITCH_GROK_OAUTH_CAPABILITIES` 明确 bootstrap 对应能力后执行短请求；成功证据持久化后移除开关重启，能力应继续开放。媒体首次 401 只强刷原账号一次，视频状态查询保持创建时的 Provider/账号 sticky identity。验证大于 2 MiB 且不超过 32 MiB 的图片编辑可进入 handler，wire body、gzip/deflate 任一解码层或最终 decoded body 超过 32 MiB 都返回 413；视频 request id 中的 `/`、`?`、`#`、percent escape、空值、超长值必须在零上游请求下返回 400。
10. 对 HTTP、SSE、媒体和 WS 分别注入 429 与 reset/retry hints，确认只更新绑定账号的 cooldown/rate-limit outcome，保留下游审计允许的限流头且不跨 Provider。403/5xx/network failure 同样不能授权账号切换。
11. `/v1/models?app=codex&providerId=<id>` 必须返回选定模型及 `source`、`stale`、`fetchedAtMs`。依次验收 upstream、TTL fresh cache、ETag 304、过期后的 last-known-good 和无缓存 static fallback；所有目录请求只使用已提交 RuntimePlan 中 revision/类型/身份代际一致的 managed binding。未绑定、stale generation、stale plan 或 degraded persistence 时 token 和 models 上游请求数都必须为零。
12. 将 Grok CLI version 降到已知不受支持值并触发上游 version gate，确认下游错误改写为面向管理员的 `CC_SWITCH_GROK_CLI_VERSION` / `CC_SWITCH_GROK_CLI_USER_AGENT` 指引，raw token/账号不泄漏，`cc_switch_grok_cli_version_gate_total` 增加；恢复默认 `0.2.111` 后重测。
13. Quota 抓包同时覆盖 user、weekly、monthly、task usage 和 subscriptions。`currentPeriod.end`、`billingPeriodEnd`、token expiry 及 inactive subscription 不能成为订阅到期日；仅 active subscription 的明确 expiry 或账号手工 next-payment 值可进入 UI，且不影响凭据有效性和路由。
14. 运行 `node scripts/smoke/grok-oauth-real.mjs` 验收 readiness、models metadata、Responses JSON 和完整 SSE terminal。只有显式设置 `CC_SWITCH_GROK_MEDIA_SMOKE=1` 才运行图片；缺少 base/token/provider 或仍为占位符时的 `SKIP` 只能记录为 blocked-inputs，不能记录为真实通过。
15. 检查 `/metrics` 中 Provider outcome、forward retry、WS fallback、CLI version gate、model catalog、账号 in-flight/max、warm refresh 和 persistence degraded 指标；labels 和 evidence 只含有界分类、Provider id、模型和脱敏账号，不得包含 access/refresh/ID token 或 raw OAuth/upstream body。

Claude OAuth 专项补充：

1. 同一 `claude_oauth` 账号并发触发多次 refresh 时，上游 token endpoint 不应收到重复风暴；失败后短窗口内应进入 per-token backoff。
2. 新建 Claude 授权 URL 必须包含 `prompt=login`，避免多账号浏览器会话抢占。
3. Claude proxy 请求应携带 CLI header set、基于首条 user 文本稳定合成的 `x-claude-code-session-id`，并在无客户端 `metadata.user_id` 时注入 server 合成值。
4. `anthropic-beta` 应按请求形状出现：基础请求只带 Claude Code/OAuth beta；含 `thinking`、streaming tools 或 computer-use tool 时才追加对应 beta；messages 与 profile/usage 请求的 Claude CLI UA 应保持同一版本，CCH `cc_entrypoint` 默认应为 `cli`。
5. 上游 429 时应记录当前 Provider 的 rate-limited outcome，并原样保留审计过的 rate-limit 响应头。Claude Messages/count_tokens、Share 和显式 `x-cc-provider-id` 请求均不得切换 Provider 或账号；当前账号的 429 直接返回。
6. Claude SSE 中出现 `event:error` 且类型为 `rate_limit_error`、`overloaded_error` 或 `api_error` 时，应记录当前 Provider failure；无论 error 位于下游 commit 前后都不得透明重放或切换账号，已开始输出的流以 Anthropic 终止错误帧结束。
7. 非 Claude Code 客户端请求应被改写为 billing/identity system blocks，原 system 迁移到首条 user message，并重算 CCH。
8. 上游 400 signature/thinking 错误应触发反应式降级重试：thinking block 降为 text；工具签名错误时 tool_use/tool_result 降为 text；web_search 历史块错误时剥离历史 server_tool_use/web_search_tool_result。
9. `CC_SWITCH_CCH_SALT_HEX`、`CC_SWITCH_CLI_STAINLESS_OS`、`CC_SWITCH_CLI_STAINLESS_ARCH`、`CC_SWITCH_CLI_STAINLESS_RUNTIME_VERSION` 覆盖应只用于灰度/抓包追热；默认路径应按账号 seed 稳定选择 stainless OS/arch，stream 请求 `x-stainless-timeout=600`，非 stream 请求为 `60`。
10. 长闲置 Claude OAuth 账号应由后台 60s 维护循环提前 warm-refresh；真实回归可把 access token 置空或调短 `expiresAt`，确认首个 proxy 请求前账号已恢复可用或只触发一次 singleflight refresh。
11. 若上游返回 Claude Code CLI 版本过期提示，响应体应替换为面向 cc-switch-server admin 的 `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA` 调整提示，并记录 error 日志。
12. Claude OAuth 出站 JSON 不应被 key 字母序化；抓包时至少确认原始 `model` / `max_tokens` / `messages` 相对顺序被保留，缺省工具请求应补 `tools: []`。
13. 上游响应含 `x-request-id` 时，下游客户端应能拿到同名 header，便于 Anthropic support 联合排查。
14. Claude OAuth 客户端 header 中加入未知 beta（例如 `prompt-caching-scope-2026-01-05`）时，上游不得收到该 token；已审计的 `prompt-caching-2024-07-31` 与 `token-efficient-tools-2025-02-19` 应保留，server debug 日志应能定位被过滤事件但不得记录 token/account 身份。
15. 同一 OAuth state 在多 tab 重复完成时应返回同一 completed/account 结果；Pending/preview session 可通过 `/api/accounts/login/cancel` 或 `auth_cancel_login` 幂等取消，取消后 finish/poll 必须终止，未知 state 必须拒绝。exchange 已开始后 cancel 应返回冲突，避免授权码已消费但账号未持久化。
16. Claude OAuth 直连只使用 `currentProviderClaude` 的单一绑定账号；默认并发上限为 8，provider 的 `ACCOUNT_MAX_CONCURRENT` / `MAX_CONCURRENT_REQUESTS` 可覆盖，`CC_SWITCH_ACCOUNT_MAX_CONCURRENT=0` 可关闭。达到上限时即使存在其他 Claude Provider/账号也必须返回 429，SSE 结束或中断后容量必须释放。
17. 如使用 `~/.claude/.credentials.json` 迁移，只通过显式 `POST /api/accounts/claude/credentials/import` 导入；server 不自动扫描本机目录、不写 Claude Desktop profile，也不通过控制面提供明文凭据导出。
18. 缺省 `max_tokens` / `temperature` 的请求应分别补为 `128000` / `1`；thinking 请求强制 `temperature=1` 并删除冲突的 `top_p`/`top_k`，非 thinking 显式 sampling 保持不变。
19. `POST /v1/messages/count_tokens` 与 `/claude/v1/messages/count_tokens` 应只选择 `claude`、`claude_auth`、`claude_oauth`；OAuth 抓包应包含 token-counting beta、无 generation 字段且 CCH 对最终 body 有效。Codex/Gemini/OpenRouter provider 必须被拒绝，成功响应的 `input_tokens` 原样返回且不产生生成 usage。
20. Responses/Chat 上游转 Anthropic stream 时，使用两个并行工具和 packed `function_call_arguments.done` 验证每个 block 只 start/stop 一次、arguments 不丢不重；分别以 CRLF、多事件同 chunk、JSON 每个切分点和 EOF 半帧注入，已输出后的错误不得重放请求。
21. profile refresh 后 `organization.billing_type` 应进入 `profile.billingSource`；Apple/Stripe 不应改变 plan 或生成订阅到期日，未知 billing type 应原样保留。
22. 连续 `invalid_grant` 达到 `CC_SWITCH_REFRESH_FAILURES_BEFORE_RELOGIN` 阈值后，账号应显示 `relogin` 并退出其固定 Provider 内的账号调度；网络错误、限流和普通 quota 错误不得累计该计数，手工 refresh 成功后状态应清零。
23. `GET /metrics` 应能看到账号 inflight/max、Claude retry、Provider outcome、warm-refresh、CLI version-gate、beta decision、count_tokens outcome 与 stream protocol error 指标；labels 必须保持固定枚举。该端点默认无鉴权，公网部署必须由反向代理或网络策略限制抓取来源。

Grok 的真实输入作为独立 external gate 接入环境检查：缺失时不阻断本地 release readiness，也绝不能宣称真实通过。Cursor/Copilot/Kiro/Bedrock 的真实验收变量继续由 AB7 gate 管理。所有变量齐备都只代表可以开始真实验收；non-stream、stream、usage、错误路径全绿前，不得提升 native capability。Router 内建 Share Market entitlement 的真实验收属于 Router/Market 集成边界，server 只验证 pending share edit 的签名、幂等应用、只读 managed grant 和 ack；详见 [`router-market-acceptance.md`](router-market-acceptance.md)。

## 脱敏 Evidence

以下脚本支持 `EVIDENCE_FILE=/tmp/...json`，只写脱敏摘要：

- `scripts/smoke/real-acceptance-env-check.sh`
- `scripts/smoke/router-market-smoke.sh`
- `scripts/smoke/direct-market-diagnostics.sh`
- `scripts/smoke/code-agent-regression.sh`
- `scripts/smoke/oauth-readiness-check.sh`
- `scripts/smoke/grok-oauth-real.mjs`
- `scripts/release-readiness.sh`

检查 evidence 是否包含密钥形态：

```bash
scripts/audit/evidence-redaction-check.sh /tmp/cc-switch-server-evidence/result.json
```
