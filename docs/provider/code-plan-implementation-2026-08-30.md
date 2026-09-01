# Code Plan low-score implementation loop (2026-08-30)

本页是 `/data/projects/proxy/proxy.md` 新一轮评分表的实施真值。范围是循环启动时 `cc-switch-server` 分数低于 9 的全部 Code Plan，唯一排除项是“其他 IDE / CLI 订阅”。每一项按“最高分参考源码 → Server 差距 → 可落地设计 → 代码/合同 → 专项测试 → live gate”闭环；本地 fixture 通过不冒充真实订阅验收。表中“起始分”保留进入循环时的分数，避免已完成增强后项目从范围中消失。

## 共同不变量

- managed Provider 固定绑定一个明确 Account；Provider-owned secret 固定绑定一个 credential generation。
- discovery、quota、session、thinking replay、媒体任务和 Web session 状态均必须带 Provider/runtime 与 Account/credential generation。
- 首个 eligible 401 只可刷新同一个绑定账户并在下游提交前重放一次；第二个 401、提交后失败和代际漂移均为终态。
- 不实现 pool、round-robin、quota/cooldown/concurrency 选号、跨账号或跨 Provider fallback。
- 动态目录可返回当前绑定的权威空结果；只有同一身份 scope 的缓存可在明确的 transient failure 后标为 stale，不能与静态或其他账户目录求并集。
- 外部项目是协议证据，不是可整仓同步的实现来源。每个 endpoint、credential rail 和模型 entitlement 都必须在 Server 重新建模并独立测试。

## 实施队列

| 顺序 | Code Plan | 起始分 | 最高分参考 | 本轮选择 | 状态 |
| ---: | --- | ---: | --- | --- | --- |
| 1 | Gemini CLI / Code Assist | 8.6 | OmniRoute 9.3 | 绑定账户 quota-backed 动态模型目录、同代际 stale 语义、差分 schema/signature fixtures | 本地完成；live pending |
| 2 | Antigravity / Agy | 8.9 | TokenRouter 9.7 | authenticated catalog、weekly quota/capacity evidence、mixed-tool/terminal 差分 | 本地完成；live pending |
| 3 | Grok / xAI / Grok Build | 8.8 | TokenRouter 9.7 | text/WS/media/search capability manifest、异步媒体 ownership/terminal 合同 | 本地完成；live pending |
| 4 | GitHub Copilot | 8.8 | OmniRoute 9.4 | GitHub/GHES model capability 与 endpoint provenance、三 Surface live harness | 本地完成；live pending |
| 5 | Kiro | 8.8 | OmniRoute 9.2 | 强化 Kiro 的 Provider/runtime/account/generation 目录边界；不把 Amazon Q 别名计入 Kiro 能力 | 本地完成；live pending |
| 6 | Cursor | 8.7 | OmniRoute 9.3 | fresh live-catalog 完整模型 ID、bounded stale、ServerConfig duplicate/drift fail-closed 与双 rail 回归 | 本地完成；live pending |
| 7 | Kimi Code | 8.8 | CLIProxyAPI 9.2（并列中选择协议内核更强者） | canonical thinking/signature 差分、双 transport 目录与终态 | 本地完成；live pending |
| 8 | Qoder / COSY | 8.8 | TokenRouter 9.7 | site × model × effort × context capability、签名/session 错误合同 | 本地完成；live pending |
| 9 | DeepSeek Web Account | 5.8 | OmniRoute 9.7 | session/PoW、thinking/search/tool、严格流终态与同代际 session cache | 本地完成；live pending |
| 10 | API Key Coding Plans / Ollama Cloud | 8.7 | OmniRoute 9.4 | typed Profile 扩展、目录漂移审计、region × Surface 验收生成器 | 本地完成；live pending |
| 11 | Web Cookie / Web Session 订阅 | 1.5 | OmniRoute 9.8 | 独立高风险 rail、固定 origin、最小 Cookie、无 jar/redirect、逐 Provider 合同 | 隐藏 typed 推理本地完成；live pending |
| 12 | Amazon Q Developer | 0.0 | OmniRoute 4.0（仅产品分轨线索）；官方 Amazon Q CLI `15cc8f3cd18c` 为协议真值 | 独立 Account/OIDC/Profile/Driver；Amazon Q CLI wire identity、分页目录与 EventStream；绝不接受 Kiro credential/catalog/origin | 本地完成；live pending |

## 1. Gemini CLI / Code Assist

### 参考代码

- OmniRoute：`open-sse/services/codeAssistSubscription.ts`、`open-sse/services/usage/antigravity.ts`、Gemini translator/sanitizer 与 `tests/unit/gemini-*.test.ts`。
- 取其动态 entitlement 目录、quota source/stale 表达、schema/thought-signature 畸形样本；不采用已弃用的 OmniRoute `gemini-cli` 账号调度或 combo 行为。

### 设计与落点

