# 代码审计缺口修复计划（Phase X）

> **来源**：2026-07-06 对 desktop（`/data/projects/cc-switch` @ `d7d33e51`）、server（本仓库 @ `6fce8e4`）、router（`/data/projects/cc-switch-router`）三方源码的独立交叉审计。审计**不采信任何既有规划文档的状态标记**，全部结论来自代码比对与实测（`cargo test` 503 passed / typecheck / 路由与命令面逐条 diff）。
> **定位约束**：server 是 token 反代服务端；desktop 客户端专属功能（Skill/MCP/OpenClaw/Hermes/OMO/prompts/sessions/deeplink 等）维持排除，本计划不涉及。
> **关联文档**：`docs/server-desktop-ui-parity-plan.md`（UI 同源移植主计划）、`assets/contract/web-runtime-contract.json`（功能契约）、`UPSTREAM_IMPORT.md`（上游吸收台账）。

## 审计结论摘要（2026-07-06 基线）

| 维度 | 实测结果 |
| --- | --- |
| 命令面 | desktop 338 个注册命令中 227 个进入 server 契约（226 implemented），其余 111 个全部为客户端专属功能，主线零遗漏 |
| router 契约 | router→client 控制面（`/_ctl/*`、`/_share-router/*`）全部实现；client→router 调用面缺 3 个次要端点（见 X9/X10） |
| proxy 管线 | 主流组合 native 已落地；Copilot/Kiro/DeepSeekAccount 为 fallback skeleton，Cursor/Bedrock 为 planned；in-module 测试深度约为 desktop 的 1/5（见 X5–X7） |
| 前端 | desktop 231 个组件文件中 217 个同路径存在、203 个字节级相同；i18n 四语言为 desktop 严格超集；1 个 TS 编译错误（见 X1）；`styles.css` 5817 行过渡层仍被引入（见 X8） |
| 工程 | `cargo test` 503/503 通过；`sync-desktop-ui.mjs --check` 处于失败状态（见 X3）；desktop 新提交 `d7d33e51` 未吸收（见 X2） |

---

## 任务状态（2026-07-07 复核更新）

| 任务 | 状态 | 依据 |
| --- | --- | --- |
| X1 typecheck 修复 | **已完成** | 提交 `18cbca5`；typecheck exit 0 复核通过 |
| X3 sync 漂移门禁 | **已完成** | `node scripts/sync/sync-desktop-ui.mjs --check` exit 0，含同源树反向漂移检测 |
| Phase R 结构重构 | **已完成并关闭** | R1–R7 全部实施，提交 `65721b8`；关闭登记见 `docs/architecture-refactor-plan.md` 第七节 |
| X2 Ollama clamp 吸收 | **待办（P0 唯一剩余）** | `rg 'xhigh' src/proxy` 零命中；desktop 漂移仍停在 `d7d33e51` 单个提交 |
| X4–X11 | 待办 | 按下方执行顺序推进；文中 `src/http.rs`、`src/core/*` 旧路径按 Phase R 映射表对应到 `src/api/*`、`src/domain/*`、`src/clients/*` |

## P0 — 阻塞构建 / 门禁失效（应最先完成）

### X1 修复 web typecheck 错误（阻塞前端构建）

- **现状证据**：`npm --prefix web-src run typecheck` exit 2。唯一错误：
  `web-src/src/components/settings/ServerSecuritySettings.tsx(108,13): error TS2322` —— `onClick={onSignOut}` 把 `MouseEvent` 传给了 `(options?: { clearPasswordCache?: boolean }) => void`。
- **实施细节**：第 108 行改为 `onClick={() => onSignOut()}`（不透传事件对象；保持第 47 行 `onSignOut({ clearPasswordCache: false })` 调用方语义不变）。
- **验收标准**：`npm --prefix web-src run typecheck` exit 0；`npm --prefix web-src run build` exit 0；`scripts/static-checks.sh` 通过。
- **工作量**：S（分钟级）。**依赖**：无。

