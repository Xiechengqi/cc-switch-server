# 代码审计缺口修复计划（Phase X）

> **来源**：2026-07-06 对外部 Provider/协议证据、Server（本仓库 @ `6fce8e4`）和 Router（`/data/projects/cc-switch-router`）的交叉审计。审计**不采信任何既有规划文档的状态标记**，全部结论来自协议证据与实测（`cargo test` 595 passed / typecheck / Router 契约逐条核对）。
> **定位约束**：Server 是独立 token 反代服务端；客户端专属功能（Skill/MCP/OpenClaw/Hermes/OMO/prompts/sessions/deeplink 等）维持排除，本计划不涉及。
> **关联文档**：`assets/contract/web-runtime-contract.json`（Server 功能契约）、`UPSTREAM_IMPORT.md`（外部 Provider 审计台账）。
> **后记（2026-07-22）**：Server 已删除模型成本计算、pricing store 和 provider 成本配置。下文 pricing 条目仅保留当时的审计与重构记录。
> **独立性决策（2026-07-22）**：本文是历史审计记录，不再是外部 UI 吸收清单。Web UI、locale、命令面和页面结构只能按 Server 需求与契约演进，禁止从外部仓库批量复制或覆盖。

## 审计结论摘要（2026-07-06 基线）

| 维度 | 实测结果 |
| --- | --- |
| 命令面 | Server runtime contract 已分类 retained 与 excluded 命令；X4 补入 7 个 email_auth 主线命令 |
| router 契约 | router→client 控制面（`/_ctl/*`、`/_share-router/*`）全部实现；client→router 次要端点已补齐 heartbeat 探测、runtime-refresh、client-tunnel 状态/释放（见 X9/X10） |
| proxy 管线 | 主流组合 native 已落地；Copilot/Kiro/DeepSeekAccount 为 fallback skeleton，Cursor/Bedrock 为 planned；协议回归缺口见 X5–X7 |
| 前端 | Server Web typecheck 曾有 1 个错误（见 X1）；`styles.css` 5817 行过渡层仍被引入（见 X8） |
| 工程 | `cargo test` 595/595 通过（565 lib + 30 集成）；外部 Ollama 协议证据揭示 reasoning clamp 缺口（见 X2） |

---

## 任务状态（2026-07-07 复核更新）

| 任务 | 状态 | 依据 |
| --- | --- | --- |
| X1 typecheck 修复 | **已完成** | 提交 `18cbca5`；typecheck exit 0 复核通过 |
| Phase R 结构重构 | **已完成并关闭** | R1–R7 全部实施，提交 `65721b8`；关闭登记见 `docs/architecture-refactor-plan.md` 第七节 |
| X2 Ollama clamp 协议修复 | **已完成** | `src/proxy/adapters.rs` 针对 Ollama 目标传入 `ReasoningEffortMode::Ollama`；fixture 覆盖 `xhigh→max`、显式关闭→`none`、非 Ollama 透传；`UPSTREAM_IMPORT.md` 已登记 `d7d33e51` |
| X4–X11 | **本地可实施项已完成** | X4/X5/X6/X7/X9/X10/X11 静态实现与 R4 写路径收敛均已落地；X8 第二批 CSS 过渡层削减（5817→1917 行，门禁 2000 行 + 35 个零引用 class 上限）；`api/types.rs` 按域拆分为 `api/types/{common,auth,backup,settings,providers,accounts,usage,shares,router,models}.rs`；capability native 升级、真实发信/OAuth/router 验收仍按外部环境 gate |

## P0 — 阻塞构建 / 门禁失效（应最先完成）

### X1 修复 web typecheck 错误（阻塞前端构建）

- **现状证据**：`npm --prefix web-src run typecheck` exit 2。唯一错误：
  `web-src/src/components/settings/ServerSecuritySettings.tsx(108,13): error TS2322` —— `onClick={onSignOut}` 把 `MouseEvent` 传给了 `(options?: { clearPasswordCache?: boolean }) => void`。
- **实施细节**：第 108 行改为 `onClick={() => onSignOut()}`（不透传事件对象；保持第 47 行 `onSignOut({ clearPasswordCache: false })` 调用方语义不变）。
- **验收标准**：`npm --prefix web-src run typecheck` exit 0；`npm --prefix web-src run build` exit 0；`scripts/static-checks.sh` 通过。
- **工作量**：S（分钟级）。**依赖**：无。

