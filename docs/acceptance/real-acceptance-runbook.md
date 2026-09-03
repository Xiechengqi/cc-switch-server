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
CC_SWITCH_QODER_REAL_RAIL=global_oauth QODER_REAL_RECEIPT_FILE=/tmp/qoder-global-oauth-receipt.json node scripts/smoke/qoder-real.mjs
CC_SWITCH_QODER_REAL_RAIL=global_pat QODER_REAL_RECEIPT_FILE=/tmp/qoder-global-pat-receipt.json node scripts/smoke/qoder-real.mjs
CC_SWITCH_QODER_REAL_RAIL=cn_oauth QODER_REAL_RECEIPT_FILE=/tmp/qoder-cn-oauth-receipt.json node scripts/smoke/qoder-real.mjs
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
| `CC_SWITCH_QODER_REAL_RAIL` | 单次 Qoder 验收 rail：`global_oauth`、`global_pat` 或 `cn_oauth` | 可完整记录 |
| `QODER_REAL_RECEIPT_FILE` | 本次 Qoder 脱敏 receipt 的仓库外绝对路径；文件必须尚不存在 | 只记录路径类别，不提交文件 |
| `QODER_GLOBAL_OAUTH_TEST_ACCOUNT` | Qoder Global Device OAuth Account ID 或唯一 selector | receipt 只记录稳定摘要 |
| `QODER_GLOBAL_PAT_TEST_ACCOUNT` | Qoder Global PAT Account ID 或唯一 selector | receipt 只记录稳定摘要 |
| `QODER_CN_OAUTH_TEST_ACCOUNT` | Qoder CN Device OAuth Account ID 或唯一 selector | receipt 只记录稳定摘要 |
| `CC_SWITCH_QODER_<RAIL>_{CLAUDE,CODEX,GEMINI}_PROVIDER_ID` | 对应 rail 的三个显式 Provider ID；必须固定同一 Account generation | receipt 只记录整体 binding 摘要 |
| `CC_SWITCH_QODER_<RAIL>_MODEL` | 可选的三 Surface 共同 live catalog 模型；缺省时取目录交集 | 可完整记录 |
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
| `CC_SWITCH_COPILOT_CLAUDE_PROVIDER_ID` | 显式绑定待测 Copilot Account 的 Claude Provider ID | 只记录是否存在 |
| `CC_SWITCH_COPILOT_CODEX_PROVIDER_ID` | 显式绑定同一 Copilot Account 的 Codex Provider ID | 只记录是否存在 |
| `CC_SWITCH_COPILOT_GEMINI_PROVIDER_ID` | 显式绑定同一 Copilot Account 的 Gemini Provider ID | 只记录是否存在 |
| `CC_SWITCH_COPILOT_MODEL` | 可选的三 Surface 共同 entitlement model；缺省时从三份动态目录交集选择 | 可完整记录 |
| `DEEPSEEK_WEB_ACCESS_TOKEN_FIXTURE` | DeepSeek Web import-only bearer fixture；只可用于授权的真实验收环境 | 不记录明文、长度、prefix 或 digest |
| `CC_SWITCH_DEEPSEEK_PROVIDER_ID` | 显式绑定待测 `deepseek_account` Account 的 Claude Provider ID | 只记录是否存在 |
| `CC_SWITCH_DEEPSEEK_MODEL` | DeepSeek Web reviewed 低成本验收模型，默认 `deepseek-v4-flash` | 可完整记录 |
| `CC_SWITCH_CODING_PLAN_PROFILE_ID` | 本轮 region × Surface typed Coding Plan Profile id | 可完整记录 |
| `CC_SWITCH_CODING_PLAN_API_KEY_FIXTURE` | 当前 Profile 对应套餐发放的静态 Key | 不记录明文、prefix、长度或 digest |
| `CC_SWITCH_CODING_PLAN_MODEL` | 当前 Profile reviewed catalog 中的低成本模型 | 可完整记录 |
| `OLLAMA_API_KEY` | Ollama Cloud 推理及官方 `/api/me`、`/api/usage` 真实 fixture | 不记录明文、prefix、长度或 digest |
| `KIRO_TEST_ACCOUNT` | Kiro/AWS Builder ID 测试账号 | 记录脱敏 email |
| `KIRO_REGION` | Kiro device flow region，默认 `us-east-1` | 可完整记录 |
| `KIRO_START_URL` | Kiro/AWS SSO start URL | 可完整记录 |
| `KIRO_REFRESH_TOKEN_FIXTURE` | Kiro 已导入 refresh token fixture | 不记录明文 |
| `AMAZON_Q_TEST_ACCOUNT` | Amazon Q Developer Builder ID/IdC 测试账号；不能填写 Kiro Account | 记录脱敏 email/账号名 |
| `AMAZON_Q_REFRESH_TOKEN_FIXTURE` | Amazon Q SSO OIDC refresh token fixture；不能复用 Kiro token | 不记录明文、prefix、长度或 digest |
| `CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID` | 显式绑定待测 Amazon Q Account 的 Claude Provider ID | 只记录是否存在 |
| `CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID` | 显式绑定同一 Amazon Q Account generation 的 Codex Provider ID | 只记录是否存在 |
| `CC_SWITCH_AMAZON_Q_MODEL` | 可选的两个 Surface 共同动态 entitlement model；缺省时使用 `defaultModel` | 可完整记录 |
| `CC_SWITCH_AMAZON_Q_RUNTIME_REGION` | Amazon Q runtime region，只允许 `us-east-1` 或 `eu-central-1` | 可完整记录 |
| `CC_SWITCH_AMAZON_Q_PROFILE_ARN` | IAM Identity Center 场景可选的 Amazon Q profile ARN | 只记录是否存在，不记录完整 ARN |
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
6f. 保持 Responses normalizer 默认开启，分别注入任意 byte split、CRLF、多行 data、comment、无 data 控制帧、文本/JSON ping、HTML 200、普通文本 200、JSON scalar/array、坏 UTF-8/JSON、超限 partial event、`[DONE]`/`response.done` 无真实终态，以及 completed/failed/incomplete。只允许白名单 liveness 被丢弃；每个下游 `data:` 必须是单行合法 JSON object（可选终态后的 `[DONE]` 除外），HTML/文本/坏载荷须在 commit 前返回 502，commit 后须收到不带 `[DONE]` 的 `response.failed/upstream_stream_protocol_error`，usage 为 `protocol_error`。检查 `Content-Type`、`Cache-Control/no-transform`、`X-Accel-Buffering`、`nosniff`，并确认 headers/liveness/lifecycle 不满足首事件预算。临时启用 HTTP/2 PING 做同一长流对照，确认它不改变业务 timeout；HTTP/1.1 对照不应被误判为已启用。
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

