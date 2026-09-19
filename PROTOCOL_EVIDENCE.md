# Protocol evidence policy

`cc-switch-server` 的 Provider 产品范围、身份和运行时行为由本仓库维护，当前权威来源是：

- `assets/contract/server-provider-requirements.json`：必须覆盖的 ProviderType 与 App Surface；
- `assets/contract/provider-registry.json`：Family、Profile、Driver、凭据与模型策略；
- `assets/contract/provider-legacy-compatibility.json`：兼容窗口内只读的 S1/旧 Web fixture；
- `assets/contract/*-protocol.json`、本仓库测试及厂商公开协议：具体 wire 行为证据。

外部仓库可以在一次明确的协议研究中作为只读差异证据，但不是实现同步源，也不能成为构建、测试、发布或运行时输入。吸收证据时必须在本仓库形成最小合同、独立实现与回归测试；证据不足的能力保持 `live_pending` 或 fail closed。

历史上从其他开源项目改编的代码与界面工作记录在 `SOURCE_PROVENANCE.json`，完整归属与许可证见 `THIRD_PARTY_NOTICES.md`。这些记录用于合规和溯源，不赋予外部仓库当前产品权威性，也不要求 CI checkout 外部源码。

Provider 合同变更至少运行：

```bash
node scripts/audit/audit-server-provider-contract.mjs
node scripts/audit/audit-provider-coverage.mjs --check
node scripts/audit/audit-ui-provider-matrix.mjs --check
```

## 2026-09-18 Kiro tool names, profile fallback, and compaction gate freeze

Kiro 增量证据追加在 `assets/contract/kiro-reference-delta.json`，保留 KI-01～KI-05 历史合同。只读来源为 `kiro.rs@f413e7de` 的裸 namespaced tool 恢复、`kiro.rs@3194bb29` 的 profileArn 兼容回退，以及 `kiro.rs@0b8c7dec/@d62054f5/@13763b69` 的 Responses Compact 结构；完整 commit、路径和提交态 SHA-256 可由 `node scripts/audit/audit-kiro-reference-delta.mjs --check-sources` 可选复核。默认审计、构建、测试、发布和运行时不读取外部 checkout。

KI-N1 使用只含本次请求 `tools` 声明的注册表，依次执行实际上游名精确映射、完整原名精确匹配、唯一 `__` child 恢复和普通名保留。历史消息、Account、Share、session 与进程缓存均不能提供候选；同名普通工具优先，两个 namespace 的同 child 返回稳定 `KIRO_EVENT_STREAM_INVALID`，不猜测目标。fixture 覆盖 builtin、超长 hash、历史污染、分片参数及 Claude Messages、Codex Chat Completions、Codex Responses 三 Surface。

KI-N2 只在带 profileArn 的请求收到 403，或收到正文明确包含 `Improperly formed request` / `Invalid profileArn` 的 400 时，才在同一 `q.{region}` host 去除 ARN 重试。401、429、5xx、timeout/TLS/transport、decode 和成功响应结构错误均保留原分类并停止；没有独立 receipt 的 q→CodeWhisperer host fallback 已关闭。模型目录沿用已冻结的 403 区域候选合同，但新增的 400 回退同样受正文与“原请求确实带 ARN”双重约束。

KI-N3 当前只完成保守门禁：Kiro 与独立 Amazon Q 的 `/responses/compact` 或普通 Responses compaction trigger 在模型目录、凭据刷新和推理发网前稳定拒绝，decoy fixture 请求数为零。参考项目的 remote compaction 仅证明值得后续差分研究，不能替代本产品的绑定账号真实 receipt；`runtimeEnabled=false`、`live_pending` 和空 receipt 保持不变。

## 2026-09-18 Codex bootstrap, metadata, WebSocket fairness, and memory freeze

Codex 增量证据追加在 `assets/contract/codex-reference-delta.json`，保留原有 CX-01～CX-06 历史合同。只读来源为 `CLIProxyAPI@cb73cd99` 的空 bootstrap announcement、`CLIProxyAPI@b5ba02c2` 的大帧写入公平性、`codex2api@dc47d131` 的 input-item metadata 层级和 `codex2api@19ee8db4` 的请求生命周期内存预算；完整 commit、文件路径与提交态 SHA-256 可由 `node scripts/audit/audit-codex-reference-delta.mjs --check-sources` 可选复核。默认审计、构建、测试、发布和运行时均不读取外部 checkout。

CX-N1 在 HTTP JSON、SSE、WS 与 Lite 共用的 Responses 分类器中，只把载荷语义为空的 `response.output_item.added`、`response.content_part.added` 和 `response.reasoning_summary_part.added` 视为 Lifecycle；未知类型、文本、工具参数、server operation、错误或任意其他可见字段继续 fail closed 为 Business。CX-N2 只删除 `input[*]` 对象顶层的 `internal_chat_message_metadata_passthrough`，根对象、content、arguments 和用户嵌套同名字段逐字保留，且转换幂等。

CX-N3 将上游 Responses WebSocket 拆成单 reader/single writer actor、有界读写队列和 32 KiB continuation frame；大上传期间双向 Ping/Pong 与下游取消均有冻结时限。写入已提交、peer terminal 与 stale cached socket 的 replay 边界保持保守：只允许原 Provider、原 Account、原 rail 的 pre-commit WS→HTTP fallback，已发送或已产生业务输出时不重放，也不切换身份或站点。

CX-N4 为每个 Codex HTTP 请求或 Responses WebSocket turn 建立独立 sticky budget，统一计入 raw/decoded/normalized body、响应 wire/decoded body、transport pending、semantic prelude、tool arguments、normalized event 及 WS read/write queue。重试共享原预算；耗尽立即取消/丢弃当前上游，禁止继续 HTTP replay、账号/Provider/rail/site fallback，并返回稳定 HTTP 503（`Retry-After: 1`、`cc_switch_request_memory_exhausted`）或 WS error 后 Close。reservation 与最终 `Bytes` owner 绑定，terminal、cancel 和最后一个 body view drop 后释放；指标只含低基数组件、结果、字节数、limit 与 high-water，不记录内容、凭据或身份。默认 256 MiB，可由 `requestBodyLimits.memoryBudgetMb` 或 `CC_SWITCH_REQUEST_MEMORY_BUDGET_MB` 在 16～1024 MiB 内配置。

