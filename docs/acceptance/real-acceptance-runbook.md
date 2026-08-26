# 真实验收运行手册

本手册只记录变量名、最小权限、执行顺序和脱敏规则，不保存真实 token、账号、OAuth raw response 或 provider secret。

## 安全边界

- 真实密钥只放在 shell 环境或 `/tmp/cc-switch-server-real.env` 这类私有临时文件中。
- 仓库内只允许提交 `.env.example` 的占位符。
- 记录验收结果时只写 URL、token prefix、状态码、requestId、脱敏 email 和时间；不要写 token 明文、refresh token、raw provider response。
- 真实 provider 测试使用短 prompt、固定模型、固定 expected status，不跑大输入、不压测。
- Codex Images 探针是显式例外：默认 `all` 会发起四次高质量 4K 生成并产生真实费用，只有确认账号、预算和 Cloudflare 被测路径后才设置 `CC_SWITCH_CODEX_IMAGES_SMOKE=1`。
- OAuth 能力必须等真实账号 non-stream/stream、refresh、错误路径都回归后才能把 capability 从 `manual_token_store` 切到 NativeOAuth。
- 缺少经授权的真实 OAuth 账号、订阅 entitlement 或生产反代路径时，对应项目只能记录为 `live_pending`/blocked；本地 mock、fixture、smoke 或 readiness 通过不能升级为真实通过。

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
RUN_TESTS=1 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

`RUN_TESTS=0` 仅用于负向审计：脚本会记录 `local-contracts-unverified`，输出 `decision=blocked` 并以状态码 `1` 退出；不得将其作为本地合同或发布验收通过证据。

真实 Router/Gateway/provider 输入齐备后：

```bash
STRICT=1 scripts/smoke/real-acceptance-env-check.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/router-share-smoke.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
scripts/smoke/oauth-readiness-check.sh
node scripts/smoke/grok-oauth-real.mjs
CC_SWITCH_CODEX_IMAGES_SMOKE=1 node scripts/smoke/codex-images-real.mjs
RUN_REAL=1 scripts/release-readiness.sh
```

Images 探针覆盖 dedicated SSE、dedicated JSON `b64_json`、dedicated JSON `url` 和 Responses image tool；它检查首块/最大静默、严格 base64、图片签名、SHA-256，以及经 Router Share 鉴权的 capability HEAD/GET 长度、MIME、`no-store` 和 `nosniff`。输入就绪或脚本启动不等于真实通过，必须保留脱敏成功摘要；不要提交生成图片或 capability token。

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

## Share 配额手工修正验收

以下步骤覆盖「已消耗 Token」两层修正：per-user grant 配额与 Share 总量。二者是
不同层级，验收时必须分别确认，不能用其中一个的结果推断另一个。

1. per-user：在供应商远程分享的「用户限制」中给某用户选择「每 7 天」并把周期开始
   时间填成过去的某个 UTC 时间点（不得晚于当前时间，精确到分钟）。保存后确认返回的
   `usageQuota.windowStartsAtMs` / `windowEndsAtMs` 与该锚点一致，且 `effectiveTokensUsed`
   由历史请求重建而不是清零。
2. per-user：把「已消耗 Token」设为高于观测值的数字并保存，确认 `usageRebase.targetTokens`
   等于该值、`usageRebase.appliedBy` 记录的是当前已验证管理员邮箱、`usageQuota.manualOffsetTokens`
   等于 `effective - observed`。随后发起一次真实请求，确认新增量是**累加**在该基线上。
3. per-user：把目标值填成低于观测历史的数字，必须被 `cc_switch_share_usage_target_below_observed`
   拒绝；并发编辑同一 grant 必须出现 `cc_switch_share_user_grant_revision_conflict`；
   Router Share Market 托管的 grant 必须返回 `cc_switch_share_market_grant_read_only`。
4. per-user：等待或人为跨过一个固定周期边界后刷新，确认旧基线不再生效
   （`usageQuota.rebaseApplies` 为 false 且 `manualOffsetTokens` 归零），配额不会被上个
   周期的修正继续压制。
5. Share 总量：在同一个已存在的 Share 上编辑「已消耗 Token」总量。它与 per-user 配额
   相互独立——Share 总量是纯累加器，永远不从 Usage 历史重建，因此这里是**直接赋值**。
   确认未触碰该字段的保存不会覆盖期间产生的真实消耗。