1. `oauth.gemini_code_assist` discovery 从 unsupported 升为 fixture-verified。
2. 每次 discovery 对 Provider 绑定的唯一 `gemini_cli` Account 强制执行 quota refresh；401 只刷新该 Account 一次。
3. 从 `retrieveUserQuota.buckets[].modelId` 形成去重的 Gemini entitlement 目录；不把 Antigravity Claude bucket、静态目录或其他 Account 合入。
4. refresh transient failure 时只允许返回同 auth identity generation 的旧目录，并显式标记 `stale=true`；无同 scope 目录时失败关闭。
5. 补 API mock、代际漂移、权威空目录、0% quota、schema/signature/terminal fixtures；更新 Registry conformance 与 coverage。

代码：`src/clients/oauth/gemini_models.rs`、`src/state.rs`、`src/api/providers.rs`、`assets/contract/provider-registry.json`。验收：`cargo test gemini_models --lib`、`cargo test gemini_code_assist_discovery --lib`、`cargo test gemini --lib`、Provider audits。真实 gate 仍要求 OAuth/project/non-stream/stream/tool/image/quota/401 的脱敏 receipt。

### 实施结果

- [x] Driver contract revision 2，discovery/conformance 为 `supported` / `fixture_verified`。
- [x] quota-backed Gemini-only catalog、重复/前缀/0% 正规化和成功空目录语义。
- [x] 同账号一次 401、同代际 transient-only stale、401/403/代际漂移 fail-closed。
- [x] API mock、纯 parser、156 项 `gemini` 聚焦测试与 Provider/docs audits。
- [ ] Live gate：需要真实 Google OAuth/Code Assist entitlement receipt；本地完成不改变 `live_pending`。

## 2. Antigravity / Agy

### 参考代码

- TokenRouter：`backend/internal/pkg/antigravity/{request_transformer.go,schema_cleaner.go,stream_transformer.go}`、`backend/internal/service/{antigravity_gateway_compat.go,antigravity_gateway_retry.go,antigravity_quota_fetcher.go}`、对应接口文档。

### 设计与落点

1. 在现有 `loadCodeAssist`/`retrieveUserQuota` 基础上加入绑定账户 authenticated model catalog；Antigravity 与 Agy 保留不同 family/identity。
2. 若参考 endpoint 有可复验证据，增加 `retrieveUserQuotaSummary` weekly window；它只做当前账户 evidence，不参与路由。
3. capability evidence 记录 project/tier/privacy、Gemini/Claude family、capacity、weekly quota 的 source/observed/expires 状态。
4. 对 mixed function + Google Search、thought signature、terminal reason、429 retry-delay 和畸形 stream 做差分测试。
5. endpoint 选择只能读取当前 Account tier；禁止遍历 base URL 后跨身份或按容量改路。

主要落点：`src/clients/oauth/quota.rs`、新增 model-catalog 模块、`src/domain/accounts/capability_evidence.rs`、`src/proxy/{adapters.rs,stream_transforms.rs,antigravity_retry.rs}` 与 Registry conformance。

### 实施结果

- [x] `special.antigravity` / `special.agy` Driver contract revision 3，discovery/conformance 为 `supported` / `fixture_verified`。
- [x] 对固定绑定账号调用 `fetchAvailableModels`，保留 model quota/reset、thinking/image/token/MIME capability、deprecated alias 和 Gemini/Claude/GPT/other family；成功空目录权威。
- [x] catalog cache 绑定 Provider type + Account + `authIdentityGeneration`；只有 network/408/429/5xx 可用同身份 stale，401/403、解析失败、代际或 runtime binding 漂移 fail closed；Agy/Antigravity 互不借用。
- [x] fresh catalog 持久化当前代际 model-catalog、family 与 capacity evidence；mixed search/function、thought signature、terminal 与 bounded structured retry 继续由既有差分 fixture 覆盖。
- [x] TokenRouter 未提供独立可复现的 weekly-summary endpoint，因此明确保留 unavailable，不从 FiveHour 或 model reset 猜测 weekly quota。
- [ ] Live gate：需要真实 Antigravity 与 Agy OAuth/project/model/stream/tool/image receipt；本地完成不改变 `live_pending`。

## 3. Grok / xAI / Grok Build

### 参考代码

- TokenRouter：`docs/interfaces/grok_upstream.md` 及 Grok gateway、WS、image/video/search、usage/media task 实现。

### 设计与落点

1. 机器可读 capability 区分 Responses HTTP/SSE、WS、hosted search、image、image edit、video 与异步任务操作。
2. 媒体 task key 必须覆盖 Provider、Account、auth generation、task kind 和 upstream task id；poll/cancel/result 不能跨代际。
3. 增加 poll/cancel timeout、部分完成、重复 terminal、WS 断流和 media owner mismatch 的合同测试。
4. 动态 model catalog 保存 source/stale/fetchedAt 与 text/media capability，不把 xAI API、Grok Build 和 Grok Web token 混用。
5. 扩充真实 smoke，只保存状态、终态、任务摘要和 usage，不保存媒体正文或 bearer。

主要落点：`src/proxy/grok*`、`src/clients/oauth/grok_models.rs`、Grok media API/state、`scripts/smoke/grok-oauth-real.mjs` 与 capability contract。

### 实施结果