### X2 按协议证据修复 Ollama Codex reasoning effort clamp

- **现状证据**：外部 commit `d7d33e51`（2026-07-04）记录 Ollama reasoning effort 枚举**拒绝 `xhigh`、接受 `max`**。Server `src/proxy/transforms.rs` 的 effort 映射路径（`:172`、`:244`、`:2228-2247`）当时为纯透传，会把 `xhigh` 原样转发给 Ollama 导致 4xx。
- **实施细节**：
  1. 在 Server 的 Codex Chat 请求归一路径（`transforms.rs` reasoning effort 写出处）增加平台规则：目标 Provider 为 `OllamaCloud`（或 base URL 含 `ollama`）时，将 `xhigh → max`，并保持 Server thinking 契约稳定。
  2. 平台判定接线点放在 adapter 侧（`src/proxy/adapters.rs` Codex→Ollama 分支）传入 transforms，避免 transforms 直接感知 provider 类型字符串。
  3. 根据协议边界新增 Server fixture（覆盖：别名模型 + `xhigh` 输入 → `max` 输出；非 Ollama 目标不 clamp）。
  4. `UPSTREAM_IMPORT.md` 登记 `d7d33e51` 为「已按证据独立实现」。
- **验收标准**：新 fixture 通过；`cargo test` 全绿；台账登记完成。
- **工作量**：S–M。**依赖**：无。

## P1 — 功能/语义缺口（server 内可静态完成，capability 升级另有真实验收 gate）

### X4 Share owner 变更验证码流（决策 + 实现）

- **状态（2026-07-07）**：**静态实现已完成**。`src/clients/router/email_auth.rs` 新增 owner-change router client（新邮箱验证码校验后，签名调用 `/v1/installations/change-owner-email`）；`/web-api/invoke` 契约补入 7 个 `email_auth_*` 主线命令；`email_auth_change_owner_email` 成功后同步更新 `server.json` owner email、批量更新本地旧 owner shares 并触发 share sync/event；旧 `update_share_owner_email` / `transfer_share_owner` 兼容入口增加 verified target gate，未验证的新 owner 返回 4xx；Share 页面接入 `ShareOwnerChangeEmailDialog` 两步入口。真实发信、router 线上限流和端到端邮箱收码仍归入真实环境验收。
- **现状证据**：Router owner 变更协议要求向新 owner 发码、验证，再调用 `/v1/installations/change-owner-email`。Server 当时的 `web_transfer_share_owner` / `web_update_share_owner_email`（`src/http.rs:6378/6399`）只要求 admin 会话、ACL 和格式校验，未满足邮箱所有权验证要求。
- **决策项（先于实现）**：单管理员部署下当前约束是否足够？两个方案：
  - **方案 A（推荐）**：owner 变更前必须向新 owner 邮箱发验证码并校验。
  - **方案 B**：维持现状，在契约 notes 与文档中显式记录「server 信任 admin 会话，owner 变更不发码」为有意分歧。
- **实施细节（方案 A）**：
  1. ✅ `src/clients/router/email_auth.rs` 复用既有 router 签名调用基建，新增 `change_owner_email(old_email, new_email, access_token)`，对接 router `/v1/installations/change-owner-email`；发码复用 `/v1/auth/email/request-code`，验证码校验复用 `/v1/client-web/auth/email/verify-code`；
  2. ✅ `transfer_owner_email` / `update_owner_email` 增加 verified target gate：目标邮箱必须已是当前 configured owner 且本地 `email-auth.json` 登录状态匹配；新 owner 变更必须走 `email_auth_change_owner_email`；
  3. ✅ `/web-api/invoke` 契约新增 7 个 `email_auth_*` 命令（`assets/contract/web-runtime-contract.json` + dispatcher 分支 + 审计脚本双向校验）；
  4. ✅ 前端 `ShareOwnerChangeEmailDialog` 已接入 Share 页面，复用 `web-src/src/lib/api/emailAuth.ts` 的发码/验证封装。
