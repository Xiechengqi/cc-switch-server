# 历史：三方 Token 路由交易系统现状审计与系统规整计划

> **状态：已废止（historical / superseded），不得作为当前架构、实施计划或发布依据。**
>
> 审计日期：2026-08-18
>
> 适用仓库：`cc-switch-server`、`cc-switch-router`、`cc-switch-market`
>
> 本文仅保留旧三方架构审计的历史证据。文中关于独立 `cc-switch-market`、三方运行拓扑、Market 资金风险和对应路线图的“当前”表述均已失效，不得据此恢复旧 Token Market。
>
> 当前产品边界和实施记录以 [`docs/token-market-decoupling-plan.md`](token-market-decoupling-plan.md) 为准；Server 的现行开发约束以仓库根目录 `AGENTS.md` 为准。

## 1. 文档定位与阅读规则

`cc-switch-server` 是独立的 Server 产品。它不是 desktop `cc-switch` 的整仓 fork，外部仓库只能作为 Provider 类型、协议行为和缺陷证据来源。本文的代码规整计划必须遵守仓库根目录 `AGENTS.md` 的边界、依赖方向、状态写入和 UI 契约约束。

本文优先级约定：

- **P0-条件**：在特定部署条件成立时会造成管理员接管或资金直接损失；满足条件即阻断发布。
- **P1**：高影响安全、资金、数据一致性或升级风险，应在结构重构前处理或明确接受风险。
- **P2**：韧性、可观测性、性能、文档和维护性问题；不能因为是 P2 就从计划中删除。

“事实”与“判断”分开记录：代码路径和提交状态是事实；风险等级会注明依赖的部署假设；需要改变产品信任模型的事项不在审计阶段擅自修改。

## 2. 审计基线与已验证范围

### 2.1 三仓库提交基线

| 仓库 | 系统角色 | 审计时 committed baseline | 审计时工作树 | 说明 |
| --- | --- | --- | --- | --- |
| `/data/projects/cc-switch-server` | Router 的 Client installation / token server | `2c2caa9` | 代码树干净；本次新增本文档及用户已有 Web 改动不属于该 baseline | 结论以该提交的实现为准 |
| `/data/projects/cc-switch-router` | 公网 Router、隧道、边缘授权 | `47b4374` | 有大量未提交 Telegram/notification/schema 改动 | dirty diff 不归因于 committed baseline；本审计中的 ingress 结论以 committed code 为准 |
| `/data/projects/cc-switch-market` | 独立计费/交易 Market | `2530348` | 仅 `src/api_keys.rs` 有未提交改动 | capability 过滤的变更单独记录，不当作已发布行为 |

### 2.2 已通过的本地/静态检查

- Server provider coverage audit：通过。
- Server UI provider matrix audit：通过。
- Server product-boundary audit：通过。
- 三仓库 `cargo check --all-targets`：通过。
- Upstream provider baseline：16 个权威 Provider type；Server runtime 22 个 type，其中 6 个为 Server-only compatibility type。Claude/Codex/Gemini preset 数量分别为 15/7/4，universal recipes 为 2。
- Provider 覆盖的权威快照为 `assets/contract/upstream-provider-source-baseline.json`，对应 upstream commit `b1dee0153da94316fb50416c679a11f74cc66f14`。

这些结果证明静态覆盖和编译边界基本成立，不证明真实 OAuth、真实 Router/Market、上游账号或支付 webhook 已验收。

### 2.3 当前验证红灯

1. Kiro 稳定错误测试在默认 Rust 测试栈上 `stack overflow`/`SIGABRT`；设置 `RUST_MIN_STACK=67108864` 后该测试通过。不能把“增大测试栈”当成根因修复。
2. `api::grok_catalog_provider_tests::auxiliary_inference_routes_reject_disabled_share_surfaces` 失败：fixture 中 disabled share 没有 binding，触发“Share must have between one and three bindings”。这是当前 fixture/契约不一致的证据，不应通过放宽运行时 binding 不变量来掩盖。
3. `scripts/smoke/oauth-readiness-check.sh` 仍调用 `core::account_managers::`、`core::accounts::`、`core::oauth_clients::` 旧过滤器；当前路径已迁移，命令会显示 `running 0 tests` 但脚本仍可 exit 0。adapter 部分的 104 个测试通过，真实 OAuth 输入缺失并产生 warnings；该脚本目前不能作为“OAuth 测试通过”门禁。

## 3. 三方角色、数据面与交易面

### 3.1 “Client”一词的两个含义

在 Router 协议中，`cc-switch-server` 进程是 **Client installation**：它注册 installation identity、建立 SSH reverse tunnel，并接受 Router 签名的 ingress。最终使用 Claude/Codex/Gemini CLI 的人或程序是外部 consumer，不是该协议意义上的 Client installation。Market 中的 client/API key 又是计费主体。后续文档必须分别使用 `consumer`、`Router Client installation`、`Market user/API key`，避免把三者混称为 client。

### 3.2 最小数据流