6. Share 总量：把总量设到 Token 限额之上，确认 Share 变为 `exhausted` 且被禁用；再设回
   限额之下，确认状态回到 `paused` 且**仍然是禁用**，必须由操作者显式恢复。
7. 重置：对该 Share 执行 reset usage，确认每个 grant 的派生快照与操作者基线一并清除；
   之后的请求或刷新都不得让旧基线复活。
8. Provider 官方配额（Accounts/OAuth 上游订阅与限流状态）是**外部数据**，Server 无法
   rebase，也不提供本地修改入口。验收只确认它可以从上游刷新并如实展示；任何"看起来
   可以改"的入口都属于缺陷。

以上每一步都需要真实 Router/Share/请求输入。缺少真实输入时只能记录本地验证结果，
不得标记为真实通过。

## 必需变量

### server 基础

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `SERVER_URL` | 被测 server base URL | 可完整记录 |
| `CC_SWITCH_SERVER_TOKEN` | server 登录 bearer token | 不记录明文，只记录是否存在 |
| `CC_SWITCH_IMAGE_STORE_DIR` | 多副本共享的 durable capability 目录；必须支持跨进程锁与 atomic rename | 只记录挂载类别，不记录宿主机敏感路径 |
| `CC_SWITCH_CODEX_IMAGES_SMOKE` | `1` 时显式允许付费的 4K Images/Responses 探针 | 可完整记录 |

### Router/Gateway public probe

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `ROUTER_BASE_URL` | 真实 router base | 可完整记录 |
| `ROUTER_API_TOKEN` | Router Share/Gateway API 的调用 token | 只记录 prefix |
| `ROUTER_API_TOKEN_HEADER` | `Authorization`、`x-api-key` 或 `x-goog-api-key` | 可完整记录 |
| `CC_SWITCH_SHARE_URL` | 同时承载 Claude/Codex/Gemini 协议的 Router Share URL，不带 API path | 可完整记录 |
| `SHARE_ID` | server 本地 share id | 可完整记录 |
| `PROBE_MODEL` | 低成本 probe 模型，默认 `probe` | 可完整记录 |
| `STREAM_PROBE` | `1` 时执行短 stream probe | 可完整记录 |
| `REQUIRE_STREAM_USAGE` | `1` 时 stream 摘要必须看到 usage 字段才算通过 | 可完整记录 |
| `RUN_CONTRACT_TESTS` | code-agent evidence 的合同测试门禁；真实验收必须为 `1` 且全部通过 | 可完整记录 |
| `MATRIX_LIVE_EVIDENCE_FILE` | 每个 code-agent case 必需维度的脱敏通过清单；缺失时不得标记 `live_verified` | 仅记录路径，不提交私有 evidence |

### provider 和 OAuth

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `CLAUDE_PROVIDER_TOKEN` | Claude app/provider 真实低成本回归 | 不记录明文 |
| `CODEX_PROVIDER_TOKEN` | Codex app/provider 真实低成本回归 | 不记录明文 |
| `GEMINI_PROVIDER_TOKEN` | Gemini app/provider 真实低成本回归 | 不记录明文 |
| `CODEX_OAUTH_TEST_ACCOUNT` | Codex OAuth Plus/Pro 测试账号 | 记录脱敏 email |
| `CLAUDE_OAUTH_TEST_ACCOUNT` | Claude OAuth 测试账号 | 记录脱敏 email |
| `CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT` | Claude Max 5x OAuth 专项账号；缺失时 5x 等级验收必须记录为 blocked-inputs | 只记录脱敏 email 和计划显示名 |
| `CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT` | Claude Max 20x OAuth 专项账号；缺失时 20x 等级验收必须记录为 blocked-inputs | 只记录脱敏 email 和计划显示名 |
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
| `CC_SWITCH_GROK_MODEL` | Grok 单模型验收值，默认 `grok-4.6` | 可完整记录 |
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
5. Router Share/Gateway 入口只记录 URL、状态码、requestId 和脱敏账号，不记录 provider raw response。

Codex OAuth 专项补充：