- [x] `oauth.grok_responses` Driver contract revision 4；动态目录使用固定 Provider/runtime + Account + auth/token generation scope，成功空目录权威。
- [x] 目录只在 network/408/429/5xx 使用同 scope、24 小时内的 last-known-good；401/403、坏 JSON、超大 body、无绑定、持久化降级和代际漂移均失败关闭，不再返回静态 entitlement。
- [x] 首次 models 401 只强刷并重放原绑定账号一次；返回前复核 Provider revision/runtime fingerprint、Account 与 auth/token generation，第二账号请求数保持为零。
- [x] discovery raw 输出保守的 text/build_text/image/video/unknown family 与 account-scoped Responses HTTP/SSE、WS、search、image/edit/video/async-task manifest；目录不能证明的媒体/搜索能力保持 `unknown`。
- [x] `grok-media-tasks.json` schema v4 增加显式 `video_generation` task kind，owner key 覆盖 Share、用户 namespace、task kind/id、Provider、Account、auth generation、runtime fingerprint 与 upstream plane；v1-v3 精确迁移并继续在身份漂移时冲突关闭。
- [x] 181 项 `grok` 聚焦测试通过，覆盖同账号 401、权威空目录、auth/token 代际 stale 隔离、坏响应、完整 task key 与旧 schema 迁移。
- [ ] Live gate：需要真实 xAI OAuth 的目录、HTTP/SSE、WS、hosted search、image/edit/video 与异步 task receipt；fixture 通过不改变 `live_pending`。

## 4. GitHub Copilot

### 参考代码

- OmniRoute：`src/lib/oauth/providers/{github.ts,ghe-copilot.ts}`、`open-sse/services/{githubCopilotModels.ts,usage/github.ts}`、GitHub/GHES executors/registry。

### 设计与落点

1. 解析 `/models` 的 capability、policy、preview、limits 字段，并保存受信 `api_origin` provenance；未知字段不扩大能力。
2. github.com 与 GHES 的 device/token exchange/model/quota receipt 分区；GHES discovery 不借公共 GitHub 静态目录。
3. 三 Surface 共用同一个短期 Copilot token scope，tool/thinking/usage/terminal 差分测试共用 canonical fixtures。
4. model/quota refresh 代际 fencing 和同账号 401 budget 进入 API 级并发测试。

主要落点：`src/clients/oauth/copilot_models.rs`、Copilot exchange/quota、`src/proxy/{adapters.rs,stream_transforms.rs}`、acceptance matrix。

### 实施结果

- [x] `/models` 解析并保留 display/vendor、picker/policy/preview、supported endpoints、context/output limits、tools/vision/reasoning；disabled/non-chat/non-routable 模型不会进入 entitlement。
- [x] 成功空目录权威；github.com/GHES 均不再运行时回退公共静态目录。只有 network/408/429/5xx 可读同 Account、auth/token generations、domain、受信 origin 的 stale；401/403、坏 JSON/结构和 scope 漂移失败关闭。
- [x] 返回前复核 Provider binding、Account identity、Account token generation、短期 Copilot token generation、GitHub domain 与 endpoint origin；fresh 结果写入当前代际 `model_entitlement` capability evidence。
- [x] Provider model API 输出 capability、origin 和 domain 元数据；`cargo test copilot_models --lib` 6 项及 `cargo test copilot --lib` 53 项通过。
- [ ] Live gate：新增三 Surface harness 后仍需真实 github.com 与 GHES device/models/quota/non-stream/stream/tool receipt；没有凭据时保持 `live_pending`。

## 5. Kiro / Amazon Q

### 参考代码

- OmniRoute：Kiro executor/EventStream/model/region/usage，以及标为 `amazon-q` 的 registry/executor wiring。源码复核显示 `amazon-q` 直接别名到 Kiro OAuth、Kiro token refresh 与 `KiroExecutor`，并使用同一静态模型目录，因此它不能证明独立 Amazon Q credential、entitlement 或 data-plane 合同。

### 设计与落点

1. Kiro 保持现有 profile ARN、region authority、严格 EventStream、tool/image bounds；模型目录 scope 补全 app、Provider revision/runtime fingerprint、Account/auth generation/token generation、profile 与 runtime region。
2. 成功空目录是当前绑定账户的权威 entitlement；只有 network/timeout/408/429/5xx 可读取同 scope 的 bounded last-known-good。无缓存时失败关闭，不再用静态模型猜测 entitlement。
3. 首个目录 401 只能强刷同一个绑定 Account 一次并重放一次；坏 JSON、缺少 models 数组、超过 2 MiB、第二次 401、Provider/runtime/account/generation 漂移均失败关闭。
4. Claude/Codex 三个 text Surface 保留 fixture-verified；Gemini 保持 unsupported。推理热路径不得用静态模型绕过绑定账户目录授权。
5. Amazon Q 不再是 Kiro alias 或 reserved spelling：仅独立 `amazon_q_oauth` Account、`claude.amazon_q_oauth` / `codex.amazon_q_oauth` Profile 与 `special.amazon_q` Driver 可用；旧的非 typed alias 仍直接拒绝，不能落入 generic HTTP、Kiro 或 Bedrock。

### 实施结果

