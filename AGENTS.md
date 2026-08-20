# cc-switch-server 开发约定

## 产品方向

本仓库是独立 server 产品，聚焦 code agent 订阅反代 / token server 能力。

不要把 upstream desktop cc-switch 整仓复制进来，也不要长期 merge upstream main。

## 必须覆盖

当前 cc-switch 中 Claude、Codex、Gemini 三类 app 已支持的所有供应商类型，都必须进入 server 覆盖范围。

供应商覆盖以 Provider 基线仓库（`CC_SWITCH_PROVIDER_AUDIT_ROOT`，默认同级 `../cc-switch`）中以下五个权威来源为准，快照见 `assets/contract/upstream-provider-source-baseline.json`：

- `src-tauri/src/proxy/providers/mod.rs`
- `src/config/claudeProviderPresets.ts`
- `src/config/codexProviderPresets.ts`
- `src/config/geminiProviderPresets.ts`
- `src/config/universalProviderPresets.ts`

## 禁止默认迁移

除非明确证明服务于 Claude/Codex/Gemini 反代主线，否则不要迁移：

- Tauri window/tray/updater/deeplink。
- Claude Desktop profile 写入和桌面 UI。
- MCP、skills、session manager。
- release notes、桌面安装资产、截图资产。

## 外部 Provider 审计

外部仓库改动只作为 Provider 类型和协议行为证据，不作为实现同步源。根据证据调整 Server 前，必须更新或核对：

- `UPSTREAM_IMPORT.md`
- `docs/provider/coverage.md`

## 状态写入

新代码禁止在 `state.rs` 之外直接对 `ServerStateInner` 的存储字段 `.write().await` 后修改数据；必须通过 `ServerStateInner` 的域方法封装读改写和持久化策略。跨存储写操作按字段声明顺序获取锁：config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins。

shares 写路径已收敛到 `mutate_shares_immediate` / `try_mutate_shares_immediate` / `mutate_shares_debounced` / `mutate_share` / `replace_shares` / `validate_share_invocation`，调用方不得再直接感知 shares 的立即保存或 debounce 落盘细节。

## 依赖方向

`domain` 不能依赖 `api`、`clients`、`proxy`；`proxy` 不能依赖 `api/http` 或 `clients`。转发热路径需要触发出站 OAuth/router 客户端时，必须通过 `state.rs` 或控制面编排方法封装状态读写、锁和持久化策略。

## UI 独立性

Server Web UI 以本仓库的产品需求、Server API 和 `assets/contract/web-runtime-contract.json` 为唯一实现依据，人工验收见 `docs/acceptance/manual-ui-checklist.md`。

禁止从 cc-switch 或其他外部项目批量复制、同步或覆盖 React 组件、样式、locale、运行时命令和页面结构。外部项目只能作为 Provider 类型、协议行为或缺陷修复的审计证据；吸收时必须按 Server 边界重新设计、逐项实现并独立 review。

本地-only 工作索引（已 gitignore，不提交）：`docs/remaining-work-index.md`。

## 文档

`docs/README.md` 是全部文档的唯一索引。新增或移动 `docs/` 下的文档时必须同步：

- 在 `docs/README.md` 中登记，并标注状态（权威 / 生成 / 历史 / 数据）。
- 归档件放进 `docs/history/`，开头必须带只读横幅（`归档文档 · 只读 · 不代表当前实现`）。
- 生成类文档不得手工编辑，改动源在生成脚本或 `assets/contract/`。
- 移动文件后更新引用它的脚本、`.env.example` 和其他文档；`node scripts/audit/audit-docs-index.mjs`（已并入 `scripts/static-checks.sh`）会校验链接可解析、索引完整和归档横幅存在。

架构叙述的真值来源是 `docs/architecture/overview.md`；协议线格式以 `cc-switch-router/PROTOCOL.md` 为准；系统级文档站在 `tokenswitch-docsify`。

## 管理员密码修改

「设置 - 密码修改」**设计如此：不用输入旧密码，直接修改**。UI 固定为一行 —— 左侧标题「密码修改 / 修改管理员登录密码」，右侧只有「新密码」输入框 + 「保存」按钮。

- 前端只走 `POST /web-api/auth/password/set`（`web-src/src/lib/server-legacy-api.ts` 的 `changeServerPassword`），请求体只有 `{ newPassword }`。
- 授权凭据是**当前这个管理员会话本身**：后端 `web_password_set` 先 `require_web_admin_session`，再 `set_admin_password`，其中会 `state.clear_sessions()`；前端随即清 token 并派发 `SERVER_AUTH_EXPIRED_EVENT`，强制用新密码重新登录。这层「改完必须重新登录」不能去掉。
- 不要改用 `POST /web-api/auth/password/change`。那条路径要求 `currentPassword`，而邮箱验证码 / API Token / Router SSO 登录时前端根本没有明文密码可填，会让这些登录方式无法改密码。`/change` 端点本身保留（供其他调用方和契约测试使用），只是设置页不用它。
- 因此**禁止**在设置页加回「当前密码」「确认新密码」输入框，或把布局改成多列表单。历史教训：`e9dc404` 因 Router 白名单缺失把 UI 改成调 `/change`（Router 侧真正的修复 `ddc49c5` 在一秒后就合入了），`2dbef0d` 进一步把「当前密码」做成真实输入框并改成三列布局——两次都是误改，已回退。
- Router 侧 `is_allowed_client_web_path()` 放行整个 `/web-api/` 前缀，`/set` 经隧道可达，不需要为它单独加白名单条目。

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

- `server.json`：password hash、owner email、router、client tunnel subdomain、`requestBodyLimits`（本地请求体上限，生效值为 `min(本地, Router 声明)`）。
- `providers.json`：Claude/Codex/Gemini provider 配置和分类后的 ProviderType。

不要把这些文件的存在误判为最终 DB 迁移完成；SQLite 兼容和旧 cc-switch DB 读取必须另行设计和验收。
