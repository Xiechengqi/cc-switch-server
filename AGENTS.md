# cc-switch-server 开发约定

## 产品方向

本仓库是独立 server 产品，聚焦 code agent 订阅反代 / token server 能力。

不要把 upstream desktop cc-switch 整仓复制进来，也不要长期 merge upstream main。

## 必须覆盖

当前 cc-switch 中 Claude、Codex、Gemini 三类 app 已支持的所有供应商类型，都必须进入 server 覆盖范围。

供应商覆盖以这些来源为准：

- `/data/projects/cc-switch/src-tauri/src/proxy/providers/mod.rs`
- `/data/projects/cc-switch/src/config/claudeProviderPresets.ts`
- `/data/projects/cc-switch/src/config/codexProviderPresets.ts`
- `/data/projects/cc-switch/src/config/geminiProviderPresets.ts`
- `/data/projects/cc-switch/src/config/universalProviderPresets.ts`

## 禁止默认迁移

除非明确证明服务于 Claude/Codex/Gemini 反代主线，否则不要迁移：

- Tauri window/tray/updater/deeplink。
- Claude Desktop profile 写入和桌面 UI。
- MCP、skills、session manager。
- release notes、桌面安装资产、截图资产。

## 上游吸收

上游改动只做选择性吸收。每次吸收前必须更新或核对：

- `UPSTREAM_IMPORT.md`
- `docs/provider-coverage.md`

## UI 对齐

Server Web UI 必须以 desktop 组件同源移植为准。正式实施计划见 `docs/server-desktop-ui-parity-plan.md`；人工验收见 `docs/manual-ui-checklist.md`。功能边界以 `docs/web-runtime-contract.json` 为准。

本地-only 重构笔记（已 gitignore，不提交）：`UI_PARITY_PLAN.md`、`DESKTOP_ALIGNMENT_TASKS.md`、`SERVER_IMPLEMENTATION_PLAN.md`、`MIGRATION_LEDGER.md`、`docs/remaining-work-index.md`。

同步 desktop 组件：

```bash
node scripts/sync-desktop-ui.mjs
```

## 验证

完成代码改动后优先运行：

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- `node scripts/audit-provider-coverage.mjs --check`
- `node scripts/audit-ui-provider-matrix.mjs --check`
- `scripts/smoke-local.sh`
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh`

真实 router/market/OAuth/share-market grant 输入齐备前，只能运行本地验证和离线 readiness；不得把缺真实输入的项目标记为真实通过。

当前可用的 server-native 持久化文件：

- `server.json`：password hash、owner email、router、client tunnel subdomain。
- `providers.json`：Claude/Codex/Gemini provider 配置和分类后的 ProviderType。

不要把这些文件的存在误判为最终 DB 迁移完成；SQLite 兼容和旧 cc-switch DB 读取必须另行设计和验收。