1. Device Code 的 start/poll/cancel 必须绑定发起登录的管理员主体和 device-code 有效期，另一管理员不能 poll/cancel。同一 `device_code` 并发 poll 时只允许一个上游 exchange，其余返回 pending；完成后重复 poll 返回同一账号结果，cancel 后必须失效。
2. 新登录和 refresh 的 ID/access token 必须通过 OpenAI JWKS 的 RS256、issuer、各自 audience、expiry/nbf 校验；合并身份必须同时含非空 `subject` 和 `chatgpt_account_id`，冲突或缺失任一字段均 fail closed。轮换 `kid` 时应刷新缓存，未知 `kid` 必须拒绝。
3. 同一 refresh token 导入第二个账号必须拒绝；模拟 `refresh_token_reused` 时账号应立即进入 relogin，不等待普通 invalid-grant 阈值。
4. 抓包确认 HTTP、WebSocket、Images 的 `originator` 与 User-Agent family 匹配，`version` 不低于 `0.144.1`。Token 换票 / refresh / Device 流应带同一官方 `originator` + User-Agent，且不发 `version`。清空版本缓存后验证内置 `0.144.1`；用 mock GitHub latest release 验证更高稳定三段 semver 原子落盘并在重启后生效，低版本、prerelease、build metadata、前导零、超大 body、超时和损坏缓存均不得降低或阻断服务；合法且不低于内置版本的显式 `CC_SWITCH_CODEX_CLIENT_VERSION` 始终优先。再用 `CC_SWITCH_CODEX_CLI_VERSION_SYNC_DISABLED` 与缩短的 `CC_SWITCH_CODEX_CLI_VERSION_SYNC_INTERVAL_HOURS` 验证禁用和周期同步。
4a. Share URL 流式 `gpt-*` 在 `response.created` / `response.in_progress` 后立刻收到 `server_is_overloaded` 时，客户端不得先看到该致命码。允许同一 binding 静默重试后成功；已出现 `output_text.delta` 后只能改写为 `server_error` 并保留 overloaded 原文。该错误不得写入账号 cooldown，也不得换号。`rate_limit_exceeded` 必须原码转发。
5. 本地账号 ID 必须由已验证 user subject 稳定派生，同 subject 重登应原子复用旧记录；workspace 只能选择 token claims 中的 organization/Account-ID，不能作为本地 principal。修改后出站 `ChatGPT-Account-Id` 应随选择变化，伪造 ID 必须被控制面拒绝。
6. Responses Lite 请求应覆盖 `additional_tools`、custom tool call/output continuation、tool_search forced choice 和同名冲突错误；Chat 上游回程应恢复 custom item，stream 完成事件包含非空 output。
6a. 对官方 OAuth 调用公开 `/v1/responses/compact`，抓包确认实际上游路径是 `/backend-api/codex/responses` 而非 `/responses/compact`，body 为 `stream=true`、`store=false`，且唯一 `compaction_trigger` 位于 input 最后；任意 chunk 切分下最终下游只返回一个 JSON response。普通 `/v1/responses` 自带 trigger 时必须仍访问 `/responses` 并保留客户端 stream/non-stream 语义。模拟 failed、incomplete、缺 terminal、坏 JSON 和中途断流，确认不会伪装为 completed。
6b. 选 manifest 明确 `use_responses_lite=true` 与 `false` 的模型各一个，并分别构造请求模型经 single-model policy 映射为另一类模型的两种方向；抓包确认 Lite header/body mutation 只取最终模型能力。未知模型保持信号透传；不支持模型同时移除 HTTP header 和 WS marker。原生 WS 与 WS→HTTP fallback 也执行同一最终模型判断。
6c. 分别在 HTTP、首次 401 replay、原生 WS response.create、WS→HTTP fallback 和 Dedicated Images 携带同一份含 Unix/Windows/Unicode path、remote URL、commit hash 的 turn/client metadata；确认所有出站均使用同 scope 稳定占位符、remote 被删除、commit 为 40 hex，第二次清洗不变化，且客户端未携带的字段不被新增。malformed/超过 32 KiB 的可选 header 应被丢弃，日志和错误中不得出现原文。
6d. 保持 `CC_SWITCH_CODEX_ROUTING_HINT_ENABLED=0`，确认客户端和 Account extra header 都不能把 `x-codex-routing-hint` 发往上游。仅在专用测试账号确认后临时设为 `1`：model-only 与 priority HTTP 请求分别应发送 `<final-model>` 和 `<final-model>;tier=priority`，未知 tier 只发模型，Images 使用内部 Responses 最终模型；WebSocket handshake 始终无 hint。若任一真实请求被拒，立即关闭并保持 `live_pending`。
6e. 用会在首个业务事件之后至少静默 60 秒的 reasoning 流验证普通文本 SSE：首业务事件前绝无 comment，之后约每 15 秒收到 `: keepalive`，最终仍收到真实 terminal；缩短 `STREAM_IDLE_TIMEOUT_MS` 后确认心跳不能阻止上游 idle timeout。再设 `CC_SWITCH_CODEX_RESPONSES_KEEPALIVE_MS=0` 验证禁用，并通过 Nginx/Cloudflare 确认中间层不缓冲小 chunk。
7. SSE 与 WebSocket 分别模拟空 `response.completed.response.output`，确认按 `output_index` 重建；已有非空 output 不覆盖，第二个 response 不得串入前一轮状态。
8. provider 的 `codexWebsocketEnabled=false` 应使 GET WS 返回 503，并保持 POST Responses SSE 可用；恢复开关后再跑 text/binary WS 与 Windows reset 场景。
9. 推理等级由客户端选择并透传：`low`、`medium`、`high`、`xhigh`、`max` 保持不变，仅把非 wire 别名 `ultra` 规范为 `max`；日志分别显示 requested/effective effort。Claude `output_config.effort`/`thinking.effort` 与 Gemini `generationConfig.thinkingConfig.thinkingLevel` 必须经过转换保留；`/v1/models` 应返回 Sol/Terra/Luna。
10. usage fixture 同时覆盖 nested `cache_write_tokens`、cache read、cache creation 显式零值和 Anthropic exclusive input，核对 fresh/read/write/output 四桶与总 Token。
11. Codex Images 必须同时验收 `/v1/images/generations` 与 `/v1/images/edits`：短 prompt 的 non-stream `b64_json` 能完整解码，`stream=true` 在上游生成完成前收到 `: connected`，超过 15 秒的生成持续收到 keepalive，partial/completed/error 的事件名前缀分别符合 generation/edit；显式 Responses image tool 的 SSE/JSON 也必须在上游首个业务事件前提交 comment/空白。edit 上传一张大于 1 MiB 的真实图片应到达上游；用两张 base64 图片验证超过 32 MiB HTTP body、但图片聚合不超过 32 MiB 时仍可进入 handler。单图大于 20 MiB、图片聚合大于 32 MiB、HTTP decoded envelope 大于 48 MiB、超过 16 张、伪造输入或输出 MIME/signature、非法参数和 `n>1` 必须在零上游或受控边界失败。模拟 `response.failed`、incomplete/cancel、无终止 EOF、首事件/idle timeout、错误 body 超限与客户端断连，核对已提交 wire `200` 时的流内 error 以及 usage 的 400/502/504/499、stream status、error message、inflight 归零，且其他账号/Provider 请求数为零；提交后不得透明 failover 或 overflow retry。`response_format=url` 应返回签名 ingress 中同一 Share host 下的随机 capability URL；携带 Router token 的 GET/HEAD bytes、Content-Type/Length、`no-store`/`nosniff` 正确，无效 token 为 404，缺少 Router 鉴权为 401。保留一个 URL，重启 Server 后旧 URL 仍应在 TTL 内可下载；再让两个副本挂载同一 `CC_SWITCH_IMAGE_STORE_DIR`，由不同副本分别生成和下载。共享目录必须验证跨进程锁、atomic rename、目录同步和权限；没有共享目录时才配置生成与下载的实例粘性。通过 Cloudflare Worker/Tunnel 再执行一次：Worker 必须直接透传 `Response.body`，不能调用 `.text()`、`.json()` 或 `.arrayBuffer()`；确认无 524、小心跳实际 flush、文件路由不被 Cache 拦截且 Router 鉴权不被移除。执行 `CC_SWITCH_CODEX_IMAGES_SMOKE=1 node scripts/smoke/codex-images-real.mjs`，只记录 requestId、状态、首块/最大静默时间、字节数、格式、SHA-256 和脱敏账号，不记录图片内容或 capability token。
12. server 不应自动读取或写入运行主机用户的 `~/.codex/auth.json`；只测试显式登录/导入。TLS/JA3 只有在 rustls 请求出现可重复的上游拒绝证据时才开启专项评估。
13. 从配置中的非 loopback HTTPS Client URL 发起 CLI OAuth，确认授权请求仍使用 `http://localhost:1455/auth/callback`。浏览器本地回调失败后提交完整地址栏 URL 应完成同一管理员主体的会话；裸 code、`127.0.0.1`、错误端口/path、重复 state、过期/取消会话、另一管理员会话、非同源页面、未配置的 host 和远程 HTTP Client URL 都必须拒绝。另以 `0.0.0.0` 或 `::` 启动 Server，确认携带伪造 `Host: 127.0.0.1` 的远程请求仍被拒绝；只有 Server 实际绑定 loopback 时才允许本机例外。Device OAuth 同时保持可用。
14. Provider 中伪造 OAuth authorize/token、quota 或 inference endpoint 后保存/转发必须被固定 endpoint policy 拒绝或覆盖，OAuth token 不得发往自定义 host；managed OAuth Provider 缺少显式账号绑定时必须拒绝保存，不能隐式选同类型第一个账号。
15. `GET /api/accounts`、账号 upsert/refresh/quota 响应及兼容 invoke 响应不得包含 access/refresh/id token、API key、extra headers、profile、raw 或 refresh error 原文；只允许 `has*`/状态/配额/脱敏身份字段。
16. HTTP non-stream、SSE、Images、image-tool 去除后的二次请求、WebSocket handshake 与 WS→HTTP fallback 分别模拟首次 401：同一账号只强刷一次并重物化 Authorization/workspace header；仍为 401 时直接返回并只记录原账号 cooldown。Share binding 不得跨 Provider/账号；为其他 Provider/账号设置 mock 后断言其上游请求数为零。
17. 分别以 0、1、2 个 Codex OAuth 账号启动，确认 `GET /api/accounts`/`auth_get_status` 返回 `unconfigured`、自动 `ready`、`needs_selection`。`needs_selection` 只阻断依赖 active account 的账号中心操作；已明确绑定账号的 Share HTTP/SSE/WS/Images/models/alpha-search 数据面继续使用自身 binding。调用 `POST /api/accounts/codex/active` 后，Provider、Share、revision 和 RuntimePlan 必须保持不变，重启后账号中心选择保持。把 Share 绑定账号置于 cooldown、quota 耗尽和并发上限时请求应直接失败，其他账号/Provider 请求数始终为零，SSE/WS 结束或断连后原账号 inflight 必须归零。
18. 同一 Codex session 连续两个 WS response 应只建立一个上游连接；更换 Provider/runtime/workspace/credential 必须生成新 pool key。用 `CC_SWITCH_CODEX_WS_CACHE_MAX_CONNECTIONS`、`CC_SWITCH_CODEX_WS_CACHE_IDLE_MS`、`CC_SWITCH_CODEX_WS_CACHE_MAX_AGE_MS` 缩短参数验证 capacity/idle TTL/max age，并验证 `codexWebsocketEnabled=false` 的禁用行为。
19. 分别模拟 WS connect refused/timeout、握手 5xx、stale cached socket 和发送 `response.create` 前的 send failure，确认仅这些阶段可通过同账号 HTTP/SSE 回退；握手 400/401/403/429 不得作为传输 fallback。成功发送 `response.create` 后再模拟 read failure、close 1009 和缩短 `STREAM_FIRST_BYTE_TIMEOUT_MS` 导致的首事件超时，均应只终止流且不重放；缩短 `STREAM_IDLE_TIMEOUT_MS` 验证首业务事件后的空闲超时同样不重放。`cc_switch_codex_websocket_fallback_total{source,result}` 与 cache/retry 指标应对应增加。
20. 保持 `CC_SWITCH_CODEX_OVERFLOW_AUTO_COMPACT` 未设置或为 `0`，模拟 HTTP 400 和 SSE `response.failed/context_length_exceeded`，确认无内部摘要和重放。设置为 `1` 后确认同一 Provider/账号按“原请求 → 摘要 → 压缩后请求”最多执行一次，摘要 usage 的 `dataSource=codex_overflow_compact_summary`；摘要失败应使用省略标记继续一次，压缩后再次 overflow 直接返回。首个业务事件前的 SSE overflow 可压缩，已提交业务事件后的 overflow 必须保持原错误且绝不重放。
21. 在 OpenAI OAuth Provider 配置页依次执行 referral eligibility、send 和 tracking。先记录 Provider revision，再在请求途中修改 Provider binding，旧 expected revision 必须冲突且零上游；正常路径的 Authorization、workspace 与 cookie session 只能属于 Provider 已提交账号。Eligibility 中 `offer_id=credits_1000`/grant amount 仅作为本次上游证据展示，不得在缺失时合成。业务 403、收件人失败、Cloudflare HTML challenge、超 10 邮箱、重复/非法邮箱和超 1 MiB body 分别验收；首次 401 只刷新原账号一次，另一个账号请求数为零。
22. 为两个 Share 绑定不同 Codex OAuth 账号，并只在其中一个开启 `allowPersonalCredits`、`autoConsumeBankedReset`。构造正常窗口耗尽但 personal credits 可用、credits 耗尽、unlimited 和 overage-limit 四种 quota，确认可用性只改变开启策略的 Share。对自动 Reset 构造 workspace 不匹配、stale details、非 available、非 `codex_rate_limits`、缺 credit id、窗口外和窗口内多候选；只有同 workspace 最早临期的有效候选可消费。消费前并发修改 Share revision/policy/binding/runtime/account generation 或 credit 必须停止。用两个独立锁竞争进程或专用 fixture 指向同一个 reset lock 文件，验证并发请求只允许一次消费；不要尝试并发启动两个共享同一 config dir 的 Server。401 前后 request id 完全相同，成功只刷新原账号 quota，其他账号请求数为零。
23. 对同一账号建立两个 Share 和两个模型。普通 model-capacity/未知 Codex 429 只让当前 `(Share,runtime,model)` 冷却五分钟，另一个 Share/模型仍可用；`usage_limit_reached` 或明确耗尽 window 只冷却固定账号，不切换账号、Provider 或模型。随后开启 previous-response cache：HTTP JSON 聚合、任意 chunk 切分 SSE、text/binary WS 和 WS→HTTP fallback 的 completed 均可续接 function/custom/shell/MCP call/output；message、reasoning/encrypted content、image/web-search 不得进入缓存，server item id 必须删除，重复 `(type,call_id)` 不注入。更换 Share、签名用户、runtime、workspace、账号代际或 response id 均 miss；缺 principal、TTL 到期、failed/incomplete/error、超 8 MiB、超 200 items 均不写入。普通 continuation miss 保持既有上游清洗行为；只有 `previous_response_id` 加当前未配对 tool output 的必需上下文 miss 返回精确 409，body 必须含 `type=invalid_request_error`、`code=response_context_unavailable`、`param=previous_response_id`，当前已有完整 call/output pairing 时不得误报。
24. 上述 Share 策略保存、Bundle Share 保存和 Router descriptor 同步均检查 camelCase/snake_case 兼容、默认值、10..10080 reset lead 校验、revision/fingerprint 变化和重启持久化。账号中心 active account 改变不得影响 Referral 的 Provider 身份或任一 Share 的 credits/reset/cooldown/cache namespace。

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
14. 运行 `node scripts/smoke/grok-oauth-real.mjs`，通过同一个 `CC_SWITCH_SHARE_URL` 验收 models metadata、Responses JSON 和完整 SSE terminal。只有显式设置 `CC_SWITCH_GROK_MEDIA_SMOKE=1` 才运行图片；缺少 Share URL/Router token 或仍为占位符时的 `SKIP` 只能记录为 blocked-inputs，不能记录为真实通过。
15. 检查 `/metrics` 中 Provider outcome、forward retry、WS fallback、CLI version gate、model catalog、账号 in-flight/max、warm refresh 和 persistence degraded 指标；labels 和 evidence 只含有界分类、Provider id、模型和脱敏账号，不得包含 access/refresh/ID token 或 raw OAuth/upstream body。

