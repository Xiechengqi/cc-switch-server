# 文档索引

> 本文件是 cc-switch-server 全部文档的**唯一索引**。新增文档必须在此登记并标注状态，否则 `scripts/static-checks.sh` 的文档检查会失败。
>
> 最后核对：2026-09-02。

## 状态标记

| 标记 | 含义 |
| --- | --- |
| **权威** | 描述当前实现，可据以判断行为；改代码时必须同步更新 |
| **生成** | 由脚本生成，**不要手工编辑**，改动源在脚本或 `assets/contract/` |
| **历史** | 已归档，只作过程与决策证据保留；**不得**据此判断当前实现 |
| **数据** | 不是叙述性文档，是脚本消费的固定装置或样本 |

## 架构 `docs/architecture/`

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [overview.md](architecture/overview.md) | 权威 | 三角色拓扑、两条链路、8 个模块的分层与强约束。**架构叙述的真值来源** |
| [router-contract.md](architecture/router-contract.md) | 权威 | Server 侧实现的 Router 协议：控制面、探针、IngressContext、Share 契约 v2 |
| [storage.md](architecture/storage.md) | 权威 | 数据目录、文件清单、凭据加密、用量存储、备份、写入锁序 |
| [usage-accounting.md](architecture/usage-accounting.md) | 权威 | Usage token 计量口径 |

## 指南 `docs/guide/`

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [getting-started.md](guide/getting-started.md) | 权威 | 启动、三种初始化方式、远程 CLI OAuth、常用命令、本地验证与真实验收 |
| [configuration.md](guide/configuration.md) | 权威 | **全部**配置项与环境变量、请求体上限、Provider 存储格式迁移 |
| [router-integration.md](guide/router-integration.md) | 权威 | Router 联调步骤、验收重点、相关 API、排障 |
| [deployment.md](guide/deployment.md) | 权威 | 生产部署 |
| [data-migration.md](guide/data-migration.md) | 权威 | 数据目录跨环境迁移 |
| [remote-debugging.md](guide/remote-debugging.md) | 权威 | 远程环境调试 |

## Provider `docs/provider/`

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [coverage.md](provider/coverage.md) | **生成** | 由 `scripts/audit/audit-provider-coverage.mjs` 生成，源为 `assets/contract/provider-coverage.json` |
| [regression-matrix.md](provider/regression-matrix.md) | 权威 | Code Agent 回归矩阵的说明与判读口径；矩阵数据本身以同目录 `regression-matrix.json` 为准 |
| [regression-matrix.json](provider/regression-matrix.json) | 数据 | 矩阵**真值来源**；`MATRIX_PATH` 默认值，被 `scripts/smoke/code-agent-matrix-summary.mjs`、`scripts/smoke/code-agent-regression.sh`、`scripts/static-checks.sh` 与 `scripts/release-readiness.sh` 消费 |
| [transform-coverage.md](provider/transform-coverage.md) | 权威 | 跨协议 transform 覆盖 |
| [claude-oauth.md](provider/claude-oauth.md) | 权威 | Claude OAuth 单账号反代 |
| [codex-oauth.md](provider/codex-oauth.md) | 权威 | Codex OAuth 单账号反代 |
| [grok-oauth.md](provider/grok-oauth.md) | 权威 | Grok OAuth 单账号反代 |
| [kimi-code.md](provider/kimi-code.md) | 权威 | Kimi Code 单账号反代 |
| [qoder-cosy.md](provider/qoder-cosy.md) | 权威 | Qoder COSY 单账号反代、站点/凭据 rail、动态模型 capability 与签名边界 |
| [codebuddy-oauth.md](provider/codebuddy-oauth.md) | 权威 | CodeBuddy OAuth 单账号反代实现合同：单 ProviderType + 国内/国际站点、固定身份 OAuth/refresh、三 Surface、严格终态与 `fixture_verified / live_pending` 边界 |
| [trae-solo.md](provider/trae-solo.md) | 权威 | Trae CN Solo Server-native 单账号 bridge 实现合同：固定 endpoint、callback capability、三 Surface、严格终态、目录/额度与真实验收门禁 |
| [deepseek-web.md](provider/deepseek-web.md) | 权威 | DeepSeek Web bearer、session/PoW、严格终态与单账号恢复边界 |
| [api-key-coding-plans.md](provider/api-key-coding-plans.md) | 权威 | 20 个 region × Surface typed API Key Plan、外部证据漂移门禁与 Ollama Cloud |
| [web-session.md](provider/web-session.md) | 权威 | Grok/Perplexity Web Session 隐藏 typed Provider、独立 Cookie rail、严格终态、作用域隔离与 live-pending 门禁 |
| [cursor.md](provider/cursor.md) | 权威 | Cursor AgentService 验收 |
| [code-plan-delta-2026-08-23.md](provider/code-plan-delta-2026-08-23.md) | 权威 | 2026-08-23 Code Plan 协议增量、证据边界与 live 缺口 |
| [code-plan-enhancement-2026-08-24.md](provider/code-plan-enhancement-2026-08-24.md) | 权威 | 七类低于 9 分 Code Plan 的参考、实施、拒绝项、不变量证明与 live gates |
| [code-plan-implementation-2026-08-30.md](provider/code-plan-implementation-2026-08-30.md) | 权威 | 低分 Code Plan 的 15 个循环实施真值；Loop 13/14 为 CodeBuddy CN/Intl 与 Trae CN Solo，Loop 15 为 Qoder Global/CN wire 分项追平，包含代码落点、专项验收和整体 review gate |

