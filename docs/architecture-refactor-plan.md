# 架构优化重构计划（Phase R）

> **来源**：2026-07-06 技术架构评审（基于同日三方代码交叉审计，见 `docs/code-audit-gap-plan.md` 摘要表；本文档所有行数/引用数均为当日 HEAD `6fce8e4` 实测值）。
> **性质**：纯结构重构——**不改变任何运行时行为、不新增功能、不修 bug**。任何与功能相关的改动一律走 `docs/code-audit-gap-plan.md`（Phase X）。
> **执行者提示**：本文档面向可以直接开工的实施者（人或 code agent）。每个任务给出前置条件、精确操作、验收命令与提交规范；`cargo check` 的编译错误是重构过程中的**导航工具**而非事故，按提示逐个修复即可。

---

## 一、诊断：当前结构的问题清单（全部实测）

### 1.1 `src/http.rs` 单文件控制面（11342 行）——最大病灶

- 204 个 `async fn`：121 条 REST 路由的 handler、227 命令的 `/web-api/invoke` dispatcher（44 个 `web_*` handler，`web_invoke_dispatch` 位于 4504 行起）、路由表、`ApiError`（文件内 496 处引用）、`require_session`（96 处引用）。
- 文件尾部 `mod tests`（9932 行起，25 个测试，~1400 行）。
- 后果：所有 API 改动都发生在同一文件，检索/评审/合并成本高；对 router 的对外契约端点（`/_ctl/*`、`/_share-router/*`，有 HMAC+nonce 纪律）与普通内部 REST 混在一起，契约变更无法在 review 中一眼识别。

### 1.2 `src/state.rs` 的 `ServerStateInner` God Object（1499 行文件，24 个全 `pub` 字段）

- 11 个 `RwLock` 存储 + 锁/缓存/隧道/事件总线全部 `pub`，200+ 调用点直接 `.write().await` 任意存储。
- 两条纪律靠人肉维持：**锁顺序**（跨存储写操作的获取顺序无封装，写错即潜在死锁）；**持久化策略**（立即原子写 vs debounce 落盘的选择散落在 `http.rs` 各 handler，如 `save_shares_debounced(&state)` 漏写一处即掉数据风险）。

### 1.3 `src/core/` 29 个模块平铺，三种性质的代码未分离

| 性质 | 模块（实测出站 I/O 依据） | 问题 |
| --- | --- | --- |
| 纯领域（类型/存储/规则，无出站 I/O） | `provider.rs`（类型）、`providers.rs`（存储）、`provider_matrix.rs`、`universal_providers.rs`、`accounts.rs`、`account_managers.rs`、`oauth_login.rs`（内存会话）、`shares.rs`、`model_health.rs`、`usage.rs`、`pricing.rs`、`config.rs`、`ui_settings.rs`、`config_transfer.rs`、`failover.rs`、`health.rs`、`stream_check.rs`、`web_auth.rs` | 与出站客户端混排，依赖方向不可见 |
| 出站 I/O 客户端（含 `reqwest` 调用） | `router_client.rs`、`tunnel.rs`（SSH）、`oauth_clients.rs`、`quota.rs`（`use reqwest::...`，后台刷新真实调上游）、`account_refresh.rs`（`http: &reqwest::Client`）、`email_auth.rs`（签名调 router）、`copilot_device.rs`、`kiro_device.rs` | 测试策略/变更原因与领域层完全不同 |
| 基础设施 | `storage.rs`（原子写）、`backup.rs`、`live_config_import.rs` | 同上 |
| 命名混淆 | 单数 `provider.rs`=类型、复数 `providers.rs`=存储，仅靠约定区分；`core/usage.rs` 与 `proxy/usage.rs` 同名 | 可读性 |

### 1.4 bin-only crate（无 `lib.rs`）

- 无法建 `tests/` 集成测试目录（HTTP 契约测试只能塞在 `http.rs` 尾部）。
- 任何模块可 `crate::` 任意互访，分层只是口头约定（例如没有任何机制阻止 `proxy` 反向依赖控制面）。
- 59k 行任何改动全量重编 binary。

### 1.5 外围问题

- `docs/web-runtime-contract.json`、`docs/provider-coverage.json`、`docs/provider-fixtures/structures.json` 被 `include_str!` 编进二进制（`src/web_runtime.rs:55`、`src/coverage.rs:68/163`）——运行时资产放在文档目录，存在被"整理文档"误动的风险。
- `proxy/cursor/` 内文件带冗余前缀（`cursor/cursor_agent_proto.rs` 等 12 个文件）。
- `scripts/` 20+ 脚本平铺。
- `web-src/` 中"desktop 同源文件"与"server-local 文件"的边界只存在于 `scripts/sync/sync-desktop-ui.mjs` 的隐式路径清单中；同源文件被本地改动不会被发现（单向检查）。

### 1.6 结构良好、明确不动的部分

