<h1 align="center">cc-switch-server</h1>

<p align="center"><strong>一个无桌面依赖的 code-agent token server，为 Claude、Codex、Gemini 及 cc-switch 供应商提供 Web 管理、反代转发和 share/router 联通能力。</strong></p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-async-000000?style=flat-square&logo=rust">
  <img alt="Apps" src="https://img.shields.io/badge/Claude%20%2F%20Codex%20%2F%20Gemini-proxy-2563eb?style=flat-square">
  <img alt="Runtime" src="https://img.shields.io/badge/runtime-binary%20%2B%20web-16a34a?style=flat-square">
  <img alt="Storage" src="https://img.shields.io/badge/storage-JSON-0f766e?style=flat-square">
</p>

`cc-switch-server` 是独立 server 产品，不是 `cc-switch` 的派生 UI 或整仓 fork。Server 的产品需求、API、运行时契约和 Web UI 均在本仓库独立设计与维护；外部项目只用于审计 Claude、Codex、Gemini Provider 类型和协议行为，不作为代码或界面同步源。

当前仓库只维护 server 运行路径：HTTP API、静态 Web UI、本地 JSON store、反代转发、router/share tunnel 和真实验收脚本。不迁移 Tauri window/tray/updater/deeplink、Claude Desktop profile 写入、MCP、skills、session manager 和桌面安装资产。

## 在系统中的位置

Token 路由交易系统有三个角色、两个运行时组件：**Client**（本仓库 `cc-switch-server`）、**Router** 与 **Client / Share Market**（后两者同为 `cc-switch-router` 进程）。

管理面链路：

```text
browser / operator
  -> cc-switch-server :15721
  -> Web UI / control plane / health
```

数据面链路：

```text
Claude / Codex / Gemini client
  -> Router Share URL / signed Gateway route
  -> cc-switch-router
  -> signed ingress over SSH reverse tunnel
  -> cc-switch-server Share binding
  -> provider adapter / bound account
  -> upstream provider or OAuth backend
```

`:15721` **不对外提供推理 API**。完整架构见 [`docs/architecture/overview.md`](docs/architecture/overview.md)。

## 特性

- **管理面**：setup、password / API token 登录和 router 邮箱验证码登录；Web UI 覆盖 provider、account、share、usage、router、backup、diagnostics。
- **多协议反代**：Claude Messages、Codex Chat Completions / Responses（双向互转）、Gemini `/v1beta/*`、OpenAI-compatible `/v1/models`；已接入 OpenRouter、Ollama、Nvidia、DeepSeek、SubRouter、OpenCode Go 等 preset。
- **Server-native OAuth**：Claude / Codex（Device + CLI PKCE）/ Gemini / Antigravity / Cursor / Copilot / Kiro / Grok / Kimi / Qoder 的登录、刷新、profile 与 quota，全部在 Server 侧完成，无桌面依赖。
- **显式绑定，不做故障转移**：Managed OAuth Provider Bundle 必须显式绑定账号；请求不按占用、quota、cooldown、并发或错误切换账号，首个 401 只在原账号强刷并重放一次。
- **Router 集成**：installation register、client tunnel、share tunnel、share batch sync、Router Share request log sync、pending share edit pull/ack/事件监听；ingress 新鲜度采用非对称窗口（≤30s 前签发、≤5s 未来）。
- **用量计量**：记录完整 request lifecycle、Provider Bundle/Surface、Share/用户、实际上游模型、重试、延迟与 Token 观测状态，通过 Server-native REST 提供聚合、筛选、明细和 cursor 分页。**只统计 Token / 状态 / 延迟，不计算成本或 USD 金额**，也不保存外部 Token Market 用户、价格或账本。
- **持久化安全**：JSON 写入使用 temp file fsync + atomic rename + 父目录 fsync；凭据以 XChaCha20-Poly1305 加密；`/api/backup` 支持创建、列出、恢复，恢复前自动 pre-restore 快照。
- **可观测**：`/web-api/events` 认证 SSE 推送 Usage/Share/tunnel 事件；`/metrics` 暴露 Prometheus 指标；`version --json` 与 `/version` 输出版本、commit、build time、target、profile、rustc 和 dirty 状态。