上述四项均为本仓库 `fixture_verified`。26 个 Codex WebSocket 回归覆盖 ping、公平写入、取消、handshake fallback、stale socket 与零 post-commit replay；压缩膨胀、raw+decoded+normalized 合计、稳定错误和 `Bytes` 生命周期另有专项测试。它们不构成真实 ChatGPT entitlement、生产网络性能或任何仍为 CX-03/CX-05/CX-06 `live_pending` 能力的 receipt。

`069f3ef` 将 Codex 429 scope、reset 解析和 capacity retry/error 决策收敛到 `src/proxy/providers/codex/`，`forwarder.rs` 继续持有 dispatch、共享总 attempt budget、CommitGuard、terminal 和 usage 编排。该结构切片不改变 wire，也不改变固定 Provider/Account、同账号 pre-commit 恢复和零 post-commit replay 边界。私有 receipt harness 将真实验收拆成 `gpt_image_2_5`、`gpt_image_2_5_flare`、`gpt_image_2_5_sunburst`、`ws_prewarm` 四个互不外推的 operation；前三项分别要求 exact-model generation/edit/Responses、usage/error/cooldown、capability、多副本和 Cloudflare 证据，WS 项要求上游接受、至少五个 benchmark sample、P50 TTFB 收益、连接复用和严格 pre-`response.create` WS→HTTP fallback。付费 `codex-images-real.mjs` 仅为 `probe_only/live_pending`，不能生成验收 receipt。

后续 EVID-N1 迁移把 Codex 差异资产提升为 append-only schema v2。原 schema v1 的 `policy`、`sources`、`capabilities`、`incrementalEnhancements`、`realAcceptance`、`wireGoldens` 六个字段分别由不可变 digest 固定；新增的两个干净只读 snapshot 为 `CLIProxyAPI@b773607e`（tree `a740e14d`）与 `codex2api@de41a5e3`（tree `b767333a`）。9 个 source delta 由 13 条不可变 observation 完整映射到 adopt、differential、live_gate 或 reject 处置以及 committed target baseline/implementation object；CX-R1 明确拒绝商业计价、自动 driver-model fallback、账号池、轮换和跨账号恢复。audit 默认只读取本仓库及其已提交 Git object，显式 `--check-sources` 才复核外部已提交对象。当前没有真实凭据，四个 operation 的 `receipt` 均为 `null`，状态保持 `live_pending`。

## 2026-09-18 Antigravity grounding, model capability, and conversation-edge freeze

本轮增量证据追加在 `assets/contract/antigravity-reference-delta.json`，不改写 2026-09-11 的历史 observation。只读来源为 `CLIProxyAPI@ef63d2e7/@7fcbdf88/@a9e92b81/@b681a1e0/@8c984672/@fd3e6623` 与 `Antigravity-Manager@734e2bde/@9fd77989`；每个完整 commit、路径和提交态 SHA-256 均由 `scripts/audit/audit-antigravity-reference-delta.mjs --check-sources` 可选复核，默认构建、测试和运行时仍不读取外部仓库。

后续 EVID-N1 迁移把该资产提升为 append-only schema v2：原 schema v1 `sources`、`capabilities` 和 file digest 原样保留，另追加 source commit/tree、观察时工作树声明、路径/符号、处置、target baseline、implementation commit 与 fixture 映射。audit 内固定历史 observation digest，默认只检查仓库内资产和目标合同；`--check-sources` 才读取外部已提交 Git object。`e5bfc34` 仅迁移 Antigravity lifecycle 编排并增加两条 rail 的私有 receipt validator，没有改变下述 requestType 决策，也没有把 fixture 提升为真实厂商证据。

Server 的独立 grounding 合同只读取选中候选，按 URL 去重 web chunk、忽略无效索引，并将 Gemini UTF-8 byte range 映射为下游 Unicode scalar range。非流式与流式分别生成 Claude `server_tool_use`/`web_search_tool_result`/citation、Responses `web_search_call`/`url_citation` 和 Chat annotation；搜索事件、文本、citation 与 terminal 的顺序由自包含 fixture 冻结，citation 不得晚于 terminal，URL/title 不进入日志或指标。ASCII、中文、emoji、组合字符、重复/缺失 chunk、跨增量 part 和只在最终快照出现 annotation 均有回归测试。

模型搜索能力只接受目录中的 `supportsWebSearch`、`supports_web_search`、`webSearchSupported` 或 `nativeCapabilities.webSearch` 布尔值；冲突或缺失为 `Unknown`。证据以 Account `authIdentityGeneration`、TTL 和有界 `model_search:*` dimension 持久化。请求开始时固定使用当前 Account snapshot：请求模型有新鲜 `Supported` 才保留；`Unsupported`、`Unknown`、缺失或过期时使用已验证的 `gemini-2.5-flash` fallback；fallback 被当前目录明确标为 `Unsupported` 时失败关闭。运行时绑定代际漂移在发网前冲突，不重新选择账号、Provider、rail 或站点。

转换边缘合同同时冻结为：billing metadata 只有在 trim 后为单独一行且以精确前缀 `x-anthropic-billing-header:` 开头时才从 system block 删除，多行正文、消息和工具参数不受影响；只有开头连续 system/developer 提升为顶层 system，中途指令按原顺序包装为 `<system-reminder>`；孤立、错配或部分工具输出以版本化 user 文本可逆保留，不伪造 pairing；空 Gemini text part 不关闭活动 Claude text block。

`requestType` 的两个成熟参考结论互相冲突，因此 AG-N6 没有生产行为变更。当前合同继续固定普通文本、仅 function tools 和历史 functionCall/functionResponse 为 `agent`，含 web search（包括 function + search 混合）为 `web_search`。`antigravity_oauth` 与 `agy_oauth` 均无真实 receipt，故该决策和 compaction 继续为 `live_pending`，fixture 不得升级为厂商接受性证据。

## 2026-09-18 Claude rate-limit, reasoning replay, and terminal freeze

Claude 增量证据追加在 `assets/contract/claude-reference-delta.json`。只读来源为 `CLIProxyAPI@44eaef00/@75ce6352/@377c315f/@2bcebaa8/@7c32971b`；实现与测试 Git object 的提交态 SHA-256 可由 `node scripts/audit/audit-claude-reference-delta.mjs --check-sources` 复核，默认构建、测试和运行时不读取外部 checkout。没有采纳账号池、跨账号/Provider fallback、credential cloaking、organization-hash 身份迁移或浏览器指纹模拟。