### X2 吸收 desktop 上游提交 `d7d33e51`（Ollama Codex reasoning effort clamp）

- **现状证据**：desktop `d7d33e51`（2026-07-04）在 `providers/codex.rs` 增加 Ollama 平台规则（Ollama 的 reasoning effort 枚举**拒绝 `xhigh`、接受 `max`**，`effort_value_mode: "ollama"`），并在 `transform_codex_chat.rs` 增加 clamp 逻辑 + 测试（共 104 行）。server `src/proxy/transforms.rs` 的 effort 映射路径（`:172`、`:244`、`:2228-2247`）为纯透传，全仓无任何 `xhigh` 处理——模型别名（如 gpt-5.5）在 provider model mapping 生效前会把 `xhigh` 原样转发给 Ollama 导致 4xx。
- **实施细节**：
  1. 在 server 的 Codex Chat 请求归一路径（`transforms.rs` reasoning effort 写出处）增加平台规则：目标 provider 为 `OllamaCloud`（或 base URL 含 `ollama`）时，effort 值按 desktop `effort_value_mode="ollama"` 语义 clamp（`xhigh → max`），并保持 thinking 参数按 desktop 规则输出。
  2. 平台判定接线点放在 adapter 侧（`src/proxy/adapters.rs` Codex→Ollama 分支）传入 transforms，避免 transforms 直接感知 provider 类型字符串。
  3. 移植 desktop 该提交的测试用例到 server fixture（覆盖：别名模型 + `xhigh` 输入 → `max` 输出；非 Ollama 目标不 clamp）。
  4. `UPSTREAM_IMPORT.md` 登记 `d7d33e51` 为「已移植」。
- **验收标准**：新 fixture 通过；`cargo test` 全绿；台账登记完成。
- **工作量**：S–M。**依赖**：无。

### X3 修复 `sync-desktop-ui.mjs` 漂移门禁（当前 --check 常态失败）

- **现状证据**：`node scripts/sync/sync-desktop-ui.mjs --check` 失败 3 项：
  1. `defaultPaths` 引用 `components/providers/ProviderPresetSelector.tsx`——desktop 已不存在该文件；
  2. 引用 `components/providers/ProviderHealthIndicator.tsx`——desktop 实际文件是 `HealthStatusIndicator.tsx` / `ProviderHealthBadge.tsx`（server 已同源持有 `ProviderHealthBadge.tsx`）；
  3. desktop `components/share/CreateShareDialog.test.ts` 未同步到 server。
  门禁失败意味着 desktop UI 后续漂移**不会被发现**。
- **实施细节**：
  1. 更新 `scripts/sync/sync-desktop-ui.mjs` 的 `defaultPaths`：删除 2 个失效条目，按 desktop 现状补 `ProviderHealthBadge.tsx`（`HealthStatusIndicator.tsx` 属 ProxyPanel 依赖，随其同步状态决定是否登记豁免）；
  2. 同步 `CreateShareDialog.test.ts` 到 `web-src/src/components/share/`（若 server 测试链路不跑 vitest，则在脚本中为 `*.test.ts` 建立显式跳过清单，禁止静默失败）；
  3. 将 `sync-desktop-ui.mjs --check` 纳入 `scripts/static-checks.sh`（当前未纳入，属门禁盲区）。
- **验收标准**：`node scripts/sync/sync-desktop-ui.mjs --check` exit 0；`scripts/static-checks.sh` 包含该检查且通过。
- **工作量**：S。**依赖**：无。

---

## P1 — 功能/语义缺口（server 内可静态完成，capability 升级另有真实验收 gate）

### X4 Share owner 变更验证码流对齐 desktop（决策 + 实现）

