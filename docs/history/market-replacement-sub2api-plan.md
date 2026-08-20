# 基于 sub2api 的 Share Market 替换评估与实施规划

> **归档文档 · 只读 · 不代表当前实现**
>
> | 项 | 值 |
> | --- | --- |
> | 状态 | 历史记录（archived），仅作过程与决策证据保留 |
> | 归档日期 | 2026-08-20 |
> | 原路径 | `docs/market-replacement-sub2api-plan.md` |
> | 当前权威 | [`docs/architecture/overview.md`](../architecture/overview.md)、[`docs/share/access-policy.md`](../share/access-policy.md) |
>
> 不得据此判断当前目录结构、行号、测试数量、能力状态或产品边界。文档索引见 [`docs/README.md`](../README.md)。

> **历史/探索性文档（非当前权威计划）**：当前产品方向已调整为先从 `cc-switch-server` 与 `cc-switch-router` 完整剔除旧 Token Market，只保留 Client + Router、Router Share Market 与 Client Market。当前权威计划见 [`token-market-decoupling-plan.md`](token-market-decoupling-plan.md)。本文仅保留此前对 sub2api 及独立 Market 形态的可行性研究；不应据此开始 fork、迁移或替换实施。
>
> 状态：**历史评估，未实施（已被当前解耦计划取代）**
>
> 评估日期：2026-08-18
>
> 适用范围：`cc-switch-server`、`cc-switch-router`、`cc-switch-market`、`/data/projects/proxy/sub2api`
>
> 本文只记录现状、边界、决策门禁和迁移路线。不授权删除仓库、修改业务代码、迁移数据库、切换流量或关闭旧 Market。

## 1. 结论先行

这个方向**有条件地合理**：把独立的 `cc-switch-market` 从 Server/Router 的财务与交易耦合中剥离，并以 `sub2api` 的 PostgreSQL/Redis/用户、支付、计费和运营基础设施为底座，能够降低当前 Rust Market 的维护成本，也更适合把 Market 做成独立产品。

但它不是“换一个二进制”或“fork 后改几个 URL”：

```text
错误的理解：fork sub2api → 改路由 → 删除 cc-switch-market

正确的理解：保留 Router 的 Share entitlement/edge kernel
           + 重新定义 Market ↔ Router ↔ Server contract
           + 在 sub2api 上新增 Share catalog/grant/settlement 领域
           + 灰度对账后再剔除旧 Market
```

推荐采用**方案 A：独立 Market 替换，Router 保留最小 Share entitlement kernel**。新 Market 负责用户资金、目录、订单、usage 结算和 provider earnings；Router 负责公网入口、实时 entitlement、并发/headroom、撤销和 tunnel；Server 继续掌管 Provider/Account 凭据、Share binding、协议适配和上游转发。

不建议直接采用**方案 B：一次性删除 Router 内所有 `share_market`/`client_market`/`market_billing`**。那会把 Router 的产品边界、数据库和协议一起重写，除非产品已经另行定义完整的 grant、revoke、lease、usage event 和 edge authorization 替代协议。

这里的“完全剔除”应拆成三个层次：

1. **必须剔除**：Server/Router 对旧 `cc-switch-market` 的 direct URL、旧 bearer/session、旧 webhook/credential、旧 request-log adapter 和旧 Market 专属 readiness/deployment 依赖。
2. **应迁移**：旧 Market 的用户资金、价格、订单、usage settlement、provider earnings/payout、运营后台和交易数据，全部迁到新的 sub2api-derived Market。
3. **不能默认剔除**：Router 的 Share entitlement、grant/revoke、edge headroom/concurrency、ingress Share identity，以及 Server 的 managed grant/usage observation。这些是当前系统的运行时安全内核，不是旧 Market 的实现细节。

只有产品明确要重写 Router 的授权内核，并先提供等价的新协议时，才把第 3 层列入删除范围。

## 2. 审计基线与术语

### 2.1 提交和工作树

| 仓库 | 角色 | 审计基线 | 工作树注意事项 |
| --- | --- | --- | --- |
| `/data/projects/cc-switch-server` | Router 的 Client installation / token server | `2c2caa9` | 有用户已有 Web/UI、`web-dist` 和文档改动，不能覆盖或回退 |
| `/data/projects/cc-switch-router` | 公网 Router、隧道、边缘授权 | `47b4374` | 有 Telegram/notification/schema 等未提交改动；本规划不把 dirty diff 当作已发布行为 |
| `/data/projects/cc-switch-market` | 旧的独立计费/交易 Market | `2530348` | `src/api_keys.rs` 有未提交改动；capability source-of-truth 尚未冻结 |
| `/data/projects/proxy/sub2api` | 计划作为定制 Market 基座 | `8869775ed` | 审计时工作树干净 |

### 2.2 术语约定