后续 EVID-N1 迁移把该资产提升为 append-only schema v2：原 schema v1 `sources`、`localContracts`、`realAcceptance` 分别由不可变 digest 固定，新增 `sourceExtensions`、两个 source snapshot 和 14 条 observation。冻结 snapshot 为 `CLIProxyAPI@b773607e`（tree `a740e14d`，工作树干净）与 `OmniRoute@02c663cd`（tree `25b36e49`，未提交改动明确排除）；12 个 source delta 均绑定完整 commit、tree、路径、符号与 source digest。audit 默认只从本仓库已提交 Git object 核对 target baseline/implementation tree 和 anchors，显式 `--check-sources` 才读取外部已提交对象。

14 条 observation 分别记录 adopt、differential、live_gate 或 reject 处置，并映射 CL-01～CL-06、CL-N1～CL-N4、CORE-N1、LIVE-N1 与 CL-R1。`CLIProxyAPI@fc96a87f` 的 organization-hashed credential filename 只作为拒绝证据：Account identity 继续由本仓库显式绑定，CL-R1 不声明 implementation commit 或 fixture，避免未来把 credential migration 当成待实现缺口。

429 分类只在 5h/7d 子窗口明确 `rejected`、利用率证据不冲突且每个被拒窗口均有合法 reset 时写当前 Account generation 的共享窗口 cooldown。`unified` 单独拒绝、缺失/非法 reset、冲突 header 和未知 429 只允许精确 model cooldown；健康共享窗口下的 `7d_oi` 只影响 Fable pool 或精确 model；fast-credit、overage-disabled 与 organization spend-cap 明确信号只记 request entitlement。reset/Retry-After 继续受时间范围和全局上限保护，指标只包含固定 scope/reason/evidence，不记录 header 原值。

CAQS EnvelopeVersion 4 仍视为厂商 opaque signature：Server 不解析、不生成、不截断，也不把内容写入日志或指标。native Claude 初次出站逐字保留非空字符串；Responses 流/非流通过本仓库带 MAC 的 reasoning carrier 往返恢复原块，空白、空值和非字符串不能被误认作可重放签名。初始 turn 的 billing/session fingerprint 在 system migration、cache metadata、后续轮次和 retry rewrite 前取值；后续历史变化不改变该初始锚点。

standalone、错配和部分 tool output 继续用版本化 user 文本可逆保留，不伪造 tool pairing。通用流终态 guard 在收到唯一合法 `message_stop` 后立即完成、结算 usage 并释放上游 body；终态后的无 EOF、取消或断连不再反记为 upstream failure，终态前断连仍严格失败。以上只证明本地 `fixture_verified`；真实 Claude rail、Fable entitlement、CAQS 接受性和限流 header 仍为 `live_pending`。

`6b0a0fe` 将 quota header 观测、429 scope 应用、Fable/Account/model cooldown 与 transport replay-safe 决策收敛到 `src/proxy/providers/claude/`，不改变 wire、usage、terminal 或固定绑定恢复语义。私有 receipt harness 将真实验收拆为 `oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1` 四个 operation；每项独立绑定 target commit、Account generation、Provider binding、Share revision、模型和可选 plan projection。fixture 只能生成 `contract_verified/live_pending`，真实 receipt 必须存放在仓库外并使用 `0600` 权限。当前未提供真实凭据，四项状态均保持 `live_pending`、`receipt=null`。

## 2026-09-11 Antigravity replay, session, schema, and transport freeze

本次只读差异研究冻结于 `assets/contract/antigravity-reference-delta.json`，对应 `CLIProxyAPI` commit `09a29bd345bc44c473abe7fd07859e32df2ea543` 与 `Antigravity-Manager` commit `85fb4fe688997d3a0c2930b7a202cf22f617b092`。外部源码只提供协议缺陷和边界的交叉证据，不进入本仓库构建、测试或运行时；默认审计只核对本地合同，人工运行 `node scripts/audit/audit-antigravity-reference-delta.mjs --check-sources` 才读取外部 Git object。冻结文件摘要为：

- `CLIProxyAPI/internal/cache/antigravity_reasoning_replay_cache.go`：`76498bed74e737f02aa708567942301131ff3c163270a4d575ee31fc87ee452f`；
- `CLIProxyAPI/internal/runtime/executor/antigravity_reasoning_replay.go`：`9c62466b0ba8e8435341f127953076406fd302dbf2e90428dc9f752db5381975`；
- `CLIProxyAPI/internal/runtime/executor/antigravity_executor_request.go`：`6f3a0e28045944b807ca2cf92c0e503bd15ca9636d8cf6183387c7344aa468a6`；
- `CLIProxyAPI/internal/util/gemini_schema.go`：`a824f1f36b0b3b5bbf16e5a629b4f37ba8a67e868645a1a1d7d554b70b0f4f74`；
- `Antigravity-Manager/src-tauri/src/proxy/common/session.rs`：`7c4fb3c753b6c0ba85e7c0e79ed0e5faecc6d786f698f1649506d2e76c0abccc`。

Server 独立实现只吸收四组有限事实：reasoning signature 需要 conversation/model 隔离并以 snapshot CAS 防并发覆盖；`request.sessionId` 必须按 conversation 派生，且只有精确的 1,048,576-token 累积上限错误允许同绑定升代重试一次；Antigravity 的 JSON Schema 接受集比通用 Gemini 方言更窄；连接池配置必须参与 client cache identity。实现进一步把 replay scope 扩展至 App、Provider revision/runtime、Account credential generations、Share、用户 namespace 与 upstream plane，所有恢复保持 pre-commit、同 Provider/Account 且预算有界。默认传输固定 HTTP/1.1 短连接；显式连接池按 credential scope 隔离，因此不声称或模拟 HTTP/2 GOAWAY 支持。

`CLIProxyAPI@70f45604522282965247add17f6cce619876919b` 的 compaction 仅是风险证据。其 capsule 使用固定字符串派生 secret，且本仓库没有真实 Antigravity receipt 证明该 wire 能力，因此未吸收实现。Server 明确拒绝 Antigravity `/responses/compact` 和 `compaction_trigger`，状态保持 `live_pending`；未来只有在冻结真实 receipt 后，才可另行设计使用本仓库根密钥按用途派生、绑定 Provider/Account/session/model 的 versioned AEAD。离线 fixture 仅支持 `fixture_verified`，不表示真实 Google entitlement 或长流已经验收。

## 2026-09-11 Claude differential contract freeze