```text
Claude/Codex/Gemini CLI (consumer)
        │
        ├── 直接使用 Router Share URL
        │
        └── 使用 Market API URL / API key
                    │
                    ▼
        cc-switch-router（公网入口、边缘鉴权、Share entitlement、并发）
                    │  SSH reverse tunnel + HMAC ingress v2
                    ▼
        cc-switch-server（Client installation）
          Share binding → Provider → bound Account/OAuth credential
                    │
                    ▼
             上游模型服务
```

### 3.3 交易/结算时序

```text
Market API key/session
  → 价格解析、余额检查、预授权 user_reserved
  → 选择 Router Share
  → Router entitlement / app / 并发 / 在线状态校验
  → Router 签发 ingress v2（含 share、installation、request binding）
  → Server 校验 Router、installation、Share、Provider、Account
  → Server 转发上游并记录 usage/stream lifecycle
  → Market 根据 usage 状态结算 user_reserved/user_cash、佣金、provider payable
  → Router/Market 同步 request log、库存和风险状态
```

必须保持的权威边界：

- Server 是 Provider/Account 凭据、协议适配、Share binding、反代热路径和 Server usage 的权威。
- Router 是公网路由、SSH tunnel、ingress 签名、用户/API token、Share entitlement、边缘并发与可达性的权威。
- 独立 Market 是用户余额、预授权、价格、usage 解析、ledger、provider earnings/payout 的权威。
- Router 内建 Share Market 和 Client Market 是 Router 的目录/授权能力，不等同于独立 Market 的资金账本。
- Market 不应保存 Server provider token 或 share token 明文；跨系统同步应使用 Share descriptor、installation、usage 和 grant 状态。

### 3.4 主要信任边界

1. Internet → Router：完全不可信，Router 先做用户/API key、Share entitlement、app 和并发检查。
2. Router → Server：通过 tunnel、Router identity、installation identity、control secret 和 ingress v2 建立信任；Server 不能只信任普通注入 header。
3. Server → Provider：Provider credential 是最高敏感数据，必须留在 Server 的 account/provider 存储和热路径内。
4. Market → Router：Market 的库存/价格决策不能替代 Router 的实时 entitlement；同步延迟和撤销语义必须写入契约。
5. Admin/owner → Web terminal：这是潜在的主机级运维权限，不应被普通“管理面”标签掩盖。

## 4. `cc-switch-server` 当前整体架构

### 4.1 模块分层

```text
src/api/       HTTP 路由、认证、控制面、Web invoke、契约端点
src/domain/    Provider、Account、Share、Usage、Settings、Web auth 等领域规则
src/clients/   OAuth、Quota、Router、Tunnel 等出站客户端
src/infra/     storage、backup、credentials、HTTP、time 等基础设施
src/proxy/     ingress 后的推理热路径、协议转换、Provider adapter、stream
src/state.rs   全局状态、跨域编排、worker 生命周期和持久化协调
```

依赖方向已经有产品硬约束：`domain` 不依赖 `api`、`clients`、`proxy`；`proxy` 不依赖 `api/http` 或 `clients`；需要出站 OAuth/Router 客户端的热路径通过 `state.rs` 或控制面编排方法完成。后续拆分必须继续保持该方向，不能为了“方便调用”把依赖倒灌回去。

### 4.2 推理请求热路径

1. Axum 路由进入 ingress middleware；带 Router ingress 的推理请求先验签。
2. Server 校验 Router identity、installation identity、protocol epoch、时间窗口、method/path/query/body digest 和 replay。
3. `require_router_share_ingress` 要求推理请求带 `share_id`；Server 依据 Share binding 选择 Provider/Account，不按占用或错误跨账号漂移。
4. `proxy/forwarder.rs` 负责请求解析、协议/Provider adapter、OAuth refresh 协调、上游请求、流式提交边界、重试/错误语义、usage 和 audit 记录。
5. `transforms.rs`、`adapters.rs` 和 provider-specific 子模块承担跨 Claude/Codex/Gemini/OpenAI-compatible/Gemini-native 协议转换。
6. 结果写入 usage、share request log、quota/health 信号，并由后台同步给 Router/Market。

当前最大结构事实是文件和职责过度集中：`src/proxy/forwarder.rs` 约 33.6k 行，`src/state.rs` 约 29.5k 行，`src/proxy/transforms.rs` 约 9.8k 行，`src/proxy/adapters.rs` 约 8.4k 行，`src/api/mod.rs` 约 4.8k 行。它们不是简单的“文件太长”，而是把 stream lifecycle、状态编排、协议转换和控制面契约绑在同一变更半径内。

### 4.3 `ServerStateInner` 与启动生命周期

`ServerStateInner` 同时持有 JSON store 与锁、OAuth refresh 协调、Router/tunnel 生命周期、quota/health worker、backup/audit、upgrade/restart、Share 编排、replay/cache/concurrency、terminal 和外部 HTTP client，是明显的 capability God Object。后续不应按行数机械切文件，而应先按 capability 和事务边界建立服务对象，再由 `state.rs` 保留薄 façade。

启动顺序大致为：

