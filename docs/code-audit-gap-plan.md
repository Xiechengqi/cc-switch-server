# 代码审计缺口修复计划（Phase X）

> **来源**：2026-07-06 对 desktop（`/data/projects/cc-switch` @ `d7d33e51`）、server（本仓库 @ `6fce8e4`）、router（`/data/projects/cc-switch-router`）三方源码的独立交叉审计。审计**不采信任何既有规划文档的状态标记**，全部结论来自代码比对与实测（`cargo test` 503 passed / typecheck / 路由与命令面逐条 diff）。
> **定位约束**：server 是 token 反代服务端；desktop 客户端专属功能（Skill/MCP/OpenClaw/Hermes/OMO/prompts/sessions/deeplink 等）维持排除，本计划不涉及。
> **关联文档**：`docs/server-desktop-ui-parity-plan.md`（UI 同源移植主计划）、`assets/contract/web-runtime-contract.json`（功能契约）、`UPSTREAM_IMPORT.md`（上游吸收台账）。

## 审计结论摘要（2026-07-06 基线）

| 维度 | 实测结果 |
| --- | --- |
| 命令面 | desktop 338 个注册命令中 234 个进入 server 契约（234 implemented；X4 补入 7 个 email_auth 主线命令），其余 104 个为客户端专属功能或显式排除 |
| router 契约 | router→client 控制面（`/_ctl/*`、`/_share-router/*`）全部实现；client→router 次要端点已补齐 heartbeat 探测、runtime-refresh、client-tunnel 状态/释放（见 X9/X10） |
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
| X2 Ollama clamp 吸收 | **已完成** | `src/proxy/adapters.rs` 针对 Ollama 目标传入 `ReasoningEffortMode::Ollama`；fixture 覆盖 `xhigh→max`、显式关闭→`none`、非 Ollama 透传；`UPSTREAM_IMPORT.md` 已登记 `d7d33e51` |
| X4–X11 | 进行中 | X4 静态实现已落地（7 个 `email_auth_*` invoke 命令入契约；owner change 走新邮箱验证码 + router `/v1/installations/change-owner-email`；直接 owner update/transfer 增加 verified target gate）；X5 静态实现已落地（请求时 Copilot internal token 交换、endpoint 发现、per-account 缓存；真实 capability 升级仍待外部账号验收）；X6 第一批已落地（server-native `proxy::kiro` 协议桥 + desktop 23 个 fixture，暂不接真实转发）；X7 第二批 streaming 覆盖已落地，门禁提升到 78%；X8 第一批 CSS 过渡层削减已落地（5817→2660 行，新增 3000 行门禁）；X9 已补齐 runtime-refresh 与 client-tunnel 状态/释放合同；X10 方案 A 已落地（heartbeat 真实探测 router）；X11 品牌图标豁免已登记；R4 受管 store 生产写路径已清零并纳入静态门禁；文中 `src/http.rs`、`src/core/*` 旧路径按 Phase R 映射表对应到 `src/api/*`、`src/domain/*`、`src/clients/*` |

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

- **状态（2026-07-07）**：**静态实现已完成**。`src/clients/router/email_auth.rs` 新增 owner-change router client（新邮箱验证码校验后，签名调用 `/v1/installations/change-owner-email`）；`/web-api/invoke` 契约补入 7 个 `email_auth_*` 主线命令；`email_auth_change_owner_email` 成功后同步更新 `server.json` owner email、批量更新本地旧 owner shares 并触发 share sync/event；旧 `update_share_owner_email` / `transfer_share_owner` 兼容入口增加 verified target gate，未验证的新 owner 返回 4xx；Share 页面接入 `ShareOwnerChangeEmailDialog` 两步入口。真实发信、router 线上限流和端到端邮箱收码仍归入真实环境验收。
- **现状证据**：desktop owner 变更需两步验证（`commands/email_auth.rs` 的 `email_auth_request_owner_change_code` → 向新 owner 发码 → `email_auth_change_owner_email`，经 router `/v1/installations/change-owner-email`）。server 当前 `web_transfer_share_owner` / `web_update_share_owner_email`（`src/http.rs:6378/6399`）只要求 admin 会话 + 目标邮箱已在 ACL（transfer 路径）+ 格式校验——`core/shares.rs:1017` 的 `normalize_verified_email` **只做格式检查，不做任何验证**（测试名 `update_owner_email_renormalizes_acl_without_verification` 亦自证）。这 7 个 `email_auth_*` 命令是 desktop 338 命令中唯一「主线相关但未进 server 契约」的一组。
- **决策项（先于实现）**：单管理员部署下当前约束是否足够？两个方案：
  - **方案 A（推荐，对齐 desktop）**：owner 变更前必须向新 owner 邮箱发验证码并校验。
  - **方案 B**：维持现状，在契约 notes 与文档中显式记录「server 信任 admin 会话，owner 变更不发码」为有意分歧。
