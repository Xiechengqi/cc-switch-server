# Router/Market 真实闭环验收

本流程只描述 `cc-switch-server` 侧操作和验收，不修改 router、market 或 cc-switch 代码。

## 前置条件

- server 已启动，能访问 `SERVER_URL`。
- 已完成 setup，并通过 password 登录拿到 bearer token。
- 真实 router/market 可访问。
- 允许创建测试 client、share、subdomain。
- 至少准备一个可用 Codex provider，或明确使用会返回可诊断错误的测试 provider。

## 环境变量

```bash
export SERVER_URL=http://127.0.0.1:15721
export CC_SWITCH_SERVER_TOKEN=...
export SHARE_ID=...
export DIRECT_SHARE_URL=https://share-subdomain.example.com
export MARKET_API_URL=https://market-api.example.com
export MARKET_URL=https://market.example.com
export ROUTER_API_TOKEN=...
export ROUTER_API_TOKEN_HEADER=Authorization
export STREAM_PROBE=0
```

`ROUTER_API_TOKEN_HEADER` 可选 `Authorization`、`x-api-key`、`x-goog-api-key`。默认使用 `Authorization: Bearer ...`。
`STREAM_PROBE=1` 时会额外执行 direct/market stream 请求；没有真实 provider/token 时保持默认 `0`。

## 执行顺序

1. `GET /health` 和 `GET /version`。
2. `GET /api/router/status` 和 `GET /api/router/diagnostics`。
3. `POST /api/router/register`。
4. `POST /api/router/client-tunnel/lease`。
5. `POST /api/router/batch-sync`。
6. `POST /api/router/share-edits/pull`，拉取并应用 pending share edits。
7. `POST /api/shares/runtime-snapshot`。
8. direct share URL 调 `/v1/responses`。
9. market api URL 调 `/v1/responses`。
10. `POST /api/usage/router-sync/retry`。
11. 导出 shares、provider health、usage logs。

推荐直接运行：

```bash
scripts/smoke/router-market-smoke.sh
```

## 字段对照

### Share descriptor

对照 server `/api/shares` 中 `runtimeSnapshot` 与 router/market 表格：

- `shareId`
- `app`
- `providerId`
- `providerType`
- `providerName`
- `accountEmail`
- `subscriptionLevel`
- `quotaPercent`
- `health`
- `appRuntimes`
- `appProviders`
- `appAvailability`

Ollama Cloud 等无百分比 quota 的 provider 不应显示伪造 `0%`，也不应按百分比参与排序或健康判断。

### Request log

同一请求三侧对照：

- `requestId`
- `shareId`
- `source` / `dataSource`
- `country` / `countryIso3`
- `userEmail`
- `requestedModel`
- `actualModel`
- `actualModelSource`
- `pricingModel`
- `statusCode`
- `inputTokens`
- `outputTokens`
- `cacheReadTokens`
- `cacheCreationTokens`
- `totalTokens`

server 只自动同步 `dataSource=direct` 的 share request log 到 router；market source 日志应由 market 侧负责，避免重复。

### Share edit / marketGrant

share-market grant 会在 router 中转成 pending share edit。server 侧通过两种方式处理：

- 后台监听 `/v1/shares/edit-events`。
- 手动调用 `POST /api/router/share-edits/pull`。

处理流程：

1. 用 installation identity 签名请求 `/v1/shares/pending-edits`。
2. 将 `ShareSettingsPatch` 应用到本地 share 的 owner、ACL、appSettings、forSale、price、limits、expiresAt、autoStart。
3. 同步更新后的 share descriptor 到 router。
4. 回写 `/v1/shares/edit-ack` 为 `applied` 或 `rejected`。
5. 更新本地 `marketGrant.status/grantId/lastError/updatedAtMs`，供 Web Share 页和 router descriptor 展示。

真实 add/revoke smoke 使用：

```bash
scripts/smoke/share-market-grant-smoke.sh
```

该脚本只调用 router 的 share-market grant API 和 server 的 share edit pull/API，不修改 router、market 或 cc-switch 代码。缺少 `SHARE_MARKET_GRANT_TOKEN`、buyer/listing/order 等真实输入时，它输出 `[BLOCKED]` 并写入脱敏 evidence；不会把 add/revoke 标记为通过。

## 通过标准

- router client 表中 0 share client 也显示在线/健康。
- `clientSubdomain.routerDomain` 能打开 server Web。
- router share 表字段与 server runtime snapshot 一致。
- market admin Shares 页能看到 app runtime、provider、model、quota、health。
- direct share URL 和 market api URL 都能命中正确 binding。
- request log 国家/IP/source 不丢，direct/market 不重复。
- share-market grant add/revoke 能通过 pending share edit 应用到 server share，并回写 ack。

## 阻断记录

如果失败，记录：

- 失败步骤。
- server 脚本输出。
- `/api/router/diagnostics`。
- `/api/router/tunnels`。
- `/api/shares`。
- `/api/usage/logs?limit=20`。
- router/market 对应错误响应或后台日志摘要。