- `src/proxy/` 整体成型；`adapters.rs`（5540 行）与 `transforms.rs`（3291 行）大但内聚，且是 Phase X（X2/X5/X6/X7）主战场，**本计划禁止拆分或移动其内容**（仅 R5 的 cursor 文件重命名例外）。
- `web-src/` 前端**冻结**：必须镜像 desktop 组件路径（同源移植 + 漂移检查依赖路径对应），禁止任何目录重组。

---

## 二、目标架构

### 2.1 组织原则

按运行时三平面组织，让边界在编译期可见：**数据面**（`proxy/`，热路径转发）、**控制面**（`api/`，REST + invoke + router 契约端点）、**领域与外设**（`domain/` 纯领域、`clients/` 出站 I/O、`infra/` 基础设施）。

**依赖方向规则（R1 起由 CI 门禁强制）**：

```
api → domain / clients / proxy(装配与只读)
proxy → domain（禁止 → api）
clients → domain
domain → infra（禁止 → api / clients / proxy）
```

### 2.2 目标目录树（Phase 1 完成态）

```
src/
  main.rs                  # 薄入口（≤60 行：parse CLI → 调 lib）
  lib.rs                   # crate 根，声明全部顶层模块
  cli.rs  admin.rs  build_info.rs
  state.rs                 # 过渡期保留，随 R4 逐步瘦身

  domain/
    mod.rs
    providers/  {mod.rs, model.rs, store.rs, matrix.rs, universal.rs, live_import.rs}
    accounts/   {mod.rs, store.rs, managers.rs, login.rs, oauth.rs}
    sharing/    {mod.rs, shares.rs, model_health.rs, router_contract.rs}
    usage/      {mod.rs, store.rs, pricing.rs}
    settings/   {mod.rs, config.rs, ui_settings.rs, transfer.rs}
    failover.rs  health.rs  stream_check.rs  web_auth.rs

  clients/
    mod.rs
    router/     {mod.rs, client.rs, tunnel.rs, email_auth.rs}
    oauth/      {mod.rs, quota.rs, refresh.rs, copilot_device.rs, kiro_device.rs}

  infra/
    mod.rs  storage.rs  backup.rs  time.rs

  api/
    mod.rs                 # 路由表装配 + serve()
    error.rs               # ApiError 及全部 impl
    session.rs             # require_session / 会话辅助
    providers.rs  accounts.rs  shares.rs  usage.rs
    router.rs  settings.rs  backup.rs  events.rs
    control/    {mod.rs, share_router.rs, ctl.rs}   # /_share-router/* 与 /_ctl/*
    invoke/     {mod.rs, dispatch.rs, handlers.rs}  # /web-api/invoke
    web/        {mod.rs, runtime.rs, assets.rs, coverage.rs}

  proxy/                   # 不动（仅 R5 重命名 cursor/ 内文件）

assets/contract/           # R5：include_str! 运行时资产从 docs/ 迁入
tests/                     # R3 后可选：HTTP 契约级集成测试
```

---

## 三、分阶段实施任务

> **全局纪律（每个任务适用）**
> 1. 每个 R 任务独立成组提交，**禁止与任何功能改动混在同一提交**；提交信息格式 `refactor(structure): R<n> <内容>`。
> 2. 每步完成必须通过第六节验证基线**全部命令**后才允许开始下一步。
> 3. 移动文件用 `git mv` 保持历史。
> 4. 除本文档明确列出的重命名/移动外，**不修改任何函数体逻辑**；发现 bug 记录到 `docs/code-audit-gap-plan.md`，不顺手修。
> 5. `cargo check --all-targets` 直接取退出码，禁止管道吞码（`cargo check | tail` 会掩盖非零退出）。

### R1 lib/bin 拆分 + 依赖方向门禁

**前置**：工作树干净。
**工作量**：约半天。

操作：

1. 新建 `src/lib.rs`，内容为 `src/main.rs` 现有的 10 行 `mod` 声明改为 `pub mod`：
   ```rust
   pub mod admin;
   pub mod build_info;
   pub mod cli;
   pub mod core;      // R2 中将被 domain/clients/infra 取代
   pub mod coverage;
   pub mod http;
   pub mod proxy;
   pub mod state;
   pub mod web_assets;
   pub mod web_runtime;
   ```
2. `src/main.rs` 删除全部 `mod` 声明，改为 `use cc_switch_server::{admin, build_info, cli, http, state};`（crate 名 `cc-switch-server` → lib 路径 `cc_switch_server`）。`main.rs` 只保留 `main()`、`serve()`、`init_tracing()`、`print_version()` 四个函数（当前共 58 行，结构不变）。
3. 编译错误处理：原 `pub(crate)` 项对 bin 不可见时，将 main.rs 用到的少量项（`ServerStateInner::load`、`state::restore_tunnels` 等 5 个）提升为 `pub`；不要全局放宽可见性。
4. `Cargo.toml` 无需显式 `[lib]`/`[[bin]]`（默认约定即可）；如 `cargo test` 出现测试目标歧义再补显式声明。
5. 在 `scripts/static-checks.sh` 末尾追加依赖方向门禁（R2/R3 完成前对尚不存在的目录自动跳过）：
   ```bash
   echo "== dependency direction =="
   ! rg -n 'use crate::(http|api)\b' src/proxy || { echo 'proxy must not depend on api/http'; exit 1; }
   ! rg -n 'use crate::clients|crate::clients::' src/proxy || { echo 'proxy must not depend on clients'; exit 1; }
   if [ -d src/domain ]; then
     ! rg -n 'use crate::(api|http|clients|proxy)\b' src/domain || { echo 'domain must stay pure'; exit 1; }
   fi
   ```

