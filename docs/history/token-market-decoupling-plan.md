# Client + Router：旧 Token Market 解耦/剔除计划与实施记录

> **归档文档 · 只读 · 不代表当前实现**
>
> | 项 | 值 |
> | --- | --- |
> | 状态 | 历史记录（archived），仅作过程与决策证据保留 |
> | 归档日期 | 2026-08-20 |
> | 原路径 | `docs/token-market-decoupling-plan.md` |
> | 当前权威 | [`docs/architecture/overview.md`](../architecture/overview.md)、[`docs/share/access-policy.md`](../share/access-policy.md) |
>
> 不得据此判断当前目录结构、行号、测试数量、能力状态或产品边界。文档索引见 [`docs/README.md`](../README.md)。

> 状态：**仓库内实施完成，生产部署与真实 E2E 待独立环境输入**。
>
> 版本：2026-08-18 / v2.1（含 Gateway inventory 隐私收口）。
>
> 适用仓库：`/data/projects/cc-switch-server`（Client）与 `/data/projects/cc-switch-router`（Router）。
>
> `/data/projects/cc-switch-market` 不属于本轮修改范围，也不再是 Client + Router 的运行时依赖。
>
> 收尾审查与受控残留分类见 [`token-market-decoupling-review.md`](token-market-decoupling-review.md)。

## 1. 决策与最终边界

将旧 Token Market 从 Client + Router 中完整剔除是合理且已经落地的方向。旧实现把 Market email、subdomain、bearer session、Router host/proxy、token 交易和 request settlement 绑在单个 Router 上，无法支持未来“一个独立平台在多个 Router 分别采购 Share Market 车位”的模型。

最终系统边界为：

```text
CLI / Code Agent
       │
       ▼
cc-switch-server (Client)
  Provider / Account / Share / proxy / usage
       │ Client + Share tunnel、Contract v2
       ▼
cc-switch-router (Router)
  public ingress / Share Market / Client Market
  market_access / market_billing / neutral Gateway adapter

未来独立 Token Market（未实现）
       │ 每个 Router 独立注册、采购 seat/entitlement
       └──────────────► neutral Gateway ↔ Share contract
```

这里的“剔除”只针对旧独立 Token Market 集成，不按名称删除所有 `market` 模块：

- **已删除**：旧 Market registry、bearer/session、Market public host、Market tunnel、`/_market/proxy`、旧 discovery、旧 Share sale/ACL wire、旧 Market request-log/notification/runtime 表和专属 smoke/env。
- **必须保留**：Share Market、Client Market、`market_access`、`market_billing`、Client/Share tunnel、Share grant/revoke、usage/revision、Gateway Ed25519 基础。
- **未来另行设计**：跨 Router tenant/gateway/seat/grant contract，以及独立 Token Market 的用户、API key、token 定价、余额、结算和退款。

是否使用 `/data/projects/proxy/sub2api` 不是本次解耦的前提，也没有在本轮作出决定。

## 2. 最终处理矩阵

### Server

| 能力 | 最终处理 |
| --- | --- |
| `/api/token-markets`、`list_token_markets`、`PublicTokenMarket`、Router `/v1/markets` discovery | 删除 |
| Web runtime/query/UI 的 Token Market selector 与价格字段 | 删除 |
| `MarketSlug`、`PublicHostKind::Market`、Market host claim | 删除；本地 control DB 最终只允许 Client/Share |
| Share active contract | 固定为 Contract v2：`freeAccess` + `userGrants` |
| v1 sale/ACL/appSettings 字段 | 仅在一次性持久化迁移和负向 API 测试识别；active API 明确拒绝 |
| Provider/Account/Share/proxy/usage | 保留 |
| Router Share Market managed grant | 保留，Owner 不可伪造或修改 |

### Router

| 能力 | 最终处理 |
| --- | --- |
| `router_markets`、旧注册/session/host/tunnel/proxy/admin/notification active path | 删除 |
| `/v1/markets*`、`/v1/market/*`、`/v1/admin/markets*`、`/_market/proxy*` | 保留显式 `410 Gone` 退役响应 |
| `/markets` Web 旧书签 | 保留到 `/share-market/` 的安全跳转 |
| `market_request_logs` 与旧 disabled/failure/runtime/notification 表 | migration 21 校验归档后物理删除 |
| `legacy_token_market_*` archive/manifest | migration 19 临时生成并校验；migration 21 同事务前置校验后物理删除 |
| Gateway observation | 只保留 gateway/share/model/status/latency/token/region；拒绝下游用户、API key、USD、settlement 与未定义 tenant/consumer 字段 |
| Share Market、Client Market、access、billing | 完整保留 |

## 3. 已实施内容

### 3.1 Server active path

