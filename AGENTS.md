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

## 外部 Provider 审计

外部仓库改动只作为 Provider 类型和协议行为证据，不作为实现同步源。根据证据调整 Server 前，必须更新或核对：

- `UPSTREAM_IMPORT.md`
- `docs/provider-coverage.md`

## 状态写入

新代码禁止在 `state.rs` 之外直接对 `ServerStateInner` 的存储字段 `.write().await` 后修改数据；必须通过 `ServerStateInner` 的域方法封装读改写和持久化策略。跨存储写操作按字段声明顺序获取锁：config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins。

shares 写路径已收敛到 `mutate_shares_immediate` / `try_mutate_shares_immediate` / `mutate_shares_debounced` / `mutate_share` / `replace_shares` / `validate_share_invocation`，调用方不得再直接感知 shares 的立即保存或 debounce 落盘细节。

## 依赖方向

`domain` 不能依赖 `api`、`clients`、`proxy`；`proxy` 不能依赖 `api/http` 或 `clients`。转发热路径需要触发出站 OAuth/router 客户端时，必须通过 `state.rs` 或控制面编排方法封装状态读写、锁和持久化策略。

## UI 独立性

Server Web UI 以本仓库的产品需求、Server API 和 `assets/contract/web-runtime-contract.json` 为唯一实现依据，人工验收见 `docs/manual-ui-checklist.md`。

禁止从 cc-switch 或其他外部项目批量复制、同步或覆盖 React 组件、样式、locale、运行时命令和页面结构。外部项目只能作为 Provider 类型、协议行为或缺陷修复的审计证据；吸收时必须按 Server 边界重新设计、逐项实现并独立 review。

本地-only 工作索引（已 gitignore，不提交）：`docs/remaining-work-index.md`。

## 验证

完成代码改动后优先运行：

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- `node scripts/audit/audit-provider-coverage.mjs --check`
- `node scripts/audit/audit-ui-provider-matrix.mjs --check`
- `scripts/smoke/smoke-local.sh`
- `RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh`

真实 router/market/OAuth/share-market grant 输入齐备前，只能运行本地验证和离线 readiness；不得把缺真实输入的项目标记为真实通过。

当前可用的 server-native 持久化文件：

- `server.json`：password hash、owner email、router、client tunnel subdomain。
- `providers.json`：Claude/Codex/Gemini provider 配置和分类后的 ProviderType。

不要把这些文件的存在误判为最终 DB 迁移完成；SQLite 兼容和旧 cc-switch DB 读取必须另行设计和验收。
