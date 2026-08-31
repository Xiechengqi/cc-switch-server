# Qoder COSY 单账号反代

本文定义 cc-switch-server 的 `qoder_cosy` Provider 边界。一个 Qoder Provider 固定绑定一个 Qoder Account；Server 不实现账号池、轮询、权重、按配额/并发选号、自动切站，或 OAuth/PAT/其他 Provider 之间的 fallback。

## 站点与凭据 rail

- Global 与 China 是账号身份的一部分，分别固定到经过审计的 origin。请求中的模型、403、quota 或网络错误不能改变站点。
- Global/CN OAuth 通过受限 device flow 建立账号；Global 另支持显式导入 `pt-*` PAT。PAT 每次只为原绑定账号交换短期 job token，不进入 OAuth refresh rail。
- Provider binding 固定 Account id 与 `authIdentityGeneration`。session、catalog、quota、conversation 与短期 job token scope 同时包含 App、Provider revision/runtime fingerprint、Account、site、credential rail、auth generation 和 token generation。
- 第一个 eligible 401 只允许刷新原 OAuth Account，或为原 PAT 重新交换一次 job token；仅在下游提交前重放一次。第二个 401、提交后错误、generation drift 与站点漂移均为终态。

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
- session single-flight 创建并按完整 runtime/account/generation scope 缓存。conversation id 还包含 Share、签名用户与模型，复用的外部 session id 不能跨 scope 命中。
- Claude Messages、OpenAI Responses/Chat 和 Gemini 三类输入归一到同一 COSY conversation contract，再按原 Surface 输出。声明工具、reasoning、usage 和 terminal 事件使用同一个 model capability 与严格 SSE 状态机。
- 下游业务输出提交前的 401 可按上述一次预算恢复；提交后的 401、断流、重复或畸形 terminal 只能结束当前流，不能重放。

## 验收边界

48 项离线 Qoder 聚焦测试覆盖 Global/CN capability、公开 alias、权威空目录、坏目录、context/reasoning payload、clock skew、UUID nonce、三 credential rails × 三 Surfaces、session/provider/account generation fencing、pre/post-commit 401，以及连续两次 401 只恢复原绑定账号一次。

真实验收仍需分别提供 Global OAuth、CN OAuth 与 Global PAT 的脱敏 receipt，覆盖 login/import、refresh/job-token exchange、catalog、non-stream/stream、tools、reasoning、quota、首个/第二个 401 和 generation rotation。没有真实凭据时只能标记 `fixture_verified` / `live_pending`，不能标记 live verified。