- [x] Kiro catalog key 覆盖 app、Provider id/revision、runtime fingerprint、Account、auth/token generation、profile scope 与 runtime region。
- [x] 成功空目录权威；仅 transient failure 可读同 scope stale；无静态 entitlement fallback，malformed/oversized success 失败关闭。
- [x] Provider 管理目录、公开 `/v1/models` 与推理热路径共用集中式解析；首次 401 仅刷新并重放原绑定 Account，decoy Account 零访问。
- [x] Amazon Q 已按官方 CLI wire 独立实现 AWS SSO OIDC device/refresh、CLI identity/targets、分页目录/defaultModel、quota、Claude/Codex EventStream 与同绑定首次 401 一次重放；Kiro Account/catalog/quota/endpoint identity 永不参与。
- [x] Amazon Q 两个 server-native Profile 已有显式 S1 创建桥，固定库存/API contract、coverage/regression/import 真值与真实验收 runbook/external gate 已同步。
- [x] Kiro focused suite 与 Amazon Q fixture suite、Provider coverage audit 通过；两条产品 rail 均保持独立 scope 与 `fixture_verified`。
- [ ] Live gate：Kiro 仍需真实 Builder ID/IdC/Social/API Key 的跨 region 目录与推理 receipt；Amazon Q 仍需真实 Builder ID/IdC 的双 Surface、tool/image/quota/401/撤销 receipt，完成前保持 `live_pending`。

## 6. Cursor

### 参考代码

- OmniRoute：`open-sse/executors/cursor/`、`open-sse/services/cursorSessionManager.ts`、`open-sse/utils/cursorAgentProtobuf/`、OAuth/provider 实现。

### 设计与落点

1. 对官方 ServerConfig protobuf 增加 version/capability drift evidence，未知字段保留但不自动开放 builtin。
2. OAuth DeepControl 与 API-key exchange 两 rail 分别做 discovery→AgentService→401 的 differential；共享同一个 rail 内 401 budget，绝不互相 fallback。
3. park/resume、tool continuation、image、MCP wrapper、绝对 business-output deadline 和 authoritative empty catalog 补 API/stream fixtures。
4. acceptance smoke 输出 endpoint digest、rail、generation、conversation/session 摘要和 terminal，不输出 token/body。

### 实施结果

- [x] 审计 OmniRoute `b342c1a361f2` 的 live-catalog model resolver；仅当 API-key `/v1/models` 目录仍 fresh 且属于 exact App/Provider revision/runtime/credential generation/key digest scope 时，完整 `*-fast` 模型 ID 原样进入 AgentService protobuf。
- [x] 未命中 fresh exact-scope 目录时保留既有兼容映射；OAuth 静态 alias、其他 Provider、其他 runtime/key generation 和 stale 目录不能授权完整 ID 透传。
- [x] API-key catalog last-known-good 增加 1 小时硬上限；成功空目录仍权威，认证/协议错误继续清除当前 scope，过期 stale 自动移除。
- [x] ServerConfig field 27 与内部 `agentUrl`/`agentnUrl` 重复时失败关闭，避免 first/last-wins 解析差异；两 URL 仍必须同时满足受信 HTTPS origin 合同。
- [x] `special.cursor` Driver contract revision 4，forward/test conformance 为 `fixture_verified`；289 项 `cursor` 聚焦测试覆盖双 rail、目录 scope、完整模型 ID wire、ServerConfig、park/resume、MCP、图片与 deadline。
- [ ] Live gate：OAuth 与 API-key 必须分开运行 `docs/provider/cursor.md` 的真实矩阵和 SDK differential；没有真实凭据时仍为 `live_pending`。

## 7. Kimi Code

### 参考代码

- CLIProxyAPI：`internal/runtime/executor/{kimi_executor.go,kimi_thinking_replay.go}`、`internal/cache/kimi_thinking_replay_cache.go`、`internal/signature/kimi_validation.go`、canonical thinking provider。

### 设计与落点

1. 以外部 canonical thinking 向量补 Claude native Messages 与 Chat bridge 的 signature-only、placeholder、并行 tool、跨 turn replay。
2. replay scope 继续覆盖 Provider、Share、用户、session、model family、Account/auth generation；命中/拒绝/过期增加脱敏 metric。
3. `/coding/v1/models` 目录解析 capability/上下文；权威空目录不与静态模型合并。
4. 三 Surface non-stream/stream/tool/count_tokens/401 共享同一身份验收清单。

### 实施结果

- [x] `oauth.kimi_code` Driver contract revision 3；Claude native Messages/count_tokens 与 Codex/Gemini Chat bridge 保留 `fixture_verified`。
- [x] `/coding/v1/models` 成功空目录权威；上游非空但 reviewed 交集为空视为合同漂移。仅 network/408/429/5xx 可读取 App、Provider revision/runtime、Account、auth/token generation 完全相同且不超过 24 小时的 stale；静态 entitlement fallback 已删除。
- [x] 首个 models 401 只强刷并重放原绑定 Account 一次，第二个 401 与刷新失败终止；initial refresh transient 仅能读取刷新前 exact-generation stale，credential persistence、认证和协议错误失败关闭。
- [x] signed thinking replay scope 增加 token refresh generation，写入前同时复核 auth/token generations；signature-only、placeholder、parallel tool、跨 turn、流 `message_stop`、400/422 CAS 删除与代际漂移均有 canonical fixture，hit/miss/reject/expire metric 不含租户标签。
- [x] 37 项 `kimi` 聚焦测试及 Provider/docs audits 通过；目录、推理和 replay 均未增加账号选择、pool 或跨凭据 fallback。
- [ ] Live gate：需要真实 Kimi Device OAuth 的三 Surface catalog/non-stream/stream/tool/image/count_tokens/quota/401 receipt；本地完成不改变 `live_pending`。