Gemini Code Assist 单账号专项补充：

1. 为 `gemini.google_oauth` 或 `claude.google_oauth` Provider 显式绑定一个 `gemini_cli` Account，另建一个未绑定账号作为负向候选。调用 `POST /api/providers/:id/fetch-models`，抓包确认只有绑定账号的 Bearer、Gemini CLI identity、`loadCodeAssist` 与 `retrieveUserQuota` 被使用，负向账号请求数为零。
2. `retrieveUserQuota.buckets` 同时返回重复 Gemini id、`models/` 前缀、0% remaining、非 Gemini model 和非法 id。目录应去重、去前缀、保留 0% 元数据并排除非 Gemini/非法 id；响应必须包含 `source=authenticated_retrieve_user_quota`、`stale=false` 和 `fetchedAtMs`。
3. 成功的空 buckets 是当前绑定的权威空目录，不得合入静态或其他账号模型。模拟 408、429、可重试 5xx 时，只可返回相同 `authIdentityGeneration` 的旧目录并标记 `source=same_account_cached_retrieve_user_quota`、`stale=true`。
4. 模拟 401/403、删除/换绑、Account generation 变化、credential persistence degraded 和旧代际缓存。以上情况即使存在旧目录也必须失败关闭，且第二账号、静态目录和 models upstream 请求数均为零。首个 401 只有存在 refresh token 时可刷新并重放同账号一次；第二个 401 终止。
5. 用目录返回的一个低成本模型完成 Gemini non-stream、stream、tool、image 和 quota；保存脱敏 receipt 的 model id、project/tier 摘要、source/stale/fetchedAt、终态与 usage，不保存 project id、raw quota body 或 token。未完成本项时保持 `live_pending`。

Antigravity / Agy 单账号模型目录专项补充：

1. 分别为 `special.antigravity` 与 `special.agy` Provider 显式绑定一个对应类型 Account；另建未绑定账号。调用 `POST /api/providers/:id/fetch-models`，抓包确认 `loadCodeAssist` 与 `fetchAvailableModels` 只使用各自绑定账号的 Bearer、project 和 Antigravity identity，未绑定账号请求数为零，Agy 不借 Antigravity 的 credential 或 cache。
2. `fetchAvailableModels.models` 同时返回 Gemini、Claude、GPT、未知安全 model id、0% remaining、reset time、thinking/image、thinking budget、max token、MIME 与 `deprecatedModelIds`。响应应逐模型保留明确字段并标记 family；未知字段不得自动扩大能力，非法 model id 应丢弃。
3. 成功空 `models` 是权威空目录。网络、408、429、5xx 只能返回同 Provider type、Account id 与 `authIdentityGeneration` 的旧目录，且 `source=same_identity_cached_fetch_available_models`、`stale=true`；Agy、其他账号和旧代际缓存均不得命中。
4. 模拟首次 401 后原账号 refresh/replay 成功，再模拟第二次 401、403、坏 JSON、缺 `models`、Provider revision/runtime/binding 变化和 Account generation 变化；以上终态必须失败关闭，不得合并静态目录或切换账号。成功 fresh 目录应写入当前代际的 model-catalog、Gemini/Claude/GPT family 与 capacity capability evidence；stale 结果不得刷新证据时间。
5. 用目录中的低成本 Gemini 与 Claude 模型分别完成 non-stream、stream、mixed Google Search + function tool、thought signature、图片输入、429 retry-delay、畸形 stream 与 terminal reason 验收。receipt 只保存 rail、脱敏账号、model family、source/stale/fetchedAt、能力摘要、终态与 usage，不保存 project、token 或 raw body。没有独立可复现的 weekly quota endpoint 时必须记录 `unavailable`，不得从 5h/reset time 猜测周额度；完成真实账号验收前保持 `live_pending`。

