# 架构总览

> 状态：**权威文档**。本文件是 cc-switch-server 架构叙述的唯一真值来源，Router 侧对应文档为 `cc-switch-router/ARCHITECTURE.md`，系统文档站对应 `tokenswitch-docsify`。三者出现冲突时，涉及 Server 内部结构的以本文为准，涉及 Router 内部结构与协议线格式的以 Router 仓库 `PROTOCOL.md` 为准。
>
> 最后核对：2026-10-25（Server `312d127`，Router `3a489ab`）。

## 1. 系统角色

Token 路由交易系统由**三个角色**构成，运行时只有**两个进程组件**：

| 角色 | 承载组件 | 仓库 | 职责 |
| --- | --- | --- | --- |
| Client | `cc-switch-server` | 本仓库 | 持有 Provider 账号与凭据，执行反代转发，向 Router 注册并维护隧道，管理 Share 与用量 |
| Router | `cc-switch-router` | `/data/projects/cc-switch-router` | 公网入口、签名校验、IngressContext 注入、SSH 反向隧道终结、区域调度 |
| Client / Share Market | `cc-switch-router`（内置） | 同上 | Share Market（拼车位挂牌、租用与授权下发）、Client Market（主机供给、开通与租约），以及两者共用的准入与资金 |

> Router 同时承担 Router 与 Client/Share Market 两个角色，二者在同一进程内实现，不是独立服务。

以下组件已下线，**不得**被描述为当前运行时组成：

- 旧 Token Market 集成。Router 上 `/v1/markets*`、`/v1/market/*`、`/_market/proxy/*` 一律返回 `410 Gone`。
- 任何 Tauri 桌面端或外部 Provider 仓库。Server 的 Provider 产品范围由本仓库 requirements/Registry 决定，证据边界见 [`PROTOCOL_EVIDENCE.md`](../../PROTOCOL_EVIDENCE.md)。

### 1.1 资金边界

Server 不参与计费：不保存价格、余额、账单或结算，只统计 Token、状态与延迟。Share Market 与 Client Market 的付费发布、报价和购买均使用 Router 的平台余额；Router 的 `disabled` / `shadow` / `enabled` 财务模式只控制新的官方渠道充值，不切换到旧的 Provider 资金轨道。`shadow` 只允许 Owner 测试充值；开放或重新开放普通用户充值须通过当前渠道及账本验收门槛。关闭充值或退回影子测试不影响既有余额的消费、转账、已发起订单的核对或成熟收益的结算，也不会恢复旧资金写入口。

Provider 的净收益完成七天沉淀后由 Router 自动计入其平台余额，可用于消费或按 Router 的规则流转；外部提现、提现地址和手动「收益转入余额」均已退役。对 Server 而言，Share Market 的租用、恢复和欠费暂停体现为 Router 经 pending Share edit 下发的 `managedGrant` `upsert`（授予/恢复）与 `revoke`（回收/暂停）；Server 落库为 `manager=routerShareMarket` 的授权并据此执行访问与限额。Client Market 欠费由 Router 侧暂停该 Client 的隧道，不需要 Server 配合。当前资金契约以 `cc-switch-router/docs/market-platform-wallet-implementation.md` 开头的现行说明和 `cc-switch-router/docs/market-earnings-automatic-credit.md` 为准，不能把前者保留的旧提现规划当作当前实现。

## 2. 两条链路

Server 对外暴露单端口 `:15721`，但链路语义完全不同的两条流量**不共用入口**。

### 2.1 管理面

```
浏览器 → cc-switch-server:15721 → 内嵌 Web UI / /api /web-api 控制面 / /health /ready /metrics
```

- 由 `src/api/` 提供，鉴权走管理员会话、API Token、邮箱验证码或 Router SSO。
- 该端口**不提供推理 API 服务**给外部买家。

### 2.2 数据面

```
Code Agent CLI
  → Router Share URL（公网）
  → cc-switch-router：验签 + 注入 IngressContext
  → SSH 反向隧道
  → cc-switch-server：Share 绑定解析
  → Provider 账号凭据
  → 上游供应商
```