```text
metrics
→ load state
→ restore tunnels
→ public IP discovery
→ installation heartbeat
→ audit uploader / share log sync
→ periodic backup / share sync retry
→ auto upgrade
→ status/quota/version workers
→ share edit listener
→ HTTP serve
```

这意味着启动阶段已经包含网络、副作用和多个后台 worker；需要在文档中明确 `ready` 与 `healthy` 的含义，并让单个外部依赖失败不会把所有控制面误报为不可用。

### 4.4 持久化现状

当前仍是多文件存储，而不是已经完成的统一 DB 迁移：

```text
server.json                 password hash、owner、Router、tunnel、request limits
providers.json              Claude/Codex/Gemini Provider 配置
accounts.json + accounts.key 账号和加密根密钥
shares.json                 Share descriptor/binding
usage/                      请求明细、journal、rollup
tunnels.json                tunnel 状态
email-auth.json             邮箱认证状态
provider-health.json        Provider health
grok-media-tasks.json       媒体任务
ui-settings.json            UI 设置
web-auth-sessions.json      Web session
router-control.sqlite       Router 控制/同步状态
image-capabilities/         图片 capability 文件
OAuth pending/recovery/quarantine、upgrade/restart、audit/log spool
```

`server.json`、`providers.json` 等 server-native 文件存在，不等于 SQLite 兼容、旧 cc-switch DB 读取、全量迁移、跨 store snapshot 已完成。新代码仍必须通过 `ServerStateInner` 域方法写状态；跨存储写操作按 `config → providers → accounts → usage → shares → ui_settings → sessions → oauth_logins` 获取锁；shares 只能使用既有的 `mutate_*`/`replace_shares`/`validate_share_invocation` 路径。

### 4.5 端点和可达性矩阵

| 面 | 典型路径 | 当前可达性/授权 | 规整重点 |
| --- | --- | --- | --- |
| 健康与诊断 | `/health`、`/ready`、`/metrics`、`/version` | 基本公开 | 生产暴露矩阵、敏感字段和反代边界要写清 |
| 首次 setup | `/api/setup/*`、`POST /api/setup/bootstrap` | setup 未完成时 handler 不要求 session | 首次接管策略必须有明确威胁模型 |
| 登录入口 | `/api/auth/login`、邮箱 code、`/web-api/auth/*` | 登录/setup/refresh/OAuth callback 有公开子集 | 明确 direct、Router tunnel、local-only 的差异 |
| 普通控制面 | `/api/*`、普通 `/web-api/*` | handler 内部校验 bearer/session | 统一 endpoint/auth contract |
| 推理数据面 | Router Share ingress | `verify_router_ingress` → `require_router_share_ingress` | 不允许把 Router header 当公网信任输入 |
| Client Web tunnel | 静态资源与 `/web-api/*` | Router 先做 owner/admin/API token，再签 ingress；Server 仍复验 | `/api/*`、`/v1/*` 不经该 tunnel |
| 内部控制 | `/_ctl/*`、`/_share-router/*` | `/_ctl/*` 公网 404；Share router 路径要求控制签名 | 保持 nonce/timestamp/HMAC 和 header stripping |

systemd 示例直接监听 `0.0.0.0:15721`，本地默认是 `127.0.0.1:15721`；默认配置目录也分别是 `$HOME/.cc-switch-server` 与 `/var/lib/cc-switch-server`。这些差异必须和 CORS、反向代理、备份、setup 暴露一起描述。

### 4.6 Router ingress v2

Server 与当前 Router 实现使用 `SIGNATURE_VERSION = 2` 和签名域 `cc-switch-router-ingress-v2`。v2 将以下内容纳入可信请求绑定：

- HMAC、Router identity、installation identity、protocol epoch。
- method、path/query、body SHA-256。
- `issued_at_ms`：最多接受过去 30 秒、最多接受未来 5 秒。
- 16,384 项 replay cache；重放和容量达到上限时 fail-closed。
- Server 剥离客户端伪造的 `x-cc-switch-*`/ingress headers，再注入已验证的 Share/user/web identity。
- 内部 ingress 诊断响应头只供 Router 读取，不能泄露给公网调用方。

Server 仍兼容 v1 到 `1_788_825_600_000`（2026-09-08 00:00 UTC，代码使用 `<=`）；过期后必须 fail-closed。Router 的 `PROTOCOL.md` 仍写 `cc-switch-router-ingress-v1`，并引用旧行号，未记录 v2 body binding、replay、内部诊断头和 cutoff。这是发布协调风险，不是单纯文案问题。

推荐升级顺序：先升级 Router（能识别并剥离新的 internal ingress diagnostics），再升级依赖 v2 的 Server；回滚时先回滚 Server，再回滚 Router。应在两仓库增加同一组签名 fixture 和 cutoff contract test。

## 5. `cc-switch-router` 与 `cc-switch-market` 当前架构

### 5.1 Router

Router 的主职责是公网入口、用户/API token、Share entitlement、app/并发/边缘健康、SSH reverse tunnel、ingress 签发和 Client Web tunnel。它还包含 Share Market、Client Market 等内建目录/授权能力。Router 不应承担独立 Market 的余额 ledger；Server 也不应绕过 Router 直接把公网请求当作可信请求。