- 删除 Token Market discovery REST/invoke、Router discovery client、类型、query cache 和 Web selector；
- 删除 Market public-host domain 类型与 claim 分支；
- `.env.example`、smoke、real-acceptance 和 release-readiness 不再要求旧 Market URL/credential；
- 删除 `router-market-smoke.sh` 与 `direct-market-diagnostics.sh`，新增 Client + Router 的 `router-share-smoke.sh`；
- readiness 接入 `scripts/audit/audit-token-market-decoupling.mjs`。

### 3.2 Share Contract v2

正式 wire、REST、invoke 和 UI 只包含 `freeAccess` 与 `userGrants`。Server/Router `ShareDescriptor` 使用 `contractVersion=2` 和 `deny_unknown_fields`；跨仓 lease fixture 已升级并重新签名。

以下字段全部退休：

```text
acl
forSale / for_sale
officialPricePercent / official_price_percent
forSaleOfficialPricePercentByApp / for_sale_official_price_percent_by_app
sharedWithEmails / shared_with_emails
marketAccessMode / market_access_mode
accessByApp / access_by_app
appSettings / app_settings
```

Server REST/import/invoke 对 camelCase 与 snake_case 退休字段都 fail-closed。Router active model 不包含这些字段，未知字段不会被静默丢弃。完整访问规则见 [share-access-policy.md](../share/access-policy.md)。

### 3.3 Server 持久化退休

`shares.json` 在加载边界执行一次性迁移：

- canonical grants 为空时，提取仍有意义的旧 ShareTo 邮箱；已有 grants 时陈旧 ACL 不能覆盖；
- `freeAccess` 缺失时仅把旧 `Free` 迁为公开免费，旧 `Yes` 收窄为私有；
- 删除所有 v1 sale/ACL 字段，原子写回并重新解析验证；
- 删除历史 `legacy-token-market-archive` payload；
- 只写不含 email/价格/credential 的 `data-retirement-audit.json`（source SHA-256、字段计数、删除文件计数）。

Server 本地 Router-control DB 先在 v2 事务中复制并校验旧 Market host，随后 v3 校验后物理删除 archive/manifest；最终 `public_hosts` 只接受 `client/share`。

### 3.4 Router 路由与身份退休

- 删除旧 registry/auth/session/host/tunnel/proxy/store/API/UI 活跃实现；
- 所有旧 Market URL 由同一个 retirement router 返回 `410 Gone`，避免落入 UI catch-all 或其他代理；
- `public_hosts.kind=market` 被迁移删除，并由约束/trigger 阻止重新插入；
- Gateway 签名绑定收到的原始 body SHA-256，不对反序列化对象重新编码；
- self-reported Gateway owner email 只作为本地审计元数据，不参与 Share 可见性、proxy、headroom、feedback 或 observation 授权；
- Gateway Share inventory wire 使用 opaque `shareName`，不包含 Share/installation owner email、Provider account email 或 Provider API URL；owner-scope feedback 只使用不序列化的 Router 内部分组键；
- 新的中性 tenant/seat grant 尚未定义，因此普通 Share 对 Gateway 继续整体 fail-closed。

### 3.5 Router 数据物理退休

Router 使用追加式 migration，未修改已发布的 frozen baseline：

1. **migration 19**：冻结旧 writer；复制旧 live 表到临时 `legacy_token_market_*` archive；生成行数和 SHA-256 manifest；创建中性 `gateway_request_observations`。
2. **migration 20**：建立 canonical `free_access` 策略及其与 Share Market entitlement 的互斥约束。
3. **migration 21**：执行前由 Rust 再次验证 migration 19 archive；只把同时具备已知 Share 与用户邮箱的最小 usage 迁入 canonical Share log；写聚合 `data_retirement_audit`；把 `capacity_request_observations` 收口为 Gateway-only view；物理删除全部旧 live/archive/manifest 表。

被物理删除的 live 表：

```text
router_markets
market_notification_emails
market_request_logs
market_disabled_shares
market_share_model_failure_state
market_share_runtime_states
```

对应的全部 `legacy_token_market_*` archive 和 manifest 同时删除。若 archive 行数或 checksum 被篡改，migration 21 fail-closed，不记录成功版本，也不删除 archive。

Metrics DB 同样重建 canonical `llm_request_metrics`，物理删除 `market_email` 列与 `legacy-token-market` rows；新 Gateway metrics 只写 `gateway_id`，不写外部 USD cost。

### 3.6 UI 与普通 Share 设置

- Router 旧 Markets 页面组件已删除，旧 URL 仅重定向；
- Share 编辑把“共享与配额 - 是否出售”替换为一个“公开免费使用”复选框，默认私有；
- “授权邮箱”文本框删除，ShareTo 统一由“授权用户与配额 - 添加授权用户”创建；
- Share 卡片和 usage/connection test 只读取 canonical grants；
- Share Market 自己的 listing/seat 免费或付费报价继续存在，与普通 Share 的 `freeAccess` 严格互斥。

## 4. 明确保留的能力

以下名字虽然包含 market 或服务未来外部容量消费者，但不是旧 Token Market 残留：