## 8. Qoder / COSY

### 参考代码

- TokenRouter：`backend/internal/pkg/qoder/{auth.go,client.go,models.go,session.go,signature.go,site.go}`、Qoder gateway/token/quota 与 `docs/interfaces/qoder_upstream.md`。

### 设计与落点

1. 把 Global/CN 返回的 model metadata 正规化成 `model × effort × context × modality` capability；未知 effort 不外推。
2. COSY signing/session/job token 增加 clock skew、nonce/session mismatch、重复 terminal、401 后 generation drift fixtures。
3. site 是 Account authority，不因模型、quota、403 或 endpoint 错误自动切站。
4. 三 Surface 的 tool/thinking/usage/terminal 使用同一 capability 与验收记录。

### 实施结果

- [x] `special.qoder_cosy` Driver contract revision 2；Global/CN alias 与 live route 集中建模，成功空目录权威，未知 live route 只按 exact ID 发布且不外推 reasoning/context。
- [x] `/v1/models` 只读取当前 Provider/runtime 显式绑定的唯一 Qoder Account；发布 live model 的 reasoning efforts、context window、text modality 与 tool capability，不与配置静态目录或第二账号求并集。
- [x] 上下文矩阵按站点与 route 注入：fixed route 只写 `model_config.max_input_tokens`，runtime-selectable route 同时写 `parameters.context_length` 与 `chat_context.extra.ideModelConfigOverride`；未知 route 不猜测。
- [x] COSY signing timestamp 限制为当前时钟正负五分钟，请求 nonce 必须是规范 UUID v4；clock skew、坏 nonce、site/capability、session/generation、三 rail × 三 Surface 与 pre/post-commit 401 均有 fixture。
- [x] 目录首次 401 只刷新 OAuth 或重新交换 PAT 的原绑定 Account 一次；连续第二个 401 终止，干扰 Account 的 token generation 与请求数保持不变。
- [ ] Live gate：需要 Global/CN OAuth 与 Global PAT 的真实 login/exchange/catalog、三 Surface stream/tool/quota/401 receipt；本地完成不改变 `live_pending`。

## 9. DeepSeek Web Account

### 参考代码

- OmniRoute：`open-sse/executors/deepseek-web.ts`、`deepseek-web-with-auto-refresh.ts`、`deepseek-web/stream-format.ts`、`open-sse/lib/deepseek-pow.ts` 及 DeepSeek Web session/tool/terminal tests。

### 设计与落点

1. 保留 import-only bearer，不接受或保存密码；session、PoW challenge/answer 和任何复用状态都绑定 Provider、Account、`authIdentityGeneration` 与固定 `chat.deepseek.com` origin。
2. 补 token/session 401 分类、失效 session 单次重建、PoW clock/expiry/difficulty/algorithm bounds；所有恢复只使用当前绑定 Account，禁止 Cookie、其他 token 或 Provider fallback。
3. 实现 thinking/search 三类片段、引用、tool prompt/call/result 还原与严格 `[DONE]`/EOF/重复 terminal 状态机；畸形流在下游提交后只终止，不重放。
4. 增加动态/权威模型能力表达和独立 `experimental`/`live_pending` 风险状态；通用 DeepSeek API Key 与 Web Account rail 不混用。
5. 增加真实 token 的 non-stream/stream/thinking/search/tool/401/session-rebuild/PoW 脱敏 receipt；没有真实输入时仅为 fixture verified。

主要落点：`src/clients/deepseek/`、`src/proxy/deepseek.rs`、DeepSeek Account manager/capability evidence、acceptance matrix 与独立 smoke。

### 实施结果

- [x] `deepseek_account` 导入收敛为最大 16 KiB 的单一 bearer-only rail；拒绝 Bearer 整串、空白/控制字符、refresh/ID token、API key、scope、extra header，以及 profile/raw 内递归出现的 Cookie、password、session 或替代 credential。
- [x] 生产 origin 固定 `https://chat.deepseek.com`；30 分钟、最多 256 scope 的 single-flight session cache 同时绑定 App、Provider revision/runtime、Account auth/token generation、Share/user/client session 与 reviewed model。
- [x] 仅复用 session 的 400/404/409 可在下游提交前由原绑定 Account 重建一次；401/403/429/5xx、新 session 失败、第二次失败、代际漂移和提交后错误均终止，干扰 Account 从未被读取。
- [x] PoW 限制 algorithm、target、challenge、salt/signature、expiry/horizon、difficulty 与运算上限；Claude Native bridge 覆盖多轮、thinking、search citation、nonce-bound tool use/result、严格唯一终态和截断/terminal 后数据拒绝。
- [x] reviewed model catalog、Native Provider dry-run/network test 和 discovery 均要求同一显式 Account/generation；目录只发布 text/tools/thinking/search，并明确 `fixture_verified` / `live_pending`，images、Codex 与 Gemini 保持 unsupported。
- [x] 新增 `docs/provider/deepseek-web.md`、coverage/Phase-0/regression/acceptance 证据；`cargo test deepseek --lib` 22 项通过。
- [ ] Live gate：仍需真实 bearer 的 import、non-stream/stream、thinking/search/tool、session expiry/rebuild、401/403/429/5xx、PoW drift 与撤销脱敏 receipt；本地完成不改变 `live_pending`。