- 由 `src/proxy/` 提供，入口路由集中在 `src/api/mod.rs` 的 `inference_router()`。
- 请求身份来自 Router 注入的 IngressContext，Server 侧不接受未经 Router 签名的推理调用。

推理入口路径（以 `inference_router()` 定义为准）：`/v1/messages`、`/v1/messages/count_tokens`、`/v1/chat/completions`、`/v1/responses`、`/v1/responses/compact`、`/v1/responses/input_tokens`、`/v1/models`、`/v1/images/generations`、`/v1/images/edits`、`/v1/images/files/:token`、`/v1/videos/generations`、`/v1/videos/:request_id`、`/v1/alpha/search`、Gemini 的 `/v1beta/*`、`/gemini/v1beta/*`、`/gemini/v1/*`，以及 `/claude/*`、`/codex/*`、`/backend-api/codex/*`、无前缀与双 `/v1/v1` 等兼容别名。

## 3. 代码分层

`src/lib.rs` 声明的顶层模块：

| 模块 | `.rs` 数 | 职责 |
| --- | --- | --- |
| `api/` | 46 | HTTP 路由、控制面、Web UI 资产、终端会话、请求/响应类型 |
| `proxy/` | 111 | 推理转发热路径：协议适配、流式改写、重试与降级、用量计量 |
| `domain/` | 61 | 业务模型与不变量：providers / accounts / sharing / usage / router / settings |
| `clients/` | 38 | 出站客户端：OAuth 设备流与刷新、Router 客户端、隧道、DeepSeek PoW |
| `repository/` | 5 | providers / accounts / shares / usage 的 SQLite 事务型权威库与 JSON→SQLite 迁移边界 |
| `state/` | 2 | 从 `state.rs` 拆出的后台调度器与 in-flight 准入跟踪 |
| `infra/` | 8 | 基础设施：存储、凭据加解密、HTTP 客户端、备份、时间、公网 IP、网络身份 |
| `logging/` | 4 | 日志初始化、审计、捕获缓冲 |
| `self_update/` | 4 | 版本探测、升级、重启 |
| 顶层 `.rs` | 14 | `state.rs`、`admin.rs`、`setup.rs`、`cli.rs`、`metrics.rs`、`client_tunnel_provision.rs`、`provider_identity.rs` 等 |

顶层文件（共约 40.3k 行）中 `state.rs` 单文件约 33.8k 行，仍是全仓最大的单点；其余依次为 `client_tunnel_provision.rs`（约 1.4k）、`admin.rs`（约 1.3k）、`metrics.rs`（约 1.0k）、`setup.rs`（约 0.8k）。

### 3.1 api/

- 平铺模块：`accounts` `providers` `shares` `router` `usage` `settings` `events` `backup` `self_update` `session` `logs` `models` `debug` `error` `request_audit` `provider_health_scheduler` `subscription_quota`。
- 子目录：`control/`（Router 控制面与 `/_share-router/*` 探针）、`invoke/`（`/web-api/invoke/*` 兼容派发）、`web/`（内嵌资产、运行时上下文、覆盖率视图）、`terminal/`（运维终端会话）、`types/`（对外类型）。
- 路由定义集中在 `app_router()`，是接口的唯一权威来源。

### 3.2 proxy/

平铺文件按供应商与关注点划分，主要有 `forwarder.rs`（转发主循环）、`adapters.rs`/`transforms.rs`/`stream_transforms.rs`（协议改写）、`streaming.rs`、`retry_policy.rs`、`request_governance.rs`、`usage.rs`，以及供应商专属的 `claude_oauth.rs` `codex_models.rs` `codex_request_policy.rs` `copilot_model_map.rs` `copilot_optimizer.rs` `grok.rs` `kimi.rs`/`kimi_runtime.rs` `kiro.rs` `qoder.rs`/`qoder_runtime.rs` `deepseek.rs` 等；子目录 `cursor/`（Agent 协议桥接）、`kiro/`、`deepseek/`、`providers/`、`execution/`、`forwarder/`、`codex_instructions/`。