Claude OAuth 专项补充：

1. 同一 `claude_oauth` 账号并发触发多次 refresh 时，上游 token endpoint 不应收到重复风暴；失败后短窗口内应进入 per-token backoff。
2. 新建 Claude 授权 URL 必须包含 `prompt=login`，避免多账号浏览器会话抢占。
3. Claude proxy 请求应携带 CLI header set、基于首条 user 文本稳定合成的 `x-claude-code-session-id`，并在无客户端 `metadata.user_id` 时注入 server 合成值。
4. `anthropic-beta` 应按请求形状出现：基础请求只带 Claude Code/OAuth beta；含 `thinking`、streaming tools 或 computer-use tool 时才追加对应 beta；messages 与 profile/usage 请求的 Claude CLI UA 应保持同一版本，CCH `cc_entrypoint` 默认应为 `cli`。
5. 上游 429 时应记录 Share 所绑定 Provider 的 rate-limited outcome，并原样保留审计过的 rate-limit 响应头。Claude Messages/count_tokens 请求不得切换 Provider 或账号；绑定账号的 429 直接返回。
6. Claude SSE 中出现 `event:error` 且类型为 `rate_limit_error`、`overloaded_error` 或 `api_error` 时，应记录 Share 绑定 Provider failure；无论 error 位于下游 commit 前后都不得透明重放或切换账号，已开始输出的流以 Anthropic 终止错误帧结束。
7. 非 Claude Code 客户端请求应被改写为 billing/identity system blocks，原 system 迁移到首条 user message，并重算 CCH。
8. 上游 400 signature/thinking 错误应触发反应式降级重试：thinking block 降为 text；工具签名错误时 tool_use/tool_result 降为 text；web_search 历史块错误时剥离历史 server_tool_use/web_search_tool_result。
9. `CC_SWITCH_CCH_SALT_HEX`、`CC_SWITCH_CLI_STAINLESS_OS`、`CC_SWITCH_CLI_STAINLESS_ARCH`、`CC_SWITCH_CLI_STAINLESS_RUNTIME_VERSION` 覆盖应只用于灰度/抓包追热；默认路径应按账号 seed 稳定选择 stainless OS/arch，stream 请求 `x-stainless-timeout=600`，非 stream 请求为 `60`。
10. 长闲置 Claude OAuth 账号应由后台 60s 维护循环提前 warm-refresh；真实回归可把 access token 置空或调短 `expiresAt`，确认首个 proxy 请求前账号已恢复可用或只触发一次 singleflight refresh。
11. 若上游返回 Claude Code CLI 版本过期提示，响应体应替换为面向 cc-switch-server admin 的 `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA` 调整提示，并记录 error 日志。
12. Claude OAuth 出站 JSON 不应被 key 字母序化；抓包时至少确认原始 `model` / `max_tokens` / `messages` 相对顺序被保留，缺省工具请求应补 `tools: []`。
13. 上游响应含 `x-request-id` 时，下游客户端应能拿到同名 header，便于 Anthropic support 联合排查。
14. Claude OAuth 客户端 header 中加入未知 beta（例如 `prompt-caching-scope-2026-01-05`）时，上游不得收到该 token；已审计的 `prompt-caching-2024-07-31` 与 `token-efficient-tools-2025-02-19` 应保留，server debug 日志应能定位被过滤事件但不得记录 token/account 身份。
15. 同一 OAuth state 在多 tab 重复完成时应返回同一 completed/account 结果；Pending/preview session 可通过 `/api/accounts/login/cancel` 或 `auth_cancel_login` 幂等取消，取消后 finish/poll 必须终止，未知 state 必须拒绝。exchange 已开始后 cancel 应返回冲突，避免授权码已消费但账号未持久化。
16. Claude OAuth Share 请求只使用 Share binding 对应 Surface 的单一绑定账号；默认并发上限为 8，provider 的 `ACCOUNT_MAX_CONCURRENT` / `MAX_CONCURRENT_REQUESTS` 可覆盖，`CC_SWITCH_ACCOUNT_MAX_CONCURRENT=0` 可关闭。达到上限时即使存在其他 Claude Provider/账号也必须返回 429，SSE 结束或中断后容量必须释放。
17. 如使用 `~/.claude/.credentials.json` 迁移，只通过显式 `POST /api/accounts/claude/credentials/import` 导入；server 不自动扫描本机目录、不写 Claude Desktop profile，也不通过控制面提供明文凭据导出。
18. 缺省 `max_tokens` / `temperature` 的请求应分别补为 `128000` / `1`；thinking 请求强制 `temperature=1` 并删除冲突的 `top_p`/`top_k`，非 thinking 显式 sampling 保持不变。
19. `POST /v1/messages/count_tokens` 与 `/claude/v1/messages/count_tokens` 应只选择 `claude`、`claude_auth`、`claude_oauth`；OAuth 抓包应包含 token-counting beta、无 generation 字段且 CCH 对最终 body 有效。Codex/Gemini/OpenRouter provider 必须被拒绝，成功响应的 `input_tokens` 原样返回且不产生生成 usage。
20. Responses/Chat 上游转 Anthropic stream 时，使用两个并行工具和 packed `function_call_arguments.done` 验证每个 block 只 start/stop 一次、arguments 不丢不重；分别以 CRLF、多事件同 chunk、JSON 每个切分点和 EOF 半帧注入，已输出后的错误不得重放请求。
21. profile refresh 后 `organization.billing_type` 应进入 `profile.billingSource`；Apple/Stripe 不应改变 plan 或生成订阅到期日，未知 billing type 应原样保留。
22. 连续 `invalid_grant` 达到 `CC_SWITCH_REFRESH_FAILURES_BEFORE_RELOGIN` 阈值后，账号应显示 `relogin` 并退出其固定 Provider 内的账号调度；网络错误、限流和普通 quota 错误不得累计该计数，手工 refresh 成功后状态应清零。
23. `GET /metrics` 应能看到账号 inflight/max、Claude retry、Provider outcome、warm-refresh、CLI version-gate、beta decision、count_tokens outcome 与 stream protocol error 指标；labels 必须保持固定枚举。该端点默认无鉴权，公网部署必须由反向代理或网络策略限制抓取来源。
24. 分别使用真实 `CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT` 与 `CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT` 完成 OAuth 登录；变量值使用账号 ID 或 email。设置 `SERVER_URL`、`CC_SWITCH_SERVER_TOKEN`、`CC_SWITCH_SHARE_URL` 和 `ROUTER_API_TOKEN` 后运行 `node scripts/smoke/claude-oauth-real.mjs`。脚本通过公开账号 API 强制刷新两个账号 quota，并独立验收普通 Share count_tokens、Messages JSON 与完整 SSE terminal。Auth Center 账号行、Provider 账号选择器和订阅 quota 应分别稳定显示 `Claude Max 5x` / `Claude Max 20x`，后端 subscription `planType` 应分别为 `claude_max_5x` / `claude_max_20x`。不得提交 `accounts.json`、token、完整 profile/bootstrap/roles/usage body 或未脱敏 email。
25. 对每个真实等级只记录脱敏账号、`planType`、`planLabel`、evidence `source` / `stale` / `conflict`、HTTP 状态与时间。全新登录应优先由实时 usage/bootstrap/profile 证据解析且 `stale=false`；只有实时证据仅给通用 Max、兼容旧倍率被采用时才允许 `stale=true`。实时 5x 与 20x 相互冲突时必须出现 `claude_plan_conflict`，不能静默覆盖。
26. 20x 已有本地 `default_claude_max_20x` fixture 证据，但仍需真实账号确认当前 Anthropic 响应。5x 当前只有同形 `..._5x` 解析规则，没有 checked-in 真实 fixture；在 5x 账号和脱敏响应证据齐备前，release evidence 必须写 `blocked-inputs` 或 `SKIP`，不得写 live passed。
27. 真实专项账号缺少任一个时，脚本会为对应等级输出独立 `[SKIP]`；只运行本地 resolver/API/UI 测试并将该等级标为未验收。不得用手工编辑 `subscriptionLevel`、伪造 bootstrap 或另一个等级账号替代真实通过。Share、5x、20x 三个 gate 的 SKIP/FAIL/PASS 必须分别记录，不能用其中一个 PASS 覆盖其他 gate 的缺失输入。

Grok 的真实输入作为独立 external gate 接入环境检查：缺失时不阻断本地 release readiness，也绝不能宣称真实通过。Cursor/Copilot/Kiro/Bedrock 的真实验收变量继续由 AB7 gate 管理。所有变量齐备都只代表可以开始真实验收；non-stream、stream、usage、错误路径全绿前，不得提升 native capability。Router 内建 Share Market entitlement 的真实验收属于 Router/Share 集成边界，server 只验证 pending share edit 的签名、幂等应用、只读 managed grant 和 ack；详见 [`router-market-acceptance.md`](router-share-acceptance.md)。

## 脱敏 Evidence

以下脚本支持 `EVIDENCE_FILE=/tmp/...json`，只写脱敏摘要：

- `scripts/smoke/real-acceptance-env-check.sh`
- `scripts/smoke/router-share-smoke.sh`
- `scripts/smoke/code-agent-regression.sh`
- `scripts/smoke/oauth-readiness-check.sh`
- `scripts/smoke/grok-oauth-real.mjs`
- `scripts/release-readiness.sh`

检查 evidence 是否包含密钥形态：

```bash
scripts/audit/evidence-redaction-check.sh /tmp/cc-switch-server-evidence/result.json
```
