<h1 align="center">cc-switch-server</h1>

<p align="center"><strong>一个无桌面依赖的 code-agent token server，为 Claude、Codex、Gemini 及 cc-switch 供应商提供 Web 管理、反代转发和 share/router 联通能力。</strong></p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-async-000000?style=flat-square&logo=rust">
  <img alt="Apps" src="https://img.shields.io/badge/Claude%20%2F%20Codex%20%2F%20Gemini-proxy-2563eb?style=flat-square">
  <img alt="Runtime" src="https://img.shields.io/badge/runtime-binary%20%2B%20web-16a34a?style=flat-square">
  <img alt="Storage" src="https://img.shields.io/badge/storage-JSON-0f766e?style=flat-square">
</p>

`cc-switch-server` 是独立 server 产品，不是 upstream desktop `cc-switch` 的整仓 fork。它选择性吸收 desktop 中服务于 Claude、Codex、Gemini 反代主线的 provider、OAuth、usage、pricing、share 和 router 能力，并把桌面依赖替换为单机 Web/API 服务。

当前仓库只维护 server 运行路径：HTTP API、静态 Web UI、本地 JSON store、反代转发、router/share tunnel 和真实验收脚本。不迁移 Tauri window/tray/updater/deeplink、Claude Desktop profile 写入、MCP、skills、session manager 和桌面安装资产。

典型链路：

```text
Claude / Codex / Gemini client
  -> cc-switch-server local or public endpoint
  -> provider adapter / account manager / usage recorder
  -> upstream provider or OAuth backend
```

注册到 router 后的 share 链路：

```text
market or direct share URL
  -> cc-switch-router
  -> SSH reverse tunnel
  -> cc-switch-server share binding
  -> selected provider / account
```

## 特性

- 提供 setup、password/API token 登录和 router 邮箱验证码登录；Web UI 覆盖 provider、account、share、usage、router、pricing、backup 和 diagnostics 常用操作。
- 支持 Claude、Codex、Gemini 三类入口：`/v1/messages`、`/v1/chat/completions`、`/v1/responses`、Gemini `/v1beta/*` 和 OpenAI-compatible `/v1/models`/`/models`。
- 保留 cc-switch provider metadata、AuthBinding、未知扩展字段和 Universal Provider 模型配置，导入/同步时尽量不丢 desktop 配置。
- 支持 provider / Universal Provider JSON 导入导出；导入时按 server 当前分类逻辑重新 upsert，不信任旧导出里的分类结果。
- 已实现 Codex Chat Completions 与 Responses 的直接互转，保留 max/reasoning/response_format/tool/usage 等 Codex bridge 关键字段。
- 已接入 Claude/Codex/Gemini/OpenAI-compatible/Gemini-native/Anthropic-native 之间的主要跨协议 adapter contract，并把 OpenRouter、Ollama、Nvidia、DeepSeek、SubRouter、OpenCode Go 等 preset 纳入 coverage。
- Cursor 三入口保持 AgentService planned；已移植协议、请求、事件、tool、h2、session、identity、image 前置层，并在显式 opt-in 下接入 Claude/Codex/Gemini AgentService driver。
- GitHub Copilot 和 Kiro 已提供 device flow 静态导入路径；真实 token refresh、live models、usage 和 proxy 回归完成前仍保持 fallback/manual-import。
- Codex、Claude、Gemini、Ollama、Antigravity/Agy 等账号在手动导入 refresh token 后可执行 server-native refresh/profile/quota；proxy 转发前会自动刷新临近过期的 managed account。
- 支持 router installation register、client tunnel、share tunnel、share batch sync、direct share request log sync、pending share edit pull/ack/event 监听。
- 支持 share-market grant add/revoke 通过 router pending edit 应用到 server share，并同步 per-app 授权展示状态。
- usage log 记录 requestId、sessionId、source、provider、model、stream status、cache/usage detail，并提供 summary/trends/provider/model stats。
- 内置全局模型定价和 limits 运维面：模型定价 CRUD、成本回填、provider 日/月成本、账号 quota 和 share 限额展示。
- JSON 写入使用 temp file fsync、atomic rename 和父目录 fsync；`/api/backup` 支持创建、列出、恢复主要 store，恢复前自动 pre-restore 快照。
- `/api/events` 通过 SSE 推送 usage/share/tunnel 事件，Web 当前页会 debounce 刷新。
- `cc-switch-server version --json` 和 `/version` 会输出版本、commit id、commit message、build time、target、profile、rustc 和 dirty 状态。

