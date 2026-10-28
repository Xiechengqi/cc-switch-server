# Protocol evidence policy

## 2026-10-26 账号重置周期：实现与证据门槛

核对既有quota parser与生成`docs/provider/coverage.md`，展示resets_at还可能是账期/包到期，不能通用触发。实现只使用Codex明确604800秒共享窗（绝对优先、缺失才相对秒）、Claude共享seven_day（含同身份被动头），不将短窗/review/模型桶混为账号主周期。Codex HTTP/WS被动观测带provider revision/account incarnation/identity generation fence；WS仅接受当前WHAM风格形状，离线fixture不证明全部真实WS schema。

条件adapter包括Kiro/Q的AGENTIC_REQUEST订阅base、Cursor Dashboard included原账期、Copilot有限premium及既有paid/free UTC日期、Kimi共享weekly、GLM明确type/unit、MiniMax active general weekly毫秒、Volc AFPWeekly.ResetTime毫秒/Coding ResetTimestamp秒。每种均有字段缺失/单位/桶隔离负例；不从数组位置、展示名、最大duration/最晚reset推主桶。Provider-owned来源固定Family/origin/credential scope，沿用总credential generation保护quota slot，不伪称独立slot计数器。

公共codex-resets.com按实施规划2026-10-26只读OpenAPI核对冻结：executed regular，observed或白名单x_post，announced_at作为publicAnnouncement业务边界，不是逐账号执行时间；scheduled/watch/banked不触发。运行时固定无auth HTTPS、禁止redirect、分页/ETag/大小/退避有界，状态变化不代替history。status只作变化探测，不声称完整schema已被证明。

兑换仅2xx/windows_reset数量不足以证明主桶。预留reset_windows(scope=shared,limit_window_seconds=604800,reset=true)明确回执形状的正例只验证本地防护，没有厂商原始回执证据；现有计数回执仍unknown、不重消费。restore fence后无自动reconcile解锁。不得将自造fixture当厂商证据。

Grok主桶、Qoder status/稳定scope、Antigravity/AGY summary与Share模型覆盖尚未闭合，明确conditional；CodeBuddy/Trae包到期不触发；Ollama/Gemini模型桶/按量key/不可查询Plan拒绝不可靠周期。TokenRouter的project-doc会话限制继续保留，未读取其业务源码。全部25 ProviderType/20 typed Profile分类、研究时固定参考HEAD、延期开放门槛及53项离线证据见`docs/share/account-reset.md`与原实施规划。没有新增参考工作区的构建/CI/运行依赖，所有真实验收独立live_pending，未部署。

## 2026-10-25 Grok CLI 明确版本拒绝后的默认身份更新

用户报告 Grok OAuth / Codex 检查收到上游错误：`Your Grok CLI version (0.2.111) is outdated. Please update to version 1.0.13 or later via grok update or the installation documentation.` 这满足既有 GR-N4 对明确 version-rejected signal 的要求。Server 使用自己的身份常量，宿主机升级 Grok CLI 不会改变检查请求；现将统一默认版本与 User-Agent 更新为 `1.0.13` / `grok-shell/1.0.13 (linux; x86_64)`，模型目录、device flow、quota 和 HTTP/WS/media 共用该版本入口。已有环境变量覆盖继续优先，部署升级必须同步清除或更新旧值。

本次依据是用户报告的拒绝及其给出的最低版本，没有将参考项目的更高版本当作接受性证据。旧 observation/GR-N4 extension 保持不可变历史，当前身份变更单独记录于 `assets/contract/grok-reference-delta.json` 的 `cliIdentityUpdate`。没有新增出站 header；group header、remote compaction 与真实 operation receipt 的门槛保持原状。离线回归不能证明固定账号已接受新版本，真实验收仍为 `live_pending`。

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

## 2026-10-20 Codex OAuth 带序号 keepalive 的 Responses SSE 兼容边界

远程 Router 的只读故障调查显示，目标 Share 在 2026-10-19 UTC 有 1,293 次 502、925 次 200；其中 1,283 次 502 在 30–35 秒结束。目标 Server 上传的失败审计同属 `response_finalize / cc_switch_forward_failed`，留存的错误响应为 `Responses SSE liveness event contained a non-liveness payload`。Router 到目标 Server 的隧道连接失败会走 503，而这里的 502 是目标 Server 在业务帧提交前的协议拒绝。生产原始上游心跳帧没有留存，不能据此断定每次失败的确切字段；约 31 秒的聚集时间也不是本地默认 120 秒首事件超时的证据。部署版本 `6545191` 与本地修复前的该解析器一致。

只读核对干净的 `CLIProxyAPI@ae3ee3be303fb07fb8173279923413005bf32177`：`internal/runtime/executor/codex_executor_grokbuild_keepalive_test.go::TestCodexExecutorExecuteStream_GrokBuildConvertsKeepaliveToSSEComment` 明确使用 `event: keepalive` 和 `{"type":"keepalive","sequence_number":3}`，并将其转换为下游 SSE comment。次选 `codex2api@5ee6c29273773a2398531d061d23f199ca8c25f7` 的 `proxy/executor.go::ReadSSEStreamWithEvent` 对 SSE data 不做该字段形状拒绝，但其宽松读取不是本 Server 的成功/失败合同。官方 [OpenAI Docs Responses streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses) 说明公开 SSE 用法，不定义私有 OAuth keepalive 字段，也不能证明生产每一帧与参考 fixture 相同。

目标 `ResponsesTransportDecoder` 原仅将单键 `type=ping|keepalive` JSON 认作 liveness；上述已声明 `event: keepalive` 的固定反例在旧实现按同一错误文案失败。现仅把 `type=keepalive`、可选非负 `u64` 整数 `sequence_number` 纳入心跳白名单，`ping` 仍只允许单键对象；再次对候选原始 JSON 作严格反序列化，避免未知/重复顶层键及 `null`、浮点、字符串、负数或越界序号被误识别为心跳。SSE `event`/`type` 冲突、业务/错误字段伪装仍失败关闭；无声明事件的非法候选不静默滤除。固定低基数指标记录被接受的带序号心跳和本次 `non-liveness payload` 拒绝分支，不记录序号、原始帧或请求正文。普通 HTTP 流、强制 SSE 的非流聚合、Dedicated Images 和 WS→HTTP fallback 共用该入站修复；出站 wire、首事件/idle 时钟、终态门禁、usage、单账号/Share 绑定与同账号 401 预算均不改变。

离线回归只能证明参考形状被正确处理，真实 OAuth 接受性与目标 Share 的 502 指纹消退仍须在目标 Server 发布后独立观察；本次不修改远程 Router，也不把公开文档或参考样例误报为生产原始帧证据。

## 2026-10-14 Qoder 非流输出与嵌套错误分类

只读对照 `TokenRouter@7faf9469bc695` 的旧 `service/qoder_gateway_service.go` 与 `TokenRouter@6d676f1d100e11f2f068949f5779e960c862079c` 的 `upstream/qoder/{execute,stream_conversion}.go`：Chat、Messages、Responses 非流路径都先收集 SSE 事件，再构造客户端 JSON、记录用量并提交会话；对应构造函数的文本、工具、停止原因和 usage 映射保持，Responses 桥主要改为注入时间/随机源。较新的 `c25dbf0c` 对这些非流构造函数主要补充注释，未形成应照搬的新 COSY wire。参考会在 `IsDone` 提前停止读取；目标仍要求唯一终态和真实上游 EOF。

参考 `upstream/qoder/client.go::applyQoderErrorPayload` 可从嵌套 `data` 和 JSON 字符串 message 提取厂商错误码与限额重置时间；从 `6d676f1` 到 `c25dbf0c` 这段主要是说明性注释。目标原只读取顶层及一层 `error`/`data`，使两层 `data.code=115` 的固定反例误作普通 400。现在只在至多 8 层的 `error`/`data` 链上查找，并把至多 64 KiB 的 JSON 字符串 message 用作缺失码/重置时间的后备；外层非 null 错误码优先、仅公开 `112`/`115`、文本脱敏和一次同账号 401 预算不变。该离线分类修复不证明厂商真实错误形状，三条 rail 的真实验收在本 goal 外且继续 `live_pending`。

## 2026-10-14 Qoder Responses 续接声明不得静默丢失

`Qoder/AGENTS.md` 首选 TokenRouter；其当前工作树已删除阻断 `project-doc` 的根目录 `AGENTS.md`，但删除未提交，故这里只读取已提交业务对象。`TokenRouter@7faf9469bc695` 的 `backend/internal/service/qoder_gateway_service.go` 已有 `previous_response_id`→会话别名和追加计划，当前 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 `backend/internal/upstream/qoder/{payload.go,conversation.go,execute.go}` 延续该行为。参考的账号池和宽松状态回退不进入本 Server。

目标 Codex Responses→Chat 转换原会丢掉 `previous_response_id`，然后 Qoder 专属入口照常发起新生成。三 rail × 流/非流的未知响应 ID 回环在旧实现返回 200，证明客户端续接声明可被无声忽略。中间安全门槛曾拒绝全部非空 ID；当前实现仍在转换前校验原始请求：缺失/`null` 不变，非空且至多 512 字节、无首尾空白的未知或不可用 ID 返回固定 409 `response_context_unavailable`；畸形类型、空 ID、空白边界和超长 ID 返回固定 400。命中当前账号的有界别名后，只向原 COSY session 追加本轮 input；新别名只在唯一完成终态、上游 EOF、身份和输出转换均校验通过后，随最终完成事件的交付边界发布。该局部续接不改变其他 Surface、三 rail 单账号/Share 绑定、同账号 401 预算、唯一终态+EOF 或 receipt schema；真实账号验收在本 goal 外，状态继续 `live_pending`。

每次 Qoder Codex Responses 请求都有独立下游响应 ID，SSE 规范化层覆写可能重复的上游 Chat ID，流式与非流式由同一个请求 ID 派生 `resp_`。响应别名缓存已接入转发：键包含当前 runtime、Share、用户、精确上游模型键和完整响应 ID，值还核对原始下游模型名、system/tools 指纹与待配对调用；单模型策略把不同下游模型映射到同一上游键时也不能串接。缓存有 2 小时 TTL、1024 项、8 MiB 总量、64 KiB 单项上限；续接先只读现有 session/catalog，原子领取别名后才发网，未知、过期、重复、并发或身份漂移均在发网前失败。省略的 system/tools 从别名继承，显式漂移与无匹配的工具结果拒绝。三 rail × 流/非流首轮→次轮、工具结果、取消、Share/用户/模型/账号漂移和并发回环已通过；这些离线证据不替代厂商真实接受性，TokenRouter 当前 HEAD 剩余 committed-source 补审和最终整体 review 仍待完成。

## 2026-10-14 Qoder 候选终态必须等上游 EOF 才交付

只读核对 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 `backend/internal/upstream/qoder/execute.go` 与 `871edf920`：参考把流式推理从客户端取消信号中分离，仍由服务端 abort 和固定超时约束，以便断连后收集尾部 usage；它的 Key 租约、会话别名和后台执行所有权不同于本 Server，不能直接移植。目标 Qoder decoder 虽然只在唯一终态加上游 EOF 后置 `is_terminal`，但流格式转换器原可把终止理由立即转换成 Claude `message_stop` 或 Codex `response.completed`。因此客户端取到完成事件后马上关闭时，成功账务还排在 `yield` 后，被误记为取消；上游在候选终态之后发错时，成功事件也可能先泄漏。前者的 Global OAuth 固定反例先红，后者的三 rail × Claude/Codex 回环证实此边界。

现在 Qoder 专属流暂存候选终态开始的下游输出，保留先前正文流出；暂存计入同一请求内存预算。只有 decoder 通过唯一终态、上游 EOF、runtime 身份和转换收尾校验后才交付暂存事件，并在交付最终 chunk 前记录成功账务。终态前断连、尾部错误、超时或内存失败仍走中断/失败，不会发出早到的完成事件。Codex 续接别名在同一最终交付边界先发布：客户端收到 `response.completed` 后可立即发下一轮，不必额外 poll HTTP EOF；若发布失败，当前流先发送失败事件而不交付成功终态。客户端在最终 chunk 交付前取消不会发布，交付后的取消视为已完成。该局部修复不改 COSY 请求 wire、401 恢复预算、Share/Account 单绑定、其他 Provider 的流转换、receipt schema 或三 rail `live_pending` 状态；真实账号验收仍在本 goal 外。

## 2026-10-14 Grok Responses SSE 流首 UTF-8 BOM

按 `Grok/AGENTS.md` 核对干净的首选 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `backend/internal/infra/provider/cli/responses_response.go::normalizeResponseStream` 和 `responses_advanced_test.go::TestResponsesStreamAlwaysStripsBOMAndDoomLoopControlEvents`：参考在兼容 Responses SSE 边界去掉流首 UTF-8 BOM；次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六处未提交 Codex 文件排除。参考不证明目标 direct-XAI OAuth rail 实际发送 BOM，但流首 BOM 是可确定的 SSE 解析边界。

目标 Grok 包装层原把 BOM 原样下传；当它紧贴首个 `data:` 字段时，共享 SSE 解码器把该行当作未知字段，可能丢失唯一成功终态。任意分片的固定反例在旧实现先红。现仅在 Grok OAuth Responses SSE 入口探测最多三个流首字节，完整匹配 BOM 才移除；非匹配前缀、流中间的 BOM 和 EOF 时不完整前缀原样保留，交给原协议门禁。首个 `data:` 完成事件随后可由主 Responses 检查器确认成功；探测缓冲计入 Grok 请求内存预算。不改出站请求、HTTP JSON/原生 WS、Account/Provider/Share 固定绑定、同账号恢复、搜索/质量成功门槛、GR-N4 或 receipt schema v1/revision 1。离线反例不构成厂商真实接受性，也不启用 remote compaction；不做 UI 自动化或部署测试。

## 2026-10-14 Antigravity Gemini 尾部 assistant 纯思考回合

按 `Antigravity/AGENTS.md` 只读核对两个推荐且工作树干净的参考：`CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `a2718314` 中的 `DropGeminiTrailingThoughtOnlyContent` 同时识别 `model` 与 `assistant` 角色，先清理纯思考尾部回合再补 Gemini 边界；`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的 thinking store 对 `thought:true` 和工具/正文区分提供交叉背景，但没有定义此尾部删除规则。参考不证明目标双 rail 真实收到该形状。

目标最终 v1internal wire 规范化已清理 `model` 纯思考尾部，却漏掉其自身其他尾部判断也接受的 `assistant` 角色。`user` 后跟签名 `assistant` 纯思考回合的固定反例在旧实现先红，尾部回合原样残留。现仅将尾部纯思考清理谓词扩展到 `assistant`：仍要求每个 part 显式 `thought:true`，且只含思考文本/签名字段，至少保留一个回合；带正文、工具调用或媒体的 assistant 回合、Claude 目标和其他历史不删除。只修 Gemini 目标的最终出站内容边界，不改 endpoint、签名、Account/Provider/Share 单绑定、AG-N6 requestType、AG-D3 replay/compaction 或 receipt schema v2/revision 2。离线反例不能替代 Antigravity/AGY 双 rail receipt、厂商接受性或当前未提交工作树 committed-target observation；不做 UI 自动化或部署测试。

## 2026-10-14 CodeBuddy 非流收尾调用栈与取消边界

按 `CodeBuddy/AGENTS.md` 只读核对首选、业务工作树干净的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go::Aggregate/rewriteChatStream`；两个未跟踪文档排除。参考分别聚合非流和清洗流式输出，但没有本 Server 的共享 `forward_with_attempt`、usage durable commit 或测试线程栈，因此不从它推断新的 WorkBuddy wire 或任务生命周期。

目标默认 2 MiB Rust 测试线程栈下，CodeBuddy Provider 三 Surface 自测单独运行会 SIGABRT 栈溢出；3 MiB 时通过。GDB 在溢出点观察到 `forward_with_attempt` → `forward_codebuddy` → `finish_qoder_nonstream` → `log_usage` → `commit_usage_append` 的嵌套 poll 调用链；堆化测试 future 或仅堆化收尾 future 仍失败。现仅在 CodeBuddy 非流已完成上游聚合后，将共享收尾 future 交给独立 Tokio 任务，正常路径等待结果；复用既有 `ProviderStreamTaskAbortGuard`，外层请求取消时中止该任务并释放随任务持有的 Account/Share guard。默认栈固定反例先红后绿，默认栈 library 全套通过。该改动不改变 WorkBuddy 出站、SSE 内容、错误分类、同账号恢复预算或 Intl/CN 绑定；它也不证明生产环境曾发生栈溢出。完整默认栈 `cargo test` 的 API 合同目标仍有独立 Claude/Kiro 用例栈溢出，留待相应类别处理。两站真实 receipt、CB-N4 CN 精确目录/接受性与未提交工作树 committed-target observation 仍缺；不做 UI 自动化或部署测试。

## 2026-10-14 Qoder HTTP 200 内层错误不能被成功终态遮蔽

`Qoder/AGENTS.md` 首选 TokenRouter，但其 `AGENTS.md` 要求先调用当前会话不可用的 `project-doc` 技能；遵守门禁，未读取其业务源码或运行访问该源码的 `--check-sources`，最新状态仍是 `unreviewed_skill_gate`。只读核对次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 的已提交 `internal/providers/qoder/direct.go::upstreamError/collectUpstream/writeOpenAIChunks`：它观察 HTTP 200 SSE body 内的非空 `error`，而不只看 wrapper HTTP 状态。未跟踪的 `proxy.html`、`proxy.md` 未纳入证据；参考的账号池和较宽松终态策略不迁入本 Server。

目标 `QoderSseDecoder` 原把 `{"error":{"code":115,"message":"quota exhausted"}}` 的内层事件投影为无意义帧并丢弃；固定反例在旧实现先红。现于已验证的 wrapper 与已解析的内层 JSON 边界，非 `null` 的顶层 `error` 先于业务规范化进入现有结构化上游错误分类：已知 `112`/`115` 保持权限/限额语义，无法分类且无限额时间线索的错误以 502 失败；`error:null` 仍不妨碍正常业务。即使同帧带成功 `choices`，或前面已有部分业务输出，也不把失败改成成功终态。仅收紧 Qoder 入站 SSE，不改 COSY 出站、三 rail 单账号/Share 固定绑定、同账号恢复、唯一终态+EOF 或 receipt schema v2/revision 2。离线反例不证明厂商真实发送该形状；三条真实 rail receipt、TokenRouter 合规补审、当前未提交工作树 committed-target observation 与最终整体 review 仍缺。不做 UI 自动化或部署测试。

## 2026-10-14 Kiro 权威 tokenUsage 数字形状不得截断

按 `Kiro/AGENTS.md` 只读核对工作树干净的首选 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/metadata.rs::TokenUsage`：四个计数是带缺省零的 `i32`，缺失 `tokenUsage` 可用其他来源，已提供的快照才是本轮权威。参考的 typed 解析不把浮点、字符串、数组或越界数转换成有效 token 计数；它不证明八个 auth-kind×region 的真实事件形状或 KI-N5 context-output 分摊。

目标 `KiroUsageAccumulator` 原将 `tokenUsage` 的浮点数截断到整数，非数字或越界字段还可能变为零值权威快照。完整 EventStream 固定反例在旧实现先接受 `outputTokens=2.75`，与精确计量合同冲突。现于非流和流式共用的 wire 解析入口检查 `messageMetadataEvent`/`metadataEvent` 的权威 `tokenUsage`：显式计数须为 JSON 整数且在 `i32` 范围内，对目标既有 camel/snake 别名一并检查；`tokenUsage` 仅可为对象或 `null`。畸形快照以固定、脱敏 `KIRO_EVENT_STREAM_INVALID` 失败，不再输出成功终态；缺失字段默认零、合法负整数后续归零、无 `tokenUsage` 的回退仍保留。`metricsEvent`/context 百分比及 KI-N5 shadow 未在此项重新定权。只收紧 Kiro/Amazon Q 入站计量，不改出站请求、EventStream 分帧/CRC/容量、Account/Provider/Share 单绑定、同账号恢复、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不替代八份真实 receipt、KI-N5 专项接受性及当前未提交工作树 committed-target observation；不做 UI 自动化或部署测试。

## 2026-10-13 Grok 私有 doom-loop SSE 控制帧的兼容边界

按 `Grok/AGENTS.md` 只读核对干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `backend/internal/infra/provider/cli/responses_response.go::isPrivateBuildControlEvent` 和 `responses_advanced_test.go::TestResponsesStreamAlwaysStripsBOMAndDoomLoopControlEvents`：Build Responses 流可能包含 `response.doom_loop_check` 控制帧，兼容出口不把它当公开 Responses 事件。次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件排除；该参考不证明目标 direct-XAI OAuth 账号真实发送此帧，也不定义新出站 wire。

目标 Grok SSE 检查器原只规范化普通 ping，私有控制帧会原样下传。固定分片反例在旧实现先红。现仅当可解析对象的 `type` 精确为 `response.doom_loop_check`、可选 `event` 与之相同，且没有失败/错误/终态语义或 `id`、`retry`、未知字段时，将该帧改为无数据 SSE 注释；否则逐字节保留并由既有主 Responses 门禁判定。HTTP 回环确认文本与成功终态保留、私有帧不出现在 Codex Responses 客户端。此项仅改 Grok OAuth Responses SSE 入站，不改 HTTP JSON、WebSocket、出站请求、Account/Provider/Share 单绑定、同账号恢复、搜索/质量成功门槛、GR-N4 版本/header 或 receipt schema v1/revision 1。离线 fixture 不证明 xAI 真实发送；inference、media、remote_compaction 三份真实 receipt、GR-N4 同账号接受性及当前未提交工作树 committed-target observation 仍缺，不做 UI 自动化或部署测试。

## 2026-10-13 Qoder 强制工具选择不得退化为全部工具

`Qoder/AGENTS.md` 推荐 TokenRouter，但该仓 `AGENTS.md` 要求当前会话先调用不可用的 `project-doc` 技能；本轮遵守门禁，没有读取其业务源码或运行外仓审计，最新参考增量仍是 `unreviewed_skill_gate`。次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 的已提交 `internal/providers/qoder/adapter.go` 保留客户端 `tools` 和 `tool_choice` 字段；其未跟踪的 `proxy.html`、`proxy.md` 排除。该参考不定义目标 Server 无法向 COSY 直接转发选择字段时的错误策略，也不授权迁移账号池。

目标生产 payload builder 将强制函数选择投影为只保留对应工具，但原在未找到名称时回退全部工具，静默把“只能调用指定工具”扩成“可以调用任意工具”。Global/CN 的固定请求反例旧实现先红。现在对已声明名称精确收窄；无效、未声明的函数选择以及无工具的 `required` 在发网前拒绝，`none`、`auto`、合法 `required` 和合法指定函数保持各自语义。Claude/Codex/Gemini 入口先归一为 Chat 再共用此 builder；这只修本地请求保真，不改 COSY 编码/签名、站点/凭据 rail、Account/Provider/Share 单绑定、同账号 401 预算或 receipt schema v2/revision 2。离线反例不证明厂商真实接受；三条 rail 的私有 receipt 和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Kiro 工具调用完成时必须具备名称

只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：其已提交 `src/kiro/model/events/tool_use.rs::ToolUseEvent` 将 `name` 定义为必填 `String`，而 `input` 与 `stop` 可缺省。目标 Server 已为分片兼容允许后续事件省略名称，因此不把参考的逐帧必填规则直接移入解析器。

目标工具累积器原允许同一 `toolUseId` 的所有分片均缺名称，完成后构造 `name:""` 的下游 `tool_use` 并报告成功；固定 EventStream 反例在旧实现先红。现只在 `stop=true` 完成或 EOF 恢复无参工具时要求该调用已得到非空白名称；首片给出名称、后续片省略仍沿用原名。流式和非流式共用累积器，以固定脱敏 `KIRO_EVENT_STREAM_INVALID` 拒绝无名完成，不改合法工具 JSON 聚合、无参恢复、出站请求、八路 Account/Provider/Share 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线构造不证明厂商真实发送；八份真实 receipt 和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Grok 强制函数选择必须指向保留的工具

只读核对 `Grok/AGENTS.md` 首选、工作树干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`：`responses_tool_choice.go::normalizeToolChoice` 对函数选择查已声明工具身份；次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 `openai_gateway_grok.go::shouldDropGrokToolChoice` 在最终工具过滤后比较函数名。后者六处未提交 Codex 文件未纳入证据。两者更广的别名、账号池或质量重试策略不迁入本 Server。

目标最终 Grok 工具清理原只比较 `tool_choice.type` 与工具类型，`tools=[lookup]`、强制选择 `missing` 的固定 HTTP 反例旧实现先红。现在仅保留匹配已清理工具类型且函数名也精确匹配的对象型选择；普通 `auto|none|required`、已声明函数与 hosted 类型保持既有行为。Responses HTTP、Chat 转换后的 Responses、WebSocket `response.create` 及其 HTTP fallback 共用最终请求准备入口。此为离线协议完整性修复，不推断 xAI 真实接受性，不改 endpoint/header、Account/Provider/Share 单绑定、同账号恢复、Grok quality observer 或 receipt schema。三份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-13 Claude 历史工具调用 ID 的出站可移植性

只读核对 `Claude/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `e9c8769a`：其 Claude executor 只将 `tool_use.id` 与 `tool_result.tool_use_id` 的非空、非 `[A-Za-z0-9_-]` 值映射到可移植 ID，同时保留调用/结果关联并避开合法 ID 冲突。参考另有 Devin 专用可逆编码，不适用本 Server 的 Claude OAuth 请求；这里不推断真实 Anthropic 对所有非法 ID 的具体错误文案。

目标最终 Claude OAuth Messages 合同原只规范工具名称，`call/one` 的固定请求反例在旧实现原样出站。现于最终 CCH 签名前，以域分离 SHA-256 的 URL-safe 短别名修复非可移植历史工具 ID；同一原 ID 请求内复用，预先占用全部合法 ID，冲突时选择未使用别名。合法 ID、其他字段、空值及非字符串值不因本项改变，重跑规范化幂等；count_tokens 走同一消息历史入口。只修本地出站身份，不改 endpoint/header/beta、工具名称映射、Account/Provider/Share 单绑定、CL-N7/CL-N8、模型目录或 receipt schema v1/revision 1。五份真实 operation receipt、thread/beta/工具 ID 厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Antigravity 末尾媒体工具结果的 Gemini turn 边界

只读复核 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `15d2148b`：其 HTTP 回环证明 Gemini 模型的最后一轮为 `model` 角色、`functionResponse.parts` 含 `inlineData` 时，上游返回 400，补一个空 `user` turn 后可继续；文本工具结果与 `user` 角色媒体结果保持原样。另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的思考预算与会话策略不能替代这条请求形状证据。CLIProxyAPI 新增 `9126772d` 的 dependent schema 关键词在目标 Antigravity 的现有 allowlist 清洗中已被移除，不重复改动。

目标最终 wire 原把这种原生 Gemini `model` 角色媒体工具结果留在末尾；固定封装反例旧实现先红。现在仅在外层最终模型明确为 Gemini、末尾 `model`/`assistant` 工具结果的嵌套 parts 出现 `inlineData`/`fileData`（含 snake_case）时追加一次空 `user` turn；重复规范化幂等，原媒体及工具关联不改。Antigravity/AGY 两条绑定 rail 的 Gemini 适配器最终封装回归均覆盖，普通文本结果、既有 user 结果、Claude 最终模型和无 envelope 的 raw body 不变。此为本地 wire 修复，不推断两条真实 OAuth rail 已接受，也不改 endpoint/header、AG-N6 requestType、签名 replay、compaction disabled、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v2/revision 2。真实 receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 CodeBuddy 独立 usage 尾帧可省略 choices

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go::Aggregate`：其 JSON 结构的 `Choices` 可缺省，仍保留独立 `Usage`；未跟踪 `proxy.html`、`proxy.md` 排除。参考在首个 `[DONE]` 即停止扫描，比本 Server 的唯一终态 + EOF 合同宽松，此处不迁入该行为，也不推断厂商真实发过省略形状。

目标流式 decoder 已接受有对象 `usage`、无 `choices` 的尾帧；非流聚合器原要求每帧含 `choices`，使同一有效尾帧在两条下游路径结果分歧。固定 decoder→aggregator 反例在旧实现先以 502 失败。现非流聚合器仅在 `usage` 已通过对象校验时容许缺省 `choices`；显式非数组 choice、无 usage 的缺省 choice、畸形 usage、终态与 EOF 规则保持。Intl/CN × Claude/Codex/Gemini × stream/non-stream 本地回环覆盖 cache read/write 与一次生成、零 refresh。此项只修入站尾帧兼容，不改出站 wire、Account/Provider/Share 单绑定、同账号恢复、receipt schema v2/revision 2 或 CB-N4 disabled。双站真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Qoder Chat choice 与终止原因的本地失败关闭

`Qoder/AGENTS.md` 推荐 TokenRouter，但该仓 `AGENTS.md` 要求当前会话先调用不可用的 `project-doc` 技能。本轮没有读取其业务源码或运行外仓源码审计；最新参考增量仍为 `unreviewed_skill_gate`。这里只审目标 `QoderSseDecoder`、`QoderChatSseAggregator` 与既有冻结 CLI oracle，不据此推断新 COSY wire。

目标原先对 `choices[].index` 解析失败默认 0，并在流式规范化前不检查 `finish_reason` 的类型与空白值。非法 choice 可能被改写为 index 0，伪终止值可能先到下游；直接非流聚合虽拒绝布尔终止值和非零 index，却同样会把不可解析的 index 默认为 0。固定布尔/空白终止原因、非零/不可解析 index 反例先红。现两层共用 index 校验，流式 decoder 还在转发前拒绝非 null、非有效字符串的终止原因；缺失 index 默认 0、合法非空字符串和原有同帧业务+终态继续可用。唯一终态+EOF、请求 wire、Global/CN 站点与 OAuth/PAT rail、Account/Provider/Share 绑定和 receipt schema v2/revision 2 均不变。三份真实 rail receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审与当前未提交工作树 committed-target observation 仍缺；离线反例不等于厂商真实接受性。

## 2026-10-10 CodeBuddy 混合业务错误帧优先级

只读核对 `CodeBuddy/AGENTS.md` 推荐的干净 `cli2api@a6502521e1e509a4257153025c642240c39cd54b`：其 WorkBuddy `client.go::Classify` 将非零业务码用于错误分类，而 `sse.go` 是宽松复制/聚合器，不定义目标 Server 的多租户成功门槛；两个未跟踪文档排除。本项不据参考推断新的厂商 SSE 格式。

目标 `CodeBuddyUpstreamError::from_event` 原在任意 `choices` 字段存在时跳过已识别的业务错误码。固定 `14003` 帧同时携带空 choices 时，首个 Intl 非流回环先得到普通 502；携带完整 content/finish choice 再接 `[DONE]` 时，旧非流路径先错误返回 200。现业务码优先于 choices，decoder 保留 429 限流分类，直接非流聚合入口同样拒绝混合错误；修复后的 Intl/CN × 流/非流回环不刷新账号。`code=0` 正常帧、请求 endpoint/header/body、站点/Account/Provider/Share 绑定、独立 usage 尾帧、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 与 CB-N4 disabled 决策不变。离线构造不证明真实厂商会发混合帧；两站真实 receipt、CN 新模型厂商证据及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 CodeBuddy 工具调用显式 index 的身份边界

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/sse.go::mergeToolCalls`：参考按工具 index 聚合，但其宽松解析不定义目标 Server 的坏索引成功门槛；两个未跟踪文档排除。目标原先把 `tool_calls[].index` 显式 `null`、非法字符串或负数当成缺省，流式 decoder 也未校验工具 index。固定 `index: null` 反例在旧实现先红。现 decoder 与非流聚合器共用显式索引校验，要求非负且小于 128；缺失仍按现有 ID/位置回退，合法数字字符串继续可用。Intl/CN × Claude/Codex/Gemini × 流/非流仅作本地回环，不据此推断新厂商 wire 或真实接受性。出站请求、单账号绑定、唯一终态+EOF、receipt schema v2/revision 2 与 CB-N4 disabled 决策不变；两站真实 receipt 和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Antigravity replay 缓冲只限制未完成事件

只读复核 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：`antigravity_reasoning_replay.go::ObserveSSELine` 逐行观察 replay 载荷，不能把单个网络 read 的大小当作事件大小。另一推荐项目 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 仍是已有协议差分参考，但本项不从它推导目标 Server 的 SSE 缓冲容量；不据参考推断新 Google wire。

目标 `AntigravityReplayStreamAccumulator::push` 原在解析前比较“旧残留+整个网络块”与 16 MiB 限额：两条各约 8 MiB、已经完整的注释事件加一个签名终态若同批抵达，合法流会被误判溢出并丢失可用 replay。固定反例先红。现分段消化输入，逐条完整事件及时释放缓冲，只让未完成事件占用 16 MiB 上限；超限的不完整事件仍丢弃候选，终态后事件仍不进入 replay。必要时释放大突发留下的空容量，避免旁路长期占用请求内存。此项不改主数据面成功/失败、上游 Antigravity/AGY 请求、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction 或 receipt schema。大块本地注释 fixture 不证明真实 Google 接受性；两条 OAuth rail receipt、AG-N6/AG-D3 专项证据及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Antigravity replay 同 ID 工具语义不得漂移

只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `antigravity_reasoning_replay_test.go::TestPrepareAntigravityGeminiReasoningReplayRejectsReusedIDWithChangedCall` 与对应 `functionCallPartLocationForReplayWithSchemas`：参考先核对同 ID 工具调用的名称/参数，若发生漂移，不再把旧签名转贴到相邻语义相同的调用。另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的思考签名存取仍仅作上下文差分，不定义目标匹配算法。本项不推断新 Google wire。

目标 `patch_model_content` 原按缓存工具 ID 命中后不复核 name/args；固定“旧 ID 改参数 + 邻近相同旧语义调用”和“旧 ID 改名”反例先红。现同 ID 候选必须唯一且名称、规范化参数均与缓存一致，否则整轮不注入签名；缺省 ID 与原有语义/出现序号匹配继续使用。Antigravity OAuth 与 AGY OAuth 的 Gemini 本地 HTTP 回环验证：改动后的参数原样出站，旧签名没有附着任一调用，仅向当前绑定账号发送一次续接请求。此为 AG-D3 本地 replay 身份隔离，不改变 endpoint/header/body 构造、Account/Provider/Share scope、AG-N6 requestType、终态提交、compaction disabled 或 receipt schema；两条真实 rail receipt、AG-N6/AG-D3 专项接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-11 Antigravity replay 工具结果的 ID 与名称须同属旧调用

只读复核 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `antigravity_reasoning_replay.go::functionResponsesCanRestoreID`：其按 ID 关联工具结果时仍核对非空名称与原调用一致。`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 只作为已有 Gemini 签名差分背景，不定义本 Server 的 replay 身份算法。参考的 `unknown` 名称兼容规则未移入目标。

目标 `response_matches_calls` 原在结果 ID 命中后忽略名称，且非空但不匹配的 ID 可按同名调用回退；固定“同 ID、不同名”反例先红，另补“不同 ID、同名”回归。现在显式非空 ID 只按同一 ID 查找，非空结果名称也必须等于缓存调用名；缺失 ID 的同名匹配保持。Antigravity OAuth 与 AGY OAuth 的 Gemini 本地 HTTP 回环确认旧签名不注入改名结果，只向原绑定账号转发一次。仅修 AG-D3 本地 replay 关联，不改上游 wire、Account/Provider/Share scope、AG-N6、compaction disabled 或 receipt schema。离线反例不证明 Google 实际发送该形状；两条真实 receipt、AG-N6/AG-D3 厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-11 Claude SSE 单事件容量边界

只读核对 `Claude/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `claude_executor_stream.go::validateClaudeStreamingResponse`：参考按 SSE `data:` 行验证基本 JSON 和 `message_start` / `message_delta`，其扫描容量不定义本 Server 的单事件 8 MiB 合同。本项仅修目标自身 Anthropic SSE 检查器，不据参考推断新厂商 wire。

目标 `AnthropicSseInspector::push_inner` 原在拆分前只要整个网络块存在任一完整事件边界，就跳过超限检查。`message_start` 后同块附超过 8 MiB 的未完成事件时，固定反例先错误返回 `Lifecycle`；现排空完整事件后检查剩余半帧长度，立即以本地协议错误拒绝。两条各自小于上限、合计超过 8 MiB 的完整事件正例保持可用。仅收紧本地逐事件容量与失败时机，不改变 Claude OAuth 出站 Messages/count_tokens、Account/Provider/Share 绑定、CL-N7/CL-N8、429 scope、receipt schema v1/revision 1 或已关闭能力。离线异常块不证明 Anthropic 实际发送该分块；五份 operation 真实 receipt、thread/beta 厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Claude 终止原因的本地形状与后续内容栅栏

只读复核 `Claude/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `claude_executor_stream.go::validateClaudeStreamingResponse`：参考要求消息起点及进度事件存在，其宽松扫描不定义目标 Server 对终止原因类型或后续内容的成功门槛。本项依据目标自身 `AnthropicSseInspector` 与 Messages JSON 合同收紧，不推断 Anthropic 新 wire。

旧检查器把布尔、数字或空白 `message_delta.delta.stop_reason` 当作合法进度，显式 `end_turn` 后仍允许新的内容块，最后 `message_stop` 可成功；固定类型及晚到内容反例先红。现 Messages JSON 与 SSE 共用 `stop_reason`/`stop_sequence` 类型校验；SSE 非空终止原因出现后，内容块及冲突的第二原因失败关闭，缺省/`null` 的 usage-only delta 仍可继续内容。Claude OAuth HTTP 回环证实晚到内容在成功提交前返回 502，不产生 completed 用量。出站 Messages/count_tokens、Account/Provider/Share 绑定、CL-N7/CL-N8、静态目录、receipt schema 均不变；五项真实 operation receipt、thread/beta 接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-08 Antigravity catalog publication fence and partial replay differential

本轮只读对照 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`（tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525`）的 `sdk/cliproxy/antigravity_models.go::fetchAntigravityModelCapabilityHintsForAuth` 与 `antigravity_models_fencing_test.go::TestAntigravityProbeFencesOlderTokenResult`；参考通过 account/route key 和 probe revision 阻止旧 token 探测晚返回覆盖新目录，且 `models:{}` 是权威空目录。目标起点是 `cc-switch-server@ac69ecb1c652c0ac924394203617df3b57fa2498`（tree `af3aba76cb72a21f61c1c4e3e8aa0e76df5408da`）。目标既有缓存键缺 token generation，成功响应会按账号清其它代际，能力证据也只复核 auth generation；本轮工作树在 `src/clients/oauth/antigravity_models.rs`、`src/api/providers.rs`、`src/state.rs` 独立加入精确 token/项目/URL 缓存作用域、请求发布序号、写前账号 token/项目复核和持久化证据 CAS。空目录会清除旧逐模型 search 能力记录；stale 仍不写能力证据。A 旧 token 挂起、B 新 token 空目录先发布、A 晚返回的 loopback 测试、旧代际失败不得删除新 key、旧项目不得写新账号证据及“新凭据代际优先于旧调用序号”的 fixture 均为离线合同，不代表真实 Google 目录验收；无账号池或跨绑定 fallback。

AG-D3 只读差分另参照 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`（tree `9211a067165c376775c98ecf3d522b5562380ffa`）的 `src-tauri/src/proxy/thinking_store.rs::is_wrapped_claude_payload` / `ensure_raw_claude_thought_signature`，以及 CLIProxyAPI 同 HEAD 的 `internal/runtime/executor/antigravity_reasoning_replay_test.go` context fingerprint/reused ID 用例。目标本地 fixture 证明双层 CAQS 作为 opaque 字节随签名 replay 保留，多图顺序改变导致历史 hash 不匹配、并行 tool/result 必须完整、未见 terminal 的断连不写 replay；既有 CAQS v4 round-trip 与碎片 SSE terminal 测试同属离线证据。Manager 的双层签名判别与预算/模型后缀策略未直接迁入：目标 REST/Claude 作用域及真实验收尚不足以把其转换规则当作 Google wire 权威。AG-D3 的思考预算/模型后缀和 terminal 后下游断连仍需独立差分与私有 receipt，故这里只记 `partial_fixture_verified_uncommitted`，不宣称 AG-D3 完成。未进行 UI 自动化或部署测试。

本轮局部 Antigravity 关键词 74/74，全量 Rust library 3274 passed / 1 ignored，`cargo check --lib`、rustfmt、Clippy 与 Provider contract/coverage/UI matrix 均通过；Phase 0 清单 6/6 和 Antigravity 已有 reference-delta 的 committed-source 审计通过。`src/api/providers.rs`、`src/state.rs` 的完整性锚点已在两个 Provider Phase 0 清单中按当前工作树源码更新。新增 AG-N12/AG-D3 尚无提交态 implementation tree，不能伪写进要求已提交实现对象的 append-only reference-delta v2 observation；真实 `antigravity_oauth` / `agy_oauth` receipt 均仍 `live_pending`。

### Antigravity 同 ID 生命周期与固定思考预算接续

2026-10-09，继续按 Antigravity 单类别核对冻结的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`；CLIProxyAPI 的 replay accumulator 以 SSE 行载荷观察响应，目标 Server 则需自行切分事件。目标 `next_sse_event` 原先先搜索所有 `\r\n\r\n`，仅在完全找不到时搜索 `\n\n`：同一缓冲中先到 LF 事件、后到 CRLF 终态时，把两条 `data:` 合并，JSON 解析失败，已完成的签名轮次不会进入 replay。混合分隔符回归在旧实现先因未识别终态失败；现在选择字节位置最早的完整分隔符，保留原有碎片与纯单一换行路径。此项只修本地 SSE replay 捕获，不推导新的 Google wire；真实双 rail receipt、AG-N6/requestType、compaction 和当前未提交工作树的 committed-target observation 均不因此升级。

AG-N12 的剩余同 ID 反例已落到账号存储：`AccountStore` 为活跃账号分配进程内单调的 incarnation，正常 upsert/刷新保持，删除后同 ID、同 token、同 auth/token generation 的重建取得新值；该值不序列化到账号文件或 API，重启时重新分配，因为旧进程的在途调用不能跨重启继续。目录缓存 key、API 调用前后快照和能力证据写入在同一 incarnation 上复核；旧实例即使晚返回且请求序号较大，也不能清新实例缓存或写新账号证据。`AccountStore` 生命周期 fixture、缓存倒序 fixture 和 API/state 相同 ID/凭据/代际的重建 fixture 均为本地证据，真实账号删除/重绑仍需单独 receipt。

AG-D3 的 `maxOutputTokens=1024`、固定 `thinkingBudget=2048` 在原 Antigravity 最终 wire 边界先出现红灯：目标会原样发出参考 Manager 明确视作无效的组合。现在仅对 Antigravity 的固定非负整数预算、且显式给出输出上限的请求提前返回 400；不悄悄扩大调用者的 `maxOutputTokens`，自适应 `-1`、`thinkingLevel` 和缺上限的请求不据此推断。显式模型档位 `gemini-3.5-flash-medium` 与预算保持原值；CLIProxyAPI 的 `model(high)` 后缀优先于 body 是其通用路由策略，目标 Registry 的 Antigravity `single/passthrough` 绑定不在无真实目录和 receipt 时把它解释为另一个上游模型，记为 `not_adopted_pending_live`。已有 `antigravity_stream_commits_replay_at_terminal_before_upstream_eof` 实测 terminal 后下游断连仍可在同作用域续接，故该项离线绿灯无需再改生产流。双层 CAQS 的不同 REST/Claude 载体也不据参考代码猜造或重编码签名。AG-D3 的离线差分记录至此覆盖计划列举的形状，但两条真实 rail 仍 `live_pending`，当前工作树未提交，不能写 committed-target observation。

Antigravity 本地收尾：完整 `cargo test` 为 library 3276 passed / 1 ignored，加上非 library 124 + 1 + 1 passed；Clippy `--all-targets -D warnings`、rustfmt、差分空白检查、Phase 0 / Provider contract / coverage / UI matrix、文档索引和 Antigravity 既有 reference-delta `--check-sources` 全绿。此结果仅说明当前工作树离线回归通过；AG-N12/AG-D3 尚无 implementation commit/tree，`antigravity_oauth` 与 `agy_oauth` 的真实 receipt 也均缺失，不能提升为提交态 observation 或 live verified。

2026-10-09 的目标 Server 内部复核发现 AG-N12 仍有同代次凭据边界：`fetchAvailableModels` 每次发网，但 transient 失败可按缓存键回退，而原键没有实际 Bearer 身份。保持 Account ID/两代际/项目/URL 不变，将 Bearer 从 A 改为 B 后模拟 503，旧流程先错误返回 A 的目录；现以域分离 SHA-256 Bearer 摘要隔离缓存和 stale fallback。发布序号只仲裁同一精确缓存键，不让旧 Bearer 的晚返回删去新 Bearer 的目录；最多保留 128 份目录，按获取时间与请求序号淘汰旧项，淘汰只会使 stale 不可用。API 返回及能力证据持久化前另复核当前账号的同一请求身份。此项不来自新的 Google wire 推断，不更改成功空目录、24 小时 stale 上限或 AG-D3/requestType/compaction 决策；回环和直接状态反例不是两条 OAuth rail 的真实接受性。推荐参考的冻结 committed objects 仍只作只读交叉证据；当前工作树尚无新的 committed-target observation。

2026-10-09 继续检查 AG-N12 的成功请求并发仲裁：旧请求晚于同一精确缓存键的新请求返回时，`publish_catalog` 虽拒绝旧结果写缓存，调用方仍将旧目录标成 `stale=false` 返回；倒序回环先复现此反例。现在拒绝发布后仅在精确键仍有较新目录时返回该目录及其原始 revision/抓取时间；若较新账号代际已取代旧键，则非重试 409 失败关闭，控制层保留这一冲突语义而不转成 502。两种结果均不把旧成功响应当成当前 entitlement。只读交叉核对 CLIProxyAPI 和 Antigravity-Manager 的冻结提交态；Manager 以原始模型键保留目录，本项没有据此推断新 Google wire。成功空目录、同代次不同 Bearer 并行缓存、24 小时 stale 策略、AG-N6、AG-D3 与 compaction disabled 状态不变；回环不是两条真实 rail receipt，仍为 `live_pending`。

此处本地身份冲突另有类型标记；上游原生 HTTP 409 不会被误投影为本地代际冲突。最终代码完整 Rust library 3319 passed / 1 ignored，其他目标 124 + 1 + 1 passed；全量 Node audit 1896/1896、Antigravity 专项 165/165、smoke Node 13/13、本地服务回环、Clippy、rustfmt、Provider/Phase 0/coverage/UI matrix、文档索引与参考提交态差分检查通过。控制层源码变化后的 Phase 0 writer 摘要由生成器刷新。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；未获得 Google 真实 receipt，也未做 UI 自动化或部署测试。

### Antigravity 私有 receipt v2 验收范围（2026-10-08）

本轮只加固本仓库 `scripts/smoke/antigravity-real.mjs` 的证据门槛，不改生产反代 wire，也不从参考项目推导新的 Google 接受性。旧 `realAcceptance` schema/revision 1 是冻结历史合同，保持原文不变；`assets/contract/antigravity-reference-delta.json` 追加 `AG-LIVE-V2-2026-10-08` 扩展，当前验收只接受 receipt schema 2 / harness revision 2。两条 OAuth rail 仍各自 `receipt=null/live_pending`，compaction 不自动启用。

静态审查与离线反例发现：旧校验器只判断目录时间戳为正数、receipt 日期可解析，且读取目录和 receipt 后不再复核 Account/Provider/Share；旧 v1 scope digest 的目录快照不包含获取时间和完整内容。两条 rail 上，过旧/超前目录与 receipt 的 8 个反例、Account token generation / Provider revision / Share revision 漂移的 6 个反例先复现错误接受，随后以目录最多 2 小时、receipt 最多 24 小时、未来时钟偏差最多 5 分钟及接受前末次绑定复核修复。v2 再将每份目录的 `fetchedAtMs` 与按完整 `data` 计算的 domain-separated SHA-256 放进 scope digest 和 receipt 快照；模型 ID 不变但目录新增项、或 receipt 快照 digest 被篡改的两 rail 共 4 个负例均被拒绝。正常 fixture 和上述负例合计 Node 28/28。失败只输出脱敏摘要。末次复核是多次控制面读取后的快照栅栏，不声称跨 API 原子事务或目录在其后永不变化。

Antigravity reference-delta `--check-sources`、Provider Phase 0/coverage/UI matrix、文档索引、脚本语法与差分空白检查均通过。`RUN_REAL=0` 不发送真实请求，只返回 `blocked_inputs/live_pending`；未获得两条 rail 的私有真实 receipt，AG-N6/requestType、compaction 与本次新验收逻辑的 committed-target observation 仍待独立证据。没有 UI 自动化或部署测试。

### Antigravity 私有 receipt 真实路径复核（2026-10-08）

本项是目标验收安全边界，不从参考项目推断新的 Google wire。`scripts/smoke/antigravity-real.mjs` 原只按字面路径要求私有 receipt 在仓库外；仓库外符号链接指向仓库内的合法 fixture receipt 时，旧脚本仍给出 `contract_verified`，单项先红灯。现在对实际存在的路径执行 `realpath`，再次拒绝仓库内目标，并从解析后的真实路径读取。链接绕过在任何控制面/目录请求前被拒；两条 OAuth rail 的普通外部 receipt fixture 均保持通过。Antigravity Node 29/29，全量 audit Node 330/330、smoke Node 13/13；已核对 `docs/provider/coverage.md`，此私有路径约束不改变生成的 Provider 覆盖矩阵。

Antigravity 已冻结 reference-delta `--check-sources`、Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法与差分空白检查均通过。生产 wire、AG-N6 与 compaction 决策、schema v2/revision 2 不变；两条 rail 的 `RUN_REAL=0` 仍为 `blocked_inputs/live_pending`，没有真实账号 receipt。本次验收器改动尚未提交，不追加伪造的 committed-target observation；没有 UI 自动化或部署测试。

### Antigravity 双 rail 双目录的签名绑定核对（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的提交态不变；本项没有从外仓引入新的 Google wire。目标 Server 的 `/v1/models` 实际按 Router 签名 Share 与 App 选择 Provider，普通 `providerId` 查询不参与选择。`antigravity_oauth`、`agy_oauth` 私有 receipt 验收此前只以 `app/providerId` 请求 Claude/Gemini 目录，若 token 指向另一 Share 且模型 ID 相同，目录结果不能证明 receipt 声称的绑定。两条 rail 的正常回环 fixture 加入每 App 的 `expectedShareId/expectedProviderId` 断言后均先红；现验收器传入精确预期，由已有签名 ingress 端到端测试覆盖的 Server 栅栏在目录处理前拒绝错 Share/Provider。每条 rail 另外要求 Claude/Gemini 各请求一次；Antigravity 专项 Node 46/46。

此项只收紧私有目录证据，不改 Antigravity/AGY 生产 wire、AG-N6 requestType、AG-D3/compaction 决策或 receipt schema v2/revision 2。目录路由绑定不等于真实 Google entitlement 或跨 API 原子快照，两条真实 receipt 继续缺失。新增 JS/文档仍在未提交工作树，不能用旧 committed-target observation 冒充本轮证据；未做 UI 自动化或部署测试。

2026-10-08 的 Antigravity/AGY 私有证据时效复核只修本仓库验收器，不新增 Google wire 观察。两条 rail 各让 Claude 或 Gemini 目录在取得时距两小时上限 1.2 秒，或让 receipt `recordedAt` 距 24 小时上限 1.2 秒，再延迟末次 Share 响应 2 秒；旧流程六例均在证据已过期后错误接受。现在初次检查与末次绑定复核后共用目录和 receipt 时间谓词，以同一接受时刻检查双目录及 receipt；六例均失败关闭。生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 和 receipt schema v2/revision 2 不变。末次快照不是跨 API 原子事务，两条真实 receipt 仍各自 `live_pending`。

### Antigravity 双 rail 私有 receipt 控制面响应大小栅栏（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的提交态不变；本项是目标验收器自身的资源边界，不从参考推导新 Google wire。旧 `antigravity-real.mjs` 在完整读取控制面响应后才检查 4 MiB；`antigravity_oauth` Account 的声明长度超限、`agy_oauth` Provider 的无长度分块实际超限且连接不结束，两条 loopback 反例均先红灯（约 5 秒超时）。现先查 `Content-Length`，读取时逐块累计实际字节并在超限时中止，两例均快速、脱敏地失败关闭。Antigravity 专项 Node 32/32、全量 audit Node 354/354、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策和 receipt schema v2/revision 2 均不变。两条 rail 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；当前 JS 加固未提交，不追加伪造的 committed-target observation。未做 UI 自动化或部署测试。

### Antigravity 双 rail 私有 receipt 静态硬链接边界（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交状态不变。本项只修本仓库私有验收器的文件位置约束：`realpath` 已拒绝仓库外符号链接指向仓库内，但不能识别仓库内外路径共用 inode 的硬链接。`antigravity_oauth`、`agy_oauth` 两个合法 fixture 各自先复现错误接受；现于任何控制面请求前要求已解析 receipt `nlink=1`，两例均以固定脱敏错误失败关闭。双 rail 正常 fixture、目录/绑定漂移与控制面响应大小用例保持通过；Antigravity 专项 Node 35/35、全量 audit Node 384/384、smoke Node 13/13，reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

静态硬链接检查不提供文件并发替换的原子防护，也不能证明部署代码与本地 HEAD 一致。生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 决策及 receipt schema v2/revision 2 未变。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；本工作树 JS 加固未提交，不追加假的 committed-target observation。未做 UI 自动化或部署测试。

### Antigravity 双 rail 私有 receipt 请求前限量快照（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交状态未变。本项只修目标验收器自己的证据读取窗口。旧流程虽检查 receipt 的仓库外真实路径、权限与单链接，却在 Account/Provider/Share 和目录请求后才按路径读取，且没有 receipt 字节上限。`antigravity_oauth`、`agy_oauth` 各自在首个 Account 请求将无效文件原子替换为有效文件，旧流程均错误写出 `contract_verified/live_pending`；两份超过 1 MiB 的文件也先触发控制面请求。现请求前从只读描述符获取最多 1 MiB 的快照，核对文件类型、dev/inode、链接数、大小和权限；后续只解析快照。四个反例均在对应边界失败关闭，正常双 rail、静态硬链接、目录内容/时效、绑定漂移与响应大小用例保持通过，Antigravity 专项 Node 41/41。

这不是新的 Google/AGY wire 结论；生产转发、AG-N6 requestType、AG-D3/compaction 决策及 receipt schema v2/revision 2 不变。快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 425/425、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求或 receipt；AG-N6 与 compaction 仍缺独立真实证据。新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据；未做 UI 自动化或部署测试。

2026-10-08 的 Antigravity/AGY 私有 receipt 快照补强只处理读取期间的普通原位改写。两条 rail 各在首次 `readSync` 返回有效字节后、EOF 检查前，对同 inode 文件等长覆盖并改变 mtime；旧验收器两例都继续用旧字节输出 `contract_verified/live_pending`。现在同一只读描述符在读前后核对 dev/inode、nlink、size、mode、纳秒 mtime/ctime，变化即在任何控制面请求前脱敏失败。此项不改变生产 Google/AGY wire、AG-N6 requestType、AG-D3/compaction 或 receipt schema v2/revision 2；元数据复原与并发精确竞态不由此证明，两条真实 rail 仍分别 `live_pending`。

2026-10-08 的 Antigravity/AGY 双 rail receipt 最终文件身份复核只修本仓库私有验收器，不新增参考 Google wire。两条 rail 各在首次 Account 控制面读取时把先前已快照的有效 receipt 换成另一个 inode 的无效文件，或将原 inode 等长覆盖并改变 mtime；旧流程四例仍用旧内存内容错误输出 `contract_verified/live_pending`。现保留读后指纹，在末次 Account/Provider/Share、目录与 receipt 时效复核后重新解析配置路径，比较 dev/inode、nlink、size、mode、纳秒 mtime/ctime；四例均脱敏失败，正常双 rail fixture 保持通过。此复核不是跨 API/文件系统原子事务，不能证明恶意元数据复原或最终检查后的稳定性，也不构成真实 rail 接受性。生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction 与 receipt schema v2/revision 2 不变；两份真实 receipt 仍缺，新增 JS 未提交不能冒充 committed-target observation，未做 UI 自动化或部署测试。

2026-10-09 的 Antigravity/AGY Share 投影复核仍只修本仓库双 rail 私有 validator，不新增 Google wire。生产 `Share` 的 `bindings` 已包含 `share.app` 主条目，导入不变量要求每 App 至多一条且主字段一致；旧验收器却拼接主字段后用 `some` 找匹配，并不检查 Share 是否启用。两条 rail 各构造 Claude/Gemini 重复冲突绑定、主字段冲突、disabled 与 paused 五种反例，旧流程十例均错误输出 `contract_verified/live_pending`。现直接要求原始 `bindings` 对所选两 App 各恰好一条精确匹配、主字段与其唯一条目一致，并要求 `enabled=true/status=active`；十例先红后绿，正常双 rail 按实际 Share 形状通过，错误均在目录发网前拒绝。Antigravity 专项 Node 75/75、全量 audit Node 749/749、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。runbook 同步纠正先读 receipt 快照、再查控制面的实际顺序。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项接受性与当前工作树提交态 observation 仍缺。生产 wire、receipt schema v2/revision 2、compaction disabled 决策不变，控制面快照不是跨 API 原子事务；未做 UI 自动化或部署测试。

2026-10-08 的双 rail receipt 最终内容复核继续只收紧本仓库私有验收器，不新增 Google wire 事实。`antigravity_oauth`、`agy_oauth` 各在首个 Account 请求时把已快照的文件等长改写，并在对抗 fixture 中把末次元数据读数伪装成原指纹；旧流程两例仍错误接受。现在末次用 `O_NOFOLLOW` 只读描述符核对完整原始字节、EOF、读前读后指纹及配置路径指向，两例均脱敏拒绝。fixture 伪装元数据只证明元数据比较本身不能代替内容比较，不意味着普通用户能恢复真实 `ctime`；末次检查后的竞态、跨 API 原子性、真实厂商接受性仍未获证明。生产协议、AG-N6/AG-D3/compaction 与 receipt schema v2/revision 2 保持不变，两条真实 receipt 仍为 `live_pending`。

2026-10-09 的 Antigravity/AGY 控制面 revision 作用域复核只修本仓库双 rail 私有 validator，不新增 CLIProxyAPI、Antigravity-Manager 或 Google wire 事实。旧验收器把缺失的 Provider/Share revision 默认成 `0`，并接受空 runtime fingerprint；只要私有 receipt 也按不完整作用域生成，两条 rail 各有六类反例会错误输出 `contract_verified/live_pending`：缺 Claude revision、负 Gemini revision、空 Claude/Gemini fingerprint、缺 Share revision、负 Share revision。现在要求 Provider `providerRevision` 和 Share `configRevision` 均为明确存在的非负安全整数，两个 runtime fingerprint 均为非空字符串；十二例先红后绿且在目录请求前拒绝，正常双 rail 与既有文件、Share、目录、时效和漂移 fixture 保持通过。Antigravity 专项 Node 88/88、全量 audit Node 901/901、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项接受性和当前工作树提交态 observation 仍缺。生产 Antigravity/AGY wire、Share 路由、receipt schema v2/revision 2 与 compaction disabled 决策不变；本地控制面读取不是跨 API 原子事务，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

2026-10-09 的 Antigravity/AGY 控制面真实投影复核继续只修本仓库私有验收器。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交状态未变，不推导新 Google wire。目标 Server `ProviderView` 为顶层 `revision` 与 runtime `providerRevision`，但旧 Antigravity mock 使用不存在的顶层 `providerRevision`；mock 改为真实形状后，两条正常 rail 先全部失败。旧 validator 虽已检查 revision 安全整数和 fingerprint 非空白，仍只读取虚构字段。两 rail × Claude/Gemini 各构造仅有旧式顶层字段、缺顶层 revision、缺 runtime revision、两份不一致四类反例，并在非目标 App 保留旧字段使 receipt 作用域自洽；旧流程十六例均错误输出 `contract_verified/live_pending`。现要求两个 Provider 各自真实顶层/runtime revision 均为非负安全整数且相等，正常双 rail 恢复通过，十六例在目录发网前脱敏拒绝；既有缺字段、负数、空 fingerprint 与 Share revision 反例保持通过。Antigravity 专项 Node 105/105、全量 audit Node 1156/1156、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。双 rail `RUN_REAL=0` 仍各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项证据与当前工作树提交态 observation 仍缺。生产 wire、Share 路由、receipt schema v2/revision 2 和 compaction disabled 决策不变；控制面复核不是原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

### Antigravity 双 rail OAuth refresh 凭据验收门槛（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `internal/auth/antigravity/auth.go::TokenResponse` 区分 access 与 refresh token；参考仅作只读交叉证据。目标真实 receipt 的 `same_account_first_401_recovery` 检查要求原账号具备恢复凭据，但旧验收器只检查 `hasAccessToken`。两条 ProviderType rail 各自的“初始缺 refresh token”和“末次 Account 快照失去 refresh token”四个 loopback 反例先全部错误接受。现在两次 `validateAccount` 都要求 Server 脱敏投影 `hasRefreshToken===true`；初始缺失在查询模型目录前拒绝，末次缺失在最终接受前拒绝，正常双 rail fixture 不变。

这只证明当前 Account 的 refresh 凭据存在，不证明其可用性或真实 401 replay，也不改变生产 wire、AG-N6/AG-D3/compaction 决策及 receipt schema v2/revision 2。Antigravity receipt Node 46/46、全量 audit Node 463/463、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法与差分空白检查通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求或 receipt。JS 增量未提交，不追加虚假的 committed-target observation；未做 UI 自动化或部署测试。

### Antigravity/AGY OAuth Account 布尔投影完整性复核（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交状态不变。本项依据目标 Server 自身 `AccountPublicView` 合同收紧双 rail 私有 receipt validator，不推导新的 Google wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`；旧 validator 只拒绝值为 `true`。`antigravity_oauth`、`agy_oauth` 各将两个字段分别设为缺失、`null`、字符串 `"false"`，十二个 loopback 反例旧流程均错误输出 `contract_verified/live_pending`。现在要求两字段都明确为布尔 `false`，十二例均在 Provider 与双目录请求前脱敏拒绝，正常双 rail fixture 保持通过。

这只提高私有验收证据完整性，不改变生产 Antigravity/AGY wire、Share 路由、AG-N6 requestType、AG-D3/compaction 或 receipt schema v2/revision 2；控制面读取不是跨 API 原子快照，离线 fixture 不证明 Google 接受性。Antigravity 专项 Node 118/118、全量 audit Node 1293/1293、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，仍缺两份真实 receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树的 committed-target observation；未做 UI 自动化或部署测试。

## 2026-10-08 Cross-provider implementation review

第三轮跨类别复核使用当前 `cc-switch-server` 工作树，不将未提交实现称为 committed evidence。全量 Rust library 为 3269 passed / 1 ignored；本地 audit/smoke Node 单测为 163/163。七类可合规复核的 reference-delta `--check-sources` 以及 Qoder 自包含默认 audit、Provider coverage/UI matrix 和文档索引均通过；Qoder 最新 TokenRouter 仍禁止业务源码复核，不能因默认 audit 通过而提升 `unreviewed_skill_gate`。没有进行 UI 自动化、部署或真实账号验收。

Review 发现 Claude thread alias 缓存原称 `MAX_BYTES=8 MiB`，但只核算 alias 文本，没有计入 BTreeMap/FIFO 两份作用域键和消息 ID。精确超长 session fixture 先红后绿；现在单项 64 KiB 与全局 8 MiB 均按键、Entry 和 alias 文本/结构容量计算，过大作用域不写入状态，不扩大身份或恢复权限。`src/state.rs` 新增缓存域方法也令 Phase 0 Provider writer inventory 的该文件 SHA-256 失效；核对 Provider writer 实现未改变后，将 `assets/contract/provider-writer-inventory.json` 的当前源码摘要更新为 `21ce629989161eb8d75c1326b05f95ca86c10db23d0211ba22119923c639ed26`，Phase 0 Node 测试随后 6/6 通过。该清单只是当前文件完整性锚点，不是 Claude 新行为的真实 receipt。

整体目标仍未达到完成条件：计划中的 AG-N12 并发目录代际/CAS 与 AG-D3 差分尚无本轮实现证据；KI-N5 只有 shadow、未切生产计量；Qoder 最新源码受 `project-doc` 门禁；各类需要真实账号的 operation/rail/site receipt 仍各自 `live_pending`。后续应先回到 Antigravity 单类别完成 AG-N12/AG-D3，再继续缺失的独立门禁和提交态 evidence，最后重做逐项整体审计。

### 八类接续后的中期复核（2026-10-08）

本段更新上述较早的中期测试快照，不追改其当时结论。当前 `cc-switch-server` HEAD 仍为 `ac69ecb1c652c0ac924394203617df3b57fa2498`；43 个已跟踪路径改动、1 个未跟踪实现文件尚无新提交对象。CLIProxyAPI、Antigravity-Manager、codex2api、OmniRoute、grok2api、sub2api、kiro.rs、cli2api 的当前 HEAD/tree 与增强计划第 18.1 节冻结点逐一一致；七类可访问的 reference-delta `--check-sources` 通过，但只核对既有 committed-source/target observation。Qoder 的 `project-doc` 技能门禁仍未满足，仅自包含默认 delta 与 CLI oracle 通过，不能称其最新业务源码已审。

全量 Rust library 为 3283 passed / 1 ignored，其他目标 124 + 1 + 1 passed；rustfmt、`cargo check`、Clippy、Node audit 329/329、Node smoke 13/13、Provider contract/Phase 0/coverage/UI matrix、文档索引、差分空白检查通过。八类合同当前主体共有 28 个 `receipt=null/live_pending`，Claude Opus 5.5 与 Codex Astra 扩展各 1 个，合计 30 个独立真实验收项；Antigravity v2 对原两 rail 的重述不重复计数。`RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0` 的离线 readiness 返回 `blocked_inputs`、0 failures、八个真实输入缺口和 `deployment-not-tested`；该命令的 `local-contracts-unverified` 只说明它跳过内置测试，与前述独立全量测试不冲突。此处不是最终完成审计：新增工作树实现缺 committed-target observation，所有真实 receipt 仍缺，GR-N4/KI-N5/CB-N4 等专项门禁未提升。按用户约束未运行 UI 自动化或部署测试。

## 2026-10-08 CodeBuddy downstream-disconnect differential

CB-D3 只读参考是 `cli2api@ead3202649032c95cf8dd4783bc9860c24a3ce79`（tree `753dfc6b9ab24108c8b790a2633cc9643b38401e`）的 `internal/gateway/compat_stream.go::streamLogStatus` 和各 Surface 调用点；该文件提交态 SHA-256 为 `f5c9b660ce0f2d13093fefb3d6c0c3256248fb1e16271eddd85857e656246d89`。参考当前 HEAD 是 `a6502521e1e509a4257153025c642240c39cd54b`（tree `876656d0780afde26af9b1ab0a87e0ea58d8c752`），工作树的 `proxy.html`、`proxy.md` 两个未跟踪文件继续排除；`ead3202..HEAD` 没有修改 `compat_stream.go`、`native_responses.go`、`responses.go` 或 `openai_stream.go` 的收尾逻辑。目标起点是 `cc-switch-server@ac69ecb1c652c0ac924394203617df3b57fa2498`（tree `af3aba76cb72a21f61c1c4e3e8aa0e76df5408da`）；本轮工作树实现尚未冻结提交态 target object。

Intl/CN × Claude Messages、Codex Responses、Gemini 三个 Surface 的 loopback 差分表明：在 `[DONE]` 前、收到 `[DONE]` 但上游 EOF 未到，以及已有 `finish_reason` 后 EOF 未到时断连，`ShareStreamInterruptGuard` 均将 UsageLog 标为 `client_cancelled` / `Interrupted`，并释放上游 body 与两类租约；HTTP 200 已提交不能再改写，但不因此把账务 outcome 标为 success。完整消费到 EOF 的正常流仍记 `completed`。另有一个先红后绿的真实局部竞态：Claude/Codex 在上游 EOF 验证后产出的最终 terminal chunk 已被下游取走，若客户端不再 poll，原 CodeBuddy 分支的成功记账位于 `yield` 后而不会执行，guard 反而误记取消。现仅在 CodeBuddy EOF 收尾分支先提交成功账务、再 yield 已生成的最终 chunk；`[DONE]` 唯一性、EOF、runtime generation 和 transform 检查均仍在其前，Gemini 的早期 `finishReason` 不被当作 EOF。

本地 CodeBuddy 关键词 Rust 71/71（`RUST_MIN_STACK=67108864`；默认测试线程栈会在既有 API 测试中溢出）、Intl/CN 私有 receipt Node gate 6/6、CodeBuddy reference-delta `--check-sources`、Provider coverage/UI matrix 通过。CB-D3 是本地 fixture 修复，不提升 Intl/CN 两站真实 receipt，也不启用 CB-N4 v4.1；没有进行 UI 自动化、部署或真实账号测试。

### CodeBuddy 私有 receipt 末次绑定复核接续

推荐参考 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 仍为已审提交态；两个未跟踪 `proxy.html` / `proxy.md` 继续排除。本项不从参考推断新的 CodeBuddy wire，只修本仓库私有 receipt 验收的时间窗口。原 `scripts/smoke/codebuddy-real-receipt.mjs` 在开始时读取 Account、三 Surface Provider 和 Share，然后查询三份目录、解析 receipt，但接受前不再核对绑定；Intl/CN 各自的 Account token generation、Claude Provider revision 或 Share revision 在期间变化时，六个 mock 都错误地接受旧 scope receipt，先红灯复现。

当前工作树在 receipt 内容校验后重新读取精确 Account、Provider 和 Share，按原有 generation/binding digest 重算并与起点逐字段比较；任一漂移都失败关闭，不输出凭据或接受旧 scope。正常 fixture 的控制面读取由各一次变为各两次，Intl/CN 两站仍保持 `contract_verified/live_pending`。这是接受时的末次快照栅栏，不声称跨三个独立 API 调用的原子事务，也不替代私有真实账号证据；生产 CodeBuddy 转发、Intl/CN 站点路由、receipt schema v2/revision 2 和 CB-N4 disabled 状态均未改变。

CodeBuddy 私有 receipt Node 为 13/13；reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、脚本语法和 `git diff --check` 均通过。`RUN_REAL=0` 下 Intl/CN 分别返回 `blocked_inputs/live_pending`，未访问真实服务。新增验收逻辑尚在工作树，不能写 committed-target v2 observation；没有 UI 自动化或部署测试。

### CodeBuddy 双站目录 fresh TTL 验收接续（2026-10-08）

本次不是新外仓 wire 判定：推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的提交态未变，两个未跟踪文档继续排除。目标生产 `src/proxy/codebuddy_runtime.rs::CODEBUDDY_MODEL_CATALOG_TTL_MS` 为 8 分钟；`prepare_codebuddy_runtime` 只在期内返回非 stale 缓存，过期后重新获取，瞬态失败的历史目录明确标为 stale。私有 receipt 旧校验器却允许 `source=codebuddy_live_model_catalog, stale=false` 的目录时间戳落后两小时或超前五分钟。Intl/CN × 9 分钟过旧和未来 2 分钟共四个 mock 先红后绿；7 分钟缓存两站正例通过。现验收脚本按生产 TTL 要求目录不足 8 分钟、最多超前 1 分钟，并在末次接受前重新核对三份快照的时间窗口。

CodeBuddy 专项 Node 20/20，全量 audit Node 329/329、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法与差分空白检查通过。`docs/provider/coverage.md` 已核对，生成的覆盖矩阵不因私有验收时限而变。生产 CodeBuddy wire/缓存 TTL、receipt schema v2/revision 2、CB-N4 disabled 均未改变；Intl/CN `RUN_REAL=0` 仍是 `blocked_inputs/live_pending`，没有真实 receipt。新增验收逻辑尚未提交，不能追加假的 committed-target observation；未进行 UI 自动化或部署测试。

### CodeBuddy 双站私有 receipt 控制面响应大小栅栏（2026-10-08）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的提交态未变，未跟踪 `proxy.html`、`proxy.md` 继续排除。本项是本仓库验收器自身的资源边界，不推断新 CodeBuddy wire。旧 `codebuddy-real-receipt.mjs` 在完整读取控制面响应后才检查 4 MiB；Intl Account 的声明长度超限、CN Provider 的无长度分块实际超限且连接不结束，两条 loopback 反例均先红灯（约 5 秒超时）。现先查 `Content-Length`，读取时逐块累计实际字节并在超限时中止，两例均快速、脱敏地失败关闭。CodeBuddy 专项 Node 23/23、全量 audit Node 351/351、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

生产转发、缓存 TTL、Intl/CN 站点绑定、receipt schema v2/revision 2 均不变，CB-N4 `deepseek-v4.1-flash` 仍 disabled。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；当前 JS 加固未提交，不追加假的 committed-target observation。未做 UI 自动化或部署测试。

### CodeBuddy 双站私有 receipt 静态硬链接边界（2026-10-08）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的已提交状态不变，未跟踪 `proxy.html`、`proxy.md` 继续排除。本项只修目标验收器的证据文件位置约束：`realpath` 能拒绝仓库外符号链接指向仓库内，却不能识别仓库内外路径共用 inode 的硬链接。Intl/CN 两个合法 fixture 先分别复现错误接受；现要求已解析 receipt `nlink=1`，两例均在任何控制面请求前以固定脱敏错误失败关闭。正常双站 fixture、目录 TTL、绑定漂移和控制面响应大小用例保持通过；CodeBuddy 专项 Node 26/26、全量 audit Node 381/381、smoke Node 13/13，reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

静态硬链接检查不提供并发文件替换的原子防护，也不证明部署代码与本地 HEAD 一致。生产 CodeBuddy 转发、缓存 TTL、站点绑定和 receipt schema v2/revision 2 未变；CB-N4 `deepseek-v4.1-flash` 继续 disabled。Intl/CN 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求或 receipt。此 JS 加固未提交，不追加伪造的 committed-target observation；未做 UI 自动化或部署测试。

### CodeBuddy 双站私有 receipt 请求期间替换栅栏（2026-10-08）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的提交态未变，未跟踪 `proxy.html`、`proxy.md` 继续排除。本项只修目标验收器的文件读取窗口，不推断新的 CodeBuddy wire。旧流程启动时检查仓库外路径、权限和硬链接，却在 Account/Provider/Share 与三份目录请求后才按路径读取 receipt；Intl/CN 各自让首个 Account 请求将无效文件原子替换为有效文件，旧流程均错误写出 `contract_verified/live_pending`。现于控制面请求前从只读文件描述符限量获取最多 1 MiB 快照，复核文件类型、dev/inode、链接数、大小和权限，之后只解析该快照。两例均失败关闭，正常双站 fixture、静态硬链接、目录 TTL、绑定漂移及响应大小用例保持通过，CodeBuddy 专项 Node 29/29。

本项不改生产转发、Intl/CN 站点绑定、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策；快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 419/419、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法与差分空白检查通过。Intl/CN `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求或 receipt；CB-N4 仍缺 CN 厂商目录/接受性证据。新增 JS 未提交，不能以旧 committed-target observation 冒充当前实现证据；未做 UI 自动化或部署测试。

### CodeBuddy 双站 Account profile 站点验收门槛（2026-10-08）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的已提交状态未变；未跟踪 `proxy.html`、`proxy.md` 未读。其凭据 `domain` 决定 CN/Global ChatBase，是站点身份必须校验的交叉信号；目标的官方 CLI 证据与生产 `CodeBuddyAccountProfile::parse` 仍为优先依据。目标私有 receipt validator 原先只使用脚本 `--site` 和 receipt 自述，未读取真实 Account profile 的站点。Intl/CN × 错站点、缺投影、末次站点漂移共六个 loopback 反例先全部错误接受。现 `AccountPublicView` 仅对通过生产 profile parse（包含站点与受控 domain 校验）的 `codebuddy_oauth` Account 发布 `codebuddyCredentialScope.site`；validator 在首末两次读取均精确比较，不匹配或旧服务缺字段就失败关闭。投影不公开 domain、UID 或 token；六例转绿，正常双站 fixture 保持通过。

这不改变生产转发、站点路由、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策；控制面/fixture 不能代替 Intl/CN 各自独立的真实账号 receipt，也不证明跨 API 原子性。CodeBuddy receipt Node 36/36、关键词 Rust 72/72、全量 audit Node 458/458、smoke Node 13/13；`RUST_MIN_STACK=16777216 cargo test --all-targets --quiet` 为 library 3288 passed / 1 ignored，其他目标 124 + 1 + 1 passed。Clippy `--all-targets -D warnings`、rustfmt、CodeBuddy reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查均通过。Intl/CN 的 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前增量未提交，不能新增虚假的 committed-target observation；未做 UI 自动化或部署测试。

### CodeBuddy 双站三 Surface 私有目录签名绑定（2026-10-08）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` / tree `876656d0780afde26af9b1ab0a87e0ea58d8c752` 的已提交 Gateway `/v1/models` 是展示目录聚合入口，不是本 Server 的 Share 绑定证明；外仓未跟踪 `proxy.html`、`proxy.md` 排除。目标自身的 `/v1/models` 先由 Router 签名 Share 和 App 选定 Provider，普通 `providerId` 查询不参与选择。Intl/CN 私有 receipt validator 原只传 `app/providerId`，即使另一 Share 有同名模型也无法证明所选 Share/Provider。双站正常 fixture 各对三 App 增加预期绑定断言后先红；现在目录请求带 `expectedShareId/expectedProviderId`，服务端已有签名 ingress 测试覆盖匹配与错 Share/Provider 冲突。各站另核对三 App 各请求一次；CodeBuddy 专项 Node 38/38。

这只收紧验收目录的路由作用域，不改变生产 CodeBuddy wire、Intl/CN 身份、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled；目录通过不是厂商模型权益或跨 API 原子快照，双站仍需各自真实 receipt。新增 JS/文档工作树尚未提交，旧 committed-target observation 不能替代本轮实现证据；未执行 UI 自动化或部署测试。

2026-10-08 的 CodeBuddy 私有 receipt 接受时效复核只涉及本仓库 validator，不新增 `cli2api` 或厂商 wire 事实。Intl/CN 各以 `recordedAt` 距 24 小时上限 1.2 秒、末次 Share 响应延迟 2 秒构造本地反例；旧流程只在解析内容时查时间，两站均在 receipt 已过期后错误接受。现内容校验与末次绑定复核后共用同一时间谓词，过期即脱敏失败。目录自身的八分钟 TTL 末次复核保持不变；生产转发、站点身份、receipt schema v2/revision 2 及 CB-N4 disabled 决策均未改。末次读取不是跨 API 原子事务，离线 fixture 不能代替 Intl/CN 各自的真实 receipt。

2026-10-08 的 CodeBuddy 私有 receipt 文件快照补强仅涉及本仓库验收器：Intl/CN 各在首次 `readSync` 返回已读字节后、EOF 检查前，对同 inode 文件进行等长原位覆盖并强制改变 mtime。旧流程只核对初始 stat、读取长度和 EOF，两站均继续用旧字节错误输出 `contract_verified/live_pending`；现对同一只读描述符在读取前后比较 dev/inode、nlink、size、mode、纳秒 mtime/ctime，两例均在任何控制面请求前脱敏失败。此项不改变生产 CodeBuddy wire、站点/Account/Share 绑定、目录 TTL 或 receipt schema v2/revision 2；元数据复原/并发精确竞态不由它证明，Intl/CN 真实 receipt 仍分别缺失。

2026-10-08 的 CodeBuddy 双站 receipt 最终文件身份复核只修目标私有 validator，不新增 cli2api wire 事实。Intl/CN 各在首次 Account 控制面读取时将先前已快照的有效 receipt 换成另一个 inode 的无效文件，或对原 inode 等长原位覆盖并改变 mtime；旧流程四例仍用旧内存内容错误输出 `contract_verified/live_pending`。现保留读后文件指纹，在末次 Account/Provider/Share 和目录/receipt 时效检查后，重新解析配置路径并比较 dev/inode、nlink、size、mode、纳秒 mtime/ctime；四例均脱敏失败，正常双站保持通过。此复核不构成跨 API/文件系统原子事务，不能覆盖恶意元数据复原或最终检查后的改动，也不证明 Intl/CN 真实厂商 entitlement。生产 CodeBuddy wire、目录 TTL、站点身份、receipt schema v2/revision 2 与 CB-N4 disabled 决策均不变；新增 JS 未提交，不能用旧 committed-target observation 冒充本轮证据，未做 UI 自动化或部署测试。

2026-10-09 的 CodeBuddy Share 唯一绑定复核只修本仓库双站私有 validator，不新增 cli2api 或厂商 wire 事实。先核对生产 `Share` 投影：`bindings` 自身包含 `share.app` 的主绑定，导入不变量要求每 App 至多一条且主字段与该条一致。旧验收器却将主字段再次拼入数组，只用 `some` 寻找正确条目；Intl/CN × Claude/Codex/Gemini 的六个冲突重复绑定反例均错误输出 `contract_verified/live_pending`。现在直接逐 App 要求原始 `bindings` 中恰好一条精确匹配，并核对主字段与该条一致；再补 Intl/CN 两个主字段冲突反例。六例先红后绿，两个主字段反例亦拒绝，正常双站按真实 Share 形状保持通过且在目录发网前失败关闭。CodeBuddy 专项 Node 62/62、全量 audit Node 738/738、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。Intl/CN `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，真实 receipt 与 CB-N4 CN 目录/接受性证据仍缺。生产转发、receipt schema v2/revision 2 和 CB-N4 disabled 决策不变；当前工作树未提交，旧 committed-target observation 不证明本轮实现。未做 UI 自动化或部署测试。

2026-10-09 的 CodeBuddy 双站显式启用状态复核仍只修本仓库私有 validator，不新增 cli2api 或厂商 wire 事实。唯一绑定和主字段一致性已经校验，但旧条件只拒绝 `enabled=false` 或已存在且非 active 的 `status`，会把缺少任一字段的 Share 当作有效。Intl/CN 各构造缺 enabled、缺 status、disabled、paused 四种反例；缺字段四例先错误输出 `contract_verified/live_pending`，已有 disabled/paused 四例保持拒绝。现要求 `enabled=true/status=active`，八例均在目录请求前失败，正常两站与旧有绑定、目录、receipt 时效/文件、漂移 fixture 保持通过。CodeBuddy 专项 Node 71/71、全量 audit Node 888/888、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 CN 厂商目录/接受性与当前工作树提交态 observation 仍缺。生产 CodeBuddy wire、Share 路由、receipt schema v2/revision 2 和 CB-N4 disabled 决策不变；fixture 不是厂商接受性。参考 cli2api 的两个未跟踪 `proxy.html`、`proxy.md` 继续排除，未做 UI 自动化或部署测试。

2026-10-09 的 CodeBuddy Intl/CN 控制面投影与作用域复核仍只修本仓库私有 receipt validator。推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD 未变，未跟踪 `proxy.html`、`proxy.md` 继续排除，本项不引入新 CodeBuddy wire。目标 Server `ProviderView` 实际为顶层 `revision` 与 runtime `providerRevision`，旧 CodeBuddy mock 却给不存在的顶层 `providerRevision`；mock 改为真实形状后，Intl/CN 两个正常 fixture 先失败。旧 validator 从虚构字段回退成 `0`，可把实际 Provider revision 错误绑定进 receipt。两站各构造 Claude/Codex/Gemini 的缺顶层 revision、缺 runtime revision、两份不一致、仅有旧式虚构顶层字段、空白 fingerprint，另构造缺 Share revision，共十六类自洽反例；旧流程三十二例未按预期拒绝。现要求三个 Provider 各自的两份 revision 为非负安全整数且相等、三个 fingerprint 为非空白字符串、Share `configRevision` 明确为非负安全整数；正常双站按真实投影恢复通过，三十二例在目录请求前脱敏拒绝。CodeBuddy 专项 Node 104/104、全量 audit Node 1139/1139、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 CN 目录/接受性和当前工作树提交态 observation 仍缺。生产 CodeBuddy wire、站点身份、Share 路由、receipt schema v2/revision 2 与 CB-N4 disabled 决策不变；控制面复核不是原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

### CodeBuddy Intl/CN Account 布尔投影完整性复核（2026-10-09）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD/tree 未变，未跟踪 `proxy.html`、`proxy.md` 继续排除。本项只核对目标 Server 自身 `AccountPublicView` 与私有 receipt validator，不推导新 CodeBuddy wire。真实 `/api/accounts` 必带布尔 `hasApiKey` 与 `needsRelogin`；旧 Intl/CN validator 只拒绝值为 `true`，两站各构造两字段分别缺失、为 `null`、为字符串 `"false"` 的六种反例，共十二例均错误输出 `contract_verified/live_pending`。现要求两字段明确为布尔 `false`，十二例先红后绿，均在 Provider 与三份目录请求前脱敏失败，正常双站 fixture 保持通过。

此项只收紧私有验收证据，不改生产 CodeBuddy wire、Intl/CN 站点身份、Share 路由、receipt schema v2/revision 2 或 CB-N4 disabled 决策；控制面分次读取不是原子快照，fixture 不证明真实账号接受性。CodeBuddy 专项 Node 117/117、全量 audit Node 1280/1280、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。Intl/CN 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、CB-N4 国内站目录/接受性及当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

## 2026-10-08 Qoder receipt terminal verification hardening


TokenRouter 最新 HEAD 仍因仓库要求的 `project-doc` 技能未加载而处于 `unreviewed_skill_gate`；本轮没有读取其业务文件、执行其业务命令或运行需复核外部源码的 `--check-sources`，也没有把最新提交解释成 no-wire。仅在本仓库已有的 Qoder 三 rail 私有验收脚本上做离线差分。loopback 红灯证明：Claude/Codex/Gemini 流各自在一个成功 terminal 后再发 error/failed 帧，原 harness 仍写出 `contract_verified` receipt；Codex 流在 `response.completed` 前发 `[DONE]` 也会通过。修后共享 SSE 收集器先读至 EOF、扫描全部帧，再拒绝任一 error、failed、incomplete 或非最终位置/重复的 `[DONE]`；只有其后各 Surface 的唯一成功 terminal 检查通过，才允许写 receipt。

这只收紧 `scripts/smoke/qoder-real.mjs` 的证据可信度，不更改 COSY wire、Account/Provider/Share binding、三条 rail 的生产行为或既有 receipt schema v2/harness revision 2。正常的 Codex `response.completed` 后单个最终 `[DONE]` 仍可通过。离线 `qoder-real.test.mjs` 13/13、CLI oracle、默认 Qoder reference-delta、Provider coverage/UI matrix 均通过；默认 delta audit 只证明既有冻结对象未漂移，不等于最新 TokenRouter 已审。Global OAuth、Global PAT、CN OAuth 仍分别为 `live_pending`；没有真实账号 receipt，也没有运行 UI 自动化或部署测试。

### Qoder 三 Surface 工具参数 receipt 门禁接续

最新 `TokenRouter` 仍要求本会话不可用的 `project-doc` 技能，因此不读取其业务文件，不把未审提交写成 no-wire 结论；本项仅对照本仓库已冻结的 Qoder CLI oracle 和自有三 rail 验收脚本。原 `scripts/smoke/qoder-real.mjs` 只检查名为 `lookup` 的工具调用是否出现，六种 Claude/Codex/Gemini × 流/非流 mock 即使返回 `wrong-key` 参数，仍会写出 `tool=pass` receipt；六个负例均先复现红灯。

当前工作树要求工具参数是仅含 `key: "qoder-acceptance"` 的对象。Claude 非流检查 `tool_use.input` 和 ID；流式按 content block index 合并 `input_json_delta`，只在对应 block stop 后核对 ID、名称和完整参数。Codex 非流检查 `function_call.call_id/arguments`；流式只从完成的 `response.completed.response.output` 或 `response.output_item.done` 检查，不以尚未完成的 added/delta 冒充完整调用。Gemini 两种模式均检查 `functionCall.args`。解析失败只输出固定错误标签，不记录原始参数；正常分片 Claude 工具调用另有正例。此变更仅提高私有 receipt 的证据门槛，不修改 Qoder 生产 COSY wire、receipt schema v2/revision 2 或能力开关。

Qoder receipt Node 为 21/21；CLI oracle、默认 reference-delta、Server Provider contract/coverage/UI matrix、脚本语法与 `git diff --check` 均通过。三条 rail 的 `RUN_REAL=0` 分别仍为 `blocked_inputs/live_pending`，没有真实请求或 receipt；现有 reference-delta 审计仅复核旧冻结对象，不会访问受技能门禁保护的最新外仓源码。该工作树未提交，也未做 UI 自动化或部署测试。

## 2026-10-08 Kiro terminal-tool recovery and usage shadow

本轮仅用只读的已提交 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`（tree `5d22f040f0e4b0aa775e61547540b3c040293cd8`）作差分。`src/anthropic/stream.rs` 的该提交态 SHA-256 为 `1c2168d5f67ab8ce8cbcc1e2b7a97c5bdafb6ba598f179e1e169e86abdc6c1b0`，`src/anthropic/handlers.rs` 为 `ca41d9f8d49a6ffc339b83759a0ff8fc0826633a11585d3dafb30a33f7f9e3f4`。相关历史提交：无参工具 `93459b55f05f55558ea721ad59f7d7a14757a2dc`；流式 usage `700fbdbba2a854f52cd6d54ddf60159d1628ea71`；非流式 usage `baacf84216833a1662918d9dc5b9e5c23cd6c8dc`；web-search 循环 `dbb8d9c79889b52c7b7e767843c8b773f02c05fa`。目标起点是 `cc-switch-server@ac69ecb1c652c0ac924394203617df3b57fa2498`（tree `af3aba76cb72a21f61c1c4e3e8aa0e76df5408da`）；本轮实现仍在工作树，未冻结新的 target commit/tree，故未追加要求 committed target object 的 reference-delta v2 observation。

KI-N4 的精确红灯是 `toolUseEvent(stop=false,input="")` 后 `endEvent`：流与非流均漏掉无参工具；若先有文本，流仍可能发正常 `message_stop`。目标现改为仅在真正零字节 pending 时恢复 `{}`，多工具按 ID 排序并经过当前请求的名称注册表/工具桥接；任何非空未完成参数在生成成功终态前返回 `TOOL_JSON_INCOMPLETE`。显式 `stop=true` 的合法工具及已有“已停止但坏 JSON、另有可用输出时丢弃坏工具”边界不变。离线 Rust fixture 先红后绿，包含两 Surface、文本前缀、多工具、namespace 和非空半截拒绝；这不是上游真实账号验收。

KI-N5 只留 shadow：当 `contextUsage=80`、本轮输出 `9` 且无明确 input usage 时，当前报告输入 `80`，假设 context 包含本轮输出的候选值为 `71`。`metricsEvent.inputTokens` 和 `messageMetadataEvent.tokenUsage` 继续优先，后者显式 cache 零值不被本地估算覆盖。参考项目的扣减逻辑本身不能证明所有 Kiro auth kind、region、模型及事件顺序的真实语义；没有绑定账号的脱敏 receipt 前，不改变 `KiroUsageAccumulator::final_usage`、usage authority 或账务口径，保持 `live_pending`。Kiro/Amazon Q Compact 与共享 cache 仍 runtime disabled。

本地验证：Kiro 关键词 Rust 187/187、Amazon Q 14/14、receipt 门禁 5/5；`cargo fmt --check`、`cargo check --lib`、Clippy `--all-targets -D warnings`、Provider contract/coverage/UI matrix 及既有 Kiro reference-delta `--check-sources` 均通过。审计现阶段只验证旧冻结记录；KI-N4/KI-N5 的新 committed-target observation 仍须在实现提交后追加，不得把未提交工作树伪称已冻结。没有运行 UI 自动化、部署或真实账号测试。

### Kiro KI-N4 纯空白工具参数收尾接续

推荐参考的已提交 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 仍无新提交；其 `src/anthropic/stream.rs::ToolJsonAccumulator::finish` 将 `input.trim().is_empty()` 视作可恢复的无参工具，与收到 `stop=true` 时的解析一致。目标工作树的 `ToolJsonAccumulator::push(stop=true)` 已遵守这条语义，但 `finish()` 仅识别零字节；`toolUseEvent` 收到 `stop=false`、参数只有空格/换行/制表符后遇到 `endEvent` 的双 Surface fixture，先以 `TOOL_JSON_INCOMPLETE` 红灯复现。现在仅把收尾判定改为纯空白，按原有请求内工具名称注册表恢复 `{}`；含任何非空白的半截 JSON 继续在成功终态前失败，缓冲上限和多工具顺序不变。这是 KI-N4 的局部离线一致性修复，不改变 KI-N5 的 context usage authority。

新增 fixture 转绿后，Kiro 关键词 188/188，Kiro receipt Node 5/5，已冻结 Kiro reference-delta 的 `--check-sources`、Provider contract/coverage/UI matrix、Clippy `--all-targets -D warnings` 与 rustfmt 均通过。完整 `cargo test --quiet` 在默认线程栈被既有 CodeBuddy 测试栈溢出中断；使用 `RUST_MIN_STACK=67108864` 重跑为 library 3283 passed / 1 ignored，非 library 124 + 1 + 1 passed。`RUN_REAL=0` 的 Builder ID / `us-east-1` receipt gate 返回 `blocked_inputs/live_pending`，其余七个 scope 仍各自无 receipt；未联网。当前实现仍未提交，不能追加 committed-target v2 observation，也未做 UI 自动化或部署测试。

### Kiro 八路私有 receipt 目录与末次绑定门槛（2026-10-08）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交状态未变；本项是目标验收器独立缺口，不推断新的 Amazon Q/Kiro wire。旧 `scripts/smoke/kiro-real-receipt.mjs` 把任意正数目录时间戳和任意格式合法来源当作 fresh，fixture 使用的来源名甚至与 Server 生产实现不一致；查完两份目录和 receipt 后也未再核对 Account、Claude/Codex Provider、Share。四种 auth kind × 两 region × Account token generation/Claude Provider revision/Codex Provider revision/Share revision 的 32 个反例，以及八个 scope × 过旧/超前/伪来源目录的 24 个反例，均先错误接受。

验收现在只认 Server 生产代码返回的 `kiro_list_available_models` 与 `kiro_account_cache`，两份目录均须非 stale、在 5 分钟内且未来偏差不超过 1 分钟；5 分钟与 `kiro_runtime.rs` 的 fresh cache TTL 相符，24 小时 stale 展示缓存不能冒充真实验收。receipt 校验后重读精确 Account、两 Provider、Share，重算包含两代 generation、Provider revision/runtime fingerprint 和 Share revision 的绑定摘要；漂移即失败关闭。八路正常 fixture 和当前绑定 cache 的正例仍通过，专项 Node 64/64。末次控制面快照不构成跨 API 原子事务。

生产 EventStream、usage authority、账号/region 路由和旧 receipt schema v1/revision 1 均未改。KI-N5 context usage 仍只做 shadow，不改收费；Kiro/Amazon Q Compact 与共享 cache 继续 disabled。Kiro reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过；全量 Node audit 312/312。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，八份真实 receipt 仍 `null/live_pending`，当前工作树不能伪写新 committed-target observation；未做 UI 自动化或部署测试。

### Kiro 八路私有 receipt 控制面响应大小栅栏（2026-10-08）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的提交态未变。本项是本仓库验收器自己的资源边界，不推导新 Amazon Q/Kiro wire。旧 `kiro-real-receipt.mjs` 在完整读取控制面响应后才检查 4 MiB；Builder ID / `us-east-1` 的声明长度超限、API Key / `eu-central-1` 的无长度分块实际超限且连接不结束，两条 loopback 反例均先红灯（约 5 秒超时）。现先检查 `Content-Length`，读取时逐块累计实际字节并在超限时中止，均快速、脱敏地失败关闭。Kiro 专项 Node 67/67、全量 audit Node 343/343、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

生产 EventStream、KI-N5 usage authority、auth kind/region 路由、receipt schema v1/revision 1 均不变，Compact 与 shared cache 继续 disabled。八个 auth-kind × region 入口各自以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`；没有真实请求或 receipt。此 JS 改动未提交，不追加伪造的 committed-target observation；未做 UI 自动化或部署测试。

### Kiro 八路私有 receipt 静态硬链接边界（2026-10-08）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的已提交状态不变。本项只修本仓库验收器的证据文件位置约束：`realpath` 能拒绝仓库外符号链接指向仓库内，却不能识别仓库内外路径共用 inode 的硬链接。四种 auth kind × 两 region 的八个合法 fixture 各自先复现仓库外硬链接被错误接受；现对已解析 receipt 要求 `nlink=1`，八例都在任何控制面请求前以固定脱敏错误拒绝。正常八路 fixture、目录与绑定漂移、控制面响应大小用例保持通过；Kiro 专项 Node 76/76、全量 audit Node 374/374、smoke Node 13/13，reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

静态硬链接检查不构成文件并发替换的原子防护，也不能证明部署代码与本地 HEAD 一致。生产 Kiro/Amazon Q EventStream、auth kind/region 路由、KI-N5 usage authority 和 receipt schema v1/revision 1 均未改；Compact 与 shared cache 仍 disabled。八个 `RUN_REAL=0` 入口逐一返回 `blocked_inputs/live_pending`，无真实请求或 receipt。新增 JS 逻辑仍未提交，不追加假的 committed-target observation；未做 UI 自动化或部署测试。

### Kiro 八路私有 receipt 请求期间替换栅栏（2026-10-08）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的提交态未变。本项只修本仓库验收器的读取窗口，不推导新 Amazon Q/Kiro wire。旧流程启动时检查仓库外路径、权限和链接数，却在 Account/Provider/Share 及双目录请求后按路径读取 receipt；Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八路分别在首个 Account 请求将无效文件原子替换成有效文件，旧流程全部错误写出 `contract_verified/live_pending`。现于控制面请求前从只读文件描述符限量取得最多 1 MiB 快照，并复核文件类型、dev/inode、链接数、大小和权限；之后只解析该快照。八例均失败关闭，正常八路 fixture、硬链接、目录时效、绑定漂移和响应大小用例保持通过，Kiro 专项 Node 85/85。

这是私有证据门槛，不改生产 EventStream、auth kind/region 路由、KI-N5 usage authority、receipt schema v1/revision 1；Compact 与 shared cache 仍 disabled。快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 412/412、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过。八路 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求或 receipt；新增 JS 尚未提交，不能将旧 committed-target observation 当作当前实现证据。八份真实 receipt、KI-N5 计量权威证据及最终整体 review 仍缺，未做 UI 自动化或部署测试。

### Kiro 八路 receipt 的真实 auth kind / runtime region 控制面投影（2026-10-08）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` / tree `5d22f040f0e4b0aa775e61547540b3c040293cd8` 的已提交对象未变。目标旧 `AccountPublicView` 只有 token 形状、`tokenType` 和代际，不公开 Kiro 的实际 auth kind/出站运行区域；`kiro-real-receipt.mjs` 虽要求八个 auth-kind×region 私有 receipt，却仅将操作者选值写入摘要而不比对 Account 的真实值。Builder ID 选值遇到 IdC 账号投影、API Key 的 EU 选值遇到 US 实际区域，两例回环 fixture 先错误接受。现在仅对 `kiro_oauth` 公开脱敏的 `kiroCredentialScope.authKind/runtimeRegion`：auth kind 复用现有 refresh dispatch 解析，region 复用 `operational_runtime_identity_from_account` 的出站权威；无有效运行身份时不发布该投影。验收器要求两字段与选择完全一致，缺字段的旧服务也失败关闭。profile ARN、profile/raw 内容、token 与机器 ID 均不进入响应；receipt schema v1/revision 1、生产 EventStream、region/Account 选择和 KI-N5 usage authority 不变。

错 kind/region 两例先红后绿，缺投影反例与八路正常 fixture 一并通过；Kiro receipt Node 89/89、全量 Node audit 447/447、Kiro Rust 190/190，Account 公开投影及 auth-kind 解析定点 Rust 2/2，全量 Rust library 3286 passed / 1 ignored，其他目标 124 + 1 + 1 passed。Clippy `--all-targets -D warnings`、rustfmt、reference-delta `--check-sources`、Provider Phase 0/Server contract/coverage/UI matrix、文档索引及差分空白检查通过。八个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，八份真实 receipt 仍为 `null/live_pending`；KI-N5 context usage 扣减仍只做 shadow，Compact/shared cache 继续 disabled。当前增量尚未提交，既有 committed-target observation 不能代表本工作树；未做 UI 自动化或部署测试。

### Kiro 八路双目录的签名 Share / App Provider 验收绑定（2026-10-08）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态未变，本项不新增 Kiro/Amazon Q wire 事实。目标 `/v1/models` 由 Router 签名 Share 按 App 选择 Provider；旧 Kiro 私有验收器的 Claude、Codex 请求只有普通 `app/providerId`，不能证明 Router token 实际指向 receipt 中的 Share。八个 auth-kind×region fixture 各自因缺预期 Share/Provider 参数先红；现每份两条目录查询分别带 `expectedShareId/expectedProviderId`，Server 的签名绑定栅栏在目录工作前拒绝错 Share 或错 App Provider。此项只加强私有目录证据，不把 KI-N5 shadow 提升为账务权威，不开启 Compact/shared cache；八份真实 receipt 仍各自 `live_pending`。

2026-10-08 的 Kiro 双目录时效复核只修目标验收器，不推导新 Kiro/Amazon Q wire。八个 auth-kind×region scope 各让 Claude、Codex 某一份目录在取得时距五分钟上限一秒，末次 Share 响应延迟 1.5 秒；旧流程 16 例全部在目录过期后错误接受。现把五分钟/未来一分钟时间判定收敛为同一谓词，末次绑定复核后分别重查两份目录，16 例失败关闭。生产 EventStream、KI-N5 usage authority、目录 TTL、receipt schema v1/revision 1 不变；Compact/shared cache 继续 disabled。这是末次非原子快照，八份真实 receipt 仍各自 `live_pending`。

2026-10-08 的 Kiro 八路 receipt 时效复核仍只涉及目标私有 validator。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 各将 `recordedAt` 设为距 24 小时上限 1.2 秒，延迟末次 Share 响应 2 秒；旧流程八例均在 receipt 过期后错误接受。现初次与末次复核共用 receipt 时间谓词，并以同一接受时刻核对 Claude/Codex 两份目录与 receipt，八例均脱敏失败关闭。生产 EventStream、KI-N5 usage authority、receipt schema v1/revision 1 不变，Compact/shared cache 继续 disabled；八份真实 receipt 仍各自 `live_pending`。

2026-10-08 的 Kiro 八路私有 receipt 文件快照复核只修本仓库验收器，不引入新的 kiro.rs wire 推断。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 各在首次读取有效字节后、EOF 前把同 inode receipt 等长原位覆盖并改变 mtime；旧流程八例均错误输出 `contract_verified/live_pending`。现同一只读描述符在读取前后比较 dev/inode、nlink、size、mode 与纳秒级 mtime/ctime；八例均在控制面请求前脱敏失败。元数据复原或精确并发写入下的原子快照不由此证明；生产 EventStream、auth kind/region 路由、KI-N5 usage authority、receipt schema v1/revision 1 均不变，Compact/shared cache 保持 disabled。八份真实 receipt 仍分别 `live_pending`，未提交 JS 不构成新的 committed-target observation；未做 UI 自动化或部署测试。

2026-10-08 的 Kiro 最终文件身份复核仍只修本仓库私有 validator，不新增 kiro.rs wire 事实。Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八个 scope 各在首次 Account 读取时把已快照的有效 receipt 路径替换为另一 inode 的无效文件，或将原 inode 等长原位覆盖并改变 mtime；旧流程十六例全部错误接受旧快照。现保存读取后的 bigint 文件指纹，在最终 Account/双 Provider/Share、双目录和 receipt 时效复核后重新解析配置路径，并比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；十六例均脱敏失败，正常八路与既有目录/绑定 fixture 通过。末次检查不是跨 API/文件系统原子事务，元数据复原及检查后的变化仍可能逃过此门槛；离线 fixture 不是厂商接受性。Kiro 专项 Node 149/149、全量 audit Node 681/681、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；八份真实 receipt 和 KI-N5 专项证据仍缺，当前 JS 未提交，不以旧 committed-target observation 冒充新实现证据。未做 UI 自动化或部署测试。

2026-10-08 的 Kiro 最终内容复核继续只修本仓库八路私有 validator，不新增 kiro.rs wire 事实。Builder ID、IdC、Social、API Key × 两区域各在首次 Account 请求期间把原 receipt 等长改写，并由测试预加载钩子让末次路径与描述符元数据读取仍报告原指纹；旧流程八例错误输出 `contract_verified/live_pending`。现最终通过 `O_NOFOLLOW` 只读描述符重读完整、至多 1 MiB 的原始字节和 EOF，复核读前读后指纹及配置路径；八例均脱敏失败，正常八路与既有替换、原位改写、双目录、绑定及时效 fixture 保持通过。元数据伪装只是对抗 fixture，不证明普通用户能恢复真实 `ctime`；末次检查后的竞态与跨 API 原子性仍不保证，也不能由 fixture 推断厂商接受性。Kiro 专项 Node 158/158、全量 audit Node 728/728、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；八份真实 receipt 与 KI-N5 专项证据仍缺，当前工作树未提交，不以旧 committed-target observation 冒充新实现证据。未做 UI 自动化或部署测试。

2026-10-09 的 Kiro 八路 Share 绑定复核只修本仓库私有 validator，不新增 kiro.rs 或厂商 wire 事实。生产 Share 原始 `bindings` 已含主 App 条目，导入不变量要求每 App 至多一条且主字段一致；旧验收器却另拼主字段再用 `some` 找 Claude/Codex Provider，也不检查 enabled/status，旧 mock 还缺主字段与启用状态。Builder ID、IdC、Social、API Key × 两区域各构造 Claude 重复冲突绑定、Codex 重复冲突绑定、主字段冲突、disabled、paused 五类反例，旧逻辑四十例全部错误输出 `contract_verified/live_pending`。现按真实 Share 投影要求原始 `bindings` 对两 App 各恰好一条精确绑定、主字段与其唯一条目一致，且 `enabled=true/status=active`；四十例先红后绿，正常八路与双目录、漂移、时效 fixture 保持通过，错误在目录请求前拒绝。Kiro 专项 Node 199/199、全量 audit Node 854/854、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 usage authority 及当前工作树提交态 observation 仍缺。生产 Kiro/Amazon Q EventStream、auth kind/region 路由、Share 路由、receipt schema v1/revision 1 和 Compact/shared cache disabled 决策不变；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

2026-10-09 的 Kiro 八路控制面投影与 revision/fingerprint 复核只修本仓库私有 receipt validator。推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态未变且参考工作树干净，本项不推导新 Amazon Q wire。目标 Server 的 `ProviderView` 是顶层 `revision` 加 runtime `providerRevision`，旧 Kiro mock 却使用不存在的顶层 `providerRevision`；mock 改为真实形状后，八个正常 auth-kind×region scope 先全部失败。旧 validator 从虚构字段或 Provider 内层 revision 回退，可能把真实 Provider 作用域记为 `0`。八路各构造 Claude/Codex 的缺顶层 revision、缺 runtime revision、两份不一致、仅有旧式虚构字段、空白 fingerprint，另构造缺 Share revision，共十一类自洽反例；旧流程八十八例全部错误输出 `contract_verified/live_pending`。现要求两份 Provider 各自的顶层/runtime revision 均为非负安全整数且相等、两个 fingerprint 为非空白字符串、Share `configRevision` 明确为非负安全整数；正常八路按真实投影恢复通过，八十八例均在双目录发网前脱敏拒绝。Kiro 专项 Node 288/288、全量 audit Node 1057/1057、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 计量权威及当前工作树提交态 observation 仍缺。生产 Kiro/Amazon Q EventStream、auth kind/region 路由、Share 路由、receipt schema v1/revision 1 与 Compact/shared cache disabled 决策不变；多次控制面读取不是原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

### Kiro 八路 Account relogin 布尔投影完整性复核（2026-10-09）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD/tree 未变且工作树干净。本项只核对目标 Server 自身 `AccountPublicView` 与私有 receipt validator，不推导新的 Kiro/Amazon Q wire。真实 `/api/accounts` 必带布尔 `needsRelogin`；旧 Builder ID、IdC、Social、API Key × 两区域的八路 validator 只在该值为 `true` 时拒绝。每路分别构造缺字段、`null`、字符串 `"false"` 三种反例，旧流程二十四例全部错误输出 `contract_verified/live_pending`。现要求精确布尔 `false`；二十四例先红后绿，均在 Provider 与双目录请求前脱敏失败，正常八路 fixture 保持通过。

此项只收紧私有验收证据，不改生产 EventStream、auth kind/region 路由、Share 路由、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策。离线 fixture 不证明真实厂商接受性，分次控制面读取不是原子快照。Kiro 专项 Node 313/313、全量 audit Node 1245/1245、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；八份真实 receipt、KI-N5 专项证据和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Kiro 八路 Claude/Codex 双目录接受前内容复核（2026-10-09）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交 HEAD/tree 未变、工作树干净；KI-N5 context usage 仍只作 shadow，本项不推导新的 Kiro/Amazon Q EventStream wire。目标 Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 八个私有 receipt 已绑定初次 Claude/Codex 两份完整目录摘要，但末次 Account、双 Provider、Share 复核后只检查旧时间戳。八路 × 两 App 各构造第二次目录撤下所选模型、添加模型、标为 stale、改成伪来源或给出超前时间，共八十个 loopback 负例；旧 validator 只请求首轮双目录，错误输出 `contract_verified/live_pending`。

现在末次绑定后通过同一签名 Share 与对应 App Provider 栅栏分别重读 Claude/Codex 目录；两份当前内容摘要须各自等于 receipt 绑定的初次摘要，模型仍唯一、来源合法、非 stale，四份目录时间戳在共同接受时刻均新鲜。八十个负例先红后绿；另按八路 × 两 App 验证 `kiro_list_available_models`→`kiro_account_cache` 合法来源切换十六个正例，不要求来源字符串不变。Claude 末次目录先失败会立即终止，不发无意义的第四次 Codex 目录请求。Kiro 专项 Node 410/410、全量 audit Node 1475/1475、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。此项只提高私有目录证据门槛，不改变生产目录 TTL、EventStream、auth kind/region 路由、KI-N5 计量权威、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策；分次读取不是跨控制面/上游原子快照，fixture 不证明厂商接受性。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，八份真实 receipt、KI-N5 专项证据和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Kiro 八路双 App 目录 ID 完整性与顺序无关摘要（2026-10-09）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的已提交参考状态未变、工作树干净；本项仅调整目标 Server 私有 receipt 验收器，不从参考推导新 Kiro/Amazon Q wire。旧验收器只要求所选模型恰好出现一次，允许完整目录中其他条目缺失、空白或重复 ID；完整数组直接计算摘要，纯行顺序变化又会误判为内容漂移。Builder ID、IdC、Social、API Key × 两区域 × Claude/Codex 各构造三种无效 ID，共四十八个旧流程错误接受的 loopback 反例；另有十六个纯重排正例在旧流程被错误拒绝。现要求初次与末次目录的所有模型 ID 均为非空、无首尾空白且唯一，按 ID 排序后计算完整 `data` 摘要。四十八个负例和十六个正例先红后绿；十六个末次模型能力字段变化反例仍被拒绝，已有模型增删、stale、伪来源、超前时间和合法来源切换的回归保持原语义。

本项不改变生产 EventStream、目录顺序/缓存、auth kind/region 路由、KI-N5 usage authority、receipt schema v1/revision 1 或 Compact/shared cache disabled 决策。Kiro 专项 Node 492/492、全量 audit Node 1713/1713、smoke Node 13/13；Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。八个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`。离线 fixture 不能证明真实厂商接受性，多次目录/控制面读取不是原子快照。八份私有真实 receipt、KI-N5 专项证据及当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Kiro 模型目录缓存身份隔离（2026-10-09）

推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态仍无新差异，工作树干净，`--check-sources` 已复核冻结对象。本项是目标 Server 的生产缓存作用域修复：旧 `kiro_runtime.rs::ModelCacheKey` 仅凭账号 ID、代际、profile ARN 与区域等定位目录，未区分同 ID/同代际的凭据替换、同 ID/同 token 的账号删除重建，以及实际 ListAvailableModels 请求中的机器 ID 变化。三条本地双身份回环测试各先命中旧目录错误失败；现在缓存键包含 AccountStore 进程内 incarnation 和域分离的请求身份 SHA-256 摘要，摘要只覆盖 access token、认证方式、token 类型、机器 ID，不把原文写入缓存键。`state.rs` 在刷新后、目录返回前复核账号实例、代际、请求身份、有效运行身份和 Provider 绑定；三例转绿。该结果不改变 Kiro/Amazon Q 请求 wire、目录 TTL、KI-N5 计量权威、八路 receipt schema v1/revision 1 或 Compact/shared cache disabled 决策，也不证明真实厂商接受性。

完整 Rust library 3294 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Kiro receipt Node 492/492、全量 audit Node 1896/1896、smoke Node 13/13、Clippy `--all-targets -D warnings`、rustfmt、Kiro reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 与文档索引通过。`src/state.rs` 的 Phase 0 writer 摘要已从最终源码重新生成。八个 `RUN_REAL=0` 全是 `blocked_inputs/live_pending`；八份私有真实 receipt、KI-N5 专项证据和本轮未提交工作树的 committed-target observation 仍缺。未做 UI 自动化或部署测试。

### Qoder 三 rail 三目录接受前内容复核（2026-10-09）

本项只审计目标 Server 自身的 Qoder 私有验收器。Qoder 推荐参考 TokenRouter 的 `AGENTS.md` 要求当前会话不可用的 `project-doc` 技能，因此未读取其业务源码，也未运行外仓 `--check-sources`；既有 reference-delta 仅作自包含核验，最新参考增量保持 `unreviewed_skill_gate`，不据此推断新 COSY wire。

Global OAuth、Global PAT、CN OAuth 三条 rail 的 receipt 原已绑定 Claude、Codex、Gemini 三份初次完整目录摘要，但末次 Account/三 Provider/Share 复核后只检查初次时间戳。每条 rail × 每个 App 分别构造第二次目录撤下选定模型、添加模型、标记 stale、改成伪来源或给出超前时间，共四十五个 loopback 负例；旧流程均仅请求首轮三份目录，错误发布 `contract_verified/live_pending`。现在末次绑定后通过同一签名 Share 与对应 App Provider 栅栏重读三份目录，要求选定模型仍存在、来源合法、非 stale，当前完整 `data` 摘要分别等于 receipt 绑定的初次摘要；初次与末次共六份时间戳在同一接受时刻复核新鲜性。四十五例先红后绿，正常三 rail 保持通过。

本项只收紧私有目录证据，不改变生产 COSY wire、site/rail/Account 路由、目录 TTL、receipt schema v2/revision 2 或既有 `live_pending` 决策。分次读取不是跨控制面/上游原子快照，fixture 不证明厂商 entitlement。Qoder 专项 Node 225/225；自包含 Qoder reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三条 `RUN_REAL=0` 入口与真实 receipt、登录/轮换、权威空目录、受控两段 401 及当前未提交工作树的 committed-target observation 仍待核验；未做 UI 自动化或部署测试。

收尾复核：全量 Node audit 1521/1521、smoke Node 13/13，本地 server smoke 通过；三条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。当前工作树未提交，不能以旧 committed-target observation 代表本轮实现。最终整体 review 未完成；下一单类别为 CodeBuddy。

### Qoder 三 rail 三 App 目录 ID 完整性与顺序无关摘要（2026-10-09）

推荐 TokenRouter 的 `AGENTS.md` 明确要求当前会话可调用 `project-doc`，而本会话没有加载该技能；因此未读取或运行其业务源码、未执行外仓 `--check-sources`。本项仅根据目标 Server 现有公开目录与私有 receipt 合同改进目标验收器，不推导新 COSY wire。旧验收器将完整目录中每个 ID 强制转成字符串并去空白、过滤无效条目，再用集合寻找三 App 公共模型；其他条目缺失、空白、重复或非字符串 ID 仍可能被接受，纯行重排又会改变完整数组摘要。Global OAuth、Global PAT、CN OAuth × Claude/Codex/Gemini × 四种无效 ID 共三十六个 loopback 反例在旧流程错误发布 `contract_verified/live_pending`；三 rail × 三 App 的九个纯重排正例在旧流程被错误拒绝。现要求初次和末次每份目录所有模型 ID 为非空字符串、无首尾空白且唯一，并按 ID 确定性排序后计算完整 `data` 摘要；三十六个负例与九个正例先红后绿。末次能力字段改变仍拒绝，已有模型增删、stale、伪来源、超前时钟回归保持原语义。

本项只收紧私有证据，不改变生产 Qoder COSY wire、site/rail/Account 路由、目录 TTL、receipt schema v2/revision 2 或 `live_pending` 决策；分次读取不是跨控制面/上游原子快照，离线 fixture 不证明真实账号 entitlement。Qoder 专项 Node 281/281、全量 audit Node 1769/1769、smoke Node 13/13；自包含 Qoder reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。三份真实 receipt、登录/轮换、权威空目录、受控两段 401 和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### CodeBuddy Intl/CN 三目录接受前内容复核（2026-10-09）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 HEAD/tree 未变，未跟踪 `proxy.html`、`proxy.md` 继续排除。只读核对其 WorkBuddy `/v3/config` 产品目录：上游失败不回退静态列表，禁用模型与 CLI agent allowlist 会影响可用模型。本项不从参考推导新 CodeBuddy wire，只修目标 Server 私有 receipt 验收器的目录时序。

Intl/CN 的 receipt 已绑定 Claude、Codex、Gemini 三份初次完整目录摘要，但旧 validator 在末次 Account/三 Provider/Share 复核后只检查初次目录时间戳。两站 × 三 App 各构造第二次目录撤下选定模型、添加模型、标记 stale、改为伪来源或给出超前时间五类反例，共三十例；旧流程都只请求首轮目录并错误输出 `contract_verified/live_pending`。现在末次绑定后通过同一签名 Share 与对应 App Provider 栅栏重读三份目录，要求每份当前目录来源合法、非 stale、恰好包含一次选定模型、完整 `data` 摘要分别等于 receipt 绑定的初次摘要；初次和末次共六份时间戳在同一接受时刻仍须新鲜。三十例先红后绿，正常双站 fixture 保持通过。

本项只收紧私有目录证据，不改变生产 CodeBuddy wire、站点身份、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策；分次读取不是跨控制面/厂商原子快照，fixture 不能替代 Intl/CN 独立真实 receipt。CodeBuddy 专项 Node 148/148；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件。两份真实 receipt、CB-N4 CN 目录/接受性与当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

收尾复核：全量 Node audit 1552/1552、smoke Node 13/13；Intl/CN 两个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`。最终整体 review 尚未完成，下一单类别按循环回到 Antigravity。

### CodeBuddy 双站三 App 目录 ID 完整性与顺序无关摘要（2026-10-09）

推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 WorkBuddy `/v3/config` 实现仍是只读参考；参考工作树中的 `proxy.html`、`proxy.md` 未跟踪文件排除。其目录会按 disabled 与 CLI agent allowlist 筛选模型，不能把目录顺序或静态 alias 当成真实 entitlement。本项只调整目标 Server 私有 receipt 验收，不推导新的 CodeBuddy wire。旧 Intl/CN validator 只要求所选模型恰好一次，允许完整目录其他条目缺失、空白、重复或非字符串 ID；完整数组直接计算摘要，又会误拒绝纯行重排。双站 × Claude/Codex/Gemini × 四种无效 ID 共二十四个反例在旧流程错误输出 `contract_verified/live_pending`；双站 × 三 App 的六个纯重排正例在旧流程被错误拒绝。现要求初次和末次每份目录全部 ID 为非空字符串、无首尾空白且唯一，按 ID 确定性排序再计算完整 `data` 摘要；二十四个负例、六个正例先红后绿。六个末次能力字段变化反例仍拒绝，已有模型增删、stale、伪来源、超前时间回归保持原语义。

本项只收紧私有目录证据，不改变生产 CodeBuddy wire、站点身份、目录 TTL、receipt schema v2/revision 2 或 CB-N4 disabled 决策。分次读取不是跨控制面/厂商原子快照，fixture 不能代替 Intl/CN 真实账号验收。CodeBuddy 专项 Node 186/186、全量 audit Node 1807/1807、smoke Node 13/13；CodeBuddy reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。Intl/CN 两个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。两份真实 receipt、CB-N4 CN 厂商目录/接受性和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Antigravity/AGY 双 rail Claude/Gemini 目录接受前内容复核（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD/tree 未变；本项不推导新 Google wire，只修目标 Server 私有 receipt 验收器。`antigravity_oauth`、`agy_oauth` 已分别将 Claude/Gemini 两份初次完整目录摘要绑定进 receipt，但旧 validator 在末次 Account/双 Provider/Share 复核后只查初次目录时间戳。双 rail × 两 App 各构造第二次目录撤下所选模型、添加模型、标记 stale、改为伪来源或给出超前时间五类 loopback 反例，共二十例；旧流程均只请求首轮双目录并错误输出 `contract_verified/live_pending`。

现在末次绑定后通过同一签名 Share 和对应 App Provider 栅栏重读 Claude/Gemini 目录，要求当前目录来源权威、非 stale、所选模型恰好出现一次、完整 `data` 摘要分别等于 receipt 绑定的初次摘要；初次和末次共四份目录时间戳在同一接受时刻复核新鲜性。二十例先红后绿，正常双 rail fixture 保持通过。此项只收紧私有证据，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction、目录 TTL、receipt schema v2/revision 2 或 compaction disabled 决策；分次读取不是跨控制面/上游原子快照，fixture 不证明 Google 账号权益。

Antigravity 专项 Node 139/139、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt、AG-N6/AG-D3 专项接受性和当前未提交工作树的 committed-target observation 仍缺。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；未做 UI 自动化或部署测试。

收尾复核：全量 Node audit 1573/1573；两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。最终整体 review 尚未完成，下一单类别为 Claude。

### Antigravity/AGY 双 rail 目录 ID 完整性与顺序无关摘要（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的已提交 HEAD/tree 均未变，参考工作树干净。本项只调整目标 Server 私有 receipt 验收，不推导新的 Google wire。旧双 rail validator 只要求所选模型恰好一次，允许完整目录中其他条目缺失、空白、重复或非字符串 ID；摘要直接使用返回数组，纯行重排也会被误判为内容变化。`antigravity_oauth`、`agy_oauth` × Claude/Gemini × 四种无效 ID 共十六个 loopback 反例在旧流程错误输出 `contract_verified/live_pending`，四个纯重排正例在旧流程被错误拒绝。现要求初次与末次每份目录所有模型 ID 为非空字符串、无首尾空白且唯一，按 ID 确定性排序后计算完整 `data` 摘要。十六个负例与四个正例先红后绿；四个末次能力字段变化反例仍拒绝，既有模型增删、stale、伪来源与超前时钟回归保持原语义。

本项只收紧私有目录证据，不改变生产 Antigravity/AGY wire、AG-N6 requestType、AG-D3/compaction、目录 TTL、receipt schema v2/revision 2 或 compaction disabled 决策。分次读取不是跨控制面/上游原子快照，fixture 不证明 Google 账号接受性。Antigravity 专项 Node 165/165、全量 audit Node 1833/1833、smoke Node 13/13；Antigravity reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。两份真实 receipt、AG-N6/AG-D3 专项证据和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Claude 五 operation 静态模型目录接受前一致性复核（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD/tree 未变、参考工作树干净。本项依据目标 Server 自身版本化 `claude_code_wire_profile` 静态目录合同收紧私有 receipt validator，不推导新 Anthropic wire。旧五项 validator 仅从初次签名 Share 目录确认所选模型恰好一次；末次 Account/Provider/Share 复核后不再读取目录，也不检查目录 `source/stale/fetchedAtMs`。五 operation 各构造第二次目录撤下选定模型、添加模型、标记 stale、改为伪来源或改动 capture 时间五类反例，共二十五例旧流程只读首轮目录并错误输出 `contract_verified/live_pending`。

现在初次目录必须显式为 `source=claude_code_wire_profile`、`stale=false`，且 capture 时间为正的安全整数；末次绑定后通过同一签名 Share/Provider 栅栏重读目录，要求选定模型仍恰好一次，并比较两次完整 `data` 摘要和上述元数据。二十五例先红后绿，正常五项 fixture 保持通过。静态 capture 时间不套动态 TTL；目录摘要没有写入现有 receipt schema v1，只证明单次验收期间目录未漂移，不能证明真实订阅 entitlement 或跨 API 原子性。

Claude 专项 Node 220/220、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 接受性和当前未提交工作树的 committed-target observation 仍缺。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；未做 UI 自动化或部署测试。

收尾复核：全量 Node audit 1599/1599；五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。最终整体 review 尚未完成，下一单类别为 Codex。

### Claude 五 operation 静态目录 ID 完整性与顺序无关摘要（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD/tree 未变，参考工作树干净。本项仅调整目标 Server 的私有 receipt 验收器，不推导新 Anthropic wire，也不把版本化静态目录当作账号 entitlement。旧 validator 只要求所选模型 ID 恰好出现一次，允许完整目录中其他条目缺失、空白、重复或非字符串 ID；直接对返回数组计算摘要，又会误拒绝纯行重排。五 operation × 四种无效 ID 共二十个 loopback 反例在旧流程错误输出 `contract_verified/live_pending`，五个纯重排正例在旧流程被错误拒绝。现要求初次与末次完整目录所有 ID 为非空字符串、无首尾空白且唯一，按 ID 确定性排序后计算完整 `data` 摘要。二十个负例与五个正例先红后绿；五个末次能力字段变化反例仍拒绝，既有模型增删、stale、伪来源与 capture 时间漂移回归保持原语义。

本项只收紧单次私有目录稳定性证据，不改变生产 Claude OAuth wire、静态目录内容、CL-N7/CL-N8、canonical plan 或 receipt schema v1/revision 1。摘要没有绑定进现有 receipt，分次读取不是跨 API 原子快照，fixture 不证明真实 Anthropic entitlement。Claude 专项 Node 252/252、全量 audit Node 1865/1865、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。五份真实 receipt、续线程/beta 厂商接受性和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Codex Astra/WS 绑定目录与 Account manifest 接受前复核（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交 HEAD/tree 未变且参考工作树干净。本项不推导新 OpenAI wire，只修目标 Server 私有 receipt 验收器。`gpt_6_astra` 与 `ws_prewarm` 先通过签名 Share/预期 Provider 查询公开目录和绑定账号 `client_version` manifest，旧 validator 仅确认两者在初次读取时各包含一次选定模型；末次 Account/Provider/Share 复核后不重读。两 operation 各构造第二次公开目录撤下或添加模型、manifest 撤下或添加模型、所选模型 `use_responses_lite` 能力变化五类反例，共十例旧流程只读首轮目录/manifest 并错误输出 `contract_verified/live_pending`。

现在末次绑定后经同一签名 Share/Provider 栅栏重读两份证据，两次各要求选定模型恰好一次；对公开目录完整 `data` 与账号 manifest 完整 `models` 分别按 id/slug 规范化顺序后比较摘要。十例先红后绿；两 operation 的 manifest 纯行顺序变化正例保持通过。三个 Image 2.5 operation 不走这项模型/WS 门槛。本项不改生产 HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策；目录/manifest 摘要未绑定进既有 receipt，只证明单次验收期间未漂移，分次读取不是跨 API 原子快照，fixture 不证明 ChatGPT 账号 entitlement。

Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；未做 UI 自动化或部署测试。

收尾复核：Codex 专项 Node 183/183、全量 Node audit 1613/1613、smoke Node 13/13；五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。最终整体 review 尚未完成，下一单类别为 Cursor。

### Codex Astra/WS 完整目录与 Account manifest 模型标识完整性（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态未变，参考工作树干净。本项只调整目标 Server 的 Astra/WS 私有 receipt 验收器，不推导新 OpenAI wire。旧流程已重读公开 Codex 目录和绑定账号 manifest，并比较按模型 id/slug 排序的完整摘要；但两份列表都只要求所选模型恰好一次，其他条目缺失、空白、重复或非字符串 id/slug 仍可通过。两 operation × 两份列表 × 四种无效标识共十六个 loopback 反例在旧流程错误输出 `contract_verified/live_pending`。现要求初次及末次完整目录的所有 id、manifest 的所有 slug 均为非空字符串、无首尾空白且各自唯一；排序使用确定性字符串比较。十六个反例先红后绿，公开目录与 manifest 的纯行重排仍可通过；末次目录能力字段变化、既有模型增删和 manifest `use_responses_lite` 变化仍拒绝。三个 Image 2.5 operation 不套这条 Astra/WS 门槛。

本项只提高私有模型证据完整性，不改变生产 Codex HTTP/WS/Image wire、Share 路由、receipt schema v1/revision 1、Images disabled 或 WS prewarm 未启用决策。两份摘要未写入现有 receipt，分次读取不是跨 API 原子快照，fixture 不证明真实 ChatGPT 账号权益。Codex 专项 Node 205/205、全量 audit Node 1887/1887、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。五份真实 receipt 和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Cursor API Key 完整目录接受前内容复核（2026-10-09）

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 Cursor 参考对象经 `--check-sources` 复核；参考工作树中未提交的 Codex 内容继续排除。本项不推导新 Cursor protobuf/session wire，只收紧目标 Server 私有 API Key receipt 验收。旧 validator 已在末次 Provider/Share 绑定后重读签名目录，检查选定模型仍唯一、`source=cursor_public_api`、非 stale 与双时间戳新鲜，但不比较完整目录内容。API Key 两个 loopback 反例分别在第二次目录添加模型、改变所选模型的 `capabilities.tools`，旧流程均错误输出 `contract_verified/live_pending`。

现在 API Key 首末两份目录各把完整 `data` 按模型 id 排序后计算摘要，并在共同接受时刻比较；模型添加或能力字段变化均脱敏拒绝，目录纯行顺序变化仍可通过。OAuth 的静态 alias 目录不是账号 entitlement，不额外重读或提高证据等级。此项只证明单次私有验收期间 API Key 目录稳定，不将摘要写进现有 receipt schema v2，也不改变生产路由、目录 TTL 或 rail 身份；分次读取不是跨控制面/上游原子快照，fixture 不证明真实 Cursor 权益。

Cursor 专项 Node 90/90、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；两份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；未做 UI 自动化或部署测试。

收尾复核：全量 Node audit 1617/1617；OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`。最终整体 review 尚未完成，下一单类别为 Grok。

### Cursor API Key 完整目录 ID 门槛（2026-10-09）

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 Cursor 参考对象仍作只读证据；参考工作树中未提交的 Codex 内容继续排除。本项只调整目标 Server 私有 API Key receipt 验收，不推导新 Cursor protobuf/session wire。旧 validator 已在末次绑定后重读动态目录、比较按模型 ID 排序的完整 `data` 摘要，却只要求所选 `*-fast` 模型恰好一次；其他条目缺失、空白、重复或非字符串 ID 仍可能被接受，排序还把坏 ID 转为空串。四种 loopback 反例在旧流程错误输出 `contract_verified/live_pending`。现要求初次和末次 API Key 完整目录所有模型 ID 为非空字符串、无首尾空白且唯一，并按确定性 ID 顺序计算摘要；四例先红后绿。纯行重排仍接受，已有模型增加与能力字段变化、stale、伪来源和时间失效反例仍拒绝。OAuth 的静态 alias 目录不是账号 entitlement，不加此动态目录门槛。

本项只收紧私有动态目录证据，不改变生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份、Share 路由、目录 TTL 或 receipt schema v2/revision 2。完整摘要不写入既有 receipt，分次读取不是跨控制面/上游原子快照，fixture 不证明真实 Cursor 权益。Cursor 专项 Node 95/95、全量 audit Node 1892/1892、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。OAuth/API Key 两条 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。两份真实 receipt 和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Grok 三 operation 目录 ID 完整性与顺序无关摘要（2026-10-09）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交参考对象经 `--check-sources` 复核；后者六项未提交 Codex 内容继续排除。GR-N4 同账号版本/header 接受性仍为 `live_pending_no_wire_change`。本项只收紧目标 Server 私有 receipt 目录证据，不推导新 xAI wire。现有三 operation validator 已绑定初次完整 `data` 摘要，并在末次 Account/Provider/Share 复核后重读目录；但只检查所选模型唯一，允许其他条目缺 ID 或重复 ID，同时数组顺序变化会使相同目录摘要不等。

三 operation 各构造其他模型 ID 重复、缺失两类首轮目录反例，共六例旧流程错误输出 `contract_verified/live_pending`；另各构造所选模型与第二模型纯行顺序对调，共三例旧流程错误拒绝。现在对每份目录的所有 ID 要求非空、无首尾空白且唯一，再按 ID 排序后计算完整 `data` 摘要。六个反例先红后绿，三个纯重排正例先红后绿；三项所选模型能力字段改变的负例及原有模型增删、stale、伪来源、超前时间门槛继续通过。`upstream`→`cache_fresh` 等合法来源切换仍不要求字面相同。

此项只改变私有证据的规范化与失败关闭规则，不改变生产 Grok HTTP/WS/media/compaction wire、目录顺序或缓存、默认 CLI 版本、group header、receipt schema v1/revision 1 或 remote compaction disabled 决策。分次读取不是跨控制面/上游原子快照，fixture 不证明真实 xAI entitlement。Grok 专项 Node 155/155、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 专项证据与当前未提交工作树的 committed-target observation 仍缺。本轮只改私有 JS 验收器、测试和文档，未重跑无变化的全量 Rust 套件；未做 UI 自动化或部署测试。

收尾复核：全量 Node audit 1631/1631；三个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`。最终整体 review 尚未完成，下一单类别为 Kiro。

### Grok 三 operation 目录摘要精确 ID 全序（2026-10-09）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交对象没有新的 Grok wire 差异；后者六处未提交 Codex 文件仍排除，`--check-sources` 复核通过。目标私有 receipt validator 的 `localeCompare` 排序可把精确不同的 `café` 与 `cafe\u0301` ID 判为同序，导致仅目录行重排也改变摘要。inference、media、remote_compaction 各一个回环正例先红后绿；现用精确字符串全序排序目录行，原有全部 ID 唯一性、目录能力摘要及 receipt scope 保持。此处不是 xAI 模型命名事实，不改对象字段 canonical、生产 Grok HTTP/WS/media/compaction wire、receipt schema v1/revision 1、`0.2.111`、group header 或 remote compaction disabled 决策。

Grok 专项 Node 159/159、全量 audit Node 1896/1896、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 固定账号版本/header 接受性和当前未提交工作树的 committed-target observation 仍缺。离线 fixture 不构成厂商验收，本轮未重跑无变化的全量 Rust 套件，也未做 UI 自动化或部署测试。

## 2026-09-28 CodeBuddy cache-usage conservation freeze

CB-N6 冻结在 `assets/contract/codebuddy-reference-delta.json` 的 `CB-OBS-0016` 与 `incrementalEnhancementExtensions.CB-N6`。一次性只读证据取自 `cli2api@34c91b899aa2289f5ee49618d2d300f3eeaffedb`（tree `a0a1a3e6530e160284dba8eb18b85555ceab6a10`）及 `a9da609`、`fff3419` 的缓存 usage 历史对象；参考工作树中的 `proxy.html`、`proxy.md` 两个未跟踪文件继续全部排除。新增 snapshot 同时冻结 `f44e887` 的 incomplete terminal、`b415dd0` 的 namespace identity、`9f9b66e` 的 custom tool，以及 `aeaa4ac` 的 malformed arguments 宽松丢弃行为。默认审计完全自包含，显式 `node scripts/audit/audit-codebuddy-reference-delta.mjs --check-sources` 才复核上述 committed object、文件 SHA-256 和符号。

独立实现冻结在 `4f8c937a7ad19366d01f36bb994cb84959f129c4`（tree `dc5ad894db16e71fb5e48aa395edff6bd9e8a98e`）。CodeBuddy decoder 与非流 aggregator 会先把缓存读写归一到稳定顶层/详情字段，再进入共享转换与 usage 日志。读取优先级固定为显式顶层 `cache_read_tokens` / `cache_write_tokens`、协议 nested details、最后才是 vendor legacy alias；显式 `0` 是权威值，非法负数、小数和越界整数不参与优先级。Claude 输出把 inclusive prompt 拆成 fresh/read/write，Codex Responses 保留 inclusive `input_tokens` 与 details，Gemini 保留 inclusive prompt 和 cached count；三者的 total 都不把缓存量重复相加。CodeBuddy 的 Chat→Claude 流会持续增量输出工具参数、捕获独立 usage 尾帧，并只在唯一 `[DONE]` 与上游 EOF 均验证后提交成功 terminal，避免 finish_reason 先到时把 cache usage 固化为零或截断流先成功后报错。

Intl/CN × Claude/Codex/Gemini × stream/non-stream 的 loopback fixture 已统一校验 cache read/write、显式零、冲突优先级和总量守恒；内部 usage parser 使用相同优先级。参考的 incomplete terminal、namespace identity 与 custom tool 行为已经由共享 Responses bridge 的既有 fixture 覆盖，没有增加 CodeBuddy 私有分支。`aeaa4ac` 对已完成 malformed call/result 的静默整对丢弃明确不采纳：目标继续在任何 done/completed 前稳定 fail closed，不伪造 `{}`，也不隐藏损坏数据。

本地 CodeBuddy 聚焦回归为 70/70，Clippy 与 reference-delta 默认/committed-object 复核通过。CB-N6 只提升为 `fixture_verified`；Intl/CN 两站仍分别为 `receipt=null/live_pending`，CB-N4 继续 runtime disabled。此次修复不改变 Provider/Share/Account/site binding、attempt budget、计费权威或同账号 pre-commit 401 边界，也未引入账号池、轮换或跨身份 fallback。

## 2026-09-28 Codex Responses compatibility and observability freeze

### 2026-10-12 Codex subscription-expiry source and probe identity correction

只读复核 `codex2api@b1bfe1b4` 的 WHAM expiry 与 subscription sync：当前服务端 `/backend-api/wham/usage` 已可能直接返回 `subscription_expires_at`、`subscription_active_until` 或 `chatgpt_subscription_active_until`，而 `/backend-api/subscriptions` 对维护请求身份和出口挑战更敏感。Server 已将同一绑定 Account/workspace 的新鲜 WHAM 日期置于 `/subscriptions.active_until` 与 `/accounts/check.entitlement.expires_at` 之前，并复用现有 Codex maintenance identity；历史快照和已验证 token claim 只能作为同 workspace/plan 的 stale fallback，不能覆盖更新日期或授权路由。HTTP/网络/JSON/anti-bot/not-provided 保持分离，真实账号验收前仍为 `live_pending`。

CX-N5～CX-N12 冻结在 `assets/contract/codex-reference-delta.json` 的 `CX-OBS-0014`～`CX-OBS-0022`。一次性只读证据取自干净的 `CLIProxyAPI@acdace936fa7df2905500c7f5e0a97d683138dea`（tree `f7e64bd57e383e31be75a7077ef918f5154f0ab8`）与 `codex2api@9368ed82e039ac8458f9c264b2298b47d0cbf833`（tree `d878a5e7b8a94665dad14631d40be639d923a345`），并固定 octal NUL schema、cache breakpoint、私有 SSE event、连续工具轮 reasoning、item metadata、上游自报模型、全双工 steering 与 orphan required 的九个历史 committed delta。当前合同共有 18 个 source delta 和 22 条不可变 observation；`node scripts/audit/audit-codex-reference-delta.mjs --check-sources` 可复核 commit/tree、文件 SHA-256 和符号，默认构建、测试、发布与运行时不读取外部 checkout。

独立实现冻结在 `f8907742b163b239a981cddcc3962410f0712677`（tree `9e4bfc5d8fdfd882c74bc63d1f2ab3510bea5a33`）。schema sanitizer 只在 schema `pattern` 识别活动 `\0` 与奇偶反斜线形态，保留 `\x00`、文本 `\u005c0` 和用户数据；Responses、Compact、WebSocket 与 WS→HTTP fallback 共用精确 item/part sanitizer，只删除协议位置的 `internal_chat_message_metadata_passthrough`、`author`、`recipient` 和 `prompt_cache_breakpoint`，arguments 与普通嵌套 JSON 不递归清理。SSE 同时检查 declared event 和 payload `type`：普通客户端只看到公开闭集，经过识别的原生 Codex 客户端额外只允许 `codex.response.metadata`，错误和终态不会被误吞。

Responses→Chat 在同一连续工具轮中复用最近一次真实 reasoning，遇到 user/system 边界即清空，从不生成 `[reasoning unavailable]`。JSON、SSE、WebSocket 与 WS→HTTP fallback 会有界观察终态优先的上游自报 model，并把 model、来源、mismatch 与 conflict 写入本地 UsageLog；这些字段不覆盖 `actual_model`，不改变响应、路由、计费、retry/fallback，也不进入 Router Share 日志外部契约。WebSocket 只在真实终态提交该观察，断流和取消不把中间声明伪装成终态。

CX-N11 与 CX-N12 都先由差分 fixture 复现红灯，再做窄修复。Codex WebSocket writer 使用容量 8 的有界队列；大写入期间只允许顶层 `type=response.steer` 的 Text/Binary 帧原样入队，每条仍计入请求内存预算，queue full、第二个 `response.create`、嵌套伪装、坏 JSON 和非 Codex 数据帧立即 fail closed，Ping/Pong 与取消不被写入阻塞。strict response schema 同时覆盖 `/text/format/schema` 与 `/text/format/json_schema/schema`：composition branch 按可见 properties 裁剪 required，纯 object 无字段来源时删除 orphan required，`$ref/$dynamicRef` 保持 opaque。

Codex 关键词回归 399 项及新增 steering、reported-model 专项均已通过；reference-delta 默认审计与 committed-object 复核通过。CX-N5～CX-N12 均只提升为 `fixture_verified`。GPT Image 2.5 三个 variant、WS prewarm、Astra entitlement 和其他真实账号 operation 仍各自保持 `receipt=null/live_pending`；本轮没有引入账号池、跨 Account/Provider/rail/site fallback，也没有以本地 fixture 代替真实 ChatGPT 验收。

整体 review 在 `26eb389e5b0c8457f64bb70f60614ff5b1c1a140` 又关闭两处边界：缺少 declared event 与 payload type 的 JSON SSE 数据帧不属于公开闭集，默认过滤；composition 内任意嵌套 `$ref/$dynamicRef` 都可能提供当前节点的 required 字段，因此 schema repair 保守保留 required。`CODEX-R2-FINAL-2026-09-28` 以 committed target tree 和两条 fixture 冻结该修复；它是 CX-N7/CX-N12 的实现复核扩展，不是新增外部 source observation，也不改变五个真实 operation 的 `live_pending` 状态。

2026-10-08 子 agent 回放差分修正上一段 item metadata 结论：`input[*].type=agent_message` 的 `author` 是上游必填身份字段，无条件删除会得到 `Missing required parameter: input[N].author`。只读参考 `codex2api@b1bfe1b4` 的 `PrepareResponsesBody` 与原生 agent-context fixture 保留 `author` / `recipient`。Server 因此仅对该精确 item 类型保留两个字段，普通 message/tool item 继续清理；HTTP、Compact、原生 WebSocket 与 WS→HTTP fallback 共用同一 sanitizer，不引入账号、Provider 或路由变化。真实凭据复测仍待完成。

## 2026-10-08 Codex native arguments and client tool-schema differential

第三轮只读参考分别是 `CLIProxyAPI@5d8bb3d9`（tree `bfd6871b2e69151de407d17d56cda3439d8ca1bc`）的 `internal/translator/codex/openai/responses/codex_openai-responses_request.go::normalizeEmptyFunctionCallArguments`，以及 `@71a5f1f6`（tree `2ef57457a1c05e09879fcafa7e13b935ea86d7ca`）和 `@67cb32b6`（tree `48e1d16d4545d02793175314b228b650f50fd154`）的 `internal/client/codex/tool-schema/tool_schema.go`、`internal/runtime/executor/helps/payload_helpers.go`。目标起点为 `cc-switch-server@ba85f5fe611de0b8c8d3e9b2c320402cac47f071`（tree `583a799ab04fe133a74451e9c9fbfa0f9df82453`）；外部源码仅作此次只读研究证据。

CX-N13 先用 `codex_oauth_blank_function_arguments_are_exactly_normalized` 复现红灯：Codex OAuth 原生 Responses 历史 `input[*].type=function_call` 的空/全空白字符串 `arguments` 未转为字符串 `"{}"`。当前工作树只在该精确位置归一；非空坏 JSON、custom tool、消息正文与同名用户字段保持原样。`codex_oauth_blank_function_arguments_cover_http_compact_and_websocket` 另覆盖 HTTP、Compact、WS `response.create` 和 WS→HTTP fallback 的同一准备语义。CX-D3 的精确整数参数 fixture 也先复现红灯；独立修复仅对已识别的 Codex CLI/TUI 客户端、非 Codex 目标身份、参考列出的 `exec_command` / `write_stdin` 等工具及指定直接属性执行 `number`→`integer`，保留普通数值字段、嵌套字段、非官方客户端、Codex→Codex 和无变化请求的原始 body；namespace、`additional_tools`、Claude/Gemini 风格声明及类型数组去重有对照用例。该预适配步骤不改变 Provider/Account/Share 选择或请求恢复边界。

CX-D4 的首帧前 EOF loopback、首事件 deadline、Chat EOF 无 `finish_reason`、URL citation/search 与 tool round-trip 在本地逐项对照；首帧前 EOF 保持 502，Chat 截断为 failed 而非 completed，citation 与工具语义由既有共享 fixture 覆盖。没有据参考提交动态下载客户端版本，也没有重写 reported-model 审计。以上仅为工作树本地 fixture，尚无 implementation commit/tree 或真实 ChatGPT Account receipt；五个既有 Codex operation 与新增差分的真实接受性继续 `live_pending`。`assets/contract/codex-reference-delta.json` 的 committed-object 历史保持不变，待提交态才能另行追加目标对象 observation。

本轮验证为 Codex 关键词 406/406、全量 Rust library 3261 passed / 1 ignored，`cargo fmt --check`、`cargo check`、Clippy `--all-targets -D warnings`、Provider contract/coverage/UI matrix，以及 Codex reference-delta 默认与 committed-source 审计通过。未做 UI 自动化或部署测试。

## 2026-09-28 Grok catalog capability and identity-gate freeze

GR-N3 与 GR-N4 冻结在 `assets/contract/grok-reference-delta.json` 的 `GR-OBS-0012`～`GR-OBS-0013`。一次性只读证据取自干净的 `grok2api@5e5ad75556b61a2c4a8fcf344d83bfe7760f2b42`；实际评审的功能提交是 `9dda42ae41fbb3d7e1948aef5f1a4dab2d270b7e`，两者共享 tree `889cf18ab5363fe8d6d408b16c734625e1065d24`。两个追加式 source delta 分别固定目录能力和 Build 1.0.40/conversation-group 身份的八个 committed object；默认审计不读取外部 checkout，显式 `node scripts/audit/audit-grok-reference-delta.mjs --check-sources` 才复核 commit/tree、文件 SHA-256 与符号。`sub2api@ab99d56e` 的六项工作树修改仍全部排除。

GR-N3 的独立实现冻结在 `a0f567a60d6285cbd9d1e6b4b8396ee95582be3c`（tree `b469c3292585b28497490f969a9e55dd99abf0ef`）。`GrokModelCapability` 与 model ID 在同一个目录 snapshot 中原子替换，并精确绑定 App、Provider ID/revision/runtime fingerprint、Account、auth identity generation 与 token refresh generation。解析保留上游 reasoning menu 顺序并去重，只接受 `none|minimal|low|medium|high|xhigh|max`；default 必须属于过滤后的菜单，显式 `false`/`0` 与字段缺失保持不同语义。成功空目录权威清除旧能力，stale catalog 只可展示，不能向 HTTP、WebSocket 或 WS→HTTP fallback 扩大运行时权限。

目录 manifest v2 展示 reasoning menu/default、context window、max completion 与 backend search；这些字段不会自动开启搜索、remote compaction，也不会把上游 context/max 当作本地请求内存预算。fresh capability 下的 reasoning effort 只能来自当前菜单；显式 `supports_reasoning_effort=false` 只移除 effort，不误删独立 summary 控制。没有 fresh capability 时继续使用既有保守静态判断。实现没有复制参考项目的 process-global profile map，也没有采用缺少 xAI 权威价格的 Grok 4.7 临时计价推断。

GR-N4 保持 `live_pending` 且没有生产 wire 改动：Server 继续使用已评审的 `grok-shell/0.2.111` 身份，只发送隔离后的 `x-grok-conv-id`，不新增 `x-grok-conv-group-id`。参考项目自身采用 Build `1.0.40` 和稳定 UUIDv5 group id，不能证明同一固定账号会接受目标项目直接替换身份。只有 catalog 与 inference 的 current-commit 私密 receipt，或明确可复现的 version-rejected signal，才能触发后续常量/header 变更；一个 operation 的结果不能外推到 media、remote compaction、其他 Account/Provider/Share/rail。

Grok 关键词回归为 224/224，Clippy、格式、reference-delta 默认审计与 committed-object 复核均通过。合同现有 13 条 append-only observation；`GR-N3=fixture_verified` 只证明本地目录/运行时闭环，inference、media、remote compaction 与 GR-N4 身份/header 接受性仍分别保持 `receipt=null/live_pending`。

## 2026-10-08 Grok group-header injection boundary

本轮只读复核确认推荐的 `grok2api` 已提交 HEAD 仍为 `7c889a960e2638341b4dae9a5c81af0e0f38c87f`（tree `eec71f0920553af1a1cc67daec838399d364ea92`），相对先前冻结的 `5e5ad755` 仅 README 与图片变化；`sub2api` HEAD 仍为 `ab99d56e9626e6cd731592dae8553c9758a0efa2`，六项未提交文件全部排除。`grok2api@9dda42ae` 的 `backend/internal/infra/provider/cli/adapter.go` 确实为其 Build 1.0.40 发送 `x-grok-conv-group-id`，但仍不能证明目标的固定账号会接受该 header 或新版本。

目标起点 `cc-switch-server@ba85f5fe611de0b8c8d3e9b2c320402cac47f071` 的 `GrokForwardContract` 自身不产生 group header；差分检查发现 `account.extra_headers` 经共享 `apply_account_header_overrides` 可注入 `X-Grok-Conv-Group-Id`，绕开 GR-N4 的 disabled 决策。`grok_oauth_account_headers_cannot_override_cli_contract` 先复现红灯；当前工作树把该名称加入精确 Grok OAuth 受控头集合，并由 `grok_account_cannot_inject_unreviewed_conversation_group_header` 验证绑定账号注入在发网前返回 400，原有 `x-grok-conv-id` 不被替换。默认 CLI profile 另断言 group header 仍缺席。此修复不发送新 header，不改 `0.2.111`、目录、模型、请求体、WS 恢复或账号选择。

Grok 本地关键词 226/226，三项 operation 的 receipt Node 门禁 11/11，reference-delta 已提交源对象复核、Provider coverage/UI matrix 通过。`RUN_REAL=0` 的 inference、media、remote_compaction 均只返回 `blocked_inputs/live_pending`，没有真实请求。现有 inference receipt 的 `version_gate` 检查标志本身也不包含同账号 catalog/HTTP/WS 的具体版本和 header 接受/拒绝事实，不能单独关闭 GR-N4；还需 fresh、脱敏的固定 Account header-level receipt。三项 operation 与 GR-N4 仍各自 `receipt=null/live_pending`，remote compaction 仍 disabled。本轮未做 UI 自动化或部署测试，工作树尚无 implementation commit/tree。

并行出现的 Codex-only 提交 `ac69ecb1c652c0ac924394203617df3b57fa2498` 没有纳入 Grok 修复；在该最新 HEAD 与当前工作树组合上重跑全量 Rust library 为 3263 passed / 1 ignored，`cargo fmt --check`、`cargo check`、Clippy `--all-targets -D warnings` 和 Grok committed-source 审计均通过。该结果证明本地共享回归，不提升真实 Grok 验收。

### Grok 目录发布与账号实例栅栏接续

推荐参考的提交态边界仍为 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 和 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2`；前者本轮无新增 Grok wire，后者六项未提交 Codex 文件继续排除。以下是目标独立发现的并发缺口，不冒称参考实现提供了同一修复：旧 token 目录请求 A 挂起、新 token 请求 B 先发布后，A 晚返回会删除 B 的模型/能力缓存；同 ID 删除重建且 auth/token generation 相同时也无法区分新旧账号。回环 fixture 先复现上述红灯。

当前工作树在 `src/clients/oauth/grok_models.rs`、`src/api/providers.rs`、`src/state.rs` 将进程内 account incarnation 纳入精确目录作用域，并按账号实例、Provider revision、auth/token generation 和请求序号比较发布优先级；API 返回前再次核对账号实例及缓存 revision，过时请求稳定拒绝为 409。旧调用不能清除新 token 的能力，删除重建后即使 ID 和两代 generation 相同也不能借用旧目录；正常刷新不改变账号实例。此变更不改 Grok `0.2.111`、group header、模型能力解释、账号选择、推理 HTTP/WS wire 或 remote compaction 开关。

本地倒序 token、发布优先级及同 ID 重建 fixture 已转绿。Grok 关键词 230/230；完整 `cargo test --quiet` 为 library 3282 passed / 1 ignored，非 library 124 + 1 + 1 passed；Grok receipt Node 11/11、reference-delta `--check-sources`、Provider contract/coverage/UI matrix、`cargo fmt --check`、Clippy `--all-targets -D warnings`、`git diff --check` 均通过。Phase 0 清单已同步并通过自检，`src/api/providers.rs` SHA-256 为 `d26e0acdbd4cb68cc2bf8ad00e9c1322bb52502905db696985ac75b536a25a89`，`src/state.rs` 为 `e592816b06a232f8f63a97b528edfa83cc8344533480cad4106f7269b8c632d8`。`RUN_REAL=0` 下 inference、media、remote_compaction 均为 `blocked_inputs/live_pending`，未访问真实服务。此工作树尚无 implementation commit/tree，既有 v2 audit 只复核旧 committed observation；不得追加虚假的 committed-target observation。GR-N4 仍需同一固定账号的版本/header receipt，三个 operation 各自仍需真实 receipt；没有 UI 自动化或部署测试。

## 2026-09-28 Cursor OmniRoute no-wire refresh

Cursor 的第二轮 append-only review 从 `OmniRoute@02c663cdd0e8577bdcf2b01a44046bcd46dc6a7a`（tree `25b36e4993a8cc52da22b3d0b0aa26bd4c60426b`）比较到 `443d66996d69d7ecf887fd61158404a3cca1b192`（tree `8d1c312f61d115767c9478416cb039750eaf113a`）。区间内 14 个 committed path 只涉及依赖/lockfile、可选本地 embedding、测试运行器和共享 `streamTiming` 的 monotonic clock；六个已冻结的 Cursor protobuf、session、executor 与对应 fixture path 均无变化，没有 OAuth/API-key header、ServerConfig、AgentService protobuf/gzip、模型目录、工具桥、park/resume 或 terminal wire 增量。

`assets/contract/cursor-reference-delta.json` 新增第二个只读 snapshot 和不可变 `CUR-N2-2026-09-28` review extension，逐项固定 from/to commit/tree、14 个 changed path、六个 reviewed wire path 与 `changedCursorWirePaths=[]`。参考仓库当时 22 项未提交/未跟踪 Codex 内容全部排除；默认 audit 不读取外部 checkout，显式 `node scripts/audit/audit-cursor-reference-delta.mjs --check-sources` 才复核 Git range、tree、路径清单和历史文件 SHA-256。

结论保持 `reviewed_no_wire_delta`，因此不制造 Cursor 生产代码改动。既有 CUR-01/CUR-03、CORE-N1/N2 仍为 `fixture_verified`，OAuth 与 API-key 两条 CUR-N1 receipt 继续相互独立地保持 `null/live_pending`；共享 stream-timing 变化不能替代任何真实账号验收，也不能扩展 retry/fallback 边界。

## 2026-10-08 Cursor committed-source and dual-rail gate recheck

按 `Cursor/AGENTS.md` 的推荐，仅复核 OmniRoute 已提交对象：HEAD 仍为 `443d66996d69d7ecf887fd61158404a3cca1b192`，tree 仍为 `8d1c312f61d115767c9478416cb039750eaf113a`，相对前次冻结点没有新提交。参考工作树仍有 22 项 Codex 相关本地改动，全部排除；本仓库 `src/proxy/cursor/` 与 `src/proxy/providers/cursor/` 本轮也没有差分。`node scripts/audit/audit-cursor-reference-delta.mjs --check-sources` 复核六个 Cursor wire/test 对象、10 个 legacy 字段和 8 条不可变 observation 均通过，CUR-N2 继续是 `reviewed_no_wire_delta`，不据此改写 Cursor executor。

当前工作树的 Cursor Rust 关键词回归 304/304；`node --test scripts/audit/cursor-real.test.mjs` 为 21/21，分别覆盖 OAuth/API-key receipt 的 scope、Provider/Share/credential generation、22 项检查、body hash、测量、恢复决策、rail mismatch、伪造 live、过期与秘密脱敏。以 `RUN_REAL=0` 对两条 rail 分别运行 harness，仅得到 `blocked_inputs/live_pending`；缺服务地址、凭据、精确 Provider/Share/model 与仓库外私有 receipt，且本轮明确禁止部署测试。Provider coverage 与 UI matrix 静态审计通过。两条 CUR-N1 真实 rail 仍独立 `receipt=null/live_pending`；离线检查不能提升任一 rail 的订阅接受性。

同日 Cursor API Key 目录发布栅栏增量：推荐的 OmniRoute 提交态仍为 `443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a`，其 22 项未提交 Codex 工作树改动全部排除，故这不是参考新增 Cursor wire。目标 `src/api/mod.rs::append_cursor_api_key_models` 使用请求开始时的 Provider 快照等待模型目录；凭据轮换后，旧快照命中的旧目录仍会被发布。本地确定性 fixture 在旧快照和旧 credential-scope 缓存下轮换同一 Provider API Key，先复现旧模型进入结果，再于进入响应前核对当前 Provider 类型、revision、credential generation、driver、runtime fingerprint 与静态凭据代际；不再发布漂移的目录。旧缓存仍只以旧凭据作用域为键，不会授权新凭据；不改变 OAuth rail、Cursor protobuf、session/continuation 或跨账号路由。

修复后 Cursor 关键词 Rust 305/305、完整 `cargo test` library 3279 passed / 1 ignored，非 library 124 + 1 + 1 passed；Clippy `--all-targets -D warnings`、rustfmt、Cursor receipt Node 21/21、reference-delta `--check-sources`、Provider contract/coverage/UI matrix 均通过。`src/api/mod.rs` 完整源码 SHA-256 更新为 `25603df278d6682833374eab27f8444fed2964396a7abee7fdc282aea9521b8e`，Phase 0 compatibility 清单由生成脚本同步并通过 `--check`。`RUN_REAL=0` 两条 rail 均为 `blocked_inputs/live_pending`；当前代码仍未提交，不能追加 committed-target observation，也没有真实 Cursor 账号验收或部署/UI 测试。

### Cursor 双 rail receipt 绑定与 API Key 目录新鲜度接续（2026-10-08）

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 HEAD 未变；22 项未提交 Codex 工作树路径继续排除。本项是目标验收器独立发现的缺口，不冒称上游新增 Cursor wire。旧 `scripts/smoke/cursor-real.mjs` 在两条 rail 首次读取 Provider/Share、OAuth Account 后查模型并解析 receipt，接受前不再核对绑定；API Key rail 即使 `/v1/models` 返回静态来源、`stale=true` 或过旧/超前的时间戳，只要含所选 `*-fast` ID 也会过关。OAuth 的 Account token generation / Provider revision / Share revision 与 API Key 的 credential generation / Provider revision / Share revision 六个漂移反例，以及 API Key 四个目录反例，均先复现错误接受。

现在 receipt 校验后末次重读精确 Account（OAuth）、Provider、Share，比对 Account ID 与绑定中两代 OAuth generation 或 API Key credential generation、Provider revision/runtime fingerprint、Share revision；漂移即失败关闭。API Key 目录另要求 `source=cursor_public_api`、`stale=false`、安全整数 `fetchedAtMs` 在 5 分钟内且未来偏差不超过 1 分钟，时限对应 Server 现有 API Key 动态目录 TTL。OAuth 静态目录不套用 API Key 的来源要求。正常两 rail fixture 与上述负例合计 33/33。公共模型列表的来源/时间是展示级证据，仍不能单独证明某个 ID 必定来自当前上游动态目录或真实账号 entitlement；真实 receipt 的模型、请求体和结果仍须单独验收。末次控制面快照也不是跨 API 原子事务。

生产 Cursor protobuf/session/目录发布逻辑与旧 receipt schema v2/revision 2 均未改。Cursor reference-delta 的已提交源对象 `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过；全量 Node audit 230/230。OAuth/API Key 两条 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，真实 rail 仍 `receipt=null/live_pending`，新增验收逻辑在未提交工作树，不能伪写 committed-target observation；未做 UI 自动化或部署测试。

### Cursor 双 rail 私有 receipt 控制面响应大小栅栏（2026-10-08）

推荐 OmniRoute 的已提交 HEAD/tree 仍为 `443d66996d69d7ecf887fd61158404a3cca1b192` / `8d1c312f61d115767c9478416cb039750eaf113a`，22 项未提交 Codex 改动不作 Cursor 证据。本项仅修本仓库 `cursor-real.mjs` 的私有验收读取边界：旧逻辑在 4 MiB 检查前等待完整响应。OAuth Account 的超限声明长度、API Key Provider 的超限无长度分块且连接保持不结束，两条回环反例均先红灯（约 5 秒超时）；现在读取前检查声明长度，读取中累计实际字节并在超限时中止，均快速、脱敏地失败关闭。Cursor 专项 Node 36/36、全量 audit Node 337/337、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix 通过。

生产 Cursor protobuf/session/目录发布 wire、两 rail 身份和 receipt schema v2/revision 2 均未改变。OAuth/API Key 两个 `RUN_REAL=0` 入口各自仍为 `blocked_inputs/live_pending`，没有真实请求或 receipt；当前 JS 改动未提交，不追加伪造的 committed-target observation。此加固不能证明账号 entitlement，也不提供跨控制面 API 的原子快照；未做 UI 自动化或部署测试。

### Cursor 双 rail 私有 receipt 静态硬链接边界（2026-10-08）

OmniRoute 已提交 HEAD/tree 仍为 `443d66996d69d7ecf887fd61158404a3cca1b192` / `8d1c312f61d115767c9478416cb039750eaf113a`；22 项未提交 Codex 工作树改动继续排除。本项只修本仓库私有证据文件约束：旧 `cursor-real.mjs` 的仓库外路径和真实路径检查挡住符号链接，却会接受与仓库内文件同 inode 的外部硬链接。OAuth/API Key 两条合法 fixture 均先复现错误接受；现要求 receipt 文件只有一个硬链接，两例在控制面请求前失败关闭。Cursor 专项 Node 39/39、全量 audit Node 361/361、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

该静态检查不证明并发文件替换的原子性，也不能证明部署服务的 commit 与本地 HEAD 一致。生产 Cursor wire、rail 身份和 receipt schema v2/revision 2 不变；两条 `RUN_REAL=0` 入口仍各为 `blocked_inputs/live_pending`，无真实请求或 receipt。当前 JS 改动未提交，不追加假的 committed-target observation；未做 UI 自动化或部署测试。

### Cursor 双 rail 私有 receipt 请求期间替换栅栏（2026-10-08）

OmniRoute 已提交 HEAD/tree 仍为 `443d66996d69d7ecf887fd61158404a3cca1b192` / `8d1c312f61d115767c9478416cb039750eaf113a`；22 项未提交 Codex 工作树改动继续排除。本项只修目标验收器的证据读取窗口：启动时的路径、权限和硬链接检查不能阻止控制面请求期间同路径文件被替换。OAuth 在首个 Account 请求、API Key 在首个 Provider 请求将无效 receipt 原子替换为有效文件，旧实现两例均错误接受。现于请求前打开只读描述符，核对文件类型、dev/inode、链接数、大小和权限，限量读取最多 1 MiB 并固定字节快照；后续不再按路径重读。两例脱敏失败，正常 fixture 保持通过；Cursor 专项 Node 42/42、全量 audit Node 399/399、smoke Node 13/13，reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

此快照拒绝控制面请求期间的路径替换影响验收，不承诺读取瞬间的并发原位改写完全原子，也不证明部署服务运行本地 HEAD。生产 Cursor protobuf/session/目录发布 wire、OAuth/API Key 身份及 receipt schema v2/revision 2 未变；两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前 JS 加固未提交，不追加假的 committed-target observation；未做 UI 自动化或部署测试。

### Cursor OAuth 与 API Key 目录权威区分（2026-10-08）

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 的已提交对象未变，22 项未提交内容排除。目标 `src/api/models.rs::openai_model_list` 对 `CursorOAuth` 发布 Provider 模型配置和内置别名，`src/proxy/cursor/model.rs::resolve_cursor_model_with_catalog` 可将 `*-fast` 解析为请求 selector；这两者都不是绑定 OAuth Account 的动态 entitlement 目录。`CursorApiKey` 的静态列表则被清空，`src/api/mod.rs::append_cursor_api_key_models` 仅在精确凭据作用域的 public API 目录可用时追加模型，并返回 source/stale/fetchedAtMs。故双 rail receipt 只对 API Key 要求 `cursor_public_api`、非 stale 和时间窗口；OAuth 目录命中只能作为请求路由前提，真实账号接受性仍须同 Account/Provider/Share 的私有 receipt。

本轮未发现新增 committed Cursor wire 或可复现生产缺口，不改生产实现或 receipt schema。Cursor Node 42/42，reference-delta `--check-sources` 通过；OAuth 与 API Key 分别 `RUN_REAL=0` 得到 `blocked_inputs/live_pending`。环境未提供服务、凭据、精确绑定及仓库外真实 receipt，两 rail 继续分别 `receipt=null/live_pending`；不以静态目录、fixture 或参考提交态冒充真实授权和新工作树的 committed-target observation，未做 UI 自动化或部署测试。

### Cursor OAuth Account 布尔投影完整性复核（2026-10-09）

推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 已提交状态未变；其未提交 Codex 路径不作 Cursor 证据。本项只核对目标 Server 自身 `AccountPublicView` 与私有验收器，不推导新的 Cursor wire。真实 `/api/accounts` 投影必带布尔字段 `hasApiKey` 和 `needsRelogin`；旧 OAuth validator 只拒绝字段为 `true`，两个字段分别缺失时均错误接受 `contract_verified/live_pending`。现要求两者明确为 `false`，两例先红后绿，均在 Provider 与模型目录请求前脱敏失败，正常 OAuth 与 API Key fixture 保持通过。

这只收紧 OAuth Account 证据完整性，不改变生产 protobuf/session/目录 wire、两 rail 身份或 receipt schema v2/revision 2；离线 fixture 不证明真实账号接受性，跨 API 复核也不是原子快照。Cursor 专项 Node 82/82、全量 audit Node 1201/1201、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt 与当前未提交工作树的 committed-target observation 仍缺，OAuth/API Key 独立保持 `live_pending`；未做 UI 自动化或部署测试。

### Cursor API Key 动态目录接受前再验证（2026-10-09）

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` / tree `8d1c312f61d115767c9478416cb039750eaf113a` 的提交态未变；22 项未提交 Codex 工作树内容继续排除，CUR-N2 的 `reviewed_no_wire_delta` 不变。本项依据目标自身 API Key 动态目录证据边界修私有 receipt validator，不引入新 Cursor protobuf/session wire。旧流程只在初次目录读取时要求所选完整 `*-fast` 模型恰有一项、`source=cursor_public_api`、`stale=false` 和五分钟新鲜度；receipt 检查和末次 Provider/Share 复核之后只重算原时间戳。API Key 回环服务在第一次目录返回有效结果，第二次分别撤下该模型、标记 stale、或把来源改为静态 registry；旧 validator 三例都只发一次目录请求，错误输出 `contract_verified/live_pending`。

现在只对 API Key rail 在末次绑定复核后，经同一签名 Share 和预期 Provider 栅栏重新请求目录，再验证精确模型唯一、动态来源、非 stale 与新鲜时间；初次与末次时间戳均在共同接受时刻有效。三例先红后绿，正常 API Key fixture 改为两次目录请求，OAuth 静态别名 fixture 仍只请求一次。Cursor 专项 Node 86/86、全量 audit Node 1359/1359、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。生产 Cursor wire、目录发布、OAuth/API Key 身份与 receipt schema v2/revision 2 不变；多次请求不是跨控制面/上游原子快照，也不证明真实厂商 entitlement。OAuth/API Key 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，两份真实 receipt 和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

## 2026-09-28 Kiro no-wire refresh

推荐参考仓库 `kiro.rs` 的第二轮 HEAD 仍为首轮冻结的 `be0c04219d9d1b93b7fe5c3d7b9e7c9cf0d05863`，tree 仍为 `5e656c1bf70aac0224a68251b2065966db01e933`，工作树干净。首轮与第二轮 snapshot 的 committed range 因而为空；模型/凭据、Builder ID/IdC/Social、profile ARN、CLI/IDE endpoint、prompt-cache、remote-compaction、session affinity 和 EventStream decoder/frame 等 15 个冻结对象均无新提交可吸收。

`assets/contract/kiro-reference-delta.json` 追加 `kiro-rs-2026-09-28` snapshot 与不可变 `KIRO-NO-WIRE-2026-09-28` review extension，固定相同的 from/to commit/tree、完整 15-path reviewed set、`changedCommittedPaths=[]`、`changedKiroWirePaths=[]` 和 `productionCodeChanged=false`。默认审计只验证本仓库冻结摘要，显式 `node scripts/audit/audit-kiro-reference-delta.mjs --check-sources` 才以只读 Git object 复核 tree、文件 SHA-256 与空提交区间；外部仓库仍不进入构建、测试、发布或运行时。

结论为 `reviewed_no_wire_delta`，不创建 Kiro 生产代码提交。KI-N3 remote compaction 和 KI-05 shared cache 继续 runtime disabled；Builder ID、IdC、Social、API Key × `us-east-1`、`eu-central-1` 的八份私密 receipt 继续各自为 `null/live_pending`。参考仓库没有变化不能替代固定 Account、auth kind、region、Provider/Share generation 的真实验收，也不能扩大同账号 pre-commit 恢复边界。

## 2026-10-08 Claude `pause_turn` Responses terminal extension

第三轮只读参考为 `CLIProxyAPI@e6f6f26ad5563e7dd597a3aa889d67261cb6a509`（tree `68e882b737c88f7561122f3121cee6110fb6df72`）的 `internal/translator/claude/openai/responses/claude_openai-responses_response.go::claudeResponsesIncompleteDetails` 和对应测试。参考把 Anthropic `stop_reason=pause_turn` 识别为未完成的 server-tool 迭代，而不是 token 上限。Server 基线的非流 `anthropic_responses_incomplete_reason` 与流式 `AnthropicResponsesState` 均把它误报为 Responses `completed`；两条定点测试先复现红灯，再共用 `anthropic_responses_incomplete_details` 输出 `response.incomplete`、`status=incomplete`、显式 `incomplete_details=null`。`max_tokens`、context-window 与 refusal 的原有 reason、Claude 原生 stop reason 和 Chat finish reason 不被重新解释。

本地验证：`anthropic_pause_turn` 2/2、`anthropic_` 128/128、`claude` 270/270、`cargo check` 与 rustfmt 均通过。实现尚在工作树，未冻结 implementation commit/tree，也没有真实 Anthropic receipt；因此本项只证明本地协议 fixture，不提升任何 Claude operation 的 `live_pending`。CL-N7 的 thread continuation alias 状态仍需单独的精确身份/Share 作用域与成功终态差分，不由此项推断完成。

## 2026-10-08 Claude thread continuation tool alias state

一次性只读差分核对 `CLIProxyAPI@678a92578ca346ff9e77d4811cbd41f899a8cc96`（tree `8e19836aea61f2f90f42e3aa5b94e99cc908dbf3`）的 `claude_executor_tool_state.go`（blob `c36f615a50b2193759599690950f0b7a372d76c6`）、`claude_executor_execute.go`（`3ee64fa61779578c3197357e0424133be2a3223a`）和 `claude_executor_stream.go`（`f8b220390e79cea66a15896a1f0877d95f8f6966`）。参考在 `thread.type=continue` 且未重发 `tools` 时按 `previous_message_id` 恢复别名，缺失状态返回 404，成功响应后按新 message ID 继承。Server 原有 `normalize_claude_oauth_tool_names` 只建立请求级声明映射，确认该差分。

继续只读核对 `CLIProxyAPI@a2976eb8a303f11b4ea5177bce9f9ff752634dfc`（修复 executor 拷贝丢失缓存）和 `@e7b22260ddce1dc4e39610d82b0503a5d445fec6`（missing-thread 错误同时支持 plain text/结构化 JSON）。Server 将缓存置于共享 `ServerStateInner`，不依赖逐请求 executor 副本；现有 Anthropic error envelope 已把 404 映射为 `type=error` / `error.type=not_found_error`，不复制参考的响应序列化代码。

Server 独立实现的状态只在精确 `claude_oauth` / Claude Messages thread 请求上启用，键同时含 Share、规范化用户、显式 session、Provider ID/revision/runtime fingerprint 与 Account ID/auth-identity generation；不使用首条用户文本派生的 session。进程内缓存限制 10 分钟、1024 条、8 MiB 总别名数据、64 KiB 单条和 512 字节 message ID；账号或 Provider 代际变化、跨用户/Share/session、过期或淘汰都不能命中。省略或清空 `tools` 的 continuation 在发网前恢复历史 tool name 与下游响应名；缺状态明确 404，不换账号、不重选 Provider。只有经 Anthropic JSON 成功语义检查或 SSE `message_stop` 成功终态确认后才按新 ID 写入，HTTP 200 error、流错误、终态前取消和缺终态不提交。

本地 loopback fixture 覆盖非流创建→续接/响应恢复、错 session 发网前拒绝、流式 `message_stop` 后续接、HTTP 200 error 不污染缓存，以及缓存过期、容量淘汰与 Provider/Account 代际隔离。这里仍没有真实 Claude OAuth thread create/continue receipt；上游是否接受精确 thread beta/shape、无显式 session 的兼容性与跨进程续接保持 `live_pending`，不能把 mock 成功外推为订阅账号验收。外部源码不进入构建、测试、运行或日常同步。

同轮只读差分还核对了 `CLIProxyAPI@26e5efbbdf6739eafa782cb98225e3fc2fbea99c` 的线程 cache breakpoint 预留与 `@958eeca719c84338fa6ddbd83f52dbb7335d95de` 的 `tool_addition` / `tool_removal` 内名称映射。Server 基线的缓存上限恒为四、工具别名只走顶层 tool use/reference，因此分别新增“有非 null `thread` 时最多三个显式断点”和精确 `tool.type=tool_reference`、custom `tool_definition` 名称映射；server tool、未知声明、普通文本与工具参数保持原样。`@39632e52a6e07a52b909d45fca3a53aca4f28fc4` / `@eca1d113859efe75984773b9cb69dca45476259d` 的 leading-user run/single terminal user 是 cloaking 路径修复，Server 当前第三方 caller system 的迁移语义不同；仅冻结本地输入顺序 fixture，不因参考实现文字或 mock 结果改写此策略。其真实接受性仍待 receipt。

本轮静态/离线验证：Claude 关键词 279/279、Anthropic 关键词 129/129、`cargo check`、`cargo clippy --all-targets -- -D warnings`、rustfmt、Provider coverage/UI matrix、Claude wire profile 及 reference-delta 默认和已提交 source object 审计均通过。工作树实现尚未冻结为目标 committed object；上述检查不替代真实订阅 receipt。

共享 Anthropic 流检查器改动另经 64 MiB 测试栈的 Rust `cargo test --lib` 全量回归：3256 passed、1 ignored、0 failed。Claude 本地差分可转入下一类别，但五个真实 operation 和本轮 thread/beta 接受性仍保持各自 `live_pending`。

同日 Claude 续线程增量复核：以同一只读 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `internal/runtime/executor/claude_executor_tool_state.go::prepareClaudeOAuthToolNamesForRequest` 为差分锚点，发现参考和目标都仅在续接请求没有工具声明时读取历史别名。目标精确三段 loopback 先复现：`create` 声明非法名称 A，`continue` 新增非法名称 B 且历史仍引用 A，再无声明续接；第二段的旧别名响应无法回映，历史 A 也未恢复。Server 在相同 Share/用户/显式 session/Provider revision+runtime/Account 身份代际与 `previous_message_id` 下，即使续接带新声明也可选读取历史映射；新旧 wire 或原名大小写冲突及并集超过 128 条均在发网前拒绝。合并后的映射用于历史请求、当前响应与成功终态的新 message ID；无历史状态但携带完整声明的请求仍保持原兼容行为，无声明且缺状态仍为 404。修复不扩大跨账号/跨线程权限，也不据参考的账号池选择行为改变路由。十项 thread-alias 定点测试和 Claude 关键词 282/282 已通过；真实 thread/tool-change 接受性及五个 operation receipt 仍 `live_pending`，当前工作树未提交，不能追加 committed-target observation。

收尾复核：完整 `cargo test` 为 library 3278 passed / 1 ignored，另有非 library 124 + 1 + 1 passed；Claude 私有 receipt harness 的离线 Node 测试 13/13、Clippy `--all-targets -D warnings`、rustfmt、Phase 0 / Server Provider contract / coverage / UI matrix、Claude wire profile 与 reference-delta `--check-sources` 均通过。`RUN_REAL=0` 对 `oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 五个入口分别返回 `blocked_inputs/live_pending`，未触发真实网络；缺失的是服务端、Router、精确账号/Provider/Share/模型与仓库外私有 receipt 输入。离线 fixture 不提升任何 live 状态，也不代替提交态 v2 observation。

### Claude 私有 receipt 末次身份栅栏接续（2026-10-08）

推荐参考 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 未变；本项是目标验收器的独立缺口，不声称参考项目提供相同修复。`scripts/smoke/claude-real-receipt.mjs` 原先只在开始读取一次 Account、Provider 和 Share，随后执行模型/可选套餐检查并解析 receipt；若期间 token generation、Provider revision 或 Share revision 变化，旧 scope receipt 仍会被接受。五个 operation × 三类漂移的 15 个 loopback 反例均先复现错误接受。

现在 receipt 内容验证后，验收器再读取同一 Account、Provider、Share，并比较 Account ID、auth/token generation、Provider revision/runtime fingerprint 与 Share revision。漂移即失败关闭；五个正常 fixture 各控制面资源读取两次，专项测试 29/29、全量 Node audit 202/202。receipt schema v1/revision 1、已有 plan projection、生产 Claude OAuth wire 和恢复策略均不变。末次快照不是跨三个 API 的原子事务，也不证明之后不会再变化。Claude reference-delta 的 15 项冻结源码差分、17 条旧 observation 及五个 `live_pending` operation 经 `--check-sources` 复核；wire profile、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`；真实 receipt 仍未提供，工作树未提交，不能新增 committed-target observation 或提升厂商接受性；未做 UI 自动化或部署测试。

### Claude 私有 receipt 真实路径复核（2026-10-08）

本项仅修目标验收器的私有文件边界，不从 `CLIProxyAPI` 推导新 Claude wire。仓库外符号链接指向仓库内合法 `oauth_inference` receipt 时，旧 `claude-real-receipt.mjs` 按字面路径检查仍给出 `contract_verified`；该回环反例先红灯。现在真实路径解析后再次拒绝仓库内目标，后续读取也使用解析结果，故在任何控制面/模型请求前失败关闭。五个 operation 的正常 fixture 与既有身份漂移反例维持通过，Claude 专项 Node 30/30、全量 audit Node 331/331、smoke Node 13/13。

Claude reference-delta `--check-sources`、Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法与差分空白检查通过；已核对生成的 `docs/provider/coverage.md`，此验收路径约束不改变覆盖矩阵。生产 CL-N7/CL-N8、receipt schema v1/revision 1 与五个 operation 的真实 `live_pending` 均不变。`RUN_REAL=0` 五个入口仍为 `blocked_inputs/live_pending`，没有真实 receipt；工作树未提交，不追加伪造的 committed-target observation。未执行 UI 自动化或部署测试。

### Claude 五项私有 receipt 控制面响应大小栅栏（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` / tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` 的提交态未变；本项是本仓库验收器的资源边界，不是新增 Anthropic wire。旧 `claude-real-receipt.mjs` 完整读取控制面响应后才检查 4 MiB；`oauth_inference` Account 的声明长度超限、`opus_5_5` Provider 的无长度分块实际超限且连接不结束，两条 loopback 反例均先红灯（约 5 秒超时）。现先查 `Content-Length`，读取时逐块累计实际字节并在超限时中止，两例均快速、脱敏地失败关闭。Claude 专项 Node 33/33、全量 audit Node 357/357、smoke Node 13/13；reference-delta `--check-sources`、离线 Claude wire profile、Server Provider contract/Phase 0/coverage/UI matrix 通过。

生产 Claude OAuth wire、CL-N7/CL-N8、plan projection 与 receipt schema v1/revision 1 均不变。五个 operation 的 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；当前 JS 加固未提交，不追加假的 committed-target observation。未做 UI 自动化或部署测试。

### Claude 五项私有 receipt 静态硬链接边界（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` / tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` 的已提交状态不变。本项只修本仓库验收器的证据文件位置约束：`realpath` 已拒绝仓库外符号链接指向仓库内，但仓库内外路径共用 inode 的硬链接仍能通过。`oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1`、`opus_5_5` 五个合法 fixture 各自先复现错误接受；现于任何控制面请求前要求 receipt `nlink=1`，五例均以固定脱敏错误失败关闭。正常 operation fixture、plan projection、绑定漂移与控制面响应大小用例保持通过；Claude 专项 Node 39/39、全量 audit Node 390/390、smoke Node 13/13，reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix 通过。

静态硬链接检查不提供文件并发替换的原子防护，也不能证明部署代码与本地 HEAD 一致。生产 Claude OAuth wire、CL-N7/CL-N8、plan projection 及 receipt schema v1/revision 1 未变。五个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；当前 JS 加固未提交，不追加假的 committed-target observation。未做 UI 自动化或部署测试。

### Claude 五项私有 receipt 请求前限量快照（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` / tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` 的已提交状态未变。本项只修目标验收器的证据读取窗口，不推导新 Anthropic wire。旧流程虽检查 receipt 仓库外真实路径、权限和单链接，却在 Account/Provider/Share、模型与 plan 请求后按路径读取，且无 receipt 字节上限。`oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1`、`opus_5_5` 五项各自在首个 Account 请求将无效文件原子替换成有效文件，旧流程全部错误接受；五份超过 1 MiB 的文件也先触发控制面请求。现于请求前从只读描述符取得最多 1 MiB 快照，核对文件类型、dev/inode、链接数、大小和权限，后续只解析该快照。十个反例均失败关闭，正常五项、静态硬链接、plan projection、绑定漂移及响应大小用例保持通过，Claude 专项 Node 51/51。

生产 Claude OAuth wire、CL-N7/CL-N8、plan projection 与 receipt schema v1/revision 1 均未改；快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 437/437、smoke Node 13/13；Claude reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，无真实请求或 receipt；续线程及 `pause_turn` 的厂商接受性仍待独立证据。新增 JS 未提交，不能以旧 committed-target observation 冒充本次实现证据；未做 UI 自动化或部署测试。

2026-10-08 的 Claude 五项私有 receipt 快照补强只处理本仓库验收器，不引入 Anthropic wire。每项各在首次 `readSync` 返回有效字节后、EOF 检查前，把同 inode receipt 等长原位覆盖并改变 mtime；旧流程五例都继续用旧字节输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后核对 dev/inode、nlink、size、mode、纳秒 mtime/ctime，五例均在控制面请求前脱敏失败。元数据复原与精确并发竞态不由此证明；CL-N7/CL-N8、receipt schema v1/revision 1 及五份真实 operation 状态不变。

2026-10-08 的最终文件身份复核仍只修本仓库 Claude 私有 validator，不新增参考项目或 Anthropic wire 事实。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各在首次 Account 读取时把已快照的有效 receipt 路径替换为另一 inode 的无效文件，或等长原位覆盖原 inode 并改变 mtime；旧流程十例全部错误接受内存中的旧 receipt。现保留读取后的 bigint 文件指纹，在最终 Account/Provider/Share、receipt/plan 时效复核之后重新解析配置路径，比较 dev/inode、链接数、大小、权限与纳秒级 mtime/ctime；十例均脱敏失败，正常五项保持通过。末次检查并非跨 API/文件系统原子事务，元数据复原和检查后的变化仍可能逃过此门槛；fixture 不是厂商接受性，五份真实 receipt 仍为 `live_pending`。Claude 专项 Node 99/99、全量 audit Node 641/641、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；新增 JS 尚未提交，不以旧 committed-target observation 冒充新逻辑。未做 UI 自动化或部署测试。

2026-10-09 的 Claude Share 投影复核只收紧本仓库五项私有 operation validator，不新增 Anthropic wire 事实。生产 `Share` 的 `bindings` 已包含 `share.app` 主条目，导入不变量要求每 App 至多一条且主字段一致；旧验收器拼接主字段后用 `some` 找正确 Claude 绑定，且不检查 enabled/status。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各构造重复冲突绑定、主字段冲突、disabled 与 paused 四种反例，旧流程二十例全部错误输出 `contract_verified/live_pending`。现直接要求原始 `bindings` 中恰好一条 Claude 绑定、主字段与其唯一条目一致，并要求 `enabled=true/status=active`；二十例先红后绿，正常五项按真实 Share 形状通过，错误均在目录发网前拒绝。Claude 专项 Node 126/126、全量 audit Node 770/770、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt、thread/beta 接受性及当前工作树提交态 observation 仍缺。生产 Claude OAuth wire、CL-N7/CL-N8、plan projection、receipt schema v1/revision 1 不变；控制面复核不是跨 API 原子事务，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

2026-10-08 的五项 receipt 最终内容复核仍只涉及私有 validator。五个 operation 各在首个 Account 请求时等长改写已快照的 receipt，再用对抗 fixture 让末次元数据读取返回原指纹；旧流程五例均错误输出 `contract_verified/live_pending`。现在末次以 `O_NOFOLLOW` 描述符复核完整原始字节及 EOF，并复核读前读后指纹和配置路径，五例均脱敏失败。fixture 的元数据伪装不证明普通用户能恢复真实 `ctime`，末次检查后的并发变化或跨 API 原子性也未获证明；CL-N7/CL-N8、生产 Anthropic wire、receipt schema v1/revision 1 与五份真实 receipt 状态不变。

2026-10-09 的 Claude 五 operation 控制面作用域复核只修本仓库私有 validator，不新增 CLIProxyAPI 或 Anthropic wire 事实。旧验收器对缺失的 Provider/Share revision 回退为 `0`，并把仅含空格的 runtime fingerprint 视为非空；receipt 若按此不完整作用域自洽生成，`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各有三类反例错误输出 `contract_verified/live_pending`。现在要求控制面直接提供非负安全整数 `providerRevision`、`configRevision` 和非空白字符串 `runtimeFingerprint`；十五例先红后绿并在目录请求前拒绝，正常五项与既有 plan、Share、receipt 文件/时效及末次漂移 fixture 保持通过。Claude 专项 Node 142/142、全量 audit Node 917/917、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。五个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性与当前工作树提交态 observation 仍缺。生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、plan projection 和 receipt schema v1/revision 1 不变；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

2026-10-09 的 Claude 五 operation 控制面真实投影复核仍只修本仓库私有 receipt validator。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 未变，本项不推导新 Anthropic wire。目标 Server `ProviderView` 为顶层 `revision` 与 runtime `providerRevision`，旧 Claude mock 却使用不存在的顶层 `providerRevision`；mock 改为真实形状后，五个正常 operation 先全部失败。旧 validator 虽已拒绝缺失 revision、缺 Share revision 与空白 fingerprint，仍只读取虚构字段。五项各构造仅有旧式顶层字段、缺真实顶层 revision、缺 runtime revision、两份不一致四类自洽反例，旧流程二十例错误输出 `contract_verified/live_pending`。现要求两份真实 Provider revision 均为非负安全整数且相等；五项正常 fixture 恢复通过，二十例在模型目录发网前脱敏拒绝，既有 plan、Share、receipt 文件/时效与漂移 fixture 保持通过。Claude 专项 Node 163/163、全量 audit Node 1177/1177、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 厂商接受性与当前工作树提交态 observation 仍缺。生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、plan projection 与 receipt schema v1/revision 1 不变；控制面复核不是原子快照，fixture 不证明真实账号接受性。未做 UI 自动化或部署测试。

### Claude 五 operation OAuth Account 布尔投影完整性复核（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的已提交 HEAD 不变；本项依据目标 Server 自身 `AccountPublicView` 合同加固私有 receipt validator，不推导新 Anthropic wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`，旧五项 validator 只拒绝值为 `true`。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 各把两个字段分别设为缺失、`null`、字符串 `"false"`，共三十个 loopback 反例旧流程错误输出 `contract_verified/live_pending`。现在要求两个字段均明确为布尔 `false`，三十例在 Provider 与模型目录请求前脱敏失败；正常五项和 Opus access-only 正例保持通过。

本项只提高私有验收证据完整性，不改变生产 Claude OAuth wire、Share 路由、CL-N7/CL-N8、canonical plan 或 receipt schema v1/revision 1；控制面复核不是跨 API 原子快照，fixture 不证明 Anthropic 厂商接受性。Claude 专项 Node 194/194、全量 audit Node 1324/1324、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt、续线程/beta 接受性与当前未提交工作树的 committed-target observation 仍缺。未做 UI 自动化或部署测试。

### Claude 恢复/套餐 receipt 的凭据能力门槛（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `internal/auth/claude/anthropic_auth.go` 使用独立 refresh token 发起刷新；本仓库生产模型更明确区分 `refreshable_oauth` 与 `access_only_setup_token`，后者不进入 token refresh。目标 `oauth_inference` receipt 要求同账号 401 恢复，Max 5x/20x、Fable 5.1 receipt 要求 fresh canonical plan，但旧验收器五项共用的 Account 检查只要求 access token。四个 refresh/plan 相关 operation × 初始 access-only、缺能力投影、缺 refresh token、末次能力漂移、末次 refresh 凭据丢失共 20 个 loopback 反例先错误接受；Opus 5.5 的 access-only 正例也先确认可接受。现在四项在首末 Account 快照都要求 `credentialCapability=refreshable_oauth` 与 `hasRefreshToken=true`，旧服务缺能力字段失败关闭；Opus 5.5 保持独立 access-only 路径。

此项只验证账号当前凭据拓扑，不证明 refresh token 可用、真实 401 replay、套餐权益或 CL-N7/CL-N8 厂商接受性；生产 Claude wire、plan projection、receipt schema v1/revision 1 均不变。Claude receipt Node 73/73、全量 audit Node 485/485、smoke Node 13/13；Claude reference-delta `--check-sources`、离线 wire profile、Server Provider contract/coverage/UI matrix、文档索引、脚本语法、rustfmt、`cargo check --lib` 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，五项真实 receipt 仍分别 `null/live_pending`；新增 JS 未提交，不能冒充 committed-target observation。未做 UI 自动化或部署测试。

### Claude 五 operation 模型目录签名绑定（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` / tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525` 的提交态未变；本项不引入新的 Anthropic wire。目标 Server `/v1/models` 按 Router 签名 Share 与 App 选择 Provider，而 Claude 私有 receipt validator 原只用普通 `app/providerId` 查询，并不能证明目录属于 receipt 的 Share。`oauth_inference`、Max 5x、Max 20x、Fable 5.1、Opus 5.5 五项正常回环 fixture 对单次目录请求新增预期 Share/Provider 断言后全部先红；现验收器发送 `expectedShareId/expectedProviderId`，复用已测试的签名 ingress 匹配/冲突栅栏，五项均转绿。Claude 专项 Node 73/73。

2026-10-08 的 Claude 私有证据时效复核只修本仓库 validator，不推导新的 Anthropic wire。五项 operation 各让 receipt `recordedAt` 在内容校验时距 24 小时上限 1.2 秒，Max 5x/20x 和 Fable 5.1 另各让 canonical plan `observedAt` 距 15 分钟上限 1.2 秒；末次 Share 响应延迟 2 秒后，旧流程八例均在证据过期后错误接受。现初次与末次接受共用时间谓词，并以同一接受时刻分别核对 receipt 和适用 operation 的 plan；八例均失败关闭。Opus 5.5 的 access-only 正路不因此要求 plan。生产 Claude OAuth wire、receipt schema v1/revision 1、CL-N7/CL-N8 和五份独立 `live_pending` 状态均不变；末次读取仍不是跨 API 原子事务。

此项只收紧目录路由证据，不改变 Claude OAuth wire、CL-N7/CL-N8、plan projection 或 receipt schema v1/revision 1。静态目录中的模型存在不证明订阅权益、真实 thread/beta 接受性或跨 API 原子性；五份真实 operation receipt 仍缺。新增 JS/文档未提交，不能用旧 committed-target observation 冒充本轮实现证据；未做 UI 自动化或部署测试。

## 2026-10-08 Codex third-round local recheck


Codex 推荐的两份只读源码仍为 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`（tree `f43e8e15ea31e3d1dd1cf83250d9fdb627c06525`）及 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`（tree `ac06270d0a34c656ba7d7f142ff3c9b13dfa2dbb`），与第三轮规划审计的冻结点一致，没有新的已提交协议增量。目标已将空白历史 function-call arguments 的精确归一化放在 Codex OAuth HTTP/Compact、WS `response.create` 和 fallback 共用的 request preparation 上；官方形态 Codex 客户端向非 Codex executor 的整数工具 schema 窄修仍以 Provider 类型和字段白名单限定。首事件前 EOF、已提交后不得切 Provider 的保护和终态用例也在本仓库。未发现需要新增生产 wire 改动的可复现反例。

本次 Codex 关键词 Rust 407/407、Codex 私有 receipt harness 离线 Node 13/13、reference-delta 18 个 source delta / 22 条 immutable observation 的 `--check-sources`、Server Provider contract/coverage/UI matrix 均通过。`RUN_REAL=0` 的 `gpt_6_astra`、三个 GPT Image 2.5 exact-model operation 与 `ws_prewarm` 五个入口都返回 `blocked_inputs/live_pending`，未访问真实服务。故这些局部合同与现有 committed-source 审计不能提升未提交 CX-N13/CX-D3 的 committed-target 证据，也不能将任何真实 operation 标为通过；无跨账号 fallback、动态客户端指纹或部署/UI 验收。

### Codex 私有 receipt 末次身份栅栏接续（2026-10-08）

推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 提交态未变；这次修复是目标验收器自身的时间窗口，不从参考项目推断新的 OpenAI wire。`scripts/smoke/codex-real-receipt.mjs` 原先读取 Account、Provider、Share 后查可选模型目录、解析 receipt，但接受前不重读身份绑定。五个 operation × Account token generation、Provider revision、Share revision 的 15 个 loopback 反例先全部错误接受。现在 receipt 校验后重读同一绑定，比较 Account ID/两代 generation、Provider revision/runtime fingerprint 与 Share revision；任一漂移都失败关闭，五个正常 operation 各资源读取两次。专项 Node 29/29，receipt schema v1/revision 1、生产 Codex HTTP/WS/image wire、恢复与配额决策均不变。

官方 OpenAI 文档的 Responses 流式事件指南（`https://developers.openai.com/api/docs/guides/streaming-responses`）只用于区分平台流式语义与本地证据门槛，不能证明订阅账号的模型或 WS prewarm entitlement。末次控制面快照也不是跨 API 原子事务。Codex reference-delta 的 18 项冻结源差分、22 条旧 observation 和五个 `live_pending` operation 经 `--check-sources` 复核；Server Provider contract/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过；全量 Node audit 218/218。五个 `RUN_REAL=0` 入口均为 `blocked_inputs/live_pending`，真实 receipt 仍缺失，当前验收逻辑未提交，不能追加假的 committed-target observation；未做 UI 自动化或部署测试。

### Codex 私有 receipt 控制面响应大小栅栏（2026-10-08）

此项是本仓库证据验收器自身的资源边界修复，不从 `CLIProxyAPI` 或 `codex2api` 推导新 OpenAI wire。旧 `codex-real-receipt.mjs` 在读取完整响应后才校验 4 MiB：声明超限长度、或分块实际超限但保持连接的控制面响应，会在限额检查前等待超时。两个本地 loopback 反例先红灯；现于读取前检查 `Content-Length`，读取时逐块累计字节，超过 4 MiB 立即中止，失败输出保持脱敏。Codex 专项 Node 32/32、全量 audit Node 334/334、smoke Node 13/13。

推荐参考提交态仍为 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`；其冻结 `--check-sources` 通过不构成当前未提交 JS 的 committed-target observation。Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。五个 operation 分别以 `RUN_REAL=0` 返回 `blocked_inputs/live_pending`；未读取真实凭据或发送真实请求，五份真实 receipt 仍缺。receipt schema v1/revision 1、生产 Codex HTTP/WS/image wire、模型 entitlement 和账号恢复不变；未做 UI 自动化或部署测试。

### Codex 私有 receipt 静态硬链接边界（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态不变；本项只修本仓库证据文件位置约束。旧 `codex-real-receipt.mjs` 对仓库外路径解析 `realpath`，但同一 inode 在仓库内、外各有一个硬链接时仍会接受外部路径。`gpt_6_astra` 的本地合法 receipt fixture 先复现错误接受；现在要求该文件只有一个硬链接，超出即在控制面请求前失败关闭。正常五个 operation fixture 不变，Codex 专项 Node 33/33、全量 audit Node 358/358、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

这证明静态硬链接绕过被拒绝，不证明对并发文件替换的原子防护，也不能证明部署代码与本地 HEAD 一致。生产 Codex wire、receipt schema v1/revision 1 不变；五个 `RUN_REAL=0` 入口仍各为 `blocked_inputs/live_pending`，无真实请求或 receipt。当前 JS 加固未提交，不追加假的 committed-target observation；未做 UI 自动化或部署测试。

### Codex 私有 receipt 请求期间替换栅栏（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态不变。本项只修目标验收器的证据读取窗口：旧 `codex-real-receipt.mjs` 启动时检查路径、权限和硬链接，但在控制面请求完成后才读文件。五个 operation 各用本地回环服务在首次 Account 请求时，将原先无效的 receipt 原子替换为同路径有效文件；旧流程均错误接受。现于请求前打开只读文件描述符，核对文件类型、dev/inode、链接数、大小和权限后，限量读取最多 1 MiB 并固定字节快照；之后不再按路径重读。五个反例转为脱敏失败，正常 fixture 不变；Codex 专项 Node 39/39、全量 audit Node 396/396、smoke Node 13/13，reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

此快照防止控制面请求期间的路径替换影响所验收的证据，不承诺快照读取瞬间的并发原位改写完全原子，也不证明部署服务运行本地 HEAD。生产 Codex HTTP/WS/image wire、模型 entitlement 和 receipt schema v1/revision 1 未变；五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，没有真实请求或 receipt。当前 JS 改动未提交，不追加假的 committed-target observation；未做 UI 自动化或部署测试。

### Codex Astra 与 WS prewarm 账号 manifest 验收栅栏（2026-10-08）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态未变；本项由目标自己的模型权威边界触发，不推断新 OpenAI wire。目标 `/v1/models` 普通列表可包含 `src/proxy/codex_models.rs` 的 `gpt-6-astra` 冷启动兼容项，不能单独证明绑定账号上游 manifest 的 entitlement。旧私有验收器对 `gpt_6_astra` 与 `ws_prewarm` 只要求普通列表有精确 ID，即使账号 manifest 缺该模型仍可接受 receipt。两项“公开列表有、账号 manifest 无”回环反例先红灯；现在还通过同一 Router Share 的允许路径 `/v1/models?client_version=0.144.1` 读取 Server 转发的绑定账号 manifest，要求 `models[*].slug` 中恰有一个精确模型。缺失与重复各两例均失败关闭；普通列表、完整 receipt、末次 Account/Provider/Share 栅栏仍需通过。三个 Image 2.5 exact-model operation 不受此新增请求影响。

`0.144.1` 与当前 `src/codex_identity.rs::DEFAULT_CODEX_VERSION` 一致，离线测试锚定该默认值；它不是对运行中动态同步版本或真实账号授权的推断。直连 Share 路由不放行 `/backend-api/codex/models`，故验收使用已放行的 `/v1/models` 查询分派，不改 Router。Codex 专项 Node 45/45、全量 audit Node 443/443、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，无真实请求或 receipt；生产 Codex HTTP/WS/image wire、模型 entitlement 规则和 receipt schema v1/revision 1 均未改。新增 JS 未提交，不能以旧 committed-target observation 冒充本次实现证据；未做 UI 自动化或部署测试。

2026-10-08 的 Codex receipt 时效复核只修目标 validator，不新增 OpenAI wire 判断。Astra、三个 Image 2.5 变体和 WS prewarm 五项分别将 `recordedAt` 设为距 24 小时上限 1.2 秒，延迟末次 Share 响应 2 秒；旧流程五例均在 receipt 已过期后错误接受。现内容校验与末次绑定读取后共用同一时间谓词，过期即脱敏失败。生产 HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 与 WS prewarm 未启用状态均不变；五份真实 operation receipt 仍各自 `live_pending`，离线 fixture 不是厂商接受性或部署 commit 证据。

2026-10-08 的 Codex 私有 receipt 文件快照复核只修本地 validator，不新增 OpenAI wire 事实。Astra、三个 Image 2.5 变体及 WS prewarm 各在首次读取有效字节后、EOF 前，对同 inode 文件等长原位覆盖并改变 mtime；旧流程五例均错误接受旧字节。现同一只读描述符在读取前后比较 dev/inode、链接数、大小、权限与纳秒级 mtime/ctime，五例在控制面请求前脱敏失败关闭。元数据复原或精确并发竞态下的原子快照未被证明，离线 fixture 也不证明真实 entitlement。生产 HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 与 WS prewarm 未启用状态均不变；五份真实 receipt 继续 `live_pending`，未提交 JS 不能冒充 committed-target observation；未做 UI 自动化或部署测试。

2026-10-08 的 Codex 最终文件身份复核仍只修本仓库私有 validator，不引入参考项目或 OpenAI wire 新事实。`gpt_6_astra`、三个 Image 2.5 变体和 `ws_prewarm` 各在首次 Account 读取时把已快照的有效 receipt 路径换成另一 inode 的无效文件，或将原 inode 等长原位覆盖并改变 mtime；旧流程十例全部错误接受旧快照。现保存读取后的 bigint 文件指纹，最终 Account/Provider/Share 和 receipt 时效复核后重新解析配置路径，并比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；十例均脱敏失败，正常五项通过。末次文件检查不是跨 API/文件系统原子事务，元数据复原及检查后变化仍可能逃过此门槛；fixture 不证明真实账号 entitlement。Codex 专项 Node 74/74、全量 audit Node 652/652、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，真实 receipt 仍缺；当前 JS 未提交，不将旧 committed-target observation 作为新实现证据。未做 UI 自动化或部署测试。

2026-10-09 的 Codex Share 投影复核只修本仓库五项私有 operation validator，不引入新的 OpenAI wire 事实。生产 `Share` 的 `bindings` 已包含 `share.app` 主条目，导入不变量要求每 App 至多一条且主字段一致；旧验收器拼接主字段后用 `some` 找 Codex Provider，且不检查 enabled/status。Astra、三个 Image 2.5 exact-model operation 与 WS prewarm 各构造重复冲突绑定、主字段冲突、disabled 和 paused 四种反例，旧流程二十例全部错误输出 `contract_verified/live_pending`。现在直接要求原始 `bindings` 中恰好一条 Codex 绑定、主字段与对应唯一条目一致，并要求 `enabled=true/status=active`；二十例先红后绿，正常五项按真实 Share 形状通过，错误均在模型目录/manifest 发网前拒绝。Codex 专项 Node 101/101、全量 audit Node 791/791、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt 及当前工作树提交态 observation 仍缺。生产 HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 与 WS prewarm 未启用决策不变，fixture 不能证明 ChatGPT 账号 entitlement。未做 UI 自动化或部署测试。

2026-10-08 的 Codex 五项 receipt 最终内容复核仍只收紧本仓库私有 validator，不新增 OpenAI wire 事实。五个 operation 各在首个 Account 请求时等长改写已快照 receipt，再用对抗 fixture 让末次元数据读取返回原指纹；旧流程五例均错误接受。现在末次经 `O_NOFOLLOW` 只读描述符核对完整原始字节、EOF、读前读后指纹和配置路径，五例均脱敏拒绝。fixture 的元数据伪装不能证明普通用户能恢复真实 `ctime`；末次检查后变化、跨 API 原子性及真实账号接受性继续待证。生产 Codex HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 与 WS prewarm 未启用决策均不变。

2026-10-09 的 Codex 五 operation 控制面作用域复核只修本仓库私有 receipt validator。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的提交态不变；公开 Responses WebSocket 文档不能证明私有 OAuth entitlement，本项不引入新的 OpenAI wire。旧流程将缺失的 Provider/Share revision 回退为 `0`，且接受空白 runtime fingerprint；五项各构造三份与不完整作用域自洽的 receipt，十五例均错误输出 `contract_verified/live_pending`。现在直接要求控制面 `providerRevision`、`configRevision` 为非负安全整数，`runtimeFingerprint` 为非空白字符串；十五例在目录/manifest 发网前均脱敏拒绝。Codex 专项 Node 117/117、全量 audit Node 933/933、smoke Node 13/13；Codex reference-delta `--check-sources`、Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 和差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，真实 receipt 与当前工作树的提交态 observation 仍缺；fixture 不证明账号接受性或跨 API 原子快照。本轮不改生产 HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 与 WS prewarm 未启用状态；未做 UI 自动化或部署测试。

2026-10-09 的 Codex 五 operation 控制面真实投影复核仍只修本仓库私有 receipt validator。推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交状态未变；本项不增加 OpenAI wire 或私有 OAuth entitlement 判断。目标 Server `ProviderView` 为顶层 `revision` 与 runtime `providerRevision`，旧 Codex mock 却使用不存在的顶层 `providerRevision`；mock 改为真实形状后，Astra、三个 Image 2.5 变体与 WS prewarm 五项正常 fixture 先全部失败。旧 validator 虽已拒绝缺 revision、缺 Share revision 与空白 fingerprint，仍只读取虚构字段。五项各构造仅有旧式顶层字段、缺真实顶层 revision、缺 runtime revision、两份不一致四类自洽反例；旧流程二十例错误输出 `contract_verified/live_pending`。现要求真实顶层/runtime Provider revision 均为非负安全整数且相等；五项正常 fixture 恢复通过，二十例在模型目录或 manifest 发网前脱敏拒绝，既有 Share、manifest、receipt 文件/时效和末次漂移 fixture 保持通过。Codex 专项 Node 138/138、全量 audit Node 1198/1198、smoke Node 13/13；Codex reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt 与当前工作树提交态 observation 仍缺。生产 Codex HTTP/WS/Image wire、receipt schema v1/revision 1、Images disabled 和 WS prewarm 未启用决策不变；控制面复核不是原子快照，fixture 不证明账号接受性。未做 UI 自动化或部署测试。

### Codex 五 operation OAuth Account 布尔投影完整性复核（2026-10-09）

推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的已提交 HEAD 不变；官方 [Create a model response](https://developers.openai.com/api/reference/resources/responses/methods/create) 只约束公开 Responses API，不能证明订阅账号 OAuth entitlement。本项依据目标 Server 自身 `AccountPublicView` 合同收紧私有 receipt validator，不推导新的 OpenAI wire。真实 OAuth Account 投影必带布尔 `hasApiKey` 和 `needsRelogin`；旧 Astra、三个 GPT Image 2.5 变体、WS prewarm 五项 validator 只拒绝值为 `true`。五项各把两个字段设为缺失、`null`、字符串 `"false"`，三十个 loopback 反例旧流程错误输出 `contract_verified/live_pending`。现在要求两个字段都明确为布尔 `false`，三十例在 Provider 与目录/manifest 请求前脱敏拒绝，正常五项 fixture 保持通过。

这只提高私有验收证据完整性，不改变生产 Codex HTTP/WS/Image wire、Share 路由、Images disabled、WS prewarm 未启用或 receipt schema v1/revision 1；分次控制面读取不是原子快照，fixture 不证明真实账号接受性。Codex 专项 Node 169/169、全量 audit Node 1355/1355、smoke Node 13/13；参考 delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查通过。五项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；五份真实 receipt 与当前未提交工作树的 committed-target observation 仍缺。未做 UI 自动化或部署测试。

## 2026-09-28 Claude text/citation and tool-name freeze

CL-N5 与 CL-N6 冻结在 `assets/contract/claude-reference-delta.json` 的 `CL-OBS-0016`～`CL-OBS-0017`。一次性只读证据取自干净的 `CLIProxyAPI@acdace936fa7df2905500c7f5e0a97d683138dea`（tree `f7e64bd57e383e31be75a7077ef918f5154f0ab8`）以及两个历史 committed object：`781a203b` 的相邻 Anthropic text/citation 合并、`75b854eb` 的 Claude 工具名语法。15 个 source delta 的文件摘要和 17 条 append-only observation 可由 `node scripts/audit/audit-claude-reference-delta.mjs --check-sources` 可选复核；默认构建、测试、发布和运行时不读取外部 checkout。

独立实现冻结在 `77940334fda6291706d3807d0d3cdcb49c767d65`（tree `96d2d8dd85812e90a490426d32867ca262cbe84d`）。Anthropic→Responses 的流式与非流式转换会把相邻 text block 合并为一个 assistant message/output item；citation 的 `start_index` / `end_index` 以累计 Unicode scalar 计算，并在新上游 text block 处重置局部搜索起点，避免重复 cited text 回指前一 block。thinking、tool、server tool、refusal 与 terminal 仍是明确边界，不会跨语义粘连。

Claude OAuth 对不符合 `^[A-Za-z0-9_-]{1,64}$` 的普通 custom/MCP 名称默认生成有域分离的请求级可逆 alias；合法自定义名逐字节保留，既有 Claude Code builtin canonical case 和 server tool 行为不变。declaration、forced choice、历史 `tool_use` / `tool_reference`、JSON 和碎片化 SSE 响应共用同一 map；大小写声明、既有合法 wire 名或 alias 碰撞全部在发网前返回 400，不做有损截断。`CC_SWITCH_CLAUDE_CUSTOM_TOOL_ALIAS=disabled` 只保留为事故回滚。

Claude 关键词回归为 270/270，Anthropic 关键词回归为 131/131；Clippy、wire profile、reference-delta 默认审计与 committed-object 复核均通过。CL-N5/CL-N6 只提升为 `fixture_verified`；没有真实 Anthropic 凭据，`oauth_inference`、Max 5x/20x、Fable 5.1、Opus 5.5 和非法 MCP 名称的真实接受性仍保持各自 `live_pending`，本轮没有引入账号池或跨 Account/Provider/rail/site fallback。

整体 review 在 `05d498cf7bbcf1d4e1e67fd1f422c09bf9e8a456` 进一步限定 CL-N6 的 response alias 回映边界：`input`、`arguments`、`args` 与 `functionResponse/function_response` 以下属于 caller/model payload，必须保持 opaque；只有已审查的协议 call-name 位置可以恢复原工具名。`CLAUDE-R2-FINAL-2026-09-28` 绑定 committed target tree、JSON 与分片 SSE decoy fixture；这项窄修复不改变五个真实 operation 的 `live_pending` 状态。

## 2026-09-28 Antigravity reasoning and tool-wire freeze

AG-N7～AG-N11 冻结在 `assets/contract/antigravity-reference-delta.json` 的 `AG-OBS-0018`～`AG-OBS-0022`。一次性只读证据取自干净的 `CLIProxyAPI@acdace936fa7df2905500c7f5e0a97d683138dea`（tree `f7e64bd57e383e31be75a7077ef918f5154f0ab8`）及五个历史 committed object：`6dea3dfa` 的 Responses reasoning summary、`3de5709d` 的 function-response 邻接、`5af6cd75` 的 `$ref` 结果保护、`49eec664` 的 tool choice fail-closed、`580df95a` 的 tool-call ID 碰撞映射。18 个 source delta 的文件摘要和 22 条 append-only observation 可由 `node scripts/audit/audit-antigravity-reference-delta.mjs --check-sources` 可选复核；默认构建、测试、发布和运行时不读取外部 checkout。

独立实现冻结在 `aeeaf1a61e20d8e55c0569c67894fb31dfd54feb`（tree `80784c56b02fd565dae0795463d24861397fba60`）。Responses 的 reasoning effort 与 summary 可见性分别处理；显式 `summary:none/null` 关闭 `includeThoughts`，但不删除 thinking budget。最终 Antigravity wire 边界会把与当前 model function call 匹配的结果前移并合并到紧邻 user turn，同时保留其余文本/媒体顺序；replay 后会再次执行同一归一化，避免补回签名调用后破坏邻接。

Gemini function response 的 `response` 内递归出现字符串 `$ref` 时，完整结果会在有深度和节点上限的检查后编码为 opaque `response.result`；普通用户 JSON、tool arguments 与 declaration schema 不受影响。未知、空白或未声明的 source tool choice，以及原生 Gemini 的非法 mode、空/未知 `allowedFunctionNames` 和清洗后工具名冲突，均在发网前稳定返回 400，不会静默放宽成 auto。tool call ID 映射只存活于单请求：合法值保留、非法字符稳定清洗、碰撞使用确定性后缀，result 优先按 raw ID 匹配；`call573`/`call_573` 一类兼容匹配只有唯一候选时才接受，歧义拒绝。

Antigravity 关键词回归为 69/69；reference-delta 默认审计与 committed-object 复核均通过。AG-N7～AG-N11 只提升为 `fixture_verified`，没有真实 Google 凭据，因此 `antigravity_oauth`、`agy_oauth`、AG-N6 requestType、compaction 和既有 live receipt 仍保持 `live_pending`/runtime disabled；本轮没有引入账号池、跨 Account/Provider/rail/site fallback。

## 2026-09-19 CodeBuddy request-lifecycle memory freeze

CORE-N2 的 CodeBuddy 切片冻结在 `assets/contract/codebuddy-reference-delta.json` 的 `CB-OBS-0015` 与 `CORE-N2-CODEBUDDY` evidence extension。一次性只读证据继续只取 `cli2api@624874a0331f5e8f012ef104b633e69826487458`（tree `c2f02a394e1381adf8291949822ce4b08a48dfb3`）的 committed object，并继续排除工作树中的 `proxy.html`、`proxy.md`。参考实现为若干控制面/error body 设置 1 MiB 上限、为非流 chat body 设置 16 MiB 上限、为单条 SSE line 设置 16 MiB 上限，同时用 `strings.Builder` 和 tool-call map 聚合非流内容。它只提供“读取、单行和 retained aggregation 应有界”的差分信号，不证明统一 sticky request-lifecycle budget、payload/header 副本、下游 transform、capacity 分类或 401 恢复语义。默认审计不读取外部 checkout，只有显式 `node scripts/audit/audit-codebuddy-reference-delta.mjs --check-sources` 才复核冻结 Git object。

生产实现冻结在 `573dc478b21ebcd2d43a4efafc19796c0416990f`（tree `b99432ce78c4c540b648aa84496a884f1449b410`）。它只为精确的 `ProviderType::CodeBuddyOAuth` 启用 sticky budget，并覆盖 Claude、Codex、Gemini 三个 Surface。canonical JSON、runtime/catalog、prepared payload、JSON wire、请求 ID、目标 headers、错误 body/解析工作集、SSE transport chunk/rolling buffer/event parse/canonical `Bytes` owner、非流 content/reasoning/tool-call 聚合与最终 `Value`，以及三 Surface 下游 bridge retained state 都在分配或增长前核算。

Intl/CN 同账号 pre-commit 401 recovery 前后复用同一预算；耗尽是 sticky 的，未提交响应稳定返回 `503 / cc_switch_request_memory_exhausted`，已提交 stream 只发送一个脱敏 terminal。usage 归类 `memory_capacity`，Provider outcome 为 `CapacityShed`，不会 replay、refresh、换 Account/Provider/site，也不会记为 `NetworkFailure`。分发边界堆化 `forward_codebuddy` future，避免大型异步状态依赖提高 Tokio 默认测试线程栈。

专项验证为 CodeBuddy 66/66、request-memory 26/26、memory-exhaustion 15/15，`cargo check --lib`、Clippy `--all-targets -D warnings`、格式与差异检查均通过。15 条 append-only observation 与 `CORE-N2-CODEBUDDY=fixture_verified` 只证明本地容量合同；Intl/CN 两站真实订阅 receipt 仍分别为 `null/live_pending`，CB-N4、企业与多模态能力继续保持 disabled/未提升。

## 2026-09-19 Kiro request-lifecycle memory freeze

CORE-N2 的 Kiro 切片冻结在 `assets/contract/kiro-reference-delta.json` 的 `KI-OBS-0014` 与 `CORE-N2-KIRO` evidence extension。一次性只读证据继续只取 `kiro.rs@be0c04219d9d1b93b7fe5c3d7b9e7c9cf0d05863`（tree `5e656c1bf70aac0224a68251b2065966db01e933`）的 committed object：`MAX_MESSAGE_SIZE` 与 `DEFAULT_MAX_BUFFER_SIZE` 都是 16 MiB，分别拒绝超长 AWS EventStream frame 与 rolling decoder buffer。该参考只证明 frame/buffer retained state 必须有界，不证明统一 request-lifecycle budget、图片解码/resize 膨胀、canonical/wire 副本、下游 transform 或容量分类。默认审计不读取外部 checkout，只有显式 `node scripts/audit/audit-kiro-reference-delta.mjs --check-sources` 才复核冻结 Git object。

生产实现冻结在 `7526159242d3279ec9fd2eca56a6efacf0c3c9da`（tree `76f89befd1e2f28a227e37d2998c874011dcecde`）。它只为精确的 `ProviderType::KiroOAuth` 与 `ProviderType::AmazonQOAuth` 启用 sticky budget，覆盖 Claude Messages、Codex Chat Completions/Responses 的非流与流式 EventStream；raw/decoded body、canonical JSON、prepared request、序列化 wire body、目标 headers、prompt-cache 临时状态、非流响应 wire/解压/聚合/转换，以及 EventStream decoder/frame、tool JSON、tool leak filter、SSE builder、usage parser 与 downstream transform 共享同一请求预算。图片路径在 base64 decode 前预留 decoded upper bound，并核算像素 decode、resize、RGB、JPEG 与重新 base64 的工作集。

同账号 401 replay 前后不会重置预算；容量耗尽是 sticky 的，未提交响应返回稳定 `503 / cc_switch_request_memory_exhausted`，已提交 200 的 stream 发送同码 503 terminal。usage 归类 `memory_capacity`，Provider outcome 归类 capacity shed，不会进入普通 `NetworkFailure`，也不会重放、换 Account、Provider、auth kind 或 region。prepared/canonical reservation 跟随流式 Response body 生命周期；内外两层 stream 堆化，避免大型状态按值穿过 `Poll` 导致默认测试线程栈溢出。

专项验证为 Kiro 102/102、request-memory 21/21、Amazon Q 14/14，`cargo check --lib`、Clippy `--all-targets -D warnings`、格式与差异检查均通过。14 条 append-only observation 与 `CORE-N2-KIRO=fixture_verified` 只证明本地容量合同，不是八个 auth-kind × region scope 的真实 OAuth/长流/内存压力 receipt；八份 receipt 继续分别为 `null/live_pending`，remote compaction 与 shared cache 仍保持 runtime disabled。

### Grok 三项私有 receipt 目录与末次身份栅栏（2026-10-08）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态未变，后者六项未提交 Codex 工作树路径继续排除。本项修复目标 `scripts/smoke/grok-real-receipt.mjs` 的独立验收窗口，不推断新的 xAI wire。旧验收器将任何正数 `fetchedAtMs` 和任何格式合法的 `source` 当作 fresh 目录；且只在开始读取 Account、Provider、Share，目录及 receipt 处理之后不复核。inference/media/remote_compaction × 旧/超前/伪来源目录 9 个反例、三项 × Account token generation / Provider revision / Share revision 9 个反例均先错误接受。

验收现只接受 Server 生产目录代码确实返回的 `upstream`、`upstream_not_modified`、`cache_fresh`，并要求非 stale 的获取时间在 2 小时内、未来偏差不超过 5 分钟。此 2 小时是私有证据门槛，不改变 Grok 运行时可配置的目录 TTL；若长 TTL 缓存超过该窗口，必须取得新目录证据，不能假称 fresh。receipt 验证后重读同一 Account、Provider、Share，比对 Account ID/两代 generation、Provider revision/runtime fingerprint 和 Share revision，漂移即失败关闭。`cache_fresh` 与 304 续期来源的正常 fixture 仍通过；专项 Node 34/34。末次快照不是跨 API 原子事务。

生产 Grok 版本 `0.2.111`、group header、目录发布、推理/media wire 与 receipt schema v1/revision 1 均未改；GR-N4 仍需同账号版本/header 真实记录，remote compaction 仍 disabled。Grok reference-delta `--check-sources`、Server Provider contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过；全量 Node audit 253/253。三项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，真实 operation receipt 仍 `null/live_pending`，当前新验收逻辑未提交，不可新增虚假的 committed-target observation；未做 UI 自动化或部署测试。

### Grok 三项私有 receipt 控制面响应大小栅栏（2026-10-08）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态不变，sub2api 六项未提交 Codex 工作树改动排除。本项是目标验收器自己的资源边界，不是新增 xAI wire。旧 `grok-real-receipt.mjs` 在等待完整控制面响应后才检查 4 MiB；inference 的声明长度超限、media 的无长度分块实际超限且连接保持不结束，两条本地回环反例均先红灯（约 5 秒超时）。现先查 `Content-Length`，再按实际读取字节逐块限额，超限立即中止并保持脱敏失败。Grok 专项 Node 37/37、全量 audit Node 340/340、smoke Node 13/13；reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

生产 Grok 版本 `0.2.111`、group header、推理/media/WS wire、目录发布、receipt schema v1/revision 1 与 remote compaction disabled 状态均不变。inference、media、remote_compaction 的 `RUN_REAL=0` 入口仍各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；GR-N4 仍缺同一固定账号的版本/header 接受性证据。此 JS 加固未提交，不追加伪造的 committed-target observation；未做 UI 自动化或部署测试。

### Grok 三项私有 receipt 静态硬链接边界（2026-10-08）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交状态不变；sub2api 六项未提交 Codex 工作树改动仍排除。本项只修目标私有验收器的文件位置约束：仓库外 receipt 路径和 `realpath` 检查不能拒绝与仓库内文件共用 inode 的硬链接。inference、media、remote_compaction 三个合法 fixture 分别从仓库内硬链接到仓库外，旧实现先错误接受；现要求已解析 receipt 文件 `nlink=1`，三例在任何控制面请求前脱敏失败关闭。正常 fixture 和既有目录、绑定、响应大小反例保持通过；Grok 专项 Node 41/41、全量 audit Node 365/365、smoke Node 13/13，reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix 通过。

这只证明静态硬链接绕过被拒绝，不证明文件并发替换的原子防护或部署代码与本地 HEAD 一致。生产 Grok `0.2.111` 身份、group header、推理/media/WS wire、目录发布及 receipt schema v1/revision 1 未变；remote compaction 仍 disabled。三项 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；GR-N4 同账号版本/header 真实证据仍缺。当前 JS 加固未提交，不能新增假的 committed-target observation；未做 UI 自动化或部署测试。

### Grok 三项私有 receipt 请求期间替换栅栏（2026-10-08）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者六项未提交 Codex 文件继续排除。本项只修目标验收器的文件读取窗口，不引入新的 xAI wire。旧流程启动时检查 receipt 路径、权限及硬链接，却在 Account/Provider/Share 和目录请求后才按路径读取；inference、media、remote_compaction 各自让首个 Account 请求把无效文件原子替换为有效文件，三项均先错误写出 `contract_verified/live_pending`。现于控制面请求前从只读文件描述符限量取得最多 1 MiB 快照，并核对文件类型、dev/inode、链接数、大小和权限，之后只解析固定快照。三项替换反例均失败关闭；正常 fixture、静态硬链接、目录、绑定漂移和响应大小用例保持通过，Grok 专项 Node 45/45。

这是私有证据验收边界，不改变生产 Grok `0.2.111` 身份、group header、推理/media/WS wire、目录发布、receipt schema v1/revision 1 或 remote compaction disabled 状态。快照不保证读取瞬间并发原位改写的原子视图，也不证明部署 commit 与本地 HEAD 一致。全量 audit Node 403/403、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法及差分空白检查通过。三项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，无真实请求或 receipt；GR-N4 同账号版本/header 接受性仍缺。新增 JS 未提交，不以旧提交态 observation 冒充新实现证据；30 项独立真实验收及最终整体 review 仍未完成，未做 UI 自动化或部署测试。

### Grok OAuth 账号 `x-grok-*` 身份头隔离（2026-10-08）

推荐的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交实现发送 `x-grok-client-mode`、`x-grok-session-id`、`x-grok-req-id`、`x-grok-agent-id` 等头；它们只证明参考自身的身份构造，不证明本 Server 使用这些头会获同一账号接受。`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 仍无新提交，六项未提交 Codex 文件继续排除。目标此前对 Grok 账号 `extraHeaders` 逐项拒绝部分身份头，却允许 `X-Grok-Client-Mode: headless`；实际 `apply_account_header_overrides` 测试先复现错误接受。现对精确 `GrokOAuth` 禁止账号自定义整个 `x-grok-*` 命名空间，同时保留 `x-xai-token-auth` 和 `x-authenticateresponse` 的既有拒绝；普通非身份自定义头仍可用。修复不添加出站 header，不改默认版本 `0.2.111`、group header、请求/响应 wire 或 receipt schema。

定点反例先红后绿，Grok Rust 231/231、全量 library 3284 passed / 1 ignored、其他目标 124 + 1 + 1 passed；Grok receipt Node 45/45、Clippy `--all-targets -D warnings`、rustfmt、Grok reference-delta `--check-sources`、Provider Phase 0/contract/coverage/UI matrix 均通过。inference、media、remote_compaction 的 `RUN_REAL=0` 仍各为 `blocked_inputs/live_pending`；GR-N4 同账号 catalog/HTTP/WS 版本/header 接受性和三份 operation receipt 仍缺。此工作树实现未提交，不追加虚构的 committed-target observation；未做 UI 自动化或部署测试。

2026-10-08 的 Grok 三项私有目录验收绑定复核不新增 xAI wire 事实：`grok2api@7c889a96`、`sub2api@ab99d56e` 的已提交状态未变，后者未提交 Codex 文件排除。参考 sub2api 的模型同步按传入 Account 构造凭据请求；目标 Server `/v1/models` 则按 Router 签名 Share 选 Grok Provider/Account，普通 `providerId` 查询不参与选择。旧 inference、media、remote_compaction validator 均只发送 `app/providerId`，三条缺预期 Share/Provider 参数的回环断言先红。现三项请求发送可选 `expectedShareId/expectedProviderId`，复用已验证的 Server 目录栅栏，在目录工作前与签名 Share/选定 Provider 比较；不匹配即冲突。此检查不证明真实 token、同账号 GR-N4 版本/header 接受性或厂商模型授权；三份 operation receipt 与 GR-N4 均保持独立 `live_pending`，remote compaction 仍 disabled。

2026-10-08 的 Grok 私有目录时效复核只修本仓库验收器，不引入新的 xAI wire 观察。inference、media、remote_compaction 各以取得目录时距两小时上限一秒、末次 Share 响应延迟 1.5 秒的本地反例证明旧流程会在目录已过期后错误接受；现于末次绑定复核后以同一时间谓词再次核对 `fetchedAtMs`，三例均失败关闭。生产可配置目录 TTL、默认 CLI 版本 `0.2.111`、group header、receipt schema v1/revision 1 与 remote compaction disabled 均不变。末次快照不是跨 API 原子事务，离线 fixture 不能替代三份真实 receipt 或 GR-N4 同账号版本/header 证据。

2026-10-08 的 Grok 三项 receipt 时效复核仍只涉及本仓库 validator。inference、media、remote_compaction 各将 `recordedAt` 设为距 24 小时上限 1.2 秒，延迟末次 Share 响应 2 秒；旧流程三例均在 receipt 已过期后错误接受。现初次与末次复核共用 receipt 时间谓词，并以同一接受时刻核对目录与 receipt，三例均脱敏失败关闭。生产 Grok CLI 版本 `0.2.111`、group header、HTTP/WS/media wire、receipt schema v1/revision 1 与 remote compaction disabled 均不变；GR-N4 同账号版本/header 接受性和三份真实 operation receipt 仍缺。

2026-10-08 的 Grok 三项私有 receipt 文件快照复核只修本仓库验收器，不引入新的 xAI wire 事实。inference、media、remote_compaction 各在首次读取有效字节后、EOF 前把同 inode receipt 等长原位覆盖并改变 mtime；旧流程三例均错误输出 `contract_verified/live_pending`。现同一只读描述符在读取前后比较 dev/inode、nlink、size、mode 及纳秒级 mtime/ctime，三例均在控制面请求前脱敏失败。元数据复原或精确并发竞态下的原子快照未获证明；生产默认 CLI `0.2.111`、group header、HTTP/WS/media wire、receipt schema v1/revision 1 和 remote compaction disabled 均不变。三份真实 operation receipt 与 GR-N4 同账号版本/header 证据仍缺；未提交 JS 不构成新的 committed-target observation，未做 UI 自动化或部署测试。

2026-10-08 的 Grok 最终文件身份复核仍只修本仓库私有 validator，不新增 xAI wire 事实。inference、media、remote_compaction 各在首次 Account 读取时把已快照的有效 receipt 路径替换为另一 inode 的无效文件，或将原 inode 等长原位覆盖并改变 mtime；旧流程六例全部错误接受旧快照。现保存读取后的 bigint 文件指纹，在最终 Account/Provider/Share、目录及 receipt 时效复核后重新解析配置路径，并比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；六例均脱敏失败，正常三项与既有目录/绑定 fixture 通过。末次检查不是跨 API/文件系统原子事务，元数据复原及检查后变化仍可能逃过此门槛；离线 fixture 不是厂商接受性。Grok 专项 Node 64/64、全量 audit Node 664/664、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。三项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt 和 GR-N4 专项证据仍缺，当前 JS 未提交，不以旧 committed-target observation 冒充新实现证据。未做 UI 自动化或部署测试。

2026-10-08 的 Grok 三项 receipt 最终内容复核仍只增强私有 validator，不新增 xAI wire 结论。inference、media、remote_compaction 各在首个 Account 请求时等长改写已快照 receipt，再用对抗 fixture 让末次元数据读取返回原指纹；旧流程三例错误接受。现在末次经 `O_NOFOLLOW` 只读描述符比较完整原始字节、EOF、读前读后指纹与配置路径，三例均脱敏拒绝。fixture 的元数据伪装不代表普通用户能够恢复真实 `ctime`；检查后的竞态、跨 API 原子性和厂商接受性继续待证。生产 Grok HTTP/WS/media/compaction wire、receipt schema v1/revision 1 和 GR-N4 版本决策均不变，三份真实 receipt 继续 `live_pending`。

2026-10-09 的 Grok 三 operation Share 绑定复核只修本仓库私有 validator，不新增 grok2api、sub2api 或厂商 wire 事实。生产 Share 的原始 `bindings` 已含主 App 条目，导入不变量要求每 App 至多一条且主字段一致；旧验收器却另拼主字段再用 `some` 找 Codex Provider，也不检查 Share 启用状态。inference、media、remote_compaction 各构造重复冲突绑定、主字段冲突、disabled、paused 四类反例，旧逻辑十二例全部错误输出 `contract_verified/live_pending`。现要求原始 `bindings` 中 Codex 恰好一条精确 Grok 绑定、主字段与其对应唯一条目一致，且 `enabled=true/status=active`；十二例先红后绿，正常三项及目录/时效/漂移 fixture 保持通过，错误在模型目录请求前拒绝。Grok 专项 Node 81/81、全量 audit Node 813/813、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 证据及当前工作树提交态 observation 仍缺。生产 Grok HTTP/WS/media/compaction wire、Share 路由、receipt schema v1/revision 1 与 remote compaction disabled 决策不变；控制面复核不是跨 API 原子快照，fixture 不证明 xAI 接受性。未做 UI 自动化或部署测试。

2026-10-09 的 Grok 三 operation 控制面投影与作用域复核只修本仓库私有 receipt validator。推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者六项未提交 Codex 文件排除，不推导新 xAI wire。目标 Server 的 `ProviderView` 实际为顶层 `revision` 与 runtime `providerRevision`，旧 Grok mock 却使用不存在的顶层 `providerRevision`；mock 改成真实形状后，三个正常 operation 先全部失败。旧 validator 从虚构字段或 Provider 内层 revision 回退，最终可将真实 Provider 作用域误记为 `0`。三项各构造缺 runtime revision、缺顶层 revision、两份不一致、两份均缺、旧式虚构顶层字段、缺 Share revision 与空白 fingerprint 七类自洽反例，旧流程二十一例全部错误输出 `contract_verified/live_pending`。现要求两份真实 Provider revision 为非负安全整数且一致，Share `configRevision` 为明确的非负安全整数，runtime fingerprint 是非空白字符串；正常三项按真实投影恢复通过，二十一例均在目录请求前脱敏拒绝。Grok 专项 Node 103/103、全量 audit Node 968/968、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三项 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 接受性及当前工作树提交态 observation 仍缺。生产 Grok HTTP/WS/media/compaction wire、Share 路由、receipt schema v1/revision 1 与 remote compaction disabled 决策不变；控制面复核不构成原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

### Grok 三 operation Account 布尔投影完整性复核（2026-10-09）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者六项未提交 Codex 文件不作为 Grok 证据。本项只核对目标 Server 自身 `AccountPublicView` 与私有 receipt validator，不推导新 xAI wire。真实 `/api/accounts` 投影必带布尔 `hasApiKey` 和 `needsRelogin`；旧 inference、media、remote_compaction validator 只拒绝值为 `true`，每项中任一字段缺失、为 `null` 或字符串 `"false"` 的六种反例均错误输出 `contract_verified/live_pending`，三项共十八例。现要求两个字段都严格为布尔 `false`；十八例先红后绿，在 Provider 与目录请求前脱敏失败，正常三项 fixture 保持通过。

此项仅收紧私有验收证据，不改生产 Grok HTTP/WS/media/compaction wire、账号路由、receipt schema v1/revision 1、GR-N4 版本决策或 remote compaction disabled 状态。离线 fixture 不证明真实账号接受性，跨 API 控制面读取不是原子快照。Grok 专项 Node 122/122、全量 audit Node 1220/1220、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、GR-N4 同账号版本/header 证据与当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

### Grok 三 operation 动态目录接受前内容复核（2026-10-09）

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`、`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的已提交 HEAD 未变；后者六项未提交 Codex 内容继续排除。GR-N4 同账号版本/header 决策仍需真实证据，本项不推导新 xAI wire。目标三项私有 receipt 已把初次目录的完整 `data` 摘要放进 scope 和 receipt，但在 receipt 校验及末次 Account/Provider/Share 复核后，只重新检查旧目录时间戳。回环服务让第一次目录有效、第二次分别撤下所选模型、添加新模型、标记 stale、改为伪来源或给出超前时间；旧 validator 各只请求一次，三 operation 共十五例错误输出 `contract_verified/live_pending`。

现在 inference、media、remote_compaction 均在末次绑定复核后经同一签名 Share/Provider 栅栏再读目录；两次目录时间均须在共同接受时刻新鲜，当前目录仍须含唯一精确模型、允许的来源、非 stale，且完整 `data` 摘要与 receipt 绑定的初次摘要一致。十五例先红后绿；另外三项各验证来源从 `upstream` 合法变为 `cache_fresh`、内容不变的正例，避免把来源字符串差异误判为内容漂移。Grok 专项 Node 141/141、全量 audit Node 1378/1378、smoke Node 13/13；Grok reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。生产 Grok HTTP/WS/media/compaction wire、目录缓存、默认 CLI 版本、group header、receipt schema v1/revision 1 与 remote compaction disabled 状态不变；分次读取不是跨控制面/上游原子快照，fixture 不证明真实 xAI entitlement。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，三份真实 receipt、GR-N4 同账号版本/header 接受性与当前工作树 committed-target observation 仍缺；未做 UI 自动化或部署测试。

## 2026-09-19 Grok request-lifecycle memory freeze

CORE-N2 的 Grok 切片冻结在 `assets/contract/grok-reference-delta.json` 的 `GR-OBS-0011` 与 `CORE-N2-GROK` evidence extension。一次性只读证据继续只取 `grok2api@906b9493b099d192381c698d4e320fafeccb851c`（tree `eca3a45c482375fad636578b28d73c1186d2bef4`）的 committed object：reasoning cache 默认以 4096 entry、30 分钟 TTL、scope key 与 LRU 淘汰约束状态。该参考只证明 replay retained state 必须有界，没有单 proof 字节上限，也没有统一 request-lifecycle budget；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的 6 项工作树修改继续全部排除。默认审计不读取外部 checkout，只有显式 `node scripts/audit/audit-grok-reference-delta.mjs --check-sources` 才复核冻结 Git object。

生产实现冻结在 `2eca08a97fcf3c2059d1ffa9a15782d95a98b073`（tree `098e4fe6df005d3623601cbdf6f8863c016b0cad`）。它只为精确的 `ProviderType::GrokOAuth` 启用 sticky budget，并让 HTTP JSON/SSE、Responses WebSocket turn 与 Grok media 共用同一容量语义；同一请求的 401、reasoning rejection recovery、WS→HTTP fallback 和 terminal 都不能重置预算。replay cache 在复制 proof 前先 reserve，request parse/apply、capture、stream accumulator、retry payload 与 WS 读写继续使用原请求预算；binding drift 重新准备时也不创建未核算副本。

核算范围还包括 SSE terminal/usage/transform state、completed-search set 与长 ID 摘要、multipart/base64 image-edit 扩张、媒体 raw/decoded/normalized body、响应 wire/解压 body，以及图片 JSON/SSE stream buffer。耗尽后 HTTP 返回稳定 `503 / cc_switch_request_memory_exhausted`，已提交 SSE 发同码 terminal，WebSocket 发 error 后 Close；当前上游被取消，不再 replay、换 Account/Provider/rail，也不记为普通 `NetworkFailure`。流状态本身改为堆上持有，避免大型 `try_unfold` state 按值穿过 `Poll` 层造成测试线程栈溢出。

专项验证为 Grok 184/184；完整 Rust lib 为 3153 passed、1 ignored、0 failed，Clippy `-D warnings`、格式与差异检查均通过。11 条 append-only observation 与 `CORE-N2-GROK=fixture_verified` 只证明本地容量合同，不是 inference、media 或 remote-compaction 的真实 OAuth/长流/内存压力 receipt；GR-N2 三个 operation 继续分别保持 `receipt=null`、`live_pending`，remote compaction 仍为 `runtimeEnabled=false`。

## 2026-09-19 Cursor request-lifecycle memory freeze

CORE-N2 的 Cursor 切片冻结在 `assets/contract/cursor-reference-delta.json` 的 `CUR-OBS-0008`。一次性只读证据仍只取 OmniRoute `02c663cdd0e8577bdcf2b01a44046bcd46dc6a7a`（tree `25b36e4993a8cc52da22b3d0b0aa26bd4c60426b`）的 committed object：executor 对单个 Connect frame 设 16 MiB 上限并在消费后 splice rolling buffer；session manager 默认以 5 分钟 TTL、100 个 session 上限及 close cleanup 约束 parked h2 state。该参考只证明 transport/session retained state 必须有界，既没有统一 request-lifecycle budget，也没有约束 gzip expansion；22 项工作树内容继续全部排除。默认审计不读取外部 checkout，只有显式 `node scripts/audit/audit-cursor-reference-delta.mjs --check-sources` 才复核冻结 Git object。

`3e0f5ff` 的独立实现对精确的 `ProviderType::CursorOAuth` 与 `ProviderType::CursorApiKey` 启用同一个 sticky request budget，并覆盖 Claude Messages、Codex Responses/Chat Completions 与 Gemini GenerateContent surface。Share 与 pinned Provider test 在 AgentService 发网前核算最终 normalized body；预算随后显式传入 Provider facade、driver 与 h2 transport，同一请求的 OAuth 401 replay 和 semantic retry 共用总量，耗尽即返回 `DeniedBudget`，不换 Account、Provider 或 rail。

核算范围包括 Agent plan/JSON、模型与工具 schema、图片 decoded bytes、protobuf body、unbounded h2 write queue、parser growth、gzip expansion、decoded/pending frame、错误 body、SSE writer/marker/dedup、semantic buffered events 和 completed-response replay clone。parked session 保留原 reservation；continuation 必须先向新请求预算预留 session、parser 与 pending frame，再释放旧预算，close、expiry 和失败 guard 均清理 retained state。reservation 跟随最终 `Bytes` owner；已提交 200 的四种 surface 使用稳定 `cc_switch_request_memory_exhausted` 终止码，usage 标为 `memory_capacity`，Provider outcome 记为 capacity shed 而非网络故障。

当前专项验证为 Cursor 304/304、request-memory 16/16、`cargo check --lib` 与 Clippy `-D warnings` 通过；reference-delta 默认和 `--check-sources` 审计均通过。8 条 append-only observation 与 `CORE-N2-CURSOR=fixture_verified` 只证明本地容量合同，不是 OAuth/API-key 真实长流或内存压力 receipt；两条 rail 仍分别保持 `live_pending`，不得因本切片升级。

## 2026-09-19 Claude OAuth request-lifecycle memory freeze

CORE-N2 的 Claude 切片冻结在 `assets/contract/claude-reference-delta.json` 的 `CL-OBS-0015`。一次性只读证据来自 `CLIProxyAPI@2e6b1d83f6c304a102aa33c1faf0a4f94d0d331e`（tree `48db041ce6e0ea0bb0c5c96a7fcfb0566c12eca8`）：其 Claude-compatible thinking replay 将每 session 限为 8 MiB/64 turns、每 turn 限为 512 blocks、进程总量限为 256 MiB，TTL 为一小时，并以 snapshot CAS replace/clear。该参考只证明 Claude retained replay state 必须有界，而且只适用于 API-key compatibility rail；它不能证明本仓库 Claude OAuth 的统一请求生命周期预算。默认审计不读取外部 checkout，只有显式 `node scripts/audit/audit-claude-reference-delta.mjs --check-sources` 才复核冻结 Git object。

`87fc0b0` 的独立实现只为精确的 `ProviderType::ClaudeOAuth` 启用已有 sticky budget；同一 Claude App 下的 `claude`、`claude_auth` 和其他 Provider 不被误启用。Share binding 与 pinned Provider test 均在发网前初始化预算，最终 execution 选择后的 binding 漂移兜底会补齐 raw/decoded reservation。预算在同一请求的 transport/body recovery 间共享，一旦耗尽便禁止 replay，不切换 Account、Provider 或 rail。

核算范围包括 raw/decoded/normalized request body、buffered response，以及 Claude 压缩 SSE 的完整 wire body 和解压 body；解压上限按预算剩余量收窄，decoded `Bytes` 的 reservation 跟随 backing owner 到最后一个 clone/view drop。流式路径在每次可能增长前预留并在处理后缩回实际 retained bytes，覆盖 Anthropic inspector 的 pending/open blocks/tool partial JSON、terminal detector、usage decoder、Claude error detector、工具名 alias/buffer 与通用 stream transformer。请求转换同时移除整棵 JSON 和 alias map 的不必要 clone，不改变 header/wire golden。

低预算 fixture 证明 Share 与 pinned Provider test 在发网前零请求拒绝，raw+decoded+normalized 合计超限，压缩 SSE 膨胀只请求一次且不触发 transport retry，以及已返回 200 后 tool partial JSON 累积只能发送稳定的 `503 / cc_switch_request_memory_exhausted` 终止帧。完整 Rust 回归为 lib 3132 passed/1 ignored、API contract 124 passed、两个独立 integration 各 1 passed。13 个 source delta 与 15 条 append-only observation 只支持 `CORE-N2-CLAUDE=fixture_verified`；`oauth_inference`、`max_5x_plan`、`max_20x_plan`、`fable_5_1` 四个 operation 仍为 `live_pending`，不得因本切片升级。

## 2026-09-19 Antigravity request-lifecycle memory freeze

CORE-N2 的 Antigravity 切片冻结在 `assets/contract/antigravity-reference-delta.json` 的 `AG-OBS-0017`。一次性只读证据来自 `CLIProxyAPI@09a29bd345bc44c473abe7fd07859e32df2ea543`（tree `0b407c0c70c9c2f8a771d5126b028b9afe79bf2a`）：其 replay cache 将单 entry 限为 16 MiB，流 accumulator 也在溢出时放弃捕获。该证据只证明 replay 局部状态应有界，不能证明统一请求生命周期预算；默认审计不读取外部 checkout，只有显式 `node scripts/audit/audit-antigravity-reference-delta.mjs --check-sources` 才复核冻结 Git object。

Server 的独立实现只为最终绑定到 `antigravity_oauth` 或 `agy_oauth` 的请求启用已有 sticky budget，不把同属 Claude/Gemini App 的其他 Provider 纳入。Share 请求在解压前按实际 Provider binding 初始化，pinned Provider test 按 `ProviderExecution` 初始化；最终 execution 选择后的代际漂移兜底会补齐 raw/decoded reservation。预算在所有 retry 间共享，耗尽后关闭后续 replay，并统一核算 raw/decoded/normalized body、reasoning replay cache clone/snapshot/stream accumulator，以及 grounding/citation、web-search gating、tool context 和 Chat canonicalizer pending state。

Normalized body reservation 跟随最终 `Bytes` backing owner，直到最后一个 clone/view drop 才释放；replay previous chain 使用共享 owner，accumulator 超限后立即丢弃内容。流式 retained state 超限时取消当前上游并发送稳定的 `503 / cc_switch_request_memory_exhausted` 终止帧，不再发起 retry；取消、错误和正常终态均由生命周期测试验证 reservation 归零。错误与指标不包含 prompt、citation URL、凭据或 Provider/Account 身份。

当前 67 个 Antigravity Rust 关键词测试和 17 条 append-only observation 只支持 `CORE-N2-ANTIGRAVITY=fixture_verified`。它们不是 Google OAuth、长流或真实内存压力 receipt；`antigravity_oauth`、`agy_oauth` 两条 rail、AG-N6 requestType 与 compaction 继续保持 `live_pending`，不得因本切片升级。

## 2026-09-18 Kiro tool names, profile fallback, and compaction gate freeze

Kiro 增量证据追加在 `assets/contract/kiro-reference-delta.json`，保留 KI-01～KI-05 历史合同。只读来源为 `kiro.rs@f413e7de` 的裸 namespaced tool 恢复、`kiro.rs@3194bb29` 的 profileArn 兼容回退，以及 `kiro.rs@0b8c7dec/@d62054f5/@13763b69` 的 Responses Compact 结构；完整 commit、路径和提交态 SHA-256 可由 `node scripts/audit/audit-kiro-reference-delta.mjs --check-sources` 可选复核。默认审计、构建、测试、发布和运行时不读取外部 checkout。

KI-N1 使用只含本次请求 `tools` 声明的注册表，依次执行实际上游名精确映射、完整原名精确匹配、唯一 `__` child 恢复和普通名保留。历史消息、Account、Share、session 与进程缓存均不能提供候选；同名普通工具优先，两个 namespace 的同 child 返回稳定 `KIRO_EVENT_STREAM_INVALID`，不猜测目标。fixture 覆盖 builtin、超长 hash、历史污染、分片参数及 Claude Messages、Codex Chat Completions、Codex Responses 三 Surface。

KI-N2 只在带 profileArn 的请求收到 403，或收到正文明确包含 `Improperly formed request` / `Invalid profileArn` 的 400 时，才在同一 `q.{region}` host 去除 ARN 重试。401、429、5xx、timeout/TLS/transport、decode 和成功响应结构错误均保留原分类并停止；没有独立 receipt 的 q→CodeWhisperer host fallback 已关闭。模型目录沿用已冻结的 403 区域候选合同，但新增的 400 回退同样受正文与“原请求确实带 ARN”双重约束。

KI-N3 当前只完成保守门禁：Kiro 与独立 Amazon Q 的 `/responses/compact` 或普通 Responses compaction trigger 在模型目录、凭据刷新和推理发网前稳定拒绝，decoy fixture 请求数为零。参考项目的 remote compaction 仅证明值得后续差分研究，不能替代本产品的绑定账号真实 receipt；`runtimeEnabled=false`、`live_pending` 和空 receipt 保持不变。

CORE-N1 已将 Kiro/Amazon Q 产品类型、canonical request/model/session、Account-bound catalog/region/request preparation、本地 CountTokens、keepalive、stream error 和 subscription throttle 决策收敛到 `src/proxy/providers/kiro/`。共享 forwarder 继续拥有 Share/Account lease、usage、terminal、统一 attempt budget 和同账号 401 replay；AWS EventStream codec 与协议转换仍在 `src/proxy/kiro.rs`。专项 Rust fixture 覆盖三 Surface、CountTokens 零推理发网、产品隔离、Compact gate、401 once-only、CRC/截断/timeout 与 lease 释放，拆分没有增加新 attempt 或 fallback。

LIVE-N1 私有 validator 将 Kiro 拆成 `builder_id`、`idc`、`social`、`api_key` × `us-east-1`、`eu-central-1` 八个独立 receipt。每份绑定当前 target commit、Account auth/token generation、Claude/Codex Provider revision/runtime、Share revision、签名用户/session、exact model 和两份 fresh catalog，并要求 CountTokens、两 Surface stream/non-stream、tool namespace、Prompt Cache、401/throttle/timeout/drift、Compact 与 shared-cache 关闭、decoy 和 secret scan 证据。receipt 必须在仓库外且真实模式为 `0600`；fixture 模式八组已验证 validator，但只能输出 `contract_verified/live_pending`。当前八份 receipt 仍为 `null`，KI-N3 Compact 与 KI-05 shared cache 均不会因 harness 自动启用。

EVID-N1 已将 Kiro reference delta 迁为 append-only schema v2。`1124082` 中原 schema-v1 文件 SHA-256、target commit/tree、全部十个历史字段的 canonical/逐字段 digest 均已冻结；新增干净的 `kiro.rs@be0c04219d9d1b93b7fe5c3d7b9e7c9cf0d05863` / tree `5e656c1bf70aac0224a68251b2065966db01e933` committed-object snapshot，以及 14 条不可变 observation，覆盖 KI-01～05、KI-N1～N3、CORE-N1/N2、LIVE-N1 和 KI-R1～R3。三条 reject 分别固定跨 Account/auth-kind/region/Provider 或宽 profile/host fallback、Redis/session-affinity/cache 路由，以及无独立执行器和真实证据的 remote-compaction 自动启用。每条 observation 绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；默认审计不读取外部 checkout，只有显式 `--check-sources` 才复核冻结对象。

## 2026-09-18 Codex bootstrap, metadata, WebSocket fairness, and memory freeze

Codex 增量证据追加在 `assets/contract/codex-reference-delta.json`，保留原有 CX-01～CX-06 历史合同。只读来源为 `CLIProxyAPI@cb73cd99` 的空 bootstrap announcement、`CLIProxyAPI@b5ba02c2` 的大帧写入公平性、`codex2api@dc47d131` 的 input-item metadata 层级和 `codex2api@19ee8db4` 的请求生命周期内存预算；完整 commit、文件路径与提交态 SHA-256 可由 `node scripts/audit/audit-codex-reference-delta.mjs --check-sources` 可选复核。默认审计、构建、测试、发布和运行时均不读取外部 checkout。

CX-N1 在 HTTP JSON、SSE、WS 与 Lite 共用的 Responses 分类器中，只把载荷语义为空的 `response.output_item.added`、`response.content_part.added` 和 `response.reasoning_summary_part.added` 视为 Lifecycle；未知类型、文本、工具参数、server operation、错误或任意其他可见字段继续 fail closed 为 Business。CX-N2 只删除 `input[*]` 对象顶层的 `internal_chat_message_metadata_passthrough`，根对象、content、arguments 和用户嵌套同名字段逐字保留，且转换幂等。

CX-N3 将上游 Responses WebSocket 拆成单 reader/single writer actor、有界读写队列和 32 KiB continuation frame；大上传期间双向 Ping/Pong 与下游取消均有冻结时限。写入已提交、peer terminal 与 stale cached socket 的 replay 边界保持保守：只允许原 Provider、原 Account、原 rail 的 pre-commit WS→HTTP fallback，已发送或已产生业务输出时不重放，也不切换身份或站点。

CX-N4 为每个 Codex HTTP 请求或 Responses WebSocket turn 建立独立 sticky budget，统一计入 raw/decoded/normalized body、响应 wire/decoded body、transport pending、semantic prelude、tool arguments、normalized event 及 WS read/write queue。重试共享原预算；耗尽立即取消/丢弃当前上游，禁止继续 HTTP replay、账号/Provider/rail/site fallback，并返回稳定 HTTP 503（`Retry-After: 1`、`cc_switch_request_memory_exhausted`）或 WS error 后 Close。reservation 与最终 `Bytes` owner 绑定，terminal、cancel 和最后一个 body view drop 后释放；指标只含低基数组件、结果、字节数、limit 与 high-water，不记录内容、凭据或身份。默认 256 MiB，可由 `requestBodyLimits.memoryBudgetMb` 或 `CC_SWITCH_REQUEST_MEMORY_BUDGET_MB` 在 16～1024 MiB 内配置。

上述四项均为本仓库 `fixture_verified`。26 个 Codex WebSocket 回归覆盖 ping、公平写入、取消、handshake fallback、stale socket 与零 post-commit replay；压缩膨胀、raw+decoded+normalized 合计、稳定错误和 `Bytes` 生命周期另有专项测试。它们不构成真实 ChatGPT entitlement、生产网络性能或任何仍为 CX-03/CX-05/CX-06 `live_pending` 能力的 receipt。

`069f3ef` 将 Codex 429 scope、reset 解析和 capacity retry/error 决策收敛到 `src/proxy/providers/codex/`，`forwarder.rs` 继续持有 dispatch、共享总 attempt budget、CommitGuard、terminal 和 usage 编排。该结构切片不改变 wire，也不改变固定 Provider/Account、同账号 pre-commit 恢复和零 post-commit replay 边界。私有 receipt harness 将真实验收拆成 `gpt_image_2_5`、`gpt_image_2_5_flare`、`gpt_image_2_5_sunburst`、`ws_prewarm` 四个互不外推的 operation；前三项分别要求 exact-model generation/edit/Responses、usage/error/cooldown、capability、多副本和 Cloudflare 证据，WS 项要求上游接受、至少五个 benchmark sample、P50 TTFB 收益、连接复用和严格 pre-`response.create` WS→HTTP fallback。付费 `codex-images-real.mjs` 仅为 `probe_only/live_pending`，不能生成验收 receipt。

后续 EVID-N1 迁移把 Codex 差异资产提升为 append-only schema v2。原 schema v1 的 `policy`、`sources`、`capabilities`、`incrementalEnhancements`、`realAcceptance`、`wireGoldens` 六个字段分别由不可变 digest 固定；新增的两个干净只读 snapshot 为 `CLIProxyAPI@b773607e`（tree `a740e14d`）与 `codex2api@de41a5e3`（tree `b767333a`）。9 个 source delta 由 13 条不可变 observation 完整映射到 adopt、differential、live_gate 或 reject 处置以及 committed target baseline/implementation object；CX-R1 明确拒绝商业计价、自动 driver-model fallback、账号池、轮换和跨账号恢复。audit 默认只读取本仓库及其已提交 Git object，显式 `--check-sources` 才复核外部已提交对象。当前没有真实凭据，四个 operation 的 `receipt` 均为 `null`，状态保持 `live_pending`。

2026-10-08 的 Codex 私有验收绑定复核没有引入新的外部 wire 事实：推荐 `CLIProxyAPI@0f96f568`、`codex2api@b1bfe1b4` 的已提交状态不变。目标 `/v1/models?client_version=...` 实际按签名 Router Share 选 Provider/Account，但旧 `gpt_6_astra` 与 `ws_prewarm` validator 请求不声明预期 Share/Provider；拥有别的 Share token 时，即使别的账号 manifest 包含同名模型，也无法证明所选账号的 entitlement。两项请求缺参数的回环测试先红；现 validator 传 `expectedShareId/expectedProviderId`，Server 在出站前比对签名 Share 和实际选定 Provider，不匹配即冲突，普通客户端不受影响。这只是绑定证据门槛，不证明 Router token、部署 commit 或真实上游 entitlement；五份真实 receipt 仍分别缺失，保持 `live_pending`。

同日 Codex Astra/WS 私有验收再核对普通目录：两项 manifest 已声明预期 Share/Provider，但前置 `/v1/models?app=codex` 仍只带普通 `providerId`，而生产目录由签名 Router Share 选 Provider。两项各自的回环断言先红，现普通目录与 manifest 均发送 `expectedShareId/expectedProviderId`；Server 的签名目录路由定点测试覆盖匹配成功与错 Share/Provider 冲突。Codex receipt 专项 51/51。公开 [OpenAI Docs 的 List models](https://developers.openai.com/api/reference/resources/models/methods/list) 描述的是平台当前可用模型及其基本信息，并不证明这里的 ChatGPT 订阅账号 entitlement 或 Router 绑定。此项不改变生产 HTTP/WS/Image wire、manifest 上游查询、receipt schema v1/revision 1 或三项 Images disabled 决策；五份真实 receipt 仍分别缺失，新增 JS/文档未提交，不能补伪 committed-target observation。

## 2026-09-18 Antigravity grounding, model capability, and conversation-edge freeze

本轮增量证据追加在 `assets/contract/antigravity-reference-delta.json`，不改写 2026-09-11 的历史 observation。只读来源为 `CLIProxyAPI@ef63d2e7/@7fcbdf88/@a9e92b81/@b681a1e0/@8c984672/@fd3e6623` 与 `Antigravity-Manager@734e2bde/@9fd77989`；每个完整 commit、路径和提交态 SHA-256 均由 `scripts/audit/audit-antigravity-reference-delta.mjs --check-sources` 可选复核，默认构建、测试和运行时仍不读取外部仓库。

后续 EVID-N1 迁移把该资产提升为 append-only schema v2：原 schema v1 `sources`、`capabilities` 和 file digest 原样保留，另追加 source commit/tree、观察时工作树声明、路径/符号、处置、target baseline、implementation commit 与 fixture 映射。audit 内固定历史 observation digest，默认只检查仓库内资产和目标合同；`--check-sources` 才读取外部已提交 Git object。`e5bfc34` 仅迁移 Antigravity lifecycle 编排并增加两条 rail 的私有 receipt validator，没有改变下述 requestType 决策，也没有把 fixture 提升为真实厂商证据。

Server 的独立 grounding 合同只读取选中候选，按 URL 去重 web chunk、忽略无效索引，并将 Gemini UTF-8 byte range 映射为下游 Unicode scalar range。非流式与流式分别生成 Claude `server_tool_use`/`web_search_tool_result`/citation、Responses `web_search_call`/`url_citation` 和 Chat annotation；搜索事件、文本、citation 与 terminal 的顺序由自包含 fixture 冻结，citation 不得晚于 terminal，URL/title 不进入日志或指标。ASCII、中文、emoji、组合字符、重复/缺失 chunk、跨增量 part 和只在最终快照出现 annotation 均有回归测试。

模型搜索能力只接受目录中的 `supportsWebSearch`、`supports_web_search`、`webSearchSupported` 或 `nativeCapabilities.webSearch` 布尔值；冲突或缺失为 `Unknown`。证据以 Account `authIdentityGeneration`、TTL 和有界 `model_search:*` dimension 持久化。请求开始时固定使用当前 Account snapshot：请求模型有新鲜 `Supported` 才保留；`Unsupported`、`Unknown`、缺失或过期时使用已验证的 `gemini-2.5-flash` fallback；fallback 被当前目录明确标为 `Unsupported` 时失败关闭。运行时绑定代际漂移在发网前冲突，不重新选择账号、Provider、rail 或站点。

转换边缘合同同时冻结为：billing metadata 只有在 trim 后为单独一行且以精确前缀 `x-anthropic-billing-header:` 开头时才从 system block 删除，多行正文、消息和工具参数不受影响；只有开头连续 system/developer 提升为顶层 system，中途指令按原顺序包装为 `<system-reminder>`；孤立、错配或部分工具输出以版本化 user 文本可逆保留，不伪造 pairing；空 Gemini text part 不关闭活动 Claude text block。

`requestType` 的两个成熟参考结论互相冲突，因此 AG-N6 没有生产行为变更。当前合同继续固定普通文本、仅 function tools 和历史 functionCall/functionResponse 为 `agent`，含 web search（包括 function + search 混合）为 `web_search`。`antigravity_oauth` 与 `agy_oauth` 均无真实 receipt，故该决策和 compaction 继续为 `live_pending`，fixture 不得升级为厂商接受性证据。

## 2026-09-18 Claude rate-limit, reasoning replay, and terminal freeze

Claude 增量证据追加在 `assets/contract/claude-reference-delta.json`。只读来源为 `CLIProxyAPI@44eaef00/@75ce6352/@377c315f/@2bcebaa8/@7c32971b`；实现与测试 Git object 的提交态 SHA-256 可由 `node scripts/audit/audit-claude-reference-delta.mjs --check-sources` 复核，默认构建、测试和运行时不读取外部 checkout。没有采纳账号池、跨账号/Provider fallback、credential cloaking、organization-hash 身份迁移或浏览器指纹模拟。

后续 EVID-N1 迁移把该资产提升为 append-only schema v2：原 schema v1 `sources`、`localContracts`、`realAcceptance` 分别由不可变 digest 固定，新增 `sourceExtensions`、两个 source snapshot；当前追加到 15 条 observation。冻结 snapshot 为 `CLIProxyAPI@b773607e`（tree `a740e14d`，工作树干净）与 `OmniRoute@02c663cd`（tree `25b36e49`，未提交改动明确排除）；13 个 source delta 均绑定完整 commit、tree、路径、符号与 source digest。audit 默认只从本仓库已提交 Git object 核对 target baseline/implementation tree 和 anchors，显式 `--check-sources` 才读取外部已提交对象。

15 条 observation 分别记录 adopt、differential、live_gate 或 reject 处置，并映射 CL-01～CL-06、CL-N1～CL-N4、CORE-N1、CORE-N2、LIVE-N1 与 CL-R1。`CLIProxyAPI@fc96a87f` 的 organization-hashed credential filename 只作为拒绝证据：Account identity 继续由本仓库显式绑定，CL-R1 不声明 implementation commit 或 fixture，避免未来把 credential migration 当成待实现缺口。

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

## 2026-09-28 GPT-6 Astra differential extension

本轮只读复核 `codex2api@9368ed82e039ac8458f9c264b2298b47d0cbf833`；推荐的 `CLIProxyAPI/` checkout 当前缺失，因此没有把其 2026-09-19 冻结快照外推为最新状态。参考主线已将 `gpt-6-astra` 放入冷启动模型目录，并用最终模型、Responses Lite 与原生 WebSocket 三重条件决定 structured-output 字符串长度约束。Server 独立加入 Astra 的 text/image、max reasoning、priority、Lite 冷启动能力，账号 manifest 仍按 Account identity generation 隔离并逐字段覆盖，静态目录不声称 entitlement。

Astra 的 `usage_limit_reached` 继续按 Account 额度处理，只有缺少账号额度证据的 model-capacity/模型限流进入 Share/runtime/model cooldown；新增有界 Prometheus 标签用于积累真实作用域证据。参考仓库提交 `37049414` 的 Prism 独立额度旁路已由 `600636c8` 完整回退，且其 Responses、工具、commit 和 fallback 合同不完整，Server 明确拒绝迁移。`gpt_6_astra` 私有 receipt gate 要求精确模型、manifest、HTTP/SSE/WS、max、Lite、structured output、custom tool、usage、两类 429 和兄弟模型对照；真实输入缺失时保持 `live_pending`。

## 2026-09-18 Cursor OmniRoute differential refresh

Cursor 差异合同冻结在 `assets/contract/cursor-reference-delta.json`。2026-09-18 从旧点 `a3ca33fa6442b59adc42976c795709eaf5351109` 复核到 OmniRoute `02c663cdd0e8577bdcf2b01a44046bcd46dc6a7a`：六个 Cursor protobuf/session/executor 提交对象的 SHA-256 均未变化，提交区间没有 Cursor wire、protobuf、auth 或 session 增量，因此 CUR-N2 结论为 `reviewed_no_wire_delta`，不修改生产 executor。参考仓库当时 22 项未提交/未跟踪内容全部排除。默认审计不读取外部仓库，只有人工运行 `node scripts/audit/audit-cursor-reference-delta.mjs --check-sources` 才会以冻结路径和 SHA-256 复核 object，外部 Node/Electron/SQLite/session UI 从不成为构建或运行时依赖。

EVID-N1 将合同提升为 append-only schema v2：原 v1 的 `capturedAt`、`policy`、`sources`、`incrementalReview`、`registryTruth`、`capabilities`、`enhancements`、`providerLifecycle`、`realAcceptance`、`protobufFixtures` 十个字段分别由 canonical SHA-256 固定；新增的 OmniRoute source snapshot 绑定 HEAD commit/tree，并明确记录 `worktreeClean=false`、22 项工作树内容全部排除。当前 8 条不可变 observation 分别映射 CUR-01～03、CUR-N1/N2、CORE-N1、CORE-N2、LIVE-N1，固定 source path/symbol/digest、处置理由及本仓库 committed baseline/implementation object 和 fixture，不允许用当前工作树冒充历史实现。新增 `evidenceExtensions` 只记录 `CORE-N2-CURSOR=fixture_verified`，不改写十个 legacy 字段。

Server 自包含 hex fixture 固定 ServerConfig 和 interaction 的未知字段语义、重复 field 27/URL 失败关闭、Connect frame 任意分片与 partial EOF、成功/错误 terminal envelope、plain EOF 失败关闭，以及 fresh `composer-2.5-fast` 必须保留完整 wire ID。公开模型选择入口已经存在，因此 registry `special.cursor` revision 4 将 discovery 与 forward/test 一并标为 supported/`fixture_verified`；OAuth 返回静态 aliases，API-key 目录保持 exact Provider/runtime/credential scope，成功空目录权威，transient stale 只用于展示。

CUR-02/CUR-N1 仍是双 rail 真实证据缺口。`scripts/smoke/cursor-real.mjs` 每次固定一个 rail、Provider、Share 和 credential identity，只接受仓库外权限受限的私密 receipt；公开输出不包含这些标识。loopback 测试只产生 `contract_verified`/`live_pending`，OAuth 与 API-key receipt 不得互相推导，恢复也不得切换 rail、Provider 或 Account。

CORE-N1 切片以 `src/proxy/providers/cursor/` 作为共享 forwarder 与既有 `src/proxy/cursor/` 协议实现之间的生命周期 facade，收敛 Cursor adapter、模型选择、native driver dispatch 和 h2 timeout mapping，不改变 endpoint、protobuf、session、wire、attempt、terminal 或 usage。LIVE-N1 同时把 receipt 提升为 schema v2/harness revision 2：OAuth 与 API-key 仍独立，每份私有 receipt 必须绑定当前 target commit、App、Provider/runtime revision、Share revision、精确 credential generation、完整 `*-fast` model、22 项检查、10 份 body hash、5 项测量、固定恢复决策和 decoy/secret scan；真实文件必须在仓库外且权限为 `0600`。当前未提供真实凭据，两条 rail 的 receipt 仍为 `null`、状态保持 `live_pending`。

CUR-N2 的 `reviewed_no_wire_delta` 结论保持不变：冻结区间没有新的 Cursor wire 增量。后续 `3e0f5ff` 是独立的 CORE-N2 生命周期容量切片，以同一 committed reference 中已经存在的 frame/session 有界信号作为 differential 证据，不把它误写成上游新增协议，也不改变双 rail 的真实 receipt 状态。

2026-10-08 的双 rail 模型目录验收绑定复核不引入新的 OmniRoute wire 事实：推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 Cursor 路径仍无增量，22 项未提交 Codex 改动继续排除。目标 `/v1/models` 本来就以 Router 签名 Share 选 Provider，查询中的 `providerId` 不改变此选择；旧 Cursor 私有验收器只声明 `app/providerId`，两条 rail 均无法确认目录实际属于其 receipt 中的 Share。OAuth/API Key 两项缺少预期绑定参数的回环断言先红。现目录请求带可选 `expectedShareId/expectedProviderId`，Server 比对签名 Share 与所选 Provider，不符即在目录工作前返回冲突；普通客户端不受影响。这个栅栏不将 OAuth 静态别名提升为账号 entitlement，也不替代 API Key 的精确凭据作用域、真实厂商请求或两份独立私有 receipt；两 rail 仍 `live_pending`。

2026-10-08 的 Cursor API Key receipt 时效续查只涉及目标验收器，不引入新的 OmniRoute wire。原验收在读取目录时检查一次 `cursor_public_api` / `stale=false` / 五分钟 TTL，随后解析 receipt 并末次重读控制面；若目录在这期间过期，仍会接受。回环 fixture 将目录设为距 TTL 还差一秒，并延迟末次 Share 响应 1.5 秒，旧流程错误输出 `contract_verified/live_pending`。现复用同一时间戳谓词，在末次绑定复核后、接受 receipt 前再检一次；反例转为脱敏失败，Cursor 专项 Node 43/43。OAuth 静态目录不套动态 TTL；生产 protobuf/session/目录发布、receipt schema v2/revision 2、真实 rail 状态均不变。这仍不是跨 API 原子快照，工作树未提交也不能产生 committed-target observation；未做 UI 自动化或部署测试。

2026-10-08 的 Cursor 双 rail receipt 时效复核只修私有 validator，不新增 OmniRoute wire 事实。OAuth、API Key 各将 `recordedAt` 设为距 24 小时上限 1.2 秒，延迟末次 Share 响应 2 秒；旧流程两例均在 receipt 已过期后错误接受。现在内容校验和末次绑定读取后共用 receipt 时间谓词，API Key 目录与 receipt 还以同一接受时刻复核；两例均脱敏失败关闭。OAuth 静态别名不套动态目录 TTL，生产 protobuf/session、receipt schema v2/revision 2 和两条真实 rail 状态均不变。末次读取不是跨 API 原子事务，fixture 不构成厂商接受性证据。

2026-10-08 的 Cursor 双 rail 私有 receipt 文件快照复核只修目标验收器，不新增 OmniRoute wire 结论。OAuth、API Key 各在首次 `readSync` 返回有效字节后、EOF 前将同 inode receipt 等长原位覆盖并改变 mtime；旧流程两例均错误输出 `contract_verified/live_pending`。现在同一只读描述符在读取前后比较 dev/inode、nlink、size、mode 与纳秒级 mtime/ctime；两例在任何控制面请求前脱敏失败。此门槛不能证明元数据复原或精确并发写入下的原子快照，也不证明真实 Cursor 账号 entitlement；生产 protobuf/session/目录发布 wire、OAuth/API Key 身份及 receipt schema v2/revision 2 均不变，两份真实 receipt 继续 `live_pending`。当前 JS 未提交，不生成新的 committed-target observation；未做 UI 自动化或部署测试。

2026-10-08 的 Cursor 最终文件身份复核仍只修本仓库私有 validator，不新增 OmniRoute wire 事实。OAuth 与 API Key 各在首次控制面读取时把已快照的有效 receipt 路径替换为另一 inode 的无效文件，或将原 inode 等长原位覆盖并改变 mtime；旧流程四例全部错误接受旧快照。现保存读取后的 bigint 文件指纹，在最终 Account/Provider/Share、API Key 动态目录和 receipt 时效复核后重新解析配置路径，并比较 dev/inode、链接数、大小、权限及纳秒级 mtime/ctime；四例均脱敏失败，正常双 rail 与既有目录/绑定反例通过。末次检查不是跨 API/文件系统原子事务，元数据复原及检查后的变化仍可能逃过此门槛；OAuth 静态别名没有变成 entitlement。Cursor 专项 Node 54/54、全量 audit Node 657/657、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，真实 receipt 仍缺；当前 JS 未提交，不以旧 committed-target observation 冒充新实现证据。未做 UI 自动化或部署测试。

2026-10-08 的 Cursor 双 rail 最终内容复核仍只增强私有 validator，不新增 OmniRoute wire 事实。OAuth、API Key 各在首次控制面请求时等长改写已快照的 receipt，再用对抗 fixture 让末次元数据读取返回原指纹；旧流程两例错误接受。现在末次通过 `O_NOFOLLOW` 只读描述符比较完整原始字节与 EOF，复核读前读后指纹及配置路径，两例均脱敏拒绝。fixture 的元数据伪装不代表普通用户能恢复真实 `ctime`；检查后的竞态、跨 API 原子性和真实厂商接受性继续待证。生产 protobuf/session/目录发布、OAuth/API Key 身份和 receipt schema v2/revision 2 不变，两份真实 receipt 继续 `live_pending`。

2026-10-09 的 Cursor 双 rail Share 绑定复核只修本仓库私有 validator，不新增 OmniRoute 或厂商 wire 事实。生产 Share 的原始 `bindings` 已包含主 App 条目，导入不变量要求每 App 至多一条、主字段与该条一致；旧验收器却另拼主字段再用 `some` 查找，也未检查启用状态。OAuth/API Key 各构造重复冲突绑定、主字段冲突、disabled、paused 四类反例，旧逻辑八例全部错误输出 `contract_verified/live_pending`。现要求所选 App 在原始 `bindings` 中恰好一条精确匹配，Share 主字段与其对应唯一条目一致，且 `enabled=true/status=active`；八例先红后绿，正常双 rail 和 API Key 动态目录 fixture 保持通过，错误在目录请求前拒绝。Cursor 专项 Node 66/66、全量 audit Node 800/800、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；两份真实 receipt 与当前工作树提交态 observation 仍缺。生产 Cursor protobuf/session/目录 wire、Share 路由、OAuth/API Key 身份和 receipt schema v2/revision 2 不变；控制面复核不是跨 API 原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

2026-10-09 的 Cursor 双 rail 控制面 revision/fingerprint 复核仍只修本仓库私有 receipt validator。推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 HEAD/tree 未变；参考工作树 22 项未提交 Codex 改动继续排除，CUR-N2 保持 `reviewed_no_wire_delta`。目标 Provider 投影同时给出顶层 `revision` 与 runtime `providerRevision`；旧验收器允许任一缺失时取另一份、两份冲突时优先 runtime，也会把双缺失或 Share revision 缺失补成 `0`，并接受空白 fingerprint。OAuth/API Key 各构造缺 runtime revision、缺顶层 revision、两份不一致、两份均缺、缺 Share revision、空白 fingerprint 六类自洽 receipt 反例，旧流程十二例全部错误输出 `contract_verified/live_pending`。现在要求两份 Provider revision 都是非负安全整数且相等、Share `configRevision` 明确为非负安全整数、`runtimeFingerprint` 为非空白字符串；十二例均在目录发网前脱敏拒绝。Cursor 专项 Node 79/79、全量 audit Node 946/946、smoke Node 13/13；Cursor reference-delta `--check-sources`、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。两条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，两份真实 receipt 与当前工作树提交态 observation 仍缺。生产 Cursor protobuf/session/目录 wire、OAuth/API Key 身份及 receipt schema v2/revision 2 不变；控制面复核不构成跨 API 原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

## 2026-09-18 Grok reasoning replay/root-union/quality-observation differential freeze

Grok 差异合同冻结在 `assets/contract/grok-reference-delta.json`。一次性只读证据只取 `grok2api@906b9493b099d192381c698d4e320fafeccb851c` 的十个已提交 Git object，并以 SHA-256 固定 conversation reasoning cache、明确 decode rejection recovery、Build tool root-union adapter，以及 `7f3f3d3ce030d5cf946b0bbe994f3026775fa308` 引入的启发式 quality hold/retry；历史 reasoning delta 仅记录 `8641a782`、`ca392e68`、`7d1b4246`、`3de758e7`、`22ac653a`、`72a3a347`、`5d19ccff`、`e5285ebe`。外部工作树未提交内容和 `sub2api` 的账号池、路由、fallback 都被排除；默认审计不读取外部仓库，只有人工运行 `node scripts/audit/audit-grok-reference-delta.mjs --check-sources` 才复核只读 Git object。

EVID-N1 将合同提升为 append-only schema v2：`269850a` 中原始 schema-v1 文件的 SHA-256、target commit/tree、全部十个历史字段的 canonical digest 和逐字段 digest 均被冻结。新增的 `grok2api@906b9493` 干净 snapshot 与 `sub2api@ab99d56e` dirty-worktree-excluded snapshot 固定提交态 tree；后者的 6 项本地修改全部排除，只追加四个已提交 object 作为拒绝证据。当前 11 条不可变 observation 完整映射 GR-01～05、GR-N1、CORE-N1、CORE-N2、LIVE-N1 与 GR-R1，绑定 source path/symbol/digest、处置理由以及本仓库 committed baseline/implementation object。GR-R1 明确拒绝 visible/plaintext 启发式重试、账号池/轮换、商业复合路由/计价和本地 soft quota gate；reject 记录不声明 implementation 或 fixture。

Server 的独立实现把 replay scope 固定到 Provider revision/runtime、Account auth/token generation、Share、签名用户、session/turn、model family、HTTP/WS rail 和 upstream plane；有界 cache 使用 CAS/tombstone，只从成功 completed 终态提交，并在重复/编辑 call、协议/容量/代际漂移时 fail closed。Grok Build root union 在通用 sanitizer 后解析有界 local `$ref`，只投影可证明的 object root，循环、混合 union、全非 object 与歧义 schema 在网络前明确拒绝。HTTP、CRLF/分片 SSE 与 WebSocket loopback 验证 capture/replay、parallel calls、一次 pre-commit 明确拒绝恢复及 post-commit 禁止恢复。

GR-N1 只吸收“需要观测响应质量形状”这一事实，不采用参考项目的 hold、最多六次尝试、跨账号轮换、推理 token 比例或密文长度启发式。Server 在成功 HTTP JSON、SSE 与原生 WebSocket Responses 上用有界状态机记录固定 outcome、terminal/tool/visible 布尔值和 `0|1_7|8_31|32_127|128_plus` 长度桶；不保留 plaintext/reasoning，不写 Provider、Account、Share、用户、prompt 或模型标签。单个至少 1024 字符的流式 bulk fragment 仅记为 `anomalous_dump` 诊断。观察器逐字节透传且没有 retry/rotation/response rewrite 决策入口；失败/不完整终态不冒充质量样本。

CORE-N1 将 Grok reasoning replay 的 scope 派生、snapshot ownership、CAS 清理/提交以及 Provider/Account/Share generation fence 收敛到 `src/proxy/providers/grok/`。共享 forwarder 只保留 wire 编排并调用 HTTP/WS facade；固定 Provider/Account/Share、同账号一次 pre-commit 明确拒绝恢复、共享 attempt/10 秒预算和零 post-commit replay 均未改变。

LIVE-N1 把真实门禁拆成 `inference`、`media`、`remote_compaction` 三个互不外推的 operation。每份仓库外 `0600` 私有 receipt 必须绑定当前 target commit、Provider revision/runtime、Account auth/token generation、Share revision、签名用户 namespace、精确 model/session/turn 和 fresh catalog，并精确匹配 checks、body hashes、measurements、固定恢复决策、零 decoy 请求与 secret scan。`grok-oauth-real.mjs` 仅为 `probe_only/live_pending`；fixture 只能产生 `contract_verified/live_pending`。当前没有真实 receipt，三项继续为 `receipt=null`、`live_pending`。

这些 fixture 只支持 GR-01..03、CORE-N1 与 CORE-N2 的 `fixture_verified`。GR-04 的真实 inference/media/WS/version/cooldown/catalog 仍待 operation receipt；GR-05 remote compaction 明确 `runtimeEnabled=false`，即使未来 receipt 证明上游协议也不能自动启用，必须另行完成 scope、versioned AEAD、TTL、失败语义和降级设计评审。不得借用 Grok Web Cookie、跨账号 cache 或外部商业路由。

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

## 2026-09-23 Claude Code 2.1.280 / Opus 5.5 wire profile freeze

当前 active profile 升级为官方 npm latest `@anthropic-ai/claude-code@2.1.280`。一次性只读审计下载了官方 `@anthropic-ai/claude-code-linux-x64@2.1.280` 包，Linux x64 binary SHA-256 为 `1e08503dbdf3c2cb0d706d32f3408277388d1c76ef108673e8fe42c1b322925b`。静态字符串与嵌入 model catalog 确认 Node `v26.3.0`、Stainless `0.112.1`、Axios `1.15.2` 保持不变，并确认 `claude-opus-5-5` 的 1M context、128K output、adaptive thinking、mid-conversation system/tool changes、per-turn effort/timing 与五档 effort。审计没有使用真实 token、没有访问 Anthropic inference endpoint，也没有保存原始请求或私有 build metadata。

官方 binary 同时确认九个 2.1.280 beta 名称；其排序和 body/model gate 以只读 `CLIProxyAPI@2430354330af80b645f9ffb1a51e1e7c72c4cc8e`、`@779bf317e03032c255eede61c22563f306fbd67e`、`@bd584a7523297f59b6f5dc5381a4865f34e25284` 交叉核对。`@ed751ea08af812758282e855ec6d8f7a49f6a7b3` 提供 fallback-credit gate 的差分证据，`@1cce9325738f5857cc676317abe8a8bf03086c32` 提供 overage/Fable-only 429 忽略通用 Retry-After 的差分证据；外部源码仍不进入构建、测试、发布或运行时。active billing `ping` fingerprint 为 `d7b`，active CCH golden 为 `daec4`；2.1.258 的 `1e2` / `8d393` 作为历史回归向量保留。

本地 fixture 状态为 `fixture_verified`。Opus 5.5 的 adaptive/rejects-disabled thinking 也由同一 capability registry 驱动 OpenAI ingress reasoning 转换；显式关闭 reasoning 会生成合法的 adaptive/low 请求，而不是旧式 budget 或 `thinking.type=disabled`。`assets/contract/claude-reference-delta.json` 以 append-only `realAcceptanceExtensions` 增加独立 `opus_5_5` receipt；在真实订阅账号完成精确模型、Messages JSON/SSE、count_tokens、thinking、tools、per-turn controls 与小请求 1M capability acceptance 前，状态保持 `live_pending`。该 gate 禁止模型 fallback、跨账号或跨 Provider 请求，也不以离线 model catalog 推导真实 entitlement。

`sub2api` commit `34b8bf1a6` 只作为 Fable 5.1 目录和 billing fingerprint 概念的交叉证据；其 Go 实现按 UTF-8 byte 取索引，不能覆盖官方 JavaScript UTF-16 语义。审计时 TokenRouter/sub2api 仍广告 Claude Code `2.1.220`、Stainless `0.94.0`，且二者关于取消 CCH 的判断与当前官方 binary 不符，因此都没有作为版本、identity、CCH 或 beta 的实现来源。吸收范围不含其多账号号池、账号切换或 fallback 设计。

2026-09-03 对 TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 与 sub2api `6566039bc81e8a9af94077cb272eb3d3074702dd` 做了后续一次性只读差异审计。两者的 `backend/internal/service/ratelimit_service.go` 和 `account_usage_service.go` 交叉确认：Anthropic Messages 响应头会提供独立的 `5h`、`7d`、`7d_oi` utilization/reset，其中 `7d_oi` 是 Fable 周容量窗口；主动 usage 缺失该窗口时，可以用同账号成功推理得到的被动样本补齐显示。Server 只吸收这组有限 header 名称和“主动值优先、被动值补缺”的协议事实，并独立加入数值/时间范围校验、Account identity generation 隔离、单调合并、TTL/reset 过期和 Fable entitlement 门控。没有采用外部项目的 Extra map 存储、预测窗口、调度阈值、自动暂停、账号选择、号池或 fallback 行为；普通 utilization 也不会写入仅表示已耗尽的 `capacity_pool_limits`。

离线证据只能支持 `fixture_verified`。真实 Max 5x/20x inference、Fable 5.1 entitlement、streaming、限流和版本门禁仍为 `live_pending`，必须使用已轮换且不进入日志/命令历史的私密凭据按 acceptance runbook 验证后才能升级结论。

## 2026-09-02 Qoder CLI oracle freeze

`qoder_cosy` 的 native Rust 实现以一次性、只读的官方 CLI 审计作为漂移 oracle，不在构建或运行时加载 CLI。证据冻结于 `assets/contract/qoder-cli-oracle.json`：Global `@qoder-ai/qodercli@1.1.32` bundle SHA-256 为 `24de5b12520cbe49c0027b53654eaee02bddd857e3d9f19a6198824e365d89bf`，CN `@qodercn-ai/qoderclicn@1.1.32` bundle SHA-256 为 `5a82eeffbeb015d78c4945b7f4ed989494d2ea8cc7fdf2dbfc6ad04c17418f8b`。`cli2api` commit `9b18f2de06c53f12bf2c5112c7a71e3e64755b97` 仅提供带文件摘要的 capture/plaintext projection 交叉样本，不是依赖、同步源或生产 executor。

2026-09-11 将已落地的动态 entitlement/capability、三 rail、严格 EOF terminal、generation fencing 与同账号单次恢复统一确认为 `special.qoder_cosy` Driver contract revision 2。`assets/contract/qoder-reference-delta.json` 与对应审计把 Registry、三个 Profile、oracle、coverage、生产入口和 `live_pending` 状态交叉闭合，并冻结“先官方包 digest/独立 wire，后 oracle mutation，最后 Rust”的 CLI 升级顺序。TokenRouter 当前文档提交 `3488b4a9208c41e4f9db4108ef5133cb3710648c` 仅作为可选只读 source audit 输入。

2026-09-18 的 QD-N2 只读复核将 TokenRouter 提交态推进到 `7faf9469bc6957716923b5b4a98665c0fb9715e0`。两份 Qoder 文档对象摘要未变；`qoder_gateway_handler.go` 的净变化仅适配共享 error helper 新增返回值/参数，冻结 diff 摘要为 `384062fd869020540b945f654179f250ad75650f3fb5427c7e355b710b824eb2`，没有 Qoder origin、header、签名、payload、terminal 或恢复 wire 增量。因此官方 CLI oracle 继续优先，不修改 Server 生产实现。QD-N1 的 Global OAuth、Global PAT、CN OAuth receipt 仍三条独立 `live_pending`，任一 rail 或站点的成功不得继承给另一条。

2026-09-19 的 CORE-N1 只迁移 Server 内部所有权：`src/proxy/providers/qoder/` 统一持有 exact `qoder_cosy` Account binding、Share/用户/session conversation scope、live-catalog model/payload preparation、generation fence 与 Qoder throttle；COSY codec/runtime primitive、共享 attempt budget、usage、terminal 及是否同账号 pre-commit 401 replay 的责任保持不变，endpoint/header/signature/wire/recovery 次数没有变化。LIVE-N1 同时将三条 rail 的 happy-path receipt 提升为 schema v2，绑定当前 target commit/harness、Account auth/token generation、三个 Surface Provider revision/runtime digest、Share revision、site/rail、exact model、三份 fresh catalog snapshot 与六个请求 body hash。未实际执行的 login/rotation、权威空目录和受控两段 401 继续明确写为 `not_observed`，因此真实 happy-path receipt 仍只能是 `partial_live_verified/live_pending`，fixture 仍只能是 `contract_verified/live_pending`。

EVID-N1 随后将 Qoder reference delta 迁为 append-only schema v2。`2cc8a017dee0f5ca1ab4189ad87db35c391669ea` 中原 schema-v1 文件 SHA-256、target commit/tree、全部 11 个历史字段的 canonical/逐字段 digest 均已冻结；新增干净的 `TokenRouter@7faf9469bc6957716923b5b4a98665c0fb9715e0` / tree `b205061c4f0d9b53fbffb0242f34796e10b18251` committed-object snapshot，以及 11 条不可变 observation，完整映射 QD-01～03、QD-N1/N2、CORE-N1/N2、LIVE-N1 与 QD-R1～R3。三条 reject 分别固定账号池/轮换/affinity 和跨边界 fallback、商业计费/Key/渠道/维护调度/多租户后台，以及外仓运行时依赖、fixture 冒充 live 和缺唯一 terminal + EOF 仍成功。每条 observation 绑定 source commit/tree/path/symbol/digest 与本仓库 committed target object；默认审计不读取外部 checkout，只有显式 `--check-sources` 才复核冻结对象。

2026-09-19 的 CORE-N2 在 `348557182e1a513dac3cc2de45202b7371411349` 完成精确 `qoder_cosy` 生命周期内存预算。Global OAuth、Global PAT、CN OAuth 的同账号 pre-commit 401 recovery 与 Claude、Codex、Gemini 三个 Surface 全部复用原 sticky budget；canonical JSON、runtime/catalog、prepared payload、COSY plain/编码/签名工作集、headers、错误 body、SSE chunk、decoder/canonical event、非流聚合和下游 bridge retained state 均在分配或增长前核算。最终响应/流 chunk 的 reservation 跟随 owner；耗尽稳定为 `503 / cc_switch_request_memory_exhausted`、usage `memory_capacity` 和 `CapacityShed`，不 replay、refresh 或记成 `NetworkFailure`，已提交流只产生一次脱敏 terminal。三条真实 rail receipt 仍为 `null/live_pending`。

`QD-OBS-0011` 冻结 TokenRouter 同一 committed object 的 `backend/internal/pkg/qoder/client.go`、`backend/internal/service/qoder_gateway_service.go` 与 `backend/internal/service/gateway_service.go`。参考实现的控制面 JSON 有 4 MiB `LimitReader`，Qoder SSE scanner 有显式 line ceiling，但共享默认值是 500 MiB，非流路径还会把事件保留在数组中；这些只能证明 retained state 应有界，不能证明统一 request-lifecycle budget、COSY 编码/签名膨胀、下游 transform、sticky exhaustion 或 capacity 分类。`CORE-N2-QODER=fixture_verified` 因而绑定本仓库独立实现及 committed fixture，而不是把参考项目能力外推为本地 live 结论。

2026-09-28 的第二轮参考检查在 TokenRouter `6d676f1d100e11f2f068949f5779e960c862079c` / tree `4b830127433aa21c8534891526b3617bf90d2b80` 处触发仓库技能门禁：该 checkout 的指令要求 `project-doc`，当前会话未提供此技能，且工作树存在被排除的 `UD AGENTS.md`。`QODER-SKILL-GATE-2026-09-28` 因而只冻结仓库元数据，明确记录未读取业务文件、未做业务分析且未改生产代码；它不能被解释为 no-wire 复核。最近可信证据仍是 `tokenrouter-2026-09-19`，Global OAuth、Global PAT、CN OAuth 三条 rail 均继续独立 `live_pending`。默认 Qoder audit 可自包含复核这条门禁；本轮不以 `--check-sources` 重新访问受门禁 checkout。

2026-10-08 的 Qoder 本地验收复核不新增外仓 wire 证据：TokenRouter 最新业务对象仍因 `project-doc` 不可用而保持 `unreviewed_skill_gate`。目标 `scripts/smoke/qoder-real.mjs` 原先只在三 Surface 结束后重读 Account，且接受 token refresh generation 增长，不再复核三份 Provider 和 Share。Global OAuth、Global PAT、CN OAuth 各自的 Account token 代次、Codex Provider revision、Share revision 漂移共九个离线反例先证明旧流程会错误写 receipt；现在落盘前精确重读并比较 Account 两代 generation、Provider binding 与 Surface revision/runtime digest、Share revision/identity/binding digest，九例均失败关闭，三个正常 fixture 各资源读两次。Qoder 专项 Node 31/31、全量 audit Node 322/322、smoke Node 13/13；CLI oracle、自包含 reference-delta、Provider contract/coverage/UI matrix、文档索引、脚本语法与差分空白检查通过。末次读取不是跨 API 原子事务，未改变 COSY wire 或 receipt schema v2/revision 2；三 rail 的 `RUN_REAL=0` 仍为 `blocked_inputs/live_pending`，真实 receipt 缺失。工作树改动尚未提交，不能据此追加 committed-target observation，也未运行受门禁的外仓源码审计、UI 自动化或部署测试。

### Qoder 三 rail 私有 receipt 声明长度栅栏（2026-10-08）

`TokenRouter@6d676f1d100e11f2f068949f5779e960c862079c` 的 `project-doc` 技能门禁仍未满足；没有读取其最新业务对象或运行外仓 `--check-sources`。本项只修目标 `qoder-real.mjs` 的验收资源边界：原 `readLimited` 已逐块限制 8 MiB，但声明长度超限且仅发送少量内容、不结束的控制面响应会等待超时。Global OAuth、Global PAT、CN OAuth 三条 loopback 反例先各红灯（约 5 秒），现读取前按 `Content-Length` 拒绝并取消 body，均快速失败；Global OAuth 的无长度分块实际超限回归确认 reader 会被取消。Qoder 专项 Node 36/36、全量 audit Node 348/348、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix 通过。

生产 COSY wire、Account/Provider/Share 绑定、receipt schema v2/revision 2 不变。三条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；最新 TokenRouter 增量继续 `unreviewed_skill_gate`。默认 reference-delta 通过只证明已有冻结记录，未提交 JS 加固也不构成 committed-target observation。未做 UI 自动化或部署测试。

### Qoder 三 rail 私有 receipt 输出目录真实位置栅栏（2026-10-08）

最新 TokenRouter 业务源码仍受缺失的 `project-doc` 技能门禁约束；未读取其业务对象，也未运行外仓 `--check-sources`。本项只修目标 `qoder-real.mjs` 的私有证据输出路径：字面仓库外目录若通过符号链接指向仓库内，旧验收器会把 Global OAuth、Global PAT、CN OAuth 三个合法 fixture 的 receipt 写进仓库。三例先复现错误接受；现于控制面请求前解析父目录真实位置并拒绝仓库内目标，在实际写入前再次检查。三例均无控制面请求或文件落盘，正常 rail fixture 不变；Qoder 专项 Node 40/40、全量 audit Node 378/378、smoke Node 13/13，CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix 通过。

两次静态检查不保证目录不会在检查与写入之间并发替换，也不证明部署 commit 与本地 HEAD 一致。生产 COSY wire、rail/site 选择及 receipt schema v2/revision 2 不变。三条 `RUN_REAL=0` 入口分别为 `blocked_inputs/live_pending`，没有真实请求或 receipt；最新 TokenRouter 增量仍为 `unreviewed_skill_gate`。当前 JS 修改未提交，不新增伪造的 committed-target observation；未做 UI 自动化或部署测试。

### Qoder 三 rail 私有 receipt 排他发布栅栏（2026-10-08）

TokenRouter 仓库的 `project-doc` 技能在当前会话仍不可调用；本项未读取其业务文件、运行访问该仓的命令或补审最新 wire，参考增量保持 `unreviewed_skill_gate`。目标 `scripts/smoke/qoder-real.mjs` 原先先检查输出路径不存在，再写临时文件并以 `renameSync` 发布；若另一文件在检查后出现，POSIX rename 会覆盖它。Global OAuth、Global PAT、CN OAuth 三条 rail 分别用本地预加载钩子在临时文件写完后创建目标哨兵文件，旧流程均错误覆盖哨兵并报告 `contract_verified/live_pending`。现改用同目录硬链接排他发布：目标已出现则链接失败，不覆盖；仅清理本次实际创建的临时文件。正常三 rail receipt 保持 `0600`，且成功或冲突后均无残留临时文件。Qoder 专项 Node 44/44、全量 audit Node 416/416、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、脚本语法和差分空白检查通过。

这修复最终文件的同名覆盖竞态，不保证父目录在两次真实路径检查与写入之间不被替换，也不是跨 API 的原子验收。生产 COSY wire、Account/Provider/Share 绑定、receipt schema v2/revision 2 与 rail/site 选择均未改。三条 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`，没有真实请求或 receipt；默认 delta 审计不能代替最新 TokenRouter 源码补审。新增 JS 未提交，不以旧 committed-target observation 冒充本次实现证据；未做 UI 自动化或部署测试。

### Qoder 三 rail Account profile 作用域投影（2026-10-08）

本轮只审目标自身 Qoder 控制面与验收器。旧 `resolveAccount` 以 Account ID 站点前缀及 PAT/OAuth token 形状替代真实 profile 绑定；错站点、错 credential rail、缺投影三个回环反例各自先错误写出 `contract_verified/live_pending`。现在 `AccountPublicView` 仅在 `qoder_cosy` 的 profile 通过生产 `QoderAccountProfile::parse` 时发布 `qoderCredentialScope.site/credentialRail`，不返回 UID、machine ID 或 PAT；验收器必须与所选 rail/site 精确相符，旧服务缺字段失败关闭。三例转绿且正常三 rail fixture 保持通过。TokenRouter 最新业务源码仍受本会话缺失的 `project-doc` 技能门禁约束；没有读取其业务文件或运行访问该仓的 `--check-sources`，最新增量继续 `unreviewed_skill_gate`。

2026-10-08 的 Qoder 私有目录验收增量只来自本仓库已实现的签名 Router ingress：`/v1/models` 按签名 Share 与 App 选择 Provider，普通 `providerId` 查询不参与选择。Global OAuth、Global PAT、CN OAuth 的三 App fixture 对 `expectedShareId/expectedProviderId` 增加断言后各自先红，验收脚本传入精确预期绑定后 48/48 转绿；服务端已有签名目录路由定点测试覆盖匹配 200、错 Share/Provider 409。此项只提高 receipt 目录绑定门槛，不改 COSY wire、receipt schema v2/revision 2 或任何 rail 的 `live_pending`；最新 TokenRouter 业务增量仍未审，不能把离线目录成功解释为真实厂商验收。

2026-10-08 的 Qoder 三 rail 目录时效复核只涉及本仓库私有 receipt 发布器。Global OAuth、Global PAT、CN OAuth 各自让 Claude、Codex、Gemini 中一份目录在取得时距两小时上限 1.2 秒，并把末次 Share 响应延迟 2 秒；旧流程九例均在目录过期后错误写出 receipt。现在初次获取与写文件前共用同一时间谓词，三份目录有任一过期即拒绝发布，九例均不落盘。生产 COSY wire、目录 TTL、rail/site 选择与 schema v2/revision 2 不变；happy-path 即使通过仍只能 `partial_live_verified/live_pending`。TokenRouter 最新业务对象继续受缺失的 `project-doc` 技能门槛限制，未读取或补审，不把自包含离线审计当作最新外仓证据。

2026-10-08 的 Qoder 发布前时效补强又发现临时文件写入窗口：三条 rail 各让 Claude 目录在写入前距过期 1.2 秒，并在同目录临时文件写完后阻塞 2 秒，旧流程三例仍错误排他发布 receipt。现在保留准备写入前的早期检查，并在临时文件写入和权限设置完成、硬链接发布前再次使用同一目录时间谓词；三例均失败关闭且清理临时文件。此项仍只修私有证据门槛，不改变生产 COSY wire、目录 TTL 或 receipt schema v2/revision 2；检查与原子硬链接之间不构成跨 API/文件系统原子快照，三条真实 rail 仍独立 `live_pending`。最新 TokenRouter 业务源码继续受 `project-doc` 门禁，不以默认自包含审计冒充补审。

2026-10-08 的 Qoder 三 rail 私有 receipt 父目录交换复核只修本仓库发布器。Global OAuth、Global PAT、CN OAuth 各让仓库外符号链接父目录在临时文件写入前改指仓库内；旧流程三例均写进仓库并错误报告 `contract_verified/live_pending`。现以 Linux `/proc/self/fd` 锚定已打开的仓库外目录，临时文件与排他硬链接均使用该描述符路径；写入前、链接前、链接后复核配置路径与目录 inode/真实位置，漂移则失败，若链接已创建仅撤销本次链接。三例写入期交换、三例链接后交换均无仓库内文件或残留 receipt；正常三 rail 与原排他发布测试保持通过。此项不改变生产 COSY wire、site/rail/Account 选择或 receipt schema v2/revision 2；非 Linux 缺少描述符锚定时在控制面请求前失败关闭。目录 inode 被并发移动或末次检查后再漂移的精确竞态仍不保证原子性，三份真实 rail receipt 保持 `live_pending`。TokenRouter 业务源码仍因缺失的 `project-doc` 技能而未审，本项不冒充最新外仓 wire 证据，也未做 UI 自动化或部署测试。

2026-10-08 的 Qoder receipt 内容发布复核仍只修本仓库三 rail 私有发布器，不引入 TokenRouter wire 新结论。Global OAuth、Global PAT、CN OAuth 各在临时文件写完后等长原位改写、替换临时路径，或在硬链接创建后原位改写；旧流程九例全部错误报告 `contract_verified/live_pending`。另以三条 rail 的链接后最终路径替换反例确认：仅加内容校验而沿用旧清理方式，会误删后来占据路径的文件。现把预期完整 receipt 字节留在内存，在临时文件和最终硬链接上分别用 `O_NOFOLLOW` 描述符检查普通文件、链接数、大小、`0600`、dev/inode、纳秒级 mtime/ctime 与完整字节；链接后失败只撤销仍指向本次 inode 的最终文件，替换文件保持原样。十二例均失败关闭，正常三 rail、排他发布、父目录交换及时效 fixture 保持通过。检查与链接不是文件系统原子事务，元数据复原、目录 inode 并发移动和末次检查后的变化仍不由此证明，fixture 也不是厂商接受性。Qoder 专项 Node 83/83、全量 audit Node 694/694、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。三条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`，真实 receipt 及登录/轮换、权威空目录、受控两段 401 门槛仍缺。TokenRouter 当前 `AGENTS.md` 要求本会话不可用的 `project-doc` 技能，本轮未读取其业务文件、运行仓库业务命令或外仓 `--check-sources`，最新增量保持 `unreviewed_skill_gate`。当前 JS 未提交，不以旧 committed-target observation 冒充新实现证据；未做 UI 自动化或部署测试。

2026-10-09 的 Qoder 临时 receipt 所有权复核仍只修本仓库三 rail 私有发布器，不新增 TokenRouter wire 事实。Global OAuth、Global PAT、CN OAuth 各在临时文件写入后把该路径替换成另一 inode 的哨兵文件；旧逻辑虽拒绝发布，却在 `finally` 中按路径无条件 unlink，三例均误删后来占位的文件。现在以 `O_CREAT|O_EXCL|O_NOFOLLOW` 创建并保留读写描述符，通过同一描述符写入和验字节；临时路径只有在 dev/inode 仍属本次文件时才删除。三例先红后绿，替换者内容保持原样；正常三 rail、父目录交换、目录时效、排他发布和最终链接替换 fixture 均通过。`lstat` 与 `unlink` 之间仍非原子事务，不能保证恶意并发替换或真实厂商接受性。Qoder 专项 Node 83/83、全量 audit Node 728/728、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。三条 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt、登录/轮换、权威空目录与两段 401 证据仍缺，当前工作树未提交，旧 committed-target observation 不证明本轮实现。TokenRouter 的 `project-doc` 技能当前不可调用；读取其门禁文档的同一次命令误附带只读 `git status`、`rev-parse` 元数据检查，此后未读取业务文件或运行外仓 `--check-sources`，最新增量继续 `unreviewed_skill_gate`。未做 UI 自动化或部署测试。

2026-10-09 的 Qoder 三 rail Share 绑定复核只修本仓库私有 receipt 发布器，不新增 TokenRouter 或厂商 wire 事实。生产 Share 原始 `bindings` 已含主 App 条目，导入不变量要求每 App 至多一条且主字段一致；旧验收器另拼主字段再用 `some` 找 Claude/Codex/Gemini Provider，并把缺少 enabled/status 当作有效。Global OAuth、Global PAT、CN OAuth 各构造三 App 重复冲突绑定、主字段冲突、disabled、paused、缺 enabled、缺 status 八类反例；旧逻辑误接受其中十八例，原已拒绝的 disabled/paused 六例保持拒绝。现在要求原始 `bindings` 对三 App 各恰好一条精确匹配、主字段与其唯一条目一致、`enabled=true/status=active`；二十四例均在目录请求前失败且不发布 receipt，正常三 rail 与已有目录、时效、发布和绑定漂移 fixture 通过。Qoder 专项 Node 108/108、全量 audit Node 879/879、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、两个 JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt、登录/轮换、权威空目录、受控两段 401 和当前工作树提交态 observation 仍缺。生产 COSY wire、Share 路由、site/rail、receipt schema v2/revision 2 不变；fixture 不证明厂商接受性。TokenRouter 的 `project-doc` 技能不可调用，本轮未读取其业务文件或运行外仓 `--check-sources`，最新增量继续 `unreviewed_skill_gate`。未做 UI 自动化或部署测试。

2026-10-09 的 Qoder 三 rail 控制面投影与作用域复核只修本仓库私有 receipt 发布器。推荐 TokenRouter 的 `project-doc` 技能当前会话不可调用，按其仓库门禁未读取业务源码、未运行外仓 `--check-sources`；最新增量继续 `unreviewed_skill_gate`，本项不推导新 COSY wire。目标 Server `ProviderView` 实际为顶层 `revision` 与 runtime `providerRevision`，旧 Qoder mock 却给不存在的顶层 `providerRevision`；mock 改为真实形状后，Global OAuth、Global PAT、CN OAuth 正常 fixture 先全部失败。旧发布器从虚构字段回退成 `0`，可把真实 Provider 作用域误写入 receipt。三 rail 各构造 Claude/Codex/Gemini 三个 Surface 的缺顶层 revision、缺 runtime revision、两份不一致、仅有旧式虚构顶层字段、空白 fingerprint，另构造缺 Share revision，共十六类反例；旧流程四十八例均错误进入发布路径。现要求三个 Provider 各自的两份 revision 为非负安全整数且相等、三个 fingerprint 是非空白字符串、Share `configRevision` 明确为非负安全整数；正常三 rail 按真实投影恢复通过，四十八例在目录请求前失败且不落盘。Qoder 专项 Node 157/157、全量 audit Node 1106/1106、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；真实 receipt、登录/轮换、权威空目录、受控两段 401 和当前工作树提交态 observation 仍缺。生产 COSY wire、site/rail、Share 路由、receipt schema v2/revision 2 不变；控制面复核不是原子快照，fixture 不证明厂商接受性。未做 UI 自动化或部署测试。

这只是控制面真实身份作用域门槛，不修改生产 COSY wire、receipt schema v2/revision 2，也不是厂商接受性证据。Qoder receipt Node 48/48、Qoder Rust 76/76、全量 audit Node 451/451、smoke Node 13/13；全量 Rust 在默认测试线程栈下于一条 CodeBuddy 测试 stack overflow，以 `RUST_MIN_STACK=16777216` 重跑为 library 3287 passed / 1 ignored，其他目标 124 + 1 + 1 passed。Clippy `--all-targets -D warnings`、rustfmt、CLI oracle、默认自包含 reference-delta、Provider Phase 0/Server contract/coverage/UI matrix、文档索引、脚本语法和差分空白检查均通过。三个 `RUN_REAL=0` 入口各为 `blocked_inputs/live_pending`；Global OAuth、Global PAT、CN OAuth 三份独立真实 receipt 仍各为 `null/live_pending`。当前增量尚未提交，不能冒充 committed-target observation。未做 UI 自动化或部署测试。

两份官方 bundle 共同确认：Device authorization 使用 `/device/selectAccounts`、UUID v4 nonce 与 S256；poll 为 OpenAPI `GET /api/v1/deviceToken/poll`，1 秒间隔、300 秒 TTL、404 pending 且不发送 Authorization/COSY/User-Agent；refresh 为 OpenAPI `POST /api/v1/deviceToken/refresh`，只发送 JSON body `refresh_token` 与 `User-Agent: qoder/1.1.32`，响应主字段为 `device_token`、轮换 `refresh_token`、`expires_at`。Global 36 位小写 hex machine ID 与 CN UUID v4 machine ID 是独立站点事实。Qoder CLI `1.1.32` 和 COSY wire `1.24.2` 属于不同版本空间；旧 Global center job-token endpoint 不可作为 Device refresh fallback。

oracle schema v2 额外用审计脚本内的独立摘要冻结三 rail 的精确 origin、actual/signature path、Global/CN profile、完整 signed-header 集、encoding/signature vectors、两侧 projection 与每条 accepted-difference 原因，因而不能通过同时修改 fixture 两侧来维持假绿。canonical synthetic Chat 同时保存去随机 UUID 后的完整 server body；Rust 对 Global/CN 生产 builder 生成的整棵 JSON 做 exact equality，并验证 signed-header 集没有额外字段。Global profile/session 只接受恰好 36 位小写 hex machine ID，CN 只接受 RFC 4122 variant UUID v4，错站、空白、大小写、version 与 variant 在发网前失败。

2026-09-03 的查漏补缺继续把外部仓库限制为只读交叉证据：TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 的 `qoder_gateway_service.go`（SHA-256 `1fd0f4b37b96c04927a6c3e9dc7a6711ed0bd3422eec1f7e100ab42f103b7d22`）与 `gateway_forward_as_chat_completions.go`（`25fe31a1ca48f74a49da43a3178cfd33ae34525a560a60b5239782240491af6b`）用于核对 response/tool envelope；cli2api `b67278960df9c160d45a7520ee3110b5ccb84126` 的 `worker/src/plaintext.mjs`（`1f25273ac8b8b7b156ea70945bcbedcca38007cc5fea55d7360fa2d4ca85a413`）、`sse.mjs`（`a3620d3bcf49674136c55b536a41d6f161ae2bc5513ff29fab2452d5462686a6`）与 `errors.mjs`（`d22ed21e6cbc1fe3e807de662760cb218e6f8943c7806c4118a8e76eb1d47650`）用于核对工具历史、SSE 与 Retry-After。吸收内容只包含单绑定账号内的 bounded compatibility、错误/脱敏和容量治理；没有吸收账号池、权重、轮询选号、跨账号/跨站 fallback，也没有采用“缺失终态仍成功”或旧 Center refresh 行为。文件摘要与安全不变量冻结在 oracle 的 `compatibilityPolicy`。

CN `cosy-clientip` 现由 Server 自身出站路由决定并在 catalog、quota、auth-status、generation 四条链路共用：目标路由优先，remote-DNS 情况下允许不发包的默认路由探测，运维可用 `CC_SWITCH_QODER_CN_CLIENT_IP` 提供规范 IPv4；任何下游 forwarded header 都不可信。Global 保持 machine-ID client IP 语义。响应兼容统一 OpenAI final-message/content-block/reasoning/usage 与 Anthropic-style content/tool/message envelope，工具 result 只在当前 batch 唯一关联；这些归一化不改变唯一 terminal + EOF 才成功的规则。

oracle 的 `verification` 单源仍维护 65 项既有 Rust Qoder 专项、9 项 Node mutation、7 项 loopback real-harness fixture；生成 coverage 继续只读取该 65/9/7 元数据。CORE-N2 另增的生命周期容量 fixture 使当前 Qoder 关键词测试为 74/74、request-memory 关键词测试为 24/24。Node audit 还精确校验 lifecycle method/path/timing/header/body/response、三 rail 隔离、quota、bounded compatibility 与 EOF terminal；Rust 测试直接消费同一 oracle 的 lifecycle URL builder、payload/header/catalog、quota parser、SSE decoder 与 Provider facade scope。终态只有在唯一 authoritative terminal 后读到上游 EOF 才成立。`scripts/smoke/qoder-real.mjs` 的三条真实 rail receipt 仍未提供，因此这些加固只提高离线 wire 可信度，当前证据仍只允许 `fixture_verified` / `live_pending`，不得写成 live verified。

### Qoder 三 rail Account 凭据布尔投影完整性复核（2026-10-09）

推荐 TokenRouter 的 `AGENTS.md` 要求当前会话不可调用的 `project-doc` 技能；本项仅读取了该门禁说明，未读取或运行其业务文件，也未执行外仓 `--check-sources`，最新业务增量继续为 `unreviewed_skill_gate`。本项只依据目标 Server 自身必填的 `AccountPublicView` 布尔字段，不推导新 COSY wire。旧 Qoder 私有 receipt 发布器对 Global PAT 的 `hasAccessToken/hasRefreshToken`、Global/CN OAuth 的 `hasApiKey` 使用真假值判断，且三 rail 的 `needsRelogin` 只拒绝 `true`；三 rail 共二十一种缺字段、`null`、字符串 `"false"` 反例中十七例错误进入后续验收，四例字符串凭据反例原已拒绝。现要求各 rail 的这些字段明确为布尔 `false`，并保留所需凭据字段明确为 `true`；全部二十一例在 Provider/目录请求前失败且不落盘，正常三 rail fixture 保持通过。

此项只收紧私有验收证据，不改生产 COSY wire、site/rail、Account/Provider/Share 路由或 receipt schema v2/revision 2；控制面分次读取不是原子快照，fixture 不证明真实 Qoder 接受性。Qoder 专项 Node 179/179、全量 audit Node 1267/1267、smoke Node 13/13；CLI oracle、默认自包含 reference-delta、Server Provider contract/Phase 0/coverage/UI matrix、文档索引、JS 语法、rustfmt 与差分空白检查均通过，受门禁约束的外仓 `--check-sources` 未运行。三个 `RUN_REAL=0` 各为 `blocked_inputs/live_pending`；三份真实 receipt、登录/轮换、权威空目录、受控两段 401 证据和当前未提交工作树的 committed-target observation 仍缺；未做 UI 自动化或部署测试。

2026-10-09 的目标 Server 内部复核发现，Qoder 生产 session/model-catalog 缓存原作用域只有 Account ID 与两种 generation；同代次直接变更凭据/机器身份或同 ID 重建账号，仍可能命中旧内容。现把进程内 Account incarnation 与实际请求身份的域分离 SHA-256 摘要加入作用域，返回前及 generation 前再复核当前身份。摘要仅用于进程内缓存键，不保存凭据明文；回环验证同代次 token/机器 ID 变化会重新拉目录。本项不使用未获 `project-doc` 门禁许可的最新 TokenRouter 业务源码，不推导新 COSY wire；最新参考增量仍为 `unreviewed_skill_gate`，三条真实 rail 仍为 `live_pending`。

## 2026-09-01 CodeBuddy OAuth evidence freeze

`codebuddy_oauth` 的实现合同来自一次性、只读的协议研究，优先级如下：

2026-09-11 的 revision 2 差分新增 `cli2api@e5893f0864149caef4c3b9752454190e45875c33` 空消息处理与 `@32aa108b7442cf168c1606d19f9691feafbc116c` 空 stream delta 处理作为只读交叉证据。本地实现额外保留空 content 的 tool result、严格 named choice 校验及唯一 terminal + EOF；`assets/contract/codebuddy-reference-delta.json` 固定源码摘要、CB-01 至 CB-04 与 Intl/CN 独立 `live_pending` 门禁。

2026-09-18 增量只读取 `cli2api@624874a0331f5e8f012ef104b633e69826487458` 及 cdc80d6、a9ae393、6ac6f83、d361559、eef5b2c、76c4dab 的已提交 Git object；参考工作树中的 `proxy.html`、`proxy.md` 未跟踪文件明确排除。CB-N1 对历史工具轮次执行连续、唯一 ID、一一配对及完整 JSON arguments 校验，修复可复现 11148，但不伪造 result 或修改 output。CB-N2 在空消息过滤前把 assistant `reasoning_content` 规范化为上游 `reasoning`，Responses reasoning/message/function call 由共享 transform 合并成同一 turn。CB-N3 的 Claude/Chat/Responses namespace 闭环已有 request-local 共享 bridge 覆盖，不增加裸 child 猜测。CB-N5 冻结 terminal 前截断、`[DONE]` 后无 EOF 与 marker 前后取消；共享 guard 会关闭上游 body、释放 Account/Share 租约，仍只有唯一 `[DONE]` + EOF 才成功。

参考实现的 `deepseek-v4.1-flash` 精确 native ID、顶层 reasoning 字段和 context-window 只是 CB-N4 二级信号。本仓库没有 CN 绑定账号 receipt 或冻结厂商目录，因此 reviewed allowlist 不开放该 ID，`runtimeEnabled=false`、状态保持 `live_pending`；不采用 `deep-model` 静默改写，也不把模型专属字段泛化。

2026-09-19 的 CORE-N1 只调整 Server 内部所有权：`src/proxy/providers/codebuddy/` 统一持有 exact `codebuddy_oauth` Account binding、canonical request/model、Intl/CN live-catalog capability 与 payload preparation，以及 Provider/Account generation fence。共享 forwarder 继续拥有 Share/Account lease、usage、terminal、共享 attempt budget 和是否执行同账号 pre-commit 401 replay；endpoint、header、wire、错误记录路径与恢复次数没有变化。LIVE-N1 同时新增 Intl/CN 两份相互独立的 schema-v2 私有 receipt validator，绑定当前 target commit/harness、Account auth/token generation、三个 Surface Provider revision/runtime digest、Share revision/binding digest、exact model、三份 fresh `codebuddy_live_model_catalog` snapshot 与六个 body hash。16 项检查必须全部通过才可在真实模式接受 `live_verified`；fixture 永远只能得到 `contract_verified/live_pending`。当前未提供两站真实 receipt，合同中的 receipt 继续为 `null`，CB-N4、企业与多模态也不会被 receipt 自动启用。

EVID-N1 随后把 CodeBuddy reference delta 迁为 append-only schema v2。`d81056c786196357c1074336a06a6cb847c9f9d6` 中原 schema-v1 文件 SHA-256、target commit/tree、全部 11 个历史字段的 canonical/逐字段 digest 均已冻结；新增 `cli2api@624874a0331f5e8f012ef104b633e69826487458` / tree `c2f02a394e1381adf8291949822ce4b08a48dfb3` committed-object snapshot，并明确记录工作树不干净、`proxy.html` 与 `proxy.md` 两个未跟踪文件全部排除。该次首轮冻结的 15 条不可变 observation 一一映射 CB-01～04、CB-N1～N5、CORE-N1/N2、LIVE-N1 与 CB-R1～R3，绑定 source path/symbol/digest、处置理由和本仓库 committed baseline/implementation object。三条 reject 固定账号池/轮换/affinity 与跨 domain/site/Account/Provider fallback、签到/企业运营/prompt rewrite/商业 Key/多租户后台，以及未经验证的企业/图像/视频能力、外仓运行时依赖、fixture 冒充 live 和缺唯一 terminal + EOF 仍成功。默认审计不读取外部 checkout，只有显式 `--check-sources` 才复核冻结 Git object。

1. CodeBuddy CLI `2.142.0` bundle、站点 overlay，以及国际个人订阅账号的脱敏真实流量；
2. 本仓库 [`docs/provider/codebuddy-oauth.md`](docs/provider/codebuddy-oauth.md) 已冻结的端点、OAuth、refresh、目录、计费与 terminal 约束；
3. `cli2api` commit `9b18f2d` 的 WorkBuddy CN/Global adapter，仅作为国内实现、payload、错误投影与缺陷的交叉样本。
4. `cli2api@624874a0331f5e8f012ef104b633e69826487458` 的提交态增量，只用于 CB-N1～CB-N5 差分，不覆盖前两级证据。
5. `cli2api@34c91b899aa2289f5ee49618d2d300f3eeaffedb` 及五个历史 committed delta 的第二轮只读增量，只用于 CB-N6 与共享 Responses bridge 复核，同样不覆盖前两级证据。

实现不得在构建或运行时读取上述外部源码。国际站固定为 `https://www.codebuddy.ai`，国内站固定为 `https://copilot.tencent.com`；站点属于账号身份，不允许失败后换 host。CLI `2.142.0` 证据优先于 `cli2api` 使用的旧 `2.139.0` wire。国际 fixture 可据此标为离线通过；国内真实数据面、企业账号、图像/视频与本仓库真实订阅 receipt 仍是 `live_pending`。

2026-09-03 又对 `/data/projects/proxy/CodeBuddy/workbuddy-switch` 做了一次性只读差异审计，只吸收单账号控制面证据：`domain` 决定受控 billing origin，summary / paid / free 三路新资源接口及旧 `/v2` 回退，`X-Client-Platform: web`，逐请求用量分页与 prompt 字段白名单裁剪，以及闲置 refresh session 的 `12153` 终态。Server 独立实现将稳定身份冻结为 `site + uid + enterpriseId`，把 domain 降为同站可更新路由属性；任一路资源 401 交给已有单次 refresh/replay，官方用量只缓存安全投影。明确不吸收该项目的账号池/轮换、自动签到、进程与本地配置切换，也不吸收 prompt rewrite。企业账号和多模态继续 fail closed；未执行真实订阅验收，因此 registry 的 forward/test/discovery 仍保持 `live_pending`。

2026-10-09 的目标 Server 自身复核发现，CodeBuddy 权威目录缓存原来只用 Account ID、site/domain 和 auth/token generation 区分身份；同代次直接更换凭据/UID 或同 ID 重建账号会继续命中旧目录。Intl/CN 回环中的同代次 access token 变化先复现旧缓存命中，再以进程内 Account incarnation 和域分离 SHA-256 请求身份摘要隔离新旧目录，并在目录返回和 generation 前复核。摘要覆盖实际凭据、profile、有效期及测试端点，明文不进入缓存键；只用于目标本地运行时，不是来自 cli2api 的新协议事实。推荐 cli2api 当前 HEAD 的 CodeBuddy 专属实现自既有已审对象后无新增提交，外部未跟踪文件继续排除。双站 receipt 和 CB-N4 厂商目录/接受性仍独立待证。

## 2026-10-09 Claude thread alias 账号实例与凭据隔离复核

只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `claude_executor_tool_state.go`：其续线程工具别名存储按 message ID 复用，不提供本 Server 的 Share/Account 隔离语义。本项依据目标 Server 自身多用户、托管账号生命周期补强 CL-N7，不从参考推断新的 Anthropic wire。原缓存键已有 Share、用户、显式 session、Provider revision/runtime、Account ID 与身份代次，但缺账号实例和同代次凭据；同 ID 重建或直接改 token 可能命中前一身份的历史工具别名。

现将进程内 Account incarnation、token refresh generation 及域分离 SHA-256 凭据摘要加入键；摘要绑定实际出站 Bearer、账号 refresh token、profile、有效期和重新登录状态，明文不进入缓存键。写入/读取作用域构造时若当前账号 access token 与最终出站 Authorization 不一致，禁用该次缓存而非以新账号身份记录旧请求。精确旧别名未命中时，缺工具声明的续接仍在发网前 404；不会换账号或回退到其他 Share。单元 fixture 覆盖实例和摘要隔离，转发回环覆盖同代次 token 改变后旧状态拒绝、恢复原 token 后正常续接。完整 Rust 3303 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Node audit/smoke 1909/1909、Clippy、rustfmt、Claude reference-delta `--check-sources`、离线 wire profile、Provider/Phase 0/coverage/UI matrix、文档索引和差分空白检查均通过。五个 `RUN_REAL=0` 入口仍为 `blocked_inputs/live_pending`，未发起真实请求。进程内实例不承诺跨重启稳定，回环不证明真实 Anthropic thread/beta 接受性；五份真实 receipt 与当前工作树提交态 observation 仍待补齐。未做 UI 自动化或部署测试。

同日继续检查 CL-N7 的 message ID 唯一性边界：推荐 CLIProxyAPI 的只读提交态按 ID 后写覆盖 alias，目标旧缓存也如此；若同一精确 Share/用户/session/Provider/Account 身份的两个成功响应异常复用一个 message ID，却使用不同别名，后续 continuation 会读到最后一份映射。目标缓存单测先复现。现把该键标记为冲突直至原 TTL 到期，既不返回任一映射，也不允许同一 ID 的下一次写入解除冲突；即使 continuation 带完整工具声明，已知冲突也在发网前以 409 拒绝。相同映射的重发和无状态但声明完整的普通续接保持可用，缺必需状态仍为 404。这是 Server 自身的 fail-closed 状态管理，不代表 CLIProxyAPI 或 Anthropic 有新的 wire 保证；进程重启、TTL 过期或容量淘汰后的历史冲突不持久化，五份真实 operation receipt 与 thread/beta 接受性仍是 `live_pending`。

本地转发回环复现相同上游 ID 的双成功响应，证明后续带工具声明的请求在发网前 409；完整 Rust library 3321 passed / 1 ignored、其他目标 124 + 1 + 1 passed。Claude 私有 Node 252/252、全量 audit 1896/1896、smoke Node 13/13、本地服务 smoke、Clippy、rustfmt、离线 wire profile、reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引和差分空白检查通过。`state.rs` 变化后的 Phase 0 writer 摘要由生成器刷新；五项 `RUN_REAL=0` 仍为 `blocked_inputs/live_pending`，没有真实 Anthropic receipt，也未做 UI 自动化或部署测试。

### Claude SSE CR 与混合换行解析一致性（2026-10-09）

只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 Claude translator/response 处理路径；其按事件载荷处理流，不是本 Server 的字节切分实现来源。本项依据 SSE 行终止规则与目标 Server 自身的一致性合同：`AnthropicSseInspector` 旧代码虽能识别 `\r\r` 事件边界，事件内字段却只按 LF 拆行；Claude OAuth 工具名恢复器连 CR-only 和混合换行空行都无法切分。固定 CR-only 的 `message_start`/`message_stop` 与工具名响应、混合 LF/CRLF/CR 空行的两层反例先红。现共用按最早两个完整行终止符识别事件的边界函数，检查器按 CR/LF 拆字段，恢复器按 CR/LF 定位 `data:`，保留原始分隔字节。逐字节 split、既有 LF/CRLF 和多行 `data` 测试保持通过。此项只修本地流解析/响应名恢复，不推断新的 Claude OAuth 上游请求 wire，也不提升五项真实 operation、thread/beta 厂商接受性或未提交实现的 committed-target observation。

## 2026-10-09 Codex previous-response 工具上下文身份隔离

只读核对推荐 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 Codex `previous_response_id` 转发，以及 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的 `responseCacheOwner`/WebSocket replay：后者按下游 API Key ID 建 owner，不能直接作为本 Server 签名 Share、多用户、托管账号作用域。目标原本只按 Share、principal、runtime fingerprint 与 verified workspace 保存工具上下文；同 Share/workspace、同身份代次直接换 token 后，新的请求可命中旧 `response_id` 的本地 call context。定向回归先在旧实现证明作用域完全相同。

Server 独立扩展 `PreviousResponseCacheScope`：加入 Provider ID/revision、Account ID/进程内 incarnation、auth/token generation，以及从当前 access/refresh/ID token、token type、账号额外头、verified subject、有效期与 relogin 状态计算的域分离 SHA-256 摘要，键中不保存明文凭据。缓存读取、写入和 tombstone 继续共用同一精确作用域；HTTP 与 WebSocket 都由同一作用域函数取得，跨账号或直接凭据变化的历史工具项不会注入。HTTP 在最终授权头形成后再次核对：若已注入的上下文跨了刷新/凭据变化，发送前稳定 409；未依赖旧上下文的请求把新成功状态写入最终凭据作用域。WebSocket 每个 `response.create` 都比较连接 Authorization 与当前账号，漂移则要求重连。绑定 Share 保护禁止直接删除其所引用的账号；同 ID 重建仅通过缓存层的实例隔离 fixture 验证，不绕过该保护。普通无依赖续接和真正缺历史工具上下文时的 409 语义、TTL/容量、Codex 出站 wire 均不改变。进程内 incarnation 不承诺跨重启持久稳定，离线 fixture 不是 ChatGPT live receipt。Codex 关键词 409/409、完整 Rust library 3305 passed / 1 ignored、其他目标 124 + 1 + 1 passed；Node audit/smoke 1909/1909，Clippy、rustfmt、Codex reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引及差分空白检查通过。五个 `RUN_REAL=0` 入口仍是 `blocked_inputs/live_pending`；无真实 receipt，也未做 UI 自动化或部署测试。

2026-10-09 继续复核 Codex previous-response 的 ID 精确身份：参考 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 仍只作只读交叉证据；后者本地 cache 的 trim 行为不适合作为本 Server 签名 Share 工具历史的身份授权规则。目标自己的请求提取与缓存键原来都裁剪 `previous_response_id`，使 `" resp-a"` 或 `"resp-a "` 错误命中 `"resp-a"` 的历史 function call。缓存命中和请求提取两条定向反例在旧实现先红。现保留客户端原始 ID，缓存只接受非空、至多 512 字节、无首尾空白的精确键；完成响应的上游 ID 也不再裁剪后入库。HTTP/WS 共用提取与缓存边界，带未配对 tool output 的无效 ID 因无法取得历史而按现有 `response_context_unavailable` 409 失败关闭。合法 ID、同账号作用域、TTL/容量、成功终态写入与 Codex 出站 wire 不变；本地 fixture 不证明真实 ChatGPT 账号接受性，五项真实 operation 仍独立 `live_pending`。

精确注入回归确认带空白的 ID 不读取相邻精确键的历史工具项。最终完整 Rust library 3324 passed / 1 ignored，其他目标 124 + 1 + 1 passed；Codex 私有 Node 205/205、全量 Node audit 1896/1896、smoke Node 13/13、本地服务 smoke、Clippy、rustfmt、reference-delta `--check-sources`、Provider/Phase 0/coverage/UI matrix、文档索引与差分空白检查均通过。五项 `RUN_REAL=0` 均为 `blocked_inputs/live_pending`，离线 release readiness 也因缺真实输入/部署验收保持 blocked；未做 UI 自动化或部署测试。

## 2026-09-01 Trae CN Solo evidence freeze

`trae_solo` 的 wire 来自对 `cli2api` commit `9b18f2d` 中 `internal/providers/trae/` 的一次性只读审计。采纳的事实仅限固定端点、Cloud-IDE 身份头、Solo payload、模型详情、订阅额度与 `metadata/output/token_usage/done/error` 事件结构；实现由本仓库独立完成，外部源码不是同步源或依赖。

固定出站 origin 为 OAuth `https://api.trae.com.cn`、Agent `https://trae-api-cn.mchost.guru`、Billing `https://api.trae.cn`、浏览器授权 `https://www.trae.cn`。明确拒绝参考实现中的三类行为：callback 消费任意 pending flow、导入的 `api_host` 控制凭据目的地、以及 EOF/error 后合成成功终态。Server-native Solo bridge 不等于 Trae IDE MITM、插件注入或桌面流量劫持。

离线 fixture 或 mock 只能证明合同接线；真实 OAuth、refresh、目录、额度、三 Surface 流式/tools/reasoning、401 恢复与错误码仍保持 `live_pending`，直到本仓库留存脱敏 receipt。

## 2026-10-09 Cursor 精确会话身份隔离

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 `open-sse/services/cursorSessionManager.ts` 只读显示：parked h2 stream 在进程内按 conversation ID 保留并用同一流回送 MCP 结果。这是续流机制的参考，不是本 Server 多用户 Share/账号的身份授权模型；未提交 Codex 工作树不纳入证据，CUR-N2 `reviewed_no_wire_delta` 保持。目标自己的 OAuth session/Responses scope 原先未覆盖同代次实际 token 与 Cursor 机器身份、账号实例，API Key 在 verified account ID 相同时 session 也只靠 credential generation。换 OAuth token 的旧作用域相等反例先红；新实现以账号 incarnation、域分离凭据摘要和 API Key 实际 key 摘要隔离 parked stream 与已完成 Responses，取得/开流后复核，401 refresh 成功的 Responses 按最终作用域缓存。OAuth token/机器/同 ID 重建及已验证 API Key 同代次换 key 的 fixture 覆盖隔离。conversation affinity 不代替精确状态作用域；分次复核不是跨存储原子性，也不改变 Cursor protobuf/session wire 或 receipt schema。OAuth/API Key 均没有真实 receipt，保持 `live_pending`。

## 2026-10-09 Cursor 续传不透明 ID 的精确匹配

沿用已审 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 提交态作为 parked-stream 机制参考；其 22 项未提交 Codex 工作树内容继续排除，CUR-N2 的 `reviewed_no_wire_delta` 不变。本项不推断新 Cursor wire，仅修目标 Server 自身多租户续传身份边界。旧完成响应缓存、parked response/tool 索引和消费 claim 会裁剪 ID，`" resp-a"` 可命中 `"resp-a"` 的状态；工具结果校验、cold-resume 判定与 Responses 历史合并也会裁剪 `call_id`，使带空白结果可能匹配另一工具调用。缓存、session 与结果匹配三处定向反例在旧实现先红；历史合并和 Gemini 入口另补针对性回归。

现在完成响应、parked 索引和 claim 仅接受非空、无首尾空白的原始 ID；包括 Gemini 显式 function-response ID 在内的工具结果按原始 ID 校验与去重，cold-resume 仅接受精确结果 ID，Responses 历史合并拒绝带空白的调用/结果 ID，错误不回显 ID。Gemini 省略 ID 时沿用既有 name fallback，但显式空白 ID 不再悄悄折叠。合法续传、精确 Share/用户/账号作用域、TTL/容量、OAuth/API Key rail、AgentService protobuf 与 receipt schema 均不变。离线回归不能证明真实 Cursor 厂商接受性；两份独立真实 receipt 继续 `live_pending`，当前未提交工作树不产生 committed-target observation。

## 2026-10-09 Grok 模型目录同代次请求身份隔离

推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的已提交 CLI adapter 测试和 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的 `grok_observed_models.go` 均以当前 Bearer 请求 `/v1/models`；后者尚有六项未提交 Codex 内容，全部排除。参考只证明请求与账号 Bearer 相连，不授权移植其账号池、模型快照或 Header 版本。目标自身旧 scope 没有实际 Bearer，同 ID 同 generation 直接改 access token 后旧 reasoning capability 仍命中，生产回环先红。现以域分离 SHA-256 摘要绑定规范化 Bearer 和 relogin 状态，精确隔离 fresh/stale 目录及 reasoning 能力；发网前、401 强刷前和返回前复核账号身份，同代次不同 Bearer 的缓存并行条目不会互相驱逐，容量限 128 条。此处不改变默认 `0.2.111`、group header、推理/media/WS wire、目录 TTL 或 receipt schema，GR-N4 继续 `live_pending_no_wire_change`。离线回环和分次读写不能证明跨存储原子性或真实 xAI entitlement；inference、media、remote_compaction 三项私有真实 receipt 仍缺并保持 `live_pending`。

## 2026-10-09 Kiro 本地 prompt-cache 同代次身份隔离

只读复核推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态；目标自己的动态模型目录缓存已按请求身份隔离，但 SQLite 本地 prompt-cache 估算 namespace 仍只有 Account ID 与代际，同代次替换 Bearer 时可复用旧估算。固定 session 的测试先证明旧 namespace 完全相同。现将从持久化 Account 派生的域分离 SHA-256 摘要加入 namespace，覆盖规范化出站 Bearer、refresh token、实际机器 ID、profile ARN、区域、endpoint、认证方式、token type 与 relogin 状态；凭据原文不进入新键。401 强刷路径会重新准备请求和 namespace。该摘要不使用非持久 Account incarnation，以保留相同身份跨进程重启的 SQLite TTL 命中；同 ID、同代际、同全部有效身份字段的账号删除重建仍是待解的持久实例身份边界。此项仅修目标本地 fallback 计量隔离，不推导新 EventStream wire、不改变上游计费或 KI-N5 usage authority；八份真实 auth-kind×region receipt 仍缺，保持 `live_pending`。

## 2026-10-09 Qoder 目录别名与精确模型键一致性

推荐 TokenRouter 的仓库 `AGENTS.md` 仍要求当前会话不可调用的 `project-doc` 技能；未读取其业务文件或运行访问该仓的源码审计，最新增量保持 `unreviewed_skill_gate`。本项只核对目标 Server 自身的动态目录和请求解析：上游若启用与公开 alias 同名的原始 key，而 alias 真正指向的 route 未启用，旧 `/v1/models` 会发布一个请求时必然 403 的 ID；两者都启用时，公开 capability 也可能取自同名原始 key 而请求实际走 alias route。未知厂商 key 被强制转小写、带首尾空白的目录 key 被裁剪又会使公开 ID、原始 `model_config` 与请求解析不一致。三类单元反例均先红。

目标实现现在只对已审 alias 做 ASCII 大小写不敏感匹配，未知 key 保持原样；目录 key 含首尾空白时失败关闭。公开目录跳过任何请求解析会映射到另一个 route 的原始 key，只由实际启用的 alias route 发布该 ID 与 capability。成功空目录、live-enabled entitlement、site/rail/Account 绑定和 COSY 出站 wire 均不改变。这是本仓库内部一致性修复，不据此声明 TokenRouter 最新行为或真实 Qoder entitlement；Global OAuth、Global PAT、CN OAuth 三份 receipt 仍独立 `live_pending`。

## 2026-10-09 CodeBuddy `/v3/config` 业务状态与原始模型 ID 边界

只读核对推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 `internal/providers/workbuddy/client.go::Models` 与 `catalog.go::catalogModel`；两个未跟踪文档不作证据。参考将 `/v3/config` 作为权威目录、失败不回退静态目录，按原始 `model.ID` 与 CLI allowlist 交集筛选并原样公开 ID。目标 Server 自身 reviewed allowlist 需要保持更严格的站点门槛，但旧 parser 把所有上游 ID 先裁剪、转小写，可能将 `FAST-MODEL` 冒充已审核 `fast-model`；业务 `code` 若存在却不能解析，也会被当成缺字段而接受后续 `models`。两组定向测试先红。

现要求目录 ID 非空且无首尾空白，按上游原文与 Intl/CN reviewed allowlist 精确比对；大小写不同的未知项只计入脱敏漂移，不自动获得 entitlement。若存在顶层业务 `code`，必须可解析且为 0，否则失败关闭。此改动只收紧目标目录证据，不推导新聊天 wire、不启用 CB-N4 `deepseek-v4.1-flash`，不改变 Intl/CN Account/Share 绑定、目录 TTL 或 receipt schema；两个站点的真实 receipt 仍独立 `live_pending`。

## 2026-10-09 Grok reasoning replay 的同代次请求身份隔离

只读核对推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 的已提交 CLI adapter：请求携带当前账号 Bearer；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件继续排除。参考不提供本 Server 多租户 reasoning proof 的授权语义，本项不推断新 xAI wire。目标 Grok 模型目录已按实际 Bearer 隔离，但 reasoning replay scope 仍只含 Account ID、auth/token generation；同 ID、同代次将 Bearer A 改为 B 后，HTTP 回环复现 B 的续接仍注入 A 缓存的 opaque proof，反例先红。

现在 replay scope 增加进程内 Account incarnation 和规范化 Bearer/relogin 状态的域分离 SHA-256 请求身份摘要；状态层在同一个账号读锁下取齐代际、实例与摘要，提交成功终态前再精确复核，旧身份的迟到响应不能写入新身份的 proof。合法同身份 HTTP/WS replay、30 分钟 TTL、容量、CAS 与一次明确拒绝恢复预算不变；摘要不保留明文 Bearer。分次读取不构成跨存储原子事务，离线 fixture 不能证明真实 xAI 接受性。GR-N4 同账号版本/header 门槛和 inference/media/remote_compaction 三份真实 receipt 继续独立 `live_pending`，未提交工作树不产生新的 committed-target observation。

## 2026-10-09 Kiro 本地 prompt-cache 账号重建隔离

继续只读核对推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的提交态 prompt-cache 机制；它不能替代本 Server 的 AccountStore 生命周期身份。目标此前已按凭据摘要隔离 SQLite 本地估算，但同 ID、同代际、同凭据删除重建的 namespace 仍完全相同，定向反例先红。现为 Kiro/Amazon Q 账号持久化随机 owner ID：正常更新与重启保留，在线删除重建和 ProviderType 切换轮换；生产绑定路径将其加入 namespace。旧存储缺 owner ID 时按 ProviderType 与 ID 派生稳定值并在下次保存时持久化，避免无关重启使本地估算全失效。此字段不承载上游授权，不更改 EventStream、KI-N5 usage authority、上游账单或本地缓存 TTL。

旧文件若在离线状态下被手工重建为完全相同的内容，加载器无法得知曾有删除事件；这仍是明确边界。直接、未绑定的测试调用使用独立占位作用域，不证明生产身份。生产 `prepare_bound_request` 定向测试确认同账号字段重建后仅 owner ID 改变，SQLite 估算测试确认旧条目不命中；真实 Kiro/Amazon Q 接受性及八份 auth-kind×region receipt 继续独立 `live_pending`，未提交实现不产生 committed-target observation。

## 2026-10-09 Kiro 本地 prompt-cache 嵌套字段签名边界

只读复核推荐 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 的 `src/anthropic/cache_metering.rs::without_cache_control`：它只移除 prompt block 顶层的断点声明。目标 `src/proxy/kiro_prompt_cache.rs` 原递归删除所有同名字段，导致工具 `input_schema/properties/cache_control` 的不同定义生成相同本地缓存签名；固定回归先观察到错误 cache read。目标现只删除 block 顶层字段，并保留嵌套字段及确定性对象顺序。签名域从 v3 升到 v4，隔绝可能与新签名碰撞的旧 SQLite 条目；这是本地估算的保守冷启动，不改变出站 Kiro/Amazon Q wire、上游账单或 KI-N5 usage authority。八份真实 receipt、Compact 与 shared cache 的独立验收仍缺，后两者继续 disabled；离线回归不等于厂商接受性。

## 2026-10-09 Qoder usage 显式无效值的本地计量栅栏

推荐 TokenRouter 当前 `AGENTS.md` 要求本会话不可用的 `project-doc` 技能，因此未读取其业务源码或运行源码差分审计，最新参考增量继续 `unreviewed_skill_gate`。读取门禁时曾误附带只读 Git 状态/HEAD 查询，此后停止访问该仓；这里不据其最新实现推断 COSY wire。本项只审目标 Server 自身响应兼容层：旧 `canonical_qoder_usage` 将负数、对象、超范围字符串等显式 token 计数视为缺失并补 0，若优先别名有效还会忽略备用别名中的无效值；定向反例先红。

现将存在的 usage 与 token 计数字段逐项验证，非法值在 decoder 和直接聚合入口均按协议错误拒绝；保留已审兼容的非负数字、数字字符串和有限小数截断。该更改不调整官方 CLI oracle、COSY 请求 wire、三 rail 绑定、唯一终态+EOF 或 receipt schema，也不能证明真实 Qoder usage 形式。Global OAuth、Global PAT、CN OAuth 三条 rail 继续独立 `live_pending`；未提交工作树不产生 committed-target observation。

## 2026-10-09 CodeBuddy finish_reason 后业务帧隔离

只读核对推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 的已提交 WorkBuddy `sse.go` 与 `client_test.go`：样本中 `finish_reason` 后可有独立 usage 帧，最后是 `[DONE]`；其宽松复制器不提供本 Server 的严格终态授权。两个未跟踪文档继续排除。目标 Server 自身旧解码器只禁止 `[DONE]` 后业务数据，因此 `finish_reason` 后、`[DONE]` 前追加 content 仍被转发；单元反例先红。

现于 CodeBuddy decoder 与非流聚合器保留 finish-reason 状态：合法的 `choices:[]` usage 尾帧继续接收，后续任何有语义 business choice 则按协议错误终止，不能靠晚到 `[DONE]` 写成功。Intl/CN × Codex stream 回环确认输出失败终态、不输出 completed、不刷新账号；非流也在发出成功响应前拒绝。唯一 `[DONE]`+EOF、站点/账号绑定、请求 wire 与既有测试中无 finish-reason 的 liveness 样本不变。本地 mock 不证明真实厂商接受性；两站 receipt 与 CB-N4 仍为 `live_pending`/disabled，未提交增量不产生 committed-target observation。

## 2026-10-09 Codex Responses 同批重复结束标记

官方 OpenAI Docs [Responses streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses) 将 `response.completed` 定义为成功终态。只读复核 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `internal/runtime/executor/codex_executor_stream_output_test.go` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 的 `frontend/src/pages/docs/docsGuides.ts`：两者都以 Responses 终态事件判成功；参考代码不定义本 Server 对可选 `[DONE]` 的唯一性合同。本项不引入新的 OpenAI wire 假设。

目标自己的 Responses transport inspector 原先在同批 `response.completed` 后每遇到一次 `[DONE]` 就重编码一次，因此重复尾标记可向下游出现两次。现已在第二个标记处失败关闭；SSE 与行分隔 JSON、跨 inspector push 和同批两种情况都有单元反例，Codex HTTP stream/non-stream 同批回环验证协议错误而非成功记账。流式 HTTP 收到终态后会主动取消上游 body，不继续读取更晚到达的批次；故本修复只保证已经读入批次的唯一性，不声称侦测取消后的上游数据。合法单个可选 `[DONE]`、无 `[DONE]` 的成功终态、账号/Share 绑定、请求 wire 与 receipt schema 不变。离线回环不证明真实 ChatGPT 账号接受性，五项 operation receipt 仍独立 `live_pending`；未提交工作树不产生 committed-target observation。

## 2026-10-09 Cursor 本地续传容量与客户端工具 ID 唯一性

只读核对推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 提交态的 `open-sse/services/cursorSessionManager.ts`：parked h2 流保存 `pendingToolCalls` 与 `exec_msg_id`/`exec_id` 映射；该参考不定义本 Server 多租户作用域、Responses 缓存预算或异常上游 ID 修复。22 项未提交 Codex 工作树内容仍排除，CUR-N2 的无新 Cursor wire 结论不变。

目标 Server 的 completed-response 缓存原先限制 response ID，却允许任意长度的 conversation ID 且不将这两个身份字符串计入 8 MiB/64 MiB 预算；513 字节 conversation ID 定向反例先红。现在 conversation ID 限 512 字节，缓存预算包含 response/conversation ID、scope 摘要与条目固定开销。目标的 Cursor 工具映射原先对空白或重复的上游 tool-call ID 直接透传/覆盖：前者会被续接入口拒绝，后者可能丢掉第一条待处理的 `exec_id`。现仅对空、首尾空白、超 512 字节或当前 pending map 已占用的上游 ID 生成唯一客户端 ID，保留各自上游 `exec_msg_id`/`exec_id`；正常唯一 ID 原样保留。映射函数与实际 `surface_mcp_tool_call` 定向回归验证三条待处理调用均可区分。此项修目标本地内存和工具续接边界，不更改 Cursor AgentService protobuf、OAuth/API Key 身份、Share 路由或 receipt schema；离线测试不能证明真实厂商接受性，两条 rail 仍独立 `live_pending`。

## 2026-10-09 Grok SSE 旁路观测与主语义终态一致性

只读复核推荐 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 CLI `responses_compaction_forward.go` 的 `response.completed` 终态样本，以及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 gateway 对 Responses 终态集合的处理；后者六项未提交 Codex 文件排除。参考不定义本 Server 搜索能力 evidence 的持久化门槛，GR-N4 同账号版本/header 接受性不由此改变。

目标主 Responses transport 会拒绝 `event:` 与 JSON `type` 冲突、伪 ping/未知 SSE 字段，并且 `response.done` 不单独满足成功终态；但 Grok 搜索/质量旁路检查器原只取 `data` JSON，冲突帧会先被算作 completed search/quality。它还把 `response.done`、非标准 `succeeded/done` 状态或失败状态的 `response.completed` 当作成功，且失败终态之后的迟到 completed 能覆盖观察。冲突声明、非终态/失败终态和失败后迟到完成的单元反例在旧实现先红；修复后旁路拒绝不可信 SSE 字段/声明，完成判断复用主语义分类并要求若有 status 则为 `completed`，首次终态后冻结观察。HTTP SSE 回环确认只有 `response.done`、没有合法终态的搜索响应最终失败且不写搜索能力 evidence。合法已完成的 `response.output_item.done` 仍可作为单项 hosted search 能力证据；正常 `response.completed`、请求 wire、Account/Share 绑定、媒体/WS 行为和 receipt schema 不变。本地回环不能证明真实 xAI entitlement，三项真实 operation receipt 仍独立 `live_pending`。

## 2026-10-09 Qoder SSE 混合行结束符与唯一终态

本轮再次读取推荐 TokenRouter 的门禁说明，当前会话仍无法调用其要求的 `project-doc` 技能，因此没有读取该仓库业务源码或运行其 `--check-sources`；最新参考业务增量继续是 `unreviewed_skill_gate`。以下结论只来自目标 Server 自身的解析器与已冻结 Qoder CLI oracle，不推导新 COSY wire。目标 `QoderSseDecoder` 和 `QoderChatSseAggregator` 原先在整块缓冲中优先查找任意 CRLF 空行，才查找 LF 空行；较早的 LF 事件和较晚的 CRLF 事件同批到达时，会被错误合并成多 `data:` 事件。两层固定反例先红，现按最早完整空行切分，并以 CR/LF 拆字段；所有分块位置与 CR-only 用例通过。非流聚合器原还会接受第二个 `[DONE]`，与原生解码器的唯一终态不一致；固定反例先红后绿。请求、站点/rail、Account/Share 绑定、唯一终态+EOF 与 receipt schema v2/revision 2 不变。Global OAuth、Global PAT、CN OAuth 真实 receipt 及登录/轮换、权威空目录、受控两段 401 证据继续独立 `live_pending`，离线回归不证明厂商接受性。

## 2026-10-09 CodeBuddy SSE 混合空行的最早边界

只读核对推荐 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/sse.go`：它按行扫描 WorkBuddy 流，未提供目标 Server 所需的跨 chunk 字节级空行判断；两个未跟踪 `proxy.html` / `proxy.md` 继续排除。目标 `CodeBuddySseDecoder::next_event_boundary` 原先只比较 `LF+LF`、`CRLF+CRLF`、`CR+CR` 三种固定组合。较早事件以 `LF+CRLF` 结束、同批后面有另一个可识别空行时，旧逻辑把事件与 `[DONE]` 并成 JSON `data` 并报尾随内容；定向反例先红。现扫描最早的两次连续行结束，支持混合组合；任意字节切分回归与 Intl/CN × Claude/Codex/Gemini × stream/non-stream 回环均通过，工具内容及 cache usage 保留。该改动不改变 WorkBuddy 上游请求、Account/Provider/Share 绑定、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 或 CB-N4 disabled 决策。离线混合换行 fixture 不能证明厂商真实发送此格式，两站 receipt 仍独立 `live_pending`。

## 2026-10-09 Antigravity replay 捕获器的 CR 与混合 SSE 空行

只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 及 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`：前者 replay accumulator 按已切出的 SSE 行观察签名，后者的 pipeline/session 不定义目标 Server 的字节级分帧；两者均不提供新的 Google wire 接受性结论。目标 `AntigravityReplayStreamAccumulator` 原只识别 LF+LF、CRLF+CRLF，字段也只按 LF 拆行。签名事件以 CR+CR、终态以 LF+CRLF 结束时，旧流程在同批输入下未能形成完整 replay；固定反例先红。现按最早连续两个 CR/LF/CRLF 行结束符切事件，再按 CR/LF 拆字段；所有字节切分位置均能保留签名及终态。既有终态帧到达即可提交 replay 的生命周期合同不变；本项只修本地捕获解析，不改变出站请求、Account/Share 绑定、AG-N6 requestType、AG-D3/compaction 或 receipt schema。离线格式反例不证明 Google 实际发送该格式，两条真实 rail 仍独立 `live_pending`。

## 2026-10-09 Claude 成功响应的 model 字段门槛

只读核对 `Claude/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：其 `internal/runtime/executor/claude_executor_stream.go::validateClaudeStreamingResponse` 对 `message_start.message` 的 `id` 和 `model` 都要求非空。目标 `src/proxy/anthropic_semantics.rs::validate_message` 原只要求 `id`，流式与非流式共用该缺口；缺失、空串、全空白 `model` 的六个局部反例均先红。现共用校验要求非空 `model`，API 回环确认缺字段的 JSON 和 SSE 都在成功提交前返回 `502`，正常模拟响应补齐真实协议字段；Claude 及 Anthropic-compatible 请求、账号/Share 绑定、响应 body 内容和 receipt schema 不变。该参考不证明任何新模型 entitlement，本地回环也不代表真实 Anthropic 接受性。五项真实 operation receipt 及续线程/beta 厂商证据仍为 `live_pending`，未提交工作树不产生 committed-target observation。

## 2026-10-09 Codex completed 事件与 response.status 一致性

`Codex/AGENTS.md` 推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 均为干净提交态。后者 `admin/codex_test_diagnostics_test.go::TestCodexTestRecorderObservesFailureShapes` 特别保留 `type=response.completed` 而 `response.status=incomplete` 的诊断状态；它是矛盾输入的风险证据，不是官方 wire 允许性的证明。目标 `classify_value` 旧流程仅凭事件名将此类响应记为成功；`incomplete/in_progress/queued` 三种状态的 JSON 与 SSE 反例先红，随后补上 `null`/数字两种非字符串回归。现在直接分类器不再给成功，普通 Responses JSON 和 SSE 检查在成功提交前以固定协议错误拒绝；缺 status 和合法 completed 保持通过。OpenAI Docs 技能要求的官方页面本轮因两个官方域名 TLS 握手失败未能取得，因此不从该参考推导新的上游协议能力。出站 payload、路由/计费、Account/Share 身份和 receipt schema 未变；五项真实 Codex operation 继续 `live_pending`，当前未提交工作树没有新的 committed-target observation。

## 2026-10-09 Cursor 同作用域二级索引歧义隔离

推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 `open-sse/services/cursorSessionManager.ts` 说明 parked h2 stream、pending tool-call 映射与按 ID 查找；其 22 项未提交 Codex 内容排除。该参考的线性查找也不定义多会话重复 ID 的安全选择，本项按目标 Server 自己的多租户作用域与并发会话约束处理。目标 `CursorSessionManager` 原来的 scoped response/tool-call 二级索引各只保存一个 binding；同作用域的两个不同 parked 会话绑定相同 ID 时，第二个覆盖第一个，查找会错误选择后写会话。双 ID 反例先红。现在同一 scoped ID 保留候选，只有当前主 registry 中恰好一个有效会话时才解析；重复绑定同一会话幂等，关闭其中一个会话后剩余唯一候选恢复可用。此项不改 Cursor protobuf、出站请求、Account/Share/rail 绑定、客户端工具 ID 生成或 receipt schema；离线索引测试不证明厂商真实生成重复 ID。OAuth/API Key 两条真实 receipt 继续 `live_pending`，未提交工作树没有新的 committed-target observation。

## 2026-10-09 Grok Responses ping 的有状态 SSE 字段保留

只读核对 `Grok/AGENTS.md` 推荐的 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `backend/internal/service/openai_gateway_grok_sse_filter.go`：它只在单一 `event: ping` 后继续缓存 `data`/comment，遇到 `id`、`retry` 或第二个 `event` 即原样回放；六项未提交 Codex 内容排除。目标 `grok_sse_event_is_ping` 原只要求每个声明事件都叫 ping，忽略其他 SSE 字段，导致带 `id` 的帧被改成 comment、重连 ID 丢失。`id`、`retry`、重复事件和未知字段四类反例先红；现只允许单一 ping 声明与 `data`/comment，其余帧保持原字节。任意两段切分回归覆盖四类反例；正常 ping、CRLF、上限和后续业务帧仍走原路径。这是本地下游 SSE 元数据保真，不推断 xAI 新 wire，不改出站请求、Account/Share 绑定、搜索证据、GR-N4 版本/header 或 receipt schema。三项真实 Grok operation 与同账号版本/header 接受性继续 `live_pending`；当前未提交工作树不能写 committed-target observation。

## 2026-10-09 Kiro 本地 prompt-cache 全局选项的键顺序规范化

只读核对 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 中 `src/anthropic/cache_metering.rs` 的前缀签名和断点语义。目标已有独立的全局上下文签名，用来隔离 model/tool choice/thinking/output config；其 `serde_json` 开启 `preserve_order`，原 `global_cache_context` 直接序列化嵌套对象，语义相同的 thinking/output config 仅换对象键顺序便错误 miss。同一绑定、相同 prompt 的反例先红；现仅在哈希前递归排序全局选项对象键，保持数组顺序和值，选项值实际改变仍 miss。无需改变已按内容区分的 v4 域，旧顺序相关条目自然按新签名冷启动。本项只修本地 fallback 计量一致性，不改 Kiro/Amazon Q 出站 body、EventStream、权威 usage、KI-N5 shadow、Compact/shared cache disabled 或 receipt schema；八份真实 auth-kind×region receipt 和当前工作树 committed-target observation 仍缺。

## 2026-10-09 Qoder 不可信错误码的下游回显栅栏

`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求当前会话先加载不可用的 `project-doc` 技能；本项没有读取 TokenRouter 业务源码或运行其外仓源码审计，最新增量继续记为 `unreviewed_skill_gate`。本项只审目标 `QoderUpstreamError::from_status_body`：原逻辑把上游任意字符串 `code` 原样放进 `ProxyError.message` 和下游错误 JSON，固定携带 Bearer 文本的反例先红。最初限制为六位数字后复核发现，六位一次性验证码形状仍可能泄露，故最终仅允许本地已识别、用于分类的 `112`/`115`（JSON 数字或字符串）；未知数字、文本、非整数及超长值一律不回显。HTTP 状态和独立脱敏的 message 保持，已知权限/限额分类保持；未知代码不再作为诊断文本透出，这是刻意的安全兼容取舍。

终态后的空 wrapper 拒绝测试在原实现即通过，属于边界回归，不记为新修复或先红后绿。此项不修改 COSY 出站 wire、账号/站点/rail 绑定、唯一终态+EOF 或 receipt schema；本地测试不证明 Qoder 真实错误码集合。Global OAuth、Global PAT、CN OAuth 三份真实 receipt、当前未提交工作树的 committed-target observation 及 TokenRouter 合规补审仍待完成。

## 2026-10-09 CodeBuddy 上游错误消息与未知数字码的展示栅栏

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/client.go::Classify`：参考会保留部分原始 body/`env.Msg` 用于诊断，并不提供本 Server 的多租户下游脱敏合同；其未跟踪 `proxy.html`、`proxy.md` 排除。本项不从参考推断新的 WorkBuddy wire。目标 `CodeBuddyUpstreamError` 原只截断 `msg/message`，HTTP 上游回显 Bearer 的定向测试先红。现 HTTP 与 SSE 错误消息进入下游前复用本地敏感文本脱敏，过滤控制字符并限制长度；用于内部失败分类的递归业务码保持原值，但仅向下游文本显示本地已映射为认证、权限、限额或请求错误的码，未知数字不回显。已知 `11102/14003` 分类及原有 HTTP 状态、单账号恢复边界不变。

通用脱敏按已知敏感字段线索工作，不承诺识别无标识的任意秘密。本地 fixture 不代表 Intl/CN 厂商接受性；两站真实 receipt、CB-N4 CN 目录/能力和当前未提交工作树的 committed-target observation 仍缺。未做 UI 自动化或部署测试。

## 2026-10-09 Antigravity 迟到容量错误的账号身份栅栏

只读复核 `Antigravity/AGENTS.md` 推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。前者 `sdk/cliproxy/antigravity_models_fencing_test.go::TestAntigravityProbeFencesOlderTokenResult` 验证旧 token 的目录探测不得覆盖新 token；后者未提供目标 Server 能力证据写入的原子性合同。这只是身份隔离风险线索，不据此推断新的 Google wire。

目标 `record_limit_evidence` 原只凭 Account ID、Provider 类型和 `authIdentityGeneration` 写 `model_capacity`：同 ID 同代际把 Bearer A 改成 B 后，A 请求迟到的 Google RPC 容量错误仍能污染 B 的证据。状态层定向反例先红；现在转发两处限额路径传入本次请求 Account 快照与进程内 incarnation，状态写锁内再次核对 Provider、auth/token generation、规范化 Bearer/relogin 请求身份摘要、project ID 和 incarnation，任一漂移即不写入。状态回归覆盖当前身份写入、同 ID 删除重建、代际和项目变化；真实 HTTP 回环在 A 的 503 挂起时换成 B，确认旧响应不写 B 的能力证据。该修复只隔离本地观察，不改变出站请求、同账号重试、Share 绑定、AG-N6 requestType、AG-D3/compaction 或 receipt schema。离线回环不证明真实 Google 接受性，Antigravity/AGY 两份 receipt 仍独立 `live_pending`；当前未提交工作树也不产生 committed-target observation。

## 2026-10-09 Claude SSE 缺少 message_delta 的假成功隔离

只读核对 `Claude/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：`internal/runtime/executor/claude_executor_stream.go::validateClaudeStreamingResponse` 要求 `message_start` 后出现 `message_delta`，其 `claude_executor_test.go::TestClaudeExecutor_ExecuteOpenAINonStreamRejectsIncompleteClaudeStream` 明确拒绝只有 start/stop 的样本。参考只提供截断流风险证据，不决定目标 Server 的账单、重试或下游错误格式。

目标 `AnthropicSseInspector` 原在 `message_start` 后直接遇 `message_stop` 时输出成功终态；固定单元反例先红。现只有经 `validate_message_delta` 校验过至少一条 delta，才允许成功 stop。实际 Claude OAuth HTTP 回环以同批 start/stop 响应验证：代理在提交成功前返回 502，且不留下 completed usage；合法的含 delta 流仍可在 stop 后立即结束而无需等待上游 EOF。已有错误事件、分块/混合换行和内容块回归不变。此项不修改出站 Anthropic wire、Account/Share 绑定、credential refresh、receipt schema 或模型 entitlement；本地回环不证明真实 Anthropic 账号接受性，五项真实 operation receipt 与 committed-target observation 仍待完成。

## 2026-10-10 Codex Responses 完成项 ID 与非空终态 output 保真

[官方 OpenAI Docs 的 Responses streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses)将 `response.completed` 作为成功完成事件；文档本身不证明本轮补 ID 的厂商接受性。只读复核 `Codex/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`。前者 `internal/runtime/executor/codex_executor_terminal.go::hydrateCodexCompletedOutputItemIDs` 对非空 `response.completed.response.output` 按 `output_index` 从已完成项补缺失 ID，同时保留终态原有字段；索引项与无索引项分别收集。后者的 `admin/test_connection.go` 也以 completed/failed 做诊断终态，但不定义本 Server 的 output 修复规则。本项不从参考推断新模型权限或上游请求 wire。

目标原 HTTP SSE 和 WebSocket patcher 在终态 output 非空时直接原样返回，缺失的 item ID 不补；两条固定反例先红。非流聚合原更会在有 `output_item.done` 时整体覆盖非空终态 output，丢掉其原有项/字段；另一个反例显示有索引项后跟无索引项会复用索引 0 并覆盖前者，均先红。现三条路径共用“仅补缺失/null/空白 ID，不改已有 ID 或其他字段”的辅助规则；非流聚合仅在终态 output 为空时按显式索引排序重建，并把无索引项追加，不再碰撞。Codex OAuth 非流 HTTP 回环确认终态 output 原有字段与补齐 ID 同时保留。该修复只处理本地 Responses 输出身份/聚合，不改出站请求、单账号/Share 绑定、WebSocket 重试边界、usage 或 receipt schema；离线测试不能证明真实 ChatGPT entitlement，五项 operation receipt 与当前未提交工作树的 committed-target observation 仍独立待办。

## 2026-10-10 Cursor 冷续接失败快照的消费标记顺序

只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/services/cursorSessionManager.ts` 与 `open-sse/executors/cursor.ts`：其 parked h2 流按待处理工具 ID 续接，发送成功才删除对应映射；22 项未提交 Codex 内容继续排除。该参考不定义本 Server 的多租户冷续接、请求内存预算或消费 claim，本项不推断新的 Cursor wire。

目标 `close_unusable_continuation_session` 原在取得 parked 会话后、复制语义上下文及预留内存之前写入 scope 隔离的已消费 ID。1 字节请求内存预算导致快照失败时，原实现仍留下 claim，后续重试会被误判为重复续接；固定反例先红。现仅在快照和所有内存预留成功后、关闭原会话之前写 claim，保持成功路径阻止并发重复开流的栅栏。失败路径仍由既有 reservation guard 关闭该 parked 会话，但不再占用原 ID。此项只修本地失败记账顺序，不更改 AgentService protobuf、OAuth/API Key rail、Account/Share 绑定或 receipt schema；离线回归不证明真实厂商账号接受性，两条 Cursor 真实 receipt 仍独立 `live_pending`。

## 2026-10-10 Grok SSE 混合空行与超大帧恢复

只读核对 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 和排除六项未提交 Codex 文件的 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2`。后者已提交 `openai_gateway_grok_sse_filter.go::scanSSELinesPreservingEndings` 逐行识别 CR、LF、CRLF，并对块末尾 CR 暂缓判定；参考只提供 SSE 分帧机制证据，不证明 xAI 新 wire 或真实模型权限。

目标 `GrokResponsesSseInspector` 原只识别 LF+LF、CRLF+CRLF，混合 LF+CRLF、CRLF+LF 或 CR-only 空行会把 ping/搜索帧留在缓冲中，搜索完成观察无法在完整帧到达时更新。固定分块 ping 反例先红；现按最早的两次逻辑行结束分帧，块末尾 CR 等待下一块确认是否为 CRLF，字段解析兼容 CR-only。超大帧直通器也用同一边界规则恢复后续帧，避免较早混合空行使过滤长期失效。分块、搜索观察与超大帧恢复回归通过。此项仅修 Grok OAuth Responses SSE 的本地旁路观察/保真，不改上游请求、Account/Provider/Share 绑定、主终态判定、GR-N4 版本/header、媒体、remote compaction disabled 状态或 receipt schema。离线测试不证明厂商实际使用这些换行格式；三项真实 operation receipt 与同账号版本/header 接受性继续 `live_pending`。

## 2026-10-10 Kiro EventStream 上游错误诊断的下游隔离

只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/base.rs`：参考把 EventStream `error`/`exception` 帧的 header 错误码和 payload 文本保存在内部事件中。它说明此类帧含上游不可信诊断，不提供本 Server 的多租户下游回显合同，也不推断新 Kiro wire。

目标 `wire::parse_event` 原把上游 error/exception 的 code/message 装入 `WireError::Upstream`，再由 `KiroToolJsonError::from` 的 `to_string()` 直接传给非流错误和流式错误。固定 header 与 payload 各携带 Bearer 文本的两类反例先红，确认原文回显。现转换到客户端 rail 时仅保留本地稳定的 `KIRO_UPSTREAM_STREAM_ERROR` 和通用诊断，不把上游原始 code/message 交给客户端；其他 EventStream 结构/限制错误仍保留原有本地说明。此项仅收紧错误展示，不改 Kiro/Amazon Q 请求、EventStream 解码与终态、Account/Provider/Share/auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema。离线注入不证明厂商真实返回此类敏感文本；八份真实 receipt 及专项计量证据继续 `live_pending`。

## 2026-10-10 Qoder SSE 显式状态字段的失败关闭

`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求本会话不可用的 `project-doc` 技能；读取门禁后未读取业务源码，也未运行外仓源码差分审计。最新 TokenRouter 业务增量保持 `unreviewed_skill_gate`。以下仅依据目标 Server 的 Qoder SSE 解码器、既有本地 fixture 和已冻结 CLI oracle，不推断新 COSY wire。

目标 `QoderSseDecoder::decode_event` 原先对 `statusCodeValue` 的负数、空值、字符串或越界值，以及 `statusCode` 的未知名称/错误类型，都会当作字段缺失并默认 200；两字段同时出现且冲突时也静默采用前者。固定无效状态反例旧实现先红。现对显式字段逐项解析并要求 HTTP 状态处于 100～599，两字段并存须一致；可解析的数字字符串与 `OK` 兼容写法、无状态字段时的既有默认保持。无效状态在解码阶段以固定本地协议错误拒绝，不携带原始字段值。此项不改请求 endpoint/header/payload、站点/rail、Account/Provider/Share 绑定、唯一终态+EOF 或 receipt schema v2/revision 2；离线测试不证明厂商真实状态格式。Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401 与 TokenRouter 合规补审继续缺失。

## 2026-10-10 CodeBuddy SSE 终止原因类型一致性

只读复核 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/sse.go::Aggregate`：其解析结构把 `finish_reason` 定义为字符串；该参考宽松流复制器不提供目标 Server 的严格验证规则。未跟踪 `proxy.html`、`proxy.md` 排除。本项不推断新的 WorkBuddy wire。

目标 Server 非流 `CodeBuddyChatSseAggregator` 已要求终止原因为非空字符串或 `null`，流式 `CodeBuddySseDecoder` 却允许数字、布尔值、对象进入规范化输出，且这些值不会触发 `finish_reason` 后业务帧栅栏。固定异常帧反例在旧实现先红；现流式入口拒绝显式非字符串且非 `null` 的终止原因，空白字符串与 `null` 继续按既有心跳兼容处理，合法非空字符串仍激活原有栅栏。不改变请求 endpoint/header/body、Intl/CN 站点及 Account/Provider/Share 绑定、独立 usage 尾帧、唯一 `[DONE]`+EOF、receipt schema v2/revision 2 或 CB-N4 disabled 决策。离线类型反例不能证明厂商接受性；Intl/CN 两站真实 receipt 与 CN 新模型目录证据仍 `live_pending`。

## 2026-10-10 Antigravity replay 终态后的签名捕获栅栏

只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。CLIProxyAPI 的 `antigravity_reasoning_replay.go::observeResponsePayload` 可从带 `finishReason` 的同一帧保留签名，但不定义目标 Server 同一网络块内终态后附加帧的隔离；Manager 的 pipeline/thinking store 也不提供这项字节级边界。本项不推断 Google 新 wire，只保护目标自身 replay 缓存。

目标 `AntigravityReplayStreamAccumulator` 原在识别 `finishReason` 后仍继续观察同块的后续 SSE 事件，将迟到的第二个签名并入已完成轮次；同块与分块固定反例旧实现先红。现终态帧自身的 content 仍可被捕获，但之后的事件及网络块不再改变本轮 replay。主数据面终态判定、下游输出和错误语义不在本项改动范围，不能以旁路截断声称主流已验证终态后数据。出站请求、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction、receipt schema v2/revision 2 与禁用决策均不变。两条 OAuth rail 的真实 receipt 和专项厂商接受性仍 `live_pending`；离线注入不代表 Google 实际返回此类帧。

## 2026-10-10 Claude message_delta 的结构性成功门槛

只读复核 `Claude/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：其 `claude_executor_stream.go::validateClaudeStreamingResponse` 要求出现 `message_delta`，已提交流测试使用含 `delta` 对象的样本，但参考校验器本身没有证明所有厂商合法形状。以下是目标 Server 对自身成功记账边界的局部收紧，不推断新 Anthropic wire。

目标 `AnthropicSseInspector::validate_message_delta` 原只验证 `usage.output_tokens`，允许缺 `delta`、`null`、字符串或数组的假 `message_delta` 标记 `saw_message_delta`，再接 `message_stop` 形成完成终态。固定无效结构反例旧实现先红；现要求 `delta` 明确为对象，保留空对象 `{}` 的既有兼容，usage 非负校验和终态时序不变。出站 Messages/count_tokens、CL-N7/CL-N8、Account/Provider/Share 绑定、静态目录和 receipt schema 不变。离线注入不证明真实 Anthropic 返回此类帧；五份 operation receipt 与续线程/beta 接受性仍独立 `live_pending`。

## 2026-10-10 Claude 已知内容块增量的字段类型栅栏

只读核对 `Claude/AGENTS.md` 推荐的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：`internal/translator/claude/openai/responses/claude_openai-responses_response.go` 的 `content_block_delta` 分支分别读取 `text_delta.text`、`thinking_delta.thinking`、`signature_delta.signature`、`citations_delta.citation`，字段缺失会跳过对应内容；其 `claude_openai-responses_citations_test.go` 有引用对象样本。这是字段消费行为的只读证据，不代表参考实现会严格拒绝错误类型，也不证明真实上游会返回畸形增量。

目标 `AnthropicSseInspector` 原仅匹配已知 delta 名称与内容块种类；缺少或错类型的文本、引用、思考和签名字段仍可通过内容块关闭及消息成功终态。八例组成的定向测试在旧实现上先红（首例缺 `text`），修复后八例均通过；现对已知文本/思考/签名字段要求字符串，对引用要求对象，合法空字符串保持可用。未来未知内容块类型仍保持宽松透传，工具 `partial_json` 的已有合同不变。这只收紧成功响应的本地结构边界，不修改 Claude OAuth 出站 wire、Account/Provider/Share 绑定、CL-N7/CL-N8、receipt schema 或 entitlement 判定；五份真实 operation receipt 和续线程/beta 接受性仍待独立厂商证据。

## 2026-10-10 Codex Responses 同批冲突终态失败关闭

只读对照 `Codex/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`。官方 OpenAI Docs 的 Responses streaming 指南及事件参考仅确认 `response.completed` 表示模型响应完成；它们不证明 ChatGPT 订阅账号权限，也不替代真实 Codex OAuth operation receipt。本项是目标 Server 自身的终态一致性边界，不据参考推断新出站 wire。

目标 `ResponsesSseInspector` 和非流 `ResponsesSseAggregator` 原在识别首个终态后忽略同批后续 JSON。`response.completed` 后附 `response.failed`、`response.incomplete`、重复 `response.completed` 或 `error` 的固定反例在旧实现中错误通过。现两处共用终态识别规则：后续任何声明或 payload 指示的终态/错误帧都返回固定本地协议错误；普通后置 JSON 与可选单个 `[DONE]` 保持既有处理，不回显不可信错误正文。不改变出站 Responses/Images、WebSocket、Account/Provider/Share、Images/预热启用状态或 receipt schema。离线冲突注入不能证明真实上游行为；五项真实 Codex operation receipt 与未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Cursor 活跃 h2 会话的硬容量门槛

只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/services/cursorSessionManager.ts`：参考以 100 个会话为默认上限，超限驱逐最早活动者；其 22 项未提交 Codex 工作树内容不作证据。参考允许驱逐运行中会话，目标 Server 则须维持已开 AgentService 流的所有权，因此只借鉴容量上限，不照搬运行中驱逐策略，不推断新 Cursor protobuf。

目标 `CursorSessionManager::reserve` 原在插入后仅尝试驱逐非运行中会话；全部现有会话为 `Running` 时找不到候选，新请求仍成功返回，100 个上限退化为软上限。设上限为 1、两个不同运行中会话的确定性反例旧实现先红。现若停放会话可驱逐仍先按既有 LRU 让位；若注册表依然超限，在新入口逃逸给调用者或发出 AgentService 请求前关闭并删除该入口，以固定、可重试的本地 503 容量错误拒绝，不中断原有运行流。此项不改变 OAuth/API Key 凭据、Account/Provider/Share 绑定、会话索引和续接 claim、出站 wire 或 receipt schema。两条真实 Cursor rail receipt 与当前未提交工作树的 committed-target observation 仍独立缺失；本地容量反例不证明厂商实际接受性。

## 2026-10-10 Grok 推理证明流式捕获的终态后栅栏

只读核对 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 和排除六项未提交 Codex 文件的 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2`。前者 `quality_retry_scan.go` 将 completed/incomplete/failed 视为流终态；后者的 SSE 过滤器仅处理 ping 兼容。二者均不定义本 Server 多租户推理 proof 缓存的提交规则；本项只修目标自身旁路状态，不据参考推断新 xAI wire。

目标 `GrokReplayStreamAccumulator` 原在 `response.completed` 后继续收集 `response.output_item.done`，并忽略 `error` 帧；错误先于 completed 时也可留下可提交的 proof。固定同批与分块反例先红。现一旦有错误、失败、不完整或取消终态便丢弃候选；成功终态后任何额外 JSON 也丢弃候选。单个可选 `[DONE]` 只允许在成功终态后，过早或重复均拒绝。正常分片 SSE、WebSocket JSON 及完成后单个 `[DONE]` 仍可捕获。此项不改变主 Responses 终态判定、下游输出、Grok HTTP/WS/media 出站、Account/Provider/Share、GR-N4 版本/header、remote compaction disabled 或 receipt schema。离线注入不是厂商实际接受性证明；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号版本/header 证据和当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Kiro EventStream 终态 payload 与失败状态一致性

只读复核 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `parser/decoder.rs` 和 `model/events/base.rs`：参考逐帧解析有类型事件、限制缓冲并定义错误状态，但未定义本 Server 专用的 `endEvent` 成功门槛。目标合同已要求 `endEvent` 且结构损坏失败关闭，本项不从参考推断新 Kiro/Amazon Q 事件格式。

目标 `wire::parse_event` 原无条件接受 `endEvent`，即使 CRC 正确的非空 payload 为畸形 JSON；`EventStreamDecoder::finish` 也未检查此前 `feed` 已记录的失败，容量错误后若缓冲为空可返回成功。两条固定反例先红。现空 payload 与合法 JSON payload 的 `endEvent` 继续可用，非空畸形 JSON 返回本地 `KIRO_EVENT_STREAM_INVALID`；`finish` 在任何先前失败后返回已记录错误。流式和非流式转换复用同一解析器，不改出站请求、Account/Provider/Share/auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema。离线畸形帧不证明真实厂商会发送该形状；八份真实 receipt、KI-N5 专项证据及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Codex 空文本公告中的非空引用与 logprobs 提交边界

官方 OpenAI Docs 的 [Responses 类型参考](https://developers.openai.com/api/reference/resources/responses) 将 `annotations` 列为 `ResponseOutputText` 字段；[Streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses) 描述事件流。只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6`：`codex_executor_terminal.go::isCodexEmptyPart` 用闭集类型和空 `text` 判断可缓冲公告，其 OpenAI→Responses 转换器会生成 `annotations: []`、`logprobs: []` 的空文本 part。另一推荐 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 工作树干净，继续作为已冻结差分参考，本项不从它推导新 wire。官方类型与参考形状只证明字段存在，不证明私有订阅账号会返回这些组合。

目标 `is_empty_startup_part` 原只检查 `text == ""`；非空引用、非空 logprobs 或未知内容字段仍被归为 Lifecycle，可能在已有可观察业务内容时保留透明恢复窗口。顶层 part 与 message content 中的固定反例在旧实现上先红（首例为非空引用）；现文本 part 只允许已知的 `type`、空 `text` 和缺失/空数组的 `annotations`、`logprobs`，refusal part 只允许 `type` 和空 `refusal`。其他字段 fail closed 为 Business。空数组正例保留；JSON/SSE 与 WebSocket 复用分类，原始事件正文不改写。此项只收紧 CX-N1 的本地 pre-commit 语义，不改变 Codex 出站 Responses/Images、同账号恢复、Account/Provider/Share 绑定、receipt schema 或权益判断；五份真实 operation receipt 和当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Cursor 冷续接已消费 ID 的容量失败关闭

只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/services/cursorSessionManager.ts`：参考保留 5 分钟闲置 TTL、100 个会话上限和关闭清理，但没有目标 Server 的 scope-fenced 冷续接 claim 缓存。参考工作树 22 项 Codex 本地修改全部排除。本项只修目标自己的重放栅栏容量语义，不推断新的 Cursor protobuf 或厂商 wire。

目标 `CursorSessionManager::claim_continuation_ids` 原先插入新 ID 后按最旧时间驱逐，导致未过期的旧已消费 ID 在容量压力下消失。上限为 64 的固定反例先红：先消费一个 ID，再插入 64 个竞争 ID，原 ID 不再被 `continuation_was_claimed` 识别。现在在同一写锁内去重并预检整批新增 ID；空间不足时原子拒绝新批次，不改写任何未过期 claim。调用方在新 AgentService run 之前显式关闭刚获取的 parked owner，返回固定可重试 `503 cursor_continuation_claim_capacity_exhausted`，不进行跨账号/Provider 恢复。此项不改变 Cursor OAuth/API-key 出站、session scope、Account/Provider/Share 绑定、正常继续流、receipt schema 或真实权益判断。离线容量反例不证明厂商接受性；双 rail 真实 receipt 与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Grok SSE 搜索能力证据的完整成功流提交门槛

只读核对 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `backend/internal/application/gateway/quality_retry_scan.go`：它区分 `response.completed`、`response.incomplete`、`response.failed` 终态；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `backend/internal/service/openai_gateway_grok_sse_filter.go` 只处理 ping 兼容，其六项未提交 Codex 文件排除。参考不定义本 Server 的搜索 capability evidence 持久化时机，本项只修目标自身旁路提交边界，不推断新 xAI wire。

目标 `StreamForwardState` 原在每个 SSE chunk 的 Grok 旁路观察后立即将 `response.output_item.done` 中的 completed search 写入 Account；同一流随后 `response.failed`，主语义虽判失败，账号却已获得 `Search` 能力证据。包含搜索完成与失败终态的 HTTP 回环在旧实现先红。现在只累积有界布尔候选，待 EOF 时主 Responses 检查器成功结束、本地转换与内存检查通过，才在原 Account generation 门禁下持久化；失败、协议错误、中断或容量错误均不会提交。完成终态的正例仍能写入。Grok HTTP/WS/media 出站、Account/Provider/Share 绑定、默认 CLI 版本与 group header、GR-N4、remote compaction disabled 状态和 receipt schema 均不改变。离线回环不证明真实 xAI 搜索权限；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号版本/header 接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Kiro EventStream 同块多帧的待解码容量边界

只读核对 `Kiro/AGENTS.md` 推荐、工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/parser/decoder.rs`：参考也先按整个 `feed` 输入检查 buffer 容量，再解出帧，因此它只提供 16 MiB 帧/缓冲上限的差分信号，不能照搬其网络块 admission。目标 Server 的 EventStream framing、CRC 与严格失败状态仍是自身协议合同，本项不推断新 Kiro/Amazon Q wire。

目标 `EventStreamDecoder::feed` 原在解码前用未解码 buffer 加整块输入长度比较上限，使同一块中的多个合法小帧被误判为单帧超限。将待解码上限缩小到 32 字节、在一块中提供三个各 16 字节的有效帧，旧实现返回 `BufferOverflow { size: 48, max: 32 }`；跨块保留部分前缀的变体同样需要正常排帧。现在按缓冲剩余空间分段接收，每段立即排出已完成帧，仅真正无法在容量内完成的待解码帧失败并保持原错误状态。单帧 16 MiB 上限、CRC、`endEvent` 成功门槛、请求生命周期内存预算和出站请求均不放宽；流式/非流式路径共用解码器。离线帧组合不证明厂商会按该网络块形状发送；八个 auth-kind×region 真实 receipt、KI-N5 权威计量证据与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Qoder SSE 错误后的失败状态锁存

`Qoder/AGENTS.md` 推荐 TokenRouter，但其当前 `AGENTS.md` 要求先调用本会话不可用的 `project-doc` 技能；读取门禁后没有读取该仓业务源码，也没有运行访问外仓源码的差分检查。最新参考增量继续标为 `unreviewed_skill_gate`。本项只依据目标 Server 的 Qoder 状态机、既有本地 fixture 和冻结 CLI oracle，不推断新的 COSY wire。

目标 `QoderSseDecoder` 与 `QoderChatSseAggregator` 原在 `push` 已接受 `[DONE]` 后，面对后续业务帧会正确报错，却不记住该错误；调用方若继续 `finish`，仍可能得到成功终态或非流结果。两条固定反例在旧实现先红。现两个状态机均锁存任何 `push` 错误；decoder 的 `finish` 错误也锁存，已失败时不再报告完成。首次失败保留原有上游/本地协议或内存容量分类，后续调用只返回固定脱敏本地协议错误。正常唯一终态与 EOF、COSY 出站 endpoint/header/body、Global/CN 站点及 OAuth/PAT rail、Account/Provider/Share 绑定和 receipt schema v2/revision 2 均不改变。离线反例不能证明厂商真实发送该序列；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 CodeBuddy SSE 错误后的失败状态锁存

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b`：其 WorkBuddy `sse.go::Aggregate` 在首个 `[DONE]` 即停止扫描，不能证明本 Server 要求的唯一终态加 EOF；两个未跟踪文档 `proxy.html`、`proxy.md` 排除。已冻结 CodeBuddy reference-delta 的 committed-object 复核通过。本项只修目标自身的状态一致性，不推断新的上游 wire。

目标 `CodeBuddySseDecoder` 与 `CodeBuddyChatSseAggregator` 原在 `push` 已接受 `[DONE]` 后拒绝后续业务帧，却不记住失败；后续 `finish` 仍可能成功。两条固定反例在旧实现先红。现任一 `push` 错误均锁存；decoder 的 `finish` 错误也锁存，不能用迟到的 `[DONE]` 修复。第一次错误仍保留原上游/本地协议或容量分类，后续调用只返回固定脱敏本地协议错误，失败状态不再报告完成。正常唯一 `[DONE]` 加 EOF、Intl/CN 站点、WorkBuddy endpoint/header/body、Account/Provider/Share 绑定、一次同账号 401 恢复、receipt schema v2/revision 2 和 CB-N4 disabled 决策均不改变。离线反例不证明厂商会发送该序列；Intl/CN 两份真实 receipt、CB-N4 CN 模型目录/接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Antigravity replay 有签名文本段的边界保真

只读核对 `Antigravity/AGENTS.md` 推荐的两个干净提交态：`CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 的 `antigravity_reasoning_replay_test.go::TestAntigravityReasoningReplayDirectSignatureClosesSegmentBeforeUnsignedText` 明确验证有签名文本后接无签名文本须分别保留，`TestAntigravityReasoningReplayKeepsMixedTextToolTextFingerprintsSeparate` 验证多个有签名文本段的 fingerprint/位置；`Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的 `payload_audit.rs` 测试验证简化 Gemini parts 时保留原有签名，但它的 turn-anchor 策略不能代替目标 replay 的逐段匹配。冻结 Antigravity reference-delta 的 committed-object 审计通过。这些是签名归属差分，不据此推断新 Google wire。

目标 `merge_stream_content` 原将同类相邻文本无条件拼接，前段已有 `thoughtSignature` 时仍吸收后段无签名文本，或被后段第二个签名覆盖；捕获到的文本 hash 因而与客户端分段续接历史不符，无法精确回填。以有签名段后接无签名段的目标闭环反例先红。现在非空文本一旦带有效原生签名便关闭该段，后续文本单独保留；无签名前缀仍可与最后的签名碎片合并，同类思考/可见文本与工具调用原有分界不放宽。两种签名后续组合及前缀兼容正例转绿。此项只修 replay 旁路捕获，不改变主数据面成功判定、出站请求、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3/compaction disabled、receipt schema v2/revision 2 或终态前提交的既有策略。离线分段样本不证明真实 Google 上游会如此分块；`antigravity_oauth`/`agy_oauth` 两份真实 receipt、AG-N6/AG-D3 专项接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Claude SSE 本地协议诊断不回显上游字段

只读核对 `Claude/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `internal/runtime/executor/claude_executor_stream.go::validateClaudeStreamingResponse`：畸形 JSON、缺少 message id/model 等协议错误使用固定本地文字；普通上游 `error` 事件仍可携带其原始错误消息。这只是诊断边界的差分参考，不意味着两者的 SSE 状态机或厂商接受性相同。

目标 `AnthropicSseInspector` 原把不可信 `event` 名、未知 payload `type` 和未知 `delta.type` 拼进本地协议错误；`forwarder` 会把这些错误作为 502 返回下游。注入 `Bearer` 文本的检查器单测和 HTTP 回环均在旧实现先红，HTTP 错误确实回显了该文本。现在事件名与 payload 类型不一致时返回固定说明，未知事件在 `message_start` 前及未知 delta 类型均用固定占位名；已知 `text_delta` 等诊断名保留。正常上游 `error` 事件、成功流、出站请求、Account/Provider/Share 绑定与 receipt schema 不变。离线恶意帧不能证明真实上游会发送该内容；五份真实 Claude operation receipt、续线程/beta 厂商接受性和当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Codex Responses 检查器错误状态不可洗白

官方 OpenAI Docs 的 [Streaming API responses](https://developers.openai.com/api/docs/guides/streaming-responses) 把 Responses 流描述为带类型的 SSE 语义事件，列出 `response.completed` 和 `error`；它不规定目标 Server 的可复用检查器在本地失败后的状态。只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `codex_websockets_stream.go`、`codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192` 已提交 `proxy/wsrelay/executor.go`：两者都将明确的完成/失败/错误事件作为当前流读取的终止边界，但没有本 Server 的检查器失败锁存合同。本项只修目标本地状态，不从参考推断私有 Codex wire。

目标 `ResponsesSseInspector` 原在成功终态后遇到下一次 `push` 的冲突终态或畸形 JSON 时会报错，却仍保留可见成功终态，随后 `finish` 可以返回成功；`finish` 先因缺终态报错后，迟到的完成事件也可能把同一检查器恢复成成功。固定两组反例在旧代码先红。现首次 `push`/`finish` 错误按原协议、容量或缺终态类型返回，之后锁存为固定本地协议错误且不再暴露成功终态。正常成功、失败、不完整、可选唯一 `[DONE]` 与已有出站请求不变；HTTP 转发器原本在首次检查错误时就中止，本项不声称改变其已提交响应。五份真实 operation receipt、Astra/Image 2.5/WS prewarm 厂商接受性和当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Cursor 冷续接已消费 ID 的原子拒绝

只读核对 `Cursor/AGENTS.md` 推荐、HEAD 为 `443d66996d69d7ecf887fd61158404a3cca1b192` 的 OmniRoute 已提交 `open-sse/services/cursorSessionManager.ts`：参考保留同一 h2 流、5 分钟空闲 TTL、100 会话容量及关闭时清理，未定义本 Server 的跨请求冷续接 ID claim。其工作树中的未提交 Codex 文件继续排除；`CUR-N2` 的 no-new-wire 结论不变。本项只修目标多租户 replay fence，不推断 Cursor 厂商的 ID 唯一性保证。

目标 `claim_continuation_ids` 原在持有写锁时只计算新 ID 数量；遇到已消费的同作用域 ID 不报冲突，反而刷新其 claim 时间并把同批新 ID 写入。第二条 parked 会话复用旧 tool-call ID 时，`close_unusable_continuation_session` 因而可返回可用冷续接快照。session manager 的整批原子反例和 agent driver 的第二 parked owner 反例均在旧实现先红。现先在同一写锁中拒绝任一未过期旧 ID，不插入同批新 ID；调用方关闭已取得的 parked owner，返回既有 `cursor_continuation_in_progress`。不同 scope 的同字面 ID 与正常新 ID 保持可用，容量耗尽仍返回独立的可重试 503。OAuth/API-key 出站、protobuf、Account/Provider/Share 绑定、模型目录和 receipt schema v2/revision 2 不变。离线 ID 重用反例不能证明厂商会如此发送；两条真实 receipt 和当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-10 Grok SSE 成功终态后的 EOF 与质量证据提交边界

只读核对 `Grok/AGENTS.md` 推荐的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `quality_retry_scan.go`：参考将 `response.completed`、`response.incomplete`、`response.failed` 识别为终态；`sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 SSE ping 过滤器只提供兼容证据，其六项未提交 Codex 文件排除。两者都不定义目标 Server 的流 EOF 或质量指标提交时机。本项只根据目标自己的严格 Responses 合同收紧生命周期，不推断新 xAI wire。

目标 `StreamForwardState` 原在见到 Grok `response.completed` 后停止读取上游，把语义终态误当成传输 EOF；因此后续跨块冲突终态无法触发主语义检查，先前声称的搜索证据“等到 EOF”门禁实际上可能提前通过。同时 `inspect_grok_responses_chunk` 在完成帧到达时立即记录质量样本，后续传输或本地错误无法撤回。跨块 `response.completed` 后再送 `response.failed` 的 HTTP 回环在旧实现先红。现在仅 Grok Responses SSE 在语义终态后继续读取到真实 EOF；主检查器可以拒绝尾随冲突，搜索证据与质量样本都在 EOF、主语义成功及本地尾部转换/容量检查通过后提交。完成终态加真实 EOF 的正例仍分别写入搜索证据和脱敏质量计数。HTTP JSON/WS 质量路径、其他 Provider 读取策略、出站请求、Account/Provider/Share 绑定、receipt schema 和 GR-N4 决策不变。离线尾随帧反例不证明真实 xAI 会发送此序列；三项真实 Grok receipt、GR-N4 同账号版本/header 接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Kiro EventStream 的流式终态后 EOF 校验

只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/anthropic/handlers.rs::create_sse_stream`：参考持续读取上游 body，读错误与 EOF 分开处理；其事件模型不定义本 Server 的 `endEvent` 成功证据。目标自身的冻结 EventStream 合同要求 `endEvent` 且 `framesAfterTerminal=error`，流式和非流式不应因网络块切分而分歧。本项不从参考推断新 Kiro/Amazon Q wire。

目标非流式转换检查整份 body，能拒绝 `endEvent` 后的任何帧；流式转换原在看到 `endEvent` 的当前块就调用 decoder `finish` 并退出，根本不读取下一块。固定“完整尾随帧”和“半截尾随帧”两个跨块反例在旧实现先红，均错误输出正常 `message_stop`。现在流式路径持续读取到上游真实 EOF，再完成 decoder 与 builder；后续完整帧被终态栅栏拒绝，半截帧由 EOF 检查拒绝。仅 `endEvent` 后正常 EOF 的正例保留成功。传输/idle 错误也不再被终态提前退出掩盖。请求构造、CRC/容量、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 和 receipt schema v1/revision 1 不变。离线尾随帧不证明真实厂商会发送此序列；八份真实 receipt、KI-N5 计量证据与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Qoder SSE wrapper 的显式形状失败关闭

`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求先调用本会话未加载的 `project-doc` 技能；读取门禁后即停止，没有读取其业务源码或运行访问外仓的 `--check-sources`。最新参考业务增量保持 `unreviewed_skill_gate`。本项只依据目标 Server 的 SSE decoder、已有空心跳兼容规则与冻结 CLI oracle，未推断新 COSY wire。

目标 `QoderSseDecoder::decode_event` 原把非对象 JSON wrapper 的 `get("body")` 结果，以及显式非字符串 `body`，统一降为 `""`；随后单独 `[DONE]` 能令流成功。`null`、数组、标量外层和 `body: null`/对象/数值六个反例先红。现要求外层为对象；`body` 可缺失以保留空心跳，但显式存在时必须是字符串，否则返回固定本地协议错误并锁存失败。Global OAuth、CN OAuth、Global PAT 各经 Claude/Codex/Gemini Surface 的 HTTP 回环确认返回 502、只发一次生成请求且不 refresh。成功 wrapper、唯一终态+EOF、COSY endpoint/header/body、Account/Provider/Share 绑定、共享 attempt budget 与 receipt schema v2/revision 2 不变。离线反例不证明厂商真实发送畸形 wrapper；三条 rail 真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 CodeBuddy choice 形状与 index 的双路径一致性

只读复核 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/sse.go::Aggregate`：参考仅取 `Choices[0]`，其宽松复制器并未定义本 Server 的多 Surface choice 校验；未跟踪 `proxy.html`、`proxy.md` 排除。本项不推断新的厂商 wire。目标非流聚合器已拒绝非对象 choice 与非零 index，但不可解析的显式 index 默认为 0；流式 decoder 则可转发非对象 choice、非零或不可解析 index，随后 `[DONE]`+EOF 仍可成功。固定反例在旧实现先红。现两路共享校验：choice 必须为对象，显式 index 必须可解析为 0；缺失 index、数值 0 和兼容字符串 `"0"` 继续可用。Intl/CN × Claude/Codex/Gemini × 流/非流的本地 HTTP 回环拒绝畸形 index 为 502，且不触发账号 refresh。唯一终态+EOF、请求 endpoint/header/body、Account/Provider/Share 绑定、receipt schema v2/revision 2 与 CB-N4 disabled 决策不变。离线反例不证明真实厂商发送该形状；两站真实 receipt、CB-N4 CN 目录/接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Antigravity replay 历史指纹只忽略协议元数据

只读核对 `Antigravity/AGENTS.md` 推荐的干净 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 reasoning replay context fingerprint 测试，以及干净 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 对 Gemini part 签名位置的处理。两者只提供上下文隔离与签名位置的差分信号，不定义本 Server 的历史指纹算法或新 Google wire。

目标 `canonical_history_value` 原递归剔除所有名为 `thoughtSignature`、`thought_signature`、`cache_control` 的字段，连工具参数和工具结果中的同名业务键也被忽略。已缓存签名的轮次在工具结果或无 ID 工具调用参数变更后，仍可能被错误回填；两组闭环反例在旧实现先红。现历史指纹只忽略 Gemini content/part 上的签名与缓存元数据，以及 part 的 `extra_content.google.thought_signature`；任意层级工具参数/结果业务字段原样参与规范化和匹配。真正的 part 元数据变化仍可复用，业务字段变化则拒绝 replay。此项只修目标本地 AG-D3 replay 上下文隔离，不改 Antigravity/AGY 请求 wire、Account/Provider/Share 绑定、AG-N6 requestType、终态前提交策略、receipt schema 或 compaction 决策。离线反例不证明厂商真实发送同名业务键；两条 OAuth rail receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Claude SSE 检查器失败状态不可洗白

只读核对 `Claude/AGENTS.md` 推荐、工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 已提交 `claude_executor_stream.go::validateClaudeStreamingResponse`：畸形流数据和缺必要消息事件会返回失败。参考是错误分类与生命周期差分，不定义本 Server 检查器对象能否在失败后复用。本项不推断新 Anthropic wire。

目标 `AnthropicSseInspector` 原在成功 `message_stop` 后收到冲突帧时，`push` 虽报错却仍暴露成功终态，`finish` 也可能成功；另在 `finish` 因缺终态失败后，迟到的 delta/stop 可把同一检查器补成成功。两组固定序列在旧实现先红。现首次 `push`/`finish` 错误仍保留原分类，失败状态锁存；后续调用固定返回本地脱敏错误，`terminal` 与 `message_id` 不再暴露。正常 start/delta/stop、上游 `error` 事件、混合换行和既有响应转换不变。HTTP 转发器本来会在第一次错误时中断，本项不声称改变已提交的下游响应；Claude OAuth 出站 Messages/count_tokens、Account/Provider/Share、CL-N7/CL-N8、静态目录与 receipt schema 不变。离线状态序列不证明真实厂商行为；五份真实 operation receipt、thread/beta 厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Codex 空输出项公告的字段闭集门槛

按 `Codex/AGENTS.md` 只读复核干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`。前者 `codex_executor_terminal.go::isCodexBufferableOutputItem` 只把已知且无内容的 model 输出项留在启动缓冲，未知 item 类型和已知服务端操作立即释放；后者没有本 Server 的预提交字段闭集合同。官方 [OpenAI Docs Responses streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses) 仅确认带类型语义事件，不证明私有订阅会发送下述未知字段。本项不据参考推断新出站 wire。

目标 `is_empty_startup_output_item` 原只检查 `content`、`summary`、`arguments` 或 `input` 是否为空，忽略同一 item 的未知业务字段；元数据字段若被伪装为对象也可误判为空公告。固定 message/reasoning/function_call/custom_tool_call 未知字段与对象 role 五种反例，首例在旧实现先红；现每种可缓冲 item 仅接受其已知字段，身份、状态和名称元数据必须为字符串；其他形状按 Business 提前关闭透明重试窗口。标准 `id/status/role/call_id/name` 空公告正例保留。此项只收紧 CX-N1 本地分类，不改事件正文、Codex OAuth/Images/WebSocket 出站、Account/Provider/Share 绑定、预热开关或 receipt schema。离线反例不证明厂商实际发送这些字段；五份真实 operation receipt、厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Cursor 过期二级索引候选不得遮蔽有效停放会话

按 `Cursor/AGENTS.md` 只读核对 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 `open-sse/services/cursorSessionManager.ts`：参考在 `findByToolCallIds` 和 `acquire` 前先执行 `evictExpired`，只在停放态寻找可续接会话。参考工作树中未提交的 Codex 变更全部排除。它不定义目标 Server 的多租户 scope、响应 ID 二级索引或冷续接 claim；本项只借鉴过期清理的查找边界，不推断新 Cursor wire。

目标 `CursorSessionManager::resolve_index_binding` 原只核对候选是否仍在主表；一个已过期的停放会话与另一个有效停放会话共用同一 scoped response/tool ID 时，两个候选仍使查找返回 `None`。固定双索引反例首个 response 查找在旧实现先红。现查找前执行现有惰性过期清理，读取索引时再次排除已关闭或已过期候选；正在被其他请求持锁的候选仍按可能有效处理，避免错误选择。有效会话保留，过期会话及其索引清除。此项只修本地会话解析，不改 AgentService h2/protobuf、OAuth/API Key、Account/Provider/Share 绑定、冷续接 claim、完成响应缓存或 receipt schema。离线碰撞反例不证明真实厂商会复用 ID；两条真实 rail receipt 与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Grok replay 工具身份保持精确字符串边界

按 `Grok/AGENTS.md` 只读核对干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交实现；后者六处未提交 Codex 改动排除。前者的 reasoning 隔离和 Responses 终态扫描、后者的 SSE ping 兼容仅提供差分参考，不定义目标 Server 的工具名或调用 ID 规范化规则。本项依目标既有“调用名/ID 编辑不可复用旧 proof”合同修补本地上下文边界，不推断新 xAI wire。

目标 `call_fingerprint` 原经 `bounded_text` 裁剪工具名，`capture_output` 与 `apply` 也裁剪 `call_id`。客户端把 `lookup` 改成 ` lookup ` 时，旧实现错误注入原 proof；固定反例先红。现工具名只检查非空白及 256 字节上限，原始字节参与 SHA-256；调用 ID 在上游捕获和客户端预检都要求非空、至多 256 字节且无首尾空白。完全相同的合法带空白工具名仍能命中，改名或带空白 ID 不能借用旧 proof。仅收紧本地 Grok reasoning replay，不改 HTTP/SSE/WS 出站字段、Account/Provider/Share scope、终态/恢复预算、GR-N4 或 receipt schema。离线反例不证明厂商会发送这些字段；三项真实 Grok receipt、GR-N4 接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Kiro toolUseId 分片身份一致性

按 `Kiro/AGENTS.md` 只读核对干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/anthropic/stream.rs::ToolJsonAccumulator` 和 `src/kiro/model/events/tool_use.rs`：参考按 `toolUseId` 汇集 JSON 片段，最后一片 `stop=true` 后解析，并允许无参工具收尾；未定义同 ID 的名称漂移或完成后 ID 重用的目标 Server 合同。本项仅收紧本地多 Surface 工具身份，不推断新 Kiro/Amazon Q wire。

目标 `ToolJsonAccumulator` 原将同 ID 后续分片的不同非空 `name` 忽略，仍把 JSON 片段拼接并以第一个名称输出；`stop=true` 后又可用相同 ID 产生第二次工具调用。非流式转换和流式 builder 的固定反例在旧实现先红。现同 ID 的两个不同非空名称以及已完成 ID 的再次使用都返回固定、脱敏的 `KIRO_EVENT_STREAM_INVALID`；首片空名称后补名、同名分片、无参工具恢复及既有坏 JSON best-effort 边界不变。已完成 ID 集合计入流 retained memory。此项不改 EventStream CRC/终态/EOF、出站请求、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线冲突帧不证明真实厂商发送；八份真实 receipt、KI-N5 专项计量证据及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Qoder 工具调用显式 index 不得隐式回退

`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求当前会话未加载的 `project-doc` 技能；读取门禁后即停止，没有读取外仓业务源码或运行其 source audit。最新 TokenRouter 业务增量继续 `unreviewed_skill_gate`。本项只依据目标 Qoder Chat/SSE 代码、既有单选项和工具数量上限合同，以及冻结的自包含 CLI oracle，不推断新 COSY wire。

目标 `canonical_qoder_message_delta` 原将显式不可解析的 `tool_calls[].index` 回退到数组位置，`QoderChatSseAggregator` 则回退到 0；兼容工具事件的外层 `index` 也默认 0。于是 `index: null` 等畸形工具身份可被改写后由 `[DONE]` 洗成成功，固定反例在旧实现先红。现三个入口共用校验：缺失仍按原位置/0，显式值须能解析为非负整数且小于 128；非法字符串、空值、负数、越界值一律在转发/聚合前返回本地 502。数字字符串等既有合法写法不变。只收紧本地工具分片身份，不改 COSY 签名/endpoint/body、三 rail 与站点选择、Account/Provider/Share 单绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线反例不证明厂商真实发送；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Codex 错误帧不得被后续成功终态洗白

只读核对 `Codex/AGENTS.md` 推荐且干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 与 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`。CLIProxyAPI 的 Codex terminal helper 将 `error` 视为失败事件；codex2api 的 WebSocket relay 将上游 `error` 转为失败终态。参考仅支持“错误不能被后续成功覆盖”的本地判定，不证明私有订阅会实际发出冲突序列，也不要求复制其传输策略。

目标普通 HTTP Responses 的 `ResponsesSseInspector` 原把 `error` 暂存，后续 `response.completed` 会将流记为成功；非流 `ResponsesSseAggregator` 同样留下 `last_error` 却返回 completed JSON。跨网络块的两条固定反例在旧实现先红。现流式检查器在后续完成帧处把暂存错误提升为失败终态，不转发伪成功帧；非流聚合器以原错误详情返回上游失败。`error` 后 EOF、`[DONE]` 或显式 `response.failed` 仍按既有失败路径处理。此项仅修本地 HTTP SSE/非流聚合的终态优先级，不改 Codex OAuth/Images/WebSocket 出站、Account/Provider/Share 绑定、恢复预算或 receipt schema。五份真实 operation receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Codex 错误帧不得被后续 incomplete 降格

只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@0f96f568e4dbf6f84ad7399a74b78344c5eac7e6` 和 `codex2api@b1bfe1b4ae591e19c4028f943b784ca65981e192`；前者的 `codex_executor_terminal.go::codexTerminalFailureBody` 把 `error` 作为失败信号。官方 OpenAI Docs [Responses streaming 指南](https://developers.openai.com/api/docs/guides/streaming-responses) 区分 `error` 与完成事件，但不规定这种冲突序列的优先级。优先级来自目标 Server 已有“错误后 completed 失败”及非流聚合合同，不据参考或文档声称厂商会发该形状。

目标流式 `ResponsesSseInspector` 原只在 `error` 后的 `response.completed` 提升暂存错误，后接 `response.incomplete` 却记为普通 incomplete；非流聚合器已拒绝两种后续终态。跨块固定反例先红。现两种无错误终态均不能覆盖先到的错误：流式保留原内部失败分类；Codex OAuth HTTP 回环在已提交业务输出后发送公开 `error` 和合成 `response.failed`，不转发伪 incomplete 或 `[DONE]`，usage 记 `provider_failed`。上游容量码的公开 `server_error` 映射保持不变，内部仍按原错误分类。无前置错误的正常 incomplete、显式 `response.failed`、EOF/DONE 失败路径保持。仅收紧本地普通 Responses HTTP SSE 终态优先级，不改出站 Responses/Images/WS、Account/Provider/Share 单绑定、同账号恢复预算或 receipt schema。五份真实 operation receipt、厂商接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-10 Cursor 停放 h2 会话的主动空闲到期

只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/services/cursorSessionManager.ts`：参考在停放时设置 idle timer，获取或关闭时取消，定时器到期会关闭 h2 请求/连接；其 22 项未提交 Codex 文件均排除。参考的主动清理是资源生命周期证据，不定义本 Server 的多租户 scope、索引或内存预算，也不推出新 Cursor wire。

目标 `CursorSessionManager` 原仅在后续获取、解析或准入请求时惰性清理过期停放会话；若请求停止，h2 流和 request-memory reservation 可无限期留在进程。无后续注册表请求的固定反例在旧实现超过 TTL 后仍持有会话并先红。现停放时设置只持弱引用的可取消到期任务；续接或关闭会取消旧任务，到期时再次核对精确 entry、状态和最新活动时间，才关闭流、释放内存并清理 response/tool 索引。既有惰性清理保留作兜底。此项只修本地会话资源生命周期，不改 AgentService protobuf、OAuth/API Key 认证、Account/Provider/Share 绑定、冷续接 claim、模型目录或 receipt schema v2/revision 2。离线资源反例不证明厂商真实接受性；两条 rail 真实 receipt 与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-11 Grok 图片编辑 multipart 字节边界

按 `Grok/AGENTS.md` 只读核对干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 Grok media 处理；后者六项未提交 Codex 变更排除。`sub2api/backend/internal/service/grok_media.go::parseGrokMediaMultipartRequest` 使用标准 MIME multipart reader，按分片读取字节且允许文本字段没有 Content-Type。参考提供分隔行和字段类型的差分信号，不定义本 Server 的 direct-XAI 模型 entitlement 或新出站 wire。

目标 `split_multipart_parts` 原把图片中任意 `--boundary` 字节当作分片起点，`trim_part_data` 又删除图片末尾全部 CR/LF；普通文本字段也被强制要求 Content-Type。带内嵌 boundary 字节和尾随换行的图片、无 Content-Type 的 prompt 两条反例均先红。现仅识别位于完整分隔行、且后缀合法的 boundary；只移除分隔行本身的换行，不裁剪图片数据；Content-Type 只对图片必需，缺失结束边界显式 400。既有图片签名/MIME/数量及不支持字段的出站前校验保持。仅修本地入站 multipart 解码，不改 Grok OAuth Account/Provider/Share 单绑定、Responses/SSE/WS、remote compaction、GR-N4 版本/header、receipt schema 或图片上游端点。离线字节反例不证明真实 xAI 接受性；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号证据与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-11 Kiro 工具 JSON 错误的上游标识隔离

按 `Kiro/AGENTS.md` 只读核对干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：其 `ToolUseEvent` 将 `toolUseId`、`name`、`input` 作为上游事件字段，`ToolJsonAccumulator` 依 ID 聚合分片。这只证明字段来源与分片关系，不定义本 Server 多租户错误展示策略，也不推出新的 Kiro/Amazon Q wire。

目标虽已隔离上游 `error`/`exception` 帧诊断，`KiroToolJsonError` 的无效/未完结 JSON 展示原会拼入上游工具 ID 和名称。将二者设为伪凭据的流式与非流式 EventStream 反例先红。现对下游保留 `TOOL_JSON_INVALID` / `TOOL_JSON_INCOMPLETE` 分类，只公开固定说明和本地缓冲字节数；内部聚合仍保留原始字段用于正常工具输出及诊断。仅收紧失败展示，不改变出站请求、EventStream 终态/EOF、Account/Provider/Share 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不证明厂商真实发送这些值；八份真实 receipt、KI-N5 专项计量证据及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-11 Qoder usage 总数一致性与溢出边界

`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求当前会话能够调用 `project-doc`，但本会话未加载，因此读取门禁后未读取其业务源码，也未运行外仓 `--check-sources`。本项仅依据目标 Server 既有 Chat usage 兼容合同、冻结 CLI oracle 和本地解析器，不推断新 COSY wire；最新参考增量仍为 `unreviewed_skill_gate`。

目标 `canonical_qoder_usage` 原接受小于输入＋输出的显式 total，缺 total 时以饱和加法把溢出静默写成 `u64::MAX`；跨 chunk 合并也使用饱和和。原生 wrapper decoder 和非流聚合器的固定反例先红。现两桶求和必须精确可表示，显式 total 须至少为该和；合并后的极端桶值溢出也按本地协议错误拒绝。合法大于两桶之和的 total 与缺 total 时精确补和保留。仅修本地用量一致性，不改变出站 COSY body/signature、两站点/三 rail 的 Account/Provider/Share 绑定、终态/EOF、401 预算或 receipt schema v2/revision 2；离线反例不证明真实厂商计费。三份真实 rail receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-11 CodeBuddy 流式 usage 外层形状一致性

只读复核 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@a6502521e1e509a4257153025c642240c39cd54b` 已提交 `internal/providers/workbuddy/sse.go::Aggregate`：参考把 `usage` 作为上游任意 JSON 字段承载，不定义目标 Server 的多 Surface 计量展示边界；两个未跟踪参考文档仍排除。目标非流聚合器已要求 usage 为对象，流式 decoder 却把显式数字、字符串和数组原样转发，随后可由 `[DONE]` 与 EOF 输出成功。固定反例旧实现先红；现流式在规范化前要求非空 usage 为对象，`null` 当作缺失移除，合法对象（包括零值）保持。Intl/CN × Claude/Codex/Gemini × stream/non-stream 的 HTTP 回环均以本地 502 拒绝非法形状，只向绑定账号发送一次生成请求，不触发 refresh。仅修本地流/非流一致性，不改 CodeBuddy 出站 endpoint/header/body、站点/Account/Provider/Share 绑定、唯一 `[DONE]`＋EOF、缓存计量字段优先级或 receipt schema v2/revision 2。离线异常帧不证明厂商真实发送；Intl/CN 两份真实 receipt、CB-N4 接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Cursor 同 ID 的 Running/parked 会话不能按可获取性选边

只读复核 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/services/cursorSessionManager.ts`：`findByToolCallIds` 仅扫描 parked 会话并返回第一个匹配者。这说明参考的单账号续流机制，但不能给目标 Server 的多租户、同作用域重复 ID 仲裁提供安全保证；参考工作树中 22 项未提交 Codex 内容继续排除，`CUR-N2=reviewed_no_wire_delta` 不变。

目标 `resolve_index_binding` 将 Running 和 parked 的存活候选都计入唯一性，而 `acquire_resolved` 只允许取得 parked。审查调用路径后保留现有行为：同一 ID 的第二个请求可能是 Running owner 的重复结果，若只因另一 parked owner 可获取就选它，会把结果写入错误的 h2 流。新增确定性回归覆盖同 scope 下 Running/parked 同时绑定相同 response/tool-call ID 时均不可解析；Running 关闭并清理索引后，parked 才恢复为唯一候选。此轮不改生产 session/protobuf、OAuth/API-key rail、Share/账号作用域或 receipt schema；测试只确认本地保守仲裁，不能证明真实 Cursor 账号接受性。两条真实 receipt 与当前未提交工作树的 committed-target observation 仍缺，保持独立 `live_pending`；未做 UI 自动化或部署测试。

## 2026-10-12 Grok ping 帧不得吞掉失败或终态 JSON

按 `Grok/AGENTS.md` 只读核对推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交对象。前者以 SSE/identity 请求上游；后者为严格 Responses 客户端把厂商费用/保活 `event: ping` 转为注释，并接受无 `type` 的费用 JSON。`sub2api` 工作树六项未提交 Codex 改动全部排除。这些只提供 ping 兼容背景，不保证异常 payload 的厂商形状或主语义优先级；GR-N4 同账号版本/header 接受性保持待证。

目标 `grok_sse_event_is_ping` 原只要求 JSON `type` 不存在或为 `ping`，因此 `event: ping` 携带非空 `error` 时也被改成 `: ping`，错误载荷消失。固定单元反例在旧实现先红。现先用共享 Responses 语义分类，失败、错误帧、不完整或成功终态一律原样交给主门禁，仅规范化普通 ping；无 `type` 的费用载荷 `{"cost":"0"}` 保持兼容。HTTP 回环把已完成搜索和成功终态之后的错误放进 ping，主门禁在响应提交前返回 502，搜索能力证据不落盘。此项只修 Grok OAuth Responses SSE 入站保真，不改出站 URL/header/body、HTTP JSON/原生 WS、Account/Provider/Share 绑定、GR-N4、remote compaction disabled 或 receipt schema v1/revision 1。离线冲突帧不证明 xAI 实际发送；inference、media、remote_compaction 三份真实 receipt、GR-N4 同账号证据与当前未提交工作树 committed-target observation 仍缺，不做 UI 自动化或部署测试。

## 2026-10-12 Kiro EventStream 异常诊断不回显事件与 header 名

按 `Kiro/AGENTS.md` 只读复核干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/base.rs`：参考把未知事件类型归为固定的 `Unknown`，说明原始 `:event-type` 是上游输入而非客户端可信诊断。参考不定义本 Server 的错误展示边界，本项不推断新的 Kiro/Amazon Q EventStream wire。

目标此前已遮蔽上游 `error/exception` 诊断和工具 JSON 标识，但 `KiroToolJsonError::from` 对其他 `WireError` 仍直接使用 `to_string()`。`InvalidEventPayload` 会拼入上游事件名与解析器文字，`DuplicateHeader` 会拼入上游 header 名；用伪凭据字段构造的反例在旧实现先红。现只在面向客户端的转换处把这两类消息改成固定说明，保持 `KIRO_EVENT_STREAM_INVALID` 分类与内部原始错误。真实编码帧的非流式转换及流式桥接回归均证实不回显伪凭据；其他结构/CRC/容量诊断与合法帧行为保持。此项不改出站请求、EventStream 帧解码、终态/EOF、Account/Provider/Share 与 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线恶意 header 不证明厂商实际发送；八份真实 receipt、KI-N5 独立计量证据与当前未提交工作树 committed-target observation 仍缺，未做 UI 自动化或部署测试。

## 2026-10-12 Qoder 工具调用 ID 不透明身份保真

`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求当前会话先加载不可用的 `project-doc` 技能。本项未读取推荐仓业务源码，也未运行外仓 `--check-sources`，最新增量仍为 `unreviewed_skill_gate`。只依据目标 Server 的 Chat 规范化、非流聚合器和冻结的自包含 CLI oracle 确认本地身份边界，不推断新 COSY wire。

目标 `canonical_qoder_tool_call` 原复用会裁剪字符串的工具名称 helper 读取调用 ID，把 `" call-a "` 输出为 `"call-a"`；后续聚合器又按 ID 精确比较同一索引的分片，因而可能把两个不同 ID 错误合并。固定保真反例在旧实现先红。现 ID 仅用裁剪结果判定是否全空，非空值原样保留；名称裁剪、空 placeholder、显式索引校验及聚合器精确冲突拒绝保持。仅修本地工具身份保真，不改 COSY 出站请求/签名、两站三 rail、Account/Provider/Share 绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线空白 ID 不证明厂商实际发送；Global OAuth、Global PAT、CN OAuth 三份真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺；未做 UI 自动化或部署测试。

## 2026-10-12 CodeBuddy 缺失 index 时工具 ID 精确找回

按 `CodeBuddy/AGENTS.md` 只读复核推荐 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 的已提交 WorkBuddy SSE；自上次已审 `a6502521e1e509a4257153025c642240c39cd54b` 后，`internal/providers/workbuddy/` 没有源码差异。两个未跟踪参考文档继续排除。参考按 index 聚合，但不定义目标 Server 缺 index 时的并行工具身份回退；本项依据目标已有的精确 ID 稳定字段合同和模型相关的可缺 index 兼容边界，不推断新厂商 wire。

目标非流聚合器原在无 index 时先对上游工具 ID `trim()` 再查找，已保存的 ID 却是原始字符串。并行首帧包含 index 0 的 `call-a` 与 index 1 的 `" call-b "`，第二帧仅给 `" call-b "` 且顺序变为首位时，旧实现找不到 index 1，按位置误选 index 0 并报“ID 跨分片变化”；固定反例先红。现只以裁剪结果判断 ID 是否全空，非空 ID 按原值查找，显式 index、无 ID 的位置回退、稳定 ID 冲突拒绝和参数聚合不变。仅修本地非流工具分片归属，不改 WorkBuddy endpoint/header/body、Intl/CN 站点、Account/Provider/Share 单绑定、唯一 `[DONE]`+EOF、同账号恢复预算、receipt schema v2/revision 2 或 CB-N4 disabled。离线反例不证明厂商实际发送带空白 ID；两站真实 receipt、CB-N4 CN 模型目录/接受性和当前未提交工作树 committed-target observation 仍缺；未做 UI 自动化或部署测试。

## 2026-10-12 Antigravity 思考 token 的 OpenAI usage 明细

只读核对 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 已提交差分：`ab151f5d` 将 `thoughtsTokenCount` 计入 Chat completion 总数并显示 `reasoning_tokens`，`dfe0a0ef` 在无工具的流式 delta 省略 `tool_calls:null`；后者在目标实际流桥未见对应空字段，无需改动。另一推荐 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 仅保留既有签名背景。本项不把参考转换器当作 Google 原始计费权威，也不据此推断新上游 wire。

目标实际 Gemini→Anthropic→OpenAI 流桥已经把候选和思考 token 合计为输出总数，但此前没有把明确的 `thoughtsTokenCount` 保留为 OpenAI Responses `output_tokens_details.reasoning_tokens` 或 Chat `completion_tokens_details.reasoning_tokens`；双输出断言在旧桥先红。另一个 Gemini→OpenAI 辅助转换函数连 completion 总数也只计候选，候选 5／思考 7、仅思考 7 的反例先红。现只对 OpenAI 目标的 Gemini usage 帧附加显式思考明细，并让中间 Anthropic→OpenAI 两层保留该明细；非流双 rail 的 Chat/Responses 也在转换时附加，Claude 原生 usage 保持原形。辅助转换总数改为候选＋思考；缺失思考字段仍沿用既有输出，显式零会保留零明细。此项只修本地结果计量投影，不改变 Antigravity/AGY 出站请求、Account/Provider/Share 绑定、AG-N6 requestType、AG-D3 replay/compaction、终态/EOF 或 receipt schema v2/revision 2。离线计量 fixture 不能证明 Google 厂商用量口径；两条 OAuth rail 的真实 receipt、AG-N6/AG-D3 专项接受性和当前未提交工作树 committed-target observation 仍缺。未做 UI 自动化或部署测试。

## 2026-10-12 Claude deferred tool 的默认缓存断点

只读复核 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b`。其已提交 `claude_executor_cloaking.go::injectToolsCacheControl`（源于 `fe4ae4989`）在没有现有 tool 断点时选最后一个非 `defer_loading` 工具；`caching_verify_test.go` 覆盖尾部 deferred 和全部 deferred。该参考是缓存位置差分，不证明真实 Anthropic 对目标请求的接受性。参考最近新增的原生 `tool_search`、能力开关及每请求固定决策也已检查；目标当前走兼容式函数桥，不能把其行为当作已实现的原生能力。

目标 `normalize_anthropic_cache_control` 原在无 system 和既有 tool 标记时直接选 `tools.last_mut()`，可能给尾部 deferred tool 自动注入 `cache_control`。固定反例在旧实现先红；现只在自动注入时逆序寻找非 deferred 工具，兼容顶层 `defer_loading` 和目标 beta 判定已识别的 `custom.defer_loading`。全部 deferred 时不注入 tool 标记；最近 user message 的滚动标记继续生效，显式缓存标记、Native CLI 最小修改路径、请求身份和最终 CCH 顺序不变。此项不开放原生 tool-search，不改 Account/Provider/Share 绑定、CL-N7/CL-N8、静态目录或 receipt schema。离线反例不能代替五份真实 operation receipt、thread/beta 与原生工具的厂商接受性；当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-12 Codex 工具续链的精确调用身份

只读核对 `Codex/AGENTS.md` 推荐且干净的 `codex2api@30656549b7f5806af1db99ab2212cabe87cb8328` 新增 `responses_ws_context.go::responsesWSContextItemKey` 与既有 `response_cache.go::responseContextPairType`：WebSocket 历史合并以原始 `call_id` 及工具调用/结果类型配对，不以相同文本合并用户轮次。另一推荐 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 保留 Codex 整数工具 schema 等已审背景。本项只采纳不透明工具身份和类型配对的差分线索，不复制参考的完整 WS 会话快照、连接保留或跨账号恢复策略。

目标 `forwarder.rs` 的 previous-response 共享 helper 原将 `call_id` 两端空白裁掉后写入缓存展开项与判重键，使 `" call-a "` 和 `"call-a"` 碰撞；另只凭 ID 判定当前请求的调用/结果是否配对，不区分 `function_call` 与 `custom_tool_call_output`。两个反例在旧实现上先红。现仅以裁剪值检查全空，保留有效 ID 原文，按精确调用类型及 ID 判定结果是否需要旧上下文；缓存 key 的 response ID 既有不透明规则不变。HTTP 与 WS 续链共用这些 helper，作用域、容量、TTL、credential fence、未配对时的 409 和成功终态后写入策略保持。不修改 Codex OAuth 出站协议、Account/Provider/Share 绑定、Images 或 WS prewarm 开关和 receipt schema。离线形状不证明厂商会发含空白 ID；Astra、三个 Image 2.5 及 WS prewarm 五份真实 receipt 和当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-12 Cursor 会话键的请求内存计费

只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/services/cursorSessionManager.ts`：它以 conversation ID 保存同一 h2 流，默认 100 个会话和五分钟空闲 TTL；未定义本 Server 的多租户请求内存计费。推荐仓 22 项未提交 Codex 工作树内容排除，`CUR-N2=reviewed_no_wire_delta` 保持，不推断新的 Cursor wire。

目标 `try_build_plan` 会从非 Responses 请求提取客户端 `previous_response_id`，无匹配续流时 `resolve_session_key` 可把它原样作为 conversation ID。注册表和会话对象各保留一份键；旧 `session_retained_bytes` 没有计入两份 scope/conversation 字符串。4096 字节与短 ID 的同内容会话内存差分反例在旧实现先红。现请求内存估算计入两份键字符串容量和注册表键固定开销，预算不足按既有本地内存拒绝路径处理。入口路由、ID 原样保留、scope、续流、出站 protobuf、凭据/Share 绑定和 receipt schema 均不变。离线长 ID 不证明厂商会接受此形状；OAuth/API Key 两份真实 receipt 与当前未提交工作树 committed-target observation 仍缺。

2026-10-12 续审：同一已提交 OmniRoute session manager 仅作为二级会话索引的生命周期参考，22 项未提交 Codex 内容继续排除；它不定义目标 Server 的 request-memory 预算，`CUR-N2=reviewed_no_wire_delta` 不变。目标的 `response_sessions`、`tool_call_sessions` 各复制 scoped ID 和候选会话键，而原 `session_retained_bytes` 只计主键、工具 pending map。4 KiB response ID 绑定后预算不增长的固定反例在旧实现先红。现每次绑定前保守预留二级索引键、候选 Vec/哈希槽的容量；不足则在索引写入前本地拒绝，关闭/过期时随会话 reservation 释放。重复绑定可能保守多计，不改变精确 ID 匹配、歧义拒绝、OAuth/API Key scope、AgentService protobuf 或 receipt schema。离线容量反例不能证明厂商接受性；双 rail 真实 receipt 与未提交工作树的 committed-target observation 仍缺。

## 2026-10-12 Grok multipart 头部分隔符按最早空行选取

只读复核 `Grok/AGENTS.md` 推荐的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交状态；后者六项未提交 Codex 文件排除。`sub2api/backend/internal/service/grok_media.go` 使用标准 `multipart.NewReader` 逐个读取 part。该参考只提供 multipart 分帧的差分线索，不证明目标 direct-XAI 图片编辑的真实账号接受性，也不授权迁移其账号池或 media 策略。

目标 `split_part_headers` 同时支持 CRLFCRLF 和 LFLF，却总在整段 part 中优先寻找任意 CRLFCRLF。LF-only 头部后的 PNG 签名样本若含 CRLFCRLF，旧实现会把图片字节误当头部，实际图片编辑入口固定反例先红并报 400 签名不匹配。现比较两种空行位置，按最早出现的分隔符截断头部；图片字节原样交给现有签名/MIME/大小校验。完整 boundary 行、图片末尾换行、最多三图、未经支持字段拒绝、出站 JSON 和 Account/Provider/Share 单绑定均不变。离线 PNG 反例不证明 xAI 真实接受性；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号版本/header 接受性与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Grok 图片编辑 multipart MIME 参数解析

只读复核 `Grok/AGENTS.md` 推荐的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 Console 图片编辑路径，以及 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `grok_media.go::parseGrokMediaMultipartRequest`；后者六项未提交 Codex 文件全部排除。前者以结构化图片输入生成 JSON，后者用标准 MIME 解析媒体类型和 boundary。参考仅提供入站参数解析的差分信号，不证明目标 direct-XAI entitlement 或新 xAI wire。

目标 `multipart_image_edit_body` 原只按字面小写 `boundary=` 切分 Content-Type；合法 `Multipart/Form-Data; Boundary="..."` 在实际编辑转换入口返回 400，而 `multipart/form-dataevil; boundary=...` 反而可被当作 multipart 接受。固定正反例中首个合法输入在旧实现先红。现复用仓库已有 `multer::parse_boundary` 验证精确媒体类型并读取大小写不敏感、可引号包裹的 boundary，继续执行原有长度/字符、图片签名/MIME/大小/数量和 unsupported-field 校验。只修本地入站解析，不改变出站 JSON、Account/Provider/Share 单绑定、Responses/SSE/WS、remote compaction disabled、GR-N4 版本/header 或 receipt schema。离线 MIME 反例不能证明 xAI 真实接受性；inference/media/remote_compaction 三份真实 receipt、GR-N4 同账号证据与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Kiro 顶层工具组合式 schema 的无损边界

只读复核 `Kiro/AGENTS.md` 推荐且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：`src/kiro/endpoint/mod.rs` 将上游 `TOOL_SCHEMA_INVALID`（含顶层 `oneOf`/`allOf`/`anyOf`）列为客户端请求错误，不通过换号或重试解决。参考不提供本 Server 的 Claude Code 内建工具兼容展开规则，也不证明厂商接受任何顶层组合式 schema。

目标 `normalize_schema` 原将顶层组合式 schema 展开成首个显式 object 分支；两个可能对象分支会静默丢失第二个，纯 primitive 或畸形组合也可能被丢弃。多对象分支固定反例旧实现先红。现 `convert_tools` 在出站前对非单一显式 object 分支及其他不可无损展开的形状固定返回 400，保留恰好一个显式 object 分支的既有兼容路径。Claude Code 内建工具的 schema 字段改名现进入该分支，与工具输入改名一致；`Read.pages` 禁止规则也覆盖组合分支。此项只修本地请求转换，不改 Kiro/Amazon Q 出站 endpoint、认证、EventStream、Account/Provider/Share 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema。离线测试不能替代八份 auth-kind×region 真实 receipt；KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Kiro Claude Code 内建工具别名冲突边界

只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/anthropic/converter.rs::map_tool_input_to_kiro`：其 `take_first` 对 `Write` 的 `file_path/path` 等别名采用显式读取优先级。参考未定义本 Server 在同时收到两个键时应选哪一个，也不能证明双键输入获得上游认可；本项目按现有无歧义工具参数合同采取发网前拒绝，而不照搬优先级。

目标 Claude Code 内建工具桥原逐键写入映射结果，`Write` schema 同时含 `file_path` 与 `path` 时会静默丢失一个 property 并把 `required` 变成两个重复的 `path`；历史工具输入双键也可能因对象迭代顺序而选择不同值。固定双键反例旧实现先红。现仅在 Claude Code 内建工具模式下，对 schema 的 properties/required、嵌套 items 与组合分支，以及历史 `tool_use.input` 检查映射后的键冲突，并在出站前返回本地 400；单别名输入及 Raw 模式保持原有语义。此项不改 Kiro/Amazon Q 出站 endpoint、认证、EventStream、Account/Provider/Share 与八种 auth-kind×region 绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线双键反例不证明厂商实际发送；八份真实 receipt、KI-N5 独立计量证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Qoder 单 chunk 多 choice 的身份边界

`Qoder/AGENTS.md` 推荐 TokenRouter，但该仓当前 `AGENTS.md` 要求先使用当前会话未加载的 `project-doc` 技能；读取门禁后即停止，未读业务源码、未运行该仓命令或外仓源码差分审计。最新 TokenRouter 业务增量继续为 `unreviewed_skill_gate`。本项只依据目标 Server 的既有单 choice index 0 合同、已冻结 CLI oracle 与本地回环，不推导新的 COSY wire。

目标流式 `canonicalize_openai_chat_chunk` 逐个验证 `choices[].index=0`，却允许同一 chunk 有两个 index 0；非流 `QoderChatSseAggregator` 同样将两者拼成一条回答。固定双 choice 反例旧实现先红。现两层在产生下游业务前要求一个 chunk 至多一个 choice，空 choices 的 usage-only chunk 和多 chunk 续接保持。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini 的回环确认 502、只向原绑定账号发一次生成请求且不触发 refresh。只修本地响应身份完整性，不改 COSY 出站 endpoint/header/body/签名、站点/rail、Account/Provider/Share 绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线异常帧不证明厂商发送；三条 rail 的真实 receipt、登录/轮换、权威空目录、受控两段 401、最新 TokenRouter 合规补审及未提交工作树的 committed-target observation 均待完成。

## 2026-10-12 Qoder 显式语义文本类型的失败关闭

`Qoder/AGENTS.md` 推荐 TokenRouter；其 `AGENTS.md` 要求先调用当前会话不可用的 `project-doc` 技能。遵守门禁，未读取推荐仓业务源码，也未运行访问该仓源码的 `--check-sources`；最新业务增量仍为 `unreviewed_skill_gate`。本项只依据目标 Server 现有的 Chat/内容块文本合同、非流聚合器对显式非字符串文本的拒绝以及冻结的自包含 CLI oracle，不推断新的 COSY wire。

目标 decoder 原将 OpenAI Chat 的显式非字符串 reasoning 别名、`content_block_delta.data.text` 和 `type=text` 内容块的非字符串 `text` 当作缺失；错误形状可能与后续成功终态组合成空回答。固定反例在旧实现先红。现统一只允许这些可选文本字段缺失、null 或字符串，三个 reasoning 别名即使被更高优先级字段遮住也逐一校验；其他显式类型在转发前返回本地 502，并锁存 decoder 失败。合法字符串和 null 保持。Global OAuth、CN OAuth、Global PAT × Claude/Codex/Gemini × 流/非流的回环确认只向原绑定账号发送一次生成请求，不触发 refresh。仅收紧本地响应语义，不改 COSY 出站 endpoint/header/body/签名、Global/CN 站点与三 credential rail、Account/Provider/Share 单绑定、唯一终态+EOF、共享 attempt/401 预算或 receipt schema v2/revision 2。离线错误形状不证明厂商实际发送；三条 rail 真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 CodeBuddy 单 chunk 多 choice 的身份边界

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 WorkBuddy `sse.go::Aggregate`：其非流聚合器只取首个 choice，宽松流复制器则逐个处理；与此前已审提交相比 `internal/providers/workbuddy/` 无源码差异。两个未跟踪的 `proxy.html`、`proxy.md` 不作证据。参考不定义目标 Server 对多 choice 的严格失败合同，也不证明厂商实际发送该形状。

目标流式 sanitizer 与非流聚合器原分别逐个验证 `choices[].index=0`，同一 chunk 的两个合法 index 0 会被转发或拼成一条回答；固定反例旧实现先红。现两条路径在清洗或聚合业务前要求一个 chunk 至多一个 choice，空 choices 的独立 usage 尾帧及多 chunk 续接保持。Intl/CN × Claude/Codex/Gemini × stream/non-stream 十二组本地 HTTP 回环均在成功提交前返回 502，只向原绑定账号发一次生成请求且不触发 refresh。只收紧本地响应身份，不改 WorkBuddy endpoint/header/body、站点、Account/Provider/Share 单绑定、唯一 `[DONE]`+EOF、同账号恢复预算、receipt schema v2/revision 2 或 CB-N4 disabled 决策。离线帧不证明真实厂商接受性；两站真实 receipt、CN `deepseek-v4.1-flash` 目录/接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 CodeBuddy 空格文本分片的保真边界

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go::sanitizeDelta/dropEmptyString`：参考只删除值恰好为 `""` 的 dense delta 文本字段，保留 `" "` 这样的非空分片；未跟踪的 `proxy.html`、`proxy.md` 排除。参考不定义本 Server 对非字符串字段的失败合同，也不证明厂商真实发送独立空格帧。

目标 `sanitize_codebuddy_stream_chunk` 原用 `semantic_empty` 判断 `content`、`reasoning_content`、`reasoning` 和 `refusal`，该谓词会裁剪字符串；独立空格分片被丢弃，`"left" + " " + "right"` 会变成 `"leftright"`。固定 decoder→非流聚合回归在旧实现先红。现这些字段只移除 null 或真正的空字符串，保留空格原字节；显式非字符串文本在流式转发前按本地 502 拒绝，与非流聚合器的字符串边界一致。Intl/CN × Claude/Codex/Gemini × 流/非流十二组本地回环确认空格片段到达下游，且只向原绑定账号发送一次生成请求、不触发 refresh。其余稠密空字段、重复 role、dummy function_call、usage/finish 和唯一 `[DONE]`+EOF 不变。此项不改 WorkBuddy 出站 endpoint/header/body、Intl/CN 站点与 Account/Provider/Share 单绑定、同账号恢复预算、receipt schema v2/revision 2 或 CB-N4 disabled 决策。离线空格分片不能证明厂商实际发送；两站真实 receipt、CN `deepseek-v4.1-flash` 目录/接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Antigravity 工具名边界空白不得改变 wire 身份

只读核对 `Antigravity/AGENTS.md` 推荐的 `CLIProxyAPI@b601f5e7375343343922ccab5e50653bc3450c3b` 已提交 `internal/util/translator.go::SanitizedFunctionNameMap`：参考以原始名称作为映射键，再生成唯一 wire 名；`internal/util/util.go::SanitizeFunctionName` 把不合法字符变为下划线。另一推荐 `Antigravity-Manager` 的仓库约定已核对，但本项不借用其 UI/账号调度。参考的碰撞后缀策略并非目标 Server 既有的碰撞失败合同，不迁入；也不以参考源码推断新的 Google wire。

目标 `normalize_tool_names` 原先将声明名裁剪后用于匹配，却以未裁剪原值重写；`normalize_tool_call_ids` 也会裁剪调用/结果名称用于关联。同一请求中的 `" lookup "` 因此可能未经改名进入最终请求，或与声明 `lookup` 变成不同 wire 名。声明、调用与结果三种固定反例在旧实现先红。现最终 wire 规范化的首步统一拒绝有首尾空白的这些名称及 allowedFunctionNames，防止任何 ID 关联或名称重写先发生；内部空格的现有安全映射保留。实际 Gemini v1internal 请求清洗入口验证固定 400。此项只修本地出站工具身份，不改 Antigravity/AGY endpoint/header/body 其他字段、Account/Provider/Share 单绑定、AG-N6 requestType、AG-D3 replay/compaction、终态与 receipt schema v2/revision 2。离线异常名称不证明 Google 接受性；双 OAuth rail 真实 receipt、AG-N6/AG-D3 专项证据与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Claude 托管 tool_search 不得伪装成客户端函数

只读复核 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b`：其 `internal/translator/claude/openai/responses/claude_openai-responses_tool_search.go::isResponsesClientToolSearch` 只认 `execution=client`，并注明 hosted/server search 不能在 Claude 上运行；`internal/runtime/executor/claude_executor.go::applyNativeToolSearchFallback` 在原生能力不可用时去除客户端搜索并固定请求级决策。参考是执行位置和回退风险证据，不证明目标 Server 已具备原生搜索能力，也不授权直接打开该能力。

目标 Responses→Anthropic 兼容桥原把显式 `execution=server` 的 `tool_search` 声明改写成普通 `tool_search` 函数，固定反例旧实现先红。现该桥在构造工具上下文前拒绝显式非客户端执行的搜索声明，以及同类历史 call/output；默认缺省 execution 的既有兼容形状与显式 `client` 不变。Codex Surface 经 Claude API Key/OAuth Provider 的实际请求转换入口均本地返回 400。此项不启用原生 tool-search，不改 Claude OAuth endpoint/header/CCH、Account/Provider/Share 单绑定、其他 Provider 的 Responses→Chat 路径或 receipt schema。离线形状不证明真实 Anthropic 接受性；五份真实 Claude operation receipt、原生搜索厂商证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Codex 搜索结果的 previous-response 调用配对

只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `codex2api@30656549b7f5806af1db99ab2212cabe87cb8328` 与 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b`。前者 `proxy/response_cache.go::responseContextPairType` 按调用类型与原始 `call_id` 配对，包含 `tool_search_call_output`；其 `proxy/grok_namespace_tools.go` 和 `proxy/grok_preflight.go` 还识别 `tool_search_output`，但缓存配对表未纳入这个别名。后者近期的 Codex/Claude tool-search 能力宣告依赖原生 Claude 能力，不据此扩大目标 Server 的模型能力。参考不是此别名的完整修复证明；目标自己的 Responses 兼容转换明确产出与消费 `tool_search_output`，而 previous-response 缓存仅识别 `_call_output`，两层协议身份不一致。

目标带 `previous_response_id` 的 `tool_search_output` 原绕过 `requires_previous_response_tool_context`，缺历史调用时可能继续发往上游；固定反例旧实现先红。现统一将两种搜索结果映射到 `tool_search_call` 做配对，缓存可保留该结果，当前请求已有相同原始 `call_id` 与调用类型时不误报缺历史；已有缓存可注入调用，WS 缺失缓存仍在发网前返回稳定 409。此项不缓存完整对话、不改 Responses/Images/WS 出站 wire、Account/Provider/Share 固定绑定、WS prewarm disabled 决策或 receipt schema v1/revision 1。离线别名不证明真实 OpenAI 订阅上游会发送该形状；Astra、三个 Image 2.5 与 WS prewarm 五份真实 receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Antigravity Gemini 末尾纯 thought turn 边界

只读复核 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。前者已提交 `a2718314` 在 Gemini 目标移除末尾纯 thought 的 model turn，并记载仅追加空 user turn 仍可能遭上游 400；`15d2148b` 处理 model-role 工具结果内嵌媒体，但目标转换已将工具结果置于 user turn，未据此认定新缺口。参考是边界风险证据，不证明本 Server 的任意历史形状获得 Google 接受。

目标 Codex Responses 输入中的有效签名 reasoning item 可经 Responses→Anthropic→Gemini 转换成为末尾 `role=model`、仅含 thought part 的 turn；Antigravity 首次出站清洗与 replay 后规范化原均保留它。完整转换链的固定反例在旧实现先红。现仅在最终模型明确为 Gemini 时，从最终 wire 末尾连续移除这类 turn，至少保留一个 turn；包含可见正文、函数调用、媒体或未知 part 字段的 model turn、Claude 目标和无模型请求不删。首次封装与 replay envelope 共用同一幂等规范化。只修本地历史边界，不合成空 user turn，不改 endpoint/header、AG-N6 requestType、AG-D3 replay/compaction、Account/Provider/Share 绑定或 receipt schema v2/revision 2。离线反例不证明真实厂商必然发送 400；双 OAuth rail receipt、AG-N6/AG-D3 厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Claude 无效 cache_control 不得占用断点额度

只读核对 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 已提交 `be8c35f2`：其 `isValidClaudeCacheControl` 仅把 `type=ephemeral` 的对象视为有效断点；null、非对象和其他 type 不参与四断点计数、默认注入判定或 TTL 顺序归一化。参考不证明 Anthropic 对任意无效字段的接受性，也不要求本 Server 静默删除 Native CLI 原始字段。

目标 `collect_cache_paths` 原把任何存在的 `cache_control` 都计为断点，`normalize_cache_control_ttls` 还把 null 当成五分钟断点；有效断点可能被四项上限错误移除，后续有效 `ttl=1h` 被降级。默认滚动 message 断点也因末块存在 null 而不注入。固定 null、布尔及其他 type 的反例在旧实现先红。现只把 `type=ephemeral` 的对象用于缓存策略；无效标记不计费、不改变 TTL 顺序，也不阻止默认 system/tool/message 断点覆盖注入。除注入覆盖处外不改写客户端原始无效字段，保留 Native CLI 的上游判定；不改 Claude OAuth endpoint/header/CCH、Account/Provider/Share 单绑定、CL-N7/CL-N8 或 receipt schema v1/revision 1。离线边界反例不证明真实厂商接受性；五份真实 operation receipt、thread/beta 接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-12 Codex OAuth Chat 历史工具 call_id 长度边界

只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@091ff27f31285ec4fd1dd5823b44f7f51fb8f48b` 与 `codex2api@3065654914f5806af1db99ab2212cabe87cb8328`。前者已提交 `b0debb35` 在 Chat→Codex Responses 转换时，将超过 64 字节的 `call_id` 截前缀并附 SHA-256 短摘要，遇现有 ID 冲突再确定性重试，同时将原始工具结果配到缩短后的调用；后者本次未找到同等 64 字节归一化证据，其 WS 续链按原始 `call_id` 配对的实现不证明 Chat 转换边界。参考提交及 issue 说明是上游长度风险证据，但离线测试不是厂商接受性证明。

目标 Codex OAuth 的 Chat→Responses 转换原样带出任意长度 `tool_calls[].id` 与 `tool_call_id`，固定完整适配器反例旧实现先红。现只在 Codex OAuth 的 Chat 兼容入口复制请求后，对有显式过长 ID 的 assistant 工具调用、旧式 function_call 及工具结果采用同一请求内映射；预留所有原有短 ID，按 UTF-8 边界截取前缀并附 64-bit SHA-256 摘要，冲突时确定性换摘要，最终 `call_id` 不超过 64 字节。普通短 ID、非 OAuth Provider、原生 Responses 输入和客户端原始请求不改。仅修本地出站兼容转换，不改 Responses/Images/WS endpoint/header、Account/Provider/Share 绑定、previous-response 配对门禁、CX-N13/CX-D3、WS prewarm disabled 或 receipt schema v1/revision 1。Astra、三个 Image 2.5 与 WS prewarm 五份真实 receipt、厂商接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Cursor 显式图片输入不得静默丢失

只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/utils/cursorImages.ts`：显式图片 URL 或数据不合法时抛出经清洗的错误，而不是继续执行纯文本请求；远程图像另有 SSRF、大小和格式栅栏。其未提交 Codex 工作树内容不作证据。参考不证明 Cursor 厂商对具体错误文案或更大图压缩的接受性。

目标 Cursor 规划器原在 OpenAI Chat、Responses、Gemini 的部分非法图片 URL 与 Anthropic/Gemini 的无效内联 base64 上仅返回空图片列表，使含正常文字的多模态请求被静默降为纯文本。四协议固定反例先复现该行为。现在在拆解前检查显式图片字段是否能被本地提取，缺字段、不支持的 URL scheme、损坏的内联数据和会被忽略的图片位置固定、脱敏地失败；合法 HTTP(S)、data URI 与内联图片仍保留给既有图像加载器执行内容签名、大小、远程地址和重定向验证。本项不改变每图 1 MiB 上限，不实现大图压缩，也不改变 Cursor AgentService protobuf、OAuth/API Key rail、Account/Provider/Share 绑定或 receipt schema v2/revision 2。离线反例不是两条真实 rail 的厂商接受性证据；真实 receipt 和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Grok 图片编辑 multipart 字段名的标准解析边界

只读核对 `Grok/AGENTS.md` 优先的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 图片路径，以及次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 已提交 `backend/internal/service/grok_media.go`：后者经标准 `multipart.NewReader` 和 `FormName()` 获取字段名，不按每个分号直接切开 `Content-Disposition`。`sub2api` 六项未提交 Codex 工作树改动全部排除。参考只证明本地 MIME 解析风险，不定义 direct-XAI 的新出站 wire 或真实 entitlement。

目标 Grok 图片编辑原用 `split(';')` 取 `name`，且不核对 disposition 类型；`attachment; name="image"` 和 `form-data; name="image;ignored"` 可被误认成真实 `image` 字段，重复 `name` 也未被当作歧义，合法 quoted-pair 转义的字段名却会漏识别。实际入口固定反例在旧实现先红。现在只承认 `form-data`，在引号外分段、解码引号内转义，并对重复目标 `name`、未闭合引号及格式错误的参数不作有效图片字段认定；图片字节、boundary 分帧、MIME/签名/大小/数量栅栏和既有出站 JSON 不变。不改 Account/Provider/Share 单绑定、HTTP/WS、GR-N4 版本/header、remote compaction disabled 或 receipt schema v1/revision 1。离线 multipart 反例不能证明 xAI 厂商接受性；三份真实 operation receipt、GR-N4 同账号证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Kiro 图片预处理不得掩盖不可承载位置

只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：其 `src/image_resize.rs` 明确要求图片处理不能丢图，`src/kiro/model/requests/conversation.rs` 的 Kiro wire 将图片放在 user message 的 `images` 列表。参考并不证明 system 或 assistant 图片能被 Kiro/Amazon Q 原生接收，也不要求目标 Server 猜测替代承载位置。

目标 `prepare_anthropic_images_with_memory` 原会处理 system、assistant 与对象型内容里的显式图片，但后续 `system_text`、`parse_assistant_content` 和 `parse_user_content` 只将 user 数组直系 image 及其中 tool_result 数组的直系 image 送入 Kiro `images`。实际 `prepare_kiro_request` 固定反例在旧实现先红：system 图片被处理后，出站 user `images=[]`，请求仍成功。现预处理前核对图片位置：不能进入 wire 的显式图片固定、脱敏地 400 失败；受支持的 user 与 tool_result 图片继续原路径。图片数据处理、大小/格式/去重限制、Kiro/Amazon Q 出站 endpoint/header、EventStream、Account/Provider/Share 与八种 auth-kind×region 固定绑定、KI-N5 usage shadow、Compact/shared cache disabled 和 receipt schema v1/revision 1 均不改变。离线形状不证明厂商真实接受；八份私有真实 receipt、KI-N5 专项证据与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Qoder 工具调用别名身份一致性

`Qoder/AGENTS.md` 推荐 TokenRouter，但其 `AGENTS.md` 要求先使用本会话未加载的 `project-doc` 技能。读取门禁后没有读取该仓业务源码，也没有运行访问其业务对象的 `--check-sources`。本项仅依据目标 Server 的工具 ID/名称归一化、已冻结的自包含 Qoder CLI oracle 及本地回环，不推断新 COSY wire；最新 TokenRouter 业务增量仍为 `unreviewed_skill_gate`。

目标 `canonical_qoder_tool_call` 把 `id`、`tool_call_id`、`call_id` 及 `function.name`、`name`、`tool_name` 当作同一字段的兼容别名，却在并存时只取优先项。固定反例中 `id=call-a` 与 `call_id=call-b` 被悄悄规范化成 `call-a`，旧实现先红。现只要显式别名非空且互相矛盾，或显式值不是字符串/null，就在业务转发前以固定本地 502 失败；相同别名、单一别名和不透明 ID 的原始空白继续保留。此项只收紧本地工具身份，不改 COSY endpoint/header/body、站点或凭据 rail、Account/Provider/Share 单绑定、共享 attempt/401 预算、唯一终态+EOF 或 receipt schema v2/revision 2。离线冲突别名不能证明厂商真实发送；Global OAuth、Global PAT、CN OAuth 真实 receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 CodeBuddy 显式工具分片形状不得被空值清理掩盖

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go`：其非流聚合器把 `tool_calls` 解析为数组，流式清洗器会去掉空工具占位。参考并不定义目标 Server 的严格流式失败边界，也不证明 Intl/CN 厂商实际发送畸形字段；两个未跟踪的 `proxy.html`、`proxy.md` 不作证据。

目标 `CodeBuddyChatSseAggregator` 已要求 `delta.tool_calls` 是数组，但 `sanitize_codebuddy_stream_chunk` 原只在它恰好是数组时验证；显式空字符串甚至会被当作空占位删除，后续 `finish_reason=tool_calls` 可被转发成看似正常的完成帧。实际 SSE 解码入口固定反例在旧实现先红。现显式字段只接受 `null` 或数组；其他类型在业务转发前以固定本地 502 失败，`null`、空数组和合法工具分片保持原有兼容。只修本地入站响应形状，不改 CodeBuddy 出站请求、双站绑定、Account/Provider/Share 单绑定、同账号 401 预算、唯一 `[DONE]`+EOF 或 receipt schema v2/revision 2。离线畸形形状不是厂商真实行为证据；Intl/CN 两份真实 receipt、CN 模型能力门槛与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Antigravity 显式工具 ID 的最终出站类型门槛

只读核对 `Antigravity/AGENTS.md` 推荐的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 Antigravity executor/replay fixture，以及 `Antigravity-Manager@601f5e7375343343922ccab5e50653bc3450c3b` 已提交 `context_manager.rs`：两者以字符串 ID 追踪 `functionCall`/`functionResponse`，无 ID 的结果可保留或按本地上下文处理。参考不证明显式数字/null ID 能被厂商接受，也不定义本 Server 的多租户身份失败策略；没有读取或迁入未提交参考内容。

目标最终 wire 规范化允许缺失工具 ID 并在唯一配对时生成，但原把显式非字符串 `functionCall.id` 视为缺失，造出 `tool_call_0`；显式非字符串 `functionResponse.id` 也可被忽略并按唯一名称重新配对。实际 `normalize_wire_request` 的数字调用 ID 反例旧实现先红。现只在 Antigravity/AGY 最终出站边界核对两种驼峰/下划线工具字段的显式 ID：存在时必须是字符串，类型错误以固定、脱敏的本地 400 拒绝；完全缺失 ID 的既有生成/配对仍可用。此项不改请求 endpoint、模型、requestType、签名 replay、工具名称归一、Account/Provider/Share 双 rail 绑定、同账号恢复或 receipt schema v2。离线错误类型不是厂商真实发送证据；Antigravity/AGY 两份真实 receipt、AG-N6、compaction 与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Claude redacted thinking 内容块的独立完整性边界

只读核对 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：`internal/translator/claude/openai/responses/claude_openai-responses_response_test.go::TestConvertClaudeResponseToOpenAIResponses_RedactedThinkingBecomesMarkedReasoningItem` 用 `content_block_start` 的非空字符串 `data` 携带 redacted thinking，并直接以 `content_block_stop` 结束，不把它当普通 thinking delta；`internal/runtime/executor/claude_executor_stream.go::validateClaudeStreamingResponse` 只做较宽的流完成检查，不定义本 Server 对畸形 opaque 块的成功门槛。目标自身 Kiro SSE 发射器也使用相同的独立块形状。参考不是 Anthropic 厂商真实验收。

目标 `AnthropicSseInspector` 原把 `redacted_thinking` 与普通 thinking 归为同类，既不检查 start 的 `data`，又允许随后接 `thinking_delta`；非流式 `validate_message` 也只检查 `content` 为数组。实际 SSE 和 JSON 反例在旧实现先红。现已知 redacted 块要求非空字符串 `data`，且 SSE redacted 块不接受任何 delta；正常不透明数据原样留在响应，未来未知内容块的兼容路径不变。错误文案固定且不回显 `data`。仅修 Claude 入站成功语义，不改 OAuth 出站 Messages/count_tokens、CL-N7 续线程、CL-N8 `pause_turn`、Account/Provider/Share 单绑定、静态模型目录或 receipt schema v1/revision 1。离线畸形块不能证明真实 Anthropic 会发送；五份真实 operation receipt、thread/beta 厂商接受性及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Codex WebSocket 迟到中断的连接内终态栅栏

只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。CLIProxyAPI 已提交 `6331b38b` 增加 `response.interrupt` 原始控制帧转发，`d318bcc3` 将同一连接已终止响应的迟到中断幂等处理，防止污染下一轮；`codex_websockets_late_interrupt_test.go` 还验证活动中断与未知空闲中断的不同结果。codex2api 当前提交未作为该控制帧协议形状依据；两份参考均不证明目标项目的私有 ChatGPT OAuth entitlement。

目标 Codex 桥接原会将上一轮 `response.completed` 后到达的 `response.interrupt` 发到保留的上游连接。真实本地 WebSocket 回环的旧实现先红，上游观察到 `response.create` 后多出 `response.interrupt`。现按下游桥接和物理上游连接记录最多 32 个已终止响应 ID（每个最多 512 字节）；匹配的迟到中断幂等忽略，未知空闲 ID 返回固定、脱敏的本地错误帧，不发上游。活动中断要求非空字符串 `response_id`，保留原始 Text/Binary bytes、`mode` 及扩展字段；大上传写入期间的有界控制帧队列也可接收它。连接变化不会复用终态 ID；单测覆盖类型、字节保真、容量和连接隔离，回环覆盖连续两轮及活动中断。

此项仅保障活动 WebSocket 和已经由本桥接观测终态的迟到中断，不把未知 ID 解释为取消当前响应，也不改变 Account/Provider/Share 单绑定、WS→HTTP 既有恢复预算或 receipt schema v1/revision 1。HTTP fallback 的等待器仍拒绝所有 WebSocket 数据帧，尚无参考项目那种本地 HTTP 中断；未观测到终态或终态 ID 超出有界记录时不能声称幂等。离线回环不是厂商真实接受性证明；Astra、三个 Image 2.5 exact-model 与 WS prewarm 的五份真实 receipt、当前工作树 committed-target observation 仍缺。

## 2026-10-13 Cursor 大图输入与 AgentService wire 图片准备

只读核对 `Cursor/AGENTS.md` 推荐的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 已提交 `open-sse/utils/cursorImages.ts`：Cursor vision 入口最多 12 图，单图读取上限 16 MiB，最长边 8192、像素数 2500 万；解码后 JPEG 逐级质量/尺寸处理，默认约 100 KiB、`detail=high/original` 约 256 KiB，最终 wire 不超过 1 MiB。其未提交 Codex 工作树文件全部排除。参考只为边界与风险证据，不证明本 Server 两条 Cursor rail 的真实接受性，也不要求引入其 Sharp 或 Next.js 路由。

目标 `src/proxy/cursor/image.rs` 原把 1 MiB wire 限额直接用作 data URI、远程下载和内联 base64 的入口限额，有效约 3 MiB PNG 在任何图片准备前即返回 400；实际 loader 固定反例旧实现先红。现将入口与 wire 上限分开，先按 MIME/签名、SSRF、跳转、尺寸与像素边界读取并校验，再在阻塞工作线程解码 JPEG/PNG/WebP/GIF，按质量阶梯和 2000→256 像素边长收缩成 JPEG；Chat/Responses 的高细节标记保留到准备器。最终结果仍不得超过 1 MiB，工作集从同一请求内存预算预留，畸形/超限图片本地拒绝。已有四协议显式图片提取门禁、Account/Provider/Share 固定绑定、AgentService protobuf 字段、会话续接和 receipt schema v2/revision 2 不变。离线图片不等于厂商可用性；OAuth/API Key 两条真实 receipt 和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Grok 孤立 tool_choice 不得进入最终上游请求

只读核对 `Grok/AGENTS.md` 推荐且干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f`：已提交 `df4dde39` 在 Console 请求规范化后执行工具/选择配对门槛，`6db9f67f` 补全缺失、`null`、空数组三种回归；非空有效工具的选择保持。次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件排除，本项不借其定义 Grok Build 工具协议。参考只支持上游工具配对风险，不证明真实 Grok OAuth 权益。

目标 `sanitize_tools` 原在 `tools` 不存在时立即返回，把孤立 `tool_choice` 留在最终 Grok 请求；`tools=null` 则在更早的工具兼容转换被当作畸形数组 422 拒绝，空数组已有清理。HTTP Responses 固定缺失反例旧实现先红。现只有非空工具数组可保留匹配的选择；缺失、`null` 或空数组时两字段一并省略，非 null 的畸形工具类型仍 422，合法 `web_search` + `auto` 正例保留。HTTP 与 WS `response.create` 共用这一准备边界。仅修最终工具声明配对，不改模型、reasoning、endpoint/header、Account/Provider/Share 单绑定、同账号恢复、GR-N1 质量观察、媒体或 receipt schema v1/revision 1。离线请求形状不等于厂商实际接受性；Grok inference、media、remote_compaction 三份真实 receipt 与当前工作树 committed-target observation 仍缺。

## 2026-10-13 Kiro reasoning 事件的显式字段类型边界

只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`：`src/kiro/model/events/reasoning.rs::ReasoningContentEvent` 将 `text`、`signature`、`redactedContent` 反序列化为可选字符串，显式数字、对象或数组不能成为这些字段的有效内容。参考的直接事件结构不定义目标 Server 既有的 `reasoningContentEvent` 包裹及 `content`、`reasoningContent`、`redacted_content` 兼容别名；本项只对目标已识别字段采用同一类型边界，不将参考实现当作厂商 wire 权威。

目标 `wire::parse_event` 原对 reasoning 只解析为任意 JSON；后续 `reasoning_text`、`reasoning_signature`、`reasoning_redacted_content` 通过 `as_str()` 读取，显式给出对象型加密思考等错误类型会被静默丢弃，随后 `endEvent` 仍可产生正常成功。EventStream 解码入口固定反例在旧实现先红。现顶层或包裹的 reasoning 只接受对象、字符串或已有的 `null` 空占位；对象中已识别的文本、签名与 opaque 字段只接受字符串或 `null`，未知字段仍保持前向兼容。非法形状返回固定脱敏的本地 `KIRO_EVENT_STREAM_INVALID`，非流与流式共用，流式不输出成功 `message_stop`。有效 redacted opaque 数据不解析、不记录、不改写。只修 Kiro/Amazon Q 入站响应完整性，不改出站请求、auth-kind×region Account/Provider/Share 单绑定、EventStream CRC/容量/EOF、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线畸形帧不证明厂商真实发送；八份真实 receipt、KI-N5 独立计量证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Qoder 空心跳流不得形成成功回答

`Qoder/AGENTS.md` 首选 TokenRouter，但其 `AGENTS.md` 要求当前会话先调用未提供的 `project-doc` 技能；本轮没有读取 TokenRouter 业务文件，也没有运行其外部对象 `--check-sources`。转而只读核对次选且业务工作树干净的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `9d5eb0b`：`internal/providers/qoder/direct.go::ensureMeaningfulUpstream` 在首次下游字节前拒绝只有心跳的 200 SSE，非流聚合也拒绝空 body；其流式输出在已有业务片段但缺结束理由时可合成 terminal。未跟踪的 `proxy.html`、`proxy.md` 排除。参考的账号池 failover 与任意截断流补终态不能照搬到 Server 的单账号、唯一终态合同；官方 CLI oracle 的 `valid_inner_chunk + authoritative_finish_reason + upstream_eof + exactly_one_downstream_done` 仍是本地第一来源。

目标 `QoderSseDecoder` 原允许 `[DONE]` 单独设置终态，空 wrapper/空 delta 后的 EOF 也会输出成功 `[DONE]`；`QoderChatSseAggregator` 可把仅有 `[DONE]` 的非流响应补成空 `stop`。固定空流反例旧实现先红。现只转发规范化后有文本、reasoning、工具、usage 或明确结束理由的 chunk；完成时既要唯一终态与真实 EOF，也要观测有效 chunk 和明确结束理由。已存在有意义内容的 Anthropic 兼容 `message_stop` 可在本地生成 `stop` 或 `tool_calls` 终态；原生 Chat 有内容却只以裸 `[DONE]` 结束时仍失败关闭，不以猜测结束理由洗白。直接进入非流聚合器的空 `[DONE]` 也拒绝。三 rail × Claude/Codex/Gemini × 流/非流的本地 HTTP 回环固定在响应提交前失败，无跨 Provider/账号切换或 refresh。此项不改变 COSY 出站 endpoint、签名、payload、catalog、Account/Provider/Share 单绑定、现有同账号 401 预算或 receipt schema v2/revision 2；离线空流不证明真实 Qoder 会这样发送。三份真实 rail receipt、登录/轮换、权威空目录、受控两段 401、TokenRouter 合规补审及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Codex 无工具请求的并行调用标志

按 `Codex/AGENTS.md` 只读核对干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233` 已提交状态。CLIProxyAPI 的 `75fa6265` 在 Codex executor 的普通、流式与 Compact 路径中，若最终 `tools` 缺失或为空则删除 `parallel_tool_calls`，非空工具保持原值；其测试同时固定缺失、空数组和有效工具。codex2api 当前近期重点是原生 WS 工具续链/共享上下文，不将其连接池策略迁入本 Server，也不把它作为该字段的独立 wire 证明。参考不是 ChatGPT OAuth 真实接受性证据。

目标普通 Codex OAuth 请求在无工具时会补 `tools: []` 和 `parallel_tool_calls: false`，客户端显式 `true` 则仍原样出站；Lite 还会无条件写 `false`，Compact 和 WebSocket 对无工具显式标志也缺少最终配对。普通/Compact/Lite 实际准备入口的固定反例旧实现先红，普通路径发出了 `tools: []` 与 `parallel_tool_calls: true`。现共用最终工具数组判断：缺失、空数组或清洗后为空时删除并行标志；有非空工具时保留普通/Compact/WS 既有值，Lite 维持强制 `false`。HTTP、Compact、Lite 和原生 WS 准备测试覆盖该边界；WS→HTTP fallback 复用同一普通/Lite 准备路径。仅修 Codex OAuth 出站字段配对，不改 endpoint/header、模型、工具 schema、Images、Account/Provider/Share 单绑定、同账号恢复、WS prewarm disabled 或 receipt schema v1/revision 1。离线请求形状不证明厂商真实接受；五份真实 operation receipt 与未提交工作树的 committed-target observation 仍缺。

## 2026-10-13 CodeBuddy 非空旧式 function_call 不得静默丢失

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go`：清洗器仅删除 dummy `function_call`，非流聚合器只累计 `tool_calls`。两个未跟踪的 `proxy.html`、`proxy.md` 均排除。参考是风险交叉证据，不证明两站实际发送非空旧式字段。

目标原会保留非空 `delta.function_call`，但非流聚合器忽略它；`finish_reason=function_call` 加 `[DONE]` 可构成丢失工具调用的成功响应。实际 decoder 固定反例旧实现先红。现两条入口均固定、脱敏地 502 拒绝非 dummy 旧式字段，空占位继续清理，正常 `tool_calls` 保持。不改 WorkBuddy 出站请求、Intl/CN 与单账号绑定、同账号 401 预算、唯一 `[DONE]`+EOF 或 receipt schema v2/revision 2。两站真实 receipt、CN 精确模型能力证据及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Antigravity 双拼写工具字段的单一身份边界

按 `Antigravity/AGENTS.md` 只读核对干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。前者 Gemini 请求转换的 `geminiFunctionNameFields` 同时识别 `functionCall`/`function_call` 与 `functionResponse`/`function_response`；后者 pipeline estimator 以 `.get(primary).or_else(alias)` 选择工具语义。这些参考证明两种拼写都处于兼容面，但不定义同一 part 同时携带两种拼写时的厂商解释，也不证明真实 Google 接受该畸形组合。

目标最终 wire 规范化原同样优先处理驼峰字段，若同一 part 还带下划线字段，则 ID 绑定、结果配对与 `$ref` 结果清理可能只作用于优先项，而另一项原样出站。冲突调用 ID 的固定反例在旧实现先红。现最终出站边界禁止同一 part 同时出现 `functionCall`/`function_call` 或 `functionResponse`/`function_response`，即使原值相同也拒绝，因为 ID 归一化后仍可能分叉；单一驼峰或下划线字段仍兼容。错误文案固定且不回显工具内容；Antigravity/AGY 双 OAuth rail 共用边界。不改 endpoint、模型/requestType、单账号绑定、签名 replay、同账号恢复或 receipt schema v2/revision 2。两条真实 receipt、AG-N6/AG-D3 专项厂商证据及未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Claude 非流式已知内容块的完整载荷门槛

只读核对 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：其已提交 `claude_openai_native_response_test.go::TestConvertClaudeResponseToOpenAINonStream_NativeMessagesJSON` 使用带字符串文本及 `id`、`name`、对象型 `input` 的完整 `tool_use` 块作为原生非流响应；转换器随后把这些字段变成客户端工具调用。参考转换器本身偏宽松，不定义本 Server 对畸形上游块的成功门槛，也不证明厂商实际会发畸形数据。

目标 `validate_message` 原仅对已知 `redacted_thinking` 校验块载荷；`text.text` 为数字、`thinking.thinking` 为对象、`tool_use` 缺 ID 或 input 非对象，都可在 `type=message`、usage 完整时记为成功。非流 JSON 入口固定数字文本反例旧实现先红。现仅对已知文本、思考及两种工具使用块核对完整字段类型，空文本仍可用，未来未知类型保留；SSE 分片继续按自己的 start/delta/stop 合同校验。Claude OAuth 本地 HTTP 回环确认缺 ID 的完整工具块在提交成功前固定、脱敏地 502 拒绝。不改出站 Messages/count_tokens、beta、CCH、CL-N7/CL-N8、Account/Provider/Share 单绑定或 receipt schema v1/revision 1。五项真实 operation receipt 与当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Cursor 内建工具别名必须精确匹配

按 `Cursor/AGENTS.md` 核对推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 `open-sse/executors/cursor/builtinToolBridge.ts`，工作树文件对象与 HEAD 的 `537783b52f7062157a8977595b52611a97e3f9a4` 相同；22 项未提交 Codex 文件全部排除。参考 `namedTools` 只按声明名称的大小写无关精确相等选择内建工具候选，随后另核对声明 schema；不能把名称包含 `delete` 推断为 Delete 工具。此项只提供本地桥接选择的差分证据，不证明新的 Cursor protobuf 或真实 OAuth/API Key 接受性，`CUR-N2=reviewed_no_wire_delta` 不变。

目标 `resolve_mcp_tool_by_aliases` 原把 `lower.contains(alias)` 当成兼容匹配；Cursor 原生 Delete 可被客户端名为 `undelete` 的声明接收，Read 可误选 `breadboard`，搜索和 shell 也有类似风险。生产桥接 helper 的四组固定反例首个在旧实现先红。现只接受显式别名的大小写无关完整名称，或目标项目 Responses 点号命名空间的完整末段；`functions.grep` 保留，`functions.undelete` 不会成为 Delete。`read_file`、`web_fetch`、`ripgrep` 等既有固定别名保留，未命中的内建事件仍走既有类型化上游拒绝，不会向客户端伪造工具调用。声明后的参数还继续由同一个 `resolve_tool_call` 和原始 JSON Schema 验证。只修本地内建/MCP 工具路由语义，不改 AgentService protobuf、OAuth/API Key rail、Account/Provider/Share 单绑定、会话续接、模型目录、图片处理、同账号恢复或 receipt schema v2/revision 2。两条真实 rail receipt、厂商接受性和当前未提交工作树 committed-target observation 仍缺。
## 2026-10-13 Codex OAuth Responses include 的受支持字段边界

只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。前者已提交 `2044a01f` 的 `setCodexRequiredInclude` 与表格测试仅保留必需 `reasoning.encrypted_content`，并在客户端数组显式请求时保留 `web_search_call.action.sources`；未知、重复和非字符串项不会进入最终 Codex 请求。后者原生 WS 续链使用独立连接亲和，不定义目标 HTTP/WS 的 `include` 白名单。参考不能证明私有订阅账号对任意模型的网页来源权益。

目标普通 HTTP Responses 原在客户端 `include` 数组中追加必需项，却把 `file_search_call.results`、数字及重复来源原样发往上游；WS `response.create` 也未做这道归一化。固定 HTTP 反例在旧实现先红。现两条出站路径共用同一白名单：总是包含加密推理项，只有数组显式请求才包含一次网页搜索来源，其他项丢弃。缺失、非数组、只有未知值均回到必需项；不会因为存在搜索工具而隐式扩展。独立 Images 与 compact 请求、出站 endpoint/header、Account/Provider/Share 单绑定、同账号恢复和 receipt schema v1/revision 1 不变。离线请求形状不证明真实 OpenAI 接受性；五份真实 operation receipt 和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Codex WS→HTTP 回退中断帧的非破坏性拒绝

只读核对 `Codex/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：已提交 `6331b38b` 的 `readResponsesWebsocketInput` 对 `response.interrupt` 走独立控制通道；无活动上游 WS 时可以交给本地 HTTP 取消器，错误只回控制帧而不把它排进普通请求。`codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233` 近期的原生 WS 续链/会话保留不定义本 Server 的回退取消。官方 [Responses HTTP 流式指南](https://developers.openai.com/api/docs/guides/streaming-responses.md) 只证明 SSE 为有类型事件传输，不证明私有 OAuth WS 的中断语义。

目标已有活动 WS 中断转发与迟到终态 ID 处理，但 WS→HTTP 回退的发送、退避和 SSE 等待把任意 Text/Binary 当作并发请求。已输出 `response.output_text.delta` 后发未知 ID 中断的本地回环反例，旧实现直接重置连接；现对顶层中断返回固定、脱敏的本地错误帧，并继续原 HTTP 轮次，非中断并发请求仍拒绝，Grok 路径不套用 Codex 控制帧规则。错误不回显 `response_id` 或其他扩展字段。这里只修无效/不支持控制帧不得意外终止生成；匹配活动响应 ID 的跨传输本地 HTTP 取消仍未实现，不能宣称已支持。出站 body、Account/Provider/Share 单绑定、同账号 retry/refresh 预算、终态/usage 成功判定与 receipt schema v1/revision 1 均未改；离线反例不证明厂商真实发送，五份 operation 真实 receipt 和未提交工作树的 committed-target observation 仍缺。

## 2026-10-13 Cursor 推荐仓 HEAD 与未合并分支修复的证据隔离

按 `Cursor/AGENTS.md` 只读复核推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192`。参考工作树有 22 项未提交改动，均非本次 Cursor 已提交证据；Cursor 执行器、图片准备器、会话管理与内建工具桥接文件在该工作树和 HEAD 间无差分。`git log --all` 能看到 `84af297126`（空工具调用）、`64586a2c91`（部分输出超时）、`ed774c3b7f`（导入 JWT 到期时间）和 `ed23963737`（Grok 4.7 变体），但逐个 ancestry 检查均确认它们不在当前 HEAD；不能将这些其他分支的行为记作推荐提交态已支持或目标现有 wire 的权威。

当前 HEAD 的 `open-sse/utils/cursorImages.ts` 有 12 图、16 MiB 入口与 1 MiB 最终线图像、尺寸/像素/解码/远程限制，目标 `src/proxy/cursor/image.rs` 已有对应本地准备；HEAD 的 `isCursorEmptyTurn` 只按文字、思考和工具数量判断，目标非流与流式在 `TurnEnded` 时已拒绝完全无内容的回答，并拒绝缺 AgentService Exec/MCP 元数据的 marker-only 工具；两者不能据此推导新的厂商终态。目标继续对绝对期限内未完成的多协议流发显式错误，不套用非 HEAD 的仅 Chat `finish_reason=length` 处理。`CUR-N2=reviewed_no_wire_delta` 保持；不新增模型变体、账号切换、会话重放或超时成功终态。Cursor OAuth/API Key 两条真实 receipt、厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Cursor 显式工具选择不得降级为自动选择

按 `Cursor/AGENTS.md` 只读核对推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的已提交 `open-sse/executors/cursor/builtinToolBridge.ts::selectCursorBridgeTools`；该文件工作树与 HEAD 同为 `537783b52f7062157a8977595b52611a97e3f9a4`。参考对 OpenAI 指定函数的无效对象返回无候选工具，而不是扩展为整个工具列表。它不定义目标项目四种客户端入口的统一错误文案，也不证明 Cursor 上游会收到某种畸形 selector；`CUR-N2=reviewed_no_wire_delta` 不变。

目标 `extract_tool_choice` 原在 OpenAI Chat/Responses 显式 `type=function` 却缺失或错误类型名称时返回 `Auto`，Anthropic `type=tool` 同样可降级，Gemini `ANY` 的多名称或非字符串允许列表可降成不受限 `Required`。四入口固定反例的 Chat 首例在旧实现先红。现请求合同和计划构造共用前置校验，拒绝无法精确表达的显式选择；OpenAI 对象式 `none`/`required` 被保留为对应约束，四入口合法单名称正例保持。错误固定且不回显客户端名称或内容，发生在账号/AgentService 出站前。只修本地工具语义，不改 protobuf、OAuth/API Key rail、Account/Provider/Share 单绑定、会话续接、同账号恢复或 receipt schema v2/revision 2。离线选择反例不是厂商接受性证据；两条真实 rail receipt 与未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Grok 拒答文本的脱敏质量观测

只读核对 `Grok/AGENTS.md` 首选、工作树干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `backend/internal/application/gateway/quality_retry_scan.go::observeQualityResponsesOutputItem`：它在 Responses message content 中将 `refusal` 作为可见字符串计数，且不以 usage 的 output−reasoning 差值补造可见文本。次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件继续排除。参考只提供本地质量观测的差分信号；其质量 hold、重试、账号池策略不迁入本 Server，也不证明 xAI 真实拒答形状。

目标 `GrokQualityObserver` 原仅统计 `output_text`/`text`，完整 `refusal` 块会被记作 `empty`；固定 JSON 反例旧实现先红。现只在旁路计入非空拒答文本，流式 `response.refusal.delta` 优先于 done/完成快照，避免重复计数；既有固定 outcome 与长度桶不变，正文不落盘、不进标签。不改 HTTP/SSE/WS 出站、下游原字节、主成功终态、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v1/revision 1。离线反例不能证明厂商真实接受；inference、media、remote_compaction 三份真实 receipt、GR-N4 同账号版本/header 证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Grok 空文本占位不得遮住完成快照

按 `Grok/AGENTS.md` 核对首选、工作树干净的 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 已提交 `quality_retry_scan.go`（对象 `db45a609d4846911c595ee0343258abff04fd63a`）：`response.output_text.delta` 只有非空字符串才计为可见内容，输出项另作聚合观察；没有把空 delta 视为已证明存在可见文本。次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 的六项未提交 Codex 文件排除，其账号池/重试策略不迁入目标。参考只提供质量观察信号，不定义私有 xAI 流的实际占位频率或新 wire。

目标 `GrokQualityObserver` 原只要看到 `response.output_text.delta` 就设置 `saw_text_delta`，空白 `response.output_text.done` 也设置 `saw_text_done`；后续完成快照虽含真实文本，仍被阻止计数并误记 `empty`。固定“空 delta＋空 done＋非空完成快照”反例旧实现先红。现仅在有非空可见文本时锁定 delta/done 来源；有效 delta 仍优先于 done/快照，只有空占位时允许后续真实文本补证。指标仍只保留固定 outcome/长度桶，不保存正文；HTTP JSON、SSE、WebSocket 的主终态与响应字节、Account/Provider/Share 单绑定、同账号恢复和 receipt schema v1/revision 1 均不变。离线反例不能证明 xAI 实际发送该顺序；inference、media、remote_compaction 真实 receipt、GR-N4 同账号证据及未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Kiro 零输入 tokenUsage 快照的权威优先级

只读核对 `Kiro/AGENTS.md` 首选且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903`（tree `5d22f040f0e4b0aa775e61547540b3c040293cd8`）：已提交 `src/kiro/model/events/metadata.rs::TokenUsage`（对象 `7bc9eb6376b466018d692193d9d43d06e4f2ea0d`）把缺失输入分项默认零，`src/anthropic/stream.rs::resolved_usage/process_kiro_event` 将整份 `tokenUsage` 保留为优先于 context 回退的最终快照，重复出现取最后一份。参考只证明精确快照的本地解析优先级，不证明八种 auth-kind×region 的厂商计量接受性，也不授权将 KI-N5 的 context-output shadow 切成生产 authority。

目标 `KiroUsageAccumulator` 原只在 `tokenUsage` 输入合计大于零时更新精确输入，导致较早的 metrics/context 值在零输入快照后仍被计费；后续 metrics 也可能覆盖已收到的快照。固定 context＋metrics＋仅含 outputTokens 的快照反例旧实现先报输入 123。现对象型快照总是覆盖输入（含零），之后的 metrics 回退不再覆盖它，重复快照仍替换前一份；后一份缺失的输出或 cache 分项按零处理，不继承前值。只修入站用量优先级，不改 Kiro/Amazon Q 请求、EventStream 转发、Account/Provider/Share 与八路 auth-kind×region 绑定、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线反例不能替代八份真实 receipt、KI-N5 独立计量证据或当前未提交工作树 committed-target observation。

## 2026-10-13 Kiro 字面 tool_use XML 泄漏的下游过滤

只读核对 `Kiro/AGENTS.md` 推荐且工作树干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/assistant.rs::strip_tool_use_xml_leaks` 与 `src/anthropic/stream.rs::ToolUseXmlLeakFilter`：当 Kiro 把字面 `<tool_use ...>…</tool_use>` 混入助手文本时，参考在非流和跨 chunk 流式路径删除它，结构化 `toolUseEvent` 另行处理；近似 `<tool_user>` 不删。参考只提供文本泄漏的差分信号，不定义本 Server 的工具身份、账号调度或新 EventStream wire。

目标原有 `ToolLeakFilter` 只识别 `<function_calls><invoke>`，实际 EventStream 同时含字面 `<tool_use>` 和原生 `toolUseEvent` 时，前者完整出现在客户端文本中；固定非流反例旧实现先红。现先用有界保留的跨 chunk 过滤器移除真 `<tool_use>` 块，再走既有 invoke 救援；开闭标签分片、截断块及近似标签回归覆盖，过滤器保留容量计入请求生命周期预算。正常结构化工具调用、其 ID/名称/input 与去重规则不变。不改上游请求、EventStream CRC/终态/EOF、八个 auth-kind×region 的 Account/Provider/Share 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线混合帧不证明厂商真实发送；八份真实 receipt、KI-N5 专项证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Qoder 错误别名的 null 回退边界

`Qoder/AGENTS.md` 首选 TokenRouter，但其 `AGENTS.md` 要求先使用当前会话不可调用的 `project-doc` 技能；遵守门禁，未读取其业务源码或运行 `--check-sources`，最新增量仍为 `unreviewed_skill_gate`。只读核对次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/qoder/direct.go::upstreamStatusError` 与近期 `upstream_empty` 处理：该项目主要按 HTTP 状态分类，并未定义目标 Server 对已识别 `112`/`115` 错误码别名的覆盖规则；其账号池 failover、截断流补终态和限额 cooldown 策略不迁入目标。本项依据目标已有顶层、`error`、`data` 错误别名合同修复本地选择逻辑，不据参考推断新 COSY wire。

目标 `QoderUpstreamError::from_status_body` 原以 `or_else` 选择第一个存在的字段；外层 `code:null` 会遮住已支持的 `error.code=115`，使限额错误误报普通 400，外层 `null` message/重置时间也会遮住内层有效值。固定错误体反例旧实现先红，实际 SSE wrapper 也覆盖同一分类入口。现只跳过显式 `null`，仍以首个非 null 外层值为准；未知或冲突的非 null 错误码不会被内层值洗成 `115`。错误码仍仅公开 `112`/`115`，message 继续脱敏。不改出站请求、SSE 成功终态、Provider/Share/Account 与三 credential rail 单绑定、一次同账号 401 恢复或 receipt schema v2/revision 2。离线错误体不证明厂商实际发送；三条真实 rail receipt、TokenRouter 合规补审和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Qoder 思考文本别名的 null 占位边界

`Qoder/AGENTS.md` 首选 TokenRouter，但其仓库要求当前会话先调用未加载的 `project-doc` 技能；本轮未读取或运行其业务内容，也没有运行受限的外部对象 `--check-sources`。只读核对次选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a`（tree `508056fe3dcf34c39190b7920445f168b65a9526`）已提交 `internal/providers/qoder/direct.go`（对象 `2bd98c6bb5d70cb9554ab524d68eb1a64338529c`）：`frameFromBody` 及流输出使用 `reasoning_content` 作为规范字段。参考不定义目标另行支持的 `reasoning_text`/`reasoning` 别名优先级，也不证明厂商发送多别名并存的形状。

目标已审兼容合同允许三个思考文本别名；原规范化及非流聚合均用首个“存在”的字段，故 `reasoning_content:null` 会遮住 `reasoning_text:"actual thought"`。合法业务分片＋单个结束理由＋EOF 的完整 decoder→aggregator 反例旧实现先成功却输出 `reasoning_content=null`。现先逐项验证显式类型，再按优先级取首个非 `null` 值；直接 Chat 聚合入口也一致，显式空字符串不被低优先级覆盖。只修本地响应文本保真，不改 COSY 出站、SSE wrapper/唯一终态+EOF、Account/Provider/Share 与三 credential rail 固定绑定、同账号恢复或 receipt schema v2/revision 2。离线反例不能替代 Global OAuth、Global PAT、CN OAuth 的真实 receipt、TokenRouter 合规补审或未提交工作树 committed-target observation。

## 2026-10-13 CodeBuddy 默认 reasoning 档位的空值回退

只读核对 `CodeBuddy/AGENTS.md` 推荐且业务工作树干净的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a`（tree `508056fe3dcf34c39190b7920445f168b65a9526`）已提交 `internal/providers/workbuddy/catalog.go::catalogModel`（对象 `0a99f3e7dc132ff8bfab0444a4d651e3ce5b3068`）：先规范化 `reasoning.defaultEffort`，为空则回退到 `reasoning.effort`。两个未跟踪文档 `proxy.html`、`proxy.md` 全部排除。参考仅提供目录字段优先级，不证明本 Server 的 reviewed 模型白名单外能力、任意档位或两站真实接受性。

目标 `codebuddy_model_capability` 原先对原始 JSON 用 `or_else` 挑选第一个存在的值，`defaultEffort:null` 或空白字符串会挡住有效 `effort:"high"`；随后客户端明确请求 `reasoning:{}` 时，最终 WorkBuddy payload 缺少 `reasoning_effort`。目录解析→请求构造固定反例旧实现先红。现只在首字段缺失、`null`、空白或超出既有 32 字节上限时尝试旧字段；显式非字符串首字段仍保持原有不回退行为，合法非空首字段优先。请求显式 effort、live `supportedEfforts` 校验、模型白名单和未请求 reasoning 时不自动启用的边界不变。两站真实 receipt、CN `deepseek-v4.1-flash` 目录/接受性以及未提交工作树 committed-target observation 仍缺。

## 2026-10-13 CodeBuddy 拒答分片在非流聚合中的保真

只读核对 `CodeBuddy/AGENTS.md` 推荐的 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a` 已提交 `internal/providers/workbuddy/sse.go`：其流式 `sanitizeDelta` 删除空 `refusal`、保留非空值，但 `Aggregate` 只累计 `content` 与 `reasoning_content`，未保留拒答；两个未跟踪文档排除。目标流式 decoder 已保留非空 `delta.refusal`，非流聚合器却忽略它；两段拒答加合法终态的固定 decoder→aggregator 反例旧实现返回 `message.refusal=null`，修复后按序返回原文本。新增字段计入请求生命周期内存预算，既有 `finish_reason`、唯一 `[DONE]`+EOF、出站 WorkBuddy wire、Intl/CN 站点及 Account/Provider/Share 单绑定、同账号 401 恢复和 receipt schema v2/revision 2 不变。此离线反例不证明厂商真实发送拒答分片；双站真实 receipt、CN 精确模型能力及当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Antigravity 工具配置双拼写冲突的最终 wire 门槛

只读复核 `Antigravity/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`：前者 Gemini request 转换遍历 `functionDeclarations`/`function_declarations`，并处理两种 `toolConfig` 路径上的允许函数名；后者 pipeline 从两种 `toolConfig` 拼写中优先取驼峰配置。两者均不能定义同一对象双拼写同时出现时的 Google 语义，亦不提供本 Server 的多租户冲突策略。

目标最终 wire 原对工具 part 的双拼写已有拒绝，却在工具声明、顶层工具配置、内层函数调用配置及允许函数名列表只取优先项；另一套可未经校验留在上游请求。`functionDeclarations=lookup` 同时附 `function_declarations=delete_all` 的固定反例在旧实现先红。现四层别名并存时均在出站前以固定、不含客户端值的本地错误拒绝，即便两边当前内容相同也拒绝后续规范化可能造成的分叉；只用任一种拼写的请求及 envelope/raw 双入口继续可用。此项仅收紧本地工具选择身份，不改 endpoint/header、单拼写合法 payload、AG-N6 requestType、AG-D3 replay/compaction、Account/Provider/Share 单绑定、同账号恢复或 receipt schema v2/revision 2。离线构造不证明 Google 实际发送双拼写；两条真实 OAuth rail receipt、AG-N6/AG-D3 专项厂商证据和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Claude 原生前置混合 system turn 的保真

只读核对 `Claude/AGENTS.md` 推荐且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：已提交 `2d8ec73b` 在 system 插入位置跳过仅含 `output_config`、空 content 的努力档位 directive，避免其阻断后续 user run；参考不定义目标 Server 的前置文本提升或混合块拆分，也不证明任意未来块厂商接受性。目标 `hoist_leading_text_system_messages` 原已保留独立的空 directive，但若同一前置 system 消息同时有文本、`output_config` 和非文本块，只要找到一个文本块便删除整条原消息。固定混合消息反例在旧实现先红，剩余消息直接从 user 开始，努力档位及非文本块均丢失。现只把非空文本块移入顶层 `system`；消息级其他字段和未提升内容块保留在原位置，必要时形成空 content 的 directive，并在最终 CCH 前签名。纯文本消息、独立 directive、user 首条和中途 system 的既有行为不变。不改变 Claude OAuth endpoint/header/beta、Account/Provider/Share 单绑定、CL-N7/CL-N8、模型目录或 receipt schema v1/revision 1。离线请求改写不能替代五项真实 operation receipt 与续线程/beta 厂商接受性；当前未提交工作树也不产生 committed-target observation。

## 2026-10-13 Antigravity replay 签名别名的空值回退

按 `Antigravity/AGENTS.md` 只读核对干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b`。CLIProxyAPI 已提交 `antigravity_reasoning_replay.go::antigravityNativePartThoughtSignature` 按 `thoughtSignature`、`thought_signature`、`extra_content.google.thought_signature` 逐项查找非空文本；Manager 的 `thinking_store.rs` 也识别前两种拼写，但其部分读取为首字段存在即优先，不能作为空值回退的独立证明。两者都不是厂商发送多别名并存形状的证据。

目标本地 `native_signature` 原先挑第一个存在的 JSON 字段，再校验类型和长度。首字段为 `null` 时，非流及流式终态响应中的有效低优先级签名无法进入 replay 缓存；续轮请求中同样可遮住已存在的不同原生签名，使缓存签名被写入 `thoughtSignature`，与已有别名形成冲突。两条实际 replay 路径的固定反例在旧实现先红。现逐项执行原有文本、trim、16 字节至 1 MiB 的有效性判定，取首个有效签名；已有有效高优先级签名仍优先，异值原生签名仍拒绝缓存覆盖。不改变 Antigravity/AGY 出站 endpoint/header、模型/requestType、Account/Provider/Share 单绑定、同账号恢复、AG-N6、AG-D3/compaction 或 receipt schema v2/revision 2。离线反例不证明 Google 实际发送该组合；两条真实 OAuth rail receipt、AG-N6/AG-D3 专项接受性与当前未提交工作树的 committed-target observation 仍缺。

## 2026-10-13 Claude 嵌套工具引用的别名一致性

只读核对 `Claude/AGENTS.md` 首选且工作树干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `internal/runtime/executor/claude_executor_request.go`：其工具名映射在历史 `tool_result.content[]` 与 `tool_search_tool_result.content.tool_references[]` 中，只改写 `type=tool_reference` 的 `tool_name`；回程 JSON/SSE 的相同协议引用也可按反向映射恢复。参考是兼容路径证据，不证明 Anthropic 对任意未来嵌套形状的接受性，亦不授权迁入参考的原生 tool-search 能力开关。

目标 Claude OAuth 最终请求原已映射声明、forced choice、顶层 `tool_use` 和 `tool_reference`，却漏掉上述两种嵌套引用；非法自定义/MCP 名称会被声明映射成 `cc_tool_*`，历史嵌套引用仍留原名。实际 `apply_forward_contract` 的固定反例旧实现先红；JSON 回程恢复也只处理 `tool_use` 名称，顶层及嵌套 `tool_reference` 保留 wire 别名的固定反例先红。现仅在已识别的协议引用位置重写请求，回程恢复 `tool_reference.tool_name` 并复用既有 SSE 事件 patcher；未声明名称、普通文本同名字段及工具 `input` 不动。映射继续限定于请求内且在最终 CCH 前完成，不改 Claude OAuth endpoint/header/beta、Account/Provider/Share 单绑定、CL-N7/CL-N8、原生 tool-search disabled 决策或 receipt schema v1/revision 1。五份真实 operation receipt、thread/beta/工具引用厂商接受性和当前未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Codex WS→HTTP 回退的匹配 ID 本地取消

按 `Codex/AGENTS.md` 只读核对干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：已提交 `6331b38b` 在无活动上游 WS 时将 `response.interrupt` 交给本地 HTTP 取消器，其 `completeResponsesWebsocketLocalInterrupt` 发出 `response.incomplete`（`reason=interrupted`）并取消 HTTP 上下文。参考的本地实现并不校验中断 ID 与已观测响应 ID 是否相等，不能直接作为本 Server 的租户/轮次隔离规则。`codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233` 的近期 WS 续链改动不定义该跨传输取消；参考均不证明私有 OAuth 厂商真实支持中断后续链。

目标原对 HTTP 回退期间所有合法中断一律返回本地错误，即使 SSE 已发出 `response.created` 且 ID 完全匹配。实际回环中，客户端收到 `response.output_text.delta` 后发送匹配 ID 的中断，旧实现返回 `error`；修复后只在本轮已观测到非空、有界的 `response.created.response.id` 且精确匹配时，停止读取当前 HTTP 流，转发尚未提交的生命周期帧，发送本地 `response.incomplete`，用既有输出项收集器保留已完成项并清理缓存写上下文。同一 WS 连接可启动下一轮；未知/无效 ID、未观测 ID 及非中断并发请求仍按原规则拒绝。取消记为 499/`client_cancelled`，不将主动取消记作上游成功或故障。下游已收到的未完成增量不会伪造成已完成输出项；本地合成终态不含未观测的上游最终 usage，也不保证用该 ID 对私有上游续链。出站 body、Account/Provider/Share 单绑定、同账号恢复预算和 receipt schema v1/revision 1 不变。五份真实 operation receipt 与未提交工作树 committed-target observation 仍缺。

## 2026-10-13 Cursor Connect 内嵌额度错误的分类

按 `Cursor/AGENTS.md` 只读核对推荐 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 的提交态 `open-sse/executors/cursor.ts::processFrame/tryParseJsonError` 和 `open-sse/executors/cursor/cursorErrors.ts::classifyCursorErrorKind`；相关 Cursor 文件与工作树相同，未提交 Codex 文件排除。参考将 Connect 帧中的 JSON `error` 当成上游错误而非 protobuf，并把 `resource_exhausted` 按额度/请求过大线索分开；不定义本 Server 的多租户账号冷却、Provider outcome 或真实 Cursor 接受性。`CUR-N2=reviewed_no_wire_delta` 保持。

目标此前将 `grpc-status: 8`、Connect 终态 `error.code=resource_exhausted` 统一报 502，Provider 结果成为 `NetworkFailure`；JSON 错误帧则进入 protobuf 解码/空回答路径。gRPC 状态与 Provider 分类的固定反例旧实现先红。现在只在有界（8 KiB）且顶层确有 `error` 的 JSON 帧、结构化 Connect 终态及 gRPC 状态 8 上识别这些拒绝：额度耗尽为 429/`RateLimited`，仅有明确大小线索且无额度线索时为 400；未知 JSON 错误固定 502。响应文案不回显上游详情，429 只冷却当前绑定账号，重置提示沿用既有有界解析；不是通用 5xx/transport failover，也不改变非零其他 gRPC 状态、OAuth/API Key 凭据、Share/Provider 单绑定、h2/protobuf 出站或 receipt schema v2/revision 2。离线错误帧不证明厂商实际发送频率；两条真实 rail receipt、未提交工作树 committed-target observation 仍缺。

## 2026-10-14 Claude OAuth Responses 缺省输出上限

按 `Claude/AGENTS.md` 只读核对干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671`：已提交 `a881c036` 的 `defaultClaudeResponsesMaxTokensForModel` 在 Responses 省略 `max_output_tokens` 时使用已注册非 Fable 模型的 `MaxCompletionTokens`，Fable 保留 64,000 的保守默认值；显式上限走另一路。参考 Sonnet 4.6 目录值为 64,000，而本 Server 的静态 Claude Code 目录为 128,000，因此本 Server 只复用缺省策略，不照搬参考具体模型数值。参考行为不证明订阅账号能接受目录上限。

目标共享 Responses→Anthropic 转换原一律填 `max_tokens=8192`；Claude OAuth 后续仅在字段缺失时填 128,000，因此省略上限的已注册模型始终停在 8,192。实际 adapter 转换反例在旧实现先红，Opus 5.5 得到 8,192 而非 128,000。现在只在 Codex Responses→Claude OAuth 且所有兼容上限字段缺失或为 `null` 时，于共享转换前注入模型目录默认值；Fable 限 64,000，未知模型仍为 8,192。显式值及非法非空值仍由共享转换原逻辑处理；Claude API Key、Chat、其他 Provider 和原生 Claude 请求不变。注入发生在 reasoning budget 计算前。不改出站 endpoint/header、Account/Provider/Share 单绑定、原生 tool-search disabled 决策、CL-N7/CL-N8 或 receipt schema v1/revision 1。五项真实 operation receipt、模型上限厂商接受性、当前未提交工作树 committed-target observation 和最终整体 review 仍缺；不做 UI 自动化或部署测试。

## 2026-10-14 Codex 客户端整数工具字段扩展

按 `Codex/AGENTS.md` 只读核对干净的 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 与 `codex2api@84a969d8eede5d1d9bb4ceb25d97012f3e34d233`。前者已提交 `d7914afd` 在 Codex 官方客户端向非 Codex executor 转发时，按内建工具名与精确 schema 路径把误声明的 `number` 收窄为 `integer`，新增 notes、history、memories、web、collaboration 等工具的嵌套路径和别名；后者近期 WS 续链/客户端身份增量不定义该 schema 修正。参考白名单是客户端兼容证据，不是任意同名用户工具均应改写的证明。

目标已有 CX-D3 的第一批顶层字段修复，但 `codex_integer_tool_fields` 只覆盖少数工具，`normalize_codex_client_integer_tools` 只查 `parameters.properties` 的直接属性；`notes__read_file.start_line.anyOf[0]` 等新路径仍留 `number`，而任何 namespace 下叫 `wait_agent` 的工具都会被误修。精确嵌套路径与错误 namespace 的固定反例在旧实现先红。现在只扩充静态工具/字段路径白名单，按实际 namespace 组成工具名，支持白名单中的嵌套对象与数组下标，保持 64 层/既有 schema 预算门禁。Codex 官方 UA、非 Codex 目标和 Codex 入口三重门禁不变；普通 number、未知工具/namespace、用户参数值、Codex→Codex 与非官方客户端均不改。此项不改变 OAuth 出站 endpoint/header、Account/Provider/Share 单绑定、同账号恢复、CX-N13、Images/WS prewarm disabled 决策或 receipt schema v1/revision 1。参考与离线 schema 不证明具体非 Codex 上游的厂商接受性；五份真实 operation receipt、当前未提交工作树 committed-target observation 和最终整体 review 仍缺。不做 UI 自动化或部署测试。

## 2026-10-14 Qoder 当前 HEAD 只读补审的结构化追溯

`assets/contract/qoder-reference-delta.json` 在不修改 2026-09-28 `QODER-SKILL-GATE-2026-09-28` 历史记录的前提下，追加 `QODER-CURRENT-HEAD-READ-ONLY-2026-10-14`。它固定 TokenRouter 的 `7faf9469bc6957716923b5b4a98665c0fb9715e0`、`6d676f1d100e11f2f068949f5779e960c862079c`、`c25dbf0cfa5a806460ed0fbc5c43a55797148537` 三个提交及各自 tree，并记录当前工作树仅有未暂存删除的根 `AGENTS.md` 被排除。旧门禁只代表当时状态；该文件当前不存在，Qoder 可达路径的已提交对象已按增强计划 §18.302～18.320 只读补审。`--check-sources` 可选核对冻结文件和三棵 Git tree，默认审计不读取外仓；tree 身份本身不能替代逐路径语义审查。

补审发现的 Responses 续接、流式部分用量和嵌套错误分类按目标单账号边界处理；参考的多 Key 结算、账号切换及断连后后台推理不迁入。Global OAuth、Global PAT、CN OAuth 三条真实 rail 仍为 `live_pending`，真实账号验收不在当前实施 goal 内。这次结构化追溯不宣称厂商接受性，也不把未提交目标工作树写成提交态 observation。

## 2026-10-14 Claude 续线程嵌套工具引用的别名恢复

整体 review 对照普通 Claude OAuth 请求已覆盖的嵌套协议引用位置与 `claude_thread_alias::restore_request_names`，发现后者只恢复顶层 `tool_use`、`tool_reference` 和工具增删。续线程省略工具声明、沿用缓存别名时，`tool_result.content[]` 或 `tool_search_tool_result.content.tool_references[]` 内的历史非法工具名仍可能原样出站，与已缓存的 `cc_tool_*` 声明不一致。固定反例在旧实现失败：两个嵌套引用保留 `mcp__bad/name`，预期为 `cc_tool_123`。修复仅复用普通请求的 `rewrite_claude_nested_tool_references`，并让普通文本同名字段与工具 `input` 保持不透明；不改变 endpoint/header、线程状态作用域、Account/Provider/Share 单绑定或真实验收状态。定向反例修复后通过，完整 Rust library 3617 passed / 1 ignored、API 合同 124/124、其余两个集成目标各 1/1。该离线反例不证明 Anthropic 真实接受续线程，相关真实 receipt 仍在本 goal 外。

## 2026-10-14 Claude 续线程部分工具声明的旧状态门槛

整体 review 继续核对同一别名恢复链：旧 `thread_mode_from_value` 只要看到非空 `tools` 就设 `needs_state=false`。若本轮仅声明新工具，但历史 `tool_use`、嵌套 `tool_reference` 或 `tool_choice` 仍使用上轮非法名称，缺少旧缓存时这些名称无法映射；实际 HTTP 回环旧实现将请求发送上游并返回 200，而本应在发网前拒绝。原有普通请求的别名改写还覆盖 `tool_addition`/`tool_removal` 引用及 `server_tool_use`，旧状态门槛也须与这些协议位置一致。

现只在名称未由本轮声明、且确实需要非法名称别名或 Claude Code 内建工具大小写归一时要求前一 message ID 的精确作用域状态；缺失则 404、零上游请求。有完整声明时照常将历史名称与出站声明映射为同一 wire 名，合法且无需改名的历史名称、普通文本和工具参数不额外阻断。声明数在构建临时集合前限制为既有 128 项上限，超限仍为 400。此项不改变旧缓存的 TTL/容量/身份键、endpoint/header、Account/Provider/Share 固定绑定或真实账号状态；离线回环不证明 Anthropic 对续线程真实接受，真实 receipt 不在当前 goal 内。

## 2026-10-14 Codex HTTP 回退中上游错误先于匹配中断

整体 review 的本地回环固定了一个跨传输竞争顺序：Codex WS 转 HTTP SSE 后，下游先收到带响应 ID 的 `response.created` 和上游 `error`，随后发送相同 ID 的 `response.interrupt`。旧实现把它合成为 `response.incomplete`，并以 499/`client_cancelled` 结算，掩盖已到达的上游失败；该反例在旧实现先红。现在匹配中断先补发尚未交付的生命周期帧；若本轮已有锁存的上游错误，则发 `response.failed` 并按既有 Provider 失败分支结算，不再走取消路径。没有先到达的上游错误时，原有本地取消、`response.incomplete` 和 499 语义不变；未知或不匹配 ID 仍不能取消当前请求。此项只修目标桥接的终态与账务优先级，不改 Codex OAuth 出站 wire、账号绑定、恢复预算或 receipt schema。离线竞态回环不能证明私有上游对中断或续接的接受性；五项真实 operation 仍为 `live_pending`，其真实账号验收不在当前 goal 内。

## 2026-10-14 Cursor marker-only 工具拒绝的名称脱敏

`Cursor/AGENTS.md` 首选的 `OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192` 仍是已冻结提交态；22 项未提交 Codex 工作树内容排除，未发现新的 Cursor wire 差分。整体 review 转而检查目标本地拒绝路径：Composer 文本中的 marker-only 工具调用没有 AgentService Exec/MCP 元数据，不能安全续接。目标原在 `cursor_delta_events` 的 Text/TurnEnded 分支和 `flush_cursor_marker_filter` 把上游生成的工具名拼进 `ProxyError.message`；流式错误帧会向客户端回显该名称。含 `private-marker-name` 的固定反例在旧实现先红。现在三处共用固定 501 错误，不回显工具名；四种客户端入口的错误帧保持各自原有格式，调用仍失败关闭，不从 marker 猜测工具身份。此项不改 OAuth/API Key、Account/Provider/Share 单绑定、h2/protobuf、session/续接、终态/用量或 receipt schema v2/revision 2。离线伪名称不证明厂商实际发送该内容；两条真实 rail 独立 `live_pending`，采集不在本 goal 内。

## 2026-10-14 Grok SSE ping 的非对象与显式类型边界

`Grok/AGENTS.md` 首选的干净 `grok2api@7c889a960e2638341b4dae9a5c81af0e0f38c87f` 与次选 `sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2` 均未出现新的已提交 Grok wire；次选六项未提交 Codex 工作树改动继续排除。参考的 Responses SSE 兼容层处理 BOM 与私有 Build 控制帧，不定义目标 direct-XAI 计费 ping 的任意 JSON 形状。本轮整体 review 检查目标 `grok_sse_event_is_ping`：它原对 JSON 数组、标量及显式 `type=null`/数字都得到“没有字符串 type”，于是把整帧改写成 `: ping`，可能静默丢失不明上游内容。数组输入的固定反例在旧代码先红。

现在只有无数据 ping，或非空 JSON **对象**且 `type` 缺失/精确为字符串 `ping`、没有错误或终态语义的帧可规范化；其他形状原样交给主 Responses 语义门禁。正常对象型计费 ping、评论与有效 ping 保持，`id`/`retry`/重复事件/未知 SSE 字段的原有透传门槛不变，分片输入同样保真。此项只修 Grok OAuth Responses SSE 入站过滤，不改出站 endpoint/header/body、HTTP JSON/原生 WS、Account/Provider/Share 单绑定、搜索/质量成功门槛、同账号恢复、GR-N4 版本/header、remote compaction disabled 或 receipt schema v1/revision 1。离线畸形 ping 不证明 xAI 实际发送；三份 operation 仍 `live_pending`，真实账号验收不在本 goal 内。

## 2026-10-14 Kiro 歧义工具短名称错误的脱敏

`Kiro/AGENTS.md` 首选且干净的 `kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903` 已提交 `src/kiro/model/events/tool_use.rs::ToolUseEvent` 将 `name` 作为上游事件字段；这不授权目标把它当成可信客户端诊断，也不定义目标的请求内命名空间仲裁。目标 `KiroToolNameRegistry::resolve` 原在多个当前声明共享同一子名称、上游只返回该短名称时，把短名称拼进 `KIRO_EVENT_STREAM_INVALID` 错误。带 `private-child-name` 的真实 EventStream 固定反例在旧非流转换先红；该错误也会经流式桥接暴露。

现在只把歧义拒绝文字改为固定说明，不输出上游短名称；精确声明名与唯一子名称仍按原规则映射，歧义仍失败关闭。非流转换和流式桥接共同验证错误分类及不回显，流式没有成功 `message_stop`。此项不改 Kiro/Amazon Q 出站、EventStream 帧/终态/EOF、工具 ID/JSON 聚合、Account/Provider/Share 与八种 auth-kind×region 单绑定、KI-N5 usage shadow、Compact/shared cache disabled 或 receipt schema v1/revision 1。离线伪名称不证明厂商实际发送；八份真实 receipt 仍为 `live_pending`，真实账号验收不在本 goal 内。

## 2026-10-14 Qoder usage 零主计数的别名回退

`Qoder/AGENTS.md` 首选 TokenRouter；其当前工作树已删除根 `AGENTS.md`，旧 `project-doc` 门禁不再限制本次只读研究。已提交 `TokenRouter@c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 `backend/internal/upstream/qoder/client.go::ParseSSELine/qoderSSEMessageDeltaEvents` 明确规定：Chat 的 `prompt_tokens`/`completion_tokens` 为零时，分别取 `input_tokens`/`output_tokens` 别名；非零主计数优先。这是 Qoder 入站用量兼容证据，不是出站 COSY wire 或真实账号验收结果。

目标 `qoder_token_value` 原只取第一个存在的字段，故 `prompt_tokens:0,input_tokens:5` 会记成零。Chat choice 与 `message_delta` 的 decoder→非流聚合固定反例在旧实现先红；现所有存在字段仍逐项验证，只有主计数为零且备用计数非零时回退，主计数非零和全零保持。显式无效别名仍失败，`total_tokens` 的既有一致性/溢出门槛不变；流式规范化复用同一入口。此项不改 COSY 请求、三 rail/Account/Provider/Share 固定绑定、唯一终态与 EOF、同账号恢复或 receipt schema v2/revision 2。离线反例不证明厂商实际发送此组合，Global OAuth、Global PAT、CN OAuth 继续 `live_pending`；真实账号采集在本 goal 外。

## 2026-10-14 CodeBuddy 已识别业务拒绝码不触发认证恢复

`CodeBuddy/AGENTS.md` 首选 `cli2api@6cf7e4ebb8eab4b8bf43902aa3cd963124c4027a`，其已提交 `internal/providers/workbuddy/client.go::Classify` 将 HTTP 401 和 `12153` 归为认证问题，但并不定义本 Server 的刷新重放资格。目标 Provider 文档 §8.1 的 HTML 401 是可恢复的 HTTP 样本；§8.2 列出的 `12005`、`11212`、`11216` 是不可恢复的许可终态，§9.1 的 `12153` 需重新登录。参考仓两个未跟踪文档 `proxy.html`、`proxy.md` 不作为证据。

目标原先对所有 HTTP 401，或被业务码映射为 401 的 SSE 错误，都可能发起一次同账号刷新重放；这会把 `14003` 等已识别非认证码和不可恢复许可码当成可自愈错误。固定 HTTP 401+`14003` 回环在旧实现首组实际发送两次生成请求，单测也先红。现在恢复资格要求 HTTP 401 且没有已识别的业务拒绝码：无业务码 HTML 401、未知码 HTTP 401 维持原回退；已知限流/权限/请求拒绝、三项许可终态和 `12153` 均不刷新；HTTP 403 不因业务码升级为可恢复。`12005` 晚到 SSE 的旧“成功刷新”夹具改为终态拒绝，既不刷新绑定账号也不碰诱饵账号。下游状态映射与脱敏规则不变，出站 WorkBuddy wire、Intl/CN 站点、三 Surface、Account/Provider/Share 单绑定、唯一 `[DONE]`+EOF 和 receipt schema v2/revision 2 不变。离线合成错误不能证明厂商实际发送这些组合；两站继续 `live_pending`，真实账号验收不在本 goal 内。

## 2026-10-14 Antigravity Gemini 纯思考尾部的签名别名

`Antigravity/AGENTS.md` 首选的干净 `CLIProxyAPI@bf9341629ef2300ac9d846a2e80fe2756f63a671` 已提交 `a2718314` 的 `DropGeminiTrailingThoughtOnlyContent` 在 Gemini 目标删除末尾纯 thought 的 model/assistant turn；同仓 `antigravityNativePartThoughtSignature` 识别 `thoughtSignature`、`thought_signature` 与 `extra_content.google.thought_signature`。另一首选 `Antigravity-Manager@b601f5e7375343343922ccab5e50653bc3450c3b` 的 pipeline/thinking store 仅作交叉背景，未定义本 Server 的删除边界。参考不证明 Google 实际发送这些别名组合。

目标的 replay 解析已识别上述签名路径，但最终 Gemini wire 的尾部清理仅允许 `thoughtSignature`：`thought:true`、仅带 `thought_signature` 的末尾 model turn 固定反例在旧实现先红，仍留在 `request.contents`。现在仅把直接字符串 `thought_signature` 和结构精确为 `extra_content.google.thought_signature` 的嵌套字符串视为已知 thought 元数据；额外未知字段仍保留整 turn，不以宽泛递归删除掩盖正文、工具或媒体。既有至少保留一个 turn、只看最终外层 Gemini 模型、首次封装与 replay 共用归一化保持。不改 Antigravity/AGY endpoint/header、requestType、Account/Provider/Share 单绑定、签名 replay 的 scope/CAS、compaction disabled 或 receipt schema v2/revision 2。双 rail 与 AG-N6/AG-D3 的真实接受性继续 `live_pending`；真实账号验收不在本 goal 内。

## 2026-10-15 Claude 续线程内联工具定义的旧别名门槛

整体 review 再核对目标 `claude_thread_alias::needs_prior_alias_state` 与 `claude_oauth::rewrite_claude_tool_change_name`：后者会在 `tool_addition.tool_definition.definition.name` 为非 server 内联定义时重写名称，前者却只把工具增删中的 `tool_reference` 算作需要旧状态。续线程只声明新工具、历史内联定义沿用旧非法名称的固定单测先红，旧结果 `needs_state=false`；实际转发回环也先发出上游请求，返回 502 而非预期发网前 404。

现两处共用同一精确工具增删名称位置判定：旧名称不由本轮声明且确实需要别名时，必须找到上一 message ID 的精确作用域状态；缺失则 404、零上游请求，有状态则将历史内联定义恢复到缓存的 wire 名。`web_search_*` 等 server 工具定义、普通文本、工具 `input`、合法无需改名的名称、`tool_removal` 非引用形状均不扩大。定向 Claude alias 16/16、完整 Rust library 3628 passed / 1 ignored、API 合同 124/124、另两项集成各 1/1、Claude 私有 receipt 离线测试 252/252，以及严格 Clippy、格式、参考差分、Provider coverage、文档索引和差分空白检查均通过。此项只修本地发网前门槛和已有续线程名称恢复，不改 OAuth endpoint/header、Account/Provider/Share 绑定、缓存 TTL/容量/身份键、成功终态、usage 或 receipt schema v1/revision 1。离线回环不证明 Anthropic 实际接受内联定义；五份真实 operation receipt 与续线程厂商验收仍各自 `live_pending`，采集不在本 goal 内。没有运行 UI 自动化或部署测试。

## 2026-10-20 Codex Responses Lite 的并行工具开关必须显式关闭

按 `Codex/AGENTS.md` 只读复核干净的 `CLIProxyAPI@ae3ee3be303fb07fb8173279923413005bf32177` 与 `codex2api@5ee6c29273773a2398531d061d23f199ca8c25f7`。前者 `codex_executor_request.go::normalizeCodexParallelToolCalls` 先判 Lite 并显式写 `parallel_tool_calls=false`，普通请求才根据顶层工具是否存在删除该字段；后者 `executor.go::normalizeCodexResponsesLiteBody` 同时强制该布尔值及 `reasoning.context=all_turns`。公开 [Responses create 文档](https://developers.openai.com/api/reference/resources/responses/methods/create) 仅描述 `parallel_tool_calls` 为可选的并行工具调用参数；私有 Lite 必须显式为 `false` 的直接证据是本次上游 `unsupported_value` 错误与两个参考项目的出站实现，不将其冒充公开 API 保证。

纠正本文件 2026-10-13「Codex 无工具请求的并行调用标志」条目和 `docs/provider/codex-oauth.md` 的过宽表述：目标当时声称 Lite 保持强制 `false`，实际 `normalize_codex_responses_lite_body` 先写 `false` 又调用通用无工具清理，顶层工具为空时删掉该字段；原生 WS 末端重复清理，Compact 带 Lite header 时甚至跳过 Lite body normalizer。固定的最终合同测试在旧实现先红，出站 Lite body 有 `reasoning.context=all_turns` 却缺 `parallel_tool_calls`。现按最终模型能力门控的布尔决策，在普通 HTTP、Compact、原生 WS 和 WS→HTTP fallback 的最终请求策略/工具清洗后再次归一：Lite 总显式 `false`，即使顶层工具缺失、为空、仅有 `input[].additional_tools` 或图片工具剔除后为空；非 Lite 无工具继续删字段，非空工具保留原设置。Compact 同时补齐 Lite body 约束而不改变其单个 `compaction_trigger` 和上游 `/responses` 路径。Codex OAuth Account extra header 不能覆盖服务端 Lite 信号。未改模型能力判定、Account/Provider/Share 单绑定、SSE/WS 响应协议或 receipt schema。`cargo fmt --check`、`cargo check`、全量 Rust 测试（library 3637 passed / 1 ignored、API 124 passed、两项集成各 1 passed）、严格 Clippy、静态审计和本地 smoke 均通过；`RUN_TESTS=0 RUN_REAL=0` 离线 readiness 因显式跳过其内部测试、缺真实凭据和未测部署而报告 `blocked`，不是本地测试失败。离线夹具和本地回环不能证明真实 OAuth 上游已接受，真实收据仍为 `live_pending`。


## 2026-10-27 Claude R4 用量方言与强制工具边界

按 `Claude/AGENTS.md` 只读核对 `CLIProxyAPI@1f917e77c4562623aa49e9804d999524151a2f96`：`43f5386b` 的 `claudeUsageTokens.Merge/applyReasoningTokens` 将 Anthropic thinking 明细映射成 OpenAI reasoning，区分未出现和显式 0；`1d45a5f3` 的 `NormalizeClaudeToolChoice` 按 routed 5.5 模型将 any/tool 降为 auto。本仓不采纳放宽强制工具约束的策略，改为对精确已审核的 `claude-opus-5-5` 生成请求在发网前返回明确 400，保留其他模型策略和 count_tokens 字段过滤。检查在最终模型映射之后、身份/工具/签名改写之前；Antigravity 委托路径留在其独立切片核对。`pause_turn` 继续为 Responses incomplete/null，不依据参考 completed 策略改变本仓终态。

三个新用例在旧实现上分别复现 JSON thinking 明细没有 reasoning 别名、SSE 稀疏 details 覆盖旧计数，以及 Opus 5.5 forced choice 被接受。共享 JSON/SSE 转换现只将合法非负整数映射为 reasoning，Anthropic 名称优先，显式 0 保留；稀疏细项合并，非法晚到计数不抹去合法值，不向总 output/total 额外累加。cache/provider metering 和原生 Anthropic 字段不变。固定 Account/Provider/Share、刷新和重放边界未变。

本轮 R4 证据单独追加在 `assets/contract/claude-reference-delta.json::r4Reviews`，历史 observations/snapshots 不变；实现仍在工作树，`implementationCommit=null`，不能称为提交态证据。验证结果与剩余项记录在计划第 20 节。五个真实 operation 入口均以 RUN_REAL=0 检查到缺少 Server/Router/账号/Provider/Share/模型/receipt 输入，继续 blocked_inputs/live_pending，未发出真实请求，也不提升模型、工具或续线程接受性。

## R4 Qoder / shared tool identity review (2026-10-27)

Read-only committed-object comparison uses TokenRouter `d15137eb59d4d85bdc27ae563c37fcf9b0edebcd` / tree `df18b022d78c4760a89f5d0c372b5467bbc4c60f`, following its available project-doc and code-comments instructions. `assets/contract/qoder-reference-delta.json.r4Reviews` pins the bridge identity/history implementations and fixtures, Qoder payload conversion, and CN PAT credential/OAuth/status/site files by SHA-256. Eight pre-existing evidence sections, including all source snapshots and observations, retain their original content; the working-tree implementation has no implementation commit yet.

R4-CORE-01 rejects different active `(kind, namespace, name)` identities sharing one converted name before upstream access. Completed input history has a separate sorted, ASCII, bounded alias table with component-separated hashes and active/internal/bare-name reservations. It cannot authorize a selector or response mapping. Only direct discovery items may add declarations; explicit empty/null tools resets suppress that behavior, and parameter/result JSON remains opaque. Native Responses payloads remain native. Alias/error retained bytes are included in the existing request-memory estimate.

R4-QD-01 separates historical identity from next-call visibility. Declared completed history survives none or another named tool; Gemini source declarations are captured before allowedFunctionNames filtering as typed internal metadata, reserved and counted under the request budget. The next-call tool subset, exact batch pairing, continuation checks and unknown-history filtering stay enforced. `assets/contract/qoder-r4-history-delta.json` records final target COSY payload projections for retired namespace/reset cases and labels the differing reference behavior as source observation, not an executed external payload. Full retired-history acceptance still needs independent real Global OAuth, Global PAT and CN OAuth receipts.

R4-QD-02 remains researched/live_pending. The reference CN PAT flow posts `personal_token` to CN OpenAPI `/api/v1/jobToken/exchange`, then completes userinfo/Gateway identity and retains refresh/expiry. Its 1.24.2 identity does not supersede the independently frozen official 1.1.32 oracle. No CN PAT Registry/import/rail is enabled without an independent official package digest, wire oracle/mutations, identity/rotation contract and private real receipt. No external source is a build/test/release/runtime dependency, and no UI automation or deployment testing was performed.

## 2026-10-27 Antigravity R4 目录身份与两轮签名差分

按 `Antigravity/AGENTS.md` 只读核对 CLIProxyAPI `1f917e77c4562623aa49e9804d999524151a2f96` / tree `00273ce1709eac116a12932c5337ed41a9d449f6` 和 Antigravity-Manager `b601f5e7375343343922ccab5e50653bc3450c3b` / tree `9211a067165c376775c98ecf3d522b5562380ffa`。业务源码只通过提交对象研究，未写入、构建、测试或运行外仓。独立追加的 `assets/contract/antigravity-reference-delta.json::r4Reviews` 冻结来源文件 SHA-256、目标 fixture 入口及采纳/拒绝决定；原有十三个顶级证据字段保持不变，实现仍是工作树、`implementationCommit=null`。

R4-AG-01 将版本、完整 UA 和客户端 generation 作为一个不可变快照，贯穿 catalog 缓存、实际 HTTP 头、stale 回退、持久观察、发布 CAS 和能力消费。有效版本变化、A→B→A 与进程重启均使旧目录证据失效；旧或未带版本的目录不能作为当前身份的 fresh 能力。保留 Bearer、账号 incarnation/auth/token generation、project、endpoint、rail 隔离，以及成功空目录、128 项容量和同身份 stale 不延长证据的合同。ProviderExecution 共用快照选择搜索能力及实际 UA，旧请求的容量错误不能发布成新客户端观察。

R4-AG-02 的旧实现反例确认两种丢失：客户端省略前置 signed thought/carrier 而保留 unsigned calls 时丢签名；不同 chunk 的同名工具复用 part 位置但显式 ID 不同时被合并。现在只对同轮匹配调用回填前置签名，不把签名复制给并行调用，保留原思考段优先、已有签名冲突拒绝和历史/身份隔离；不同显式 ID 分开，同 ID 参数分片仍合并。`antigravity-r4-wire-delta.json` 只描述本地 fixture 期待，完整 HTTP 用例核对两条 rail、Claude/Gemini Surface、三种形状及 nonstream→stream 续接的最终报文。Claude Surface 的测试须显式绑定 Gemini 模型，不能把默认映射到 Claude 模型、未进入 Gemini replay 的请求误判成签名缺陷。具体执行结果与余项见计划 §20.3。

`tool_choice=none` 保留 Gemini `NONE` 和历史声明，不授权本轮调用。原生 Claude Opus 5.5 策略不直接套用到 Antigravity 委托合同；委托 any/named/none 与 thinking 仅作本地 wire 保真验证。无 ID 的完整调用/分片歧义待独立证据；不迁入 Manager 的追加“继续”、二次生成与拼接终态/usage，也不整体替换本仓 IR。Compact、未验证模型和两条 rail 的厂商接受性不因 fixture 提升；RUN_REAL=0 输入核对继续 blocked_inputs/live_pending，未发真实请求。未运行 UI 自动化或部署测试。

本轮验证使用相同 HEAD 的隔离工作树，带入 Claude/Qoder/Antigravity 改动，对 state.rs 仅带入本类十个 hunk，排除验证期间正在写入的 Share Market 改动。十四项定向及完整后端回归、严格 Clippy、构建通过；原 smoke 脚本改用已验证的构建产物启动后十项 HTTP 检查通过。来源审计保留 working_tree/implementationCommit=null 和隔离范围，不能据此声称含并行 Market 改动的整体根工作树已验证；readiness 本次仍 not-ready，详情见计划 §20.3。


## 2026-10-27 Codex R4 控制、被动额度与父预算入口（工作树切片）

按 Codex/AGENTS 推荐，以提交对象只读研究 CLIProxyAPI `1f917e77c4562623aa49e9804d999524151a2f96` / tree `00273ce1709eac116a12932c5337ed41a9d449f6` 与 codex2api `3c6adfda702efed67931b2d98a491d72bd9d91b5` / tree `6630ed69c78dd11dfd8bbbc3220155f3df4bfefb`。不执行外仓构建/测试/运行时，旧 sourceSnapshots/observations 及其摘要保持不变；五份独立 r4Reviews 冻结实际使用的来源文件摘要和本仓用例入口，implementationCommit=null、readMode=working_tree，不能当作提交态或厂商真实接受证据。

R4-CX-03 参照 CLI bootstrap metadata 与 codex2api preflight/idle 分类，只对闭集名称及审核形状引入 Metadata/Control。合法 metadata 不满足首语义内容或生成成功，不更新 first/idle deadline；SSE public/native 各自原有可见集合保留。活动轮次的前置 metadata 使用有界缓冲，空闲 metadata 不流入下一轮；错误先于 metadata，未知事件/字段保持保守语义。既有 SSE event/type 冲突拒绝不是本轮新增修复；Codex response.create 已发送后仍禁止透明 HTTP 重放，不采用参考更宽的恢复窗口或宽前缀丢帧。

R4-CX-04 的 control.rs 使用精确活动 response ID、物理连接身份、mode=discard_partial_items、去重、pending ACK 与已结束状态。原生 accepted/failed ACK 原字段透传，独立于生成业务和错误，不能取消另一轮或双结算；生成终态不能制造 ACK，默认等待30秒后丢连接且不重连/重放。HTTP fallback 明确返回 proxy-sourced interrupt_not_supported，保留已有输出并继续同一生成；若真实生成错误已经到达，先按该错误收口。它不合成 accepted/incomplete、不把所有 output 清空，也不宣称 HTTP 能完整履行 discard_partial_items。活动 response 身份变化沿用本地 error + Close 和一次 interrupted/502 usage，不能作为正常完成。

R4-CX-02 从审核过的新旧 rate-limit 方言及额度 headers 白名单提取稀疏快照，保留零与false、primary/secondary、plan 和完整 credits 布尔对，balance 有独立时间戳。每个显式 bucket selector 都必须匹配默认codex；metered/Spark、混合方言、未知routing/model/Authorization headers不能污染主窗口。reset乘加与期限有界，逐字段时间/权威查询时间和 Provider revision、账号 incarnation/auth/token generation 共同拒绝旧观察。读取帧时不等待DB，最多64个排队身份、每项向父预算保守预留8KiB、100ms合并、提交最多三次尝试。发布走state域方法；关闭/溢出/本地容量/提交失败不影响生成，权威额度查询负责恢复。被动观察不写推理token、Share消耗或Account cooldown，weekly-cycle仍走单独的、带token栅栏的既有观察入口。

R4-CORE-02 先把现有 request reservations 与 Codex观察队列接入进程父账本，默认只观测；CC_SWITCH_PROCESS_MEMORY_BUDGET_MB 可显式设置部署硬阈值。增长失败不改变父/子已占用量，子预算保持sticky exhausted，缩容和RAII释放不受exhaustion或上限下降阻挡；共享Bytes owner只记一次。aggregate used/high-water/limit/active reservations/rejections是逻辑保留量，不代表RSS或所有独立缓存已审核。跨类别cache/session owner切片及部署阈值测量仍是未完成项目；没有复制参考的128MiB默认值。

R4-CX-05 的独立 scripts/bench/codex-request-compression.py 只测宿主libzstd与合成JSON，51次/组的四种输入及脚本摘要冻结在codex-r4-compression-probe.json。短请求55→64字节、工具历史约1.47MB→401KB且编码CPU中位约7.5ms，说明需要大小/收益门槛；不证明Rust Server的TTFT、取消、并发RSS或上游接受。未启用新的HTTP/下游/WS压缩，也不采纳任意400后重发或仅为模仿身份默认开启的策略。

本切片的离线检查在带入前序Claude/Qoder/Antigravity变更的隔离工作树中执行，排除并行Share Market修改；具体结果与剩余事项见计划§20.4。统一性能里程碑、各类资源归属的完整验收及最终八类review尚未完成。五项真实operation仍live_pending，RUN_REAL=0不发真实请求；未做UI自动化、部署测试、提交或推送。

### Codex R4 message phase 与独立缓存归属（后续工作树切片）

R4-CX-01 的冻结参考是同一 CLIProxyAPI 提交中的 `codex_claude_response.go` 及 commentary phase/replay/cross-format 测试。本 Server 选择标准文本有损映射：原生 Responses 保留 phase；Messages 保留每个 message item 的文本 block 边界，Chat 顺序合并文本，历史再入只能恢复普通 assistant text。内部按 item 捕获 phase，稀疏 done 不抹已知阶段，冲突/未知值进入固定 Unrecognized；有界降级指标不记录原始未知字符串。公开 commentary 不冒充 thinking 或 encrypted reasoning，也不采用参考的 `cpa:commentary:v1` 私有签名协议；真实 encrypted reasoning 仍按现有认证封装回放。补齐 done-only 最后一条回答及所有 Responses→Messages/Gemini 中间状态的 retained bytes，不把已有 per-item delta 去重说成新建能力。

R4-CORE-02 的 Codex 后续入口覆盖 manifest 模型/能力字符串、previous-response JSON/scope/索引/tombstones、活动/空闲 WS 读帧、连接 wrapper/通道槽位/池索引及 referral 会话 key/entry。模型目录容量拒绝保留最近有效身份快照；previous-response 缓存拒绝维持严格 required-context miss，tombstone 预算不足可省略诊断而不放宽续接。原编码字节上限保留，JSON 分配容量与索引逻辑估值另计；编码大小计算使用无额外完整序列化副本的 counting writer。WS 消息携带其原 owner 跨 turn 移交，缓存 acquire 释放索引 owner，取消/淘汰通过 RAII 和 actor 退出释放；父容量不足不换账号、不重开生成。referral 进行中 clone 在缓存淘汰后继续持有同一个 owner。

这些是本 Server 独立设计的生命周期核算，不声称参考已实现相同缓存结构；库内部 cookie/TLS/HTTP、runtime、allocator slack 及精确 B-tree node RSS 仍不在逻辑账本内。具体专项与完整回归结果写入计划 §20.4；尚未完成的实际 Server 性能测量和跨类别 owner review 不因这段协议记录而判定完成。

本 phase/owner 切片在上述隔离树的最终源码上完成严格全目标 Clippy、格式/check/build 与完整 Rust 回归（3880通过、1忽略），后端 Node 1910通过；核对独立构建产物后，本地后端 smoke 10项HTTP检查通过。该结果不覆盖并行 Share Market 修改。离线 readiness 仍 not-ready：外部 Router 四处合同审计失败、真实输入缺失及按约束未做部署测试分别保留；不把本地 fixture 提升为厂商或部署接受证据。图片下载和 previous-response 注入临时副本、统一时间里程碑及实际 Server 性能测量仍需在当前 Codex 类别继续处理，详见计划 §20.4。

本切片已按实施前快照→隔离树的49文件增量同步主仓，四个并行Share Market重叠文件按hunk保留，其他838文件未变；Phase0摘要从主仓源重新生成。隔离验证范围和所有live_pending状态不因同步而扩大。


### Codex R4 图片下载与 previous-response 临时工作集（续接切片）

继续使用上述冻结 codex2api request_memory.go/test.go 的进程账本/共享 owner 原则，由本 Server 独立设计文件读取和 JSON 工作集归属；不声称参考已经实现相同的 Rust 缓存结构或图片合同。GET 在固定长度 payload 分配前准入父预算，保留文件长度、末端与 SHA 校验；Bytes 的最后一个 clone/slice drop 才释放。HEAD 分块验证全图 SHA，最多64KiB scratch，保留 Content-Length 与空body。容量拒绝保持本地503且不删有效文件。跨进程锁与磁盘256MiB总量仍独立，启动的 blocking 读在取消后自行完成并释放被丢弃结果。

previous-response 通过 domain/state 同步借用回调先准入再按白名单复制，保留 opaque call ID、类型配对、scope fencing、敏感字段清理及去重。当前input直接移动，解析/选择复制/合并/序列化按临时工作集核算；HTTP输出接现有Bytes owner，WS输出预留转入turn normalized owner。当前容量失败与真正required-context miss分开，不增miss计数、不扩大Provider处罚或重放边界。新增7项用例的最终验证见计划§20.4；统一时间里程碑、实际Server性能/RSS测量和Codex本类完整review仍单独待做。


图片/临时工作集续接切片的最终版本已重新完成格式/check、严格全目标Clippy、build、46项Codex定向、完整Rust3887通过/1忽略和后端Node1910通过。lookup scope key及配对检查索引都有临时allowance，容量拒绝不变成上下文miss。313个Rust文件冻结后未在编译过程中修改。原本地smoke10项HTTP检查通过；为节省构建磁盘，独立执行副本只去除调试信息，全部ELF SHF_ALLOC节地址/大小/类型/内容摘要保持一致，原文件和副本SHA分别保留。厂商接受性、统一性能里程碑、实际Server性能/RSS测量、Codex本类完整review与跨类别owner review仍不因这些绿灯提升；没有UI自动化或部署测试。


### Codex R4 单调时间里程碑与请求副本（续接切片）

本 Server 独立实现 request/attempt 单调时钟，复用此前冻结的 codex2api `security/request_memory.go` / `_test.go` 仅作为最后 owner 释放原则的参考；没有声称参考已提供同一 Rust 计时模型，也没有在本切片新增 TokenRouter 计时代码审计。HTTP 从转发入口计时，API 接收/鉴权耗时仍在外；WS 按数据帧建立 turn。metadata/control/heartbeat 不计首语义内容，attempt 终态与最终接受 attempt 的 request 终态分离。HTTP body handoff 不等于 socket 写入；WS 只在实际 sink 发送成功后记完成，并绑定入队时的 attempt，不将保守 Sent 当写入成功。最后已核算 owner 的 release 与终态、交付、异步 usage 结算分开，取消及迟到关闭不改写已经完成的结果。

新里程碑、operation wait 和结果指标只有固定标签；未归类异常保留 unfinished。读取等待不跨下游 body yield，重试等待及明确取消至末次 owner 释放分别记录，并行区间不相加为总延迟。既有 first_token_ms、wire、token、重试/提交和 Provider 健康逻辑未改变。HTTP 注入的 opaque previous_response_id 副本先准入，WS 借用原 ID；嵌套请求体及 active body 改为移动，保持精确 ID、required-context miss 和嵌套/平面 JSON 合同。周边 policy normalization、解析器/库 scratch 和其他类别缓存不因此宣称全部核算。

最终源码上56项 Codex R4 专项、完整Rust3897通过/1忽略、后端Node1910通过，格式/check、严格全目标Clippy和build通过。314个Rust文件格式化后冻结，有效编译期间无源码变动；原本地smoke10项HTTP检查通过，独立去调试副本的全部ELF SHF_ALLOC节保持一致，临时listener/config/launcher清理完毕。该验证仅覆盖隔离树，不包含主仓并行Share Market修改，细节见计划§20.4。实际 Rust Server 同负载的延迟/取消/阻塞/RSS 测量、R4-CX-05 压缩决策及 Codex 本类完整 review 仍未完成；五项真实 operation 保持 live_pending。未做 UI 自动化、部署测试、提交或推送。


## 2026-10-27 Codex R4 Server 测量装配与回退写入观测（正式采样中）

生产时间观测仍是本Server独立设计，未在本切片增加TokenRouter计时代码审计。新增根test_support下的Linux ignored测试装配真实api::app_router与正常v2签名Share ingress，不引入proxy→api/clients依赖或生产endpoint开关。固定合成CodexOAuth绑定使用既有cfg(test)端点覆盖，并在API listener前核对HTTP/WS runtime plan的精确回环scheme/host/port和URL形状。driver清除子进程显式出站代理、固定父预算仅观测，冻结源码及输入摘要；四路径与取消/慢读共22行，最终小样本全部通过。

实际小样本暴露fallback_ws缺下游写入时间；relay_codex_http_fallback_event已补实际sink等待与仅Ok后的完成标记，错误返回、重试、提交、token及健康判定不变。普通回归证明发送观察不制造first_semantic/terminal/result。最终格式/严格Clippy/check/build及完整Rust3898通过/2忽略（57项Codex R4）通过；10项本地后端smoke使用独立去调试副本，全部SHF_ALLOC节保持一致，临时listener/config/launcher清理、日志0600。315个Rust文件格式化后冻结，编译期间未变。上述结果仍限隔离树，不包含主仓并行Share Market修改。

正式101/31次采样尚未完成；不得使用小样本或先前codec数字声称正式性能、生产容量或厂商接受。RSS口径是整个debug测试进程，包含Server、假上游、客户端、collector、sampler及allocator retention；顺序负载和logical bytes都不能冒充分配计数或部署压测。前期错误的WS override装配可能回退默认地址，该轮已停止并排除，不能声明其零可能厂商连接；最终回环装配单独验证。五项真实operation保持live_pending，没有UI自动化、部署测试、提交或推送。


## 2026-10-27 Codex R4 正式Server测量与编码决策收口

`assets/contract/codex-r4-server-probe.json`冻结真实API router、v2签名Share入口和固定合成绑定的四路径22行loopback测量。19行各101次、慢读三行各31次，共2012次measured generation，排除110次warmup；每请求一次上游生成，request owner release/high-water及两条WS的实际sink等待/完成观测齐全。输入复用codec生成器且原始bytes/hash一致；prepared字节先于WS envelope，实际上游规范化应用字节另计。315个Rust文件及driver冻结的源码/构建输入测量期间未变，listener和输入目录清理已核对。最终资产及方法/数字见计划§20.4。

延迟是顺序debug测试装配的nearest-rank分位数；HTTP入口、WS握手、first_semantic、body handoff、sink成功、终态及最后逻辑owner各有明确起点/边界，操作等待可重叠，取消drain不代表异步usage完成。整进程采样RSS峰值318.34 MiB包含Server、假上游、客户端、collector/sampler及allocator retention；物理socket/actor保留量与request释放不同。该结果不证明独立生产RSS、并发容量、allocator分配数或前后提速；不据此选择进程硬阈值。

R4-CX-05独立复核冻结的CLIProxyAPI/codex2api压缩来源后，决定本轮keep_current_encoding并关闭候选，不新增HTTP出站、下游或WS压缩。codec和当前未压缩Server基线不能证明相同端点/rail的真实接受、网络收益、flush/协商安全或scratch预算；不采用任意400/415后明文重发。新增第七份R4 review和Codex离线category review，18条source delta、22条旧observation及历史codec资产不变，implementationCommit=null。五项真实operation仍live_pending；其他类别缓存/计时范围、部署硬阈值和最终跨类review继续待做。隔离树完整Rust3898通过/2忽略、57项R4专项与本地smoke10项不覆盖主仓并行Share Market集成；没有UI自动化、部署测试、提交或推送。


正式冻结后的当前后端Node为1917/1917，纯数据合同5/5，Codex冻结源码及14项后端/静态检查通过；状态写入/分层五项检查通过。曾有一次1914/1的产品边界误报：外置测试模块未继承crate root的cfg(test)。审计现仅对可证明的声明式root测试模块继承排除，生产引用或路径/include引用撤销排除，注释/字符串/path属性不豁免，并有两项新增反例回归；Rust与已冻结探针源码未改。全部22行prepared JSON另经独立重建核对bytes/hash，未混用WS envelope或上游规范化字节。主仓增量及并行改动的验证边界继续分别记录。


Codex本类17文件增量已同步主仓，876个无关文件保持不变，Phase0从主仓重生；主仓格式及12项相关静态/源码/文档检查通过，Rust在核验期间未变。完整Rust3898/Node1917/smoke10的范围仍为隔离树，不覆盖主仓并行Share Market。离线readiness以RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0实际退出1/not-ready，原因保留为外部Router四处合同问题、八项真实输入缺失、local-contracts-unverified与deployment-not-tested。Codex离线切片收口，真实五operation仍live_pending，implementationCommit=null；下一类Cursor及最终跨类review仍待推进。


## 2026-10-27 Cursor R4：长期资源父预算与 h2 生命周期

按 Cursor/AGENTS.md 只读复核 OmniRoute@443d66996d69d7ecf887fd61158404a3cca1b192 的已提交 executor、agentEndpoint、builtinToolBridge 和 cursorSessionManager，排除 21 项 tracked 和 1 项 untracked 变更。参考的同一 h2 工具续接、五分钟 TTL、100 会话容量与关闭清理继续作为生命周期证据，不推断新的厂商 wire 或复制单账号查找规则。

目标原有 running/parked request reservation 已接父预算，但 completed Responses、token/cooldown、model/endpoint cache、single-flight 和冷续接 claims 未独立归属。现以独立 RAII owner 接入同一进程账本；缓存有界 B-tree 避免空 HashMap capacity 在条目释放后滞留，稀疏索引采用保守逻辑估算。计数不冒充 RSS/allocator。completed-cache 的 serialized size 改为有上限的计数 writer，不先分配完整 JSON 副本；独立缓存仍是惰性过期，parked h2 保留既有主动 timer。

新 flight 容量拒绝发生在发网前；claims 全批预留后才发布，父预算失败返回既有 cc_switch_request_memory_exhausted，不消费半批 ID。替换失败移除同 key 旧目录/endpoint/token/context，避免旧授权复活；已到达的本次目录/endpoint/token 结果可以交还调用者而不缓存，未伪造跨 rail fallback。关闭 session 释放 Vec/String/HashMap 的实际容量并在移除索引后缩容；仍存活的 Arc 外壳直到最后 drop 才归还费用，不能把 registry 删除当所有内存归零。首次定向测试 360 通过/1 失败揭示旧“关闭即归零”断言仍持 Arc，记录保留并改为最后 owner 释放后归零；h2 夹具首编的 Unpin 类型错误另已修正。

16 项 R4 用例覆盖父预算压力、独立请求 clone、替换/淘汰/惰性到期、原子 claim、parked 转移失败和外壳释放。纯回环 h2 夹具实际观察取消/首内容超时、两 rail 的停放到期/容量淘汰后上游 request 关闭及 response owner drop；新 token 代际的迟到结果不能获取旧 stream。它不是 TLS/厂商接受性、生产 RSS/并发容量或独立进程性能测量；已有 firstTokenMs 仍从 native driver 入口计时，只接受非终态批次的渐进业务输出，没有新增成功 socket write 计量。协议、身份与 receipt schema 不变，八条旧 observation 不改，implementationCommit=null，两条 rail 继续独立 live_pending。最终离线检查范围与主仓同步见计划 §20.5；未做 UI 自动化、部署测试、提交或推送。


Cursor R4 最终隔离树验证：fmt/check/严格全目标 Clippy/build 通过，完整 Rust 3914 通过/2 忽略，其中16项 R4 用例通过；后端 Node1917/1917、另12项相关静态/源码/文档检查及新构建本地后端 smoke10项通过，冻结输入未变。strip-debug副本的全部SHF_ALLOC节保持一致，临时listener/config/launcher已清理，含临时token的完整smoke日志不公开。这些成绩不覆盖主仓并行Share Market；主仓同步和离线readiness另行记录，真实两rail仍live_pending。

Cursor R4 主仓收口：18 文件无重叠增量已应用，876 个无关文件未变，coverage/Phase0 从主仓重生；主仓格式及12项相关检查通过，Rust 核验期间不变。3914/1917/10 完整成绩仍限隔离树，不覆盖主仓并行 Share Market 集成。离线 readiness 以 RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 实际退出1 / not-ready，保留外部 Router 四处合同问题、八项真实输入缺失与 local-contracts-unverified / deployment-not-tested；两个 rail 独立 live_pending，implementationCommit=null。


## 2026-10-27 Grok R4：重提交差分与独立 resident owner

按 Grok/AGENTS.md 对 grok2api@e2fd7ab481a30a1965bb61395d437154cf247c92 与 sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2 仅读取已提交 Git objects，分别排除0/6项工作树变更；7份来源文件的 tree/hash 追加在 r4Sources，不覆盖旧 observation。参考 transient counter 与生成一次的 Idempotency-Key 属于 Build selector 重提交合同，不能授权本 Server 扩大恢复或更新版本。目标回环验证500/503/525/526各一次POST、无新两个header；保留401/proof明确拒绝与WS fallback的同绑定/提交边界，Web/SSO与95%soft quota gate不采纳。既有GroK凭据CAS反例保留，新header接受性继续live_pending。

本片独立实现 replay/CAS占位、媒体快照与持久化owner、Grok目录及OIDC/JWKS缓存的父预算；新增有限JSON计数writer。媒体候选先预留后复制，Arc共享同表，写盘/容量失败不覆盖实时归属；新容量分支经回环确认记capacity shed而非Provider失败，且不重放已生成任务。目录Arc payload/代际门槛保留；可选OIDC缓存压力不等于认证失败。稀疏元数据为保守逻辑估计，不是RSS，控制面scratch和全部持久化状态不属于已覆盖声明；本类未增加全里程碑或真实socket-write计量。首轮生产cargo check通过，新夹具因未引入uuid而编译失败；改用现有opaque_id后严格全目标Clippy和Grok定向281/281通过，其中16项R4用例通过。当前完整回归/主仓同步待完成，三个operation仍live_pending、implementationCommit=null；不做UI自动化、部署测试、提交或推送。


Grok R4媒体快照的提交后错误对账采用临时plain store/value读取，不再为确认已提交数据申请第三份resident快照。新增预算已由live/candidate占满的提交后错误回归；临时对账/序列化scratch仍不属于完整父预算覆盖。此前完整Rust编译主动中断，不计为通过，修复后的最新验证范围见计划§20.6。


Grok 修复后完整离线检查：fmt/check、严格全目标Clippy、cargo build通过；完整Rust为3931通过/2忽略，其中17项新增R4全部通过。后端Node1917/1917及另外12项相关源码/合同/文档检查通过，另核对五项状态写入/分层约束。全部385份冻结构建输入在各Rust阶段及后端检查期间不变。新构建本地后端smoke10/10，strip-debug副本全部SHF_ALLOC节一致，临时listener/config/launcher已清理，日志0600且未公开token日志。成绩范围是隔离树，不包含主仓并行Share Market完整集成。媒体提交后对账不申请第三份resident owner，容量错误不罚账号；review保留固定绑定、commit/CAS、可选缓存miss及未覆盖scratch边界。下一步只同步本类16文件增量并核验主仓，不将新实现冒充提交态或厂商真实接受。


Grok 主仓同步与收口：16个源/文档文件按Cursor收口基线增量应用，src/state.rs与并行主仓有差异但hunk预检/应用均无冲突，880个无关文件摘要不变、无新增无关文件，coverage和Phase0三份生成件从主仓重生。主仓fmt及12项相关源码/合同/文档检查通过，核验期间Rust源码不变；不替代并行Share Market的完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：外部Router两处退役Market端点、缺Share Contract v8 reader constant及缺retained Market能力四处问题仍在，八项真实输入、local-contracts-unverified和deployment-not-tested保持阻塞。未修改外仓或放宽门禁。Grok离线切片收口，三个operation仍live_pending，implementationCommit=null；下一类Kiro，随后CodeBuddy与最终整体review。无提交、推送、子代理、UI自动化或部署测试。


## 2026-10-27 Kiro R4：保留 usage 权威并补缓存/批次 owner

按 Kiro/AGENTS.md 推荐，只读研究干净 kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903 / tree 5d22f040f0e4b0aa775e61547540b3c040293cd8 的七份 committed objects：token、frame、CLI/IDE endpoint、cache metering 和 stream。其 contextUsage 减输出说明并非本 Server 的八个 auth-kind×region 真实回执；R4-KI-01 保持 metadata.tokenUsage > metrics > context/fallback、显式零、已审输入估算和 shadow-only 扣减，不迁入账号池、跨区域恢复、Redis/shared cache 或 remote compaction。r4Sources/r4Reviews独立追加，旧来源/observation不覆盖，implementationCommit=null。

本地原有 EventStream/图片/请求 reservation 已接父预算，Prompt Cache 的 entries/pending/writer 则未独立归属；原 pending tombstone 可在长期写盘失败或无 writer 时积累。现缓存/dirty index 改有界 B-tree，每个条目和待写 mutation 持独立owner，detach/失败合并移动owner，pending与in-flight共享8192项上限，通知队列仍为2。整次 TTL/条目/dirty发布先准入；拒绝只回退为无本地缓存估算，原state和删除记录保持，vendor权威usage仍优先。无writer不创建dirty队列，启动未获准条目不进内存、不删除磁盘数据。请求中的expiry/eviction计划scratch也先计入父预算；SQLite/legacy启动解析和连接内部scratch未声称全覆盖。

新增十三项R4回归覆盖内存only、原子拒绝、TTL/代际保留、in-flight计数、持久化tombstone、新旧batch合并、到期/容量owner、启动压力，以及真实adapter的取消/EOF/截断/idle timeout与输出Bytes最后释放。它们不证明厂商socket关闭、账号健康取消分类、RSS或生产提速。原 firstTokenMs 继续沿用尝试起点，完整frame/idle deadline不改；没有新增统一milestone或socket写成功计量。八个RUN_REAL=0 scope均blocked_inputs/live_pending，无真实请求。验证与同步进度见计划§20.7。


Kiro 资源清单复核补入独立模型目录与IDE版本缓存：原模型HashMap没有计数上限/统一惰性清理；现B-tree最多128目录，5分钟fresh/24小时最大stale不变，cache snapshot的Arc payload/owner共享直到最后释放。发布前预留，压力下移除被替代旧目录，当前已获取结果继续返回且不缓存；IDE仅一份可选保留值。四项新增回归核对最后owner、替换拒绝、淘汰/到期、IDE压力。控制面返回Vec/String投影及解析scratch未因此全覆盖。原有跨区域discovery候选只在既定条件下保留，不扩大推理的区域/认证rail恢复。第一轮fmt/check/严格Clippy通过后，在Rust定向测试编译阶段主动中断（exit130）补齐此遗漏，完整测试未获结果；旧报告保存，新输入须重新验证。


Kiro 修复后完整离线检查：最终fmt/check、完整Rust3944通过/2忽略及cargo build通过，其中十三项R4全部通过；严格全目标Clippy引用第二轮相同321份Rust源码的通过记录，之后只更新review状态JSON，未改变Rust实现。387份最终冻结输入在Rust步骤与后端审计中不变。后端Node1917/1917及12项相关源码/合同/文档审计通过，另核对七项状态写入/依赖方向约束。本地后端smoke10/10，strip-debug副本全部SHF_ALLOC节一致，临时listener/config/launcher清理完成、日志0600。review核对dirty与in-flight owner移动、批量准入原子性、旧generation不能覆盖新状态、目录压力不复活旧授权及EventStream输出Bytes末次释放；计量优先级、CRC/截断和固定身份仍保持。逻辑保留量不等于RSS，不声称解析/SQLite内部scratch已全覆盖，也不证明厂商socket、八路真实接受或取消健康分类。结果范围是隔离树，未包含主仓并行Share Market完整集成；八路继续live_pending，implementationCommit=null，下一步按本类增量同步主仓。


Kiro 主仓同步与收口：13个源/文档文件按Grok收口基线增量应用，hunk预检及应用无冲突，885个无关文件摘要不变、无新增无关文件；coverage及Phase0三份生成件从主仓重生。主仓fmt及12项相关源码/合同/文档检查通过，核验期间Rust源码不变；不替代主仓并行Share Market完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：外部Router四处合同问题、八项真实输入缺失、local-contracts-unverified及deployment-not-tested仍在，未修改外仓或放宽门禁。Kiro离线切片收口，八路真实验收仍live_pending，contextUsage减输出仍shadow-only，implementationCommit=null。下一类CodeBuddy，之后做最终跨类review；无提交、推送、子代理、UI自动化或部署测试。


## 2026-10-27 CodeBuddy R4：保留受审身份并接目录父预算

只读研究cli2api@c7e4e73f/tree06520c39的客户端身份、refresh、response-head deadline和WAF相关实现/测试，共11份committed对象及明确函数锚点；排除未跟踪proxy.html/proxy.md，未执行外仓。本仓已有超时、非认证refresh分类、11128诊断、11148历史修复、终态usage和取消guard，不重复从零开发。参考CLI2.161.0、Global/CN Desktop5.8.4/5.8.3、IDE2.168.2及chat-only IDE/Product/Session组合只构成差分候选。没有本仓两站真实接受证据，保持现有2.142.0整套身份与目录来源，不单独升UA，不把Global desktop目录更多模型外推到生成。个人CLI X-Private-Data=false保持；企业不随此项开放。WAF正文重写不采纳。

本地目录改为最多128项B-tree，保留8分钟fresh/24小时最大stale，查询或发布惰性清理。不可变Arc payload与resident reservation共享到最后snapshot释放；同key新目录无法缓存时移除旧授权，当前新结果仍可用且不缓存。新single-flight发目录请求前准入父预算，最多128个活跃/保留注册；等待者与guard共享owner。容量拒绝经state/API/forwarder映射为本地503 memory-capacity，不刷新、不重放、不计Provider NetworkFailure。索引/旧snapshot保留量为逻辑估算，控制面fetch/parse及不缓存结果scratch不宣称全覆盖。

八项R4回归已写入：六项目录/flight owner合同、两站×三入口的目录准入拒绝零请求/无lease泄漏、提示词/思考/工具结果正文保真；尚待编译通过。现有两站三入口报文、固定账号401及取消/终态尾部usage夹具继续复用，未伪造新身份接受。firstTokenMs仍是现有尝试起点至第一个非空下游transform输出，header与body/idle deadline保留各自范围；没有新增milestone、socket-write或性能结论。两站RUN_REAL=0均blocked_inputs/live_pending，implementationCommit=null；完整验证及主仓同步待做，最终跨类review仍未完成。


CodeBuddy 修复后完整离线检查：最终fmt/check、完整Rust3952通过/2忽略及cargo build通过，八项R4均通过；严格全目标Clippy引用首轮相同322份Rust源码的通过记录，之后只更新review状态JSON。388份最终冻结输入在Rust步骤与后端审计期间不变；后端Node1917/1917、12项相关源码/合同/文档审计和七项状态写入/依赖方向检查通过。新构建的本地后端smoke10/10，strip-debug副本全部SHF_ALLOC节一致，临时listener/config/launcher已清理，日志0600。review确认目录snapshot直到最后Arc释放才退账，替换拒绝不复活旧授权，single-flight waiter/guard保留owner，容量拒绝在两站三入口零出站且不刷新或罚账号；提示词、思考与工具正文保持。逻辑索引估算不代表RSS，控制面获取/解析及不缓存结果scratch不在全覆盖声明。成绩范围为隔离树，不含主仓并行Share Market完整集成；两站真实接受仍live_pending，保留2.142.0整套身份和目录来源，implementationCommit=null。下一步仅同步本类增量并核验主仓。


CodeBuddy 主仓同步与收口：14个源/文档文件按Kiro收口基线增量应用，hunk预检及应用无冲突，885个无关文件摘要不变、无新增无关文件；coverage及Phase0三份生成件从主仓重生。主仓fmt及12项相关源码/合同/文档检查通过，核验期间Rust源码不变；不替代主仓并行Share Market完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：Router四处合同问题、八项真实输入缺失、local-contracts-unverified及deployment-not-tested保持，未修改外仓或放宽门禁。CodeBuddy离线切片收口，Intl/CN两站仍live_pending，保留2.142.0身份及原目录来源，CB-N4保持禁用，implementationCommit=null。随后进行最终跨类review，包括早期独立缓存和Cursor取消分类的复核；无提交、推送、子代理、UI自动化或部署测试。

R4 最终跨类资源 review：Antigravity replay/CAS与目录补独立owner、clone前准入及拒绝后防旧发布；Qoder session、目录、两类flight和continuation共享实际owner，本地容量503不走auth恢复；Claude线程别名缓存/请求分开计量，冲突栅栏保持；Cursor下游drop记client_cancelled，不写Provider成功或失败样本，原metrics问题不夸大为直接账号处罚。Antigravity历史审计修正为读取声明的提交对象并核对tree，历史22条观察及摘要不变，当前R4/资源准入合同仍检查工作树；四项Node反例验证两者不能互相替代。无新厂商header/payload/能力门禁，四类本地fixture不等于真实接受。CORE03/CX05测量、可选扩展和预算边界见计划§20.9。

最终跨类 review 完整离线检查：fmt/check、严格全目标Clippy及cargo build通过；完整Rust为3977通过/2忽略、零失败，包含25项新增review回归。后端Node为1921/1921，其中四项新增审计回归通过；另27项相关检查（八类reference-delta默认与显式source审计、conformance、产品边界、Provider/Phase0/coverage、transform/bridge、文档、静态UI矩阵、脱敏和diff）及七项状态写入/依赖方向检查通过。Rust的391份冻结构建输入在各阶段不变，后端审计另冻结脚本输入。新构建本地后端smoke10/10；strip-debug副本全部SHF_ALLOC节一致，日志0600、临时listener/config/launcher已清理。完整后端成绩属于隔离树，未包含主仓并行Share Market的完整Rust集成；厂商真实receipt继续live_pending、implementationCommit=null，未做UI自动化或部署测试，也未提交或推送。

最终主仓同步与整体 review 收口：19个源/文档文件按CodeBuddy收口后的review基线增量应用，hunk预检/应用无冲突；884个无关文件摘要不变，无新增无关文件。src/state.rs保留主仓并行差异，coverage和Phase0三份生成件从主仓源重生。主仓fmt及另外27项相关合同/来源/文档检查通过，期间Rust源码不变；隔离完整Rust/Node/smoke成绩不替代主仓并行Share Market完整Rust集成。实际RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0 readiness退出1/not-ready：只读Router合同仍报4处问题（frontend/components/i18n/locale-provider.tsx、src/market_fx.rs、src/models.rs、src/market_billing.rs），缺8项真实输入，local-contracts-unverified、deployment-not-tested保持；不以独立通过的后端检查改写该命令结果，不修改外仓或放宽门禁。八类离线实施及最终整体review收口，全部真实rail/operation仍live_pending，明确的可选扩展保留于上文；未提交、推送、调用子代理、做UI自动化或部署测试。

## 2026-10-27 CORE-03 完成性复核：Claude 统一时间观察

按计划§20.10继续补齐跨类request/attempt观察器，当前切片仅扩展ClaudeOAuth。此前Codex的实现与总体进度不再被当作其他七类均已接入的证据。本次是Server独立观测设计，没有新增厂商wire事实或参考仓计时代码来源；既有Claude引用、冻结fixture和真实receipt状态保持。

三类App的Messages/Chat/Responses/Gemini流非流路径与count_tokens共用有限静态标签，起点仍是本地转发入口；原first_token_ms、deadline、用量、提交和恢复规则保持。SSE复用原terminal detector已经解析的Value，原生JSON复用语义验证，桥接JSON复用usage解析；只为计时做的校验失败不改变桥接或guard关闭时的响应判定。首内容排除空block、ping、签名、usage和停止原因；有效零count_tokens结果可观察。max_tokens/pause_turn/model_context_window_exceeded记为本attempt的incomplete，后续attempt不继承。pending forward取消先记cancelled，再释放其future/预算owner；流终态沿既有验证路径观察，body handoff与最后已核算owner释放分别记录，均不宣称HTTP成功socket写入或远端已消费。

回环夹具覆盖九种成功请求形状、headers取消、提交后取消/error不重放与owner释放，另核对控制事件和attempt隔离；最终验证与主仓同步状态见计划§20.10.1。真实Claudeoperation仍分别live_pending；其他类别CORE-03和当前主仓全量集成继续待做。未做UI自动化或部署测试，未提交、推送或调用子代理。

Claude CORE-03 主仓同步与本类收口：10个源/文档文件以冻结主仓为基线增量应用，预检与应用无冲突，897个无关文件摘要不变且无无关新增。主仓393份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、10项相关源码/合同/文档审计及diff检查全部通过，验证期间输入不变。本类574项相关Rust回归与严格lib/tests Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。未改变registry/API或手改生成件。下一类仅推进Antigravity CORE-03；其余类别及最终全量集成/review仍待完成，总目标继续进行。全部真实operation/rail仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。


## 2026-10-27 CORE-03 完成性复核：Antigravity 双 rail 时间观察

本切片由Server独立实现时间观察，沿既有Antigravity/Agy冻结wire合同接入两条rail，不增加外部参考源码、厂商字段或接受性事实。两条rail各用固定Provider标签，只覆盖当前登记的Claude/Gemini App与Gemini countTokens；Gemini CLI和未登记的Codex App不因此获得计时覆盖或能力。起点仍是本地转发入口，不包含API收包/鉴权，原first_token_ms、deadline、用量、提交、账号健康和恢复规则保持。

非流式的首内容在既有SSE聚合器处理已解析事件时观察，成功finish后再记录聚合终态；流式复用terminal detector解析与既有最终结算。JSON生成/计数复用现有embedded-error检查的Value，没有为指标另建完整响应树。空文本、纯签名carrier、usage、停止原因和心跳排除；有效零totalTokens算结果，MAX_TOKENS的incomplete提示仅属于本attempt。blocked prompt可以有完整终态而无首内容。原两处Google RPC短延迟sleep补独立Retry wait，不增加重试或缩短延迟。HTTP只记录body handoff，末次已核算request owner释放与取消/语义终态分开，不宣称socket成功写入。

新增六项本地回环/语义用例覆盖两rail、两App、流非流、零计数、聚合未EOF的首内容、headers/聚合/提交后取消、提交后error零重放及短延迟恢复的attempt隔离；相关Rust回归528通过、零失败，fmt/check及全目标严格Clippy通过，394份冻结构建输入不变，11项相关源码/合同/文档审计通过，主仓同步结果见计划§20.10.2。快照带入冻结时的主仓Share Market修改，定向成绩不替代最终全量集成。两rail真实receipt、Compact与其他能力门禁保持live_pending/disabled，implementationCommit不提升；未提交、推送、做UI自动化或部署测试。

Antigravity CORE-03 主仓同步与本类收口：8个源/文档文件以冻结主仓为基线增量应用，预检与应用无冲突，900个无关文件摘要不变且无无关新增。主仓394份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、11项相关源码/合同/文档审计及diff检查全部通过，验证期间输入不变。本类528项相关Rust回归与严格全目标Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。未改变registry/API或手改生成件。下一类仅推进Cursor CORE-03；其余类别及最终全量集成/review仍待完成，总目标继续进行。全部真实operation/rail仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

## 2026-10-27 CORE-03 完成性复核：Cursor h2 与请求/会话边界

本切片只读核对Cursor导航约定，沿已有OmniRoute冻结wire证据独立设计Server计时，没有新增参考源码、厂商字段、模型能力或真实接受事实。CursorOAuth/CursorApiKey接入共享request/attempt起点；新建h2观察原发送/headers及逐次body读取，工具结果发送仅作入队观察，不宣称socket写成功。经过既有解码、marker处理和工具校验的实际业务内容才产生FirstSemantic；原firstTokenMs、deadline、usage、健康及恢复规则保持。

共享Body观察器在内部poll可能重试后读取当前attempt，上一帧DownstreamDemand仍绑定上一attempt；终态和HTTP交付/取消保持分离。工具turn完成后parked会话继续持有实际预算，续接转移到新request并保留同一h2，不伪造新headers或提前宣称owner已释放。Auth/工具约束只观察原恢复准备，未增加重试或跨身份回退。回环覆盖与验证/同步见计划§20.10.3；相关Rust回归667通过、零失败，fmt/check及全目标严格Clippy通过，395份构建输入全程不变，12项相关源码/合同/文档审计通过。夹具曾错误提前要求部分移动后仍持有的独立响应owner释放，已改为验证其存活和丢弃两个边界，不改生产释放语义。未使用厂商凭据或执行外仓代码，两rail真实receipt仍live_pending，implementationCommit不提升，未做UI自动化或部署测试。

Cursor CORE-03 主仓同步与本类收口：10个源/文档文件按Antigravity收口基线增量应用，预检与应用无冲突，899个无关文件摘要不变且无无关新增。主仓395份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、12项相关源码/合同/文档审计及diff共15项检查通过，验证期间输入不变。本类667项相关Rust回归与严格全目标Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。共享Body观察器已随Claude/Antigravity/Codex定向回归核对，未改变registry/API或手改生成件。下一类仅推进Grok CORE-03；其余类别与最终全量集成/review仍待完成，总目标继续进行。真实rail/operation仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

## 2026-10-27 CORE-03 完成性复核：Grok HTTP、WS 与媒体

本次仅作只读参考核对：grok2api@e2fd7ab481a30a1965bb61395d437154cf247c92的gateway/timing.go（SHA-256 `8b5cc24c164dea7b62b25d93716a8fc5fded85724dca768b443a3d1ad73c2748`）、cli/semantic_streamidle.go（`6c798b00f949278cf28940431f78a31b1f13de4eea32ca31826606c87c7c597e`）、cli/video.go（`e0278bb89a356e960945ac213b286f891be20e95808d3fe6f02e8f4a8fdbe405`）；sub2api@ab99d56e9626e6cd731592dae8553c9758a0efa2的service/openai_gateway_grok_sse_filter.go（`e00567f12c8dc0cbad594992899d4d44e35e31930e6b524214de40f467d8eb7a`）、service/grok_media.go（`25564d5c4549315e7b827f12c50b9718cffe9e5a318c3d34fd757d519cf27718`）。完整路径、基线缺口和实现边界见计划§20.10.4；五份文件与对应Git对象逐字节一致，未构建或执行外仓。参考的first body、语义idle与本仓首语义内容口径不同，不混作同一性能指标。

Server独立计时补齐GrokOAuth的三App HTTP、每回合原生WS/HTTP回退与独立图片/视频入口。复用原JSON/SSE解析观察真正内容、原发送/headers/读取/本地writer完成，以及同账号刷新或proof恢复的独立attempt与retry wait；私有control/ping、connected/heartbeat和纯progress不冒充首内容。图片SSE的附加观察借用RawValue，不构造第二棵含大图/base64的完整Value树；普通JSON复用已有解析。媒体计时失败与既有HTTP/JSON成功决策独立，EOF不能覆盖已观察failure。HTTP只报告body handoff，最后已核算owner释放与终态/取消分开，视频status HTTP请求结束不代表后台任务完成。

本轮冻结验证完成：相关Rust 664项及补跑websocket 76项均通过（范围重叠），fmt/check、严格全目标Clippy及13项相关审计通过，397份构建输入不变，具体失败纠正与边界见计划§20.10.4；主仓同步与最终集成仍待完成。原first_token_ms、deadline、usage、健康、固定身份及恢复资格保持。没有新增remote compaction执行路径或任何厂商接受性事实；inference、media、remote_compaction均仍live_pending，implementationCommit=null。禁止的UI自动化和部署测试未执行。

Grok媒体观察scratch以同一请求/进程父预算可选预留：不足时只省略本帧计时，不能污染转发exhausted标志或改变既有恢复/响应；必需预留合同不变。完整JSON错误、后续完成帧/EOF后的媒体failure保持、multiline CRLF及预算拒绝/共享owner边界已加入回归，上述边界回归全部通过，详见计划。

WS每turn观察槽仅保留当前计时，取消保护先于pending future与其owner析构；异常退出补failed，已完成turn不因后续连接错误改写。共享WS回归另行完整覆盖。Grok普通输出及两种尾帧把既有预留转交实际Bytes owner，不重复准入或持有account lease。三项初始夹具/owner失败及修复已如实保留于计划，未放宽normalizer或启用Grok非流SSE聚合。

Grok CORE-03 主仓同步与本类收口：10个源/文档文件按Cursor收口基线增量应用，预检与应用无冲突，901个无关文件摘要不变且无无关新增。主仓397份构建输入与本轮已验证树逐份摘要一致；主仓fmt、include单独格式、13项相关源码/合同/文档审计及diff共16项检查通过，验证期间输入不变。本类664项相关Rust回归与严格全目标Clippy的范围保持，不冒充当前主仓最终全量Rust/Node/smoke集成。共享WS/Responses观察器已随Codex等定向回归核对，未改变registry/API或手改生成件。下一类仅推进Kiro CORE-03；其余类别与最终全量集成/review仍待完成，总目标继续进行。真实rail/operation仍live_pending，未做UI自动化或部署测试，未提交、推送或调用子代理。

## 2026-10-27 CORE-03 完成性复核：Kiro/AmazonQ EventStream

按Kiro导航只读核对kiro.rs@9066e9c4fe90c7e97d241900a6ce8eb52cc46903的已提交文件：

- `src/anthropic/stream.rs` SHA-256 `1c2168d5f67ab8ce8cbcc1e2b7a97c5bdafb6ba598f179e1e169e86abdc6c1b0`。
- `src/kiro/parser/frame.rs` SHA-256 `0c3a13d6da5d527dae0a9721a593a4d599572dd98794f9c5107b8f586ee1d035`。
- `src/token.rs` SHA-256 `b3d8c4bc99f4b08864fbc63c4f265f5195ef72b12488d2f3b3267935b5bfd860`。

均与工作树逐字节一致，未构建或执行外仓。参考仅支持初始/内容/metadata/最终事件区分；本仓在既有Value构造及完整EventStream边界独立观察。KiroOAuth/AmazonQOAuth加入有限静态标签，独立send/headers与原刷新attempt/wait接线；count_tokens保持本地，无虚构上游阶段。流式内容在builder规范化后观察，非流仅在既有完整缓冲解析后观察，不修改原first_token_ms、CRC/EOF合同、deadline、用量或健康。末owner仍由实际预算与Bytes析构报告，HTTP只记录body handoff。本轮398份冻结输入的fmt/include格式、check、636项相关Rust和严格全目标Clippy通过，14项相关审计通过；首次测试编译及第二轮Clippy夹具问题如实记录于计划§20.10.5。七文件差异已review，主仓增量同步待做；真实八路及Compact等门禁保持。

Kiro CORE-03 主仓同步与本类收口：7个源/文档文件按上一类收口基线增量应用，预检/应用无冲突；905个无关文件摘要不变，无无关新增。主仓398份构建输入逐份匹配已验证树；fmt/include单独格式、14项相关审计及diff共17项主仓检查全部通过，期间输入不变。本类636项定向Rust与严格全目标Clippy成绩保持原范围，当前主仓最终完整集成仍待完成。下一步仅推进Qoder；总目标继续进行。真实rail/operation继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。


## 2026-10-27 CORE-03 完成性复核：Qoder COSY

按Qoder导航，对TokenRouter先加载project-doc、完整读取docs/index.md及相关架构/接口导航与Qoder章节，再按code-comments核对输出观察和执行边界。只读核对TokenRouter@d15137eb59d4d85bdc27ae563c37fcf9b0edebcd：

- `docs/interfaces/qoder_upstream.md` SHA-256 `ff94767f2ee23c5cc2d45ce69d86c3bc8d8482ecc78dd659abd7efe7af4365fc`，工作树等于Git对象。
- `docs/architecture/gateway_request_lifecycle.md` SHA-256 `8de3e93680867c42d523fd990ae88f50e195391ad26aac13841b1372badb3850`，工作树等于Git对象。
- `backend/internal/upstream/qoder/output_observation.go` SHA-256 `396c53c5d8d7113713c569ba957d60e6578700ab45eabcc40cd9c0b40862db61`，工作树等于Git对象。
- `backend/internal/upstream/qoder/execute.go` SHA-256 `31f6bcbdbadbdb2bc7e423af0d23b65ad87858293c46c577c4171816b569c1d9`，工作树等于Git对象。

参考区分转换内容、终态与sink写入错误；它的流式取消后继续收集及跨Provider调度不迁入Server。目标独立接线既有COSY send/headers/read、原同账号认证恢复attempt/wait，在现有canonical Value观察文本、reasoning及实际工具字段；role/空块/usage/finish-only不记首内容。终态仅原唯一终态+EOF通过后观察，转换/发布错误仍记failed；HTTP只报告body handoff，不声称socket成功写入。398份输入冻结下fmt/include、check、661项相关Rust和严格全目标Clippy通过，15项审计通过；新回环path误注册及修正已如实记录，主仓增量同步待做。三rail真实验收及CN PAT候选门禁保持。外仓未修改、构建、测试或执行。

Qoder CORE-03 主仓同步与本类收口：7个源/文档文件按上一类收口基线增量应用，预检/应用无冲突；905个无关文件摘要不变，无无关新增。主仓398份构建输入逐份匹配已验证树；fmt/include单独格式、15项相关审计及diff共18项主仓检查全部通过，期间输入不变。本类661项定向Rust与严格全目标Clippy成绩保持原范围，当前主仓最终完整集成仍待完成。下一步仅推进CodeBuddy；总目标继续进行。真实rail/operation继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。


## 2026-10-27 CORE-03 完成性复核：CodeBuddy两站

按CodeBuddy导航只读核对cli2api@c7e4e73f64e661028a235e71306dba54ece48435，先读取其AGENTS与架构摘要；三份文件均与已提交Git对象一致：

- `internal/providers/workbuddy/ttfb.go` SHA-256 `429bf5bc9d42a770b088dcb073dc69e4a479cd387263d0c57eef88cfc2c3eb6a`。
- `internal/providers/workbuddy/ttfb_test.go` SHA-256 `c588d19b33d6298657c7b35ffd93fd279f54e9d993f065d96f96f15ba82d4111`。
- `internal/providers/workbuddy/sse.go` SHA-256 `39ab1fd31ea78b9c4f6f0455a85b33b7b75f81f9869c0993885d0d50c4f578a3`。

参考区分head等待与body生命周期，并清理空thinking/tool字段。本仓只吸收这些边界证据；继续以既有sanitize后的Chat Value观察文本/推理/实际工具，空字段、role和usage不记首内容。唯一DONE+EOF、错误锁存和原账号恢复比参考宽松聚合更严格，保持不变；没有迁入其timeout值、failover、扫描容错或遇DONE提前成功。独立send/headers/read与原三处认证恢复attempt/wait已接线，转换失败可记failed，HTTP仅body handoff，末owner与终态分开。398份冻结输入下fmt/include、check、665项相关Rust和严格全目标Clippy通过，16项审计通过；七文件差异review未发现待修问题，主仓增量同步及最终完整集成仍待做。两站live_pending，implementationCommit不提升；外仓未修改、构建、测试或执行。

CodeBuddy CORE-03 主仓同步与本类收口：7个源/文档文件按上一类收口基线增量应用，预检/应用无冲突；905个无关文件摘要不变，无无关新增。主仓398份构建输入逐份匹配已验证树；fmt/include单独格式、16项相关审计及diff共19项主仓检查全部通过，期间输入不变。本类665项定向Rust与严格全目标Clippy成绩保持原范围，当前主仓最终完整集成仍待完成。下一步仅推进最终主仓集成与整体review；总目标继续进行。真实rail/operation继续live_pending，未做UI自动化、部署测试、提交、推送或子代理调用。

## 2026-10-27 R4 当前主仓完整集成与最终review

最终验证范围为/data/projects/cc-switch-server当前未提交工作树，HEAD=075924cb3730eca0a300648a9d4c87e83c2c8dea。398份构建输入（332份Rust）已包含冻结时保留的并行Share Market修改，全部Rust步骤间摘要不变；后端审计连同脚本共冻结484份输入。include/fmt明确退出后依次check、完整cargo test、build、严格全目标Clippy全部通过：Rust4028通过/2忽略/零失败；后端Node1921/1921、另28项来源/合同/静态/脱敏检查均通过。本次成绩来自当前主仓，不以旧隔离树结果替代。八类reference-delta各自默认及--check-sources通过，仅证明已声明冻结来源范围，不等于外仓全HEAD审计。

当前主仓新构建二进制的本地后端smoke10/10通过；strip-debug副本与构建产物全部SHF_ALLOC节一致，临时listener/config/launcher清理完成，日志0600。CodeBuddy收口至全部验证结束，仓库文件清单和摘要未变。最终文档收口只更新本计划及本证据文件，不改Rust、fixture或构建输入。

CORE-03现已覆盖本轮八类声明的Provider白名单与协议入口，具体rail、语义首内容、retry wait、EOF、取消及末owner合同见增强计划§20.10～11。HTTP只记body handoff；WS本地writer完成与保守提交不同；语义终态、下游交付和最后实际owner释放不合并。原first_token_ms、usage、deadline、健康、固定绑定及恢复资格保持。新增观测不代表普通API-key路径、所有scratch/RSS或厂商成功socket写入已覆盖，也不产生新的性能提升或真实接受性结论。

原readiness以RUN_TESTS=0 RUN_REAL=0 RUN_DEPLOYMENT_TESTS=0、空EVIDENCE_FILE执行，实际退出1、not-ready。当前Router四处合同问题仍为locale-provider.tsx/market_fx.rs退役端点、models.rs缺Share Contract v8 reader constant、market_billing.rs缺保留Market能力；八项真实输入缺失，local-contracts-unverified和deployment-not-tested保持。未修改外仓或削弱门禁；本地独立全量通过不能改写readiness结果。

全部R4编号的实现/差分决定及风险回退已在增强计划§20.11逐项复核，离线交付完成；所有真实rail/site/operation继续live_pending，候选能力仍按原disabled/conditional门禁，implementationCommit=null。没有覆盖历史observation或伪造提交态，也未提交、推送、调用子代理、执行外仓代码、做UI自动化或部署测试。