Grok OAuth 单账号专项补充：

1. 待测 `grok_oauth` Provider 必须显式绑定一个账号。另建一个绑定不同账号的 Grok Provider 作为负向候选；无论 HTTP、SSE、媒体、WebSocket、429、5xx 或 refresh 失败，负向 Provider/账号的请求数都必须保持为零。
2. 浏览器 PKCE、device start/poll/cancel 和显式 auth.json 导入分别验收。新登录、device 和 auth.json 缺失 ID token、`alg=none`、伪造签名、错误 issuer/audience/nonce、过期/nbf 或未知 `kid` 均必须 fail closed；不得把 email、display name 或未验证 token payload 当作 principal。
3. Refresh 不返回新 ID token 时，只允许已有 verified subject 的账号继续；返回新 ID token 时必须重新验签并拒绝 subject 变化。轮换后的 token 必须先 durable commit，落盘失败期间 `/ready`、HTTP/SSE、媒体和 WS 都应以 degraded/503 阻止新 Grok 流量。
4. 授权请求必须包含 `workspaces:read`、`workspaces:write` 和既有 Grok CLI/conversation scopes。Device start/poll 必须携带 `x-grok-client-version` 与 `x-grok-client-surface: ui`；生产 code exchange、device poll、refresh、OIDC discovery 和 JWKS 必须使用固定的官方 `auth.x.ai` HTTPS URL。生产环境变量或 Provider 伪造 OAuth、WS、models 或 inference host 必须无效或被固定覆盖。
5. 抓包确认 HTTP/SSE、媒体、WS handshake 和 WS→HTTP fallback 使用同一 CLI identity family，默认 version/User-Agent 为 `0.2.111`，并携带一致的 `Authorization`、`x-xai-token-auth`、client identifier/version 和 conversation id。给绑定账号配置同名 `extraHeaders` 必须在零上游请求下 fail closed；legacy Provider 即使残留 `OPENAI_API_KEY` 也必须继续使用 managed token 和 CLI endpoint，配置 inference base URL 必须被固定策略忽略。
6. 对 Responses JSON 和 SSE 发送固定 `x-session-id` 与合法十进制 `x-grok-turn-idx`，确认上游值完全一致。缺失、负数、带符号、空白、非数字、超过 20 位和 `u64` 溢出都应在上游完全省略；Server 不生成、不缓存、不递增 turn，也不因非法值返回 4xx。
7. HTTP non-stream 和 SSE 分别模拟首次 401，确认只强刷绑定账号一次，重放使用新 Authorization，同时 conversation id、turn、Provider id 和 model 不变。第二次 401 直接返回并只冷却原账号，不进入通用 Provider failover。
8. `websocket` 未验证时 GET Responses WS 必须在零上游请求下返回 503。显式 bootstrap 后模拟握手首次 401，再模拟首事件前 close 1009 或 connect/5xx，确认强刷及 HTTP/SSE fallback 始终复用原 Provider、账号、conversation id、turn、single-model policy 和 in-flight lease；握手 400/401/403/429 与首业务事件后的错误不得 fallback。分别发送裸 body、nested 和 flat `response.create`，确认三者统一删除 stream/background、强制 `store=true`，且 continuation 不重复发送 instructions；握手 403/5xx 必须更新绑定账号 cooldown，entitlement headers 必须持久化。
9. `image_generation`、`image_edit`、`video_generation` 未验证时必须各自 fail closed。仅在 `CC_SWITCH_GROK_OAUTH_CAPABILITIES` 明确 bootstrap 对应能力后执行短请求；成功证据持久化后移除开关重启，能力应继续开放。媒体首次 401 只强刷原账号一次，视频状态查询保持创建时的 Provider/账号 sticky identity。检查 `grok-media-tasks.json` schema v4：owner key 必须覆盖 Share、用户 namespace、`video_generation` kind、task id、Provider、Account、auth generation、runtime fingerprint 与 upstream plane；v1-v3 fixture 迁移后仍应指向原 xAI/video identity，任一维度漂移都返回 conflict。验证大于 2 MiB 且不超过 32 MiB 的图片编辑可进入 handler，wire body、gzip/deflate 任一解码层或最终 decoded body 超过 32 MiB 都返回 413；视频 request id 中的 `/`、`?`、`#`、percent escape、空值、超长值必须在零上游请求下返回 400。
10. 对 HTTP、SSE、媒体和 WS 分别注入 429 与 reset/retry hints，确认只更新绑定账号的 cooldown/rate-limit outcome，保留下游审计允许的限流头且不跨 Provider。403/5xx/network failure 同样不能授权账号切换。
11. `/v1/models?app=codex&providerId=<id>` 必须返回当前账号的模型及 `source`、`stale`、`fetchedAtMs`，Provider 控制面 raw 还应包含保守的 model family 与 account capability manifest。依次验收 upstream、权威成功空目录、TTL fresh cache、ETag 304、network/408/429/5xx 后的同 scope last-known-good；无缓存时必须失败，禁止 static fallback。缓存 scope 必须同时匹配 app、Provider revision/runtime fingerprint、Account、`authIdentityGeneration` 与 `tokenRefreshGeneration`。首次 models 401 只强刷并重放原账号一次；第二个 401、403、坏 JSON、超大 body、未绑定、stale generation/plan 或 degraded persistence 均失败关闭，其他账号和静态 models 请求数为零。
12. 将 Grok CLI version 降到已知不受支持值并触发上游 version gate，确认下游错误改写为面向管理员的 `CC_SWITCH_GROK_CLI_VERSION` / `CC_SWITCH_GROK_CLI_USER_AGENT` 指引，raw token/账号不泄漏，`cc_switch_grok_cli_version_gate_total` 增加；恢复默认 `0.2.111` 后重测。
13. Quota 抓包同时覆盖 user、weekly、monthly、task usage 和 subscriptions。`currentPeriod.end`、`billingPeriodEnd`、token expiry 及 inactive subscription 不能成为订阅到期日；仅 active subscription 的明确 expiry 或账号手工 next-payment 值可进入 UI，且不影响凭据有效性和路由。
14. 运行 `node scripts/smoke/grok-oauth-real.mjs`，通过同一个 `CC_SWITCH_SHARE_URL` 验收 models metadata、Responses JSON 和完整 SSE terminal。只有显式设置 `CC_SWITCH_GROK_MEDIA_SMOKE=1` 才运行图片；缺少 Share URL/Router token 或仍为占位符时的 `SKIP` 只能记录为 blocked-inputs，不能记录为真实通过。
15. 检查 `/metrics` 中 Provider outcome、forward retry、WS fallback、CLI version gate、model catalog、账号 in-flight/max、warm refresh 和 persistence degraded 指标；labels 和 evidence 只含有界分类、Provider id、模型和脱敏账号，不得包含 access/refresh/ID token 或 raw OAuth/upstream body。