## Code Agent 反代支持

`cc-switch-server` 聚焦 **Claude Code / Codex CLI / Gemini CLI** 三类官方 CLI 客户端入口，并选择性吸收 desktop `cc-switch` 的 provider 桥接能力。下表评分口径与同生态中 9router、CLIProxyAPI、OmniRoute、sub2api、cockpit-tools、cc-switch、composer-api 的静态分析一致（0–10 分，侧重协议覆盖、格式互译、认证多账号、健壮性与可运维性；对比来源见本地 `proxy/proxy.md`）。

### 支持的客户端入口

| Code Agent | 反代入口 | 状态 | 说明 |
| --- | --- | --- | --- |
| **Claude Code** | `POST /v1/messages` | ✅ Native | Anthropic Messages 原生转发；支持 Claude/Codex/Gemini/OpenRouter 等跨协议 adapter |
| **Codex CLI** | `POST /v1/responses`、`POST /v1/chat/completions` | ✅ Native | Responses 与 Chat Completions 互转；Codex OAuth device flow 已接线 |
| **Gemini CLI** | `POST /v1beta/*` | ✅ Native | Gemini Generative API 透传；`GET /v1beta/models` 等列表端点已覆盖 |
| **OpenAI-compatible** | `GET /v1/models`、`GET /models` | ✅ Native | 模型列表与 OpenAI-compatible 探测 |
| **Antigravity IDE** | 经 provider 预设映射到 Claude/Gemini 接口 | ⚠️ Partial | OAuth/模型列表已接入；无独立 `/antigravity/v1*` 路由组 |
| **Cursor** | 作为 Claude/Codex 上游桥（非 IDE MITM） | ⚠️ Planned | AgentService h2/protobuf 静态 driver 已接线，需显式 opt-in；待真实验收 |
| **GitHub Copilot** | 作为 Claude 上游桥 | ⚠️ Fallback | 静态 preflight 与 model map 已接入；token 交换与 live 回归待验收 |
| **Kiro** | 作为 Claude 上游桥 | ⚠️ Planned | CodeWhisperer 协议桥已静态接线；仅 Claude app，待真实验收 |
| **DeepSeek Account** | 作为 Claude 上游桥 | ⚠️ Planned | 账密协议桥与 PoW 已接线；Codex/Gemini 路径仍为 skeleton |
| **Cline / OpenCode / Qoder / Trae / Windsurf / Zed** | — | ❌ 不支持 | server 产品边界不覆盖这些 IDE 专属 MITM 或插件生态 |

能力分级：`✅ Native` = 静态 adapter contract 已覆盖且属主线验收对象；`⚠️ Planned` = 转发/签名已接线但缺真实 non-stream/stream 验收；`⚠️ Fallback` = skeleton 或 manual import 路径；`❌` = 未实现。详见 [`docs/code-agent-regression-matrix.md`](docs/code-agent-regression-matrix.md)。

### 与同类反代项目横向对比（0–10）