Claude 差异合同冻结在 `assets/contract/claude-reference-delta.json`。一次性只读证据包括 `OmniRoute@a3ca33fa6442` 的 leading text system、`CLIProxyAPI@6a73f396` 的请求类别缓存 TTL、`@a59b1764` 的 trailing usage、`@4f038099` 的 structured output、`@ef99119e` 的顺序 content block，以及 `@35a47238` 的 helper request id。baseline 固定每个提交的实现与测试文件 SHA-256；`node scripts/audit/audit-claude-reference-delta.mjs --check-sources` 可在外部 checkout 可用时复核 Git object，但默认构建、测试和运行时不读取外部仓库。

Server 的独立合同为：confirmed-native Messages 只提升首个真实 turn 前的 text-bearing system/developer，directive-only 与真实中段 system 原位保留，最终 CCH 在移动后重算；main native 可使用 1h cache，subagent 默认 5m 但保留显式 1h，probe/helper/count_tokens 不继承 generation-only extended TTL。Claude→OpenAI Chat 只在原始下游 `stream_options.include_usage=true` 时，于 finish chunk 后、`[DONE]` 前发一个 `choices: []` usage chunk；input/cache read/cache write/output 跨生命周期按字段合并，显式零保留，失败前局部 usage 仍进入唯一终态账单记录。

OpenAI Chat/Responses→Claude 的 structured output 只在相应 `response_format` 或 `text.format` shape 上加入 Claude-scoped system instruction；普通 text/缺失 format 不改写。Chat→Anthropic 的 tool、text 与 reasoning block 始终严格顺序，后续交错内容或 tool 在当前 tool 关闭前有界保留并按 wire 次序释放。native helper 的 `x-client-request-id` 由真实 upstream base 决定：Anthropic first-party 缺值时生成 UUID v4，自定义 base 只保留客户端已有值而不凭空添加。以上 fixture 不引入通用兼容层、账号池、cloaking、跨账号或跨 Provider fallback；真实 Claude 账号仍为 `live_pending`。

## 2026-09-11 Codex differential contract freeze

Codex 差异合同冻结在 `assets/contract/codex-reference-delta.json`。一次性只读证据为 `CLIProxyAPI@e56abd56/@37ce368c` 的 Unicode schema/纯 const union、`@d1a024e9` 与 `codex2api@f5220891` 的 GPT Image 2.5、`CLIProxyAPI@25913086/@3ae9093d` 的 nested error/sequence/bootstrap overload，以及 `@bd03aabc` 的 prewarm/具名 tool output。基线记录完整 commit、文件路径与 SHA-256；`node scripts/audit/audit-codex-reference-delta.mjs --check-sources` 只在人工要求时复核外部 Git object，默认构建、测试和运行时不读取外部工作树。

Server 独立实现只吸收已能证明的正确性合同：工具 schema walker 仅进入 JSON Schema 关键字位置，并以 64 层、8192 节点和 1 MiB 保守字节预算 fail closed；只删除活动的 `\\p{`/`\\P{`（包括 `\\u005c` 绕过），大型 union 仅在所有分支为唯一、同类型、父类型兼容且无额外约束的纯 const 时变为 enum。Responses 同一 fixture 冻结 HTTP/SSE/WS 的 nested detail、sequence、首尾帧、具名无 call_id output 和 bootstrap overload；容量恢复与 WS→HTTP fallback 固定原 Provider/Account、只在 pre-commit 发生并共用总 attempt budget。

GPT Image 2.5 三个 variant 不从外部项目的静态声明推导真实 entitlement。当前没有真实 receipt，因此 `gpt-image-2.5`、`-flare`、`-sunburst` 分别保持 `live_pending`，不发布到 registry/UI，Dedicated Images 在发网前失败关闭。WS prewarm 也因缺 upstream receipt 和 TTFB 基准保持未实现；2.5 专属 quota/cache 治理依赖同一真实门禁。没有迁入 codex2api 的商业计价、账号池、轮换、跨账号或跨 Provider fallback。

## 2026-09-18 Cursor OmniRoute differential refresh

Cursor 差异合同冻结在 `assets/contract/cursor-reference-delta.json`。2026-09-18 从旧点 `a3ca33fa6442b59adc42976c795709eaf5351109` 复核到 OmniRoute `02c663cdd0e8577bdcf2b01a44046bcd46dc6a7a`：六个 Cursor protobuf/session/executor 提交对象的 SHA-256 均未变化，提交区间没有 Cursor wire、protobuf、auth 或 session 增量，因此 CUR-N2 结论为 `reviewed_no_wire_delta`，不修改生产 executor。参考仓库当时 22 项未提交/未跟踪内容全部排除。默认审计不读取外部仓库，只有人工运行 `node scripts/audit/audit-cursor-reference-delta.mjs --check-sources` 才会以冻结路径和 SHA-256 复核 object，外部 Node/Electron/SQLite/session UI 从不成为构建或运行时依赖。

EVID-N1 将合同提升为 append-only schema v2：原 v1 的 `capturedAt`、`policy`、`sources`、`incrementalReview`、`registryTruth`、`capabilities`、`enhancements`、`providerLifecycle`、`realAcceptance`、`protobufFixtures` 十个字段分别由 canonical SHA-256 固定；新增的 OmniRoute source snapshot 绑定 HEAD commit/tree，并明确记录 `worktreeClean=false`、22 项工作树内容全部排除。7 条不可变 observation 分别映射 CUR-01～03、CUR-N1/N2、CORE-N1、LIVE-N1，固定 source path/symbol/digest、处置理由及本仓库 committed baseline/implementation object 和 fixture，不允许用当前工作树冒充历史实现。

Server 自包含 hex fixture 固定 ServerConfig 和 interaction 的未知字段语义、重复 field 27/URL 失败关闭、Connect frame 任意分片与 partial EOF、成功/错误 terminal envelope、plain EOF 失败关闭，以及 fresh `composer-2.5-fast` 必须保留完整 wire ID。公开模型选择入口已经存在，因此 registry `special.cursor` revision 4 将 discovery 与 forward/test 一并标为 supported/`fixture_verified`；OAuth 返回静态 aliases，API-key 目录保持 exact Provider/runtime/credential scope，成功空目录权威，transient stale 只用于展示。

CUR-02/CUR-N1 仍是双 rail 真实证据缺口。`scripts/smoke/cursor-real.mjs` 每次固定一个 rail、Provider、Share 和 credential identity，只接受仓库外权限受限的私密 receipt；公开输出不包含这些标识。loopback 测试只产生 `contract_verified`/`live_pending`，OAuth 与 API-key receipt 不得互相推导，恢复也不得切换 rail、Provider 或 Account。