Qoder 三条单账号 rail 专项补充：

1. Global Device OAuth、Global PAT、CN Device OAuth 必须分三次独立运行，`CC_SWITCH_QODER_REAL_RAIL` 分别设为 `global_oauth`、`global_pat`、`cn_oauth`。每次只配置该 rail 的 Account selector 和 Claude/Codex/Gemini 三个 Provider ID；三个 Provider 必须为 `special.qoder_cosy` / `ready`，固定同一 `qoder_cosy` Account 和同一 `authIdentityGeneration`。另一账号、另一 rail、另一 site 与 decoy Provider 不得参与。
2. OAuth Account 必须只有 access + refresh token presence，PAT Account 必须只有 PAT presence；Global/CN 由稳定 Account ID site 前缀及 Provider binding 双重核对。脚本只读取 `has*`、generation 和状态字段，不读取或输出 credential、profile、raw OAuth body、callback URL或 refresh error 原文。
3. 三个 Surface 分别调用带显式 `app` + `providerId` 的 `/v1/models`，只接受 `source=qoder_live_model_catalog`、`stale=false`、有效 `fetchedAtMs` 和非空目录。显式 model 必须同时存在于三份目录；未指定时只从交集选择，成功空目录不能用静态目录、另一个 Account 或另一个 site 补齐。
4. Quota 只刷新本次选中的 Account，必须返回 `qoderQuota.availability=available|exhausted|unknown` 的诚实投影。它是本次 receipt 的观测字段，不授权账号选择、权重、跨账号 fallback 或跨站恢复。
5. 用同一 Share URL 对 Claude Messages、Codex Responses、Gemini generateContent 分别执行 non-stream 与 stream 短请求。每条 stream 必须读到上游 EOF 后恰有一个协议终态；缺终态、重复终态、终态后业务数据、坏 JSON 或提交后认证错误均失败，不能改用其他 Provider/Account/site。
6. `QODER_REAL_RECEIPT_FILE` 必须是仓库外、父目录已存在、目标文件尚不存在的绝对路径。receipt 只保存 commit、site/rail、model、catalog/quota 状态、两个 generation、Account/Provider binding/path-header 的 SHA-256 摘要、Surface/terminal 状态、敏感扫描摘要和三个 decoy 计数零值；禁止保存 token、PAT、prompt、callback、raw request/response/body 或可逆 Account/Provider ID。
7. 缺 `RUN_REAL=1`、Server/Share 鉴权、Account selector、任一 Provider ID 或 receipt path 时，脚本以成功退出码输出 `verificationState=blocked_inputs`、`liveState=live_pending`，不发网络请求且绝不输出 `live_verified`。三个 rail 的 receipt 缺一不可互相代替；只有对应真实账号的完整运行可以改变该 rail 的 live 状态。
8. 本地运行 `node --test scripts/audit/qoder-real.test.mjs` 会以 loopback mock 分别覆盖三条 rail、binding fail-closed、blocked inputs 与泄漏扫描；其 receipt 固定为 `contract_verified/live_pending`，只证明 harness 合同。真实 Device Flow/PAT、refresh rotation、catalog、quota、三 Surface 与故障注入 receipt 未齐前，Registry/文档继续保持 `fixture_verified` / `live_pending`。