- **现状证据**：desktop owner 变更需两步验证（`commands/email_auth.rs` 的 `email_auth_request_owner_change_code` → 向新 owner 发码 → `email_auth_change_owner_email`，经 router `/v1/installations/change-owner-email`）。server 当前 `web_transfer_share_owner` / `web_update_share_owner_email`（`src/http.rs:6378/6399`）只要求 admin 会话 + 目标邮箱已在 ACL（transfer 路径）+ 格式校验——`core/shares.rs:1017` 的 `normalize_verified_email` **只做格式检查，不做任何验证**（测试名 `update_owner_email_renormalizes_acl_without_verification` 亦自证）。这 7 个 `email_auth_*` 命令是 desktop 338 命令中唯一「主线相关但未进 server 契约」的一组。
- **决策项（先于实现）**：单管理员部署下当前约束是否足够？两个方案：
  - **方案 A（推荐，对齐 desktop）**：owner 变更前必须向新 owner 邮箱发验证码并校验。
  - **方案 B**：维持现状，在契约 notes 与文档中显式记录「server 信任 admin 会话，owner 变更不发码」为有意分歧。
- **实施细节（方案 A）**：
  1. `src/core/email_auth.rs` 复用既有 router 签名调用基建，新增 `request_owner_change_code(new_email)` / `verify_owner_change_code(new_email, code)`，对接 router `/v1/auth/email/request-code` 与 `/v1/installations/change-owner-email`（router 侧端点已在线，见 router `api.rs` 路由表）；
  2. `transfer_owner_email` / `update_owner_email` 增加「验证码通过」前置：新增内存 pending-verification store（复用 `oauth_login.rs` 的 5 分钟过期会话模式）；
  3. `/web-api/invoke` 契约新增 `email_auth_request_owner_change_code`、`email_auth_change_owner_email` 两个命令（`assets/contract/web-runtime-contract.json` + dispatcher 分支 + 审计脚本双向校验）；
  4. 前端 `web-src/src/components/share/OwnerChangeModal.tsx`（server-local 组件，已存在）改为两步流：输入新邮箱 → 发码 → 输入验证码 → 提交。
- **验收标准**：未验证时 transfer/update 返回 4xx 且错误信息可诊断；验证通过后转移成功并触发 share sync；单测覆盖 pending 过期、验证码错误、重复提交；契约审计通过。
- **工作量**：M。**依赖**：无（真实发信路径归入真实环境验收）。

### X5 Copilot 请求时 internal token 交换与端点发现

- **现状证据**：desktop `providers/copilot_auth.rs`（2105 行）在转发时用 GitHub token 换取短时效 Copilot internal token（`{github_api_base}/copilot_internal/v2/token`），并经 `/copilot_internal/user` 发现每账号 API endpoint（含 GHES 分支），带续期缓存（key = GitHub user id）。server 当前 Copilot 分支（`src/proxy/adapters.rs`，fixture `claude_copilot_static_preflight_uses_chat_endpoint_and_optimizer_headers`）**只把 provider 配置里的静态 bearer 原样转发**——没有请求时交换、没有端点发现、没有续期。静态 token 过期后所有 Copilot 请求都会失败，这是 Copilot 组合停留在 fallback skeleton 的真正代码缺口。
- **实施细节**：
  1. 新增 `src/proxy/copilot_auth.rs`（或并入 `src/core/copilot_device.rs` 同族模块）：`exchange_internal_token(github_token, domain) -> {token, expires_at, api_endpoint}`；GHES 域名分支沿 desktop `github_api_base(domain)` 规则；
  2. `ServerState` 增加 per-account internal token 缓存（`RwLock<HashMap<accountId, CachedCopilotToken>>`，过期前 60s 视为失效并重换）；
  3. adapter Copilot 分支改为：绑定 managed account（`accounts.json` 中 device flow 导入的 GitHub token）时走交换 + endpoint 发现；provider 配置显式给静态 token 时保留现行为（向后兼容）；
  4. 交换请求走 A10 代理感知 client；失败时返回结构化 `upstream_error` 而非 panic/静默；
  5. fixture：交换请求 shape（URL/header）、缓存命中不重复交换、过期重换、GHES 域名分支、静态 token 旁路。