- **consumer**：使用 Claude/Codex/Gemini CLI 或 API 的最终调用方。
- **Router Client installation**：运行 `cc-switch-server` 的安装实例；它注册 installation identity 并建立反向隧道。
- **Market user/API key**：计费和订单主体，不等于 Router Client installation。
- **Share**：Server 上绑定 Provider/Account 的可路由资源；Router 只持有其边缘身份和实时状态，不应持有 Provider token 明文。
- **entitlement/grant**：某个 buyer/consumer 对 Share、app、模型、并发和 token policy 的授权。
- **账本权威**：最终可以决定余额、预授权、结算和 provider payable 的系统；迁移前必须只有一个。

## 3. 当前系统事实

### 3.1 三方数据面和交易面

```text
consumer
   │ API key / Router Share URL
   ▼
cc-switch-router
   │ 公网入口、edge auth、Share entitlement、并发/headroom
   │ SSH reverse tunnel + signed ingress v2
   ▼
cc-switch-server（Client installation）
   │ Share binding → Provider bundle → bound Account/OAuth credential
   ▼
upstream provider

独立 Market 控制面：
API key → 价格/余额 → reserve → 选择 Share → Router proxy/lease
        → Server usage → settle / needs_review → provider/commission ledger
```

必须区分三个已有边界：

1. `cc-switch-server` 是 Provider/Account 凭据、Share binding、协议适配、上游转发和 Server usage observation 的权威。
2. `cc-switch-router` 是公网入口、installation/tunnel、ingress 签名、实时 Share entitlement、边缘并发和可达性的权威。
3. 旧 `cc-switch-market` 是用户余额、预授权、价格、usage 解析、ledger、provider earnings/payout 的权威；Router 内建 Market 模块不是这本资金账本。

### 3.2 独立 `cc-switch-market` 的现状

旧 Market README 描述的链路是：

```text
API 用户 → cc-switch-market → Router market proxy
         → Client tunnel → 上游模型服务
```

代码和 schema 体现它同时承担了：

- 用户门户、登录/session、API key、充值和支付 webhook；
- model price/routing、Router Share inventory、headroom 和 feedback；
- 请求预授权、streaming 生命周期、usage 解析和结算；
- `user_cash`、`user_reserved`、`client_payable`、`payout_reserved`、`payment_clearing`、`settlement_paid`、`fee_revenue`、`risk_loss` 等账户；
- provider 收益、提现、工单、对象存储和管理审计。

当前旧 Market 默认抽成基线为 platform `1000 bps`（10%）和 Router `500 bps`（5%）。迁移时必须把它们转换为带生效时间/版本的 pricing/settlement snapshot，不能在新 Market 中继续散落为常量；退款、`needs_review` 和 provider payout 也必须引用同一快照。

旧 Market 的典型请求状态为：

```text
reserved → streaming → settled
                    ↘ needs_review → settled / failed_released
reserved → failed_released
```

关键表包括 `request_charges`、`request_idempotency`、`router_shares`、`share_health`、`router_request_log_sync_state`、`provider_claim_profiles`、`payout_requests`、`settlement_batches` 等（`/data/projects/cc-switch-market/src/db.rs`）。

这套实现已经暴露出迁移时不能照搬的风险：Dodo webhook sentinel/验签边界、settlement affected-rows 和业务事件唯一性、monthly cap 并发超支、`risk_loss` 无稳定熔断、object store 与 DB reference 非原子、migration 链脆弱、session secret 可为占位值、API key 文件权限/原子写不统一、malformed JSON 降级为空对象，以及默认 `0.0.0.0:8080`/permissive CORS。

### 3.3 Router 的两个不同“Market”

Router 中存在两类不能混为一谈的能力。

#### A. Share entitlement/edge kernel（替换后仍需保留）

Router `src/share_market.rs`、`src/market_access.rs`、`src/client_market*.rs` 已实现：

- Share listing、seat、subscription、owner/renter ACL；
- `grant_pending → active → revoke_pending → released/failed`；
- `share_control_operations`、`operation_id`、`entitlement_id`、`share_sequence`、ack/retry/dead-letter；
- edge 并发/headroom、online/health、capability block、撤销和 Server ack；
- Router ingress 中的 Share identity 和 runtime availability。

Server `src/domain/sharing/router_contract.rs` 的 `ShareManagedGrantOperation` 要求 `operation_id`、`entitlement_id`、`share_sequence`、`expected_config_revision`、`action`、`email` 和 policy；`ShareDescriptor` 还带 `config_revision`、`descriptor_generation`、`descriptor_fingerprint`、`user_grants`、bindings、token/parallel policy。这些是运行时安全契约，不是旧 Market 的可选字段。

#### B. Router 内建时间/信用账务（是否保留必须决策）

`src/market_billing.rs` 及 schema 中的 `market_service_contracts`、`market_service_intervals`、`market_accrual_entries`、`market_invoices`、credit/dispute 表，按 Router health observation 对 supplier↔buyer seat/host 时间进行 postpaid billing；worker 约每 5 秒 reconcile，health stale 超过约 20 秒时不计费。