- **实施细节（方案 A）**：
  1. ✅ `src/clients/router/email_auth.rs` 复用既有 router 签名调用基建，新增 `change_owner_email(old_email, new_email, access_token)`，对接 router `/v1/installations/change-owner-email`；发码复用 `/v1/auth/email/request-code`，验证码校验复用 `/v1/client-web/auth/email/verify-code`；
  2. ✅ `transfer_owner_email` / `update_owner_email` 增加 verified target gate：目标邮箱必须已是当前 configured owner 且本地 `email-auth.json` 登录状态匹配；新 owner 变更必须走 `email_auth_change_owner_email`；
  3. ✅ `/web-api/invoke` 契约新增 7 个 `email_auth_*` 命令（`assets/contract/web-runtime-contract.json` + dispatcher 分支 + 审计脚本双向校验）；
  4. ✅ 前端 `ShareOwnerChangeEmailDialog` 已接入 Share 页面，复用 `web-src/src/lib/api/emailAuth.ts` 的发码/验证封装。
- **验收标准**：未验证时 transfer/update 返回 4xx 且错误信息可诊断；验证通过后转移成功并触发 share sync；契约审计通过。真实验证码错误/过期/重复提交需 router 发信环境，归入真实环境验收。
- **工作量**：M。**依赖**：无（真实发信路径归入真实环境验收）。

### X5 Copilot 请求时 internal token 交换与端点发现

- **状态（2026-07-07）**：**静态实现已完成**。`src/clients/oauth/copilot_device.rs` 提供 Copilot internal token 交换与 `/copilot_internal/user` endpoint 发现；`ServerStateInner::prepare_copilot_upstream_auth` 增加 per-account 内存缓存（过期前 60 秒刷新），GHES 分支按 desktop 语义直接使用 GitHub token 并回退 `copilot-api.{domain}`；`forwarder` 在 GitHub Copilot managed-account 路径覆盖 `Authorization` 与 API endpoint，provider 静态 secret 继续旁路。补充测试覆盖交换请求 shape、endpoint 发现/回退、缓存种子、静态 secret 旁路、Codex/Gemini Copilot 分类和 API 合同级 endpoint/header 覆盖。**capability 仍不得升级 native**，需真实 device-flow 账号 non-stream/stream + usage 验收后另行调整。
- **现状证据**：desktop `providers/copilot_auth.rs`（2105 行）在转发时用 GitHub token 换取短时效 Copilot internal token（`{github_api_base}/copilot_internal/v2/token`），并经 `/copilot_internal/user` 发现每账号 API endpoint（含 GHES 分支），带续期缓存（key = GitHub user id）。server 当前 Copilot 分支（`src/proxy/adapters.rs`，fixture `claude_copilot_static_preflight_uses_chat_endpoint_and_optimizer_headers`）**只把 provider 配置里的静态 bearer 原样转发**——没有请求时交换、没有端点发现、没有续期。静态 token 过期后所有 Copilot 请求都会失败，这是 Copilot 组合停留在 fallback skeleton 的真正代码缺口。
- **实施细节**：
  1. ✅ 新增 client 侧 `fetch_copilot_internal_token` / `fetch_copilot_api_endpoint`：GHES 域名分支沿 desktop `github_api_base(domain)` / `copilot_api_base(domain)` 规则；
  2. ✅ `ServerState` 增加 per-account internal token 缓存（过期前 60s 视为失效并重换）；
  3. ✅ forwarder Copilot 分支改为：绑定 managed account（`accounts.json` 中 device flow 导入的 GitHub token）时走交换 + endpoint 发现；provider 配置显式给静态 token 时保留现行为（向后兼容）；
  4. ✅ 交换请求走 server 的代理感知 `reqwest::Client`；失败返回结构化 `upstream_error`；
  5. ✅ fixture：交换请求 shape（URL/header）、缓存种子、endpoint 发现/回退、GHES fallback 规则、静态 token 旁路、API 合同级 endpoint/header 覆盖。