| Code Agent | 9router | CLIProxyAPI | OmniRoute | sub2api | cockpit-tools | cc-switch | **cc-switch-server** | composer-api |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Claude Code | 8.5 | **9.5** | 9.0 | 8.0 | 6.5 | 9.0 | **8.5** | 0 |
| Codex CLI | 7.5 | **9.8** | 8.5 | 9.0 | 7.0 | 9.2 | **9.0** | 0 |
| Gemini CLI | 7.5 | **9.5** | 8.5 | 8.0 | 6.0 | 8.0 | **7.8** | 0 |
| Antigravity | 6.5 | **9.5** | 8.5 | 9.0 | 7.5 | 6.0 | **5.5** | 0 |
| Cursor | 8.0 | 0 | **9.5** | 2.5 | 4.5 | 9.0 | **8.5** | **9.2** |
| GitHub Copilot | 8.0 | 0 | 8.5 | 0 | 5.0 | **8.5** | **7.5** | 0 |
| Cline | **8.0** | 0 | 3.0 | 0 | 0 | 0 | **0** | 0 |
| OpenCode | 8.0 | 0 | **9.5** | 0 | 3.0 | 5.5 | **2.5** | 2.0 |
| Kiro | **9.0** | 0 | 8.5 | 0 | 5.0 | 8.5 | **7.5** | 0 |
| Qoder | **8.0** | 0 | 7.5 | 0 | 5.0 | 0 | **0** | 0 |
| Trae | 0 | 0 | **8.5** | 0 | 5.0 | 0 | **0** | 0 |
| Windsurf | 0 | 0 | **7.0** | 0 | 5.0 | 0 | **0** | 0 |
| Zed | 0 | 0 | 5.0 | 0 | 5.0 | 0 | **0** | 0 |
| **平均（13 agents）** | 6.08 | 2.95 | 7.81 | 2.81 | 4.96 | 4.90 | **4.37** | 0.86 |
| **核心 4（Claude/Codex/Gemini/Antigravity）均分** | 7.50 | **9.58** | 8.63 | 8.50 | 6.75 | 8.05 | **7.70** | 0 |
| **IDE 体验类 4（Cursor/Copilot/Kiro/Qoder）均分** | 8.25 | 0 | 8.50 | 0.63 | 4.88 | 6.50 | **5.88** | 2.30 |

> **cc-switch-server 与 desktop cc-switch 的主要差异**：不依赖 Tauri 桌面运行时，**不提供 Claude Code 热切换**（需重启 CLI 使 provider 变更生效）；OAuth 浏览器登录部分能力仍待 server-native 接线；**额外提供** share/router 隧道、Web 管理面、remote usage 同步与多租户 share binding。Cursor/Kiro/Copilot/DeepSeek 等跨厂商后端桥与 desktop 共用 Rust 反代实现，但 capability 升级仍以真实验收为 gate。
>
> 其他项目分数摘自本地 `proxy` 目录静态分析（2026-07）；`cockpit-tools` 反代能力继承自内嵌 CLIProxyAPI sidecar，自身侧重账号管理 GUI。

### 供应商 × App 能力矩阵（摘要）

| 供应商类型 | Claude | Codex | Gemini | 能力 |
| --- | :---: | :---: | :---: | --- |
| Claude API / Auth / OAuth | ✅ | — | — | Native |
| Codex / OpenAI OAuth | ✅ | ✅ | — | Native |
| Gemini / Gemini CLI OAuth | ✅ | ✅ | ✅ | Native |
| OpenRouter / Ollama / Nvidia / DeepSeek API | ✅ | ✅ | ✅ | Native |
| Antigravity / Agy OAuth | ✅ | — | ✅ | Native（经预设映射） |
| Cursor OAuth / API Key | ⚠️ | ⚠️ | ⚠️ | Planned（AgentService opt-in） |
| AWS Bedrock | ⚠️ | ⚠️ | ⚠️ | Planned（SigV4 合同已生成） |
| GitHub Copilot | ⚠️ | ⚠️ | ⚠️ | Fallback |
| Kiro OAuth | ⚠️ | — | — | Planned（仅 Claude） |
| DeepSeek Account | ⚠️ | — | — | Planned（仅 Claude） |

完整 provider 类型与 preset 覆盖见 [`docs/provider-coverage.md`](docs/provider-coverage.md)；运行时矩阵可通过 `GET /api/provider-matrix` 获取。

## 快速开始

开发启动：

```bash
cargo run -- --host 0.0.0.0 --port 15721
```