- **验收标准**：未验证时 transfer/update 返回 4xx 且错误信息可诊断；验证通过后转移成功并触发 share sync；契约审计通过。真实验证码错误/过期/重复提交需 router 发信环境，归入真实环境验收。
- **工作量**：M。**依赖**：无（真实发信路径归入真实环境验收）。

### X5 Copilot 请求时 internal token 交换与端点发现

- **状态（2026-07-07）**：**静态实现已完成**。`src/clients/oauth/copilot_device.rs` 提供 Copilot internal token 交换与 `/copilot_internal/user` endpoint 发现；`ServerStateInner::prepare_copilot_upstream_auth` 增加 per-account 内存缓存（过期前 60 秒刷新），GHES 分支直接使用 GitHub token 并回退 `copilot-api.{domain}`；`forwarder` 在 GitHub Copilot managed-account 路径覆盖 `Authorization` 与 API endpoint，Provider 静态 secret 继续旁路。补充测试覆盖交换请求 shape、endpoint 发现/回退、缓存种子、静态 secret 旁路、Codex/Gemini Copilot 分类和 API 合同级 endpoint/header 覆盖。**capability 仍不得升级 native**，需真实 device-flow 账号 non-stream/stream + usage 验收后另行调整。
- **现状证据**：Copilot 协议要求用 GitHub token 换取短时效 internal token，并经 `/copilot_internal/user` 发现每账号 API endpoint（含 GHES 分支）。Server 当时的 Copilot 分支只把 Provider 配置里的静态 bearer 原样转发，没有请求时交换、端点发现或续期。
- **实施细节**：
  1. ✅ 新增 client 侧 `fetch_copilot_internal_token` / `fetch_copilot_api_endpoint`，按 Copilot/GHES 协议构造 API 域名；
  2. ✅ `ServerState` 增加 per-account internal token 缓存（过期前 60s 视为失效并重换）；
  3. ✅ forwarder Copilot 分支改为：绑定 managed account（`accounts.json` 中 device flow 导入的 GitHub token）时走交换 + endpoint 发现；provider 配置显式给静态 token 时保留现行为（向后兼容）；
  4. ✅ 交换请求走 server 的代理感知 `reqwest::Client`；失败返回结构化 `upstream_error`；
  5. ✅ fixture：交换请求 shape（URL/header）、缓存种子、endpoint 发现/回退、GHES fallback 规则、静态 token 旁路、API 合同级 endpoint/header 覆盖。
- **验收标准**：静态 fixture 全绿；`copilot_model_map` / `copilot_optimizer` 现有测试不回归。**capability 升级 gate**：真实 device flow 账号 non-stream/stream + usage 口径验收后才把 Copilot×3 从 fallback 升级（不在本计划内）。
- **工作量**：L。**依赖**：R4 的 accounts 域收敛已完成（state 外零直接写，字段已降 `pub(crate)`；X5 新增后台并发写路径必须复用 state 域方法）；真实验收依赖外部凭据。

### X6 Kiro 转发桥实现

- **状态（2026-07-07）**：**第一/二/三批静态实现已完成**。新增 `src/proxy/kiro.rs`，按 Kiro/CodeWhisperer 协议独立实现 Claude Messages → Kiro conversation payload、Kiro event stream → Claude SSE/JSON、tool leak rescue、thinking/redacted thinking、usage/prompt-cache 计算，并加入 23 个 Server fixture。第二批补齐 Server `Account` → `KiroAccountData` → `KiroPreparedRequest`（URL/header/body/tool map）转换。第三批在 forwarder 中为 Claude + `KiroOAuth` 增加 managed-account 专用发送路径；API 合同测试以 mock CodeWhisperer 覆盖非流式与流式。Codex/Gemini Kiro 入口、账号 refresh 完整真实验收和 capability 矩阵升级仍保持 gate。
- **现状证据**：Kiro 使用 CodeWhisperer conversation API，而 Server 三个 app 的 Kiro 组合当时均为 `fallback("*_kiro_skeleton")`；账号 device flow 导入已有，但转发路径缺失。
- **实施细节**：
  1. 新增 `src/proxy/kiro.rs`，按协议实现 conversation payload、profile ARN、机器指纹头、响应/事件流解析和 Claude Messages 双向转换；
  2. Codex/Gemini 入口复用既有 `transforms.rs` 的 Claude 中间表示（先转 Claude Messages 再进 Kiro 桥）；
  3. token 续期沿用 `kiro_device.rs` 的 refresh 路径；请求走代理感知 client；
  4. 为协议构造和解析边界新增 Server fixture；
  5. adapter 从 `fallback` 改为 `planned`（静态接线完成）——**native 升级 gate 同 X5**（真实 AWS Builder ID 账号验收）。