## Share `docs/share/`

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [access-policy.md](share/access-policy.md) | 权威 | Share 访问模型（Contract v2）与一次性持久化迁移 |
| [user-usage-rebase.md](share/user-usage-rebase.md) | 权威 | Share 用户用量重基线 |

## 验收 `docs/acceptance/`

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [real-acceptance-runbook.md](acceptance/real-acceptance-runbook.md) | 权威 | 真实验收 runbook |
| [manual-ui-checklist.md](acceptance/manual-ui-checklist.md) | 权威 | UI 人工验收清单 |
| [router-share-acceptance.md](acceptance/router-share-acceptance.md) | 权威 | Router / Share 联调验收 |

## 决策记录 `docs/adr/`

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [0001-web-terminal-ops-shell.md](adr/0001-web-terminal-ops-shell.md) | 权威 | Web 终端运维 shell |

## 历史 `docs/history/`

以下文档**全部为归档件**，每份开头带只读横幅。它们记录了迁移与整改的过程与决策，**不代表当前实现**，不得据以判断目录结构、行号、测试数量、能力状态或产品边界。

| 文档 | 说明 |
| --- | --- |
| [architecture-refactor-plan.md](history/architecture-refactor-plan.md) | 架构重构计划 |
| [code-audit-gap-plan.md](history/code-audit-gap-plan.md) | 代码审计缺口计划 |
| [code-plan-enhancement-plan.md](history/code-plan-enhancement-plan.md) | 代码计划增强 |
| [system-audit-and-normalization-plan.md](history/system-audit-and-normalization-plan.md) | 三方系统审计与规范化 |
| [market-replacement-sub2api-plan.md](history/market-replacement-sub2api-plan.md) | 独立 Market 候选实现评估 |
| [token-market-decoupling-plan.md](history/token-market-decoupling-plan.md) | 旧 Token Market 解耦实施计划 |
| [token-market-decoupling-review.md](history/token-market-decoupling-review.md) | 旧 Token Market 解耦 Review |
| [server-pre-fix.md](history/server-pre-fix.md) | Server 前置修复记录 |

## 其他

| 路径 | 状态 | 说明 |
| --- | --- | --- |
| [provider-fixtures/](provider-fixtures/) | 数据 | Provider 固定装置目录，由 `scripts/release-readiness.sh` 的 secret audit 遍历 |
| `remaining-work-index.md` | — | **本地-only 工作索引，已 gitignore，不提交**；仓库中不存在该文件属正常 |

## 仓库根目录文档

| 文档 | 状态 | 说明 |
| --- | --- | --- |
| [../README.md](../README.md) | 权威 | 产品定位、能力矩阵、快速入口 |
| [../AGENTS.md](../AGENTS.md) | 权威 | 开发约定：产品边界、依赖方向、状态写入、UI 独立性、验证清单 |
| [../PROTOCOL_EVIDENCE.md](../PROTOCOL_EVIDENCE.md) | 权威 | Provider 合同权威与外部协议证据边界 |
| [../THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md) | 权威 | 第三方代码、资产许可证与归属 |

