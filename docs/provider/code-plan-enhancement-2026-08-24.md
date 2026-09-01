# Code Plan enhancement traceability (2026-08-24)

本页追踪 `/data/projects/proxy/proxy.md` 中 `cc-switch-server` 评分低于 9、且不属于“其他 IDE Plan”的七类能力。分数是源码与证据密度评分，不是在线压测；本轮完成本地实现后，没有真实订阅 receipt 的项目仍保持原分和 `live_pending`，不得因 fixture 通过而升级为 live-verified。

## 不变量与验收边界

- 请求先固定一个 Provider；managed Provider 再固定一个显式 Account，Provider-owned API Key 则固定一个 credential generation。
- 不按模型、quota、cooldown、并发、错误或 endpoint 在账号间选择、轮询或迁移；不允许跨 Provider fallback。
- 首个 eligible 401 仅可刷新、交换或重新发现同一绑定凭据并在下游提交前重放一次；第二个 401、提交后失败和身份代际漂移均为终态。
- cache/replay/session key 必须覆盖 Provider revision/runtime 与 Account/credential generation；相同原始 id 或模型名不能跨 scope 复用状态。
- mock server、fixture、contract 和静态 Registry 审计只证明本地接线。真实账号、区域、entitlement、stream/tool/quota/refresh receipt 缺失时保持 `live_pending`。

## 队列与两个主参考

| Code Plan | 报告分数 | 主参考 1 | 主参考 2 | 本地结果 |
| --- | ---: | --- | --- | --- |
| Antigravity / Agy | 8.9 | TokenRouter 9.7，`3d91c215ce811d71cc8a996c52aabb4a034f096a` | sub2api 9.6，`d45135d87df16d48637f04ccd245727bc955ba54` | 递归 schema、mixed tools、tier endpoint 完成；27 项聚焦测试 |
| GitHub Copilot | 8.8 | OmniRoute 9.4，`c68cda7dfb49f5741195b7398e4cc6349a6d07f2` | Server Copilot protocol fixtures | Gemini Surface 完成；50 项聚焦测试 |
| Kimi Code | 8.8 | OmniRoute 9.2，`c68cda7dfb49f5741195b7398e4cc6349a6d07f2` | CLIProxyAPI 8.9，`a7e3596b7e351d800e58ed29529fbca3d1c18737` | Claude native + scoped signed replay 完成；32 项聚焦测试 |
| Kiro / Amazon Q | 8.8 | OmniRoute 9.5，`c68cda7dfb49f5741195b7398e4cc6349a6d07f2` | 9router 8.4，`699edac3273e13d4744bc46f6082618f08560702` | 坏 tool JSON 按调用隔离；129 项聚焦测试 |
| Qoder / COSY | 8.8 | TokenRouter 9.7，`3d91c215ce811d71cc8a996c52aabb4a034f096a` | OmniRoute 9.2，`c68cda7dfb49f5741195b7398e4cc6349a6d07f2` | 1.24.2、GLM-5.3、effort 完成；43 项聚焦测试 |
| Cursor | 8.7 | OmniRoute 9.3，`c68cda7dfb49f5741195b7398e4cc6349a6d07f2` | 9router 8.0，`699edac3273e13d4744bc46f6082618f08560702` | 官方 discovery/exchange 与共享 401 预算完成；197 项聚焦测试 |
| 国内 Coding Plan | 8.7 | OmniRoute 9.1，`c68cda7dfb49f5741195b7398e4cc6349a6d07f2` | 9router live endpoint evidence | Alibaba 双区域四 Profile、GLM-5.3 Codex-only 完成 |

聚焦数量是 `cargo test <keyword> --lib` 的关键词口径，可能包含同名的通用测试；它用于复现而不是宣称独立 live 用例数。

## 1. Antigravity / Agy

### 参考与取舍

