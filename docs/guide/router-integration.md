# Router 联调

> 状态：**权威文档**。最后核对：2026-08-20。
>
> 协议与契约细节见 [`../architecture/router-contract.md`](../architecture/router-contract.md)；验收剧本见 [`../acceptance/router-share-acceptance.md`](../acceptance/router-share-acceptance.md)。
>
> 本文只涉及 Router 与 Router 内建的 Client / Share Market。独立 Token Market 服务已下线，Router 上 `/v1/markets*`、`/v1/market/*`、`/_market/proxy/*` 一律返回 `410 Gone`。

## 1. 联调步骤

1. 启动 server，打开 `http://server-host:15721` 完成 setup。
2. Router URL 填 router API base，例如 `https://router.example.com`。
3. setup 会同步执行 `register → owner bind → client tunnel claim`；子域名冲突会在初始化阶段直接报错。Router 不可达时允许完成本地 setup，但健康状态会提示隧道未注册。子域名留空时 server 会自动生成唯一名称。
4. 添加 provider 或 account 后创建 share；未填写 share subdomain 时，server 会自动生成。
5. 点击 share tunnel start 后，server 会 claim share subdomain、申请 `http` lease 并建立 SSH reverse tunnel。
6. share descriptor 会在创建、修改、删除时自动同步，并在 client 启动或重新注册 router 后自动校准，无需人工全量同步。
7. Router 内建 Share Market entitlement 会通过 pending share edit 下发；Server 后台监听 edit event，也可手动调用 `POST /api/router/share-edits/pull` 拉取并回写 ack。
8. router 可经 share tunnel 调 `/_share-router/health`、`/_share-router/request-logs`、`/_share-router/share-runtime`、`/_share-router/model-health` 拉取 runtime。
9. `/_ctl/apply_share_settings` 和 `/_ctl/refresh_share_usage` 使用 router `control_secret` HMAC、timestamp、nonce 防重放。
10. Router Share URL 请求由已验签 ingress context 中的 Share 身份选择 binding。Server 只同步 Router Share/Gateway observation 所需的脱敏 request log；Router migration 21 已将可安全关联的旧 usage 最小化迁入 canonical Share log，并物理删除旧 Market 明细与 archive。

## 2. 联调验收重点

- router client 表中 0 share client 也应显示在线/健康。
- router share 表能看到 server share 的 owner、subdomain、app runtime、provider 和 quota 展示字段。
- signed Gateway/Share route 能调度 server share（真实 Gateway 输入缺失时标记 blocked）。
- Router Share URL 能经 Router 调用 server Share，request log 不重复且保留 country/IP/source。
- Router 内建 Share Market entitlement add/revoke 能通过 pending share edit 幂等应用到 Server Share。

## 3. 相关 API

| 路径 | 说明 |
| --- | --- |
| `POST /api/router/register` | installation 注册 |
| `POST /api/router/heartbeat` | 心跳 |
| `GET /api/router/status` | Router 连接状态 |
| `GET /api/router/diagnostics` | 诊断快照 |
| `GET /api/router/tunnels` | 隧道列表 |
| `POST /api/router/client-tunnel/claim` | 申领客户端隧道子域 |
| `POST /api/router/client-tunnel/stop` | 停止客户端隧道 |
| `POST /api/router/share-edits/pull` | 手动拉取 pending share edit |
| `POST /api/shares/:id/tunnel/start` | 启动 Share 隧道 |
| `POST /api/shares/:id/tunnel/stop` | 停止 Share 隧道 |
| `POST /api/shares/tunnels/restore` | 恢复全部 Share 隧道 |

完整接口以 `src/api/mod.rs` 的 `app_router()` 定义为准。

## 4. 排障

- Router 不可达：setup 仍可完成，健康状态会提示隧道未注册；检查 `GET /api/router/status` 与 `GET /api/router/diagnostics`。
- 子域名冲突：初始化阶段直接报错，改用留空让 server 自动生成。
- ingress 验签失败：Server 返回空正文 `401`，并通过 `x-cc-switch-internal-ingress-*` 头向 Router 提供原因码与时间诊断。**这些内部响应头必须由 Router 剥离，不得传给公网调用方**；普通业务 `401` 不附该诊断头。
- 时钟偏移：ingress 新鲜度窗口非对称——最多接受 30 秒前签发、最多接受未来 5 秒签发。
- 远程环境调试见 [`remote-debugging.md`](remote-debugging.md)。
