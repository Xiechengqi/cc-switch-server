# Router 契约（Server 侧视角）

> 状态：**权威文档（Server 侧）**。线格式、字段定义与 Router 内部行为以 `cc-switch-router/PROTOCOL.md` 为准；本文只描述 Server 实现了什么、在哪里实现、以及 Server 侧的判定规则。
>
> 最后核对：2026-11-04。

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
| Share 市场最终撤权与消费回执 | `src/repository/server_sqlite/market_termination.rs`；[权威说明](../share/market-termination.md) |
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
| `/_ctl/preview_account_reset` | — | 只读账号周期能力/逐用户用量预览，带config CAS，不修改policy或消费券 |
| `/_ctl/preview_value_quota` | — | 只读 USD 额度生效／覆盖预览，带 config CAS，不创建计量起点 |
| `/_ctl/edit_value_usage` | — | 独立金额 set／clear 校准，带 actor、理由、窗口和水位 CAS |
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

Share runtime 的 `modelHealth` 必须报告实际检查状态，而不是 Provider 的调度资格：首个瞬态 429 即使仍允许调度、等待二次确认，也必须保留 failed、HTTP 状态与失败说明，不能投影为 success。最近记录不重复计入当前 snapshot 对应的同秒日志。Router 卡片另从当前代表 App、当前 probe epoch 的 immutable observation 生成只读 `modelHealthStatus`（绿／黄／红／灰及 65 分钟新鲜度），不使用主 App 的空记录冒充成功；runtime 摘要不得覆盖其固定槽证据。该修正不改变 Server 既有瞬态失败确认或调度容错。

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

## 7. Share 契约 v7 兼容与 v8/v9/v10 协商

Server 普通未协商 descriptor 保持 Share Contract v7；accountReset 及其只读投影须显式协商后写 v8，一次性市场试用须 `marketTrialV1`＋reader≥9 后写 v9，USD 等值配额须 `usdEquivalentQuotaV1`＋reader≥10 后写 v10。Router 滚动升级可读 v2..v10，Share Market 最低版本仍 v5。`cacheCreationTtlV1` 为独立日志能力。各版本在 v2 的统一 Share 访问边界上逐步追加：

- v2：`freeAccess`（默认 `false`，即私有）、`userGrants`、`tokenLimit`、`parallelLimit`。
- v4：不含凭据的 Provider 运行时探针描述 `appRuntimes.*.modelProbe` / `appProviders.*.modelProbe`。它包含 Server 权威测试模型、wire model、结构化请求、响应完成模式、payload revision 和 health fingerprint。Router 的 Curl、端到端测试、半小时定时探针不得另行维护硬编码模型或请求模板。
- v5：Grok 媒体权限 `grokMediaPolicy`（图片生成、图片编辑、视频生成三个独立开关，缺失即全部关闭）。
- v6：scoped quota metadata。
- v7：quota 中独立的 `unobservedTiers`（套餐已确认但尚无真实数值观测的容量池，只用于展示），以及仅 Codex OAuth 使用的 `bankedReset` 摘要。二者都不参与调度、限流、可用性或容量评分。
- v8：`accountReset`、`accountQuotaCycle/accountResetCapability/accountUsageBaseline`和usageWatermark。heartbeat须同时声明shareContractVersion≥8与accountResetV1；Server能力缓存绑定Router URL/installation且300秒失效。普通v7省略新嵌套字段，市场合同不新增动态周期。
- v9：一次性市场试用 `marketEntitlements` 与 managed operation 的 phase、scope、原始 Token allowance／本次剩余 cap。转发前持久化请求阶段，累计修订不能倒退或复活预算；正式额度不接收试用迟到用量。使用量变化不提升静态 generation，但持续进行完整 descriptor 同步。已有 v9 状态而 Router 不再声明能力时明确拒绝降级发布，不伪装成 v7。见 [市场试用](../share/market-trials.md)。
- v10：普通 grant 的 `policy.valueQuota` 与 `tokenLimit` 互斥，Server-owned `valueMetering/pendingValueQuota` 定义 epoch、保留的计量起点、冻结价目和安全排期。动态金额不进入 descriptor 指纹，走独立带 ACK 的签名水位；能力丢失不降级为不限额，只隔离金额 Share 同步。执行快照 fetch／ack 均为 installation 签名 POST，不是公开 GET。Share Market managed upsert 也可携带 `valueQuota`（任意市场阶段，ledger scope 包含 entitlement、从首次 USD 应用起计量、不排期、不可人工校准），新试用可携带成对 `trialValueAllowanceUsdMicros/trialValueLimitUsdMicros`，累计 `trialValueUsedPicos` 与 Token 共用 usageRevision。Server 在金额预览 `checks[]` 中以常驻 ready 的 `market_seat_quota`／`market_trial_value` 声明席位／美元试用能力；试用在周期准入同一事务内附加合同和 scope 的累计／在途检查。详见 [双口径额度](../share/value-quotas.md)。