- **验收标准**：静态 fixture 全绿；`copilot_model_map` / `copilot_optimizer` 现有测试不回归。**capability 升级 gate**：真实 device flow 账号 non-stream/stream + usage 口径验收后才把 Copilot×3 从 fallback 升级（不在本计划内）。
- **工作量**：L。**依赖**：R4 的 accounts 域收敛已完成（state 外零直接写，字段已降 `pub(crate)`；X5 新增后台并发写路径必须复用 state 域方法）；真实验收依赖外部凭据。

### X6 Kiro 转发桥移植

- **状态（2026-07-07）**：**第一批协议桥已完成**。新增 `src/proxy/kiro.rs`，server-native 移植 desktop `kiro_claude.rs` 的 Claude Messages → Kiro conversation payload、Kiro event stream → Claude SSE/JSON、tool leak rescue、thinking/redacted thinking、usage/prompt-cache 计算等纯协议逻辑；desktop 23 个 fixture 已全部进入 server 并通过。当前尚未接入真实请求路径、账号 token refresh/failover 和 capability 矩阵，Kiro 组合仍保持 fallback skeleton，避免未真实验收前误报 native。
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

- **状态（2026-07-07）**：**第二批已完成**。第一批覆盖 request/response transform 与 stop_reason 基础矩阵；第二批补齐 OpenAI Chat/Responses/Gemini/Anthropic streaming tool-call delta 双向映射、关键 stream finish_reason 映射、SSE CRLF 多帧 chunk 解析；`audit-transform-coverage.mjs` 默认门禁从 75%/190 提升到 78%/198。半帧跨 chunk 重组仍需 forwarder/adapter 有状态缓冲设计，继续保留为后续架构项。
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

- **状态（2026-07-07）**：**第一批已完成**。新增 `scripts/audit/audit-css-transition-layer.mjs` 并纳入 `scripts/static-checks.sh`；按源码 class 引用面机械删除零引用规则和零引用 selector 分支，`web-src/src/styles.css` 从 5817 行降到 2660 行，构建 CSS 资产约 173KB 降到约 126KB；默认门禁 `CC_SWITCH_STYLES_MAX_LINES=3000` 防止过渡层回涨。剩余 29 个零引用 class 多为状态/派生选择器和混合规则残留，后续需结合页面截图或组件内迁移继续收口，目标 <500 行暂未达到。
- **现状证据**：`web-src/src/styles.css` 5817 行 server 自建过渡样式仍被 `main.tsx:13` 引入，与 desktop 同源 `index.css` 双设计系统并存。组件已 203/217 字节级同源，过渡 class 的实际引用面应已大幅缩小。
- **实施细节**：
  1. ✅ 写入审计脚本：提取 `styles.css` 全部 class 名，grep `web-src/src` 统计仍被引用的 class 集合；
  2. ✅ 第一批删除零引用 block / selector 分支；仍被引用或混合派生的规则保留，避免无截图情况下误删；
  3. ✅ `static-checks.sh` 增加行数上限门禁防回涨；目标态仍是 `styles.css` 只剩 login/setup 与 server-only 组件的最小集（建议 <500 行），后续需按页面继续迁移。
- **验收标准**：typecheck/build 通过；人工 checklist 抽查 providers/shares/settings 三页无样式回归；行数门禁生效。
- **工作量**：M（随页面分批）。**依赖**：无；与 X1 后的任意时间并行。

### X9 router 次要契约补齐：runtime-refresh 通知 + client-tunnel 状态/释放