它和旧独立 Market 的 token usage 账务不是同一本账。迁移必须选择：

1. 保留 Router 的 supplier↔buyer 时间租赁账本，并让新 Market 只负责 token/API 用户账本；
2. 由新 Market 完全接管支付/结算，Router 账务退化为 entitlement/runtime 状态；或
3. 两种产品并存，但明确不同收费对象、事件和 ledger，绝不对同一请求/seat 重复收费。

在产品决策冻结前，不得删除 `market_billing` 表或把它与新 token settlement 合并。

### 3.4 Router 已有的 Gateway 基础

Router 已提供比旧 bearer-session market API 更适合新 Market adapter 的接口：

```text
POST /v1/gateways/register
GET  /v1/gateway/shares
POST /v1/gateway/shares/headroom
POST /v1/gateway/shares/feedback
POST /v1/gateway/request-logs/batch
```

请求使用 Ed25519 gateway identity 和 `x-cc-gateway-id`、timestamp、nonce、signature，scope 默认包含 `gateway:shares:read`、`gateway:proxy:use`、`gateway:feedback:write`、`gateway:request_logs:write`。这条路径目前仍缺 Market-specific grant/revoke orchestration、pricing/seat semantics、usage reservation/settlement event ingestion 和 operation ack，因此需要扩展为版本化 `market-gateway` contract，而不是让新 Market 直接依赖所有旧 `/v1/market/*` 路由。

Router 的 headroom/feedback 语义也必须保留：`ShareHeadroomRequest` 查询 `share_ids`，返回 `active_requests`、`parallel_limit`、`headroom`；429/quota feedback 会按 owner scope 施加带 TTL 的 penalty，而非只标记单个请求。

Router request-log sync 对 `request_id` 做 upsert，并按 `usage_revision` 合并较新的 usage 状态；这可作为新 Market event dedup 的参考，但不应直接当作资金结算唯一键。

### 3.5 `cc-switch-server` 的不可转移边界

Server 当前支持：

- Share descriptor/installation 同步和 strict revision/fingerprint；
- `market_access_mode`、public market identity、Router Share URL；
- Router Share Market managed grant 的 add/revoke、pending edit 和 ack；
- Router ingress v2 验签、Share identity → binding → Provider/Account 选择；
- `data_source = direct / market / router_share`、usage revision 和 request-log sync。

Server 不能因为换 Market 而：

- 把 Provider/OAuth token 复制到 Market；
- 允许 Market 直接绕过 Router 调 Server 推理端点；
- 信任普通 header 伪造 Share/installation；
- 让新 Gateway 自己再转发一次上游，造成“Market 转发 + Server 转发”双跳和重复 usage。

### 3.6 sub2api 的适配价值和原生模型

sub2api 是完整 AI API Gateway，而不是纯 Share Market。其 README/代码显示：

- Go/Gin/Ent，PostgreSQL 15+，Redis 7+；
- 用户/JWT/TOTP/Passkey/OAuth、API key、subscription、admin dashboard；
- Account/Channel、Group、模型路由和倍率、Claude/OpenAI/Gemini 兼容 Gateway；
- usage log、余额/quota/rate limit/concurrency、token/cost billing；
- EasyPay、支付宝、微信、Stripe、Airwallex 等支付、webhook、退款和支付审计。

`backend/internal/repository/usage_billing_repo.go` 的普通 usage billing 在 DB transaction 内以 `(request_id, api_key_id)` dedup，校验 `request_fingerprint` 冲突，并支持余额扣减以及部分 batch hold/capture/release；`usage_billing.go` 已有 request payload hash、token、cost、quota 等字段。这些是很好的基础设施参考，但其语义仍是“Gateway 请求完成后扣用户余额/订阅 quota”，不等于 Share seat/grant/Router contract。

如果不隔离 sub2api 的原生 Account/Channel/upstream credential/Gateway forwarding，将产生：

```text
sub2api 原生 Gateway 转发一次
      + Server 根据 Share 再转发一次
      + 两套 usage/余额记录
      + Provider credential 进入错误系统
```

因此定制版应先做领域裁剪，再引入 Share Market，不应把原生 upstream domain 当作 Server Share 的替代品。

### 3.7 许可、服务条款和商业风险

`sub2api/LICENSE` 是 LGPL v3（或更高版本），但 README/README_CN 同时声明上游 Provider TOS 风险和“无商业授权”。这不是可以由工程师自行解释的商业许可结论。

在 M0 之前必须取得法务/上游作者的书面判断，并建立：

- fork commit provenance、NOTICE 和 LGPL 对应源码/修改发布流程；
- Go/npm/镜像依赖许可证清单；
- Provider 官方 TOS、账号共享/反代和支付合规审查；
- 商业部署中可证明的源码提供、动态链接/衍生作品边界和商标使用规则。