Router dirty diff 主要集中在 Telegram/notification/schema，当前没有证据表明它改变 ingress committed baseline；合并前仍需单独 review dirty diff，避免把未提交通知逻辑误当成协议实现。

### 5.2 独立 Market

Market 的核心模块为：

```text
src/main.rs       路由、启动和 worker
src/db.rs         libSQL/SQLite、migration、单写锁
src/proxy.rs      预授权、转发、usage、结算
src/ledger.rs     wallet/ledger transfer
src/topups.rs     Dodo webhook/topup
src/object_store.rs  request/webhook/payout 对象
src/router_client.rs Router share/request-log 同步
src/config.rs     环境配置
```

启动 worker 包含 Turso sync/backup、topup expiry、maintenance、Gate.io payout、Router share sync、request log sync 和 pricing sync。默认监听 `0.0.0.0:8080`，使用 permissive CORS、全局 64 MiB body limit、cookie session + CSRF、本地 SQLite 或 Turso replica；R2 object store 仍未形成完整生产实现。

Market 的关键资金状态是 `user_reserved → user_cash/provider payable/commission`；usage 不确定时会进入 `needs_review`，超出预授权部分可从 `user_cash` 取款，剩余部分由 `risk_loss` 平台账户承担。因此 settlement 的幂等、并发和风控阈值是系统核心，而不是普通后台 CRUD。

## 6. 显著问题清单

### 6.1 Server 与跨仓库问题

| ID | 级别/置信度 | 证据 | 影响 | 计划/产品决策 |
| --- | --- | --- | --- | --- |
| S-01 首次 setup 接管 | P1（条件性高危）/事实高、影响中 | `src/api/mod.rs` 的 `/api/setup/bootstrap`；`src/setup.rs` 只检查 `is_setup_complete()` | setup 未完成且管理端口可远程访问时，可设置管理员密码、owner、Router、tunnel subdomain，甚至签发 session/API token；systemd 示例为 `0.0.0.0` | 先决定 setup 是否必须本机/受保护网络完成；可选一次性 bootstrap secret、loopback gate 或安装时 secret，不能无决策直接改产品流程 |
| S-02 Router URL SSRF/内网探测 | P1/P2 /事实高、影响依赖部署 | `src/domain/settings/config.rs:normalize_router_url` 仅检查 http/https；setup 会请求 Router health/subdomain availability | 自定义 URL 可指向 loopback、private、link-local 或 metadata 地址；没有 DNS rebinding/解析结果过滤 | 若允许任意 Router URL，加入 scheme/port/解析 IP 白名单、DNS pinning 和 redirect 约束；若只支持受信 Router，改为 allowlist |
| S-03 Web session 非原子写与同步阻塞 | P1/P2 /高 | `src/domain/web_auth.rs` 在 async 请求中持 `std::sync::Mutex`，命中 token 后同步 `fs::write` 覆盖 JSON | 高并发管理请求会阻塞 Tokio worker；崩溃/断电可能截断 `web-auth-sessions.json`，导致 session 全失效 | temp+fsync+rename+目录 fsync；将认证热路径与持久化解耦或限频；保留父目录权限修复，不把当前问题误报为必然凭据泄露 |
| S-04 backup/restore 不是完整一致性快照 | P1 /高 | `state.rs::backup_targets` 只列主要 JSON/store；未覆盖 web sessions、ui settings、router-control SQLite、image capabilities、audit/OAuth/upgrade spool | 备份可恢复“主要数据”但不能承诺完整恢复；多文件替换中断会留下新旧混合状态；README 与 deployment 对边界表述不一致 | 先定义 RPO/RTO 和敏感 session 是否纳入；增加 manifest、跨文件 transaction marker/recovery、故障注入和 restore rehearsal；统一文档措辞 |
| S-05 管理面暴露与 permissive CORS | P2 /高 | `src/api/mod.rs` 使用 `CorsLayer::permissive()`；health/version/metrics 基本公开 | 直接公网监听时扩大浏览器跨源读取和诊断信息暴露面；不是单独的 auth bypass，但会放大配置错误影响 | 生产默认 loopback/反代；按 endpoint 制定 CORS allowlist、health/metrics/version 暴露矩阵 |
| S-06 Web terminal 是主机级权限 | 高影响信任边界 /事实高 | `enable_web_terminal` 默认可开；`require_web_admin_session` 接受 Router delegated owner/admin；`CC_SWITCH_TERMINAL_PERMIT_WRITE` 默认 true | Router owner/admin 经 Client Web tunnel 可执行 Server 主机 shell，默认可写；Router 被接管等同 Server 运维面被接管 | 必须明确“Router admin 是否等同 Server admin”；决定 local-owner-only、生产默认关闭/只读、独立高权限角色和审计。未经决定不直接禁用或扩权 |
| S-07 client subdomain adoption 跨文件非原子 | P2 /高 | `state.rs::commit_client_subdomain_adoption` 先写 `shares.json`、再写 `server.json`、最后更新内存 | 第二次写失败或进程崩溃会让 Share 子域名和 Server tunnel 配置分叉 | transaction marker + 启动恢复/对账；加故障注入测试 |
| S-08 God Object/超大热路径 | P2 /高 | `state.rs`、`forwarder.rs`、`transforms.rs`、`adapters.rs` 行数和职责集中 | 任意小功能触碰全局状态/stream 语义；锁、测试和 review 半径过大；机械搬文件会放大回归 | 先冻结 contract/invariant，再按 capability、stream lifecycle、usage contract 拆分；保留 state façade 和既有依赖方向 |
| S-09 文档与脚本漂移 | P1（发布流程）/高 | `docs/architecture-refactor-plan.md`、`docs/code-audit-gap-plan.md` 仍大量引用不存在的 `src/http.rs`、`src/core/*`；OAuth smoke 使用旧 test filter | 新贡献者会按旧路径改错代码；CI 可能在 0 tests 时假通过 | 本文作为 current roadmap；旧文档标记 historical；脚本对过滤器命中数为 0 直接失败，并增加 docs path/protocol drift audit |