GitHub Copilot 单账号三 Surface 专项补充：

1. 建立 Claude、Codex、Gemini 三个 `github_copilot` Provider，分别填入三个 `CC_SWITCH_COPILOT_*_PROVIDER_ID`；三者必须显式绑定 `GITHUB_COPILOT_TEST_ACCOUNT` 选中的同一个 Account 和相同 `authIdentityGeneration`，runtime 均为 `special.copilot` / `ready`。不得从 active account、模型名或 quota 选择另一个账号。
2. 运行 `node scripts/smoke/copilot-real.mjs`。脚本通过管理 API 对每个 Provider 分别执行 `fetch-models`，要求 fresh、非空且来自 `copilot_models_api` 或同身份 fresh account cache；每个模型 raw metadata 必须携带 GitHub domain、受信 HTTPS API origin、picker/policy、endpoint、limits，以及 tools/vision/reasoning capability。成功空目录仍是权威 entitlement，只是本项真实验收会因无法选出共同模型而失败，不能静态补模型。
3. 三份目录必须存在共同 entitled model；显式 `CC_SWITCH_COPILOT_MODEL` 时三者都必须包含它。随后只刷新原绑定 Account 的 premium quota，并分别在同一 Share URL 完成 Claude Messages、Codex Responses、Gemini generateContent 的非流/流、强制 function tool、usage 和唯一终态。任一 Surface 失败都不得用另一 Provider、Account 或模型静默代偿。
4. 对 github.com 与每个支持的 GHES 域分别保存脱敏 receipt：Account 只保存脱敏 selector/稳定摘要，Provider ID 只记录存在性；保存 domain、API origin、model、catalog source/fetchedAt、quota tier 和六个推理检查状态，不保存 GitHub/Copilot token、raw catalog/quota/body。缺任一真实输入时脚本只输出 `[SKIP]` 并写 `blocked_inputs`，不得标记 `live_verified`。
5. 本地回归运行 `node --test scripts/audit/copilot-real.test.mjs`；mock PASS 只证明 harness 合同，不是 Copilot 真实通过。401 验收仍只能在下游提交前强刷原 Account 一次并重放一次；第二次 401、403、domain/origin/binding/generation 漂移必须失败关闭。

Amazon Q Developer 单账号双 Surface 专项补充：

1. 只使用独立 `amazon_q_oauth` device register/start/poll 或管理员显式导入 `AMAZON_Q_REFRESH_TOKEN_FIXTURE`。抓包确认 Builder ID 固定 start URL 为 `https://view.awsapps.com/start`、OIDC region 为 `us-east-1`、client name 为 `Amazon Q Developer for command line`，scope 恰为 `codewhisperer:completions`、`codewhisperer:analysis`、`codewhisperer:conversations`。Kiro device code、client registration、Account、token 和 Profile 均不得参与。
2. 建立 `claude.amazon_q_oauth` 与 `codex.amazon_q_oauth` 两个 Provider，分别填写 `CC_SWITCH_AMAZON_Q_CLAUDE_PROVIDER_ID` / `CC_SWITCH_AMAZON_Q_CODEX_PROVIDER_ID`；两者必须显式绑定 `AMAZON_Q_TEST_ACCOUNT` 对应的同一 Account ID 和当前 `authIdentityGeneration`，runtime 必须是 `special.amazon_q`。另建 Kiro Account 与第二个 Amazon Q Account 作为 decoy，所有控制面和数据面请求计数必须始终为零。
3. 对两个 Provider 分别执行 `fetch-models`。要求 `ListAvailableModels` 从第一页遍历到无 `nextToken`，每页 body 都含 `origin=CLI`，`defaultModel` 必须真实存在于合并后的目录；目录 scope 同时匹配 App、Provider revision/runtime fingerprint、Account ID、auth identity/token refresh generation、runtime region 与 profile ARN。成功空目录、重复 nextToken、坏/超大 JSON、未知 region、第二次 401 和 generation 漂移均失败关闭，不能合并 Kiro 静态目录。
4. 刷新 quota 并抓包确认调用 `AmazonCodeWhispererService.GetUsageLimits`、官方 CLI UA、`origin=CLI` 和原绑定身份。usage/limit/subscription 投影不得制造未观察到的 plan、到期日或跨账号余额；缺失字段诚实标 unavailable。额度只用于展示，不能用于账号选择或 fallback。
5. 使用动态目录中的 `CC_SWITCH_AMAZON_Q_MODEL` 或真实 `defaultModel`，分别完成 Claude Messages 与 Codex Responses 的 non-stream、stream、声明 function tool/tool result 续轮和允许的 image input。每条 EventStream 必须校验 prelude、headers、message CRC、frame CRC、唯一终态、tool JSON 与图片边界；畸形、截断、重复 terminal、terminal 后 frame、首帧/idle timeout 和客户端取消都只终止原请求，不得重放到 Kiro、另一 Amazon Q Account 或另一个 Provider。
6. 对 inference、catalog 和 quota 分别注入首次 eligible 401。只有在下游尚未提交且 Provider/runtime/Account/auth generation 未漂移时，才允许强刷原 Amazon Q Account 并重放一次；第二个 401、403、429、5xx、network/reset 或取消均终止。验证 refresh grant 为 `refresh_token`，重物化后的 Authorization 只来自同一 Account，decoy 请求数为零。
7. 轮换 refresh/access token、runtime region、profile ARN、Provider credential binding 或 Account identity generation，确认旧 catalog/session/cache/in-flight 结果无法提交；撤销或 relogin 后两个 Provider 都应明确失败，不能落入 `special.kiro`、generic HTTP、Bedrock 或静态模型。`us-east-1` 与 `eu-central-1` 分别验收；其他 region 必须在零上游请求下拒绝。
8. receipt 只保存时间、两个 Provider ID 是否存在、脱敏 Account 稳定标识、region、model、catalog source/default、quota 摘要、各 Surface/终态/401/撤销状态与 decoy 请求计数；不保存 device code、client secret、access/refresh token、profile ARN、prompt、图片、raw EventStream 或上游错误 body。环境检查显示 `inputs-ready` 只代表可开始；上述真实链全部通过前继续保持 `fixture_verified` / `live_pending`。