- **验收标准**：fixture 覆盖请求构造、事件流→Claude SSE 转换、错误路径；`cargo test` 全绿；capability 矩阵与 Web 展示同步更新。
- **工作量**：XL。**依赖**：建议在 X5 之后（同为「managed account → 请求时凭据」模式，可复用缓存基建）。

### X7 transform/streaming 黄金用例补齐

- **状态（2026-07-07）**：**第二批已完成**。第一批覆盖 request/response transform 与 stop_reason 基础矩阵；第二批补齐 OpenAI Chat/Responses/Gemini/Anthropic streaming tool-call delta 双向映射、关键 stream finish_reason 映射、SSE CRLF 多帧 chunk 解析；`audit-transform-coverage.mjs` 默认门禁从 75%/190 提升到 78%/198。半帧跨 chunk 重组仍需 forwarder/adapter 有状态缓冲设计，继续保留为后续架构项。
- **现状证据**：Server 当时的 in-module 覆盖只有 `transforms.rs` 11 + `streaming.rs` 11 + `adapters.rs` 64（另有 fixture 宏批量用例）。**流式 tool-call 增量重组、parallel tool calls、stop_reason 映射矩阵、图片块、SSE 边界切割**等高价值协议边界覆盖明显偏薄。
- **实施细节**：
  1. 按 Server 支持的协议矩阵逐个映射到 `transforms.rs`/`streaming.rs`/`adapters.rs` 的对应入口，输出「已覆盖 / 需实现 / 不适用」三列清单；
  2. 优先新增四类 Server fixture：流式 tool-call 增量重组（含 parallel）、跨协议 stop_reason/finish_reason 映射全矩阵、SSE chunk 边界（半帧/多帧/CRLF）、图片与多模态块转换；
  3. 用 server 既有 streaming fixture 宏承载，避免逐个手写样板；
  4. 更新 `scripts/audit/audit-transform-coverage.mjs` 的基线与目标值（当前目标 70%，建议提升至 85–90%）并保持纳入 `static-checks.sh`。
- **验收标准**：审计脚本新目标达成；`cargo test` 全绿；清单三列可追踪。
- **工作量**：L（可按 transform → streaming 分两批交付）。**依赖**：X2 先行（避免同文件冲突）。

---

## P2 — 收尾与决策项

### X8 `styles.css` 过渡层削减收口

- **状态（2026-07-07）**：**第一批已完成**。新增 `scripts/audit/audit-css-transition-layer.mjs` 并纳入 `scripts/static-checks.sh`；按源码 class 引用面机械删除零引用规则和零引用 selector 分支，`web-src/src/styles.css` 从 5817 行降到 2660 行，构建 CSS 资产约 173KB 降到约 126KB；默认门禁 `CC_SWITCH_STYLES_MAX_LINES=3000` 防止过渡层回涨。剩余 29 个零引用 class 多为状态/派生选择器和混合规则残留，后续需结合页面截图或组件内迁移继续收口，目标 <500 行暂未达到。
- **现状证据**：`web-src/src/styles.css` 5817 行 Server 过渡样式仍被 `main.tsx:13` 引入，存在两套样式层并存；需要按当前 Server 组件引用面收敛。
- **实施细节**：
  1. ✅ 写入审计脚本：提取 `styles.css` 全部 class 名，grep `web-src/src` 统计仍被引用的 class 集合；
  2. ✅ 第一批删除零引用 block / selector 分支；仍被引用或混合派生的规则保留，避免无截图情况下误删；
  3. ✅ `static-checks.sh` 增加行数上限门禁防回涨；目标态仍是 `styles.css` 只剩 login/setup 与 server-only 组件的最小集（建议 <500 行），后续需按页面继续迁移。