## 10. API Key Coding Plans / Ollama Cloud

### 参考代码

- OmniRoute：API Key registry、区域/模型/quota adapter、Coding Plan 与普通 PAYG 分型。

### 设计与落点

1. 对 OmniRoute、9router 固定证据与 Server Registry 做自动 delta，只有 fixed origin、auth、route、catalog、quota 和 terminal 有证据的 plan 才新增 typed Profile。
2. 每个 plan 必须区分 region × Surface；同名模型不能跨 entitlement rail 外推。
3. 增加 Registry drift manifest 和生成审计：source commit、capturedAt、maturity、live state、模型 capability、quota provenance。
4. quota unavailable 必须诚实返回 unavailable，不抓 console Cookie；Provider-owned key 不创建推理 Account。
5. Ollama `/api/me`/`/api/usage` 继续只读，补 partial success、redirect、body limit、generation rotation 与 stale cache fixtures。

### 实施结果

- [x] 新增 `coding-plan-source-baseline.json`：固定 OmniRoute `b342c1a361f2`、9router `90b52e06ffd6` 及 20 个 origin/route/catalog/quota/Ollama 证据文件的 SHA-256。
- [x] 新增 `audit-coding-plan-registry.mjs` 与生成 manifest；逐项校验 10 个 Family、20 个 Profile、5 个 region 标签和 Claude/Codex 两 Surface 的 fixed HTTPS origin、Provider-owned credential、exact route、模型 modality/context、quota provenance、terminal、error/retry 与 maturity。普通 `--check` 已进入静态门禁，`--check-sources` 对当前外部目录的 commit/hash 也通过。
- [x] Manifest 明确所有 Profile 为 `fixture_verified` / `live_pending`，无真实证据时 tools 不从模型名外推；quota unavailable 强制 endpoint/credential slots 为空并标记 `explicit_unavailable_no_console_cookie`。
- [x] Ollama contract 已复核并纳入同一 manifest：Provider-owned API Key、并发 `POST /api/me` + `GET /api/usage`、禁 redirect、512 KiB 上限、section partial success、仅 retryable 同 generation stale、认证清 cache、Bundle single-flight、删除/轮换清理和旧代际在途结果丢弃。
- [x] 新增 `docs/provider/api-key-coding-plans.md` 与两项 Node audit tests；`cargo test coding_plan --lib` 29 项通过，`cargo test ollama --lib` 24 项通过，唯一 live test 因缺真实 Key 正确 ignored。
- [ ] Live gate：20 个 region × Surface 仍需分别保存真实推理/终态/quota/error/rotation receipt；Ollama 仍需真实推理、目录、account/usage、partial/429/rotation receipt。缺少输入时保持 `live_pending`。

## 11. Web Cookie / Web Session 订阅

### 参考代码

- OmniRoute：35 类 Web Cookie registry/executor/session 状态；优先审计官方 OAuth/API Key 不可替代且具备稳定协议证据的条目。

### 设计与落点

1. 新建独立 `web_session` credential rail；不能复用 OAuth Account、API Key slot 或通用 extra headers。
2. 每个 Profile 固定 HTTPS origin、Cookie 名 allowlist、请求 path/method、CSRF/会话刷新策略、响应终态、最大 body 和 maturity/risk；默认禁止 redirect、cookie jar、跨 origin 和响应 `Set-Cookie` 透传。
3. secret 以 Provider-owned 加密 slot 保存，日志/API 只显示 presence/digest/generation；删除或换 Cookie 立即清理 session/cache/task。
4. 先用 Grok Web/Perplexity Web 的双源证据验证框架，再逐项审计 OmniRoute 清单；每项没有独立 fixture/live gate 时不进入 visible/stable registry。
5. Web Cookie 与 `grok_oauth`、`gemini_cli` 等官方 rail 绝不 fallback；401/403 只标记当前 Web session 失效并要求显式重新导入。

### 实施结果