DeepSeek Web Account 单账号专项补充：

1. 只通过管理员 import 接口提交 `DEEPSEEK_WEB_ACCESS_TOKEN_FIXTURE`；输入必须是 token-only bearer，不能带 `Bearer `、Cookie、refresh/ID token、API key、scope、extra header、password 或 session credential。控制面响应、日志、错误、evidence 与 `accounts.json` 检查不得出现 token 明文、prefix、长度或可关联 digest。
2. `CC_SWITCH_DEEPSEEK_PROVIDER_ID` 必须指向 Claude Surface 的 `claude.deepseek_account`，且显式绑定刚导入 Account 的当前 `authIdentityGeneration`。另建一个 DeepSeek Account 作为负向候选；provider test、models、non-stream、stream、session recovery 和错误路径中该候选请求数必须始终为零。
3. 先执行 Provider dry-run，确认 `driverId=special.deepseek_account`、固定 `chat.deepseek.com` origin、reviewed model 和 `networkChecked=false`；再显式执行 network test，要求同一绑定依次完成 create-session、PoW、completion、严格 terminal，并且结构化 outcome 为 success。SKIP 或缺输入只能记录 `blocked_inputs`。
4. models discovery 必须只返回 `reviewed_deepseek_web_catalog`，`stale=false`、`entitlement=live_pending`，并逐模型携带 text/tools/thinking/search capability；成功结果不能声称动态 subscription entitlement，也不能合并 `deepseek_api`、静态或其他 Account 模型。更改 Account generation 后旧 Provider discovery 必须冲突并要求重绑。
5. 用 `CC_SWITCH_DEEPSEEK_MODEL` 分别验收 Claude non-stream 与 stream；随后覆盖 thinking、search citation 和声明 tool call/result。每条流必须恰有一个合法终态，坏 JSON、截断 EOF、重复 terminal 和 terminal 后业务数据均失败关闭且不得在已提交后重放。
6. 复用同一 Share/user/client session 发起两轮以确认 session cache；让已复用 session 分别返回 400、404、409，确认每种状态只用原 Account 新建一次并在提交前重放。401、403、429、5xx、新 session 失败、第二次失败和中途断流均不得重建或切换 credential rail。
7. 分别注入过期、未来超过 15 分钟、错误 target/algorithm、超限 difficulty、坏 challenge/signature 的 PoW，确认 completion 请求数为零。轮换 bearer 或 Account identity generation 后，旧 session/PoW/replay state 必须不可命中。
8. receipt 只保存时间、Provider/Account 的脱敏稳定标识、reviewed model、operation、HTTP/outcome、terminal/usage 摘要与负向账号请求计数；不保存 bearer、session id、PoW challenge/signature、prompt、raw SSE 或上游错误 body。上述真实链全部通过前保持 `fixture_verified` / `live_pending`。

API Key Coding Plans / Ollama Cloud 专项补充：

1. 先运行 `node scripts/audit/audit-coding-plan-registry.mjs --check --check-sources`，保存三份外部仓库 commit 匹配和 manifest current 的非敏感结果。任一 evidence file hash 漂移都必须先人工 review origin、route、catalog、quota 与 terminal，不能只刷新 hash。
2. 按 `assets/contract/coding-plan-registry-manifest.json` 的 20 个 Profile 逐项设置 `CC_SWITCH_CODING_PLAN_PROFILE_ID`、对应 Key 和 reviewed model。receipt 必须明确 family、region 与 Claude/Codex Surface；一个 Surface/region 的成功不能替代另一个，也不能把通用 compatible Provider 当成该套餐。
3. 每项执行 non-stream、stream、tool（仅 manifest/独立证据明确时）、usage 与错误 Key/429/5xx/坏终态；抓包确认 fixed origin、exact route、auth scheme 和 credential generation。另建不同 region/Surface/Provider 的干扰 Key，所有恢复路径中其请求数必须为零。
4. quota adapter 为 supported 时验收固定 endpoint、credential role、fresh/source/reset 与同 generation transient stale；认证、坏 JSON、Provider revision/credential rotation 后旧 cache 必须失败关闭。adapter 为 `unavailable` 时只接受诚实 unavailable，禁止临时抓 Cookie/HTML、PAYG 余额或另一个 plan endpoint。
5. 轮换当前 Provider Key 后，旧 generation 的 quota/cache/in-flight result 必须不可提交；Provider-owned Key 不能生成 Account 行，Account 列表与推理 binding selector 中都不得出现可选账号。terminal 后、body write 后或任何有歧义的请求都不得重放或切 rail。
6. Ollama 使用一个 `OLLAMA_API_KEY` 创建同 Bundle 的 Claude/Codex Profiles。分别验收推理与目录，再验收并发 `POST /api/me` 和 `GET /api/usage` 的 complete、account-only、usage-only、0%、429→同 generation stale、401 清 cache、redirect、超 512 KiB、坏 JSON与 credential rotation。账号/usage 投影不得阻断推理或影响调度，也不得创建 Account、保存 Cookie/HTML。
7. 本地 `cargo test coding_plan --lib`、`cargo test ollama --lib` 和 `node --test scripts/audit/coding-plan-registry.test.mjs` 通过只算 fixture。每个 Profile 与 Ollama 的 receipt 齐备前继续标记 `fixture_verified` / `live_pending`。