### 6.2 Router–Server 协议问题

| ID | 级别/置信度 | 事实 | 影响与动作 |
| --- | --- | --- | --- |
| R-01 ingress 文档仍为 v1 | P1 /高 | Router `src/ingress_context.rs` 已是 v2；Server 有 v1 cutoff；`PROTOCOL.md` 仍写 v1 和旧行号 | 运维会按错误签名域、字段和升级顺序部署；必须以代码/fixture 重新生成协议文档，并把 v1 cutoff、replay、body binding、header stripping 纳入发布 checklist |
| R-02 Router dirty diff 未归类 | P2 /中 | committed baseline 之外有 Telegram/notification/schema 改动 | 不能把未提交行为写入三方契约；合并前需按“协议相关/无关”分组 review，并重新跑跨仓库 contract test |

### 6.3 Market 资金、凭据与迁移问题

| ID | 级别/置信度 | 证据 | 影响 | 计划 |
| --- | --- | --- | --- | --- |
| M-01 Dodo webhook 默认弱验签 | P0-条件 /事实高 | `src/topups.rs:652-662`：secret 为空或等于 `dev` 即跳过签名；`.env.example` 默认 `DODO_WEBHOOK_SECRET=dev` | 生产误保留 sentinel 时，公开 webhook 可伪造充值/退款，成功事件可增加 `user_cash`，甚至使平台账户负债 | 生产对空值、`dev`、`change-me-*` fail-closed；配置向导、README、`.env.example`、启动校验统一；保留明确的本地 mock 开关但不能让它默认为生产语义 |
| M-02 settlement 受影响行数未检查，ledger 无业务幂等 | P0/P1 /事实高 | `src/proxy.rs:1138-1161` 更新 charge 后不检查 affected rows；随后无条件 transfer；`ledger::transfer` 只插入普通 `ledger_entries`，无 `(reference_type, reference_id, event)` 唯一约束 | 并发/重复 settle 的第二次调用可能更新 0 行却再次支付 provider/佣金/risk_loss；`BEGIN IMMEDIATE` 不能弥补“更新 0 行继续转账”；admin settle/release 也有先查再写竞态 | 用 `UPDATE ... RETURNING` 或 affected rows==1；状态迁移和资金事件同一事务；增加唯一业务事件/幂等键；测试重复、并发、needs_review 终态 |
| M-03 monthly spend cap 在 reserve 事务外检查 | P1 /事实高 | `handle_llm_request_with_model` 先调用 `enforce_monthly_spend_cap` 查询，再进入 reserve 写事务 | 并发请求可读到相同 spent，同时通过 cap，合计超限 | 在 reserve 的 `BEGIN IMMEDIATE` 内重新检查并写入；加入并发压力测试 |
| M-04 risk_loss 无单请求/累计熔断 | P1 /事实高、策略影响高 | overage 依次从 reserved、cash，不足直接从 platform `risk_loss` 支付 provider | 异常 usage/pricing、恶意长 stream 或 overage 可无限放大平台损失；monthly cap 不能替代 risk-loss circuit breaker | 定义单请求、用户/Provider 累计、时间窗口、人工审核和自动熔断阈值；在策略确定前至少增加告警和 dry-run |
| M-05 object store 写入和 DB reference 分离 | P1/P2 /事实高 | `src/object_store.rs` 使用 `tokio::fs::write`；`put_bytes_once` 是 check-then-write；对象引用随后单独写 DB | 并发覆盖/部分写、DB reference 与对象不一致、orphan object；webhook/request/payout/ticket 对象可能含敏感数据，权限依赖默认 umask | temp+fsync+rename、私有 root/0600、对象 hash 校验；reference 与业务事务关联；实现 orphan GC 和 restore 校验 |
| M-06 migration 链脆弱 | P1/P2 /事实高 | 巨大 `SCHEMA` 加运行时多条 `ALTER TABLE`；用错误字符串包含 `duplicate column` 判断完成；显式 migration version 覆盖不完整 | 半迁移、崩溃恢复、SQLite/Turso 行为差异难诊断；无法可靠审计 schema 版本 | 引入单一 schema version/migration table；每步可重入、可观测、可回滚/前滚；在 SQLite/Turso 上做升级矩阵 |
| M-07 单进程写锁可能成为吞吐瓶颈 | P2 /事实高、影响待压测 | `execute`、`execute_batch`、`BEGIN IMMEDIATE` 共用一个 async write lock | request charge、settlement、webhook、Router sync、Turso latency 互相排队 | 先建立延迟/队列指标和压测基线，再决定拆写队列、批量或数据库拓扑；不要凭直觉取消锁 |
| M-08 session secret 占位值可通过长度校验 | P1/P2 /事实高 | 代码默认 `change-me-market-session-secret`，只检查长度 ≥24；`.env.example` 另有可预测 sentinel | 部署忘改时 cookie session pepper 可预测 | 启动拒绝已知 sentinel；首次部署随机生成并写入私有配置；轮换/失效流程写入 runbook |
| M-09 API key secret 文件权限不一致 | P1/P2 /事实高 | `src/api_keys.rs` 将完整 key 写入 `api-key-secrets/{user_id}.json`，直接 `std::fs::write`，无 temp/0600/目录权限策略 | 本机用户、备份或错误 umask 可能读取全部 API key；崩溃可截断文件 | 复用 `write_private_json`/`create_private_file` 类策略；原子写、目录 0700、文件 0600；评估是否应改为加密存储 |
| M-10 malformed JSON 降级为空对象 | P2 /事实高 | `src/proxy.rs:351` 使用 `unwrap_or_else(|_| json!({}))` | 协议错误可能继续进入 unknown model/上游/计费路径，错误、usage 和可观测性不一致 | 对请求体明确返回 400；原文只进入受控 audit object；增加 malformed/oversize contract test |
| M-11 配置默认漂移 | P2 /事实高 | `DODO_MOCK_CHECKOUT_ENABLED` 代码默认 false，但 `.env.example`/README/向导默认 true；session/webhook sentinel 也有多套 | 运维和本地环境对“生产安全默认”理解不同，容易误启用 mock 或弱验签 | 生成单一 config reference；生产 fail-closed、本地 mock 显式 opt-in；CI 比对代码、向导、示例和文档默认值 |
| M-12 未提交 capability 过滤改动 | P2 /中（需 review） | Market dirty `src/api_keys.rs` 删除 `enabled_claude/codex/gemini` 过滤，改为只看 `raw_json.marketApps` | 若不是有意迁移，DB 中关闭 capability 仍可能进入可售列表/allowlist；若是有意迁移，则需证明 Router raw JSON 是唯一权威 | 在合并前写清 source-of-truth、迁移兼容和回滚；补 DB capability 与 raw JSON 不一致测试 |
| M-13 Market 公开监听面 | P2 /高 | 默认 `0.0.0.0:8080`、permissive CORS、64 MiB body | 直接公网部署时扩大攻击面和资源消耗；不是替代 Router 的边缘鉴权 | 生产反代/网络 ACL、CORS allowlist、body limit 分层、readiness/metrics 暴露矩阵 |

