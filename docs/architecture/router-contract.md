# Router 契约（Server 侧视角）

> 状态：**权威文档（Server 侧）**。线格式、字段定义与 Router 内部行为以 `cc-switch-router/PROTOCOL.md` 为准；本文只描述 Server 实现了什么、在哪里实现、以及 Server 侧的判定规则。
>
> 最后核对：2026-08-24。

## 1. 实现位置

| 关注点 | 代码 |
| --- | --- |
| Router HTTP 客户端、签名请求、注册与心跳 | `src/clients/router/client.rs` |
| SSH 反向隧道生命周期与 `tunnels.json` | `src/clients/router/tunnel.rs` |
| IngressContext 校验与重放防护 | `src/clients/router/ingress.rs` |
| Router 控制面本地状态 | `src/clients/router/control_store.rs` |
| 邮箱验证码登录（Router 代发） | `src/clients/router/email_auth.rs` |
| `/_ctl/*` 控制面端点 | `src/api/control/ctl.rs` |
| `/_share-router/*` 探针 | `src/api/control/share_router.rs` |
| Share 契约不变量与退役字段 | `src/domain/sharing/router_contract.rs`、`retired_fields.rs` |
| 客户端子域申领与隧道供给编排 | `src/client_tunnel_provision.rs` |

协议纪元常量：`PROTOCOL_EPOCH = "namespace-flat-1"`（`src/clients/router/ingress.rs`）。

## 2. 身份

Server 持有两类 Ed25519 身份（详见 `PROTOCOL.md §2`）：一类用于 installation 注册与控制面签名，一类用于隧道/入口链路。私钥与 `control_secret` 存放在 `server.json`，`cc-switch-server config print` 的脱敏摘要**不得**打印二者。

## 3. Router → Server 控制面

Router 主动调用 Server 的 `/_ctl/*` 端点：

| 路径 | 常量 | 用途 |
| --- | --- | --- |
| `/_ctl/apply_share_settings` | `APPLY_SHARE_SETTINGS_PATH` | 下发 Share 设置变更 |
| `/_ctl/refresh_share_usage` | `REFRESH_SHARE_USAGE_PATH` | 触发 Share 用量刷新 |
| `/_ctl/client-subdomain-adoption/prepare` | `PREPARE_CLIENT_SUBDOMAIN_ADOPTION_PATH` | 子域接管：预备 |
| `/_ctl/client-subdomain-adoption/commit` | `COMMIT_CLIENT_SUBDOMAIN_ADOPTION_PATH` | 子域接管：提交 |
| `/_ctl/client-subdomain-adoption/abort` | `ABORT_CLIENT_SUBDOMAIN_ADOPTION_PATH` | 子域接管：回滚 |
| `/_ctl/logs/tail` | `CLIENT_LOG_TAIL_PATH` | 拉取客户端日志尾部 |

鉴权为 `control_secret` HMAC + 时间戳 + nonce，三者缺一不可。常量定义在 `src/api/mod.rs:141-146`，与 Router 侧必须同步修改。

## 4. `/_share-router/*` 探针

| 路径 | 方法 | 用途 |
| --- | --- | --- |
| `/_share-router/health` | GET | 健康探测 |
| `/_share-router/request-logs` | GET | 请求日志查询 |
| `/_share-router/share-runtime` | GET | Share 运行时快照 |
| `/_share-router/model-health` | POST | 兼容的单 Share 模型探针 |
| `/_share-router/model-health/batch` | POST | 模型健康批协议 v1（滚动回退） |
| `/_share-router/model-health/batch-v2` | POST | 当前模型健康批协议 v2 |

单目标请求使用 `x-cc-switch-share-id` 定位 Share；batch 从 HMAC 签名 body 的 `targets[]` 定位，不依赖单 Share 头。v2 要求 `contractVersion=2`，按 Provider runtime 去重执行并返回稳定的 SHA-256 `observationId`、outcome、failure domain、reason 和 evidence version。失效 Share 目标或单个 Provider 组异常只省略对应结果，不中断其他组。

Router 半小时探针与“连接”弹窗共用 Share Contract v4 的 `modelProbe` 测试模型和 Provider 策略，但执行范围不同：前者是 Server 内部 Provider 测试，后者经公开 Share URL 做端到端连接测试。Server 本地半小时 scheduler 在 45 分钟内已有 Router cycle 时跳过相同 Provider。

## 5. IngressContext

Router 在数据面请求上注入两个头：

- `x-cc-switch-ingress-context`：上下文载荷
- `x-cc-switch-ingress-signature`：签名

Server 侧校验（`src/clients/router/ingress.rs`）：

- **新鲜度窗口是非对称的**：过去方向最多 `DEFAULT_MAX_CONTEXT_AGE_MS = 30_000` ms，未来方向最多 `DEFAULT_FUTURE_CLOCK_SKEW_MS = 5_000` ms。
- 重放缓存上限 `MAX_REPLAY_ENTRIES = 16_384`，条目保留时长为 `30s + 5s`。
- `request_id` ≤ 128 字节；`path + query` ≤ 16 KiB。
- 校验失败通过内部头回传诊断：`x-cc-switch-internal-ingress-error`、`x-cc-switch-internal-ingress-age-ms`、`x-cc-switch-internal-ingress-server-time-ms`。

## 6. 请求体上限协商

Router 通过**未签名**头 `x-cc-switch-ingress-body-limit` 声明其上限；Server 的生效值为 `min(本地 requestBodyLimits, Router 声明)`。本地值存于 `server.json` 的 `requestBodyLimits`。

## 7. Share 契约 v4

v4 延续 v2 的统一 Share 访问边界：

- `freeAccess`（默认 `false`，即私有）
- `userGrants`
- `tokenLimit`
- `parallelLimit`

并新增不含凭据的 Provider 运行时探针描述 `appRuntimes.*.modelProbe` / `appProviders.*.modelProbe`。它包含 Server 权威测试模型、wire model、结构化请求、响应完成模式、payload revision 和 health fingerprint。Router 的 Curl、端到端测试、半小时定时探针不得另行维护硬编码模型或请求模板。

以下 v1 字段**已退役，且对 camelCase 与 snake_case 两种写法都 fail-closed**（`src/domain/sharing/retired_fields.rs`）：
`acl`、`forSale`/`for_sale`、`officialPricePercent`、`forSaleOfficialPricePercentByApp`、`sharedWithEmails`、`marketAccessMode`、`accessByApp`、`appSettings`。

访问模型细节见 [`../share/access-policy.md`](../share/access-policy.md)。

## 8. 已下线接口

Router 上的独立 Token Market 接口 `/v1/markets*`、`/v1/market/*`、`/_market/proxy/*` 一律返回 `410 Gone`（Router migration 19 归档、21 物理删除）。Server 侧对应的迁移代码保留在 `src/domain/sharing/legacy_token_market_migration.rs`，仅用于一次性历史数据清理，**不是**可用能力。

## 9. 联调与验收

- 联调步骤：[`../guide/router-integration.md`](../guide/router-integration.md)
- 验收剧本：[`../acceptance/router-share-acceptance.md`](../acceptance/router-share-acceptance.md)
- 上层总览：[`overview.md`](overview.md)