Claude OAuth 专项补充：

1. 同一 `claude_oauth` 账号并发触发多次 refresh 时，上游 token endpoint 不应收到重复风暴；失败后短窗口内应进入 per-token backoff。
2. 新建 Claude 授权 URL 必须包含 `prompt=login`，避免多账号浏览器会话抢占。
3. Claude proxy 请求应携带 2.1.258 CLI header set、基于首条 user 文本稳定合成的 `x-claude-code-session-id`，并在无客户端 `metadata.user_id` 时注入 server 合成值。billing 应在 system 迁移前从同一原始 user text 计算 UTF-16 prompt fingerprint；`ping` 必须产生 `cc_version=2.1.258.1e2`，billing block 不得带 `cache_control`。
4. `anthropic-beta` 应按请求形状出现：普通工具数组不能触发 advanced-tool-use，只有审计过的 tool-search/deferred tool 才能触发；`thinking.display="updates"` 应加入 thinking-display-updates 且不加入 redact-thinking；Fable 5.1 应加入 mid-conversation-system。messages 与 profile/usage 请求的 Claude CLI UA 应保持同一版本，CCH `cc_entrypoint` 默认应为 `cli`。
5. 上游 429 时应记录 Share 所绑定 Provider 的 rate-limited outcome，并原样保留审计过的 rate-limit 响应头。Claude Messages/count_tokens 请求不得切换 Provider 或账号；绑定账号的 429 直接返回。
5a. 在明确绑定 Max 20x 账号的 Fable 5.1 non-stream 与 SSE 请求上，记录脱敏后的 `5h`/`7d`/`7d_oi` header 存在性、取整 utilization 和 reset 倒计时范围，不保存整组原始 header。随后刷新账号/Provider quota，必须出现 `Fable 7d`，且后端 tier metadata 为 `scope=model_family`、`capacityPool=claude_fable_7d_oi`、`modelFamily=claude-fable-5`、`relativeWeeklyCapacity=0.5`、`source=anthropic_ratelimit_7d_oi`，`queriedAt` 不早于样本时间。主动 usage 若已提供同名 tier，必须优先于被动样本。若真实响应没有 `7d_oi` 证据，本项保持 `live_pending`，不得从 7d 或计划倍率推测。
5b. 注入或等待明确的 Fable-only 429：`7d_oi=rejected` 且 5h/7d 为 `allowed`/`allowed_warning`。Fable tier 应显示 100%，后续 Fable 请求在 reset 前被本地拒绝，但同账号普通 Sonnet/Opus 请求仍能到达原绑定上游；不得换 Provider/账号。共享窗口缺失或也 rejected 时必须走账号级 cooldown。到达 reset 后 Fable 被动 tier/阻断应失效，打开中的 UI 通过 generation-aware quota event 自动刷新。
6. Claude SSE 中出现 `event:error` 且类型为 `rate_limit_error`、`overloaded_error` 或 `api_error` 时，应记录 Share 绑定 Provider failure；无论 error 位于下游 commit 前后都不得透明重放或切换账号，已开始输出的流以 Anthropic 终止错误帧结束。
7. 非 Claude Code 客户端请求应被改写为 billing/identity system blocks，原 system 迁移到首条 user message，并在移除 billing cache marker 后重算 CCH；2.1.258 golden fixture 应得到 `cch=8d393`。
8. 上游 400 signature/thinking 错误应触发反应式降级重试：thinking block 降为 text；工具签名错误时 tool_use/tool_result 降为 text；web_search 历史块错误时剥离历史 server_tool_use/web_search_tool_result。
9. `CC_SWITCH_CCH_SALT_HEX`、`CC_SWITCH_CLI_STAINLESS_OS`、`CC_SWITCH_CLI_STAINLESS_ARCH`、`CC_SWITCH_CLI_STAINLESS_RUNTIME_VERSION` 覆盖应只用于灰度/抓包追热；默认路径应按账号 seed 稳定选择 stainless OS/arch，stream 请求 `x-stainless-timeout=600`，非 stream 请求为 `60`。
10. 长闲置 Claude OAuth 账号应由后台 60s 维护循环提前 warm-refresh；真实回归可把 access token 置空或调短 `expiresAt`，确认首个 proxy 请求前账号已恢复可用或只触发一次 singleflight refresh。
11. 若上游返回 Claude Code CLI 版本过期提示，响应体应替换为面向 cc-switch-server admin 的 `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA` 调整提示，并记录 error 日志。启动前应确认两个 override 默认为空；任何低于 2.1.258 的遗留值必须被拒绝，并在日志/`cc_switch_claude_wire_profile_info` 中显示 `stale_override_rejected=true`。
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
23. `GET /metrics` 应能看到账号 inflight/max、Claude retry、Provider outcome、warm-refresh、CLI version-gate、beta decision、count_tokens outcome、quota-header observation 与 stream protocol error 指标；labels 必须保持固定枚举，quota-header 指标不得包含账号、Provider、request ID 或原始 header 值。该端点默认无鉴权，公网部署必须由反向代理或网络策略限制抓取来源。
24. 分别使用真实 `CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT` 与 `CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT` 完成 OAuth 登录；变量值使用账号 ID 或 email。设置 `SERVER_URL`、`CC_SWITCH_SERVER_TOKEN`、`CC_SWITCH_SHARE_URL` 和 `ROUTER_API_TOKEN` 后运行 `node scripts/smoke/claude-oauth-real.mjs`。脚本通过公开账号 API 强制刷新两个账号 quota，并验收当前 `CC_SWITCH_CLAUDE_MODEL` 的 Share count_tokens、Messages JSON 与完整 SSE terminal。先以普通模型运行一次；再将 `CC_SWITCH_SHARE_URL` 指向明确绑定该 Max 20x 账号的 Share，设置 `CC_SWITCH_CLAUDE_MODEL=claude-fable-5-1` 后独立运行一次，把第二次的 non-stream/SSE 结果单独记为 Fable 5.1 gate。所有凭据必须已轮换且只通过私密环境注入。Auth Center 账号行、Provider 账号选择器和订阅 quota 应分别稳定显示 `Claude Max 5x` / `Claude Max 20x`，后端 subscription `planType` 应分别为 `claude_max_5x` / `claude_max_20x`。不得提交 `accounts.json`、token、完整 profile/bootstrap/roles/usage body、shell 历史或未脱敏 email。
25. 对每个真实等级只记录脱敏账号、`planType`、`planLabel`、evidence `source` / `stale` / `conflict`、HTTP 状态与时间。全新登录应优先由实时 usage/bootstrap/profile 证据解析且 `stale=false`；只有实时证据仅给通用 Max、兼容旧倍率被采用时才允许 `stale=true`。实时 5x 与 20x 相互冲突时必须出现 `claude_plan_conflict`，不能静默覆盖。
26. 20x 已有本地 `default_claude_max_20x` fixture 证据，但仍需真实账号确认当前 Anthropic 响应。5x 当前只有同形 `..._5x` 解析规则，没有 checked-in 真实 fixture；在 5x 账号和脱敏响应证据齐备前，release evidence 必须写 `blocked-inputs` 或 `SKIP`，不得写 live passed。
27. 真实专项账号缺少任一个时，脚本会为对应等级输出独立 `[SKIP]`；只运行本地 resolver/API/UI 测试并将该等级标为未验收。不得用手工编辑 `subscriptionLevel`、伪造 bootstrap 或另一个等级账号替代真实通过。Share、5x、20x、Fable 5.1 四个 gate 的 SKIP/FAIL/PASS 必须分别记录，不能用其中一个 PASS 覆盖其他 gate 的缺失输入；本地 fixture 通过不得写成 live passed。