Cursor 的 live h2 session 与 completed Responses context 是两个独立状态域。`session.rs` 仅保存可续传的 h2 writer/tool-call 映射，并对相同 Running conversation 返回冲突；`response_state.rs` 仅保存有界、规范化、无凭据的 input/output items，用于 `previous_response_id` 新开 run。该短期 continuation context 也适用于 `store=false`，但只存在于进程内且不形成可查询的 Responses 资源。二者都按 Provider、rail、runtime、credential identity、Share/user 等维度 fencing，不能互相降级或跨 scope 查找。

`tool_schema.rs`/`tool_resolver.rs` 在客户端边界校验声明工具；required/named 请求由 `agent_driver.rs` 在业务输出提交前缓冲，最多三次同绑定 semantic attempt。它不执行工具，也不引入 Provider/账号选择或跨 rail fallback。

### 3.3 domain/

| 子域 | 关键文件 |
| --- | --- |
| `providers/` | `store.rs`/`store_v2.rs`（S1/S2 存储格式）、`credentials.rs`、`registry.rs`、`matrix.rs`、`model_routing.rs`、`storage_migration.rs` |
| `accounts/` | `store.rs`、`oauth.rs`、`login.rs`、`claude_subscription.rs`、`grok_subscription.rs`、`capability_evidence.rs`、`subscription_expiry.rs` |
| `sharing/` | `shares.rs`、`invariants.rs`、`retired_fields.rs`、`router_contract.rs`、`share_router_domain.rs`、`credential_source.rs`、`token_period.rs`、`legacy_token_market_migration.rs` |
| `usage/` | `store.rs`、`query.rs` |
| `router/` | `namespace.rs` |
| `settings/` | `config.rs`、`ui_settings.rs` |

### 3.4 clients/

- `oauth/`：Claude / Codex / Gemini / Antigravity / Copilot / Cursor / Grok / Kimi / Kiro / Amazon Q / Qoder / CodeBuddy / Trae 的设备流、JWKS、刷新、模型目录与配额查询（26 个文件）。
- `router/`：`client.rs`、`tunnel.rs`、`ingress.rs`、`control_store.rs`、`email_auth.rs`。
- `deepseek/`：含 `pow.rs`。
- 顶层：`coding_plan_quota.rs`、`ollama_cloud.rs`。

## 4. 强约束

以下规则来自根目录 [`AGENTS.md`](../../AGENTS.md)，在此复述以便架构读者一次看全；两处冲突时以 `AGENTS.md` 为准。

### 4.1 依赖方向

- `domain` **不得**依赖 `api`、`clients`、`proxy`。
- `proxy` **不得**依赖 `api/http` 或 `clients`。
- 转发热路径需要触发出站 OAuth / Router 客户端时，必须经由 `state.rs` 或控制面编排方法封装状态读写、锁与持久化策略。

### 4.2 状态写入

- 新代码禁止在 `state.rs` 之外对 `ServerStateInner` 的存储字段 `.write().await` 后直接改数据；必须走 `ServerStateInner` 的域方法。
- 跨存储写操作按字段声明顺序取锁：
  `config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins`。
- shares 写路径已收敛到 `mutate_shares_immediate` / `try_mutate_shares_immediate` / `mutate_shares_debounced` / `mutate_share` / `replace_shares` / `validate_share_invocation`；调用方不得再感知立即保存或 debounce 落盘细节。

### 4.3 UI 独立性

Web UI 只以本仓库产品需求、Server API 和 `assets/contract/web-runtime-contract.json` 为实现依据；禁止从 cc-switch 或其他外部项目批量复制组件、样式、locale 与页面结构。

## 5. 相关文档

- 协议与 Router 契约：[`router-contract.md`](router-contract.md)
- 本地持久化与数据布局：[`storage.md`](storage.md)
- 用量计量：[`usage-accounting.md`](usage-accounting.md)
- Share 访问模型：[`../share/access-policy.md`](../share/access-policy.md)
- 文档索引：[`../README.md`](../README.md)
