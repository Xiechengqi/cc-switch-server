# Client + Router 旧 Token Market 解耦整体 Review

> Review 日期：2026-08-18
>
> 范围：`cc-switch-server`（Client）与同级 `cc-switch-router`（Router）。
> `/data/projects/cc-switch-market` 未修改，也不再作为两者的运行时依赖。

## 结论

仓库内的旧 Router-local Token Market active path 已完成退役，当前产品边界是
“Client + Router”。Router 的 Share Market、Client Market、`market_access`、
`market_billing` 和 Share/Gateway 基础能力仍保留。

本轮 review 发现并修正了一个容易被遗漏的回流入口：`runtimeSnapshot` 是原始
JSON，单检查 Share 顶层字段会允许旧 Market/ACL 字段藏在任意嵌套对象中。现在
统一的递归 denylist 由 REST import、REST/invoke upsert、invoke import 以及
`ShareStore` 域写路径共同使用；命中后整个请求 fail-closed，不会自动清理并保存
用户输入。

这表示“仓库代码与文档完成解耦”可以通过本地门禁确认；不表示生产数据库、DNS、
证书、secret 或外部调用方已经完成迁移。

本轮收尾 review 另外修正了 Gateway 观测视图的身份投影：
`capacity_request_observations.user_email` 现在对 Gateway 行保持 `NULL`，
`gateway_id` 只留在 Gateway 专属列。这样 Gateway 观测仍可计入 Provider/Share
聚合总量，但不会被用户配额、用户用量或授权邮箱查询当成终端用户；对应的去重
查询和 schema/store 回归测试也已同步更新。

同一轮 privacy review 还收紧了 Gateway inventory：`GatewayShareView` 不再向 wire
序列化 Share/installation owner email、Provider account email 或 Provider API URL，
`shareName` 改为由 Share ID 派生的 opaque label。Share owner 只以 `serde(skip)` 的
Router 进程内字段参与 owner-scope feedback penalty，installation owner 已从 capacity
查询中移除；即使未来新增 tenant/seat grant 并开放 inventory，也不会因复用当前 view
自动暴露这些身份字段。

## Active path 检查

### 已确认不存在的旧实现

- Server 的 Token Market discovery、`list_token_markets`、旧 selector/query、
  Market public-host/claim 分支和独立 Market smoke/env。
- Router 的旧 registry、Market session/auth、Market tunnel/proxy、旧管理 API
  与 Markets 页面组件。
- 旧 Market URL 不是隐式落入 dashboard，而是统一返回 `410 Gone`；历史
  `/markets` 书签只安全跳转到 `/share-market/`。
- Share wire 已固定为 Contract v2：`contractVersion=2`、`freeAccess`、
  `userGrants`。

### 明确保留且不应误删的能力

- Router Share Market listing/seat/subscription/entitlement/grant/revoke。
- Router Client Market、`market_access`、`market_billing`。
- Client Provider、Account、Share、proxy、usage、Client/Share tunnel。
- Gateway Ed25519 注册、签名校验、opaque/脱敏 capacity view 和脱敏 observation
  基础。

`market_email` 在 `client_market.rs` 中仍表示 Client Market owner identity，
不是旧 Share sale 字段；Router frozen baseline schema 中的兼容列/表名也只在
迁移校验、备份清理和受控兼容读取边界出现。

## 受控残留分类

以下命中是有意保留、且不属于 active Token Market 实现：

| 类别 | 位置 | 处置 |
| --- | --- | --- |
| 一次性 Share 数据迁移 | `src/domain/sharing/legacy_token_market_migration.rs` | 启动时清理旧字段、迁移可识别 ShareTo、写非 PII receipt；完成后不再产生旧字段 |
| 统一输入 denylist | `src/domain/sharing/retired_fields.rs` | 仅用于识别和拒绝旧字段，递归检查 raw `runtimeSnapshot` |
| Router 迁移 19/20/21 | `cc-switch-router/schema/0019*`、`0020*`、`0021*` 与 schema verifier | 归档校验、策略互斥、最小 usage 迁移和物理删除旧表 |
| 负向 API 测试与 410 路由 | Server/Router tests 与 `src/api.rs` | 证明旧输入不会重新启用；返回明确退役状态 |
| frozen baseline | Router `schema/0001_baseline.sql`、受控 store/schema 代码 | 已发布 checksum 不改；新写入固定 canonical sentinel，未来另行做整表重建 |
| 历史说明 | `docs/*`、`server-pre-fix.md` | 均标注 historical/superseded，权威边界以本 review 与解耦计划为准 |

## 代码与文档残留复核

全仓扫描 `cc-switch-market`、Token Market、旧 endpoint、旧 host/tunnel、旧 Share
sale/ACL key 和旧表名后，没有发现新的 active writer、调用方或运行时依赖。现存命中
按以下边界处理：

