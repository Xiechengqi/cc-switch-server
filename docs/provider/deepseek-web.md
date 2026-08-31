# DeepSeek Web Account 单账号反代

本文定义 cc-switch-server 的 `deepseek_account` Provider 边界。它只覆盖 `chat.deepseek.com` 的 Web bearer、session 与 PoW 协议，不等同于 `api.deepseek.com` 的 `deepseek_api` API Key；Server 不实现账号池、轮询、权重、配额/冷却/并发选号或跨 Provider、Account、credential rail fallback。

## 凭据与固定身份

- 每个 DeepSeek Web Provider 必须显式绑定一个 `deepseek_account` Account 及其 `authIdentityGeneration`。当前只有 Claude Messages Surface 是 Native；Codex 与 Gemini 保持 unsupported。
- 导入只接受一段原始 bearer access token，最大 16 KiB，不能包含空白、控制字符或完整的 `Bearer ` 前缀。`tokenType` 若存在只能是 `Bearer`。
- 导入拒绝 refresh token、ID token、API key、scope、extra headers，以及 `profile`/`raw` 中递归出现的 access token、Cookie、密码、session token 或其他 credential 字段。密码登录、浏览器 Cookie、自动刷新与替代凭据恢复均不存在。
- token 只进入加密的 Account 存储；Provider 不保存副本。`deepseek_api` 的 Provider-owned key 不能创建或填充 DeepSeek Web Account，Web bearer 也不能转成 API Key。
- Account identity 或 token generation 变化后，旧 Provider/runtime/session scope 立即失效；请求必须由管理员显式重绑或继续使用相同 Account 的新代际。

## 固定协议与 session

生产 origin 固定为 `https://chat.deepseek.com`，Account、Provider 设置和请求输入不能覆盖。一次生成依次执行：

1. 创建或复用当前 scope 的 `/api/v0/chat_session/create` session；
2. 从 `/api/v0/chat/create_pow_challenge` 取得仅针对 `/api/v0/chat/completion` 的 challenge；
3. 求解 `DeepSeekHashV1` 并调用 `/api/v0/chat/completion`；
4. 以严格终态状态机投影为 Claude JSON 或 SSE。

30 分钟 session cache 最多保存 256 个 scope，并以 single-flight 创建。scope 同时包含 App、Provider id/revision、runtime fingerprint、Account id、`authIdentityGeneration`、`tokenRefreshGeneration`、Share、签名用户、客户端 session/request id 与 reviewed model；任一维度漂移都不能命中旧 session。

只有“已经复用的 session”在 completion 返回 400、404 或 409 时，才可在下游提交前删除该精确 scope、用同一个 Account 新建 session 并重放一次。401、403、429、5xx、网络/协议错误、新 session 首次失败、第二次失败和任何下游业务输出后的错误均为终态。恢复路径不会读取第二个 Account、Cookie、API Key 或另一个 Provider。

## PoW、模型与响应语义

- PoW 只接受 `DeepSeekHashV1`、固定 completion target、64 位十六进制 challenge、有界 salt/signature、1–1,000,000 difficulty、30 秒过期时钟容差和最多 15 分钟未来 horizon；求解在 blocking worker 中执行。
- reviewed catalog 只发布 `deepseek-v4-flash`、`deepseek-v4-pro` 及其已审计的 thinking/search 变体。Claude Sonnet/Opus aliases 映射到该目录，未知模型 fail closed。
- `/v1/models` discovery 必须先复核当前 Provider 的显式 Account、identity generation 和非空 bearer；结果标记 `reviewed_deepseek_web_catalog` 与 `live_pending`，只表达 text、tools、thinking/search capability，不与静态、API-key 或其他账号目录合并，也不宣称动态 entitlement。
- 多轮 prompt 保留 system/user/assistant 文本、thinking、search citation、声明工具及 nonce-bound tool call/result 关系。图片保持 unsupported。
- non-stream 与 stream 都解析 reviewed content、fragment、status 和 search-result 形态。一个响应必须有且只有一个合法 terminal；截断 EOF、坏 JSON、terminal 后数据、重复或矛盾 terminal 均失败关闭。工具请求在完整收集和校验后才向下游发布，提交后不允许透明重放。

## Provider test 与验收边界

Provider test dry-run 只做本地检查：固定 origin、reviewed model、bearer-only materialization、Provider/Account binding 和 credential generation；不会联网。显式 network test 使用同一 Native session → PoW → completion → terminal 链，另设总 deadline 和 4 MiB 读取上限，结果以结构化 outcome 返回且错误脱敏。

本地 fixture 覆盖严格导入、PoW bounds、request/stream/thinking/search/tool/terminal、session scope、400/404/409 原账号单次重建、401 终止、Account generation drift、reviewed discovery 和 Native provider test。它们只能标记 `fixture_verified`。

真实验收仍需一个明确授权的 bearer fixture，分别记录以下脱敏 receipt：

- import、Provider binding、reviewed catalog 与 dry-run/network test；
- non-stream、stream、thinking、search citation、tool call/result；
- session 复用、失效 session 单次重建、401/403、429、5xx、PoW 过期/漂移与 token 撤销；
- 日志、API、错误和持久化文件不泄露 bearer、session id、PoW signature 或 prompt。

真实凭据未提供前保持 `fixture_verified` / `live_pending`，不得标记 live verified。