在此结论完成前，sub2api 只能作为技术可行性候选，不能作为已获商业授权的事实写入发布计划。

## 4. 合理性评估

| 维度 | 判断 | 结论/前置条件 |
| --- | --- | --- |
| 产品解耦 | 高 | 独立 Market 的支付、用户和运营不应绑定 Server 发布节奏；值得拆分 |
| 基础设施复用 | 高 | sub2api 的 PostgreSQL/Redis、支付和 admin 可复用，但需替换 Share 领域 |
| 领域匹配 | 中/低 | sub2api 原生是 Account/Channel Gateway；Share/seat/entitlement 需重建 |
| 账务可靠性 | 中 | 可复用 transaction/dedup 思路，但必须重写 reserve/settle/provider payout 账本 |
| Router 集成 | 中/低 | 现有 Gateway 能做 inventory/headroom，缺 grant/revoke/event contract |
| 运行风险 | 中/高 | 双转发、双计费、stale entitlement、迁移余额不一致是阻断风险 |
| 合规/许可 | 未决 | LGPL 与“无商业授权”冲突表述需法务/作者确认；Provider TOS 另行审查 |
| 总体建议 | 条件可行 | 采用方案 A，先契约和影子运行，再切换写入权 |

### 4.1 预期收益

- Market 可独立扩容、部署、支付渠道和运营后台；
- Server/Router 不再依赖旧 Market 的 Rust schema/migration/secret；
- PostgreSQL/Redis 和事务型 usage dedup 更适合多租户资金平面；
- 可把 Share catalog、seat、grant、usage、payout 变成明确领域，而不是散落在 proxy handler 中；
- 未来可替换 Market 实现而不重新发布 Server 数据面。

### 4.2 不能低估的代价

- 需要新 contract、双写/对账和至少一个回滚窗口；
- 需要迁移余额、reserved、未结算请求、provider payable、API key 和 entitlement；
- 需要同时维护 Router/Server 兼容层，短期代码会增加；
- 需要重新实现旧 Market 的支付、风控、工单、对象存储和运营能力，而不是只移植页面；
- 需要解决 sub2api 的 license/TOS 和原生 Gateway 隔离问题。

## 5. 目标架构（推荐方案 A）

### 5.1 目标数据流

```text
consumer
   │ API key
   ▼
Custom Share Market（sub2api-derived）
   ├─ auth / API key / wallet / payment
   ├─ pricing snapshot / catalog / order / reservation
   ├─ grant orchestration / usage event / settlement / payout
   └─ signed market-gateway adapter
            │ Ed25519 + body hash + idempotency
            ▼
cc-switch-router（edge kernel）
   ├─ gateway identity / installation / tunnel lease
   ├─ Share entitlement + revoke enforcement
   ├─ online / headroom / concurrency / feedback
   └─ signed ingress
            ▼
cc-switch-server（data plane）
   ├─ Share binding / Provider / Account credential
   ├─ Claude/Codex/Gemini protocol adapter
   ├─ upstream forwarding
   └─ authoritative usage observation → event export
```

### 5.2 责任边界

| 能力 | Custom Market | Router | Server |
| --- | :---: | :---: | :---: |
| 用户、API key、钱包、充值、退款 | 权威 | — | — |
| 价格快照、目录展示、订单 | 权威 | 提供实时可用性 | 提供 capability evidence |
| Share/seat/listing 业务状态 | 权威镜像/业务 owner | edge 投影和 enforcement | Share binding 投影 |
| grant/revoke 编排 | 发起、重试、审计 | 接收、排队、edge 应用 | 应用到 Share descriptor 并 ack |
| Provider token/OAuth credential | 禁止保存 | 禁止保存 | 权威保存 |
| 协议转发 | 不做 | 只做 edge/tunnel/ingress | 权威转发 |
| headroom/online/feedback | 消费和缓存 | 权威计算 | 提供运行时信号 |
| token usage observation | 接收、去重、结算 | 传输/可观测镜像 | 权威观测源 |
| 余额/usage/provider earnings | 权威 | 若保留旧 seat billing，则只负责另一类账本 | 不记资金账 |

### 5.3 明确不做的事情

新 Market 第一阶段不做：

- 保存 Server provider token、OAuth refresh token 或 share secret；
- 复制 sub2api 的 Account/Channel 作为 Share；
- 直接代理 Server 的推理请求或绕过 Router ingress；
- 在 Market 和 Server 各自解析同一条 stream 并各自扣费；
- 让价格快照覆盖 Router 的实时 entitlement/revoke；
- 在没有对账和回滚窗口时删除旧 Market 数据库或 webhook。

## 6. M0 必须冻结的产品/法务决策

以下问题没有书面答案，不能进入代码实施：

