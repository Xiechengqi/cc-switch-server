# Claude OAuth 单账号反代

本文描述 cc-switch-server 对 Claude OAuth 的生产边界、运行时约束和验收方式。目标是把一个明确绑定的 Claude OAuth 账号稳定暴露为 Anthropic Messages API，不提供账号池、负载均衡或跨账号故障转移。

## 能力边界

- 对外入口为 Router 暴露的 Share URL 下 `POST /v1/messages` 及对应的 `count_tokens` 路径。
- Share 精确绑定一个已启用的 Claude Surface；OAuth Provider Bundle 必须绑定一个明确账号。
- 同一个 Share URL 也可承载该 Bundle 已启用的 Codex/Gemini Surface。
- 不按并发、配额、健康度或错误类型切换到其他 Claude Provider 或账号。
- 可在同一 Provider、同一账号上执行文档中列出的有限重放。

这条边界用于避免一次生成请求被两个账号同时执行、重复计费、上下文漂移，以及刷新 token 在账号之间串用。

## 入口鉴权

客户端只连接 Router Share URL，并使用 Router 用户令牌。Router 转发到 Server 时附加签名 ingress context；Server 必须同时验证 Router 身份和非空 Share 身份。

- 未签名请求返回 `401`。
- 签名有效但没有 Share 身份的 client-lane 请求返回 `403`。
- 客户端提供的 `Authorization`、`x-api-key`、Share header 或 Provider header 都不能替代签名 context，也不能改写 binding。
- `15721` 不签发或接受本地推理 token，不提供 `/r/:key` Provider 入口；该端口只承载管理 UI、控制面、健康检查和 Router 内部 ingress。

Router ingress v2 使用 `cc-switch-router-ingress-v2` HMAC 域，并把 method、完整 path/query、实际发送 body 的 SHA-256 和每次请求唯一的 request ID 一起签入 context。Server 先验证 envelope HMAC 和 Router/installation 绑定，再按普通请求 2 MiB、媒体 32 MiB、Codex Images 48 MiB 的上限读取 body，随后验证 request binding 并登记最多 16,384 项的 replay cache；method/path/body 篡改或同 installation 下 request ID 重放均返回 `401`。replay cache 满载且没有过期项可清理时拒绝新请求，不驱逐仍在有效期内的记录。v1 只兼容到 `2026-09-08T00:00:00Z`（含边界时刻），之后 fail closed。

## Provider 与账号固定

Share 请求的选择过程如下：

1. 从已验证 ingress context 解析唯一 Share，并读取其 Claude binding。
2. 编译后的 RuntimePlan 必须与该 Surface 的已提交 revision 一致。
3. 解析 Provider Bundle 的不可变账号绑定。
4. 检查账号登录状态、配额/冷却状态和并发上限。
5. 必要时先完成该账号的 token refresh，再获取该账号的 in-flight lease 开始转发。

Share 不存在、没有 Claude binding、Surface 已禁用、账号需要重新登录、账号处于冷却或并发已满时，请求直接失败。系统不会查找第二个 Claude Provider。

首次上游 `401` 需要强制刷新时，当前 inference lease 会先释放；刷新成功后只为同一 Provider、同一账号重新获取 lease 并重放一次。这样 refresh 网络等待不占用生成并发槽，也不会借刷新机会切换身份。

Gemini 的首次 project discovery 使用独立 operation lock，不复用 token refresh 锁，也不在网络探测期间占用账号 inference lease。该隔离保证跨协议共用账号基础设施时不会因无关控制面操作阻塞 Claude 请求。

## OAuth 刷新一致性

Claude OAuth refresh token 可能在刷新成功时立即轮换，旧 token 随即失效。因此刷新提交遵循以下顺序：

1. 在账号单飞锁内调用 token endpoint。
2. 校验响应与账号身份，构造候选 AccountStore。
3. 原子写入完整 `accounts.json` 候选快照。
4. 写入成功后发布内存快照。
5. 如果写入失败，仍发布新的旋转 token，进入 persistence degraded，并返回 `503`。
6. 后台以 1 秒起步、最大 60 秒的指数退避重试完整账号快照。

降级状态使用带失败代次的原子状态。旧的成功重试不能清除更新一代的落盘失败，避免错误地恢复 readiness 后在进程重启时丢失最新 refresh token。

刷新 token 与 profile/quota enrichment 分离。profile 请求失败不会污染已经成功的 token 轮换提交。

OAuth endpoint fallback 只用于明确的 connect-stage 网络失败（请求尚未送达 token endpoint）或 endpoint unavailable 状态（如 404、405、410、501、502、503、504）。请求送出后的响应头/响应体读取失败不会 fallback，避免在服务端已轮换 refresh token 时用旧 token 再请求备用端点。`invalid_grant`、access denied、429 和其他确定性拒绝不会尝试第二个 token endpoint。`Retry-After-Ms`、秒格式 `Retry-After` 和 HTTP-date 均会被解析并限制在 24 小时内。