显式 `serve` 子命令与无子命令启动兼容：

```bash
cargo run -- serve --host 0.0.0.0 --port 15721
```

首次启动后打开：

```text
http://127.0.0.1:15721
```

或直接调用 setup API：

```bash
curl -X POST http://127.0.0.1:15721/api/setup \
  -H 'content-type: application/json' \
  -d '{"password":"password123","ownerEmail":"owner@example.com","routerUrl":"https://router.example.com","clientTunnelSubdomain":""}'
```

`clientTunnelSubdomain` 为空时，server 会按 owner email 前缀最多 5 位加 5 位随机小写字母生成。

查看 binary 构建信息：

```bash
cargo run -- version
cargo run -- version --json
```

## 常用命令

配置和诊断命令只读取本地配置与 JSON store，不启动 HTTP、router 注册、tunnel 或后台监听器：

```bash
cargo run -- config path
cargo run -- config print
cargo run -- config validate
cargo run -- doctor
cargo run -- doctor --check-port
```

`config print` 输出脱敏 JSON 摘要，不打印 password/API token hash、router private key/control secret 或 provider/account token。

## 本地验证

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
RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=1 scripts/release-readiness.sh
```

有真实 router、market、provider、OAuth 或 share-market grant 输入时，把变量写入私有 env 文件后运行：

```bash
set -a
source /tmp/cc-switch-server-real.env
set +a
STRICT=1 scripts/smoke/real-acceptance-env-check.sh
RUN_PROBES=1 STREAM_PROBE=1 scripts/smoke/direct-market-diagnostics.sh
RUN_REAL=1 STREAM_PROBE=1 scripts/smoke/code-agent-regression.sh
RUN_REAL=1 scripts/release-readiness.sh
```

真实验收脚本只输出脱敏摘要；缺真实输入、skeleton 未退场或部署未测时不会标记为通过。

## 部署

构建并安装 binary：

```bash
cargo build --release
sudo install -m 0755 target/release/cc-switch-server /usr/local/bin/cc-switch-server
```

systemd unit 位于 `deploy/cc-switch-server.service`：

```bash
sudo install -m 0644 deploy/cc-switch-server.service /etc/systemd/system/cc-switch-server.service
sudo systemctl daemon-reload
sudo systemctl enable --now cc-switch-server
```

默认 unit 使用 `/var/lib/cc-switch-server` 作为配置目录。生产环境应固定该目录并做备份，里面包含 provider、account token、share、tunnel、usage 和 router identity JSON。

Docker：

```bash
docker build -t cc-switch-server .
docker run --rm -p 15721:15721 -v cc-switch-server-data:/data/cc-switch-server cc-switch-server
```

GitHub Actions 中的 `Build and Release` workflow 会在 `main` 分支 push 后构建 Linux AMD64/ARM64 binary，并覆盖发布 `latest` release。

## Router / Market 联调

1. 启动 server，打开 `http://server-host:15721` 完成 setup。
2. Router URL 填 router API base，例如 `https://router.example.com`。
3. setup 完成后 server 可执行 `register -> client tunnel claim -> lease -> SSH reverse tunnel`；失败不会影响本地 Web，可在 Router 页查看错误。
4. 添加 provider 或 account 后创建 share；未填写 share subdomain 时，server 会自动生成。
5. 点击 share tunnel start 后，server 会 claim share subdomain、申请 `http` lease 并建立 SSH reverse tunnel。
6. 点击 Router 页的 Batch sync，把 share descriptor 同步给 router。
7. share-market grant 会通过 router pending share edit 下发；server 后台监听 edit event，也可手动调用 `POST /api/router/share-edits/pull` 拉取并回写 ack。
8. router 可经 share tunnel 调 `/_share-router/health`、`/_share-router/request-logs`、`/_share-router/share-runtime`、`/_share-router/model-health` 拉取 runtime。
9. `/_ctl/apply_share_settings` 和 `/_ctl/refresh_share_usage` 使用 router `control_secret` HMAC、timestamp、nonce 防重放。
10. direct share URL 请求会按 `X-CC-Switch-Share-Id` 选择 share binding，并将 `dataSource=direct` 的 request log 同步到 router；market source 日志不由 server 回传，避免与 market 侧计费日志重复。