## 7. 必须先做的产品决策

以下事项会改变信任边界或资金语义，不能在“代码规整”过程中凭工程师偏好决定：

1. 首次 setup 是“本机初始化”还是允许远程受保护网络初始化？是否需要一次性 bootstrap secret？
2. 是否支持任意自定义 Router URL？支持时允许哪些 scheme、端口和网络地址？
3. Router delegated owner/admin 是否等同 Server 主机管理员？Web terminal 是否 local-owner-only、默认关闭或只读？
4. `needs_review`、overage、risk_loss 的终态、限额、人工审批和退款语义是什么？
5. Market 的 Share capability 权威是 Router raw JSON、Market DB 字段，还是带版本的组合契约？
6. Server 是否继续多 JSON，还是迁移到统一 SQLite/libSQL？旧 cc-switch DB 的读取/迁移范围、RPO/RTO 和跨进程部署方式是什么？
7. v1 ingress 在 2026-09-08 cutoff 前后的发布、回滚和灰度策略是什么？

## 8. 文档规整计划

### D0：历史提案（已由 Client + Router 路线取代）

- 本文不再作为当前入口；仅保留旧三方审计证据。
- 旧 Token Market 解耦专项以 `docs/token-market-decoupling-plan.md` 为当前权威执行计划；它覆盖旧 Market 的删除/保留/泛化矩阵、跨 Router Gateway 预留和 client+router-only 验收门禁。
- `docs/architecture-refactor-plan.md`、`docs/code-audit-gap-plan.md` 保留历史证据，但在完成复核前不得把其中的“已完成”、旧行号和旧路径当作现状；后续应在文件顶部加 `historical` 标识和指向本文的链接。

### D1：先写契约，再动代码

新增/整理以下文档，所有条目必须指向代码、fixture 或可执行测试：