所有生产 token rotation 路径，包括请求前预刷新、`401` 强制刷新、后台 warm refresh 和控制面手动刷新，都进入同一个 detached owner 协调器。等待同一次 refresh 的请求被取消时不会取消真正持有 token rotation 的 owner。owner 有独立的 30 秒 deadline；panic、超时以及 token endpoint 已可能接收请求但 Server 未得到完整 receipt 的情况都收敛为 unknown outcome，立即隔离旧 refresh token 并要求重新登录，不会盲目重试。panic 恢复提交使用 refresh generation CAS，旧 owner 不能覆盖更新一代的成功 token 或失败隔离。进程收到退出信号后先停止接受新的 managed refresh，并最多等待 35 秒排空既有 owner。

## Wire profile、控制面与模型目录

Claude OAuth wire 身份固定在脱敏 capture `assets/contract/claude-oauth-wire-profile.json`，当前 profile 为 Claude Code `2.1.220`、Stainless `0.94.0`、Node `v26.3.0` 和 Axios `1.15.2`。usage 使用 `claude-code/2.1.220`，bootstrap/inference 使用 CLI identity，profile、roles 和 token endpoint 使用 Axios identity；CCH 在最终 body 归一化完成后计算，不猜测未被 capture 证明的私有 build suffix。

quota refresh 并行获取 usage、profile、bootstrap 和 `/api/oauth/claude_cli/roles`。四路 enrichment 都有超时和有界响应读取；roles/profile/bootstrap 失败只影响辅助证据，不覆盖已经成功的 token 或 usage quota，也不在日志、指标或公开 API 中暴露原始 OAuth body。

Claude Max 倍率按 usage、bootstrap、profile 和 canonical cache 的固定权威顺序解析。实时 `default_claude_max_5x` / `default_claude_max_20x` 会发布 `planType`、`planLabel`、`planSource`、`planStale` 和 `planObservedAt`。缓存倍率只可在同一 Max family 内细化实时 generic `Claude Max`，最多复用 24 小时；缓存回填继承原 `planObservedAt`，不会用本次 `queriedAt` 续命。超龄倍率退出候选，存在实时 generic Max 时立即降级显示为 `Claude Max`；quota 成功但实时响应没有任何有效计划证据、兼容缓存也已超龄时，refresh 会显式清除账号持久化的旧 `subscriptionLevel`，避免账号列表永久残留 5x/20x 标签。UI 对仍在有效窗口内的缓存计划显示缓存标识，真实 5x/20x smoke gate 则只接受非 stale 证据。

Claude OAuth 模型发现不调用 Anthropic，也不会为了列模型刷新 token。管理 discovery 和 Share `GET /v1/models?app=claude` 都使用同一份版本化静态目录：

- `claude-opus-4-6`
- `claude-sonnet-4-6`
- `claude-haiku-4-5-20251001`

Share 响应同时公开 `source=claude_code_wire_profile`、`stale=false` 和 capture 的 `fetchedAtMs`。更新 Claude Code wire profile 时必须一起 review capture、Rust 常量、beta 矩阵、endpoint identity 和模型目录，不能只改 User-Agent 字符串。

## 重放矩阵

| 场景 | Messages 生成 | count_tokens | Provider/账号 |
| --- | --- | --- | --- |
| 建连失败 | 最多重放 1 次 | 有界重放 | 原 Provider、原账号 |
| 401 且账号支持刷新 | 强制刷新后重放 1 次 | 强制刷新后重放 1 次 | 原 Provider、原账号 |
| OAuth compatibility 400 / signature 错误 | 有界 body 兼容重放 | 不适用 | 原 Provider、原账号 |
| header/首事件超时 | 不重放 | 有界重放 | 不跨账号 |
| 响应 body 读取失败 | 不重放 | 有界重放 | 不跨账号 |
| 429 / 529 | 不重放 | 不重放 | 原 Provider、原账号 |
| 已向下游提交 SSE 事件后的中断 | 不重放，发送终止错误帧 | 不适用 | 不跨账号 |

Messages 的建连重放只发生在请求尚未到达可产生计费副作用的阶段。任何不明确的发送或读取错误均按“可能已执行”处理。

## Anthropic 语义守卫

原生 Anthropic 响应在返回客户端前进行协议检查：