- **验收标准**：typecheck/build 通过；人工 checklist 抽查 providers/shares/settings 三页无样式回归；行数门禁生效。
- **工作量**：M（随页面分批）。**依赖**：无；与 X1 后的任意时间并行。

### X9 router 次要契约补齐：runtime-refresh 通知 + client-tunnel 状态/释放

- **状态（2026-07-10）**：**已完成并收敛为自动同步**。`src/clients/router/client.rs` 提供 share op 传输、runtime refresh、client tunnel 查询和释放；share 变更会自动增量同步，client 启动或重新注册后会自动 reconcile。旧 `/api/router/batch-sync` 人工入口已删除；诊断页只展示同步结果和错误。
- **现状证据**：Server 已实现 Router runtime-refresh 与 client-tunnel 状态/释放协议；Share 增量变更自动推送，启动或注册后的安装级 reconcile 使用同一批量传输协议，但不再暴露人工触发 API。
- **实施细节**：
  1. ✅ `src/clients/router/client.rs` 新增 `notify_runtime_refresh()`（POST `v1/shares/runtime-refresh`，签名与既有调用一致），在 share batch-sync / upsert 成功后触发（失败仅记 `last_router_error`，不阻塞本地写路径）；
  2. ✅ `src/api/router.rs` stop 路径追加对 router `/v1/installations/client-tunnel` 的释放调用（PATCH `enabled=false`，按 router 实际 handler 对齐），未注册 identity 时跳过；
  3. ✅ fixture：状态查询、释放、runtime-refresh 的请求 shape 已覆盖；失败降级逻辑保持为记录 `last_router_error`。
- **验收标准**：静态 fixture 通过；真实时效改善归入隧道真实验收观察。
- **工作量**：S–M。**依赖**：无。

### X10 heartbeat 语义决策（记录或接线）

- **状态（2026-07-07）**：**方案 A 已完成**。`/api/router/heartbeat` 现在先对 router 发起已签名的 `v1/shares/pending-edits` 空拉探测；成功后才更新 `last_router_heartbeat_ms` / `client.last_heartbeat_ms` 并清空 `last_router_error`，失败时写入 `last_router_error`、置 `router_registered=false` 并返回 502。合同测试覆盖成功探测与 503 失败降级。
- **现状证据**：Server `/api/router/heartbeat`（`src/http.rs:4004`）当时只更新本地时间戳并置 `router_registered=true`，**不与 Router 通信**；Router 侧存在 `POST /v1/shares/heartbeat`（`api.rs:268` → `record_share_heartbeat`），但没有已验证的 Client 调用契约。当前 UI 的「heartbeat」按钮语义与用户预期（探活 Router）不符。
- **决策项**：
  - **方案 A（推荐，最小改动）**：`/api/router/heartbeat` 改为真实探测——对 router 发一次轻量已鉴权请求（如 `v1/shares/pending-edits` 空拉或 `v1/auth/session/me`），成功才更新时间戳，失败写 `last_router_error`；
  - **方案 B**：接线 router `POST /v1/shares/heartbeat`（需核对 router 侧对 payload/metadata 的要求，两个 client 都未用过该端点，需与 router 维护方确认语义）；
  - **方案 C**：维持现状，把 API/UI 文案改为「标记在线」类本地语义，消除误导。
- **验收标准**：所选方案落地后，heartbeat 结果能真实反映 router 可达性（A/B）或文案不再误导（C）；决策记录在本文档。
- **工作量**：S（A/C）或 M（B）。**依赖**：方案 B 需 router 侧确认。

### X11 Server 品牌图标体积决策

- **状态（2026-07-22）**：**已按 Server 产品边界关闭**。只打包 Server 实际可创建的 Provider 所需资产；缺少专用资产时使用 `iconInference` 回退，客户端专属功能图标不进入 embedded 首包。
- **验收标准**：Provider registry 中每个可见项都有稳定图标或显式回退；`audit-web-dist-size.mjs` 门禁维持通过。

---

## 明确不在本计划内（边界）