账号周期动态视图不进入静态fingerprint/configRevision/generation；当前复用完整descriptor同步/ACK，Notify及30秒周期持续重放，并非独立精确quota水位ACK。Router按stream、source/epoch、cycle顺序/修订与usage水位合并，允许合法下降修正，不用历史max复活旧周期；恢复换stream全量握手并退休旧流。详情和运维限制见[账号重置周期](../share/account-reset.md)。

Router 的 quota 解析拒绝未知字段，因此契约扩展必须先升级 Router，再升级写出新字段的 Server。Share 控制面线格式及严格性规则以 `cc-switch-router/PROTOCOL.md` §§7、12 为准。

以下 v1 字段**已退役，且对 camelCase 与 snake_case 两种写法都 fail-closed**（`src/domain/sharing/retired_fields.rs`）：
`acl`、`forSale`/`for_sale`、`officialPricePercent`、`forSaleOfficialPricePercentByApp`、`sharedWithEmails`、`marketAccessMode`、`accessByApp`、`appSettings`。

访问模型细节见 [`../share/access-policy.md`](../share/access-policy.md)。

独立展示扩展 `providerIdentityV1` 不提升 Share contractVersion。Router heartbeat 显式
声明后，Server 才在 `appProviders` 写出真实 Family、单一分类、保存的名称/图标/颜色和
catalog 版本。所有出站路径均省略旧 reader 不认识的字段；能力变化自动补发已有分享。
该内容推进 descriptor ACK，不改变配置 revision、执行指纹或租约快照。
详见 [供应商展示身份](../provider/presentation.md)。

Buddy 私有账号卡片第二行使用既有 `accountLabel`，与 Server 的账号 `displayName`
共用 `buddy_account_display_name`：依次选择邮箱、脱敏手机号、昵称／姓名、内部账号 ID。
标签只来自精确绑定托管账号；账号缺失或身份代次不匹配时
省略标签，Router 显示 `-`，不回退产品名或分享者邮箱。此修正不新增协议字段；
公开 Share Market 的 Buddy `accountHint` 省略，不对外发布账号显示资料或内部账号 ID。

独立展示扩展 `buddyQuotaPresentationV1` 要求 reader≥7 且显式能力，不改变普通
descriptor 或市场最低版本。Server 在 Buddy quota 输出独立的 `providerUsage` 白名单汇总、
资源包官方名称及可选 `tiers[].expiresAt`；后者与周期结束 `resetsAt` 分开。
所有出站路径对旧 reader 省略新增字段，能力绑定 URL/installation 并在 300 秒后失效，
能力变化强制回填已 ACK 的 Share。单纯账单变化仅更新动态快照和完整 descriptor，
不提升 configRevision/静态 generation/fingerprint；失败沿用既有同步错误与周期重试。

汇总仅含状态、来源、账号口径、时区、采集时间、至多 31 日的日期范围、今日/7日/本月/范围总
credits 和范围请求数。缺失或畸形数据降为 unavailable，不填零；请求/模型明细、prompt、
账号标识和原始错误留在 Server 管理边界。默认卡片显示各资源包已用/总额及简短倒计时，
随后同一段文字显示今日/本月官方 credits，不固定换行。倒计时优先取 `resetsAt`，缺失才取
`expiresAt`，无有效未来时间则省略。采集时间、完整期限、7 日用量和请求范围保留在线格式中，
不在卡片展开。
保留官方明细既有 30 分钟成功缓存及手动强刷。两套账号账本与 Share 的 Token/USD 配额分开，
不要求金额相等，不参与调度、限流或容量评分。详见 [Buddy 展示合同](../provider/codebuddy-oauth.md#2026-11-03-资源包与官方请求用量展示)。

## 8. 已下线接口

Router 上的独立 Token Market 接口 `/v1/markets*`、`/v1/market/*`、`/_market/proxy/*` 一律返回 `410 Gone`（Router migration 19 归档、21 物理删除）。Server 侧对应的迁移代码保留在 `src/domain/sharing/legacy_token_market_migration.rs`，仅用于一次性历史数据清理，**不是**可用能力。

## 9. 联调与验收

- 联调步骤：[`../guide/router-integration.md`](../guide/router-integration.md)
- 验收剧本：[`../acceptance/router-share-acceptance.md`](../acceptance/router-share-acceptance.md)
- 上层总览：[`overview.md`](overview.md)