CORE-N1 切片以 `src/proxy/providers/cursor/` 作为共享 forwarder 与既有 `src/proxy/cursor/` 协议实现之间的生命周期 facade，收敛 Cursor adapter、模型选择、native driver dispatch 和 h2 timeout mapping，不改变 endpoint、protobuf、session、wire、attempt、terminal 或 usage。LIVE-N1 同时把 receipt 提升为 schema v2/harness revision 2：OAuth 与 API-key 仍独立，每份私有 receipt 必须绑定当前 target commit、App、Provider/runtime revision、Share revision、精确 credential generation、完整 `*-fast` model、22 项检查、10 份 body hash、5 项测量、固定恢复决策和 decoy/secret scan；真实文件必须在仓库外且权限为 `0600`。当前未提供真实凭据，两条 rail 的 receipt 仍为 `null`、状态保持 `live_pending`。

## 2026-09-18 Grok reasoning replay/root-union/quality-observation differential freeze

Grok 差异合同冻结在 `assets/contract/grok-reference-delta.json`。一次性只读证据只取 `grok2api@906b9493b099d192381c698d4e320fafeccb851c` 的十个已提交 Git object，并以 SHA-256 固定 conversation reasoning cache、明确 decode rejection recovery、Build tool root-union adapter，以及 `7f3f3d3ce030d5cf946b0bbe994f3026775fa308` 引入的启发式 quality hold/retry；历史 reasoning delta 仅记录 `8641a782`、`ca392e68`、`7d1b4246`、`3de758e7`、`22ac653a`、`72a3a347`、`5d19ccff`、`e5285ebe`。外部工作树未提交内容和 `sub2api` 的账号池、路由、fallback 都被排除；默认审计不读取外部仓库，只有人工运行 `node scripts/audit/audit-grok-reference-delta.mjs --check-sources` 才复核只读 Git object。

EVID-N1 将合同提升为 append-only schema v2：`269850a` 中原始 schema-v1 文件的 SHA-256、target commit/tree、全部十个历史字段的 canonical digest 和逐字段 digest 均被冻结。新增的 `grok2api@906b9493` 干净 snapshot 与 `sub2api@ab99d56e` dirty-worktree-excluded snapshot 固定提交态 tree；后者的 6 项本地修改全部排除，只追加四个已提交 object 作为拒绝证据。10 条不可变 observation 完整映射 GR-01～05、GR-N1、CORE-N1、LIVE-N1 与 GR-R1，绑定 source path/symbol/digest、处置理由以及本仓库 committed baseline/implementation object。GR-R1 明确拒绝 visible/plaintext 启发式重试、账号池/轮换、商业复合路由/计价和本地 soft quota gate；reject 记录不声明 implementation 或 fixture。

Server 的独立实现把 replay scope 固定到 Provider revision/runtime、Account auth/token generation、Share、签名用户、session/turn、model family、HTTP/WS rail 和 upstream plane；有界 cache 使用 CAS/tombstone，只从成功 completed 终态提交，并在重复/编辑 call、协议/容量/代际漂移时 fail closed。Grok Build root union 在通用 sanitizer 后解析有界 local `$ref`，只投影可证明的 object root，循环、混合 union、全非 object 与歧义 schema 在网络前明确拒绝。HTTP、CRLF/分片 SSE 与 WebSocket loopback 验证 capture/replay、parallel calls、一次 pre-commit 明确拒绝恢复及 post-commit 禁止恢复。

GR-N1 只吸收“需要观测响应质量形状”这一事实，不采用参考项目的 hold、最多六次尝试、跨账号轮换、推理 token 比例或密文长度启发式。Server 在成功 HTTP JSON、SSE 与原生 WebSocket Responses 上用有界状态机记录固定 outcome、terminal/tool/visible 布尔值和 `0|1_7|8_31|32_127|128_plus` 长度桶；不保留 plaintext/reasoning，不写 Provider、Account、Share、用户、prompt 或模型标签。单个至少 1024 字符的流式 bulk fragment 仅记为 `anomalous_dump` 诊断。观察器逐字节透传且没有 retry/rotation/response rewrite 决策入口；失败/不完整终态不冒充质量样本。

CORE-N1 将 Grok reasoning replay 的 scope 派生、snapshot ownership、CAS 清理/提交以及 Provider/Account/Share generation fence 收敛到 `src/proxy/providers/grok/`。共享 forwarder 只保留 wire 编排并调用 HTTP/WS facade；固定 Provider/Account/Share、同账号一次 pre-commit 明确拒绝恢复、共享 attempt/10 秒预算和零 post-commit replay 均未改变。

LIVE-N1 把真实门禁拆成 `inference`、`media`、`remote_compaction` 三个互不外推的 operation。每份仓库外 `0600` 私有 receipt 必须绑定当前 target commit、Provider revision/runtime、Account auth/token generation、Share revision、签名用户 namespace、精确 model/session/turn 和 fresh catalog，并精确匹配 checks、body hashes、measurements、固定恢复决策、零 decoy 请求与 secret scan。`grok-oauth-real.mjs` 仅为 `probe_only/live_pending`；fixture 只能产生 `contract_verified/live_pending`。当前没有真实 receipt，三项继续为 `receipt=null`、`live_pending`。

这些 fixture 只支持 GR-01..03 与 CORE-N1 的 `fixture_verified`。GR-04 的真实 inference/media/WS/version/cooldown/catalog 仍待 operation receipt；GR-05 remote compaction 明确 `runtimeEnabled=false`，即使未来 receipt 证明上游协议也不能自动启用，必须另行完成 scope、versioned AEAD、TTL、失败语义和降级设计评审。不得借用 Grok Web Cookie、跨账号 cache 或外部商业路由。

## 2026-09-11 Kiro prompt-cache differential freeze

Kiro 差异合同冻结在 `assets/contract/kiro-reference-delta.json`。一次性只读证据只取 `kiro.rs@22d2c2d0695ba350890072c19990f54782827ae5` 已提交的 `src/anthropic/cache_metering.rs` 与 `src/anthropic/stream.rs`，并记录 `f2cc574`、`19b7f4b`、`47633a4` 三个历史 delta。完整 commit/file SHA-256 由 `scripts/audit/audit-kiro-reference-delta.mjs --check-sources` 可选复核；默认构建、测试、发布、运行时和审计不读取外部工作树，也不采用其中 Redis、session affinity、账号调度或成本折算策略。