- `src/share_market.rs`、`share_market_*` listing/seat/subscription/entitlement/grant/revoke/ack；
- `src/client_market*.rs`、Host quote/trade/provision/terminal/cleanup；
- `src/market_access.rs` 的 counterparty/access/credit policy；
- `src/market_billing.rs` 的 service contract、invoice、payment claim、dispute 和 suspend/resume；
- `/v1/gateways/register`、`/v1/gateway/*`、`/_gateway/proxy/*` 的 Ed25519 Gateway adapter；
- Server Provider/Account credentials、Share binding、Router signed ingress、usage state/revision 和 request identity。

审计不得以简单搜索 `market` 的方式删除这些能力。

## 5. Frozen schema 中的受控遗留

Router 的 `schema/0001_baseline.sql` 已发布并由 checksum 固定，不能修改。其 `shares` 表仍含 v1 compatibility columns；当前：

- active wire/model/UI 不暴露；
- active 授权、usage、connection test 不读取；
- 所有新写入固定为 `[]`、`{}`、`selected`、`No`；
- 测试证明陈旧列不能授予访问，canonical grants 始终优先。

这属于 schema 形状债务，不是功能兼容层。未来若清理，应新增独立 migration 重建整个 `shares` 表并审计所有外键、trigger、index 和 Turso 升级路径；本轮不为追求“零字符串命中”冒险重写 frozen baseline。

## 6. 回滚策略

仓库 migration 是向前退休：

- migration 21 前会校验临时 archive；校验失败自动停止；
- migration 21 完成后，旧 Market 明细不再保存在业务库，只保留非识别性聚合 receipt 与符合条件的 canonical Share usage；
- Contract v2 不承诺回滚到依赖 v1 sale/ACL wire 的二进制；
- 生产回滚必须恢复与旧版本配套的完整备份，并单独处理 DNS、secret、session 和外部流量，不能靠重新开放 410 路由恢复旧系统。

因此部署前仍必须完成生产 inventory、备份恢复演练和数据保留审批。仓库内测试不能替代这项授权。

## 7. 生产实施顺序（尚未执行）

1. 只读盘点部署版本、旧 Market 流量、session、host、账务、DNS/证书、secret、备份和责任人。
2. 确认旧注册与新交易已停止，处理未完成请求、退款、通知和外部调用方迁移。
3. 备份 Server `server.json/providers.json/accounts.json/shares.json/usage` 与 Router 业务/metrics DB，并演练恢复。
4. 先部署能识别并验证 retirement migration 的 Router，再部署 Contract v2 Server；观察 410、migration receipt、Share/Client tunnel 和市场账务。
5. 只有在真实验证通过且回滚窗口结束后，才删除部署环境中的旧 Market secret、DNS、证书和独立服务。

没有上述运行输入时，本项目只能报告 repository/local/offline/fixture 结果，不能声称生产剔除完成。

## 8. 验证门禁

Server：

```bash
cargo fmt --check
cargo check
RUST_MIN_STACK=67108864 cargo test -- --test-threads=1
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
node scripts/audit/audit-token-market-decoupling.mjs
scripts/smoke/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh
```

Router：

```bash
cargo fmt --check
cargo check
cargo test -- --test-threads=1
cd frontend && npm run lint && npm run build
```

跨仓：

- Contract v2 lease request/signed-payload fixture 必须一致并通过 Ed25519 验签；
- audit 必须证明 active wire/UI 无 v1 sale/ACL 字段、migration 21 删除全部旧表、metrics canonical schema 无 `market_email`；
- `git diff --check`；
- 扫描旧 endpoint、type、env、table、文档，所有命中必须属于 migration、负向测试、410/redirect 或 frozen storage 兼容边界。

最终执行结果以本轮交付说明为准；真实 Router/OAuth/Share Market grant/Client Market trade 输入未提供时，不记录真实 E2E 通过。

## 9. 未来 Token Market 的前置条件

未来独立平台应把下游 API key、token 价格、余额、reserve/settlement、退款、跨 Router 调度和用户账本留在平台自身。Router 只负责本地 Share catalog/seat、准入、grant、headroom、边缘授权、服务账务和脱敏 observation；Server 只负责 Provider/Share/proxy/usage。

开始任何新平台实现前，第一份产出必须是机器可读的 Gateway ↔ Share contract，至少定义：

- 全局 tenant ID、Router-local gateway ID、seat/subscription/entitlement ID；
- 公钥注册/轮换/撤销、scope、timestamp/nonce/body/method/path 签名域；
- grant/revoke/ack 状态机与幂等键；
- headroom/health/feedback/usage observation 的最小字段和 retention；
- 多 Router 隔离、错误码、能力版本和负向 fixture；
- Router seat/service billing 与平台下游 token billing 的单一责任边界。

不得复用旧 Market email、subdomain、`forSale=Yes` 或自报 owner email 充当跨 Router tenant，也不得把旧 `cc-switch-market` active path重新接回 Client/Router。