- [x] 新增独立 `/settingsConfig/webSession/cookie` Provider-owned 加密 secret slot；不复用 Account、API Key 或 extra headers。严格 parser 只接受 allowlist Cookie pairs，拒绝 Bearer、Authorization、Set-Cookie、JSON、控制字符、重复/未知 Cookie 与缺失 required family；API-safe summary 只含 presence、Cookie 名、digest 和 credential generation。
- [x] 新增 typed `WebSessionProfileSpec` 与机器可读 registry。Grok Web 和 Perplexity Web 使用 OmniRoute `b342c1a361f2` + 9router `90b52e06ffd6` 的双源 commit/hash 证据，固定 HTTPS origin、POST path、Cookie family、CSRF/显式重导入策略、body limit 和 terminal。
- [x] transport 合同默认禁 redirect、cookie jar、跨 origin，并禁止下游 `Set-Cookie`/`Location`/`Refresh`；exact request guard 拒绝 method/origin/path/query/fragment 漂移。
- [x] session/task/invalidation scope 覆盖 Provider key/revision、runtime fingerprint、credential generation、Profile 与 origin。轮换和删除精确清理；401/403 不重试，只失效当前 scope 并要求显式重导入，其他 Provider/rail 不可见。
- [x] 四个 Claude/Codex Profiles 均固定为 `hidden` / `experimental` / `high risk` / `implemented` / `fixture_verified` / `live_pending`，只能显式 Profile ID 创建，不进入可见 preset。
- [x] 独立 no-redirect/no-cookie-jar transport、Grok/Perplexity 专用请求翻译、Claude Messages/count_tokens 与 Codex Chat/Responses 响应生命周期、严格 NDJSON/SSE 状态机、总响应/单 frame/first-byte/idle/total timeout 上限均已接入主转发入口；完整终态验证后才生成下游响应。
- [x] reviewed 静态模型目录明确返回 `fixture_verified` / `live_pending` / `entitlement=not_asserted`；tools/function/images/attachments/previous-response 在零上游阶段拒绝，count_tokens 零上游，Share usage 标记 estimated。
- [x] 新增 `web-session-source-baseline.json`、生成 manifest、source drift audit、Node/Rust fixtures 和 `docs/provider/web-session.md`；Claude/Codex × stream/non-stream、任意分块、固定头、Set-Cookie 丢弃、非终态 partial frame 后客户端取消零重试/无 fallback、401 零重试、generation 轮换、runtime 漂移和 Share 计费均有端到端测试。
- [x] 推理 gate：typed Profile、专用转换/transport、严格终态、请求/响应上限和代际隔离已完成；仍保持 hidden，不因本地 fixture 自动进入 visible/stable。
- [ ] Live gate：逐 Profile 保存真实 transport、撤销和脱敏 receipt 后才能考虑从 experimental 提升；本地推理 fixture 不能关闭此门禁。

## 12. Amazon Q Developer

### 参考代码与证据优先级

- 评分表最高的外部参考是 OmniRoute 4.0，但其 `amazon-q` 直接别名到 Kiro OAuth、`refreshKiroToken`、`KiroExecutor` 和静态 Kiro 目录，只能借鉴独立产品标签、region/profile 状态展示，不能作为 entitlement 或数据面证据。
- 协议真值改用官方 `amazon-q-developer-cli` 固定提交 `15cc8f3cd18c`：`crates/chat-cli/src/auth/{consts.rs,builder_id.rs}`、`crates/chat-cli/src/api_client/{endpoints.rs,mod.rs,model.rs}` 和生成的 CodeWhisperer client。
- 官方证据固定：OIDC region `us-east-1`、Builder ID start URL `https://view.awsapps.com/start`、client name `Amazon Q Developer for command line`、public client、scope 为 `codewhisperer:completions` / `analysis` / `conversations`；runtime 只允许 `us-east-1`、`eu-central-1`。

### 可落地设计

1. 新增 `ProviderType::AmazonQOAuth`，Account manager、device-flow store、refresh lock、凭据 generation 与 Provider binding 均使用该类型；任何 `KiroOAuth` Account 都必须以 type mismatch 失败，reserved 拼写只在 typed family 写入真实存在后解除。
2. 新增 Amazon Q 专用 OIDC device flow：register/start/poll/refresh 都固定 AWS OIDC origin，client registration 与 token 保存在同一 Amazon Q Account 的 raw/profile 中；client secret、refresh token 和 access token 不进入日志或公开响应。
3. 新增 `special.amazon_q` Driver 与 Claude/Codex Profile。低层 Anthropic/OpenAI→CodeWhisperer 转换和 AWS EventStream decoder 可复用已审计纯函数，但 wire flavor 强制 CLI root endpoint、`application/x-amz-json-1.0`、`AmazonCodeWhispererStreamingService.GenerateAssistantResponse`、Amazon Q CLI UA 和 `origin: CLI`；拒绝 `AI_EDITOR` / `KIRO_CLI`。
4. 新增独立动态目录缓存。`ListAvailableModels` 必须 `origin: CLI`，携带同一 Account 的可选 `profileArn`，处理 `nextToken` 分页和 `defaultModel`，限制页数/总模型数/响应字节；cache key 包含 App、Provider revision/runtime fingerprint、Amazon Q Account auth/token generation、profile 与 region。成功空目录权威，只有 network/408/429/5xx 可读 exact-scope bounded stale。
5. quota/套餐证据只读取同一 Amazon Q bearer 身份的官方 CodeWhisperer operation；没有可验证响应时显式 unavailable，不借用 Kiro usage、静态目录或另一个 Account。首个 401 只可刷新并重放相同 Amazon Q binding 一次且必须在下游提交前。
6. fixture 覆盖 OIDC request shape、Kiro type rejection、CLI origin/headers、目录分页/默认模型/超限/坏 JSON、EventStream CRC/截断/终态、generation rotation、401 once-only 和取消；真实账号缺失时保持 `fixture_verified` / `live_pending`。