1. `docs/system-overview.md`：三方角色、数据面/交易面时序、权威边界和术语表。
2. `docs/trust-boundaries-and-auth.md`：端点可达性、Router ingress、delegated web session、terminal、setup threat model。
3. Router `PROTOCOL.md` 与 Server 对应章节：以 v2 fixture 生成 ingress 字段、签名域、时间窗口、replay、header stripping、cutoff 和升级顺序。
4. `docs/market-settlement-contract.md`：reserve/streaming/needs_review/settled/failed_released 状态机、usage state、ledger event、幂等键、risk_loss 和 webhook。
5. `docs/storage-backup-recovery.md`：每个 store/object 的 owner、敏感级别、原子性、备份覆盖、恢复顺序、RPO/RTO。
6. `docs/provider-coverage.md` 与 `UPSTREAM_IMPORT.md`：只记录五个权威 upstream 来源的 Provider 类型证据；外部仓库改动不作为同步实现源。

### D2：清理漂移与生成式门禁

- 重新核对 `README.md`、`docs/deployment.md`、`docs/real-acceptance-runbook.md` 的监听地址、配置目录、backup 边界和默认值。
- 给历史计划中的旧路径加 `historical/current` 标签；删除或改写会误导实施者的旧行号、旧测试数量和旧模块名。
- 修复 `scripts/smoke/oauth-readiness-check.sh` 的测试过滤器；对 `cargo test` 过滤器命中 0 项直接失败。
- 增加 docs drift audit：检查 `src/http.rs`、`src/core/` 等不存在路径，检查 Router 协议常量和文档签名域，检查配置代码/向导/`.env.example` 默认值一致性。
- 保持 `docs/remaining-work-index.md` 为本地-only 索引，不把它当作提交的权威状态源。

### D3：运维与事故 runbook

补齐 setup 接管、Router/Server 升级回滚、v1 cutoff、web terminal、webhook secret 轮换、Market ledger 对账、backup restore、object orphan GC 和 clock skew 的操作步骤。每个 runbook 都应区分“本地离线验证”“Router/Market 集成验证”“真实生产前置条件”。

## 9. 代码规整计划

### C0：先修资金和安全止血项

在任何大规模移动文件前，优先完成并测试：

1. Dodo webhook sentinel fail-closed。
2. settlement affected rows、ledger business-event 唯一约束和重复/并发 settle 测试。
3. monthly cap 与 reserve 合并到同一原子事务。
4. session secret/API key secret/private JSON 的 sentinel、权限和原子写。
5. Server web session 原子写与 backup manifest/recovery 设计。

这些变更必须独立提交，不能和纯结构重构混在一起。

### C1：Server 按 capability 和生命周期拆分

保持 `state.rs` 作为过渡 façade，逐步提取以下边界：

```text
Ingress/identity      验签、replay、header stripping、trusted context
Control auth/setup    setup、web auth、Router delegated session、password contract
Router/tunnel         installation、lease、share tunnel、client web
Provider/account      provider registry、account selection、OAuth refresh、quota
Share binding         binding invariant、entitlement、subdomain adoption
Stream lifecycle      pre-commit、first business event、idle timeout、terminal event
Usage contract        usage state/revision、request log、audit、Market export
Persistence/recovery  store、manifest、backup、restore、migration marker
Operations             terminal、upgrade、restart、metrics、health/readiness
```

`forwarder.rs` 不按“每 500 行一个文件”拆分；先把 request policy、upstream attempt、stream commit/finalize、usage finalization 和 object persistence 的接口固定，再将 Provider-specific adapter 按协议族移动。`transforms.rs`/`adapters.rs` 只在有明确 contract test 时拆分，避免重复转换逻辑。

所有新状态写路径继续通过 `ServerStateInner` 域方法，按声明锁顺序获取锁；proxy 不直接调用 clients 或 api。每次拆分至少保留一个旧/新路径等价的 contract test 和故障注入测试。

### C2：Market 按事务边界拆分

将 `src/proxy.rs` 的生命周期明确成：

```text
authorize → price snapshot → atomic reserve/cap check
          → Router request/stream
          → usage parse/state decision
          → idempotent settlement event
          → Router log sync / audit / object retention
```

把 ledger transfer、webhook processing、Router inventory、pricing 和 object store 分成明确 service；每个资金事件使用稳定 `business_event_id`，数据库约束负责拒绝重复，而不是依赖调用方先查状态。所有 admin settle/release 也必须走同一状态机。

### C3：Router/Server 共享协议测试

- 用固定 JSON/HMAC fixture 覆盖 v1/v2、method/path/body tamper、过期/未来时间、replay、容量、Router/installation mismatch 和 internal diagnostics stripping。
- 让 Router 生成 fixture、Server 验证 fixture；反向也验证 Server 的错误码不会泄露公网诊断头。
- 将 protocol epoch、signature domain、cutoff 和 body limit 作为机器可读 contract，而不是只写在 Markdown。

### C4：数据库/存储迁移

先定义 schema version、迁移锁、备份 manifest、回滚/前滚和多进程语义，再决定是否统一 SQLite。迁移前后必须验证：Provider/Account/Share binding、`accounts.key`、usage revision、ledger balance、web session 失效策略和 object references。不能以“已有 `router-control.sqlite`”宣称 Server 已完成 DB 迁移。

