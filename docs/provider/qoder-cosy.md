# Qoder COSY 单账号反代

本文定义 cc-switch-server 的 `qoder_cosy` Provider 边界。一个 Qoder Provider 固定绑定一个 Qoder Account；Server 不实现账号池、轮询、权重、按配额/并发选号、自动切站，或 OAuth/PAT/其他 Provider 之间的 fallback。

## 站点与凭据 rail

- Global 与 China 是账号身份的一部分，分别固定到经过审计的 origin。请求中的模型、403、quota 或网络错误不能改变站点。
- Global/CN OAuth 通过受限 device flow 建立账号；Global 另支持显式导入 `pt-*` PAT。PAT 每次只为原绑定账号交换短期 job token，不进入 OAuth refresh rail。
- Provider binding 固定 Account id 与 `authIdentityGeneration`。session、catalog、quota、conversation 与短期 job token scope 同时包含 App、Provider revision/runtime fingerprint、Account、site、credential rail、auth generation 和 token generation。
- 第一个 eligible 401 只允许刷新原 OAuth Account，或为原 PAT 重新交换一次 job token；仅在下游提交前重放一次。第二个 401、提交后错误、generation drift 与站点漂移均为终态。

Device lifecycle 由官方 Global/CN `1.1.32` bundle 的固定摘要与 `assets/contract/qoder-cli-oracle.json` 共同冻结。两站都使用 UUID v4 nonce、S256 PKCE、1 秒 poll、300 秒 flow TTL，`GET /api/v1/deviceToken/poll` 的 404 表示 pending。Global machine ID 是 36 位小写 hex；CN machine ID 是 UUID v4，这两个格式不能互换。

账号 profile 读取、refresh/quota 与 session 创建共用同一个 site-aware machine-ID validator：Global 必须恰好 36 位且只能是小写 hex，CN 必须是 RFC 4122 variant 的 UUID v4；前后空白、错站格式、Global 大写 hex、错误 UUID version/variant 都在发网前拒绝。生成器仍分别生成对应格式，持久化数据不能借 `trim()` 或另一站格式蒙混通过。

两站 Device OAuth refresh 都固定为 OpenAPI `POST /api/v1/deviceToken/refresh`，body 只有 `refresh_token`，header 只有 JSON Accept/Content-Type 与 `User-Agent: qoder/1.1.32`，不得携带 Authorization、COSY identity、Proxy Authorization 或路由账号头。响应主字段是 `device_token`、轮换后的 `refresh_token` 与 `expires_at`；旧 Global center `/algo/api/v3/user/jobToken` 不是 fallback。轮换 receipt 必须先 durable commit，随后才允许 userinfo/CN auth-status 完成 identity closure；失败结果未知或 identity drift 时 fail closed。

## 动态模型目录与 capability

COSY model list 是当前绑定账号的权威 entitlement。成功空目录保持为空，不与 Provider 配置、静态 alias 或其他账号目录合并。目录条目必须是 object，包含非空 key；`enable` 若存在必须是 boolean。坏结构、超限响应、缺失字段和身份 scope 漂移均失败关闭。

Global/CN alias 表集中维护，但只有对应 live-enabled route 才会发布公开 alias。上游出现未知 live route 时保留其 exact id，便于观察新 entitlement，同时不猜测 reasoning、context、图片或其他能力。`/v1/models` 可返回：

- `reasoningEfforts`：按 site × route 的 reviewed matrix，仅发布 `none`/`low`/`high`/`max` 中已证明的组合；
- `contextWindow`：只在 route 有明确协议证据时发布；
- `inputModalities: ["text"]` 与 `supportsTools: true`；图片能力保持不发布；
- `source: qoder_live_model_catalog`、fresh `fetchedAtMs`，以及权威空结果。

推理 payload 与目录使用同一 capability resolver。fixed-context route 只写 `model_config.max_input_tokens`；runtime-selectable route 还同时写 `parameters.context_length` 和 `chat_context.extra.ideModelConfigOverride.max_input_tokens`。未知 route 不注入 context，未知 effort 不注入 reasoning 字段。

## COSY 签名、session 与流

- 请求使用 reviewed COSY body encoding、AES/WAF/signature 和固定 signature path。签名 timestamp 必须位于验证时钟正负五分钟内；请求 id 必须是规范 UUID v4 nonce。
- oracle schema v2 独立冻结三 rail 的精确 origin/path/profile、完整 required/forbidden signed-header 集、encoding/signature vectors、cli2api/server projection 与 accepted-difference 原因。canonical synthetic Chat 保存去随机 UUID 后的完整 server body；Global/CN 都由生产 builder 生成后做整棵 JSON exact equality，任意额外、缺失、改型或移位字段都会失败。
- session single-flight 创建并按完整 runtime/account/generation scope 缓存。conversation id 还包含 Share、签名用户与模型，复用的外部 session id 不能跨 scope 命中。
- Claude Messages、OpenAI Responses/Chat 和 Gemini 三类输入归一到同一 COSY conversation contract，再按原 Surface 输出。声明工具、reasoning、usage 和 terminal 事件使用同一个 model capability 与严格 SSE 状态机。
- authoritative finish reason 或上游 `[DONE]` 只标记候选终态。decoder 必须继续读取到上游 EOF，期间第二个 terminal、任何业务数据或 auth error 都失败；只有 EOF 验证通过后才向下游输出唯一一次完成事件。
- 下游业务输出提交前的 401 可按上述一次预算恢复；提交后的 401、断流、重复或畸形 terminal 只能结束当前流，不能重放。

## 验收边界

oracle `verification` 是验证计数单源：56 项离线 Rust Qoder 聚焦测试覆盖 Global/CN lifecycle exact HTTP、refresh rotation/receipt/error taxonomy、capability、公开 alias、权威空目录、坏目录、quota oracle、完整 payload/header、context/reasoning、clock skew、nonce/machine identity、三 credential rails × 三 Surfaces、EOF/唯一终态、session/provider/account generation fencing、pre/post-commit 401，以及连续两次 401 只恢复原绑定账号一次；8 项 Node mutation 固定 coherent projection、accepted reason、actual/signature path、header set、encoding/signature vector 与计数漂移；7 项 loopback real-harness fixture 验证 harness 合同。生成 coverage 直接读取 56/8/7，不再维护手写副本。

`scripts/smoke/qoder-real.mjs` 按 `global_oauth`、`global_pat`、`cn_oauth` 一次只运行一条 rail。它先核对三个 Surface Provider 固定同一 Account generation，再验收 fresh catalog、quota、三 Surface non-stream/stream/tool/usage/唯一终态，并把 commit、site/rail、generation、摘要与三个 decoy 零计数写入仓库外 0600 receipt。`scripts/audit/qoder-real.test.mjs` 的 loopback 结果固定为 `contract_verified/live_pending`，不能替代真实 receipt。

真实验收仍需分别提供 Global OAuth、CN OAuth 与 Global PAT 的脱敏 receipt，覆盖 login/import、refresh/job-token exchange、catalog、non-stream/stream、tools、reasoning、quota、首个/第二个 401 和 generation rotation。当前没有真实凭据或 receipt，只能标记 `fixture_verified` / `live_pending`，不能标记 live verified。