## Code Agent 反代支持

`cc-switch-server` 聚焦 **Claude Code / Codex CLI / Gemini CLI** 三类官方 CLI 客户端入口。下表路径全部相对于同一个 Router Share URL。

| Code Agent | 反代入口 | 状态 | 说明 |
| --- | --- | --- | --- |
| **Claude Code** | `POST /v1/messages` | ✅ Native | Anthropic Messages 原生转发；支持 Claude/Codex/Gemini/OpenRouter 等跨协议 adapter |
| **Codex CLI** | `POST /v1/responses`、`GET /v1/responses` (WebSocket)、`POST /v1/chat/completions`、`POST /v1/images/generations`、`POST /v1/images/edits` | ✅ Native | Responses/Chat 互转；Device + CLI OAuth；Images/Responses Cloudflare 心跳与持久化 capability URL；有界 WS cache 与提交前 HTTP/SSE fallback |
| **Gemini CLI** | `POST /v1beta/*` | ✅ Native | Gemini Generative API 透传；`GET /v1beta/models` 等列表端点已覆盖 |
| **OpenAI-compatible** | `GET /v1/models`、`GET /models` | ✅ Native | 模型列表与 OpenAI-compatible 探测 |
| **Antigravity IDE** | 经 provider 预设映射到 Claude/Gemini 接口 | ⚠️ Partial | OAuth/模型列表已接入；无独立 `/antigravity/v1*` 路由组 |
| **Cursor** | 作为 Claude/Codex 上游桥（非 IDE MITM） | 🧪 Experimental | OAuth CLI / API Key SDK 双 rail；固定单凭据与 runtime-secret endpoint，待分轨真实验收 |
| **GitHub Copilot** | 作为 Claude 上游桥 | ⚠️ Fallback | 静态 preflight 与 model map 已接入；token 交换与 live 回归待验收 |
| **Kiro** | 作为 Claude 上游桥 | ⚠️ Planned | CodeWhisperer 协议桥已静态接线；仅 Claude app，待真实验收 |
| **DeepSeek Account** | 作为 Claude 上游桥 | ⚠️ Planned | 账密协议桥与 PoW 已接线；Codex/Gemini 路径仍为 skeleton |
| **Cline / OpenCode / Qoder / Trae / Windsurf / Zed** | — | ❌ 不支持 | server 产品边界不覆盖这些 IDE 专属 MITM 或插件生态 |

能力分级：`✅ Native` = 静态 adapter contract 已覆盖且属主线验收对象；`⚠️ Planned` = 转发/签名已接线但缺真实 non-stream/stream 验收；`⚠️ Fallback` = skeleton 或 manual import 路径；`❌` = 未实现。详见 [`docs/provider/regression-matrix.md`](docs/provider/regression-matrix.md)。

> **产品边界**：不依赖 Tauri 桌面运行时，**不提供 Claude Code 热切换**（需重启 CLI 使 provider 变更生效）；提供 Server-native OAuth、share/router 隧道、Web 管理面、remote usage 同步与多租户 share binding。

### 供应商 × App 能力矩阵（摘要）

| 供应商类型 | Claude | Codex | Gemini | 能力 |
| --- | :---: | :---: | :---: | --- |
| Claude API / Auth / OAuth | ✅ | — | — | Native |
| Codex / OpenAI OAuth | ✅ | ✅ | — | Native |
| Gemini / Gemini CLI OAuth | ✅ | ✅ | ✅ | Native |
| OpenRouter / Ollama / Nvidia / DeepSeek API | ✅ | ✅ | ✅ | Native |
| Antigravity / Agy OAuth | ✅ | — | ✅ | Native（经预设映射） |
| Cursor OAuth / API Key | 🧪 | 🧪 | 🧪 | Experimental（CLI/SDK 分轨接线，runtime endpoint 必配，live-unverified） |
| AWS Bedrock | ⚠️ | ⚠️ | ⚠️ | Planned（SigV4 合同已生成） |
| GitHub Copilot | ⚠️ | ⚠️ | ⚠️ | Fallback |
| Kiro OAuth | ⚠️ | — | — | Planned（仅 Claude） |
| DeepSeek Account | ⚠️ | — | — | Planned（仅 Claude） |