1. **真实环境验收**：router 隧道实连、direct share 端到端、market 调度、OAuth 订阅账号真实转发、Cursor（planned，静态 driver 已接线）与 Bedrock（planned，SigV4 合同已生成）的真实验收——全部依赖外部凭据/环境，沿既有 runbook 推进；X5/X6 的 capability 从 fallback/planned 升级 native 均以真实验收为 gate。
2. **`set_window_theme`**：契约中唯一 `implemented=false` 命令，浏览器由 ThemeProvider 覆盖，维持现状。
3. **`v1/shares/sync`（单条同步）**：server 用 batch-sync 覆盖同一语义，不补。
4. **DeepSeek 账密登录 / 请求时 PoW**：账密登录维持排除（import-only 不变）；请求时 PoW（`DeepSeekHashV1`）已在 Server 独立实现并用于 `DeepSeekAccount` 转发。Claude + `DeepSeekAccount` 协议桥已静态接线（Phase Y3，`planned`）；Codex/Gemini 路径仍为 skeleton，按 X6.2 Claude IR 复用路线推进。

## 执行顺序

```
✅ X1（已完成）
✅ Phase R 结构重构（R1–R7 已完成并关闭，提交 65721b8）
✅ X2（Ollama reasoning effort clamp 已完成）
✅ X4（owner 验证流，按方案 A）‖ ✅ X7 第一批（transform 用例，75% 门禁）→ ✅ X7 第二批（streaming 用例，78% 门禁）
✅ R4-accounts 收敛（X5 硬性前置已满足）→ ✅ X5（Copilot token 交换静态实现）→ X6（Kiro 桥，复用 X5 基建）
  → X8 第一批 ✅ / ✅ X9 / ✅ X10 / ✅ X11（收尾，可穿插并行）
✅ R4 全受管 store 写路径收敛（config/providers/universal_providers/accounts/failover/pricing/usage/shares/ui_settings/sessions/oauth_logins）
✅ X6 第一批（Kiro 协议桥 + 23 个 Server fixture）
✅ X6 第二批（Account→Kiro request plan + 2 个 server fixture）
✅ X6 第三批（Claude forwarder 发送/响应桥接；capability 仍保持 gate）
  → ✅ X8 第二批 CSS 收口（1917 行 / 30 个零引用 class 残留）
  → ✅ api/types DTO 域拆分（`src/api/types/` 子模块）
  → 后续仅剩外部环境 gate 验收与可选 X7 第三批（85%+ 覆盖 / 半帧重组架构项）
```

> **与 Phase R 的关系**：Phase R 已关闭（2026-07-07）。X4–X11 文中引用的 `src/http.rs`、`src/core/*` 旧路径按 R2/R3 映射表对应到 `src/api/*`、`src/domain/*`、`src/clients/*`；R4 全受管 store 生产写路径已收敛到 `ServerStateInner` 域方法并纳入静态门禁，X5 accounts 前置已满足且不得回退。剩余结构收口为 `api/types.rs` DTO 就近化，随后续功能/域改动摊销。

## 验证基线（每个任务完成前必须通过）

```bash
cargo fmt --check
cargo check --all-targets   # 直接取退出码，禁止管道吞码
cargo clippy --all-targets -- -D warnings
cargo test
npm --prefix web-src run typecheck
npm --prefix web-src run build
scripts/static-checks.sh
node scripts/audit/audit-server-product-boundary.mjs
```

## 变更记录