- TokenRouter：`backend/internal/pkg/antigravity/{request_transformer.go,schema_cleaner.go,stream_transformer.go}`、`backend/internal/service/{antigravity_gateway_compat.go,antigravity_gateway_retry.go,antigravity_quota_fetcher.go,antigravity_privacy_service.go}`。
- sub2api：同名 `backend/internal/pkg/antigravity/*` 与 `backend/internal/service/antigravity_*`，用于交叉确认 schema、tier/capacity、search 与错误分类，而不是把共同来源当作两份独立 live receipt。
- 采纳：递归清理 Gemini function schema；function declarations 与 Google Search 并存时开启 `includeServerSideToolInvocations`；已确认付费 tier 走 daily endpoint，未知/免费走 production；保留 project/privacy/tier/family/capacity 的结构化、代际绑定 evidence。
- 拒绝：smart scheduler、账号池、sticky 清除、quota/cooldown 选号、长容量重试循环、跨账号/Provider failover、浏览器池和外部 privacy 写入。

### Server 落点与阶段

- 实现：`src/proxy/tool_schema.rs`、`src/proxy/adapters.rs`、`src/proxy/antigravity_retry.rs`、`src/proxy/outbound_identity.rs`、`src/domain/accounts/capability_evidence.rs`、`src/clients/oauth/quota.rs`、`src/proxy/forwarder.rs`。
- 测试：上述模块内 unit tests；`cargo test antigravity --lib` 为 27 passed，`cargo test tool_schema --lib` 为 8 passed。

- [x] Phase A：核对 schema/search/tier/capacity 证据并排除 pool 行为。
- [x] Phase B：递归 schema 与 mixed-tool 请求合同。
- [x] Phase C：tier authority、endpoint 选择和非秘密 capability evidence。
- [x] Phase D：同 Provider/Account 代际与一次 pre-commit retry review。
- [ ] Live gate：OAuth、project bootstrap、privacy read、Claude/Gemini search、quota、stream/tool 和短延迟 429 receipt。

不变量证明：endpoint 选择只读取已绑定 Account 的 tier，不枚举账号；结构化短重试复用同一个 `ProviderExecution` 和 Account generation；长延迟只冷却当前 Share/runtime/model。

## 2. GitHub Copilot

### 参考与取舍

- OmniRoute：`src/lib/oauth/providers/{github.ts,ghe-copilot.ts}`、`open-sse/services/{githubCopilotModels.ts,usage/github.ts}`、`open-sse/executors/{github.ts,ghe-copilot.ts}`、GitHub/GHE registry。
- Server fixtures：Copilot token exchange、model map、optimizer、三 Surface bridge 与同账号 401 测试。
- 采纳：GitHub OAuth 与短期 Copilot inference token 分离、受信动态 origin、models/quota generation fencing，以及 Gemini generateContent ↔ Copilot Chat 的 request/response/stream/tool/usage bridge。
- 拒绝：desktop default-account、账号轮询、跨账号重试、M365 Copilot、Copilot Web Cookie、Provider failover 与桌面 UI/Tauri 行为。

### Server 落点与阶段

- 实现：`src/domain/providers/{matrix.rs,registry.rs}`、`assets/contract/{provider-registry.json,provider-coverage.json}`、`src/proxy/{adapters.rs,stream_transforms.rs,forwarder.rs}`、`src/api/providers.rs`、`web-src/src/server/directProviderPresets.ts`。
- 测试：`src/proxy/adapters.rs` 与 `stream_transforms.rs` 的 Gemini tool/usage/terminal cases、Provider Registry/Web source tests；`cargo test copilot --lib` 为 50 passed。

- [x] Phase A：复核 github.com/GHES exchange、origin、models 与 quota。
- [x] Phase B：Gemini Profile/Bundle/Provider Matrix 接线。
- [x] Phase C：non-stream 与 stream bridge，保留 tool id/args、usage 和单终态。
- [x] Phase D：确认三 Surface 共用同一显式 Account binding 与一次 401 状态机。
- [ ] Live gate：github.com 与 GHES 分区 login、models、quota、Claude/Codex/Gemini non-stream/stream/tool/401 receipt。

不变量证明：Registry Bundle 的三个 Surface 共享同一个 managed-account binding；模型只改变当前 Provider 的上游 model；exchange/replay 以相同 Account id 和 `authIdentityGeneration` 为前提，GHES 不借公共目录越权。

## 3. Kimi Code

### 参考与取舍