验收：`cargo check --all-targets` 与 `cargo test`（基线 503 passed）退出码 0；`scripts/static-checks.sh` 通过；`target/debug/cc-switch-server --help` 正常输出。

### R2 `core/` 拆为 `domain/` + `clients/` + `infra/`

**前置**：R1 完成。
**工作量**：约半天。全仓对 core 模块的路径引用约 250 行（实测 top：`provider` 70、`usage` 32、`router_client` 29、`accounts` 29、`email_auth` 24、`providers` 22、`storage` 18、`shares` 17、`tunnel` 16、`failover` 15、`backup` 14、`config` 13）。

**完整文件映射表**（29 个文件，逐条执行 `git mv`）：

| 现路径（src/core/） | 新路径（src/） | 新模块路径 |
| --- | --- | --- |
| provider.rs | domain/providers/model.rs | `crate::domain::providers::model` |
| providers.rs | domain/providers/store.rs | `crate::domain::providers::store` |
| provider_matrix.rs | domain/providers/matrix.rs | `crate::domain::providers::matrix` |
| universal_providers.rs | domain/providers/universal.rs | `crate::domain::providers::universal` |
| accounts.rs | domain/accounts/store.rs | `crate::domain::accounts::store` |
| account_managers.rs | domain/accounts/managers.rs | `crate::domain::accounts::managers` |
| oauth_login.rs | domain/accounts/login.rs | `crate::domain::accounts::login` |
| shares.rs | domain/sharing/shares.rs | `crate::domain::sharing::shares` |
| model_health.rs | domain/sharing/model_health.rs | `crate::domain::sharing::model_health` |
| usage.rs | domain/usage/store.rs | `crate::domain::usage::store` |
| pricing.rs | domain/usage/pricing.rs | `crate::domain::usage::pricing` |
| config.rs | domain/settings/config.rs | `crate::domain::settings::config` |
| ui_settings.rs | domain/settings/ui_settings.rs | `crate::domain::settings::ui_settings` |
| config_transfer.rs | domain/settings/transfer.rs | `crate::domain::settings::transfer` |
| failover.rs | domain/failover.rs | `crate::domain::failover` |
| health.rs | domain/health.rs | `crate::domain::health` |
| stream_check.rs | domain/stream_check.rs | `crate::domain::stream_check` |
| web_auth.rs | domain/web_auth.rs | `crate::domain::web_auth` |
| router_client.rs | clients/router/client.rs | `crate::clients::router::client` |
| tunnel.rs | clients/router/tunnel.rs | `crate::clients::router::tunnel` |
| email_auth.rs | clients/router/email_auth.rs | `crate::clients::router::email_auth` |
| oauth_clients.rs | domain/accounts/oauth.rs | `crate::domain::accounts::oauth` |
| quota.rs | clients/oauth/quota.rs | `crate::clients::oauth::quota` |
| account_refresh.rs | clients/oauth/refresh.rs | `crate::clients::oauth::refresh` |
| copilot_device.rs | clients/oauth/copilot_device.rs | `crate::clients::oauth::copilot_device` |
| kiro_device.rs | clients/oauth/kiro_device.rs | `crate::clients::oauth::kiro_device` |
| storage.rs | infra/storage.rs | `crate::infra::storage` |
| backup.rs | infra/backup.rs | `crate::infra::backup` |
| live_config_import.rs | infra/live_config_import.rs | `crate::infra::live_config_import` |

操作：