| 日期 | 变更 |
| --- | --- |
| 2026-07-06 | 初版：基于三方代码交叉审计（不采信文档状态）建立 X1–X11 任务与边界 |
| 2026-07-07 | 状态更新：X1 完成、Phase R 完成并关闭（`65721b8`）；X2 为 P0 唯一剩余；X5 增加 R4-accounts 收敛硬性前置 |
| 2026-07-07 | X2 完成：按 Ollama 协议证据实现 Codex reasoning effort clamp，下一步进入 X4/X7 |
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
| 2026-07-07 | X6 第一批完成：新增 Server-native Kiro 协议桥模块与 23 个 Claude↔Kiro payload/SSE/usage/prompt-cache fixture；真实转发接线和 capability 升级继续受外部账号验收 gate 约束 |
| 2026-07-22 | 删除外部 UI 同步任务、脚本和漂移门禁；Web 改由 Server runtime contract、产品边界审计与人工验收独立约束 |
| 2026-07-07 | X6 第二批完成：补齐 server Account 到 Kiro request plan 的转换，覆盖 profile/raw 字段解析、CodeWhisperer URL/header/body shape；真实发送与 capability 升级仍保留 gate |
| 2026-07-07 | X6 第三批完成：forwarder 接入 Claude + KiroOAuth managed-account 发送路径，mock CodeWhisperer 合同测试覆盖非流式 JSON 与流式 SSE 响应桥接；Codex/Gemini Kiro 和 capability native 升级继续保留真实账号 gate |
| 2026-07-07 | X8 第二批完成：`styles.css` 5817→1917 行；审计脚本改为 TSX-only 引用检测并增加零引用 class 门禁（35 上限） |
| 2026-07-07 | R4 DTO 收口：`api/types.rs` 拆为 `api/types/` 域子模块（10 文件，mod.rs 仅 re-export）；`cargo test` 533 lib + 30 集成通过 |
| 2026-07-07 | **Phase Y 关闭（本地项）**：Y0 能力矩阵诚实化；Y1 transform 门禁 85%（222 tests）；Y2 `SseLineBuffer` 半帧缓冲；Y3 DeepSeek Account Claude 协议桥 + forwarder；Y4 Bedrock converse fixture 补强；Y6 `codex_banked_reset_status` 只读 invoke；Y5 `styles.css` 核心层 869 行并拆分 `web-src/src/styles/{providers,auth-accounts,usage,modals,universal}.css`；真实环境验收仍 gate |
| 2026-07-07 | Review 收尾：static-checks 排除 `web-dist` 尾随空白误报；删除 `clients/deepseek/sse.rs` 死代码（SSE 解析保留于 `proxy/deepseek.rs`，-3 测试）；验证基线更新为 595（565 lib + 30 集成）；体积预算文档对齐 4.5MB 门禁；R4 正式完结登记 |
| 2026-07-07 | clippy 25 条 warning 清零；`cargo clippy --all-targets -- -D warnings` 纳入 `static-checks.sh`；R8 proxy 体积拆分挂起登记 |

---

## Phase Y — 本地收尾（2026-07-07 关闭）

> **范围**：在 Phase X 本地项完成后，继续补齐 transform 覆盖、SSE 缓冲、DeepSeek/Bedrock 静态接线、CSS 过渡层拆分与 invoke 只读快照；**不包含**真实 router/OAuth/上游转发验收。

| 任务 | 状态 | 依据 |
| --- | --- | --- |
| Y0 能力矩阵诚实化 | **已完成** | Kiro/Bedrock/Copilot/DeepSeek `provider_note` 与 `planned`/`fallback` 对齐真实接线 |
| Y1 X7 第三批（85% 门禁） | **已完成** | `audit-transform-coverage.mjs` 默认 0.85 / 216+；server 合计 222 tests |
| Y2 SSE 半帧缓冲 | **已完成** | `streaming::SseLineBuffer` + DeepSeek 流式路径接入 |
| Y3 DeepSeek Account 协议桥 | **已完成** | `clients/deepseek/{client,pow}`、`proxy/deepseek.rs`（内嵌 SSE 解析）、`forward_claude_deepseek`、mock 上游 fixture 测试；Claude `planned`，Codex/Gemini skeleton |
| Y4 Bedrock fixture | **已完成** | tool_use / inferenceConfig / session token SigV4 合同测试 |
| Y5 Providers CSS 过渡层 | **已完成** | `styles.css` 869 行（≤1200 门禁）；Providers/Auth/Usage 样式拆至 `web-src/src/styles/*.css`；零引用 class 机械删除 |
| Y6 banked reset invoke | **已完成** | `codex_banked_reset_status` 读取导入快照；invite/consume 仍 `not_implemented` |
| 真实环境验收 | **保留 gate** | Copilot/Kiro/DeepSeek/Bedrock/Cursor native 升级与 E2E 转发仍依赖外部凭据 |

### 仍待外部环境 gate

1. DeepSeek / Kiro / Copilot / Bedrock 真实账号 non-stream + stream + usage 验收
2. Codex banked reset invite/consume live API
3. Providers 页人工 UI 截图回归（见 `docs/manual-ui-checklist.md`）