1. 采用方案 A（保留 Router kernel）还是方案 B（重写 Router Market 边界）。本规划默认 A。
2. 新 Market 是否只做 Share Market，还是仍提供通用 Account/Channel API Gateway。若只做 Share，原生 Gateway forwarding 必须隔离或关闭。
3. token usage、按日/按时 seat、postpaid credit 是否同时存在；每种收费对象和账本分别是什么。
4. 新 Market 是否成为唯一资金/usage settlement authority；Router `market_billing` 保留、只读化还是迁出。
5. Market 与 Router 采用 Ed25519 Gateway identity，还是保留旧 session bearer 兼容期；建议新写路径使用 Ed25519。
6. 旧 users/API keys、余额、reserved、needs_review、provider payable、payout 和历史 usage 的迁移范围、停机窗口、保留期和回滚期限。
7. sub2api LGPL/README“无商业授权”边界、衍生代码发布义务、商标和 Provider TOS 的法务结论。
8. 旧 Market 支付渠道（Dodo、Gate.io 等）是否继续；新支付 provider、退款和 payout 的责任主体是谁。
9. Share listing/seat 的最终 owner 是 Market 还是 Router；Router 只能接受带版本的业务命令，不能出现双主写入。

## 7. `market-gateway` v2 契约规划

先写机器可读 JSON Schema、签名 fixture 和错误码，再实现任何 adapter。建议以 `/v2/market-gateway` 命名，旧 `/v1/market/*` 只作为只读/兼容入口。

### 7.1 建议接口

```text
GET  /v2/market/catalog
GET  /v2/market/shares/capabilities
GET  /v2/market/shares/headroom
POST /v2/market/grants
POST /v2/market/grants/{operation_id}/revoke
POST /v2/market/grants/{operation_id}/ack
POST /v2/market/usage-events
POST /v2/market/request-logs/batch
POST /v2/market/feedback
POST /v2/market/maintenance
```

实际部署可把 catalog/headroom/feedback 复用 Router 已有 Gateway route，但必须在 contract 中明确版本、scope、错误码和是否允许 stale 数据。

### 7.2 最小字段集合

```text
market_id, gateway_id, router_id, installation_id
share_id, capacity_pool_id, listing_id, seat_id
entitlement_id, operation_id, share_sequence
app, provider_family, capabilities, model_policy
online, parallel_limit, active_requests, headroom
share_revision, descriptor_generation, descriptor_fingerprint
request_id, usage_revision, usage_state
input_tokens, output_tokens, cache_creation_tokens, cache_read_tokens
status, status_code, timestamps, source, trace_id
```

不得把 provider credential、完整 request body、API key secret 或 payment token 放入 Router/Server 间的普通 inventory/event payload。

### 7.3 认证、重放和幂等

- Market 注册为 Router Gateway，使用 Ed25519 公钥；请求头至少包含 gateway id、timestamp、nonce、signature、body hash/action domain。
- 所有 grant/revoke 以 `operation_id` 幂等；同一 operation 的 payload 变化必须返回 conflict，而不是覆盖。
- usage 以 `request_id + usage_revision` 去重；revision 下降视为 stale，revision 相同但 fingerprint 不同视为冲突。
- settlement 使用独立、稳定且可审计的 `business_event_id`，不能只依赖 `request_id`。
- catalog 使用 `descriptor_generation + descriptor_fingerprint`；Router/Server 不接受跨 installation 的旧 descriptor。
- 设定时间戳允许偏差、nonce TTL、重试/backoff、dead-letter、顺序和迟到事件规则；撤销采用显式 grace period 和 ack timeout。

### 7.4 状态机和一致性

```text
Grant: pending → dispatched → active → revoke_pending → revoked
                         ↘ failed/retry/dead_letter

Usage: reserved → streaming → observed → settled
                           ↘ missing/parse_error/interrupted → needs_review
        reserved → failed_released
```

`observed` 只表示 Server 看到 usage，不等于已扣余额；只有 Market 的 settlement transaction 成功后才进入 `settled`。Router 的 request-log upsert、Server 的 usage revision 和 Market 的 ledger event 必须可以分别重试而不重复收费。

### 7.5 失败语义

必须区分：

- Router 不可达：新 grant 不得声称 active；请求是否 fail-closed 由已缓存 entitlement 的 TTL 决定；
- Server/上游中断：usage 进入 `needs_review` 或按明确 policy release；不得静默按 0 token 结算；
- Market 结算失败：保留可重放 event/outbox，不能让 HTTP 重试再次扣费；
- revoke ack 超时：edge 先按安全策略限制新请求，后台继续重试；
- catalog stale：可展示但不可发放新 entitlement，除非 contract 明确允许的 grace window。

## 8. sub2api 定制方向（只规划，不实施）

### 8.1 保留和复用