## 外部真值来源

| 主题 | 位置 |
| --- | --- |
| Router 内部架构与术语表 | `cc-switch-router/ARCHITECTURE.md`（§0 术语表为词汇标准） |
| Router ↔ Server 协议线格式 | `cc-switch-router/PROTOCOL.md`（11 节） |
| Router UI 测试计划 | `cc-switch-router/UI_TEST_PLAN.md` |
| 系统级文档站 | `tokenswitch-docsify` → https://docs.tokenswitch.org |
| Provider 产品范围与身份 | `assets/contract/server-provider-requirements.json` 与 `assets/contract/provider-registry.json` |

## 旧路径映射

2026-08-20 的文档重组把散落在根目录与 `docs/` 平铺层的文件收进分类子目录。外部引用旧路径时按下表更新：

| 旧路径 | 新路径 |
| --- | --- |
| `docs/usage-token-accounting.md` | `docs/architecture/usage-accounting.md` |
| `docs/deployment.md` | `docs/guide/deployment.md` |
| `docs/server-data-migration.md` | `docs/guide/data-migration.md` |
| `docs/remote-debugging.md` | `docs/guide/remote-debugging.md` |
| `docs/provider-coverage.md` | `docs/provider/coverage.md` |
| `docs/code-agent-regression-matrix.md` | `docs/provider/regression-matrix.md` |
| `docs/code-agent-regression-matrix.json` | `docs/provider/regression-matrix.json` |
| `docs/transform-coverage.md` | `docs/provider/transform-coverage.md` |
| `docs/claude-oauth.md` | `docs/provider/claude-oauth.md` |
| `docs/codex-oauth.md` | `docs/provider/codex-oauth.md` |
| `docs/grok-oauth.md` | `docs/provider/grok-oauth.md` |
| `docs/kimi-code.md` | `docs/provider/kimi-code.md` |
| `docs/cursor-agentservice-acceptance.md` | `docs/provider/cursor.md` |
| `docs/share-access-policy.md` | `docs/share/access-policy.md` |
| `docs/share-user-usage-rebase.md` | `docs/share/user-usage-rebase.md` |
| `docs/real-acceptance-runbook.md` | `docs/acceptance/real-acceptance-runbook.md` |
| `docs/manual-ui-checklist.md` | `docs/acceptance/manual-ui-checklist.md` |
| `docs/router-market-acceptance.md` | `docs/acceptance/router-share-acceptance.md` |
| `docs/web-terminal-ops-shell-adr.md` | `docs/adr/0001-web-terminal-ops-shell.md` |
| `docs/architecture-refactor-plan.md` | `docs/history/architecture-refactor-plan.md` |
| `docs/code-audit-gap-plan.md` | `docs/history/code-audit-gap-plan.md` |
| `docs/code-plan-enhancement-plan.md` | `docs/history/code-plan-enhancement-plan.md` |
| `docs/system-audit-and-normalization-plan.md` | `docs/history/system-audit-and-normalization-plan.md` |
| `docs/market-replacement-sub2api-plan.md` | `docs/history/market-replacement-sub2api-plan.md` |
| `docs/token-market-decoupling-plan.md` | `docs/history/token-market-decoupling-plan.md` |
| `docs/token-market-decoupling-review.md` | `docs/history/token-market-decoupling-review.md` |
| `server-pre-fix.md`（仓库根） | `docs/history/server-pre-fix.md` |

## 已下线主题

以下内容**不得**再出现在任何权威文档中，只允许在 `docs/history/` 中作为历史记录：

- 独立 Token Market 服务（`cc-switch-market`）；Router 上 `/v1/markets*`、`/v1/market/*`、`/_market/proxy/*` 返回 `410 Gone`
- 独立 `cc-switch-share-market` 仓库（能力已并入 Router 内建 Share Market）
- Tauri 桌面端作为 Router 客户端
- 账本抽成（10% + 5%）与 Gate.io 提现；现行结算为 USD 赊账账户 + 线下付款声明 + 供应商确认 + 12h 健康时长试用
- Share Contract v1 字段：`acl`、`forSale`/`for_sale`、`officialPricePercent`、`forSaleOfficialPricePercentByApp`、`sharedWithEmails`、`marketAccessMode`、`accessByApp`、`appSettings`
