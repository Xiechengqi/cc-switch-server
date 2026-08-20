# Share 访问策略与 Contract v2

> 状态：Server + Router 代码、迁移和本地测试已完成；真实 Router / OAuth / Share Market grant E2E 仍需独立输入与验收。
>
> 适用仓库：`cc-switch-server`（Client）与 `cc-switch-router`（Router）。
>
> 更新：2026-08-18。

## 1. 最终访问模型

普通 Share 只有两个 Owner 可编辑的访问状态：

| `freeAccess` | 访问语义 |
| --- | --- |
| `false`（默认） | 私有。只有 Owner、活动的人工 `role=shareto` grant，以及 Router Share Market 管理的有效 grant 可以调用。 |
| `true` | 公开免费。任何持有效 Router 用户 API Token 的已登录用户都可以调用；匿名请求仍被拒绝。若调用者另有活动 grant，个人 Token、并发、周期和到期策略仍优先生效。 |

`userGrants` 是授权用户、来源和个人配额的唯一真值。人工授权只通过“授权用户与配额 / 添加授权用户”维护；不存在独立的“授权邮箱”输入框。

Router Share Market 创建的 `manager=routerShareMarket` grant 由 Router 独占管理。普通 Share 编辑只能原样保留这些 grant；前端只读，Server 和 Router 后端均拒绝 Owner 伪造、修改或删除。

## 2. 活跃协议：Share Contract v2

Server 和 Router 的 Share descriptor 固定使用 `contractVersion=2`。正式访问字段只有：

- `freeAccess`：公开免费开关；
- `userGrants`：Owner、人工 ShareTo 与 Router Share Market grant；
- Share 总 `tokenLimit` / `parallelLimit`，以及 grant 内的个人 policy。

`ShareDescriptor` 和 `ShareSettingsPatch` 使用严格字段校验。以下 v1 字段不再属于 active wire、REST、invoke 或 UI contract：

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

Server 的 Share 导入和 invoke 边界对 camelCase、snake_case 两套退休字段都 fail-closed；Router 不接受带未知字段的 Contract v2 descriptor。它们不会被静默忽略，也不会继续作为兼容投影输出。

## 3. 一次性持久化迁移

### Server `shares.json`

加载旧 `shares.json` 时，Server 只在迁移边界识别 v1 字段：

1. 当 canonical `userGrants` 缺失或为空时，从旧 ACL/per-app 数据收集 ShareTo 邮箱；已有 canonical grants 时绝不让陈旧 ACL 覆盖它。
2. 仅当 `freeAccess` 缺失时，旧 `forSale=Free` 才迁为公开免费；旧 `Yes` 始终收窄为私有。
3. 删除全部退休字段，原子写回并重新解析验证；验证失败则启动失败，不带着半迁移状态运行。
4. 删除旧 `legacy-token-market-archive` payload，只保留不含 email/价格/凭据的 `data-retirement-audit.json`，记录 source SHA-256、受影响字段数和删除文件数。

这条迁移是一次性的持久化入口，不是 active API 兼容层。

### Server Router-control DB

本地 Router-control DB 的 schema v1 会先在事务中复制并校验旧 `public_hosts(kind=market)`；schema v3 随即在校验通过后物理删除临时 archive/manifest。最终 `public_hosts.kind` 只允许 `client` 和 `share`。

### Router 业务 DB

- migration 20 创建 `shares.free_access` 与 `share_access_policy_version`，迁移安全的旧 Free 状态，并安装 Free 与 Share Market entitlement 的双向互斥约束。
- Router 的 frozen baseline 已发布，`shares` 表中仍有旧兼容列。当前新写入把这些列固定清为 `[]`、`{}`、`selected`、`No`；所有 active 读取、授权和 wire 序列化只使用 `free_access` 与 `user_grants_json`。
- 旧列不是兼容 API，也不能授予访问。若以后物理删除，需要单独新增一次完整 `shares` 表重建 migration；不得修改 frozen baseline。

## 4. Share Market 互斥

“公开免费”与 Share Market listing/subscription 是两套不同分发机制，必须严格互斥：

- 有活动 listing 或非终态 subscription 时不能开启 `freeAccess`；
- `freeAccess=true` 时不能创建、重新激活或改绑 listing/subscription；
- 已排队但尚未由 Client 应用的“开启公开免费”编辑同样会阻止 listing，关闭异步控制面的竞态窗口；
- Router 业务事务先返回明确 conflict，migration 20 的 SQLite trigger 负责最终并发保护；
- 升级时，已有市场 entitlement 的旧 Free Share 迁为私有；只有没有 entitlement 的旧 Free 才迁为公开免费。

Share Market 自身的免费/付费 listing、seat、subscription、grant/revoke 和账务继续保留。它们不等于 `freeAccess`，也不恢复普通 Share 的旧“是否出售”字段。

## 5. 调用链

1. Router Share 公网入口始终要求有效的 `share:invoke` 用户 API Token；缺失或无效返回 `401`。
2. Router 校验目标 App 已绑定且启用。
3. 私有 Share 校验 Owner 或活动 canonical grant；公开免费 Share 允许任意已认证 Router 用户。
4. Router 把已验证、规范化的用户邮箱写入签名 ingress context。
5. Server 再次要求已认证身份；私有 Share重验 canonical grant，公开免费 Share 允许任意已认证身份。
6. Server 应用个人 grant 的到期、Token、周期和并发限制；Share 总限制始终是上限。

任何旧 ACL 数据库列、调用方自报邮箱、Gateway owner email 或匿名请求都不能绕过这条链路。

## 6. UI 规则

- Share 新建、Provider 快速开启和 Provider Bundle 新建一律默认私有；
- 普通编辑只显示“公开免费使用”复选框；
- 不显示“是否出售”、Market access、授权邮箱、官方价格百分比或 Token Market 选择器；
- 人工 ShareTo 只通过“添加授权用户”创建，并在同一处配置个人配额；
- Share 卡片显示“私有 / 公开免费”，授权摘要只从活动 `userGrants` 派生；
- Router Share Market grant 显示来源标记且只读。
- 用户周期用量的官方额度重置场景、`userUsageEdits` 保存协议和重基线公式见 [`share-user-usage-rebase.md`](user-usage-rebase.md)。

## 7. 回滚与验收

Contract v2 是明确的收口边界，不承诺回滚到仍依赖 v1 sale/ACL 字段的旧二进制。若必须回滚，必须恢复与旧二进制配套的完整备份并单独演练，不能依靠 Router frozen compatibility 列恢复旧授权语义。

本地门禁覆盖：

- v1 Free/Yes 的一次性迁移与退休字段物理清理；
- Contract v2 未知字段、REST/invoke 退休字段拒绝；
- Free/listing/subscription 双向 DB 阻断；
- canonical grants 覆盖陈旧旧列，旧列永不授予访问；
- 私有未知用户拒绝、公开免费已登录用户允许、匿名拒绝；
- 个人配额覆盖与 Share 总配额；
- Router-managed grant 的保留、只读和 revoke 生命周期。

缺少真实 Router、OAuth、Share Market grant 和 Client Market 输入时，只能记录本地、离线与 fixture 通过，不能标记真实 E2E 通过。
