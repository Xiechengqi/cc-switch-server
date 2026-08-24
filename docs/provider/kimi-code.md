# Kimi Code 单账号反代

本文定义 cc-switch-server 的 `kimi_code` Provider 边界。目标是把一个 Kimi Code 账号通过 Claude、Codex 或 Gemini Surface 暴露给 Router Share；不实现账号池、轮询、权重、配额溢出、错误切号或跨 Provider failover。

## 固定身份

- 每个 Kimi Provider 必须显式绑定一个 `kimi_code` Account，并固定该账号的 `authIdentityGeneration`。
- Share 先选择 Surface 和 Provider，Provider 再解析唯一账号；请求 header、模型名和错误状态都不能改变这个选择。
- 首次 401 只允许刷新该绑定账号并重放一次。第二次 401、403、429、5xx、网络失败和提交后的流错误都是终态。
- 账号 identity generation 变化后旧 Provider 绑定返回冲突，必须由管理员显式重新绑定。
- OAuth 凭据继续由加密的 `accounts.json` 保存，Provider 不保存第二份 token。

## Device OAuth

控制面提供以下管理员会话接口：

- `POST /api/accounts/kimi/device/start`
- `POST /api/accounts/kimi/device/poll`
- `POST /api/accounts/kimi/device/cancel`

Device flow 使用固定 Kimi public client、`https://auth.kimi.com` 授权域和官方 device/token path。授权开始时生成一次随机设备身份，同一个 flow 的 start、poll 和最终账号 Profile 始终复用该身份。

- 每个 device code 只能有一个 poll lease；并发 poll 返回 in-progress，不会并行消费授权结果。
- Server 遵守上游 interval 和 `slow_down`，对缺省值、最大间隔、过期时间、30 秒请求超时和 256 KiB 响应体上限做有界归一。
- 完成登录必须同时得到 access token、refresh token 和 access-token JWT 中稳定的 `userId`。
- 账号 ID 从 `userId` 的哈希稳定派生。设备 ID 与该账号 Profile 一起持久化，后续 refresh 和推理继续复用。
- Cancel 删除当前 device flow；过期 flow 在访问时清理。所有 flow 写操作由 `state.rs` 域方法封装。

手工导入同样要求 access token 和 refresh token。能从 JWT 提取 `userId` 时使用稳定 principal；兼容导入无法提取时只用 token seed 派生本地 ID，首次可验证 refresh 仍必须建立稳定身份。

## Refresh 与设备身份

Kimi refresh 使用固定 token endpoint、client ID、Kimi CLI User-Agent 和账号 Profile 中保存的 `X-Msh-*` 设备头。

- Refresh receipt 必须包含 access token，并能提取稳定 `userId`。
- 新旧 `userId` 不一致时返回身份冲突并要求重新登录，不能把新 token 写入旧账号。
- Refresh token 轮换只更新当前账号；没有 replacement 时沿用原 refresh token。
- 缺失或非法的账号级设备身份会 fail closed，不临时生成另一设备冒充原账号。

## 数据面

上游 origin 固定为 `https://api.kimi.com`，但 wire 按 App 投影，而不是所有入口都使用 OpenAI Chat：

- Claude Messages：`/coding/v1/messages?beta=true`。
- Claude count_tokens：`/coding/v1/messages/count_tokens?beta=true`。
- Codex Responses/Chat：桥接到 `/coding/v1/chat/completions`。
- Gemini generateContent/streamGenerateContent：桥接到 `/coding/v1/chat/completions`。
- 模型目录：`/coding/v1/models`。

最终身份头包括 Bearer token、`KimiCLI/1.37.0`、`X-Msh-Platform: kimi_cli`、CLI version、设备名、设备型号、OS 版本和设备 ID。

账号 `extraHeaders` 不能覆盖 Authorization、User-Agent 或任何 `X-Msh-*` 设备身份头。最终 Kimi 身份在通用账号 header 合并后重新覆盖，避免历史配置改变认证设备。

模型采用权威发现 + fail-closed reviewed allowlist：

- `kimi-for-coding`、`kimi`、`kimi-code` 以及登记的 Claude 风格 aliases 映射为 wire model `kimi-for-coding`。
- `kimi-k3` 和 `k3` 映射为 wire model `k3`。
- single-model policy 先选择配置模型；passthrough 只允许上述登记 alias。未知模型直接返回 400，不透传到 Kimi。
- 模型目录按 App、Provider revision/runtime、Account、identity/token generation 隔离并 single-flight 获取。成功空目录是权威结果；只有完全相同作用域的可重试失败才可读取有界 stale cache。
- `/v1/models` 只在当前 Share 选中的 Kimi Provider 范围内公开上游目录与 reviewed allowlist 的交集 aliases，不参与 Provider 或账号选择。

## Thinking 与工具续接

- K3 reasoning effort 只允许规范化后的 `low`、`high`、`max`，缺省为 `max`；启用 thinking 时固定 `thinking.keep=all`。
- Claude Messages 使用 Kimi 所有的 `clear_thinking_20251015` edit；Chat bridge 只在 Kimi thinking 合同下回填 reasoning history。
- signed thinking replay 必须同时匹配 App、Provider revision/runtime、Account identity generation、Share、签名用户哈希、session 和 model family。缓存只接受带签名 thinking 与 tool-use 的完整 assistant turn，单条/总容量、block 数量和 TTL 都有上限。
- 非流与流式写入都在提交前重新验证 Provider 与 Account binding；流式只在 `message_stop` 提交，因此即使上游连接暂不 EOF，也能保存完整合法 turn。错误、未知 delta、截断或代际漂移不写入。
- 仅当本次确实应用 replay 且上游返回 400/422 时 CAS 删除对应旧值，避免一个失败请求清理另一会话或更新后的内容。

## 验收边界

32 项离线 Kimi 测试覆盖 device poll 串行化、slow-down、稳定设备身份、JWT principal、账号 Profile round-trip、header 最终覆盖、三个 App 的精确 endpoint、权威模型目录、模型 allowlist、K3 effort、thinking replay、同账号 401 和代际漂移。真实 Kimi Code 账号仍需分别验证：

- device 登录与 refresh-token 轮换；
- Claude/Codex/Gemini 三个 Surface 的非流和流式文本；
- tools、图片、首个 401、第二个 401、429 和中途断流；
- `kimi-for-coding`、`k3` 与未知模型拒绝；
- 日志、控制面和持久化文件不泄露 token。

缺少真实账号证据时只能标记 offline/fixture verified，不能标记 live verified。