- **验收标准**：静态 fixture 全绿；`copilot_model_map` / `copilot_optimizer` 现有测试不回归。**capability 升级 gate**：真实 device flow 账号 non-stream/stream + usage 口径验收后才把 Copilot×3 从 fallback 升级（不在本计划内）。
- **工作量**：L。**依赖**：**先完成 R4 的 accounts 域收敛**（X5 会给 accounts 新增后台并发写路径，当前 api 层还有 17 处直接写，见 `docs/architecture-refactor-plan.md` 第七节）；真实验收依赖外部凭据。

### X6 Kiro 转发桥移植

- **现状证据**：desktop `providers/kiro_claude.rs`（3119 行 / 23 测试）实现 Claude Messages ↔ Kiro（CodeWhisperer conversation API）双向桥。server 三个 app 的 Kiro 组合均为 `fallback("*_kiro_skeleton")`（`src/proxy/adapters.rs:448/494/522`）——账号 device flow 导入已有（`core/kiro_device.rs`），但**转发路径完全没有**，是 15 个非 native 组合中除 Copilot 外唯一「代码确实缺失」的一组。
- **实施细节**：
  1. 新增 `src/proxy/kiro.rs`：移植 desktop 的请求构造（conversation payload、profile ARN、机器指纹头）、响应/事件流解析、Claude Messages 双向转换；
  2. Codex/Gemini 入口按 desktop 同构路径复用既有 `transforms.rs` 的 Claude 中间表示（先转 Claude Messages 再进 Kiro 桥），与 desktop 的 app 覆盖面对齐；
  3. token 续期沿用 `kiro_device.rs` 的 refresh 路径；请求走代理感知 client；
  4. 移植 desktop 23 个测试中的协议构造/解析用例为 server fixture；
  5. adapter 从 `fallback` 改为 `planned`（静态接线完成）——**native 升级 gate 同 X5**（真实 AWS Builder ID 账号验收）。
- **验收标准**：fixture 覆盖请求构造、事件流→Claude SSE 转换、错误路径；`cargo test` 全绿；capability 矩阵与 Web 展示同步更新。
- **工作量**：XL（desktop 源 3.1k 行）。**依赖**：建议在 X5 之后（同为「managed account → 请求时凭据」模式，可复用缓存基建）。

### X7 transform/streaming 黄金用例补齐

- **现状证据**：desktop transform 系列 203 个测试（`transform.rs` 59 / `transform_codex_chat.rs` 57 / `transform_responses.rs` 61 / `transform_gemini.rs` 26）+ streaming 系列 52 个（12/17/9/14）；server in-module 仅 `transforms.rs` 11 + `streaming.rs` 11 + `adapters.rs` 64（另有 fixture 宏批量用例）。**流式 tool-call 增量重组、parallel tool calls、stop_reason 映射矩阵、图片块、SSE 边界切割**等 desktop 高价值回归用例在 server 侧覆盖明显偏薄——这是转发正确性的主要风险面。
- **实施细节**：
  1. 以 desktop 四个 transform 文件 + 四个 streaming 文件的 `#[test]` 清单为源，逐个映射到 server `transforms.rs`/`streaming.rs`/`adapters.rs` 的对应入口，输出「已覆盖 / 需移植 / 不适用（desktop-only 语义）」三列清单，落到本文档附录或 `docs/` 下独立清单；
  2. 优先移植四类：流式 tool-call 增量重组（含 parallel）、跨协议 stop_reason/finish_reason 映射全矩阵、SSE chunk 边界（半帧/多帧/CRLF）、图片与多模态块转换；
  3. 用 server 既有 streaming fixture 宏承载，避免逐个手写样板；
  4. 更新 `scripts/audit/audit-transform-coverage.mjs` 的基线与目标值（当前目标 70%，建议提升至 85–90%）并保持纳入 `static-checks.sh`。