### 计划落点

- Account/OAuth：`src/domain/providers/model.rs`、`src/domain/accounts/{oauth.rs,managers.rs,store.rs}`、`src/clients/oauth/amazon_q_device.rs`、`src/clients/oauth/amazon_q_runtime.rs`、`src/clients/oauth/refresh.rs`、`src/state.rs`、Account API。
- Profile/Driver：`assets/contract/provider-registry.json`、`src/domain/providers/{matrix.rs,runtime.rs,registry.rs,store.rs}`、`src/proxy/{kiro.rs,kiro/endpoint.rs,forwarder.rs}`。
- 证据与验收：Provider coverage/matrix、regression matrix、官方 source baseline、专项 Rust/Node contract、真实验收 runbook；本节状态只有在上述实现和测试真实落地后才改为完成。

### 实施结果

- [x] 独立 `ProviderType::AmazonQOAuth`、Account manager、AWS SSO OIDC device register/start/poll/refresh、加密 client registration 和 generation-scoped refresh lock；Kiro Account 在网络请求前以 type mismatch 失败。
- [x] 独立 `claude.amazon_q_oauth` / `codex.amazon_q_oauth` typed Profile、`special.amazon_q` Driver、可创建 S1 bridge、CLI root endpoint/target/UA/`origin: CLI` 和严格 EventStream；共享仅限已审计纯转换/image/decoder，不共享 Kiro credential 或 endpoint identity。
- [x] exact Provider/runtime/Account/auth+token-generation scope 的分页 `ListAvailableModels`、`defaultModel`、权威空目录、transient-only bounded stale，以及同身份 `GetUsageLimits`。
- [x] Provider creation、公开 registry/preset、Account capability、adapter inventory、coverage、regression matrix、runbook 与审计真值已同步；Amazon Q 聚焦测试以及全量本地合同通过后方可提升本地成熟度。
- [ ] Live gate：真实 Builder ID/IdC 的登录、目录、Claude/Codex non-stream/stream/tools/images、401/429、额度与撤销 receipt。

## 最终整体 review gate

完成 12 个循环后必须同时满足：

1. Registry、Provider coverage、UI matrix、regression matrix 和本页状态一致；新增文档已进入 `docs/README.md`。
2. 每个 Driver 的 forward/test/discovery/connectivity conformance 与实际代码一致，没有把 generic upstream 计为原生 plan。
3. 搜索和架构 review 证明没有新增 account selector、pool、rotation、quota/cooldown/concurrency 选号或跨 Provider fallback。
4. 新增 cache/session/replay/media/web-session key 都有 Provider/runtime + Account/credential generation 测试。
5. `cargo fmt --check`、`cargo check`、专项与全量 `cargo test`、Provider/UI audits、local smoke、offline release readiness 全部通过。
6. 缺少真实凭据的项保留 `live_pending`，由 `docs/acceptance/real-acceptance-runbook.md` 的脱敏 receipt 单独关闭。

### 最终验证结果（2026-08-31 UTC）

- Registry、Provider coverage、UI matrix、regression matrix、Phase-0、文档索引、Coding Plan/Web Session source-drift manifest 与 Token Market decoupling audit 均通过；Amazon Q 与四个 Web Session Profile 的创建/可见性/成熟度真值一致。
- 增量源码审查未发现新增 account selector、pool、round-robin、quota/cooldown/concurrency 选号或跨账号/跨 Provider fallback。Amazon Q 在 Kiro/generic adapter 前使用独立 typed identity；Web Session 在 generic adapter/retry 前直接终止，401/403 与非终态 partial frame 后客户端取消均不重试或 fallback。
- 新增 catalog/cache/session/replay/media/Web Session scope 均包含对应的 Provider/runtime 与 Account auth/token generation 或 Provider-owned credential generation；代际漂移、删除和轮换合同有 fixture。
- `cargo fmt --check`、`cargo check` 通过。按仓库 release gate 的 `RUST_MIN_STACK=67108864` 执行 `cargo test --no-fail-fast`：lib 2759 通过、1 忽略，API contract 124/124，另外两个 contract 各 1/1，doc tests 通过。Amazon Q 关键词专项 9/9、Web Session 关键词专项 23/23。
- Provider/UI/Phase-0/docs/Web Session/Coding Plan audits、`scripts/smoke/smoke-local.sh`、Web TypeScript typecheck 与 `git diff --check` 通过。`RUN_TESTS=1 RUN_REAL=0 scripts/release-readiness.sh` 返回 `ready-with-known-external-blockers` / `contract_verified`；`RUN_TESTS=0` 变体按设计因跳过本地测试标记 `local-contracts-unverified`，不被冒充为通过。
- 未提供真实 Router、Share、各 Provider token 或部署输入；这些外部门禁和全部真实 Code Plan receipt 继续保持 `live_pending`，没有改写为 live verified。