1. 按表 `git mv`；删除 `src/core/mod.rs`；新建 `domain/mod.rs`、`domain/{providers,accounts,sharing,usage,settings}/mod.rs`、`clients/mod.rs`、`clients/{router,oauth}/mod.rs`、`infra/mod.rs`，只做 `pub mod` 声明（**不做旧名 re-export 兼容层**——本仓库无外部消费者，一次改净）。
2. `lib.rs` 中 `pub mod core;` 替换为 `pub mod clients; pub mod domain; pub mod infra;`。
3. 批量替换旧路径（顺序执行，**长模式在前**防止误替换；`rg -l` 定位后用 sed 或编辑器全仓替换）：
   ```
   crate::core::provider_matrix   → crate::domain::providers::matrix
   crate::core::providers         → crate::domain::providers::store
   crate::core::provider          → crate::domain::providers::model
   crate::core::universal_providers → crate::domain::providers::universal
   crate::core::account_managers  → crate::domain::accounts::managers
   crate::core::account_refresh   → crate::clients::oauth::refresh
   crate::core::accounts          → crate::domain::accounts::store
   crate::core::oauth_login       → crate::domain::accounts::login
   crate::core::oauth_clients     → crate::domain::accounts::oauth
   crate::core::copilot_device    → crate::clients::oauth::copilot_device
   crate::core::kiro_device       → crate::clients::oauth::kiro_device
   crate::core::quota             → crate::clients::oauth::quota
   crate::core::router_client     → crate::clients::router::client
   crate::core::tunnel            → crate::clients::router::tunnel
   crate::core::email_auth        → crate::clients::router::email_auth
   crate::core::model_health      → crate::domain::sharing::model_health
   crate::core::shares            → crate::domain::sharing::shares
   crate::core::usage             → crate::domain::usage::store
   crate::core::pricing           → crate::domain::usage::pricing
   crate::core::config_transfer   → crate::domain::settings::transfer
   crate::core::config            → crate::domain::settings::config
   crate::core::ui_settings       → crate::domain::settings::ui_settings
   crate::core::failover          → crate::domain::failover
   crate::core::health            → crate::domain::health
   crate::core::stream_check      → crate::domain::stream_check
   crate::core::web_auth          → crate::domain::web_auth
   crate::core::storage           → crate::infra::storage
   crate::core::backup            → crate::infra::backup
   crate::core::live_config_import → crate::infra::live_config_import
   ```
   同时处理无 `crate::` 前缀的 `use core::…`/`core::…` 变体（本仓库有少量此写法，逐个确认不是 Rust 内建 `core` crate 的引用后替换）。
4. 被移动文件内部的 `super::`/相对引用 sed 覆盖不到——以 `cargo check --all-targets` 输出为准逐个修复，直到零错误。这是本任务的主要人工环节，预计几十处。
5. 确认 `rg -n 'crate::core' src` 零命中后运行 `cargo fmt`。

验收：`rg 'crate::core|mod core' src` 零命中；`cargo check --all-targets`、`cargo test`（503 passed，数量不得减少）、`scripts/static-checks.sh`（含 R1 新门禁，此时 domain 目录检查生效）全部通过。

### R3 `http.rs` 拆分为 `api/` 模块族

**前置**：R2 完成。
**工作量**：1–1.5 天。**必须按以下顺序分 7 个提交推进**，每个提交独立通过验证基线。

| 提交 | 内容 | 要点 |
| --- | --- | --- |
| R3.1 | 建 `src/api/mod.rs`，将 `http.rs` 整体 `git mv` 为 `api/mod.rs`；`lib.rs` 的 `pub mod http` 改为 `pub mod api`；全仓 `crate::http` → `crate::api`（含 `main.rs` 的 `http::serve`）；`web_runtime.rs`/`web_assets.rs`/`coverage.rs` 同步 `git mv` 到 `api/web/{runtime.rs,assets.rs,coverage.rs}` 并更新引用与 `include_str!` 相对路径（`../docs/...` → `../../../docs/...`，R5 会再次调整，此处先保证编译） | 纯改名提交，行为零变化 |
| R3.2 | 从 `api/mod.rs` 抽出 `api/error.rs`（`ApiError` 类型 + 全部 impl + `web_invoke_unknown`/`web_invoke_not_wired` 等构造函数，原 9639/9652 行附近）与 `api/session.rs`（`require_session` 及会话辅助函数） | 两文件内所有项 `pub(crate)`；`api/mod.rs` 加 `pub(crate) use error::ApiError;` 减少后续 diff |
| R3.3 | 抽出 `api/control/`（`/_share-router/health\|request-logs\|share-runtime\|model-health` 与 `/_ctl/apply_share_settings\|refresh_share_usage` 的 handler、HMAC/nonce 校验辅助）与 `api/events.rs`（`/api/events` SSE） | router 对外契约物理隔离，文件头注释注明"变更需同步核对 cc-switch-router 调用方" |
| R3.4 | 抽出 `api/invoke/`：`dispatch.rs`（`web_invoke_compat` + `web_invoke_dispatch` 的 match，原 4372/4504 行起）、`handlers.rs`（全部 44 个 `web_*` handler 及其 `web_payload`/`web_arg_*` 参数辅助函数） | dispatcher match 分支只改路径前缀，禁止增删分支（`scripts/audit/audit-web-runtime-contract.mjs` 会双向校验 227 命令一致性） |
| R3.5 | 按域抽出 REST handler：`api/providers.rs`（15 个 provider handler + test/fetch-models）、`api/accounts.rs`（accounts/device flow/login）、`api/shares.rs`（share CRUD/binding/ACL/market/connect-info/tunnel）、`api/usage.rs`（usage/pricing/limits）、`api/router.rs`（register/heartbeat/status/batch-sync/client-tunnel/diagnostics/share-edits）、`api/settings.rs`（config/setup/auth/upstream-proxy）、`api/backup.rs` | 用 `grep -n 'async fn ' src/api/mod.rs` 生成清单，按路由表分组归属；跨域共享的小工具函数（如 `now_ms`、序列化辅助）集中到 `api/mod.rs` 或 `api/util.rs`，禁止复制多份 |
| R3.6 | 拆 `mod tests`（原 9932 行起 25 个测试）：测私有 fn 的测试跟随被测函数迁至对应子模块的 `#[cfg(test)] mod tests`；纯 HTTP 契约级测试（起 server 打请求断言响应的）迁至 `tests/api_contract.rs`（R1 的 lib 化已解锁该目录） | 测试总数不得减少（基线 503） |
| R3.7 | 收尾：`api/mod.rs` 只剩路由表装配 + `serve()` + 共享工具，目标 ≤800 行；`cargo fmt` 全仓 | — |