- Server source 只剩一次性 `shares.json`/control DB 迁移、备份排除规则、递归
  denylist 及负向测试；`.env.example`、readiness 和真实验收不再接收独立 Market
  URL 或 credential。
- Router source 只剩 migration 19/21、frozen baseline compatibility columns、410
  retirement router 和负向测试；`market_email` 在 Client Market/metrics 迁移边界的
  命中分别是合法 Client Market owner identity 或待物理删除的旧 metrics 列。
- Server 当前文档以本 review、解耦计划、Share 访问策略和 Share/Gateway acceptance
  为准；`system-audit-and-normalization-plan.md` 与
  `market-replacement-sub2api-plan.md` 已明确标为 historical/superseded。
- Router 的 `README.md`、`PROTOCOL.md`、`ARCHITECTURE.md`、`UI_TEST_PLAN.md`
  只把旧路径作为 410、迁移、负向验收或“不得恢复”的说明；Share Market、Client
  Market、`market_access`、`market_billing` 仍是当前产品能力。

解耦审计脚本的 Share key literal 规则已与 Rust `RETIRED_SHARE_FIELDS` 对齐，覆盖
`acl`、Market identity/subdomain/URL/ID 和 sale kind 等此前只由 Rust 拒绝、JS
审计未完整扫描的字段，同时为 Client Market 的合法 owner metadata 保留窄兼容边界。

## 本轮修正的回流风险

旧字段 denylist 之前只覆盖部分顶层入口。现行规则如下：

1. 递归遍历对象和数组中的所有 JSON key，覆盖 camelCase 与 snake_case 旧字段。
2. REST `/api/shares/import` 在反序列化前拒绝；REST `/api/shares` 在域校验阶段拒绝。
3. Web invoke 的 upsert/save/import 在解析或调度前拒绝。
4. `validate_and_normalize_upsert_input` 与 `validate_share_import` 作为最后一道域边界再次拒绝，防止未来新增 API 绕过 HTTP 层。
5. 错误只记录字段路径，不保存请求原文、邮箱或价格；迁移 receipt 仍只包含 checksum/count。

因此，旧 UI 或旧客户端即使把字段放入 `runtimeSnapshot`，也只能得到 4xx，不能
把它重新写入 `shares.json` 或 Router Contract v2。

## 数据与兼容性结论

- Server `shares.json` 迁移是原子写回并重新解析验证；旧 archive 删除前先写
  `data-retirement-audit.json`，archive 有 checksum/文件数保护。
- Router migration 21 在物理删除前再次校验 migration 19 archive；校验失败时
  fail-closed，不记录成功版本，也不删除 archive。
- Router frozen baseline 不修改。其旧列仍是 schema 形状债务，不应被当作 active
  Token Market 兼容 API；后续若清理，必须另起 migration 重建整表并审计外键、
  trigger、index 与 Turso 升级路径。
- Gateway 尚未定义跨 Router tenant/seat/grant contract；普通 Gateway Share
  继续整体 fail-closed。当前 inventory wire 已移除 owner/provider identity，
  但这不等于未来独立 Token Market 已可用。

## 验证结果

本轮应至少通过以下仓库级门禁（真实输入缺失的项目保持 blocked）：

### Server

```text
cargo fmt --check                         pass
cargo check                               pass
RUST_MIN_STACK=67108864 cargo test ...    pass（lib 2406 passed、API contract 123 passed、lease fixture 1 passed；1 ignored）
provider/UI provider audits               pass
audit-token-market-decoupling.mjs         pass
scripts/smoke/smoke-local.sh              pass
RUN_REAL=0 scripts/smoke/code-agent-regression.sh
                                           local contract groups pass；真实矩阵输入缺失，gate=blocked
git diff --check                           pass
```

### Router

```text
cargo fmt --check                         pass
cargo check                               pass
cargo test ...                            pass（890 passed）
frontend npm run lint                      pass
frontend npm run build                     pass
git diff --check                           pass
```

`RUN_TESTS=0 RUN_REAL=0 scripts/release-readiness.sh` 仍会给出
`decision=blocked`：当前没有真实 Router/OAuth/Share/Provider 输入、生产部署验收、
备份恢复演练和 DNS/secret 变更授权。这是正确的安全结论，不是本地代码验证失败。

## 生产交接前置项

在任何生产发布或删除外部资源前，仍需由部署责任人完成：

- 旧 Market 流量、session、host、账务、通知、DNS/证书和 secret 盘点；
- Server/Router/metrics 备份及恢复演练；
- migration 19/20/21 在目标版本上的 dry-run/receipt 核对；
- Share tunnel、Client Market、Share Market grant/revoke 的真实 E2E；
- 未完成交易、退款、通知和外部调用方迁移确认；
- 回滚窗口结束后再清理旧 secret、DNS、证书和独立服务。

在这些输入完成前，状态应写作“repository/local/fixture verified，production
blocked”，不能写作“生产已剔除”。