- **状态（2026-07-07）**：**已完成**。`src/clients/router/client.rs` 已新增 `notify_runtime_refresh()`、`get_client_tunnel()`、`update_client_tunnel()` / `release_client_tunnel()`；`/api/router/batch-sync` 与 share upsert 同步成功后会追加 `POST /v1/shares/runtime-refresh`，失败只记录 `last_router_error`；`/api/router/client-tunnel` GET 会 best-effort 查询 router 远端状态，stop 路径会 PATCH `/v1/installations/client-tunnel` 写入 `enabled=false` 后返回本地 stopped 状态。合同测试覆盖状态查询、释放请求 shape、runtime-refresh 请求 shape。
- **现状证据**：desktop `tunnel/sync.rs:524` 在本地 share/provider 变更后调用 router `POST /v1/shares/runtime-refresh` 主动触发 router 重拉 runtime；desktop `tunnel/connection.rs:157` 使用 `/v1/installations/client-tunnel`（状态查询/释放）。server 均未调用——runtime 时效依赖 batch-sync 推送与 router 自主经 `_share-router/share-runtime` 拉取，client tunnel stop 只做本地清理，router 侧 lease 悬挂到自然过期。
- **实施细节**：
  1. ✅ `src/clients/router/client.rs` 新增 `notify_runtime_refresh()`（POST `v1/shares/runtime-refresh`，签名与既有调用一致），在 share batch-sync / upsert 成功后触发（失败仅记 `last_router_error`，不阻塞本地写路径）；
  2. ✅ `src/api/router.rs` stop 路径追加对 router `/v1/installations/client-tunnel` 的释放调用（PATCH `enabled=false`，按 router 实际 handler 对齐），未注册 identity 时跳过；
  3. ✅ fixture：状态查询、释放、runtime-refresh 的请求 shape 已覆盖；失败降级逻辑保持为记录 `last_router_error`。
- **验收标准**：静态 fixture 通过；真实时效改善归入隧道真实验收观察。
- **工作量**：S–M。**依赖**：无。

### X10 heartbeat 语义决策（记录或接线）

- **状态（2026-07-07）**：**方案 A 已完成**。`/api/router/heartbeat` 现在先对 router 发起已签名的 `v1/shares/pending-edits` 空拉探测；成功后才更新 `last_router_heartbeat_ms` / `client.last_heartbeat_ms` 并清空 `last_router_error`，失败时写入 `last_router_error`、置 `router_registered=false` 并返回 502。合同测试覆盖成功探测与 503 失败降级。
- **现状证据**：server `/api/router/heartbeat`（`src/http.rs:4004`）只更新本地时间戳并置 `router_registered=true`，**不与 router 通信**；router 侧存在 `POST /v1/shares/heartbeat`（`api.rs:268` → `record_share_heartbeat`），desktop 同样未调用。当前 UI 的「heartbeat」按钮语义与用户预期（探活 router）不符。
- **决策项**：
  - **方案 A（推荐，最小改动）**：`/api/router/heartbeat` 改为真实探测——对 router 发一次轻量已鉴权请求（如 `v1/shares/pending-edits` 空拉或 `v1/auth/session/me`），成功才更新时间戳，失败写 `last_router_error`；
  - **方案 B**：接线 router `POST /v1/shares/heartbeat`（需核对 router 侧对 payload/metadata 的要求，两个 client 都未用过该端点，需与 router 维护方确认语义）；
  - **方案 C**：维持现状，把 API/UI 文案改为「标记在线」类本地语义，消除误导。
- **验收标准**：所选方案落地后，heartbeat 结果能真实反映 router 可达性（A/B）或文案不再误导（C）；决策记录在本文档。
- **工作量**：S（A/C）或 M（B）。**依赖**：方案 B 需 router 侧确认。

### X11 品牌图标体积豁免登记

- **状态（2026-07-07）**：**已完成**。`docs/server-desktop-ui-parity-plan.md` 第 8.4 节登记当前实测缺失的 24 个 desktop 图标资产、文件大小和豁免理由；`mcp.svg` 标记为 MCP excluded 功能资产，其他品牌图标维持 `iconInference` 回退并暂不进入 embedded 首包。
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
✅ X2（Ollama reasoning effort clamp 已完成）
✅ X4（owner 验证流，按方案 A 对齐 desktop）‖ ✅ X7 第一批（transform 用例，75% 门禁）→ ✅ X7 第二批（streaming 用例，78% 门禁）
✅ R4-accounts 收敛（X5 硬性前置已满足）→ ✅ X5（Copilot token 交换静态实现）→ X6（Kiro 桥，复用 X5 基建）
  → X8 第一批 ✅ / ✅ X9 / ✅ X10 / ✅ X11（收尾，可穿插并行）
✅ R4 全受管 store 写路径收敛（config/providers/universal_providers/accounts/failover/pricing/usage/shares/ui_settings/sessions/oauth_logins）
✅ X6 第一批（Kiro 协议桥 + 23 个 desktop fixture）
  → 下一轮：X6 第二批（账号凭据/请求接线，仍保持 capability gate）→ X8 后续 CSS 收口 → api/types.rs DTO 就近化 → 整体 review