验收（最终态）：`src/api/mod.rs` ≤800 行；`wc -l src/api/*.rs` 无单文件超 2500 行；`cargo test` ≥503 passed；`node scripts/audit/audit-web-runtime-contract.mjs --check` 通过（invoke 契约无漂移）；验证基线全绿。

### R4 `ServerState` 收敛（渐进，规则先行）

**前置**：R3 完成。
**性质**：不设一次性截止，定规则 + 做首个样板，之后随功能 PR 摊销。

1. **规则（写入 `AGENTS.md` 开发约定）**：
   - 新代码禁止在 `state.rs` 之外直接 `.write().await` 存储字段；必须经 `ServerStateInner` 的域方法。
   - 每个域方法内聚"读改写 + 持久化策略"（立即原子写 or debounce），调用方不再感知 `save_*_debounced`。
   - 跨存储写操作的锁获取顺序统一为字段声明顺序（config → providers → universal_providers → accounts → failover → pricing → usage → shares → ui_settings → sessions → oauth_logins），方法内注释标注。
2. **首个样板（本计划内完成）**：选 `shares`（写路径最多、debounce 语义最复杂）——在 `state.rs` 增加 `impl ServerStateInner { pub async fn mutate_share(&self, id, f) -> …; pub async fn replace_shares(&self, …) }` 一组方法，将 `api/shares.rs` 与 `api/invoke/handlers.rs` 中对 `state.shares.write()` 的直接访问全部替换；`shares` 字段可见性降为 `pub(crate)`（proxy 侧只读路径仍需访问）。
3. 后续域（providers/accounts/usage…）按同模式在各自的功能 PR 中顺带收敛，每收敛一个域，把对应字段从 `pub` 降级。

验收（样板）：`rg 'state\.shares\.write' src/api` 零命中；`cargo test` 全绿。

### R5 运行时资产与外围整理

**前置**：R3 完成（R5.1 依赖 api/web 路径定型）。各子项独立提交，可并行。
**工作量**：合计约半天。

- **R5.1 运行时资产出 docs**：新建 `assets/contract/`，`git mv docs/web-runtime-contract.json docs/provider-coverage.json docs/provider-fixtures/structures.json` 至该目录（`provider-fixtures` 整目录评估：仅 `structures.json` 被 `include_str!`，其余 fixture 文档留 docs）；更新 `api/web/runtime.rs` 与 `api/web/coverage.rs` 的 `include_str!` 路径、`scripts/static-checks.sh` 的 json parse 清单、`scripts/audit/audit-web-runtime-contract.mjs` 等脚本内的硬编码路径（`rg 'web-runtime-contract|provider-coverage.json|structures.json' scripts docs src` 逐个核对）；docs 内原位置留一行指针说明。
- **R5.2 `proxy/cursor/` 去前缀**：`cursor/cursor_agent_proto.rs → cursor/agent_proto.rs` 等 12 个文件（`cursor_protocol.rs → protocol.rs`、`cursor_session.rs → session.rs`…），更新 `proxy/cursor/mod.rs` 与全部引用。纯重命名，禁止改动文件内容。
- **R5.3 `scripts/` 分组**：`scripts/{audit,smoke,sync}/` 三个子目录归类现有脚本；更新 `static-checks.sh`、`release-readiness.sh`、CI workflow、`AGENTS.md` 中的全部路径引用（`rg 'scripts/' .github scripts docs AGENTS.md README.md` 逐个核对）。
- **R5.4 web-src 同源边界显式化**：在 `scripts/sync/sync-desktop-ui.mjs` 中把 `defaultPaths` 导出为可查询的 manifest（或旁置 `web-src/synced-manifest.json`）；`--check` 增加反向检查——manifest 内文件与 desktop 源内容不一致时按"漂移"报出（本地改动同源文件即失败），server-local 适配文件（如 `SettingsPage.tsx` 等既有 6 个偏离文件）登记进显式豁免清单。

验收：R5.1 后 `cargo test` + `static-checks` 全绿且 `rg 'include_str!\("../docs' src` 零命中；R5.2 后 `cargo test` 全绿；R5.3 后 `scripts/static-checks.sh`（新路径）与 CI workflow 语法通过；R5.4 后 `node scripts/sync/sync-desktop-ui.mjs --check` 对干净树通过、对人为改动一个同源文件能报错（验证后还原）。

### R6 workspace 化（挂起项，不在本轮执行）

目标形态：`crates/{ccs-domain, ccs-clients, ccs-proxy, ccs-api, ccs-server(bin)}`，依赖方向由 Cargo 强制。R2/R3 的目录边界即未来 crate 边界，届时为目录级平移。