完整 provider 类型与 preset 覆盖见 [`docs/provider/coverage.md`](docs/provider/coverage.md)；运行时矩阵可通过 `GET /api/provider-matrix` 获取。

## 快速开始

```bash
cargo run -- --host 0.0.0.0 --port 15721
# 打开 http://127.0.0.1:15721 完成 setup
```

完整初始化方式（Web / API / CLI）、远程 OpenAI CLI OAuth、常用命令与本地验证步骤见 [`docs/guide/getting-started.md`](docs/guide/getting-started.md)。

提交前的最小验证：

```bash
cargo fmt -- --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
scripts/static-checks.sh
```

## 部署

```bash
cargo build --release
sudo install -m 0755 target/release/cc-switch-server /usr/local/bin/cc-switch-server
```

systemd unit 位于 `deploy/cc-switch-server.service`，默认使用 `/var/lib/cc-switch-server` 作为配置目录。Docker、GitHub Actions 发布流程与生产注意事项见 [`docs/guide/deployment.md`](docs/guide/deployment.md)。

## 配置

默认配置目录为 `~/.cc-switch-server`，可用 `--config-dir` / `CC_SWITCH_SERVER_CONFIG_DIR` 覆盖。常用参数：

| 领域 | 配置 |
| --- | --- |
| 监听地址 | `--host` / `CC_SWITCH_SERVER_HOST`，默认 `127.0.0.1` |
| 监听端口 | `--port` / `CC_SWITCH_SERVER_PORT`，默认 `15721` |
| 配置目录 | `--config-dir` / `CC_SWITCH_SERVER_CONFIG_DIR` |
| 日志级别 | `--log-level` / `CC_SWITCH_SERVER_LOG`，默认 `info` |

**全部**配置项、环境变量和 Provider 存储格式迁移见 [`docs/guide/configuration.md`](docs/guide/configuration.md)；数据文件布局与加密方式见 [`docs/architecture/storage.md`](docs/architecture/storage.md)。

数据目录中的文件可能包含 token、secret 或账号信息，**不能提交到 git**。

## Router 联调

setup 时填入 Router API base 即可完成 `register → owner bind → client tunnel claim`。完整联调步骤、验收重点、相关 API 与排障见 [`docs/guide/router-integration.md`](docs/guide/router-integration.md)。

## API 入口

常用健康与管理入口：`GET /health`、`GET /ready`、`GET /metrics`、`GET /version`、`GET /api/setup/status`、`POST /api/setup/bootstrap`、`POST /api/auth/login`、`GET /api/providers`、`GET /api/accounts`、`GET /api/shares`、`GET /api/router/tunnels`、`GET /api/backup`、`GET /web-api/events`、`GET /web-api/usage/*`。

Usage 查询使用 `[fromMs, toMs)`，明细范围最多 32 天；趋势接口单次最多返回 2,000 个时间桶。

Router Share URL 下的反代入口：`POST /v1/messages`、`POST /v1/chat/completions`、`POST /v1/responses`、`POST /v1beta/*`。

完整接口以 `src/api/mod.rs` 的 `app_router()` 定义为准。

## 文档

全部文档的索引、权威性标记与状态见 **[`docs/README.md`](docs/README.md)**。

常用入口：

- [架构总览](docs/architecture/overview.md) · [Router 契约](docs/architecture/router-contract.md) · [存储](docs/architecture/storage.md)
- [快速开始](docs/guide/getting-started.md) · [配置参考](docs/guide/configuration.md) · [部署](docs/guide/deployment.md)
- [Provider 覆盖](docs/provider/coverage.md) · [回归矩阵](docs/provider/regression-matrix.md)
- [Share 访问策略](docs/share/access-policy.md)
- [外部 Provider 审计台账](UPSTREAM_IMPORT.md) · [开发约定](AGENTS.md)