## 10. 分阶段交付与门禁

| 阶段 | 目标 | 必须完成的门禁 | 不能宣称的内容 |
| --- | --- | --- | --- |
| G0 基线冻结 | 记录提交、dirty diff、术语、产品决策 | `git diff` 分类、本文和 contract issue 完成 | 不宣称真实 E2E |
| G1 止血 | webhook、settlement、cap、secret、原子写 | 单元/并发/故障注入测试；`cargo fmt --check`、`cargo check`、目标 `cargo test` | 不把默认 sentinel 留在生产路径 |
| G2 契约 | ingress v2、usage/settlement、endpoint/auth、storage/recovery 文档与 fixture 对齐 | provider/UI/boundary audits；Router↔Server contract tests | 不把旧 `PROTOCOL.md` 当权威 |
| G3 结构 | Server/Market capability 拆分，依赖方向和 state write 门禁 | `cargo check --all-targets`、`cargo test`、static checks、docs drift audit | 不以文件移动数量衡量完成 |
| G4 集成 | Router tunnel、Share grant、Market reserve/settle、request log sync | 真实 Router URL/control secret、Market session/API key、Share grant 和 clock sync | 缺输入时只能标记 blocked |
| G5 发布 | 备份恢复、升级/回滚、负载和安全演练 | `scripts/smoke/smoke-local.sh`、`RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` 仅作离线 readiness；真实环境另行证据 | 不把 readiness 当生产验收 |

代码改动后的最低本地验证集合（按仓库约定）为：

```bash
cargo fmt --check
cargo check
cargo test
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
scripts/smoke/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh
```

测试出现默认栈溢出或稳定逻辑失败时，必须记录为失败/blocked；不能只通过设置更大栈、跳过测试或让过滤器命中 0 项来获得绿灯。

## 11. 真实验收前置条件

真实三方链路开始前，必须准备并以私有 env/fixture 管理：

- Router 实际 URL、control secret、installation 注册、SSH reverse tunnel 和时间同步。
- Market API key/session、充值或测试余额、价格快照、可售 Share、grant/撤销输入和 request-log sync。
- Claude/Codex/Gemini 以及必要兼容 Provider 的真实 OAuth/API credential；真实 stream、usage、401 refresh、quota 和错误样本。
- Dodo webhook secret、签名事件 fixture、退款/重复事件样本；Gate.io payout 只在隔离账户测试。
- 备份/恢复目标目录、对象 store、权限和故障注入环境。
- 版本升级/回滚窗口：先 Router 后 Server；回滚先 Server 后 Router；v1 cutoff 前完成观测。

缺少上述输入时，报告只能写“代码/fixture/local readiness 已通过”或“blocked by missing inputs”，不能写“Router/Market/OAuth/真实交易通过”。

## 12. 不应在规整中回退的稳定契约

- Provider 覆盖以五个 upstream baseline 来源和 `assets/contract/upstream-provider-source-baseline.json` 为准；不要把 desktop upstream 整仓复制进 Server。
- Web UI 密码设置固定调用 `POST /web-api/auth/password/set`，body 只有 `{ newPassword }`；成功后清 sessions 并强制重新登录。设置页不得加回旧密码/确认密码输入框，也不得改走 `/change`。
- Router Client Web tunnel 放行整个 `/web-api/` 前缀；`/api/*`、`/v1/*` 不经该 tunnel，`/_ctl/*` 继续公网 404，`/_share-router/*` 继续控制签名。
- Server 必须独立验证 ingress，不能把 Router 注入的普通 header 当公网可信输入；v2 body/path/method/replay 约束不能被兼容逻辑削弱。
- `domain`/`proxy` 依赖方向、`ServerStateInner` 域写方法、跨存储锁顺序和 shares 写 API 是结构不变量，不因拆文件而回退。
- Server Web UI 继续以本产品需求、Server API 和 `assets/contract/web-runtime-contract.json` 为唯一实现依据；不从外部项目批量同步 React、locale、样式或页面结构。

## 13. 当前建议的下一步顺序

1. 由产品负责人确认第 7 节的 setup、Router URL、terminal、risk_loss、capability source-of-truth 和 DB 迁移决策。
2. 在 Market 先完成 M-01/M-02/M-03，建立资金事件唯一约束和并发测试。
3. 在 Server 完成 S-01/S-02/S-03/S-04 的威胁模型、原子写和恢复设计；不先做大规模文件移动。
4. 同步更新 Router `PROTOCOL.md`、Server contract fixture 和升级/回滚 runbook，消除 R-01。
5. 修复 smoke 旧过滤器和两个稳定测试红灯，建立“命中 0 tests 即失败”的门禁。
6. 通过 G1/G2 后再开始 C1/C2 的 capability 重构，每个 capability 保持独立提交、验证和可回滚。

本文完成后，任何新增审计结论都应附：仓库提交/dirty 状态、证据路径与行号、影响条件、置信度、是否需要产品决策和对应验证命令。这样文档才能持续作为三方系统的 current source，而不是再次变成过期的重构清单。