**触发条件（满足其一才启动，启动前在本文档登记决策）**：
1. 增量编译影响迭代（单行改动的 `cargo check` 超过团队体感阈值）；
2. 出现第二个交付物（如 proxy-only 部署形态）；
3. 多人并行开发需要按 crate 划 review/ownership 边界。

### R7 修复 infra 方向违例 + 门禁模式补全（2026-07-07 复核新增）

**背景**：R1–R5 实施后的独立复核（2026-07-07）发现两类残留问题，且互相掩护：
1. **`infra/` 反向依赖 `domain/`+`clients/`**——违反 2.1 节"infra 为最底层"的方向规则，共三处来源（全部实测）：
   - `infra/backup.rs:9` `use crate::domain::usage::store::now_ms`；
   - `infra/backup.rs:243-252` 备份目标清单硬编码 10 个 store 的 `*_path()` 全限定调用（7 个指向 `domain`、2 个指向 `clients`），另有测试内 3 处 `config_path`；
   - `infra/live_config_import.rs:7-8/299` 引用 `domain::providers` 的 `Provider`/`ProviderStore`。
2. **既有门禁模式有盲区**——三条门禁全部只匹配 `use crate::…` 语句，抓不到行内全限定路径（如 `crate::domain::…::foo()`），这正是 backup.rs 的 10 处违例至今隐身的原因；且缺少 infra 与 clients 两个方向的检查。已实测：升级为行内模式后 `proxy/`、`domain/`、`clients/` 三层均无存量命中，**只有 infra 需要先修**。

**工作量**：合计约半天。**执行顺序固定为 R7.1 → R7.2 → R7.3 → R7.4**（R7.4 的 infra 门禁必须在前三步清零后再上，否则立即红）。

#### R7.1 `now_ms` 下沉 `infra/time.rs`

- 新建 `src/infra/time.rs`，将 `domain/usage/store.rs:1562` 的 `pub fn now_ms() -> u128` 原样移入；`infra/mod.rs` 增加 `pub mod time;`。
- `domain/usage/store.rs` 删除原定义，改为 `use crate::infra::time::now_ms;`（domain → infra 是允许方向）——**不做 `pub use` 转发**（红线 4：一次改净）。
- 更新其余引用文件的 `use`：`infra/backup.rs`、`domain/failover.rs`、`domain/sharing/shares.rs`、`domain/sharing/router_contract.rs`、`clients/router/tunnel.rs`、`proxy/forwarder.rs`、`state.rs`（以 `rg 'store::now_ms|usage::store::now_ms' src` 实际命中为准，编译器兜底）。
- 验收：`rg 'now_ms' src/domain/usage/store.rs` 只剩 use 与调用点、无定义；`cargo test` 全绿。

#### R7.2 `live_config_import` 移入 domain

- `git mv src/infra/live_config_import.rs src/domain/providers/live_import.rs`；`infra/mod.rs` 删除声明、`domain/providers/mod.rs` 增加 `pub mod live_import;`。
- 归类依据：该模块读取 desktop 配置文件并产出 `Provider` 领域对象、读写 ui_settings 的 current-provider 键——是**领域导入服务**；其本地 fs 读取与各 store 的 `load_or_default` 同性质，不违反 domain 纯度（2.1 节的"纯"指无网络出站）。
- 更新引用（实测仅两处调用方）：`api/invoke/dispatch.rs`（4 处调用）、`api/invoke/handlers.rs`（1 处），`crate::infra::live_config_import` → `crate::domain::providers::live_import`。
- 验收：`cargo test` 全绿；`rg 'live_config_import' src` 零命中。

#### R7.3 backup 目标清单参数化（依赖倒置）

`infra/backup.rs` 不应知道"有哪些 store"，只应知道"如何快照/恢复一组文件"。

- `backup.rs` 中构造目标清单的函数（`store_paths_for_export` 及 `create_backup_inner` 内的同源清单，即 243-252 行的 10 个 `*_path()` 调用）改为**接受 `&[PathBuf]` 参数**；`pub fn create_backup(config_dir, targets: &[PathBuf], reason)`、`store_paths_for_export` 同步改签名或直接删除（由调用方持有清单）。
- 清单构造移到组合根 `state.rs`：新增 `pub fn backup_targets(config_dir: &Path) -> Vec<PathBuf>`，内容即原 10 行 `*_path()` 调用（state.rs 是装配层，允许依赖 domain/clients/infra 全部）。
- 更新调用方（实测：`state.rs` periodic backup、`api/backup.rs`、`api/invoke/dispatch.rs`）传入 `backup_targets(...)`。
- `restore_backup`/`list_backups`/`prune_backups` 按 manifest 内容工作，不涉及清单，签名不动。
- `backup.rs` 测试内 3 处 `crate::domain::settings::config::config_path(&dir)` 改为测试本地拼路径（`dir.join("server.json")`），不引用 domain。
- 验收：`rg 'crate::(api|clients|domain|proxy)' src/infra` **零命中**（R7 总验收条件）；`cargo test` 全绿；`tests/api_contract.rs` 中 backup 相关用例不回归。

