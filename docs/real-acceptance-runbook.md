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
scripts/real-acceptance-env-check.sh
STRICT=1 scripts/real-acceptance-env-check.sh
```

## 推荐顺序

静态验证（不编译、不部署、不启动服务）：

```bash
scripts/static-checks.sh
```

完整本地验证（会运行 `cargo check/test` 并通过 `cargo run` 启动本地 server）：

```bash
scripts/validate-local.sh
scripts/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

真实 router/market/provider 输入齐备后：

```bash
STRICT=1 scripts/real-acceptance-env-check.sh
RUN_PROBES=1 STREAM_PROBE=1 scripts/direct-market-diagnostics.sh
scripts/router-market-smoke.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/code-agent-regression.sh
scripts/oauth-readiness-check.sh
scripts/share-market-grant-smoke.sh
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

Cursor/Copilot/Kiro/Bedrock 的真实验收变量已经接入 `scripts/real-acceptance-env-check.sh` 的 AB7 gate 和 `scripts/oauth-readiness-check.sh` 的脱敏 evidence。变量齐备只代表可以开始真实验收；non-stream、stream、usage、错误路径全绿前，不得提升 native capability。

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

- `scripts/real-acceptance-env-check.sh`
- `scripts/router-market-smoke.sh`
- `scripts/direct-market-diagnostics.sh`
- `scripts/code-agent-regression.sh`
- `scripts/oauth-readiness-check.sh`
- `scripts/share-market-grant-smoke.sh`
- `scripts/release-readiness.sh`

检查 evidence 是否包含密钥形态：

```bash
scripts/evidence-redaction-check.sh /tmp/cc-switch-server-evidence/result.json
```