```

> **与 Phase R 的关系**：Phase R 已关闭（2026-07-07）。X4–X11 文中引用的 `src/http.rs`、`src/core/*` 旧路径按 R2/R3 映射表对应到 `src/api/*`、`src/domain/*`、`src/clients/*`；R4 全受管 store 生产写路径已收敛到 `ServerStateInner` 域方法并纳入静态门禁，X5 accounts 前置已满足且不得回退。剩余结构收口为 `api/types.rs` DTO 就近化，随后续功能/域改动摊销。

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
| 2026-07-07 | X2 完成：吸收 desktop `d7d33e51` 的 Ollama Codex reasoning effort clamp，下一步进入 X4/X7 |
| 2026-07-07 | X7 第一批完成：新增 5 个 transform 用例、覆盖跟踪清单与 75% `--check` 门禁；X4 复核为下一独立功能切片 |
| 2026-07-07 | R4-accounts 完成：state 外 accounts 写路径清零并降 `pub(crate)`，X5 的 accounts 前置解除 |
| 2026-07-07 | X5 静态实现完成：Copilot managed-account 请求时 token 交换、endpoint 发现、per-account 缓存与 forwarder 接线落地；真实 capability 升级仍待外部账号验收 |
| 2026-07-07 | X10 方案 A 完成：router heartbeat 改为已签名 pending-edits 空拉真实探测，失败不再伪造在线状态 |
| 2026-07-07 | X11 完成：登记 24 个品牌图标/MCP excluded 图标体积豁免，保留 `iconInference` 回退 |
| 2026-07-07 | X9 完成：补齐 router runtime-refresh 通知、client-tunnel 远端状态查询与 stop 释放合同；新增 API 合同测试覆盖请求 shape |
| 2026-07-07 | X7 第二批完成：补齐 streaming tool-call 双向映射、stream finish_reason 与 SSE CRLF 多帧 fixture，覆盖门禁提升到 78%/198 |
| 2026-07-07 | X8 第一批完成：机械删除零引用 CSS 过渡层规则，`styles.css` 5817→2660 行，并新增 3000 行静态门禁 |
| 2026-07-07 | X4 静态实现完成：7 个 `email_auth_*` invoke 命令入契约，owner change 走新邮箱验证码 + router change-owner，直接 owner update/transfer 增加 verified target gate，并在 Share 页面接入 owner-change 两步入口；真实发信验收保留为外部任务 |
| 2026-07-07 | R4-pricing 完成：model pricing upsert/delete 写路径收敛到 state 域方法并立即保存，`pricing` 字段降 `pub(crate)`；剩余直接写计数 42→40 |
| 2026-07-07 | R4-universal_providers 完成：universal provider import/upsert/delete 写路径收敛到 state 域方法并立即保存，字段降 `pub(crate)`；剩余直接写计数 40→37 |
| 2026-07-07 | R4-sessions 完成：legacy bearer session clear/push 写路径收敛到 state 域方法，字段降 `pub(crate)`；剩余直接写计数 37→34 |
| 2026-07-07 | R4-oauth_logins 完成：OAuth login start/finish/poll/mark 写路径收敛到 state 域方法，字段降 `pub(crate)`；按当前 grep 复核剩余 32 处（ui_settings 14、providers 8、failover 10） |
| 2026-07-07 | R4-failover 完成：控制面配置/重置写路径和 proxy 熔断热路径写入收敛到 state 域方法，字段降 `pub(crate)`；剩余直接写计数 32→22 |
| 2026-07-07 | R4-providers 完成：provider CRUD/import/sort/universal sync/fetch-model merge 写路径收敛到 state 域方法，字段降 `pub(crate)`；剩余直接写计数 22→14 |
| 2026-07-07 | R4-ui_settings 完成：invoke/settings/proxy app config 写路径收敛到 state 域方法并立即保存，字段降 `pub(crate)`；剩余直接写计数 14→0 |
| 2026-07-07 | R4 完成复核：config/usage 字段降 `pub(crate)`，测试改用 snapshot/replace_config 方法；状态写入静态门禁扩展到全部受管 store 的多行写锁与直接保存调用 |
| 2026-07-07 | X6 第一批完成：新增 server-native Kiro 协议桥模块，移植 desktop 23 个 Claude↔Kiro payload/SSE/usage/prompt-cache fixture；真实转发接线和 capability 升级继续受外部账号验收 gate 约束 |