#### R7.4 门禁模式升级 + 补全（最后执行）

将 `scripts/static-checks.sh` 的 `== dependency direction ==` 段整体替换为（模式从 `use crate::…` 升级为裸 `crate::…`，可捕获行内全限定路径；新增 infra/clients 两个方向）：

```bash
echo "== dependency direction =="
if rg -n 'crate::(http|api)\b' src/proxy; then
  echo 'proxy must not depend on api/http'; exit 1
fi
if rg -n 'crate::clients\b' src/proxy; then
  echo 'proxy must not depend on clients; route outbound client work through state/api orchestration'; exit 1
fi
if rg -n 'crate::(api|http|clients|proxy)\b' src/domain; then
  echo 'domain must stay pure'; exit 1
fi
if rg -n 'crate::(api|http|proxy)\b' src/clients; then
  echo 'clients must not depend on api/proxy'; exit 1
fi
if rg -n 'crate::(api|http|clients|domain|proxy)\b' src/infra; then
  echo 'infra must be the bottom layer'; exit 1
fi
```

说明：
- 原三条的 `[ -d src/domain ]` 存在性判断可删除（R2 已完成，目录恒存在）。
- clients 方向已实测干净，此条可随本步直接生效；infra 条依赖 R7.1–R7.3 先清零。
- 模式升级后若在其他层暴出新的行内违例（本次复核实测为零），按 2.1 节方向规则修复而不是放宽门禁。

- 验收：`scripts/static-checks.sh` 通过；临时在 `src/infra` 任一文件插入一行含 `crate::domain::` 的代码能触发失败（验证后还原）。注意升级后的模式对注释中的路径字样同样敏感（rg 不区分注释），属可接受的从严；如误报出现在注释里，改写注释措辞而不是放宽模式。

---

## 四、与 Phase X（`docs/code-audit-gap-plan.md`）的时序咬合

```
X1 → X3 → X2（P0 门禁修复，先行）
  → R1 → R2 → R3 → R4 样板 → R5        ← 本计划主体，一次性窗口完成
  → R7（infra 方向修复 + 门禁补全，半天）← 2026-07-07 复核新增，Phase R 关闭前的最后一步
  → X4 / X5 / X6 / X7 …                 ← 功能任务落在新结构上：
       X5/X6 的新模块直接进 clients/oauth/ 与 proxy/，不搬两次；
       X7 的测试落在拆分后的对应模块
```

**理由**：X5（Copilot token 交换）、X6（Kiro 桥）都要新增账号/凭据类模块且大改 proxy 周边；若先做功能再重排目录，全部要搬两次并重新过验证。R 计划总工作量 2.5–3.5 天，是纯机械、编译器全程校验的工作，收益前置。

## 五、禁止事项（红线）

1. 禁止改动 `proxy/adapters.rs`、`proxy/transforms.rs`、`proxy/forwarder.rs` 的任何函数体（R5.2 仅限 cursor 目录文件重命名）。
2. 禁止对 `web-src/` 做任何目录/路径调整（R5.4 只改 sync 脚本与 manifest）。
3. 禁止在重构提交中混入功能/行为变更；禁止"顺手修 bug"。
4. 禁止引入旧路径 re-export 兼容层长期共存（一次改净）。
5. 禁止降低测试数量（基线 `cargo test` 503 passed）或跳过任何验证基线命令。
6. 禁止使用 Playwright/Cypress 等 UI 自动化（沿用仓库既有纪律）。

## 六、验证基线（每个 R 任务/子提交完成前全部通过）

```bash
cargo fmt --check
cargo check --all-targets        # 直接取退出码
cargo clippy --all-targets -- -D warnings
cargo test                       # ≥503 passed，不得减少
npm --prefix web-src run typecheck
scripts/static-checks.sh         # 含 R1 新增依赖方向门禁
node scripts/audit/audit-web-runtime-contract.mjs --check
node scripts/sync/sync-desktop-ui.mjs --check   # 需 X3 先修复；未修复期间记录跳过原因
```

## 七、实施状态与剩余项（2026-07-07 关闭登记）

**Phase R 正式关闭**：R1–R7 已全部实施并经独立复核（提交 `65721b8`）。关闭时验证快照：`cargo test` 503 passed（484 lib + 19 集成）、typecheck 0、`static-checks.sh` 0（含 5 条行内模式方向门禁）、`sync-desktop-ui.mjs --check` 0、`rg 'crate::(api|clients|domain|proxy)' src/infra` 零命中、`rg 'crate::core' src` 零命中。

计划内仍开放的两项（均为设计上的渐进/挂起项，不阻塞关闭）：