联调验收重点：

- router client 表中 0 share client 也应显示在线/健康。
- router share 表能看到 server share 的 owner、subdomain、app runtime、provider 和 quota 展示字段。
- market API URL 能调度 server share。
- direct share API URL 能直接调用 server share，router request log 不重复且保留 country/IP/source。
- share-market grant add/revoke 能通过 pending share edit 应用到 server share。

## 关键配置

默认配置目录为 `~/.cc-switch-server`。常用参数和环境变量：

| 领域 | 配置 |
| --- | --- |
| 监听地址 | `--host` / `CC_SWITCH_SERVER_HOST`，默认 `0.0.0.0` |
| 监听端口 | `--port` / `CC_SWITCH_SERVER_PORT`，默认 `15721` |
| 配置目录 | `--config-dir` / `CC_SWITCH_SERVER_CONFIG_DIR`，默认 `~/.cc-switch-server` |
| 静态 Web | 默认使用构建时内嵌到 binary 的 Web UI；`--web-dist-dir` / `CC_SWITCH_SERVER_WEB_DIST_DIR` 仅用于开发或调试时覆盖静态目录 |
| 日志级别 | `--log-level` / `CC_SWITCH_SERVER_LOG`，默认 `info` |
| OAuth client | Gemini 浏览器登录需要 `CC_SWITCH_SERVER_GEMINI_CLIENT_ID` / `CC_SWITCH_SERVER_GEMINI_CLIENT_SECRET`；Antigravity/Agy 浏览器登录需要 `CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_ID` / `CC_SWITCH_SERVER_ANTIGRAVITY_CLIENT_SECRET` |
| 真实验收 | `ROUTER_BASE_URL`、`MARKET_URL`、`MARKET_API_URL`、`ROUTER_API_TOKEN`、`SHARE_MARKET_GRANT_TOKEN` |
| stream 验收 | `STREAM_PROBE`、`REQUIRE_STREAM_USAGE` |
| release readiness | `RUN_REAL`、`RUN_DEPLOYMENT_TESTS` |

主要本地 store：

- `server.json`：owner、password hash、router、client tunnel subdomain 和 installation identity。
- `providers.json` / `universal-providers.json`：provider 和 Universal Provider 配置。
- `accounts.json`：账号 token、profile、quota、raw snapshot。
- `shares.json` / `tunnels.json`：share、binding、ACL、market grant 和 tunnel runtime。
- `usage-logs.jsonl` / `usage-rollups.json`：请求明细和统计 rollup。
- `model-pricing.json`、`failover.json`、`email-auth.json`。

这些文件可能包含 token、secret 或账号信息，不能提交到 git。

## API 入口

常用健康和管理入口：

- `GET /health`
- `GET /version`
- `GET /api/setup/status`
- `POST /api/setup`
- `POST /api/auth/login`
- `GET /api/provider-coverage`
- `GET /api/provider-matrix`
- `GET /api/events`
- `GET /api/backup`
- `GET /api/providers`
- `GET /api/accounts`
- `GET /api/shares`
- `GET /api/router/tunnels`
- `GET /api/usage/summary`

反代入口：

- `POST /v1/messages`
- `POST /v1/chat/completions`
- `POST /v1/responses`
- `POST /v1beta/*`

完整接口以 `src/http.rs` 的 router 定义为准。

## 文档

- [上游吸收台账](UPSTREAM_IMPORT.md)
- [UI 对齐实施计划](docs/server-desktop-ui-parity-plan.md)
- [UI 人工验收清单](docs/manual-ui-checklist.md)
- [部署](docs/deployment.md)
- [真实验收 runbook](docs/real-acceptance-runbook.md)
- [provider 覆盖](docs/provider-coverage.md)
- [usage token accounting](docs/usage-token-accounting.md)