- OmniRoute：`src/lib/oauth/providers/kimi-coding.ts`、`open-sse/config/providers/registry/kimi/coding/*`、`open-sse/executors/kimi.ts`、`open-sse/services/{tokenRefresh/providers/kimiCoding.ts,usage/kimi.ts}`。
- CLIProxyAPI：`internal/runtime/executor/{kimi_executor.go,kimi_thinking_replay.go}`、`internal/cache/kimi_thinking_replay_cache.go`、`internal/signature/kimi_validation.go`、`internal/thinking/provider/kimi/apply.go`。
- 采纳：Claude Surface 直接使用 `/coding/v1/messages` 与 `/coding/v1/messages/count_tokens`；Codex/Gemini 保留 Chat bridge；signed thinking 精确绑定 Share、签名用户、session、model family 和 credential generation。unsigned/placeholder thinking 不再阻止同 scope 的真实 signed replay；非空 signature（包括 signature-only block）优先保留。
- 拒绝：Kimi Web/API rail 替代、账号池/rotation、跨 scope replay、跨账号/Provider fallback，以及把 Qoder 内 Kimi 模型当作 Kimi Code entitlement。

### Server 落点与阶段

- 实现：`src/proxy/{adapters.rs,kimi.rs,kimi_runtime.rs,forwarder.rs}`、`src/domain/kimi_cli.rs`、`src/clients/oauth/kimi_device.rs`、`src/domain/providers/matrix.rs`。
- 测试：native endpoint、catalog/401、stream commit、generation drift、unsigned/placeholder/signature-only replay；`cargo test kimi --lib` 为 32 passed。

- [x] Phase A：确认 Kimi Coding 与 Kimi Web/API 权益分轨。
- [x] Phase B：Claude native Messages/count_tokens，Codex/Gemini Chat bridge。
- [x] Phase C：signed replay exact scope、CAS generation fence 与 placeholder 语义。
- [x] Phase D：同账号一次 401、提交时点和拒绝 replay 删除 review。
- [ ] Live gate：device/import、models、quota、三 Surface stream/tool/replay/refresh receipt。

不变量证明：replay key 不含可跨账号复用的裸 session；写入/读取均校验 Provider 与 Account generation。401 仅刷新 `kimi_code` Provider 已绑定的 Account，且流在 `message_stop` 后不可重放。

## 4. Kiro / Amazon Q

### 参考与取舍

- OmniRoute：`open-sse/executors/kiro{.ts,/eventstream.ts}`、`open-sse/services/{kiroModels.ts,kiroRegion.ts,kiroExternalIdp.ts,usage/kiro.ts}`、`src/lib/oauth/providers/kiro.ts`。
- 9router：`open-sse/{executors/kiro.js,providers/registry/kiro.js,services/kiroModels.js,services/usage/kiro.js}`、`open-sse/translator/{request,response}/*kiro*`。
- 采纳：Kiro 的 profile ARN/region authority、多认证 shape、EventStream 严格终态和 tool JSON 完整性；本轮把坏 JSON 按 `toolUseId` 隔离，合法 text/tool 继续输出并计入 usage，仅剩坏 tool 时 fail closed。
- 拒绝：Kiro 与 Amazon Q entitlement 混写、账号池、quota/region 选号、任意 endpoint/region fallback、跨账号/Provider retry、外部工具在 Server 上直接执行。

### Server 落点与阶段

- 实现：`src/proxy/kiro.rs`、`src/proxy/kiro/{endpoint.rs,image.rs,tool_bridge.rs,wire/*}`、`src/clients/oauth/{kiro.rs,kiro_device.rs,kiro_runtime.rs}`、`src/domain/providers/kiro.rs`。
- 测试：mixed valid/invalid tool、invalid-only fail-closed、stream token accounting，以及既有 auth/region/model/quota/EventStream cases；`cargo test kiro --lib` 为 129 passed。

- [x] Phase A：复核 Kiro/Amazon Q 类型边界与 region/profile authority。
- [x] Phase B：按调用隔离 tool accumulator 的 invalid/incomplete/limit 错误。
- [x] Phase C：stream/non-stream 可用输出与 usage/terminal 一致性。
- [x] Phase D：确认未增加 region、Account 或 Provider fallback。
- [ ] Live gate：Builder/Social/Enterprise/API Key、多 region/profile、model/quota、stream/tool/401 receipt；Amazon Q 若未来进入产品范围须独立 Profile/driver 立项。

不变量证明：tool 隔离只改变已固定 Kiro 响应的局部解析状态；model/profile discovery scope 仍含唯一 Account、auth/token generation、profile ARN 和 runtime region；坏 tool 不触发任何身份重选。