- Go/Gin/Ent 工程骨架、PostgreSQL migration/checksum；
- Redis cache/queue、分布式锁和 rate-limit 基础设施；
- 用户/JWT/TOTP/Passkey/OAuth、API key 管理；
- payment provider abstraction、webhook 验签/幂等框架、admin/audit；
- usage billing 的 transaction、fingerprint、金额量化和 circuit-breaker 思路；
- 监控、审计、备份和部署模板（经过安全审计后）。

### 8.2 隔离或关闭

- 原生 `Account`/`Channel`/`Group` 作为上游凭据和路由真值的领域；
- 原生 Gateway forwarding、upstream credential storage、模型 group routing；
- 会与 Server 重新转发或重复计算 usage 的 handler/worker；
- 把 sub2api 的用户余额直接绑定到 Server 本地 JSON store 的逻辑。

可以保留这些模块作为未来独立 Gateway 产品，但必须在编译/配置/部署层与 Share Market profile 隔离，默认 profile 不加载它们。

### 8.3 新增领域表（候选）

```text
router_installations
router_shares
share_listings
share_seats
share_entitlements
grant_operations
usage_events
usage_reservations
settlement_events
pricing_snapshots
provider_earnings
market_disputes
reconciliation_runs
outbox_events / inbox_dedup
```

所有金额使用固定 decimal/numeric 精度；所有外部事件有 source、schema version、business event id、received/applied timestamps 和审计 actor。

### 8.4 数据平面适配器

建议新 Market 只实现一个 `market-gateway` adapter：

```text
Market domain service
  → signed Gateway client
  → Router edge API
  → Router tunnel/ingress
  → Server Share
```

不要把 HTTP 调用散落在 billing、catalog、handler 中；adapter 应统一处理签名、超时、重试、trace、幂等 header、错误映射和 circuit breaker。

## 9. 分阶段实施路线

每阶段都要有可回滚产物；在 M5 之前旧 Market 继续是生产写入权威。

### M0 — 边界、许可和账本冻结

**产出**：决策记录、法务结论、责任矩阵、风险接受表、回滚窗口和数据保留政策。

**门禁**：方案 A/B、账本权威、收费模式、sub2api 原生 Gateway 是否隔离、旧数据迁移范围全部签字确认。

### M1 — `market-gateway` contract 和 fixture

**产出**：OpenAPI/JSON Schema、Ed25519/HMAC fixture、错误码、状态机、幂等键、时钟/重放规则、Router↔Server↔Market contract test harness。

**门禁**：覆盖 grant/revoke/ack、catalog、headroom、feedback、usage revision、duplicate/late/conflict、body tamper、clock skew、installation mismatch；只有 fixture/local 通过，不宣称真实 E2E。

### M2 — sub2api-derived Market 领域骨架

**产出**：独立 fork/provenance、Share catalog/seat/entitlement、wallet/reservation/settlement/outbox、provider earnings/reconciliation、admin audit 和最小 Market UI。

**原则**：先建新领域表，不直接把旧 SQLite 表或 sub2api Account/Channel 表当作最终模型；所有写路径走 domain service 和 transaction。

### M3 — Router adapter 与 Server compatibility layer

按以下顺序增加兼容能力，不删除旧 API：

1. 新 Market 只读同步 Router inventory、capability、headroom；
2. 新 Market 生成 shadow catalog，但不签发真实 grant；
3. Router 接收 v2 operation，映射到现有 `share_control_operations`；
4. Server 继续应用现有 managed grant/ingress，新增 v2 envelope 的兼容解析；
5. usage/request-log 只做双写或影子导入，按 event id 对账。

### M4 — 影子结算和差异检测

旧 Market 仍扣费，新 Market 对同一 request 做 shadow reservation/settlement，不改变用户余额。建立实时差异：

- catalog/capability/headroom；
- grant active/revoke 状态和 ack 延迟；
- request_id、usage_revision、token counts；
- reserved/settled/released 金额；
- provider payable、commission、risk_loss；
- webhook/payment/order 状态。

设置明确阈值；任何不可解释差异都阻止切换。

### M5 — 数据迁移和对账

采用“冻结写入短窗口 + 可重放导入 + 双向校验”，不要用一次性脚本覆盖目标库。详见第 10 节。

**门禁**：余额、reserved、有效 entitlement、未结算队列、provider payable、API key 使用权和审计记录达到逐项相等或有书面差异；备份恢复演练成功。

### M6 — 灰度和切换

推荐顺序：

1. 只读 inventory；
2. shadow catalog；
3. 单个 tenant/market canary；
4. 新 Market 获得 grant/settlement 写入权，但旧 Market 保持只读和对账；
5. 扩大流量，验证 payment webhook、余额、grant/revoke、usage、payout；
6. 完成一个完整结算周期后才关闭旧 Market 写入。

回滚条件包括重复扣费、余额差异、grant revoke 超时、usage event 丢失、Router/Server 5xx 或支付 webhook 不一致。回滚时先恢复 Market 写入权，再恢复 Router/Server adapter 旧路径，最后处理未决 event；不得直接回滚数据库快照覆盖新交易。