- **验收标准**：审计脚本新目标达成；`cargo test` 全绿；清单三列可追踪。
- **工作量**：L（可按 transform → streaming 分两批交付）。**依赖**：X2 先行（避免同文件冲突）。

---

## P2 — 收尾与决策项

### X8 `styles.css` 过渡层削减收口

- **现状证据**：`web-src/src/styles.css` 5817 行 server 自建过渡样式仍被 `main.tsx:13` 引入，与 desktop 同源 `index.css` 双设计系统并存。组件已 203/217 字节级同源，过渡 class 的实际引用面应已大幅缩小。
- **实施细节**：
  1. 写一次性审计（可并入 `scripts/`）：提取 `styles.css` 全部 class 名，grep `web-src/src` 统计仍被引用的 class 集合；
  2. 删除零引用 block；仍被引用的按归属页面迁移到组件内 tailwind class 或确认为 login/setup 最小集（`ClientWebLoginPage`/`LoginPanel` 为 server-local 组件，允许保留专属样式）；
  3. 目标态：`styles.css` 只剩 login/setup 与 server-only 组件的最小集（建议 <500 行），并在 `static-checks.sh` 加行数上限门禁防回涨。
- **验收标准**：typecheck/build 通过；人工 checklist 抽查 providers/shares/settings 三页无样式回归；行数门禁生效。
- **工作量**：M（随页面分批）。**依赖**：无；与 X1 后的任意时间并行。

### X9 router 次要契约补齐：runtime-refresh 通知 + client-tunnel 状态/释放

- **现状证据**：desktop `tunnel/sync.rs:524` 在本地 share/provider 变更后调用 router `POST /v1/shares/runtime-refresh` 主动触发 router 重拉 runtime；desktop `tunnel/connection.rs:157` 使用 `/v1/installations/client-tunnel`（状态查询/释放）。server 均未调用——runtime 时效依赖 batch-sync 推送与 router 自主经 `_share-router/share-runtime` 拉取，client tunnel stop 只做本地清理，router 侧 lease 悬挂到自然过期。
- **实施细节**：
  1. `src/core/router_client.rs` 新增 `notify_runtime_refresh()`（POST `v1/shares/runtime-refresh`，签名与既有调用一致），在 share 绑定/暂停/恢复/usage reset 等影响 runtime 的写路径成功后异步触发（失败仅记 `last_router_error`，不阻塞主流程）；
  2. `src/core/tunnel.rs` stop 路径追加对 router `/v1/installations/client-tunnel` 的释放调用（DELETE/POST 以 router `api.rs` 实际 handler 为准，实施前核对），未注册 identity 时跳过；
  3. fixture：两个调用的请求 shape + 失败降级不影响本地状态。
- **验收标准**：静态 fixture 通过；真实时效改善归入隧道真实验收观察。
- **工作量**：S–M。**依赖**：无。

### X10 heartbeat 语义决策（记录或接线）

- **现状证据**：server `/api/router/heartbeat`（`src/http.rs:4004`）只更新本地时间戳并置 `router_registered=true`，**不与 router 通信**；router 侧存在 `POST /v1/shares/heartbeat`（`api.rs:268` → `record_share_heartbeat`），desktop 同样未调用。当前 UI 的「heartbeat」按钮语义与用户预期（探活 router）不符。
- **决策项**：
  - **方案 A（推荐，最小改动）**：`/api/router/heartbeat` 改为真实探测——对 router 发一次轻量已鉴权请求（如 `v1/shares/pending-edits` 空拉或 `v1/auth/session/me`），成功才更新时间戳，失败写 `last_router_error`；
  - **方案 B**：接线 router `POST /v1/shares/heartbeat`（需核对 router 侧对 payload/metadata 的要求，两个 client 都未用过该端点，需与 router 维护方确认语义）；
  - **方案 C**：维持现状，把 API/UI 文案改为「标记在线」类本地语义，消除误导。