## 5. Qoder / COSY

### 参考与取舍

- TokenRouter：`backend/internal/pkg/qoder/{auth.go,client.go,models.go,session.go,signature.go,site.go}`、`backend/internal/service/{qoder_gateway_service.go,qoder_model_aliases.go,qoder_token_provider.go,qoder_token_refresher.go}`、`docs/interfaces/qoder_upstream.md`。
- OmniRoute：`src/lib/oauth/providers/qoder.ts`、`open-sse/{executors/qoder.ts,services/qoderCli.ts,services/qoderCliResolve.ts,services/usage/qoder.ts}`、Qoder registry。
- 采纳：Global/CN site、PAT/device、COSY signing/session/job-token 的既有边界；本轮将 control version 固定为 `1.24.2`，COSY quota/refresh/auth/generation 数据面统一 `Go-http-client/2.0`，两站目录加入 `glm-5.3 -> gmodel`，客户端 effort 投影为 `low/high/max`。
- 拒绝：把 OpenAPI 的 `Qoder/1.24.2` 或 `Qoder CN/1.24.2` 误用为 COSY 数据面 UA、账号池、site 自动切换、模型名跨 entitlement 选择、entitlement 错误切号。

### Server 落点与阶段

- 实现：`src/domain/qoder.rs`、`src/clients/oauth/qoder.rs`、`src/proxy/{qoder.rs,qoder_runtime.rs,outbound_identity.rs,forwarder.rs}`。
- 测试：`src/proxy/forwarder/qoder_http_tests.rs` 明确断言 UA 与 `cosy-version`；model/effort/site/401 cases；`cargo test qoder --lib` 为 43 passed。

- [x] Phase A：区分 OpenAPI identity 与 COSY data-plane identity。
- [x] Phase B：Global/CN 1.24.2 与所有签名请求 header 收敛。
- [x] Phase C：GLM-5.3 model key 与 low/high/max effort。
- [x] Phase D：generation-scoped model/session/quota 与同账号一次 401 review。
- [ ] Live gate：Global/CN device、Global PAT、models、quota、三 Surface non-stream/stream/tool/401 receipt。

不变量证明：site 是 Account profile 的冻结 authority，401 只刷新/换取这个 Account 的 token；model alias 和 effort 只影响同一 Qoder session payload，不选择另一个 site、账号或 Provider。

## 6. Cursor

### 参考与取舍

- OmniRoute 当前快照及关键证据提交：`fa0cd5af1c9beec02fe0cf8eb964eb6757184e08`（account Agent endpoint discovery）、`c130f2aa1ccc7aaddd7a7685bd6a0e08136dccf1`（DeepControl refresh）；主要路径 `open-sse/executors/cursor/*`、`open-sse/services/cursorSessionManager.ts`、`open-sse/utils/cursorAgentProtobuf/*`、`src/lib/oauth/providers/cursor.ts`。
- 9router：`open-sse/{executors/cursor.js,providers/registry/cursor.js,services/cursorModels.js,utils/cursorProtobuf.js}`、`src/lib/oauth/providers/cursor.js`。
- 采纳：绑定 token 调官方 `ServerConfigService/GetServerConfig`；protobuf field 27 同时要求 `agentUrl`/`agentnUrl`；只信任 pathless、default-port HTTPS `api5.cursor.sh` 域族；API-key exchange 与 OAuth DeepControl refresh 使用官方默认 endpoint；one-hour exact-scope cache；discovery 和 AgentService 共享一次 401 budget。
- 拒绝：endpoint/account pool、跨 OAuth/API-key rail fallback、未受信 discovery origin、自动选择本机 active account、跨 scope cold resume、任意 Cursor builtin 在 Server 执行。

### Server 落点与阶段

- 实现：`src/proxy/cursor/{agent_endpoint.rs,agent_driver.rs,identity.rs,session.rs,credential_cache.rs,h2_client.rs}`、`src/domain/accounts/oauth.rs`、`src/state.rs`、`.env.example`、`docs/provider/cursor.md`。
- 测试：protobuf/trust/cache/bound bearer、pre-open 401 rekey、session/credential/catalog/tool/stream contracts；`cargo test cursor --lib` 为 197 passed。