### M7 — 最终剔除

仅在至少一个完整账期和回滚窗口通过后：

- 下线旧 Market direct endpoint、credential、webhook、payout worker 和数据库写入；
- 删除 Server/Router 中**只服务旧 Market**的 compatibility adapter；
- 归档旧数据、schema、对象存储和审计，不立即物理删除；
- 更新部署、监控、备份、密钥轮换和事故 runbook；
- 运行全量本地/fixture/真实验收门禁并保留证据。

## 10. 数据迁移映射和对账

### 10.1 初步映射

| 旧 Market | 新 Market 候选 | 迁移要求 |
| --- | --- | --- |
| `users` / sessions | user/auth identities | session 不迁明文 token；强制重新登录或安全 hash 迁移 |
| `api_keys` | API keys | 仅迁可验证 hash/prefix/状态；无法证明 hash 兼容时轮换 secret |
| `user_cash` / `user_reserved` | wallet + holds/reservations | decimal 精度、冻结/可用余额必须逐用户相等 |
| `ledger_entries` | immutable ledger events | 保留原 business reference、actor、时间和 checksum |
| `processed_webhooks` / topup orders | payment orders + webhook inbox | source event id 唯一；重复、乱序、退款可重放 |
| `models` / prices / routing rules | pricing snapshots/catalog rules | 迁移为带 effective time/version 的快照，不覆盖历史价格 |
| `router_shares` / health | router inventory projection | Router 是 runtime source；Market 只存带版本的镜像 |
| listings/seats/subscriptions | listings/seats/entitlements | 映射 `listing_id`、`seat_id`、`entitlement_id`、`share_sequence` |
| `request_charges` | usage events + reservations + settlement events | `request_id` 不重复结算；`needs_review` 进入人工队列 |
| provider payable/payout | provider earnings/payouts | 总额、佣金、退款、争议逐 provider 对账 |
| request-log sync | event/outbox + Router projection | 只读历史日志不作为新账本重放 |
| object refs/attachments | object refs/retention | 先复制/校验 sha256，再切 reference；失败可重试/GC |

### 10.2 对账不变量

切换前至少验证：

```text
每个 user：available + reserved + disputed = 旧账本等值
每个有效 seat：old entitlement == new entitlement == Router active projection
每个 request_id：最多一个 settlement business_event_id
每个 provider：gross - commission - refund = payable
每个 usage revision：新值 >= 旧值；迟到事件不回退
每个 webhook：source event id 恰好一次 applied 或明确 rejected
```

历史 usage 建议先只读归档，不把无法确认来源/价格的旧记录重新放入新余额账本；否则会把迁移误差伪装成新交易。

## 11. 不能删除的内容

即使独立 `cc-switch-market` 被最终下线，也不能因“剔除 Market”删除以下 Server/Router 能力：

- Router installation identity、SSH/reverse tunnel lease、signed ingress v2；
- Server Share descriptor、binding invariant、Provider/Account credential store；
- `market_access_mode`（除非产品另有等价 ACL contract）；
- `ShareManagedGrantOperation`、`entitlement_id`、`operation_id`、sequence/revision 和 Server ack；
- Router edge 的 active request/headroom/concurrency、online/health、feedback penalty 和 revoke enforcement；
- Server usage observation、`usage_state`/`usage_revision`、request id 和审计；
- Router/Server 之间防重放、body hash、installation/share mismatch 校验；
- 备份、outbox/dead-letter、对账和事故恢复工具。

只有在新 Market 已提供等价、经过真实验收的 contract 后，才可删除 Router 中仅为旧 Market 服务的 session/endpoint 兼容代码。

## 12. 主要风险和缓解

| 风险 | 影响 | 缓解/阻断条件 |
| --- | --- | --- |
| 双重转发 | 上游成本、延迟、凭据泄露、usage 重复 | 新 Market profile 禁用原生 Gateway forwarding；端到端 trace 只能出现一条 Server upstream attempt |
| 双重计费 | 资金损失、投诉 | 单一账本权威；business event 唯一约束；shadow 对账为切换门禁 |
| stale grant/revoke | 已撤销用户继续调用 | Router edge TTL/deny policy、operation ack、dead-letter 和 revoke grace 明确化 |
| usage 丢失/乱序 | 少收或多收 | request_id+revision、outbox/inbox、needs_review 队列、人工 settle/release |
| 余额迁移错误 | 直接资金损失 | 冻结窗口、checksum、逐用户/逐 provider 对账、备份恢复演练 |
| Router/Market source-of-truth 冲突 | listing/capability 不一致 | Router runtime 与 Market business projection 分离；禁止双主写 |
| sub2api 依赖 fail-open | 超额扣费/绕过限流 | billing circuit breaker、资金路径 fail-closed、Redis 故障演练 |
| 密钥/凭据越界 | Provider 或支付账户泄露 | Market 不接收 provider token；分环境 secret、轮换、最小 scope |
| webhook 重放/伪造 | 充值或退款伪造 | 签名强校验、event id 唯一、金额/商户/订单绑定、隔离支付账户 |
| LGPL/TOS 不确定 | 发布/商业运营被阻断 | M0 法务签字；保留 notice/provenance；Provider TOS 单独审查 |
| schema/版本漂移 | 灰度期间不可回滚 | contract version、migration checksum、旧 endpoint 只读兼容、feature flag |
| 观测不足 | 切换后无法定位 | trace_id、operation/request/business event 三套关联键、账务和 edge 指标 |