| 项 | 状态与后续 |
| --- | --- |
| R4 存储收敛 | **已完成生产写路径收敛**。config/providers/universal_providers/accounts/failover/pricing/usage/shares/ui_settings/sessions/oauth_logins 均已通过 `ServerStateInner` 域方法封装写锁与持久化策略，对应字段降为 `pub(crate)`；集成测试改用 `config_snapshot()` / `usage_snapshot()` / `replace_config()`，不再直接访问锁字段。当前复核命令 `rg -n -U 'state\s*\.\s*(config\|providers\|universal_providers\|accounts\|failover\|pricing\|usage\|shares\|ui_settings\|sessions\|oauth_logins)\s*\.\s*write\s*\(\s*\)\s*\.\s*await' src/api src/proxy src/clients src/domain src/infra tests` 零命中；`scripts/static-checks.sh` 已升级为全受管 store 写锁与直接保存调用门禁。**X5 前置已满足且保持：Copilot token 交换必须复用 state 域方法，不得重新引入 state 外直接写**。剩余非阻塞结构项：`api/types.rs`（1295 行 / 127 个 DTO）的域专属 DTO 随后续域改动就近搬到对应 handler 文件，`types.rs` 只保留跨域共享类型 |
| R6 workspace 化 | 挂起，三个触发条件（增量编译变慢 / 第二交付物 / 多人并行）均未满足；R2/R3 目录边界即未来 crate 边界 |

后续工作回到功能主线 `docs/code-audit-gap-plan.md`：P0 已清零；X4/X5/X6 第一/二/三批/X7/X8 第一批/X9/X10/X11 已静态落地，R4 存储写路径收敛已完成。剩余本地可实施项以 X8 后续 CSS 收口和 `api/types.rs` DTO 就近化为主，真实发信/OAuth/router/market 验收仍按外部环境 gate 推进。

## 八、变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-07-06 | 初版：基于同日架构评审建立 R1–R6 分阶段计划、目标架构、文件映射表与红线 |
| 2026-07-06 | 收紧 R1 依赖方向门禁：`proxy/` 不得直接依赖 `clients/`，出站客户端编排通过状态/控制面封装 |
| 2026-07-07 | 新增 R7（实施后复核）：修复 infra→domain/clients 方向违例（now_ms 下沉、live_config_import 归 domain、backup 目标清单参数化）；门禁模式从 `use` 语句升级为行内全限定路径，并补 infra/clients 两个方向 |
| 2026-07-07 | **Phase R 关闭**：R1–R7 全部实施、复核通过并提交（`65721b8`）；新增第七节关闭登记，R4 剩余 59 处直接写与 R6 触发条件转为长期跟踪项 |
| 2026-07-07 | R4-accounts 收敛：state 外 accounts 直接写清零，`accounts` 字段降 `pub(crate)`；R4 剩余计数从 59 降至 42，X5 accounts 前置解除 |
| 2026-07-07 | Phase X 状态回写：X4 owner 验证流静态实现后，后续主线更新为 X6、X8 后续和 R4 剩余域收敛；外部真实验证继续保留 gate |
| 2026-07-07 | R4-pricing 收敛：state 外 pricing 直接写清零，`pricing` 字段降 `pub(crate)`；R4 剩余计数从 42 降至 40 |
| 2026-07-07 | R4-universal_providers 收敛：state 外 universal provider 写路径清零，字段降 `pub(crate)`；R4 剩余计数从 40 降至 37 |
| 2026-07-07 | R4-sessions 收敛：legacy bearer session clear/push 写路径收敛到 state 域方法，字段降 `pub(crate)`；R4 剩余计数从 37 降至 34 |
| 2026-07-07 | R4-oauth_logins 收敛：OAuth login start/finish/poll/mark 写路径收敛到 state 域方法，字段降 `pub(crate)`；按当前 grep 复核剩余 32 处（ui_settings 14、providers 8、failover 10） |
| 2026-07-07 | R4-failover 收敛：控制面配置/重置写路径和 proxy 熔断热路径写入收敛到 state 域方法，字段降 `pub(crate)`；R4 剩余计数从 32 降至 22 |
| 2026-07-07 | R4-providers 收敛：provider CRUD/import/sort/universal sync/fetch-model merge 写路径收敛到 state 域方法，字段降 `pub(crate)`；R4 剩余计数从 22 降至 14 |
| 2026-07-07 | R4-ui_settings 收敛：invoke/settings/proxy app config 写路径收敛到 state 域方法并立即保存，字段降 `pub(crate)`；R4 剩余计数从 14 降至 0 |
| 2026-07-07 | R4 完成复核：config/usage 字段降 `pub(crate)`，集成测试改用 snapshot/replace_config 方法；`static-checks.sh` 状态写入门禁扩展到全部受管 store 的多行写锁和直接保存调用 |
| 2026-07-07 | Phase X 状态回写：X6 第一/二批 Kiro 协议桥、Account→request plan 与 25 个 fixture 已进入 server；后续剩 forwarder 发送/响应桥接、CSS 收口与 DTO 就近化 |
| 2026-07-07 | Phase X 状态回写：X6 第三批完成，Claude + KiroOAuth forwarder managed-account 发送路径、非流式 JSON 与流式 SSE 响应桥接已接线并由 mock CodeWhisperer 合同测试覆盖；capability 升级仍受真实账号 gate 约束 |