- **验收标准**：所选方案落地后，heartbeat 结果能真实反映 router 可达性（A/B）或文案不再误导（C）；决策记录在本文档。
- **工作量**：S（A/C）或 M（B）。**依赖**：方案 B 需 router 侧确认。

### X11 品牌图标体积豁免登记

- **现状证据**：desktop `src/icons/extracted/` 96 个文件中 26 个未移植，全部为品牌位图（PNG/JPG/WebP，如 byteplus/huoshan/qiniu 等）+ `mcp.svg`（excluded 功能）。对应 provider preset 卡片在 server 上回退到 `iconInference` 首字母图标。
- **实施细节**：
  1. 在 `docs/server-desktop-ui-parity-plan.md` 的豁免登记处（或本文档附录）列出 26 个文件与豁免理由（embedded 体积门禁 2MB）；
  2. 可选优化（不作为验收项）：位图转 WebP 压缩后评估能否纳入 `locales` 之外的懒加载 chunk；若单文件 >30KB 维持豁免；
  3. `mcp.svg` 标记为 excluded 功能资产，永久跳过。
- **验收标准**：豁免清单可追踪；`audit-web-dist-size.mjs` 门禁维持通过。
- **工作量**：S。**依赖**：无。

---

## 明确不在本计划内（边界）

1. **真实环境验收**：router 隧道实连、direct share 端到端、market 调度、OAuth 订阅账号真实转发、Cursor（planned，静态 driver 已接线）与 Bedrock（planned，SigV4 合同已生成）的真实验收——全部依赖外部凭据/环境，沿既有 runbook 推进；X5/X6 的 capability 从 fallback/planned 升级 native 均以真实验收为 gate。
2. **`set_window_theme`**：契约中唯一 `implemented=false` 命令，浏览器由 ThemeProvider 覆盖，维持现状。
3. **`v1/shares/sync`（单条同步）**：server 用 batch-sync 覆盖同一语义，不补。
4. **DeepSeek 账密登录/PoW 栈**：维持「server 不保存账密、import-only」决策，不移植。

## 执行顺序

```
✅ X1 → X3（已完成）
✅ Phase R 结构重构（R1–R7 已完成并关闭，提交 65721b8）
  → X2（P0 唯一剩余，半天）
  → X4（owner 验证流，先决策 A/B）‖ X7 第一批（transform 用例）
  → R4-accounts 收敛（X5 硬性前置）→ X5（Copilot token 交换）→ X6（Kiro 桥，复用 X5 基建）
  → X7 第二批（streaming 用例）
  → X8 / X9 / X10 / X11（收尾，可穿插并行）
```

> **与 Phase R 的关系**：Phase R 已关闭（2026-07-07）。X4–X11 文中引用的 `src/http.rs`、`src/core/*` 旧路径按 R2/R3 映射表对应到 `src/api/*`、`src/domain/*`、`src/clients/*`；R4 剩余的存储收敛（59 处直接写）与 `api/types.rs` DTO 就近化随 X 系列功能 PR 摊销，其中 accounts 域是 X5 的硬性前置。

## 验证基线（每个任务完成前必须通过）

```bash
cargo fmt --check
cargo check --all-targets   # 直接取退出码，禁止管道吞码
cargo test
npm --prefix web-src run typecheck
npm --prefix web-src run build
scripts/static-checks.sh
node scripts/sync/sync-desktop-ui.mjs --check   # X3 完成后纳入 static-checks
```

## 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-07-06 | 初版：基于三方代码交叉审计（不采信文档状态）建立 X1–X11 任务与边界 |
| 2026-07-07 | 状态更新：X1/X3 完成、Phase R 完成并关闭（`65721b8`）；X2 为 P0 唯一剩余；X5 增加 R4-accounts 收敛硬性前置；执行顺序同步 |