Server 的独立实现只在 Kiro 缺权威 token usage 时模拟下游 Prompt Cache 计量：top-level auto 与显式 breakpoint 统一限制四个，lookback 限 20 个 position，连续 tool_use/tool_result 分组，1h 必须先于 5m，命中按 entry 自身 TTL 续期，多轮 auto 不吞掉最新 user input。非法声明整次 no-cache 并记录固定原因。Provider/Share/Account/auth generation/route/session/model/thinking 等 scope 进入哈希，cache miss 或 hit 都不能改变原绑定。

本地 covered/read estimate 以整数比例映射到 Kiro context/metrics total，舍入余数归入 uncached input，三项严格守恒；上游 `tokenUsage` 或明确 cache 字段始终优先，包括显式 0。响应以 `cache_usage_source` 区分上游真值与本地估算，本地模拟不声称降低 Kiro 推理成本。持久化从同步整表 JSON 改为有界 non-blocking 通知、dirty mutation generation、合并/退避、shutdown flush 和 WAL/FULL SQLite transaction/CAS；旧 JSON 只导入并保留，磁盘失败不阻塞响应且通过 degraded 指标暴露。

这些 fixture 只支持 KI-01..03 的 `fixture_verified`。KI-04 必须按 Builder ID、IdC、Social、API Key 与 `us-east-1`、`eu-central-1` 分别留存真实脱敏 receipt，当前均为 `live_pending`。KI-05 shared/remote cache 明确 `runtimeEnabled=false`；没有多副本实际需求和真实证据前不得启用，也不得让 cache 命中参与账号选择、跨账号复用或 fallback。

## 2026-09-09 OpenAI Chat `created` compatibility freeze

OpenAI Chat Completions 的流式 `chat.completion.chunk.created` 是 Unix 秒级整数，且同一 completion 流中的所有 chunk 使用同一个时间戳。Server 将此视为所有 `/v1/chat/completions` Provider 出口的协议不变量，而不是 `grok_oauth` 的专用修补：跨协议 Responses/Anthropic/Gemini 合成必须从源头生成合法且流内稳定的 `created`，原生 OpenAI Chat 透传和专用 canonical emitter 还必须经过同一出口合同。非流式 `chat.completion` 同样不得缺失、输出 `null` 或输出非正整数 `created`。官方协议依据为 <https://developers.openai.com/api/reference/resources/chat/subresources/completions/streaming-events#chat.completion.chunk>。

本次一次性、只读差异研究冻结在 `/data/projects/proxy/Grok/grok2api` commit `8913b53fe92307a6f111b2885ab298a43c74a9ba`。只吸收以下协议事实，不复制实现，也不把该仓库加入构建、测试、发布、运行时或日常同步输入：conversation stream converter 在构造时生成一次 fallback `created`，Chat 的 role/content/reasoning/tool/finish/usage chunk 全部复用；上游存在有效 `created_at` 时优先保留首个值；Responses compatibility state 在首次确定后拒绝后续时间漂移；非流式时间为零时回退当前 Unix 秒。冻结文件为：

- `backend/internal/infra/provider/conversation/stream.go`：`82245c33dcb82fb6fc9184b5108fa3021781cae6df534fe7464e0987f062c224`
- `backend/internal/infra/provider/conversation/chat_stream.go`：`1390f7db068879527ab669816dfb159d475f7ff84fd0de8eb00a9718620b4533`
- `backend/internal/infra/provider/conversation/response.go`：`5cfa7396be4fa0d2c399538a95943f528824703bb9a1c37856c07ba42672175c`
- `backend/internal/infra/provider/conversation/chat_response.go`：`9786567cfa5f4338294498a1c140ae0ed467878e6963ab8b850755a6b13f712f`
- `backend/internal/transport/http/inference/responses_compat.go`：`a7b76f018f76c204a5137539a5821dedbb96f52c41997c933bcb4ace3da36037`

明确不采用 `backend/internal/infra/provider/web/chat.go` 的流式 Chat 时间设计：该文件在多个 chunk 分支分别调用 `time.Now().Unix()`，跨秒时会产生同流时间漂移。Server 独立实现采用流级 fallback、首个有效上游值和首次下游 Chat chunk 后冻结的状态机；只对成功 Chat envelope 补正字段，错误 envelope、SSE 控制字段和 `[DONE]` 不被伪装成成功响应。离线 fixture 只能证明兼容合同，真实 Grok OAuth/Grok CLI 仍保持 `live_pending`，直到按 acceptance runbook 留存脱敏 receipt。

## 2026-09-02 Claude Code 2.1.258 OAuth wire profile freeze

`claude_oauth` 的当前 wire profile 来自对官方 npm `@anthropic-ai/claude-code@2.1.258` native binary 的一次性静态审计，以及只连接本地 loopback、使用假凭据的出站请求捕获；审计过程没有访问 Anthropic，也没有保存或使用真实 access/refresh token。审计当日 npm `latest` / `next` 为 `2.1.258`，`stable` 仍为 `2.1.236`。发布漂移检查以 `latest` 为目标，同时只记录 `stable`；Server 构建和运行时都不访问 npm。

证据确认 Claude Code、Stainless、Node 与 Axios 的公开版本分别为 `2.1.258`、`0.112.1`、`v26.3.0` 与 `1.15.2`，并确认 `claude-fable-5-1` canonical model、mid-conversation system 能力、保留的 CCH 流程以及 prompt-derived billing suffix。billing suffix 以 salt `59cf53e54c78`、原始请求第一条 user text 的 JavaScript UTF-16 code unit 索引 4/7/20（缺位补 `0`）和有效 CLI version 计算 SHA-256，取前三位 hex；`ping` / `2.1.258` 的固定结果为 `1e2`。billing block 本身不带 `cache_control`，CCH 仍在所有 body rewrite 后生成。profile、算法常量和脱敏 golden 位于 `assets/contract/claude-oauth-wire-profile.json`，生产实现不读取外部二进制。

`sub2api` commit `34b8bf1a6` 只作为 Fable 5.1 目录和 billing fingerprint 概念的交叉证据；其 Go 实现按 UTF-8 byte 取索引，不能覆盖官方 JavaScript UTF-16 语义。审计时 TokenRouter/sub2api 仍广告 Claude Code `2.1.220`、Stainless `0.94.0`，且二者关于取消 CCH 的判断与当前官方 binary 不符，因此都没有作为版本、identity、CCH 或 beta 的实现来源。吸收范围不含其多账号号池、账号切换或 fallback 设计。