Grok 与 Amazon Q 的真实输入作为独立 external gate 接入环境检查：缺失时不阻断本地 release readiness，也绝不能宣称真实通过。Cursor/Copilot/Kiro/Bedrock 的真实验收变量继续由 AB7 gate 管理；Amazon Q 虽也在 AB7 展示，但其 gate、Account、token、Provider 与 Kiro 完全独立。所有变量齐备都只代表可以开始真实验收；non-stream、stream、usage、错误路径全绿前，不得提升 native capability。Router 内建 Share Market entitlement 的真实验收属于 Router/Share 集成边界，server 只验证 pending share edit 的签名、幂等应用、只读 managed grant 和 ack；详见 [`router-market-acceptance.md`](router-share-acceptance.md)。

## 脱敏 Evidence

以下脚本支持 `EVIDENCE_FILE=/tmp/...json`，只写脱敏摘要：

- `scripts/smoke/real-acceptance-env-check.sh`
- `scripts/smoke/router-share-smoke.sh`
- `scripts/smoke/code-agent-regression.sh`
- `scripts/smoke/oauth-readiness-check.sh`
- `scripts/smoke/grok-oauth-real.mjs`
- `scripts/smoke/copilot-real.mjs`
- `scripts/release-readiness.sh`

检查 evidence 是否包含密钥形态：

```bash
scripts/audit/evidence-redaction-check.sh /tmp/cc-switch-server-evidence/result.json
```