- 非流式 Messages 必须是 `type=message`，包含非空 `id`、assistant role、content 数组和非负 usage。
- `count_tokens` 必须包含非负整数 `input_tokens`。
- SSE 按字节增量解析，支持任意 chunk 边界、UTF-8、CRLF、comment 和多行 `data`。
- 流必须包含一个 `message_start`，业务事件只能出现在 start 之后，并以 `message_stop` 或 Anthropic `error` 结束。
- 空流、半帧、缺少终止帧、终止后的数据、事件名与 `data.type` 不一致及超过 8 MiB 的单事件都会失败。
- 下游已经收到事件后发生协议截断时，代理补发 Anthropic 终止错误帧，不透明重放整个请求。
- 收到 `message_stop` 后代理停止读取上游，及时释放连接和账号 lease。

客户端主动断开会取消上游 body，usage 日志记录为 `client_cancelled`，但不会把 Provider 标记为网络故障。

## 可观测性

- `GET /health` 始终反映进程存活；降级时 `status=degraded`。
- `GET /ready` 在 OAuth credential persistence degraded 时返回 `503`。
- `cc_switch_credential_persistence_degraded`：落盘降级 gauge。
- `cc_switch_stream_client_cancelled_total{app="claude"}`：客户端取消计数。
- `cc_switch_claude_retry_total`：按 stage/source 分类的 Claude 重放。
- `cc_switch_proxy_semantic_guard_total`：Anthropic 文档和流语义观察结果。
- `cc_switch_oauth_refresh_attempt_total` / `cc_switch_oauth_refresh_unknown_outcome_total`：有界 Provider/outcome 分类的刷新结果。
- `cc_switch_account_lease_total`：有界 Provider/result 分类的 lease 获取结果。
- `cc_switch_account_inflight{provider_type}`：按有限 Provider 类型聚合的当前账号请求数。
- `cc_switch_provider_outcome_total{app,provider_type,outcome}`：按有限 app、Provider 类型和结果聚合的上游终态。
- `cc_switch_claude_roles_total`、`cc_switch_claude_ttfb_seconds`、`cc_switch_claude_stream_duration_seconds`、`cc_switch_claude_semantic_failure_total`：roles enrichment 和流语义时延/终态。
- `cc_switch_claude_wire_profile_info`：当前固定 wire profile 的版本信息。

Prometheus 标签禁止包含账号 ID、Provider ID 或 request ID；这些实例标识只留在受访问控制且已脱敏的诊断数据中，不能进入长期时序基数。

`/ready` 失败应阻止新实例进入负载，但不强制终止当前进程；内存中的新 token 仍可用于完成已有流量和后台落盘恢复。

## 真实账号验收

脚本包含三个互相独立的 external gate：Share 推理、Max 5x 计划解析和 Max 20x 计划解析。完整运行示例：

```bash
SERVER_URL='https://server.example.com' \
CC_SWITCH_SERVER_TOKEN='<server-session-token>' \
CC_SWITCH_SHARE_URL='https://share.example.com' \
ROUTER_API_TOKEN='<router-user-token>' \
CLAUDE_OAUTH_MAX_5X_TEST_ACCOUNT='<account-id-or-email>' \
CLAUDE_OAUTH_MAX_20X_TEST_ACCOUNT='<account-id-or-email>' \
node scripts/smoke/claude-oauth-real.mjs
```

可选变量：

- `CC_SWITCH_CLAUDE_MODEL`：覆盖默认模型 `claude-sonnet-4-6`。
- `CC_SWITCH_REAL_TIMEOUT_MS`：单请求超时，范围 1 秒到 5 分钟。
- `ROUTER_API_TOKEN_HEADER`：Router 使用非默认鉴权 header 时覆盖，默认 `Authorization`。

Share gate 通过同一个 Share URL 检查 count_tokens、非流式 Messages 和完整 SSE lifecycle。两个 Max 变量分别按 Claude OAuth 账号 ID 或 email 精确匹配 `GET /api/accounts`，再调用 `GET /api/accounts/:id/quota?refresh=true&force=true`，检查账号/配额显示名、canonical `planType` / `planLabel` 以及 source/stale/conflict evidence 的一致性。脚本不输出账号选择器或完整 email。

三个 gate 单独判断输入：Share 缺 URL/token、某个 Max 等级缺账号选择器，或 Max gate 缺 Server URL/token 时，都为对应 gate 明确输出 `[SKIP]`，不阻止其他已配置 gate 运行，也绝不能把 SKIP 记为真实通过。

## 非目标与剩余外部风险

- 不实现多账号调度、权重、健康故障转移或 quota spillover。
- 不保证上游账号本身的订阅状态、风控状态或区域可用性。
- 本地测试不能证明真实 Anthropic OAuth endpoint、真实模型权限和长时间流在生产网络中可用；这些只能由上面的 external gate 给出证据。
- persistence degraded 期间若宿主机在重试成功前崩溃，旋转后的 token 仍可能丢失；readiness 和告警用于缩短这一风险窗口，无法替代可靠磁盘。
