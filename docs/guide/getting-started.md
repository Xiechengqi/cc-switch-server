# 快速开始

> 状态：**权威文档**。最后核对：2026-08-20。
>
> 架构背景见 [`../architecture/overview.md`](../architecture/overview.md)；配置项清单见 [`configuration.md`](configuration.md)。

## 1. 启动

开发启动：

```bash
cargo run -- --host 0.0.0.0 --port 15721
```

显式 `serve` 子命令与无子命令启动兼容：

```bash
cargo run -- serve --host 0.0.0.0 --port 15721
```

首次启动后打开 `http://127.0.0.1:15721`。

## 2. 初始化

### 2.1 Web 初始化

在浏览器里完成 setup 向导。

### 2.2 API 初始化（无需鉴权）

```bash
curl -fsS -X POST http://127.0.0.1:15721/api/setup/bootstrap \
  -H 'content-type: application/json' \
  -d '{"password":"password123","ownerEmail":"owner@example.com","routerUrl":"https://sgptokenswitch.cc","clientTunnelSubdomain":""}'
```

`clientTunnelSubdomain` 留空时，server 会生成可读的随机单词子域名并尽量在 Router 上验证可用性。响应中的 `sessionToken` 可直接作为 Bearer token 使用。

### 2.3 CLI 初始化（启动 HTTP 前写本地配置）

```bash
cc-switch-server init \
  --owner-email owner@example.com \
  --router-url https://sgptokenswitch.cc \
  --password password123
```

官方脚本：

```bash
scripts/bootstrap/server-init-http.sh
scripts/bootstrap/server-init-local.sh
```

## 3. 远程 OpenAI CLI OAuth

通过非本机 Client URL 管理 Server 时，OpenAI 仍只接受官方 `http://localhost:1455/auth/callback`，Server 不替换或伪造 redirect URI。Web 管理面在 HTTPS 下提供安全的手工回传流程：

1. 在 Codex OAuth 账号区选择 CLI OAuth 并打开授权链接。
2. 浏览器授权后会跳转到本机 `localhost:1455`；页面不可达是远程部署下的预期现象。
3. 从地址栏提交完整的 `http://localhost:1455/auth/callback?code=...&state=...` URL，Server 校验固定 scheme/host/port/path、state、当前管理员主体和会话期限后交换 token。

只有 Server 实际绑定 loopback 地址、请求未经过 forwarded host 且 `Host` 也是 loopback 时才允许本机例外；监听 `0.0.0.0`、`::` 或其他非 loopback 地址时，伪造 loopback `Host` 不会降级安全要求。非 loopback Client URL 必须是 Server 配置中的 HTTPS Client URL，并由同源 Web 页面发起；只接受完整 callback URL，不接受裸 code。Device OAuth 保持可用。

存在一条 Codex 凭据时该账号自动成为账号中心操作目标；存在多条凭据时可在 Web 管理面显式选择，`needs_selection` 只阻断依赖该偏好的账号中心操作，不影响已明确绑定账号的 Share 数据面。`GET /api/accounts` 等控制面响应只返回凭据存在性和运行状态，不返回或导出 access/refresh/id token、API key、extra headers、profile 或 raw 上游载荷。

其他 Provider 的登录细节见 [`../provider/claude-oauth.md`](../provider/claude-oauth.md)、[`../provider/codex-oauth.md`](../provider/codex-oauth.md)、[`../provider/grok-oauth.md`](../provider/grok-oauth.md)、[`../provider/kimi-code.md`](../provider/kimi-code.md)、[`../provider/cursor.md`](../provider/cursor.md)。

## 4. 常用命令

配置和诊断命令只读取本地配置与 JSON store，不启动 HTTP、router 注册、tunnel 或后台监听器：

```bash
cargo run -- config path
cargo run -- config print
cargo run -- config validate
cargo run -- doctor
cargo run -- doctor --check-port
```

`config print` 输出脱敏 JSON 摘要，**不打印** password/API token hash、router private key/control secret 或 provider/account token。

查看 binary 构建信息：

```bash
cargo run -- version
cargo run -- version --json
```

## 5. 本地验证

提交前建议执行：

```bash
cargo fmt -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
scripts/static-checks.sh
```

允许编译和启动本地 server 时执行完整本地验收：

```bash
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
scripts/audit/validate-local.sh
scripts/smoke/smoke-local.sh
RUN_TESTS=1 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

`RUN_TESTS=0` **仅用于负向审计**：脚本会记录 `local-contracts-unverified`，输出 `decision=blocked` 并以状态码 `1` 退出；不得将其作为本地合同或发布验收通过证据。

## 6. 真实验收

有真实 Router、Share grant、Client Market、provider 或 OAuth 端到端环境时，把变量写入**私有** env 文件后运行：

```bash
set -a
source /tmp/cc-switch-server-real.env
set +a
STRICT=1 scripts/smoke/real-acceptance-env-check.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/router-share-smoke.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
RUN_REAL=1 scripts/release-readiness.sh
```

真实验收脚本只输出脱敏摘要；缺真实输入、skeleton 未退场或部署未测时不会标记为通过。真实密钥只能存在于 shell 环境或私有临时文件（如 `/tmp/cc-switch-server-real.env`），仓库里只允许提交 `.env.example` 的占位符。

完整剧本见 [`../acceptance/real-acceptance-runbook.md`](../acceptance/real-acceptance-runbook.md)。

## 7. 下一步

- 部署到生产：[`deployment.md`](deployment.md)
- 与 Router 联调：[`router-integration.md`](router-integration.md)
- 迁移已有数据目录：[`data-migration.md`](data-migration.md)
- 远程调试：[`remote-debugging.md`](remote-debugging.md)