2026-09-03 对 TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 与 sub2api `6566039bc81e8a9af94077cb272eb3d3074702dd` 做了后续一次性只读差异审计。两者的 `backend/internal/service/ratelimit_service.go` 和 `account_usage_service.go` 交叉确认：Anthropic Messages 响应头会提供独立的 `5h`、`7d`、`7d_oi` utilization/reset，其中 `7d_oi` 是 Fable 周容量窗口；主动 usage 缺失该窗口时，可以用同账号成功推理得到的被动样本补齐显示。Server 只吸收这组有限 header 名称和“主动值优先、被动值补缺”的协议事实，并独立加入数值/时间范围校验、Account identity generation 隔离、单调合并、TTL/reset 过期和 Fable entitlement 门控。没有采用外部项目的 Extra map 存储、预测窗口、调度阈值、自动暂停、账号选择、号池或 fallback 行为；普通 utilization 也不会写入仅表示已耗尽的 `capacity_pool_limits`。

离线证据只能支持 `fixture_verified`。真实 Max 5x/20x inference、Fable 5.1 entitlement、streaming、限流和版本门禁仍为 `live_pending`，必须使用已轮换且不进入日志/命令历史的私密凭据按 acceptance runbook 验证后才能升级结论。

## 2026-09-02 Qoder CLI oracle freeze

`qoder_cosy` 的 native Rust 实现以一次性、只读的官方 CLI 审计作为漂移 oracle，不在构建或运行时加载 CLI。证据冻结于 `assets/contract/qoder-cli-oracle.json`：Global `@qoder-ai/qodercli@1.1.32` bundle SHA-256 为 `24de5b12520cbe49c0027b53654eaee02bddd857e3d9f19a6198824e365d89bf`，CN `@qodercn-ai/qoderclicn@1.1.32` bundle SHA-256 为 `5a82eeffbeb015d78c4945b7f4ed989494d2ea8cc7fdf2dbfc6ad04c17418f8b`。`cli2api` commit `9b18f2de06c53f12bf2c5112c7a71e3e64755b97` 仅提供带文件摘要的 capture/plaintext projection 交叉样本，不是依赖、同步源或生产 executor。

2026-09-11 将已落地的动态 entitlement/capability、三 rail、严格 EOF terminal、generation fencing 与同账号单次恢复统一确认为 `special.qoder_cosy` Driver contract revision 2。`assets/contract/qoder-reference-delta.json` 与对应审计把 Registry、三个 Profile、oracle、coverage、生产入口和 `live_pending` 状态交叉闭合，并冻结“先官方包 digest/独立 wire，后 oracle mutation，最后 Rust”的 CLI 升级顺序。TokenRouter 当前文档提交 `3488b4a9208c41e4f9db4108ef5133cb3710648c` 仅作为可选只读 source audit 输入。

2026-09-18 的 QD-N2 只读复核将 TokenRouter 提交态推进到 `7faf9469bc6957716923b5b4a98665c0fb9715e0`。两份 Qoder 文档对象摘要未变；`qoder_gateway_handler.go` 的净变化仅适配共享 error helper 新增返回值/参数，冻结 diff 摘要为 `384062fd869020540b945f654179f250ad75650f3fb5427c7e355b710b824eb2`，没有 Qoder origin、header、签名、payload、terminal 或恢复 wire 增量。因此官方 CLI oracle 继续优先，不修改 Server 生产实现。QD-N1 的 Global OAuth、Global PAT、CN OAuth receipt 仍三条独立 `live_pending`，任一 rail 或站点的成功不得继承给另一条。

两份官方 bundle 共同确认：Device authorization 使用 `/device/selectAccounts`、UUID v4 nonce 与 S256；poll 为 OpenAPI `GET /api/v1/deviceToken/poll`，1 秒间隔、300 秒 TTL、404 pending 且不发送 Authorization/COSY/User-Agent；refresh 为 OpenAPI `POST /api/v1/deviceToken/refresh`，只发送 JSON body `refresh_token` 与 `User-Agent: qoder/1.1.32`，响应主字段为 `device_token`、轮换 `refresh_token`、`expires_at`。Global 36 位小写 hex machine ID 与 CN UUID v4 machine ID 是独立站点事实。Qoder CLI `1.1.32` 和 COSY wire `1.24.2` 属于不同版本空间；旧 Global center job-token endpoint 不可作为 Device refresh fallback。

oracle schema v2 额外用审计脚本内的独立摘要冻结三 rail 的精确 origin、actual/signature path、Global/CN profile、完整 signed-header 集、encoding/signature vectors、两侧 projection 与每条 accepted-difference 原因，因而不能通过同时修改 fixture 两侧来维持假绿。canonical synthetic Chat 同时保存去随机 UUID 后的完整 server body；Rust 对 Global/CN 生产 builder 生成的整棵 JSON 做 exact equality，并验证 signed-header 集没有额外字段。Global profile/session 只接受恰好 36 位小写 hex machine ID，CN 只接受 RFC 4122 variant UUID v4，错站、空白、大小写、version 与 variant 在发网前失败。

2026-09-03 的查漏补缺继续把外部仓库限制为只读交叉证据：TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 的 `qoder_gateway_service.go`（SHA-256 `1fd0f4b37b96c04927a6c3e9dc7a6711ed0bd3422eec1f7e100ab42f103b7d22`）与 `gateway_forward_as_chat_completions.go`（`25fe31a1ca48f74a49da43a3178cfd33ae34525a560a60b5239782240491af6b`）用于核对 response/tool envelope；cli2api `b67278960df9c160d45a7520ee3110b5ccb84126` 的 `worker/src/plaintext.mjs`（`1f25273ac8b8b7b156ea70945bcbedcca38007cc5fea55d7360fa2d4ca85a413`）、`sse.mjs`（`a3620d3bcf49674136c55b536a41d6f161ae2bc5513ff29fab2452d5462686a6`）与 `errors.mjs`（`d22ed21e6cbc1fe3e807de662760cb218e6f8943c7806c4118a8e76eb1d47650`）用于核对工具历史、SSE 与 Retry-After。吸收内容只包含单绑定账号内的 bounded compatibility、错误/脱敏和容量治理；没有吸收账号池、权重、轮询选号、跨账号/跨站 fallback，也没有采用“缺失终态仍成功”或旧 Center refresh 行为。文件摘要与安全不变量冻结在 oracle 的 `compatibilityPolicy`。

