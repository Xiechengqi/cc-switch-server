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

## Provider 与账号固定

Share 请求的选择过程如下：

1. 从已验证 ingress context 解析唯一 Share，并读取其 Claude binding。
2. 编译后的 RuntimePlan 必须与该 Surface 的已提交 revision 一致。
3. 解析 Provider Bundle 的不可变账号绑定。
4. 检查账号登录状态、配额/冷却状态和并发上限。
5. 获取该账号的 in-flight lease 后开始转发。

Share 不存在、没有 Claude binding、Surface 已禁用、账号需要重新登录、账号处于冷却或并发已满时，请求直接失败。系统不会查找第二个 Claude Provider。

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

`/ready` 失败应阻止新实例进入负载，但不强制终止当前进程；内存中的新 token 仍可用于完成已有流量和后台落盘恢复。

## 真实账号验收

先确保待测 Share 的 Claude Surface 已绑定真实 OAuth 账号，然后运行：

```bash
CC_SWITCH_SHARE_URL='https://share.example.com' \
ROUTER_API_TOKEN='<router-user-token>' \
node scripts/smoke/claude-oauth-real.mjs
```

可选变量：

- `CC_SWITCH_CLAUDE_MODEL`：覆盖默认模型 `claude-sonnet-4-6`。
- `CC_SWITCH_REAL_TIMEOUT_MS`：单请求超时，范围 1 秒到 5 分钟。

验收依次通过同一个 Share URL 检查 count_tokens、非流式 Messages 和完整 SSE lifecycle。缺少 `CC_SWITCH_SHARE_URL` 或 `ROUTER_API_TOKEN` 时脚本明确输出 `SKIP` 并退出，不把缺少真实凭据记为通过。

## 非目标与剩余外部风险

- 不实现多账号调度、权重、健康故障转移或 quota spillover。
- 不保证上游账号本身的订阅状态、风控状态或区域可用性。
- 本地测试不能证明真实 Anthropic OAuth endpoint、真实模型权限和长时间流在生产网络中可用；这些只能由上面的 external gate 给出证据。
- persistence degraded 期间若宿主机在重试成功前崩溃，旋转后的 token 仍可能丢失；readiness 和告警用于缩短这一风险窗口，无法替代可靠磁盘。