## 13. 验收门禁

### 13.1 本地和静态门禁

按 `cc-switch-server/AGENTS.md`，代码实施后至少运行：

```bash
cargo fmt --check
cargo check
cargo test
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
scripts/smoke/smoke-local.sh
RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh
```

新 Market/Router contract 还应增加：

- schema/fixture 双向解析和 unknown field policy；
- signature/body/path/method/replay/clock skew；
- grant/revoke retry、ack、dead-letter 和 crash recovery；
- usage duplicate/late/conflict、ledger transaction rollback；
- payment webhook duplicate/refund/out-of-order；
- Postgres backup/restore、Redis unavailable、outbox replay；
- 负载下 headroom、concurrency、stale catalog 和 fail-closed 行为。

### 13.2 真实验收前置输入

真实 Router/Market/OAuth/share grant 输入齐备前，只能标记 `local/static/fixture passed` 或 `integration blocked`，不能写成真实通过。需要准备：

- Router URL、control secret、Gateway public key、installation 注册和 reverse tunnel；
- 新旧 Market API key/session、测试余额、价格快照、listing/seat/grant/revoke；
- Server Provider OAuth/API credential、真实 stream/usage/401/quota/error 样本；
- 支付 webhook secret、重复/退款事件、隔离 payout 账户；
- clock skew、备份/恢复目标、故障注入和回滚窗口。

### 13.3 切换成功标准

连续一个完整结算周期内：

- 无重复 settlement、无未解释余额差异；
- 所有 grant/revoke 在 SLA 内达到 ack 或明确失败终态；
- Router headroom/online 与 Market catalog 差异低于预设阈值；
- Server upstream 请求只发生一次，usage revision 单调；
- needs_review、退款、争议、payout 和 webhook 均可审计、可重放；
- 备份恢复和旧 Market 回滚演练成功。

## 14. 需要继续补充的设计文档

在开始实现前，建议分别建立：

1. `docs/market-gateway-contract-v2.md`：endpoint、scope、签名、schema、错误码和版本策略；
2. `docs/market-settlement-contract.md`：reserve/streaming/needs_review/settled/released、ledger event 和 risk policy；
3. `docs/share-entitlement-contract.md`：listing/seat/grant/revoke/ack、sequence/revision、stale/revoke grace；
4. `docs/market-data-migration-runbook.md`：冻结窗口、映射、checksum、对账、回滚和归档；
5. `docs/market-security-and-license-review.md`：sub2api、依赖、支付、Provider TOS、secret 和 threat model；
6. `docs/market-cutover-runbook.md`：灰度、监控、feature flag、故障处置和最终剔除清单。

这些文档仅作为历史评估保留；当前实施边界以
[`token-market-decoupling-plan.md`](token-market-decoupling-plan.md) 为准，本文不再产生
任何 fork、迁移、切流或替换任务。

## 15. 证据索引

以下路径是本次判断的主要证据（行号随代码变化，仅作当前基线定位）：

- 旧 Market：`/data/projects/cc-switch-market/README.md`、`src/db.rs`、`src/proxy.rs`、`src/router_client.rs`、`src/router_request_logs.rs`；
- Router entitlement/billing：`/data/projects/cc-switch-router/src/share_market.rs`、`src/market_billing.rs`、`src/market_access.rs`、`src/scheduling_signals.rs`、`src/api.rs`、`src/store.rs`、`schema/0001_baseline.sql`；
- Router Gateway：`/data/projects/cc-switch-router/src/api.rs` 中 `/v1/gateways/register`、`/v1/gateway/*` 和 `src/store.rs` 的 request-log upsert/revision 合并；
- Server contract：`/data/projects/cc-switch-server/src/domain/sharing/router_contract.rs`、`src/clients/router/client.rs`、`src/proxy/forwarder.rs`、`src/proxy/usage.rs`、`src/state.rs`；
- sub2api：`/data/projects/proxy/sub2api/README.md`、`README_CN.md`、`LICENSE`、`backend/internal/repository/usage_billing_repo.go`、`backend/internal/service/usage_billing.go`、`backend/ent/schema/`；
- 三方总体审计：[`system-audit-and-normalization-plan.md`](system-audit-and-normalization-plan.md)。