CN `cosy-clientip` 现由 Server 自身出站路由决定并在 catalog、quota、auth-status、generation 四条链路共用：目标路由优先，remote-DNS 情况下允许不发包的默认路由探测，运维可用 `CC_SWITCH_QODER_CN_CLIENT_IP` 提供规范 IPv4；任何下游 forwarded header 都不可信。Global 保持 machine-ID client IP 语义。响应兼容统一 OpenAI final-message/content-block/reasoning/usage 与 Anthropic-style content/tool/message envelope，工具 result 只在当前 batch 唯一关联；这些归一化不改变唯一 terminal + EOF 才成功的规则。

验证计数由 oracle 的 `verification` 单源维护：63 项 Rust Qoder 专项、9 项 Node mutation、7 项 loopback real-harness fixture；生成 coverage 直接读取该元数据。Node audit 还精确校验 lifecycle method/path/timing/header/body/response、三 rail 隔离、quota、bounded compatibility 与 EOF terminal；Rust 测试直接消费同一 oracle 的 lifecycle URL builder、payload/header/catalog、quota parser 与 SSE decoder。终态只有在唯一 authoritative terminal 后读到上游 EOF 才成立。`scripts/smoke/qoder-real.mjs` 的三条真实 rail receipt 仍未提供，因此这些加固只提高离线 wire 可信度，当前证据仍只允许 `fixture_verified` / `live_pending`，不得写成 live verified。

## 2026-09-01 CodeBuddy OAuth evidence freeze

`codebuddy_oauth` 的实现合同来自一次性、只读的协议研究，优先级如下：

2026-09-11 的 revision 2 差分新增 `cli2api@e5893f0864149caef4c3b9752454190e45875c33` 空消息处理与 `@32aa108b7442cf168c1606d19f9691feafbc116c` 空 stream delta 处理作为只读交叉证据。本地实现额外保留空 content 的 tool result、严格 named choice 校验及唯一 terminal + EOF；`assets/contract/codebuddy-reference-delta.json` 固定源码摘要、CB-01 至 CB-04 与 Intl/CN 独立 `live_pending` 门禁。

2026-09-18 增量只读取 `cli2api@624874a0331f5e8f012ef104b633e69826487458` 及 cdc80d6、a9ae393、6ac6f83、d361559、eef5b2c、76c4dab 的已提交 Git object；参考工作树中的 `proxy.html`、`proxy.md` 未跟踪文件明确排除。CB-N1 对历史工具轮次执行连续、唯一 ID、一一配对及完整 JSON arguments 校验，修复可复现 11148，但不伪造 result 或修改 output。CB-N2 在空消息过滤前把 assistant `reasoning_content` 规范化为上游 `reasoning`，Responses reasoning/message/function call 由共享 transform 合并成同一 turn。CB-N3 的 Claude/Chat/Responses namespace 闭环已有 request-local 共享 bridge 覆盖，不增加裸 child 猜测。CB-N5 冻结 terminal 前截断、`[DONE]` 后无 EOF 与 marker 前后取消；共享 guard 会关闭上游 body、释放 Account/Share 租约，仍只有唯一 `[DONE]` + EOF 才成功。

参考实现的 `deepseek-v4.1-flash` 精确 native ID、顶层 reasoning 字段和 context-window 只是 CB-N4 二级信号。本仓库没有 CN 绑定账号 receipt 或冻结厂商目录，因此 reviewed allowlist 不开放该 ID，`runtimeEnabled=false`、状态保持 `live_pending`；不采用 `deep-model` 静默改写，也不把模型专属字段泛化。

1. CodeBuddy CLI `2.142.0` bundle、站点 overlay，以及国际个人订阅账号的脱敏真实流量；
2. 本仓库 [`docs/provider/codebuddy-oauth.md`](docs/provider/codebuddy-oauth.md) 已冻结的端点、OAuth、refresh、目录、计费与 terminal 约束；
3. `cli2api` commit `9b18f2d` 的 WorkBuddy CN/Global adapter，仅作为国内实现、payload、错误投影与缺陷的交叉样本。
4. `cli2api@624874a0331f5e8f012ef104b633e69826487458` 的提交态增量，只用于 CB-N1～CB-N5 差分，不覆盖前两级证据。

实现不得在构建或运行时读取上述外部源码。国际站固定为 `https://www.codebuddy.ai`，国内站固定为 `https://copilot.tencent.com`；站点属于账号身份，不允许失败后换 host。CLI `2.142.0` 证据优先于 `cli2api` 使用的旧 `2.139.0` wire。国际 fixture 可据此标为离线通过；国内真实数据面、企业账号、图像/视频与本仓库真实订阅 receipt 仍是 `live_pending`。

2026-09-03 又对 `/data/projects/proxy/CodeBuddy/workbuddy-switch` 做了一次性只读差异审计，只吸收单账号控制面证据：`domain` 决定受控 billing origin，summary / paid / free 三路新资源接口及旧 `/v2` 回退，`X-Client-Platform: web`，逐请求用量分页与 prompt 字段白名单裁剪，以及闲置 refresh session 的 `12153` 终态。Server 独立实现将稳定身份冻结为 `site + uid + enterpriseId`，把 domain 降为同站可更新路由属性；任一路资源 401 交给已有单次 refresh/replay，官方用量只缓存安全投影。明确不吸收该项目的账号池/轮换、自动签到、进程与本地配置切换，也不吸收 prompt rewrite。企业账号和多模态继续 fail closed；未执行真实订阅验收，因此 registry 的 forward/test/discovery 仍保持 `live_pending`。

## 2026-09-01 Trae CN Solo evidence freeze

`trae_solo` 的 wire 来自对 `cli2api` commit `9b18f2d` 中 `internal/providers/trae/` 的一次性只读审计。采纳的事实仅限固定端点、Cloud-IDE 身份头、Solo payload、模型详情、订阅额度与 `metadata/output/token_usage/done/error` 事件结构；实现由本仓库独立完成，外部源码不是同步源或依赖。

固定出站 origin 为 OAuth `https://api.trae.com.cn`、Agent `https://trae-api-cn.mchost.guru`、Billing `https://api.trae.cn`、浏览器授权 `https://www.trae.cn`。明确拒绝参考实现中的三类行为：callback 消费任意 pending flow、导入的 `api_host` 控制凭据目的地、以及 EOF/error 后合成成功终态。Server-native Solo bridge 不等于 Trae IDE MITM、插件注入或桌面流量劫持。

离线 fixture 或 mock 只能证明合同接线；真实 OAuth、refresh、目录、额度、三 Surface 流式/tools/reasoning、401 恢复与错误码仍保持 `live_pending`，直到本仓库留存脱敏 receipt。