- [x] Phase A：官方 discovery/exchange/refresh 证据与 trust policy。
- [x] Phase B：endpoint decoder、size bound、strict origin 与 singleflight TTL cache。
- [x] Phase C：把 discovery 接到 OAuth CLI/API-key SDK 两 rail，不允许互相 fallback。
- [x] Phase D：pre-open recovery 重算 generation scope、保持 conversation id，并与 AgentService 共用 `AuthRecoveryState`。
- [ ] Live gate：OAuth/API-key discovery、stream/tool/image、park/resume、rate-limit 和一次 401 receipt。

不变量证明：cache scope 覆盖 App、Provider id/revision、credential generation、runtime fingerprint、rail、principal 和 access-token digest。首个 discovery 或 AgentService 401 会消耗同一个 `AuthRecoveryState`；刷新后仅重建同 binding 的 session key，conversation id 不变，第二个 401 终止。

## 7. 国内 Coding Plan

### 参考与取舍

- OmniRoute：`open-sse/config/providers/registry/bailian-coding-plan/index.ts`、`open-sse/services/{bailianQuotaFetcher.ts,usage/bailian.ts,usage/glm.ts}`、`open-sse/config/providers/registry/glm/*`；Alibaba 区域证据提交 `c9d4a45f1883d7daf150bbff631f3e83b41aa5b4`。
- 9router `55628eea02eccb4d80738cbf5be342a6dbf53026`（Alibaba Chat catalog）与 `8ed9da7165340150be968e968f7d9ea33902c7e3`（GLM-5.3 OpenAI Coding rail）提供补充最小 live 证据。
- 采纳：Alibaba China/Global(Singapore) 两 Family、Claude Messages + `x-api-key`、Codex Chat + Bearer、固定区域 origin/route/catalog；无稳定官方 quota 时明确 `unavailable`；GLM-5.3 只进入有证据的 CN/Global Codex rails。
- 拒绝：console Cookie/HTML、Alibaba Token Plan 冒充 Coding Plan、推测 quota/余额、API key 池、按余额/地区/模型切账号、跨 Provider fallback，以及把 Qoder 同名模型外推为 Zhipu entitlement。

### Server 落点与阶段

- 实现：`assets/contract/provider-registry.json`、`src/domain/providers/{registry.rs,coding_plan.rs,runtime.rs}`、`src/clients/coding_plan_quota.rs`、`src/proxy/{adapters.rs,forwarder.rs}`、`web-src/src/server/{directProviderPresets.ts,providers/bundles/familyCatalog.ts}`、Provider audit sources。
- 测试：Registry 固定 origin/protocol/auth/route/model/quota、GLM-5.3 rail isolation、API/Web source contract 与 baseline audit tests。

- [x] Phase A：区域、entitlement、protocol、credential、quota 五维正交建模。
- [x] Phase B：Alibaba 双区域四 typed Profiles 与 UI-source Bundle 接线。
- [x] Phase C：quota `unavailable` fail-closed 和 GLM-5.3 Codex-only catalog。
- [x] Phase D：确认 static credential 只属于当前 Provider generation，401 policy 为 false。
- [ ] Live gate：Alibaba 两区域 Claude/Codex inference/model/error receipt；Zhipu CN/Global Codex GLM-5.3 stream/tool/reasoning receipt；其余 20 typed Profiles 的 Server 自有真实验收。

不变量证明：四个 Alibaba Profile 都是 Provider-owned static credential，合同中的 `retrySameCredentialOnceOn401=false`；origin、protocol 与模型由选定 Profile 编译，运行时不存在账号列表或 alternate Provider。

## 整体 review 结论

- 七类增强均未引入 Account selector、pool、round-robin、quota/cooldown/concurrency 选号或跨 Provider fallback。
- managed rail 的恢复继续使用共用 `AuthRecoveryState` 或对应同账号 generation-fenced 状态机；Provider-owned Coding Plan 则明确禁止 401 replay。
- 新 cache/replay/session 状态均带 Provider/runtime 与 Account/credential generation；Cursor 额外带 rail/principal/token digest，Kimi 额外带 Share/user/session/model family。
- `fixture-verified` 与 `live-verified` 仍严格分离。没有真实凭据，因此上述七类分数不因本轮本地实现自动上调；后续只应由脱敏 live receipt 关闭各项 gate。
