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
scripts/smoke/share-market-grant-smoke.sh
RUN_REAL=1 scripts/release-readiness.sh
```

## 必需变量

### server 基础

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `SERVER_URL` | 被测 server base URL | 可完整记录 |
| `CC_SWITCH_SERVER_TOKEN` | server 登录 bearer token | 不记录明文，只记录是否存在 |

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
| `CURSOR_OAUTH_TEST_ACCOUNT` | Cursor OAuth 测试账号 | 记录脱敏 email |
| `ANTIGRAVITY_OAUTH_TEST_ACCOUNT` | Antigravity/Agy OAuth 测试账号 | 记录脱敏 email |
| `CODEX_OAUTH_REFRESH_TOKEN_FIXTURE` | Codex OAuth 手动导入 refresh token fixture | 不记录明文 |
| `CLAUDE_OAUTH_REFRESH_TOKEN_FIXTURE` | Claude OAuth 手动导入 refresh token fixture | 不记录明文 |
| `GEMINI_OAUTH_REFRESH_TOKEN_FIXTURE` | Gemini OAuth/CLI 手动导入 refresh token fixture | 不记录明文 |
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
2. 执行账号手动 refresh，记录状态码、脱敏账号、`lastRefreshError` 是否为空或仅为 profile warning。
3. 绑定 provider 到该账号，清空或过期 access token 后发起本地 share 短请求，确认 proxy 转发前自动 refresh。
4. 再跑同一 provider 的 non-stream 和 stream 短请求，记录 requestId、status、actualModel、usage 摘要。
5. direct/market 入口只记录 URL、状态码、requestId 和脱敏账号，不记录 provider raw response。

Claude OAuth 专项补充：

1. 同一 `claude_oauth` 账号并发触发多次 refresh 时，上游 token endpoint 不应收到重复风暴；失败后短窗口内应进入 per-token backoff。
2. 新建 Claude 授权 URL 必须包含 `prompt=login`，避免多账号浏览器会话抢占。
3. Claude proxy 请求应携带 CLI header set、基于首条 user 文本稳定合成的 `x-claude-code-session-id`，并在无客户端 `metadata.user_id` 时注入 server 合成值。
4. `anthropic-beta` 应按请求形状出现：基础请求只带 Claude Code/OAuth beta；含 `thinking`、streaming tools 或 computer-use tool 时才追加对应 beta；messages 与 profile/usage 请求的 Claude CLI UA 应保持同一版本，CCH `cc_entrypoint` 默认应为 `cli`。
5. 上游 429 带 `anthropic-ratelimit-unified-reset` 时，failover breaker 的 open 窗口应按该时间生效；无 header 时回退默认 open duration。
6. Claude SSE 中出现 `event:error` 且类型为 `rate_limit_error`、`overloaded_error` 或 `api_error` 时，应记录 provider failure；若 error 是首个上游 chunk，应在 3 次/10s 预算内透明重试；已开始输出的流不做透明重放。
7. 非 Claude Code 客户端请求应被改写为 billing/identity system blocks，原 system 迁移到首条 user message，并重算 CCH。
8. 上游 400 signature/thinking 错误应触发反应式降级重试：thinking block 降为 text；工具签名错误时 tool_use/tool_result 降为 text；web_search 历史块错误时剥离历史 server_tool_use/web_search_tool_result。
9. `CC_SWITCH_CCH_SALT_HEX`、`CC_SWITCH_CLI_STAINLESS_OS`、`CC_SWITCH_CLI_STAINLESS_ARCH`、`CC_SWITCH_CLI_STAINLESS_RUNTIME_VERSION` 覆盖应只用于灰度/抓包追热；默认路径应按账号 seed 稳定选择 stainless OS/arch，stream 请求 `x-stainless-timeout=600`，非 stream 请求为 `60`。
10. 长闲置 Claude OAuth 账号应由后台 60s 维护循环提前 warm-refresh；真实回归可把 access token 置空或调短 `expiresAt`，确认首个 proxy 请求前账号已恢复可用或只触发一次 singleflight refresh。
11. 若上游返回 Claude Code CLI 版本过期提示，响应体应替换为面向 cc-switch-server admin 的 `CC_SWITCH_CLI_UA_VERSION` / `CC_SWITCH_CLI_UA` 调整提示，并记录 error 日志。
12. Claude OAuth 出站 JSON 不应被 key 字母序化；抓包时至少确认原始 `model` / `max_tokens` / `messages` 相对顺序被保留，缺省工具请求应补 `tools: []`。
13. 上游响应含 `x-request-id` 时，下游客户端应能拿到同名 header，便于 Anthropic support 联合排查。
14. Claude OAuth 多账号并发时，应优先选择当前占用比例较低的账号；默认每账号上限为 8，provider 的 `ACCOUNT_MAX_CONCURRENT` / `MAX_CONCURRENT_REQUESTS` 可覆盖，`CC_SWITCH_ACCOUNT_MAX_CONCURRENT=0` 可关闭。达到上限的账号应从自动选择中跳过，显式 provider/share 绑定应返回 429，SSE 结束或中断后容量必须释放。
15. 如使用 `~/.claude/.credentials.json` 迁移，只通过显式 `POST /api/accounts/claude/credentials/import` / `GET /api/accounts/:id/claude/credentials` 操作；server 不自动扫描本机目录，也不写 Claude Desktop profile。

Cursor/Copilot/Kiro/Bedrock 的真实验收变量已经接入 `scripts/smoke/real-acceptance-env-check.sh` 的 AB7 gate 和 `scripts/smoke/oauth-readiness-check.sh` 的脱敏 evidence。变量齐备只代表可以开始真实验收；non-stream、stream、usage、错误路径全绿前，不得提升 native capability。

### share-market grant

| 变量 | 用途 | 记录方式 |
| --- | --- | --- |
| `SHARE_MARKET_URL` | 真实 share-market base | 可完整记录 |
| `SHARE_MARKET_GRANT_TOKEN` | 创建 grant 的 token | 不记录明文 |
| `SHARE_MARKET_BUYER_EMAIL` | grant buyer | 记录脱敏 email |
| `SHARE_MARKET_LISTING_ID` | listing id | 可完整记录 |
| `SHARE_MARKET_ORDER_ID` | order id | 可完整记录 |
| `SHARE_MARKET_APP_TYPE` | grant 应用范围，默认 `codex` | 可完整记录 |

## 脱敏 Evidence

以下脚本支持 `EVIDENCE_FILE=/tmp/...json`，只写脱敏摘要：

- `scripts/smoke/real-acceptance-env-check.sh`
- `scripts/smoke/router-market-smoke.sh`
- `scripts/smoke/direct-market-diagnostics.sh`
- `scripts/smoke/code-agent-regression.sh`
- `scripts/smoke/oauth-readiness-check.sh`
- `scripts/smoke/share-market-grant-smoke.sh`
- `scripts/release-readiness.sh`

检查 evidence 是否包含密钥形态：

```bash
scripts/audit/evidence-redaction-check.sh /tmp/cc-switch-server-evidence/result.json
```
