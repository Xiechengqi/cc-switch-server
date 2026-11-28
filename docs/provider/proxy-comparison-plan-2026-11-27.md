# 成熟反代源码对比与增强计划 R6

> 当前实施跟踪：[增强总账 §24](../../cc-switch-server-enhance.md#enhance-r6-implementation-20261127)。用户已授权逐类实施；下方“未实施”和 planned 均记录原研究交付时点，当前进度与验证结果以总账为准。

> 状态：**研究计划，未实施**。研究日期：2026-11-27。本轮仅修改计划文档，不修改产品代码、契约、生成资产或测试；不发起真实上游请求、不部署、不提交或推送。
>
> 目标基线：`11bfa2f134db1afc9e6da4626cbf6e693a82dc6a`，研究开始时工作区干净。本文中的源码位置、行为和缺口均指该基线；既有测试及真实验收记录属于历史证据，不是本轮执行结果。
>
> 本文是下一轮工作的依据，不取代 [架构总览](../architecture/overview.md)、Rust Provider Registry、[Provider 覆盖表](coverage.md) 或 [协议证据](../../PROTOCOL_EVIDENCE.md)。历史 R5 及其实施记录保留在 [增强总账](../../cc-switch-server-enhance.md) §21～22。

> 五类专项补充：[Claude / OpenAI / Grok / Antigravity / Buddy 深度对比 R6F](proxy-provider-focus-plan-2026-11-27.md)。同一目标基线继续追踪后，新增 Grok 递归清洗、Codex 最终握手与缓存身份、Buddy 目录世代、Gemini→Claude 响应 ID 四项静态确认缺口，以及六项条件协议研究和一项回归补证；均未实施。本文原十项编号保留，五类的具体边界和优先级以 R6F 补充为准。

## 1. 结论与研究范围

当前 Server 的主要问题已不是缺少基础反代功能。固定绑定、刷新并发控制、协议转换、严格终态、取消、用量结算、目录权限和内存所有权均已有大量实现。R5 之后又新增了 Zen 专用传输与实际 CLI 兼容、Kiro 原生 reasoning/工具增强、Codex 图片修复及启动历史流式恢复；不能重复立项，也不能把旧 session 中的结论直接套到当前代码。

本轮最有价值的增量按顺序为：

1. 修复 Zen 已支持推理字段在流式、聚合、首语义计时与续写判断中的不一致。
2. 修复 Trae 将显式关闭思考处理为“省略参数”的语义缺口；无受审关闭映射时明确拒绝。
3. 优先补证 Trae SOLO 实际返回方言。推荐 Hub 的行式解析、嵌入式 usage 和当前 Server 的具名 SSE 状态机存在实质差异；现有自造 fixture 不能解决这项协议冲突。
4. 再评估 SOLO 模型组路由、受限内联图片，以及 Qoder 新目录能力的接入。
5. 复用现有设施补齐 Qoder/Trae 的请求与实际控制值诊断、操作级验证证据，最后做小步结构拆分与 Zen 资源测量。

**OpenCode 官方项目是 Agent/客户端及其服务端运行时，不能整体当作通用推理反代。** 本轮主要读取其模型目录、请求/响应协议与传输边界。Zen 免 Key 通道按照 `OpenCode/AGENTS.md`，以 `zen-gate` 为唯一首选反代参考；官方 OpenCode 作为独立交叉证据。Console OAuth、付费 API Key、托管搜索、MCP/skills、会话执行器均不因官方仓库存在实现而进入 Server 范围。

研究覆盖推荐目录对应的订阅反代，并核对 Server 当前 29 个 ProviderType 的 Registry/requirements 边界。静态 API Key、Bedrock、Ollama、Copilot/Kimi 和隐藏 Web Profile 作为公共桥接边界交叉核对；没有对目录中全部非推荐项目重新排名，也没有宣称完成每个 Provider 的全部功能验证。

### 1.1 结论分类

| 分类 | 含义 | 后续动作 |
| --- | --- | --- |
| `confirmed_code_gap` | 当前本仓有可定位的分支不一致、语义丢失或诊断缺口；本轮为静态追踪，尚未执行失败用例 | 实施前先在正式路径复现，再作窄修复 |
| `conditional_protocol_gap` | 源码差异已确认，但参考通道、版本、权限或线上方言的适用性尚未证明 | 先补独立协议证据；不得直接开放能力 |
| `evidence_gap` | 能力已实现或存在可行设计，缺指定操作的可复核证据 | 补证，不重复实现 |
| `structural_gap` | 维护/资源可观测性方面的可测量弱点；不等于已发生线上故障 | 测量与冻结行为后分步处理 |
| `already_covered` / `not_adopted` | 已实现或违反本产品边界 | 保留回归或明确排除 |

“参考项目实现了”不等于官方保证，“参考项目称实测”不等于本项目真实回执；本文不按项目名、功能数量或测试数量给成熟度分数。

## 2. 推荐关系、提交快照和复现方式

已读取目标及适用的目录/项目 `AGENTS.md`。外仓全部只读，不拉取、不装依赖、不运行项目，不作为构建、测试、发布或运行时输入。以下路径以 `/data/projects/proxy/` 为根；共享符号链接按真实仓库去重。

| 研究对象 | 选择依据与主要用途 | 本地提交（完整 SHA） | 工作树 |
| --- | --- | --- | --- |
| `OpenCode/zen-gate` | OpenCode 目录首选；公开车道、指纹、目录、effort、恢复与流解码 | `58fa1b7bd98662696ff92777c3c7e4957058b111` | 干净 |
| `OpenCode/opencode` | 用户点名；官方模型 schema、Chat reasoning、传输抽象，辅助证据 | `cd0d037d68f389230c6452f846b3a43635cf2bf9` | 干净 |
| `CLIProxyAPI` | Claude/Codex/Antigravity 目录推荐；补充 Kimi、canonical thinking、WS | `47f18e4d82bd72f7d2b5ecf5ede93e4837a4625e` | 干净 |
| `Codex/codex2api` | Codex 目录推荐；Responses、WS、确定性输入失败与重放 | `c6aecea48d43f625dd4f7d6838f3fd8b3f3d0d47` | 干净 |
| `Antigravity/Antigravity-Manager` | Antigravity 目录推荐；pipeline、终态与预算 | `148838519e12b2f27aec227a4e789e9bacde2f49` | 干净 |
| `OmniRoute` | Cursor 目录首选；Agent h2 工具往返；补充 Copilot | `443d66996d69d7ecf887fd61158404a3cca1b192` | 22 项变化，排除 |
| `Grok/grok2api` | Grok 目录首选之一；CLI/Web 分界、reasoning 恢复与媒体 | `8535932b0783db2033657f36e2e1a84196c4e151` | 干净 |
| `sub2api` | Grok 目录推荐；模型级与账号级限流分离 | `ab99d56e9626e6cd731592dae8553c9758a0efa2` | 6 项变化，排除 |
| `Kiro/kiro.rs` | Kiro 目录首选；原生 reasoning、工具、API domain | `c37fe5afe7ee3adba836abfcee9a97aba5ea54f9` | 干净 |
| `Qoder/qoder2api-hub` | Qoder 目录首选；chat/app 目录、上下文、thinking intent | `2b0981c99166d681cdc8c9b38629c8c1b7306634` | 干净 |
| `CodeBuddy/workbuddy2api-hub` | Buddy 目录单账号能力首选；身份、转换、刷新及新 alias | `99fa0f5707554d2d7520307d3e03cbf9779cd878` | 干净 |
| `cli2api` | Buddy 目录通用网关/CLI 数据面参考；补充 Qoder/Trae | `ccd4744555060a9b78882f246263c7060d8cb6bb` | 2 个未跟踪文件，排除 |
| `CodeBuddy/workbuddy-switch` | Buddy 目录控制面参考；产品登录、现代套餐、官方用量扫描 | `22b318c1f44b335c2222e444abcc2d0f74e160f7` | 干净 |
| `Trae/trae2api-hub` | Trae 目录单账号首选；SOLO/raw 分流、新模型组、行式返回和图片 | `f7b75787680c7aded6b277aee05ddfc7fbac4ba8` | 干净 |
| `Ollama/ollama` | 官方 OpenAI 适配交叉证据，不能外推所有 Cloud 行为 | `b68b112bd8868d6278250d7d4bdfafa5cbf035c8` | 干净 |

复核结束时，上述 15 个 HEAD 与起始快照一致。OmniRoute 的变化主要涉及 Codex/价格/测试；sub2api 的变化涉及 Codex 模型；cli2api 的 `proxy.md`/`proxy.html` 未跟踪。无论是否与本轮主题相关，均不作为提交态协议证据。

复现应在对应仓库用 `git show <上述提交>:<下述相对文件>` 读取。源码行号均为该提交中的位置。下文外仓路径使用代码文本，避免文档链接审计、构建或测试依赖外部 checkout 存在。

## 3. 逐类对比：哪些应保留，哪些值得增强

| 类别 | 参考源码中的关键行为 | Server 当前路径与判断 |
| --- | --- | --- |
| OpenCode Zen | zen-gate `internal/lane/{upstream,catalog,effort,stream,http,client}.go`；官方 `packages/ai/src/protocols/openai-chat.ts`、`packages/core/src/{models-dev.ts,plugin/provider/opencode.ts}` | [Zen 主模块](../../src/proxy/opencode_zen.rs)、[catalog](../../src/proxy/opencode_zen/catalog.rs)、[专用转发](../../src/proxy/forwarder/opencode_zen_support.rs)：已超出参考的静态名单与旧 transport。保留 Bun、动态零价准入和严格终态；增强字段一致性及证据粒度，见 R6-ZEN-01/02、R6-EVID-01 |
| Claude OAuth | CLIProxyAPI `internal/runtime/executor/claude_executor.go:85` 区分 thinking 签名处理、tool ID 与专属规范化 | [Claude driver](../../src/proxy/providers/claude/mod.rs)、[claude_compat](../../src/proxy/claude_compat.rs)、[线程身份](../../src/proxy/claude_thread_profile.rs)：已有精确绑定、签名历史、恢复预算；11-24 空白无签名 thinking 修复已有记录。未确认需要重写的主协议缺口，保持差分证据 |
| Codex OAuth | CLIProxyAPI `internal/runtime/executor/codex_websockets_{executor,session}.go` 处理 WS 多轮/心跳；codex2api `proxy/{invalid_input_image,continuous_retry,responses_ws_interrupt}.go` 区分确定性失败与可恢复失败 | [Responses 传输](../../src/proxy/responses_transport.rs)、[语义检查](../../src/proxy/response_semantics.rs)、[容量恢复](../../src/proxy/openai_capacity_shed.rs) 及 [forwarder](../../src/proxy/forwarder.rs)：已有 WS 控制、活性与首语义分离、同绑定恢复和结果不确定性保护；11-26 图片与直连已修复。参考的 catch-all/换号重试不引入；无依据宣称 invalid-image 已在本仓被错误重放 |
| Antigravity / Agy | Manager `src-tauri/src/proxy/pipeline/{mod,terminal,budget,stream_lifecycle}.rs` 分离生命周期和终态；CLIProxyAPI 有专用 executor/translator | [wire](../../src/proxy/providers/antigravity/wire.rs)、[transport](../../src/proxy/antigravity_transport.rs)、[replay](../../src/proxy/antigravity_replay.rs)：已有 schema、签名、grounding、工具配对与双 rail 边界。借鉴职责拆分，不能将全部协议强行压入 Gemini IR，也不移植 prompt sanitizer/自动换号 |
| Cursor | OmniRoute 提交态 `open-sse/executors/cursor.ts:127` 处理内建工具请求，`open-sse/services/cursorSessionManager.ts:1` 保留 h2 流及 exec 映射 | [session](../../src/proxy/cursor/session.rs)、[response_state](../../src/proxy/cursor/response_state.rs)、[agent_driver](../../src/proxy/cursor/agent_driver.rs) 已区分 live h2、completed continuation、双 rail 和 running 冲突；严格验证声明工具。参考的冷恢复压平历史、并发冷开新 run 和提示词强迫工具不能作为增强 |
| Grok | grok2api `backend/internal/infra/provider/cli/responses_reasoning_recovery.go:85` 区分密文拒绝和 compaction；sub2api `backend/internal/service/grok_model_quota_block.go:10` 区分模型限流 | [Grok driver](../../src/proxy/providers/grok/mod.rs)、[grok](../../src/proxy/grok.rs)、[replay](../../src/proxy/grok_replay.rs)：已有受限恢复、媒体任务所有权和 Web/CLI 分界。参考的估算免费 Token 软门禁不能当作官方额度；不因缺新实测就重复建设 |
| Kiro / Amazon Q | kiro.rs `src/kiro/provider.rs:1100` 仅对明确历史签名错误恢复；`endpoint/{cli,ide,mod}.rs` 区分客户端和 API domain | [history](../../src/proxy/kiro/history.rs)、[structured](../../src/proxy/kiro/structured.rs)、[endpoint](../../src/proxy/kiro/endpoint.rs)、[domain](../../src/domain/providers/kiro.rs)：11-25 已吸收当前同一参考提交的主要差异，包含 reasoning、工具、域名、结构化输出等。R5 的图片最终预算也已完成；剩余是对应 rail 的真实证据，不能再次列为缺失 |
| Qoder | Hub `qoder_proxy.py:3860` 读取 `thinking_config`，`:3915` 按实际菜单规范化，`:7297` 构造 COSY；cli2api `internal/providers/qoder/direct.go:528` 使用独立 parameters 映射 | [runtime](../../src/proxy/qoder_runtime.rs)、[parameters](../../src/proxy/qoder_runtime/parameters.rs) 保留严格 chat 权限、签名、工具、目录代次和作用域。thinking 仍为 site/key 受审表，上下文与参考新版字段存在差异；先补版本/rail 证据，见 R6-QD-01。参数改写诊断见 R6-OBS-01 |
| CodeBuddy / WorkBuddy | Hub `wb_identity.py:472` 区分身份，`wb_accounts.py:2312` 锁内刷新并保存；cli2api `internal/providers/workbuddy/{catalog,mapping,client}.go` 读取模型档位；switch `official_usage.rs:403` 连续时间扫描绕过 3000 条截断 | [buddy_wire](../../src/proxy/buddy_wire.rs)、[reasoning 策略](../../src/proxy/codebuddy_runtime/reasoning.rs)、[OAuth/usage](../../src/clients/oauth/codebuddy.rs)：已实现双产品/双站点隔离、默认档位、轮换持久化、动态目录及官方扫描。**始终使用绑定目录默认推理是当前明确产品策略**，有 requested/effective 记录；参考尊重调用方档位不直接迁入。Hub 图片/视频属于候选产品扩展，不自动加入 |
| Trae | Hub `trae_proxy.py:139` 选择 SOLO function，`:6185` 校验内联图片，`:6399` 逐行读取，`:6429` 读取同帧 usage | [Trae](../../src/proxy/trae.rs)、[runtime](../../src/proxy/trae_runtime.rs) 已有三 rail、四入口、实时流/非流、工具往返和预算。SOLO 仍固定 function、拒绝图片、只认具名事件；这是本轮最大的兼容研究增量，见 R6-TR-01～04 |
| Copilot / Kimi / Ollama / 静态 Provider | OmniRoute `open-sse/executors/ghe-copilot.ts:28` 区分 api/proxy host；CLIProxyAPI Kimi executor；Ollama `openai/openai.go:544` 映射 effort | [Copilot 合同](copilot.md) 及 [账号域](../../src/domain/accounts/copilot.rs) 已有精确目录端点、发送前重核及默认不改正文；[Kimi schema](../../src/proxy/kimi/tool_schema.rs) 已有本地引用展开/资源边界；[Ollama](../../src/proxy/ollama.rs) 已有 effort 与终态合同。未发现需重新实施 R5 的依据 |

额外边界：当前 [Gemini CLI 合同](gemini-cli.md) 已明确退役旧 `gemini_cli`，保留只读识别，CLI 方向转为显式 Agy。R5 GM-01 只是历史证据，本计划不恢复旧登录、onboarding 或推理路径。SQLite、启动流式恢复及已有父内存账本也不再作为“缺失功能”。

## 4. 可执行增强条目

以下条目全部为 **planned**。优先级表示工作顺序；条件项的 P0 表示应优先研究，并不代表已经证明线上故障或获准开放新能力。

### R6-ZEN-01：推理字段识别与生命周期保持一致

**P0 · `confirmed_code_gap`。**

源码证据：

- zen-gate `internal/lane/stream.go:121` 选择第一个非空 reasoning 字段；官方 OpenCode `packages/ai/src/protocols/openai-chat.ts:1016` 同样逐字段跳过空串，识别 `reasoning_content` / `reasoning` / `reasoning_text`。
- 本仓 [公共流转换](../../src/proxy/stream_transforms.rs) `chat_reasoning_delta:5280` 包含 `reasoning_text`，但先 `find_map` 再过滤空串；较早字段为空字符串时，后续合法字段不会被选中。
- [Zen 聚合](../../src/proxy/opencode_zen/aggregate.rs) `chat:171` 未累计 `reasoning_text`；[Zen stream](../../src/proxy/opencode_zen/stream.rs) `event:239` 的 semantic 集合也没有该字段；[续写判断](../../src/proxy/opencode_zen.rs) `continuation_kind:1766` 只读取前两种拼写。
- [公共 timing](../../src/proxy/request_timing.rs) `observe_chat_value:729` 已识别 `reasoning_text`，而 [Zen forwarder](../../src/proxy/forwarder/opencode_zen_support.rs) `:641` / `:872` 使用 `decoder.semantic` 决定首语义 deadline。两个观察结果可能不一致。

直接影响：对包含合法 `reasoning_text` 的合成输入，源码路径会出现“流桥可读、非流聚合丢字段、首语义判断不同”的分歧；空的优先别名还可能遮住后续非空内容。**本轮没有声称当前 HY4/DeepSeek 真实流一定使用这些拼写。**

计划：为已支持的纯文本别名形成一套受限选择规则，供转换、Zen 聚合、语义识别与续写判断使用；保持原生签名/密文为 opaque，不把 `reasoning_details`、任意 `thinking` 或未知对象直接拼成文本。冲突字段的优先级、同内容去重和混合字段策略须明确。

验收：逐字段、空串前缀、同帧重复别名、跨帧分片、reasoning-only、与正文/工具混合，覆盖 Chat 上游到三个入口的流/非流；一致保留所支持字段，不重复 token/文本。终态后新增任何已识别语义均失败；role/usage/心跳仍不满足首语义 deadline。正式 forwarder 用例须先失败后通过，不能仅测试新 helper。

### R6-TR-01：显式关闭思考不能等同于省略

**P0 · `confirmed_code_gap`。**

[Trae source controls](../../src/proxy/trae/controls.rs) `:56` 将 Claude `thinking.type=disabled` 变成 `none`；[apply_reasoning_fields](../../src/proxy/trae.rs) `:542` 删除控制字段，`:564` 对 `none` 直接成功返回，不发送关闭标志。Chat/Responses/Gemini 的显式关闭经归一化也须分别追踪。当前“接受关闭请求”只证明字段被接受，不能证明上游默认思考被关闭。

参考价值是其控制语义处理方式，而非现成 Trae 关闭字段：cli2api `internal/providers/workbuddy/mapping.go:28` 明确区分 off switch 与档位，Qoder `direct.go:539` 显式编码关闭；这些其他产品字段**不能**直接用于 Trae。

计划：先补同一 Trae rail 的 off/on/omitted 对照证据。存在已审映射则显式编码；无法证明关闭能力的模型/rail，在发送前明确返回 unsupported control。不能靠移除返回的 reasoning 内容来假装关闭生成。省略请求保持现有默认语义。

验收：三 App、四入口，关闭/缺省/开启/冲突/坏类型各有正式发包断言；未知关闭能力零上游生成。任何允许的关闭路径都要能从最终出站 payload 与诊断中识别；不得通过 retry、更换模型或跨 rail 解决。

### R6-TR-02：优先补证并接入 SOLO 返回方言

**P0 研究 → P1 实现 · `conditional_protocol_gap`。依赖协议证据，不依赖新增模型或图片。**

参考 Hub `trae_proxy.py:6129` / `:6399` 逐 `data:` 行解 JSON，正文/工具/finish 可同帧存在，`:6429` 读取该对象顶层 usage。其 `test_issue_solo_groups.py:30` 的 fixture 没有按标准 SSE 用空行分隔每个 JSON，包含 `event:progress_notice`、连续 data JSON 和 `[DONE]`。源码及测试确认参考采用这套方言；它们不能单独证明厂商所有版本都返回同一格式。

本仓 [TraeSseDecoder](../../src/proxy/trae.rs) `:1046` 要求 SOLO data 具有事件名，`:1062` 的 `[DONE]` 特判只用于 raw，`:1110` 的 `usage` 别名也只用于 raw，SOLO 仅走 `token_usage` 和 `done`。[既有专项](../../src/proxy/trae/enhancement_tests.rs) 主要构造具名 SSE；没有覆盖上述推荐项目方言。

计划先取得同端点、同 function、同客户端身份的独立脱敏字节样本或官方协议证据。比较具名事件、data-only、每行 JSON、空行、多行 data、embedded usage 与 `[DONE]` 的实际关系。证据成立后，以 **Trae 专用且版本化的 decoder 方言**接入，复用 canonical 输出和现有生命周期；不放宽所有 Provider 的公共 SSE parser，不通过任意坏 JSON 自动切换解析模式。

验收：在完整 Server 路径重放独立样本，覆盖工具参数增量、正文与 usage 同帧、零/缺失 usage、UTF-8/网络任意分片、progress、厂商错误、晚到 usage。成功必须有该方言明确的合法终态和干净 EOF；缺终态、重复终态、终态后业务内容全部失败。仅有参考 Hub 的“有正文即返回”行为不算验收。

### R6-TR-03：模型目录与 SOLO function 成对准入

**P1 · `conditional_protocol_gap`。实现依赖 R6-TR-02。**

Hub `trae_proxy.py:118`～`:141` 根据模型组选 `solo_agent_remote` 或 `solo_work_lite`，并注明 `Doubao-Seed-Code` / `qwen3.8-flash` 的模型组限制；属于参考项目的实测声明。Server [domain 常量](../../src/domain/trae.rs) `:24`、[payload](../../src/proxy/trae.rs) `:73` 固定 `solo_work_lite`，[目录](../../src/proxy/trae_runtime.rs) `parse_trae_model_catalog:413` 读取 `get_detail_param/config_info_list`，未建模上述组信息。

计划：核对 `/api/remote/v1/models` 对当前绑定账号和身份的适用性，形成模型 → function → 目录版本的受审关系。不得复制 Hub 的静态两个模型白名单就开放。若两组实际上是不同权限/客户端 rail，先按 Registry/Profile 明确建模，不能在一次失败后自动切换；若证据支持同一 rail 的模型级 function，则固化到 admission snapshot、发送前 scope 检查及日志。

验收：旧 work 模型出站保持；agent-only 模型发向正确 function；未知组、重复冲突、无权限、目录撤权、token/设备变化均不发送。函数选择、实际模型及 usage 归属一致；4001 不触发试探式换组。没有真实账号证据时保持 `live_pending`。

### R6-TR-04：SOLO 受限内联图片

**P2 · `conditional_protocol_gap`。依赖 R6-TR-02，按模型组依赖 R6-TR-03。**

Hub `trae_proxy.py:6185` / `:6209` 已保留 user 消息的内联 PNG/JPEG/WebP/GIF，拒绝远端 URL；单图 5 MiB、合计 10 MiB。`test_issue_solo_images.py` 经过 HTTP 入口核对最终请求，这比 README 的 supportsVision 标记更有参考价值，但仍是 mock 传输证据。

Server [payload](../../src/proxy/trae.rs) `:59` 明确 `reject_images`，Registry/Share 也声明 SOLO images=false。这是可选能力差异，**不是当前代码漏转图片的缺陷**；显式拒绝比静默丢弃正确。

计划只研究 user 内联图片，先证明具体模型/function 的接受性与理解结果。沿本仓媒体规范化和请求预算实现 MIME/base64、每图/总量/数量校验，保留顺序；未证实的 tool-result/assistant 图片、远程下载、音视频保持拒绝。大小上限取经过测量且不超过既有 ingress/Router/请求内存上限的值，不机械复制参考常量。

验收：三 App/四入口转换前后图片哈希与文本顺序一致；完整请求已预约内存；超限、坏图、不可用模型零发包；同时更新 Registry/requirements 适用声明、覆盖生成源、Share 媒体授权和公开目录。离线通路通过不等于理解能力已实测。

### R6-QD-01：新版目录能力与已审 CLI wire 分开演进

**P1 · `conditional_protocol_gap`。独立于 Trae。**

Hub `qoder_proxy.py:3860` / `:3915` 读取 `thinking_config.enabled.efforts`；`qoder_catalog.py` 和 `model_context_override:8728` 使用 `context_config`。Server [capability 构造](../../src/proxy/qoder_runtime.rs) `:699` 的 effort 来自静态 site/key 表，`:1583` 只从已知字符串生成 directive；[context](../../src/proxy/qoder_runtime/parameters.rs) `:175` / `:203` 主要处理扁平 windows/default/max 字段。两者对新增 key、toggle-only、默认档位和嵌套窗口的表达能力不同。

但 Hub 的身份偏向桌面，cli2api `internal/providers/qoder/direct.go:530` 明确拒绝在 parameters 放 context switch，现有 Server 则依据冻结 CLI 观察发送 `context_length` 与 `ideModelConfigOverride`。**这是协议证据冲突，不应凭“最新仓库”覆盖既有 oracle。** 当前 high→max 等映射也已有本仓固定测试，不能未经补证改成同名直传。

计划为 Global OAuth / Global PAT / CN OAuth，按产品和 CLI 版本核对：实时目录字段、合法默认/允许档位、disable、预算与 effort 的区别、上下文窗口、最终字段位置。只将证据成立的字段纳入 typed capability；保留 chat 优先及 deny，app-only 继续 discovery-only。旧受审映射在兼容窗口保留，未知 capability 不自动授权。

验收：真实目录脱敏快照与独立 wire oracle 双证据；每种 rail 的空/撤权/冲突目录、toggle-only、未知 key、上下文窗口和新旧版本各有用例。未知或坏控制要明确拒绝/说明本地策略，不静默变成 unspecified；不得让 token budget 假冒 effort。纯元数据展示和生成权限分别验收。

### R6-OBS-01：补齐 Qoder/Trae 的参数适配诊断

**P1 · `confirmed_code_gap`（可观测性），不改变既有映射政策。**

本仓 [UsageLogContext](../../src/domain/usage/store.rs)、[Router 用量契约](../../src/domain/sharing/router_contract.rs) 已有 `requested_reasoning_effort` / `effective_reasoning_effort`。Zen [forwarder](../../src/proxy/forwarder/opencode_zen_support.rs) `:196`、Buddy `forwarder.rs:20871` / `:21062` 已填值。Qoder `apply_reasoning_directive:1674` 与 Trae `apply_reasoning_fields:535` 的结果没有通过各自正式 forwarder 同样填充；Qoder 的既有测试还明确验证 `gm51model` 请求 high 发出 max（`qoder_runtime.rs:2249` / `:2292`）。

参考 Hub `qoder_proxy.py:707`、Buddy Hub `wb_proxy.py:1273` 均区分请求/归一化档位，CLIProxyAPI `internal/thinking/apply.go:146` 起以 canonical 配置再作 Provider 编码。可借鉴“让决策可见”，不照搬它们的默认映射。

计划从最终 prepared payload 返回小型诊断结果，复用现有日志与同步字段；记录有限原因，例如 declared default、reviewed mapping、explicit disabled、unsupported。没有发送关闭标记时不能记为“已关闭”；上游 default/unknown 与具体档位区分。若需新增原因字段，明确做版本化可选扩展；第一步不要求更改 Router 或 Web UI。

验收：相同请求的流/非流、初次/认证恢复、未发送/已发送路径记录一致；对照最终 payload 验证而非再次推算。仅记录受限枚举，不拷贝未知原字符串、prompt 或工具参数；不把诊断变成新的授权/重试条件。

### R6-EVID-01：独立协议样本与细粒度验证状态

**P1 · `evidence_gap`。**

OpenCode `packages/ai/AGENTS.md` 与 provider 测试采用 recording/replay 分离。Server 已有大量 fixture、正式 forwarder loopback、reference-delta、real receipt schema 和 release gates，本项应扩展现有体系，不能再建设一套平行验收框架。

两个具体弱点：

- Trae 既有 fixture 主要按 Server 已假定的具名事件构造，推荐 Hub 却使用另一种返回方言。测试数量不解决样本独立性，R6-TR-02 是必须先补的反例。
- [conformance 数据](../../assets/contract/provider-conformance-evidence.json) 目前只有 driver × forward/test/discovery 的粗粒度层次，Zen 全为 fixture_verified/live_pending；[Zen 文档](opencode-zen.md) 和 [协议证据](../../PROTOCOL_EVIDENCE.md) 已记录特定模型、Bun profile、客户端版本及隔离 Router Share 的真实成功。粗门禁保持 pending 有其合理性，但无法机器化表达“部分操作已验、其余未验”。不能直接把整个 Zen driver 升级为 live_verified。

计划增加可选的细粒度证明视图：provider/rail、profile/transport、模型、入口与 stream、客户端版本、功能、生成次数、源/二进制摘要、验证日期和适用 scope。复用现有 receipt 规则，公开部分只保存无敏感字段的摘要/结论；真实原始凭据和请求仍留私有。旧文字回执缺必需输入时只作为 historical evidence，不补造 receipt。

验收：离线 fixture 与真实 receipt 不能互相提升；不匹配模型/rail/profile/版本的回执不授予能力；新旧 reader 兼容，粗粒度门禁保持保守；样本含已知失败、截断、重复终态和取消。CI 仍完全离线，绝不读取参考 checkout 或自动录制真实账号。

### R6-CORE-01：围绕既有生命周期做小步模块化

**P2 · `structural_gap`。在行为修复和独立用例稳定后进行。**

参考 OpenCode `packages/ai/src/route/executor.ts` 统一传输入口、CLIProxyAPI executor/translator/thinking 分工、Manager pipeline 生命周期的职责划分；不迁移它们的账号池、宽松终态或统一 Gemini 数据模型。

目标已经有 [execution](../../src/proxy/execution/mod.rs) 中的 BindingSnapshot / AttemptBudget / CommitGuard、专属 Provider facade 和 shared memory，不能再把“建立公共预算”列为待办。当前弱点是这些机制仍由大型 forwarder 组合：`forwarder.rs` 61,827 行，其中末尾主测试模块从 33,542 行开始；`state.rs` 39,000 行，末尾主测试模块从 24,730 行开始。这里的总行数包括测试，不能当作生产热路径行数。Zen/Trae 等还通过 `include!` 共享词法作用域，隐藏了实际模块依赖。

计划先选择 Zen 或 Trae 一个 driver，将 request plan、prepared upstream、decoder lifecycle 与 settlement 的依赖显式化为 Rust 模块接口；一次只移动一个职责，冻结 wire 与用量结果。state 只按域方法下沉协调，不改变写入/锁序。此项目标是缩小变更面与可审计边界，不以文件越短或新 trait 越多作为完成标准。

验收：独立模块的生产入口被真实调用；迁移前后同一 corpus 的 payload、headers、终态、attempt/usage、取消与 retained owner 一致。无新增 domain→proxy 或 proxy→clients/api 依赖；无 writer 绕过 state。历史 Zen debug 大栈修复不能作为当前 release 栈故障的证据；若主张性能/栈收益，必须重新测量。

### R6-ZEN-02：运行时资源实测

**P2 · `structural_gap` / `evidence_gap`。先测量，再决定是否需要实现调整。**

[transport](../../src/proxy/opencode_zen/transport.rs) `:141` 每次请求启动校验后的 Bun，[fetch helper](../../src/proxy/opencode_zen/fetch.mjs) 为请求体分配独立 Buffer；[lane](../../src/proxy/opencode_zen/lane.rs) `:253` 已限并发及队列，[请求预算](../../src/proxy/forwarder/opencode_zen_support.rs) `:228` 已预约 payload 空间，取消也会 kill 子进程。这里不存在“无限进程/完全未计请求体”的证据。

但是 [父内存账本](../../src/infra/resident_memory.rs) 开头明确只测 logical retained bytes，不测 allocator/transport/RSS；当前没有由该链路给出子进程运行时基线、峰值 RSS、启动开销和 cgroup 压力证据。计划用离线受控上游测量冷/热、并发 1/4/上限、慢下游、取消、最大允许正文和内存紧张场景；再决定是否需要独立 runtime reservation、压力准入或参数建议。不预先改常驻池、不以未测量数字承诺优化，不替换已验证的 Bun TLS/fetch。

验收：资源实验能关联 Rust 与全部子进程生命周期，取消后最终无存活 child，队列/启动/传输时间分开，给出测量方法与误差。任何进程复用方案须另行证明会话、请求与代理凭据隔离；无证据时保留当前每请求子进程。

## 5. 不采纳项和当前应保持的优势

Zen 视觉能力保留为可选研究，不纳入上述 10 项实施承诺。zen-gate `catalog.go:73` 起含声称探测过的模型能力，官方 OpenCode 提交态 catalog 也含视觉声明；Server [effective capabilities](../../src/proxy/opencode_zen/catalog.rs) `:116` 明确 images=false，属于有意收窄范围。若未来有需求，先针对一个精确免费模型/profile 补独立图片理解证据，再走媒体授权和预算，不能按模型名或目录声明全量开启。

- zen-gate 的出口轮换、模型替换、静态 free 名称兜底和预算型 light/balanced/deep 不迁移；后者不等于原生 reasoning effort。公开限流继续按现有作用域执行，不通过重铸 session 绕过。
- Hub `wb_proxy.py:20243` 默认 finish 为 stop，纯文本缺 DONE 仍可能走聚合返回；Trae `trae_client.py:935` 可按有无正文补 stop/error；Qoder/其他网关也存在跳过坏帧或补工具身份行为。均不能替代 Server 的唯一终态 + 干净 EOF。
- 不迁入身份/WAF 字串清洗、任意系统提示词替换、普通文字猜工具、自动删历史或模型强迫调用工具。必要协议兼容须保留明确作用域，不能借“统一 pipeline”扩展正文改写。
- CodeBuddy / WorkBuddy 的默认推理、身份/站点隔离、轮换持久化和官方用量扫描已完成；不改变既有默认政策，不复制其他生成身份的 Header 拼盘。
- 固定账号/Provider/产品/站点/rail；无账号池、跨账号/跨站点/跨 Provider fallback。模型组选择也不能成为失败后换 rail 的暗门。
- 不恢复 Gemini CLI，不引入桌面配置写入、MCP/skills/session manager、市场资金系统或新的第四 App。
- 外仓的无连接后超时、自动续写、soft quota、全文日志等政策不覆盖 Server 已有总 deadline、首语义/idle、结果不确定性保护和脱敏边界。

## 6. 分阶段顺序、依赖与退出条件

| 阶段 | 工作 | 产物和退出条件 |
| --- | --- | --- |
| A：复现与补证 | 先 R6-ZEN-01、R6-TR-01 的正式路径反例；同时研究 R6-TR-02 的独立线格式 | 本仓明确失败用例；SOLO 方言得出 accepted/rejected/仍待证结论。参考测试通过不能代替本仓反例 |
| B：窄修复 | ZEN-01；TR-01 的受审编码或明确拒绝；OBS-01 | 单个条目独立 review，控制字段、首语义、用量和取消回归通过，不引入能力扩张 |
| C：条件兼容 | 有证据才实施 TR-02；之后 TR-03/TR-04；QD-01 独立推进 | 每项更新协议证据并核对 coverage 生成源。未获得授权/协议依据的能力明确保留 blocked-by-evidence，不用静态回退绕过 |
| D：证据收口 | EVID-01，按实际支持的模型/协议/客户端导入或新采集合法 receipt | 部分实测结果可被精确表达；driver 的其余 gate 不被提升；真实输入不足仍 pending |
| E：结构与测量 | CORE-01、ZEN-02 资源实验 | 行为不变的单职责迁移；资源数据支持后续决策。无需为凑阶段修改其他已成熟 driver |

TR-02 与 QD-01 研究无结果时，可继续完成独立的 A/B 项；不把整轮工作绑到真实凭据。任何条件项发现产品/rail 不适用，应记录“不采纳/保持显式拒绝”，这也是有效的研究退出结果，不能宣称该能力已实施。

### 6.1 每个实施条目的共同约束

1. 先核对当时主仓 HEAD 与并行修改，只重查受影响差异；本章不是长期有效的源码快照。
2. 在改 Provider wire 或能力前核对/更新 `PROTOCOL_EVIDENCE.md` 和 `docs/provider/coverage.md` 的生成源；不得手改生成文件、哈希或绕过现有 evidence gate。
3. 状态写入经 state 域方法，保持锁序；保持 Registry 身份/Profile/driver 权威。外仓只在明确研究中读，不成为 CI 的隐含目录依赖。
4. 每个正式路径反例至少核对：最终上游 payload、未发送/已发送边界、首语义、终态/EOF、取消、attempt 次数、token/USD 分项和最后 owner 释放。结构性变更再覆盖撤权/refresh 身份代次。
5. 本地验证通过与真实账号可用分开记录；没有真实输入不能将 gate 升为 `live_verified`，不顺带部署或执行 UI 自动化。

### 6.2 风险与回退边界

| 变更 | 主要风险 | 实施与回退要求 |
| --- | --- | --- |
| ZEN-01 共享字段识别 | 改一个 alias helper 影响其他 Chat 桥，或同帧重复推理 | 冻结所有实际消费该 helper 的路径；只统一已支持文本别名，不统一密文。单独可回退，不触碰 Bun/profile |
| TR-01 关闭语义 | 过去被接受的 unsupported 请求改为本地错误，影响依赖旧宽松行为的客户端 | 文档与错误给出明确控制名和替代的“省略/默认”方式；不得以悄悄忽略为兼容回退 |
| TR-02/03/QD-01 协议演进 | 错认方言、function 权限或新旧 CLI 字段，改变既有可用模型 | 先有独立样本；保留已审旧版本行为，准入快照固定版本。新能力可停用，不能在失败后隐式换方言/模型/rail |
| TR-04 图片 | 多份编码膨胀、Share 权限与真实模型能力不一致 | 所有输入在发送前完成媒体授权和预算；上线前仍可保持 images=false，不为通过测试放开全模型 |
| OBS-01/EVID-01 | 诊断泄露或局部回执被误当全局验证 | 复用有限标签和脱敏契约；旧 reader 可忽略新增摘要，粗门禁保守。删除新增视图不能改变推理权限 |
| CORE-01/ZEN-02 | 生命周期移动导致提早释放/重复结算；未测就增加常驻进程复杂性 | 先一条 driver、一个职责；以相同 corpus 比较行为。资源实验无收益证据则结束研究，保留当前进程模型 |

任何已发送或输出已提交的请求均不得因回退决策自动重放。实际发布及回退仅在后续明确实施范围内安排，本轮不执行。

## 7. 后续验证矩阵与本轮实际验证

| 维度 | 必须覆盖的差异 |
| --- | --- |
| 入口/模式 | Messages、Chat、Responses；支持的其他 Provider 再含 Gemini；stream false/true |
| 纯文本/推理 | 空/缺省/不同别名/多别名、reasoning-only、明确关闭与默认、合法及未知 effort |
| 工具 | 无工具、普通 function、namespace/Code Mode、分片名称与参数、声明→调用→回执→最终回答、并行/强制选择 |
| 终态/传输 | role/usage/progress/心跳、首语义超时、idle、错误、截断、重复终态、终态后数据、干净 EOF |
| 身份/权限 | 同账号同 rail、目录版本/撤权、登录与 token generation、Share/用户隔离、兼容失败零换绑 |
| 资源/计量 | 大正文、事件/聚合上限、慢下游、未读响应取消、子进程退出、重试/续写单独 attempt、未知 usage 不补零 |

未来有代码变更时，按根 `AGENTS.md` 完成格式（含 include 源）、cargo check/test、相关正式路径测试、严格审计、生成 coverage/UI matrix、Phase 0 writer 证据及本地 smoke；真实和部署验证保留单独授权与输入门槛。无需为纯计划改动运行整套产品测试。

**本轮实际执行：**目录指令读取、提交/工作树检查、推荐源码和本仓正式路径/现有 fixture/契约的静态追踪、关键参考文件摘要核对。文档索引/链接审计通过，Provider coverage `--check` 通过，conformance 审计通过（11 个 governed drivers，粗粒度 live gate 均仍 pending），diff 空白检查通过。最终文档修改后再次核对链接、空白和变更范围。没有运行 Rust/前端测试、启动 Server、调用真实上游或重新执行历史验收，不能引用旧 R5/Zen/Kiro 的测试数量作为本轮成绩。

### 7.1 关键外部证据文件摘要

以下为上述固定提交中的文件 SHA-256，便于后续研究复核；不要求构建/测试读取外仓。

| 仓库 / 文件 | SHA-256 |
| --- | --- |
| zen-gate `internal/lane/stream.go` | `fb88a3ba50fc5cbf9d22777c57b00c4056ca044dbcb45fd30735f4fe9a21b957` |
| OpenCode `packages/ai/src/protocols/openai-chat.ts` | `7844a969c3ccb9bf2542333c4ae463711ca2f6beeca2267271225f3ec633ce7b` |
| trae2api-hub `trae_proxy.py` | `4f549a2c47c15dbb2881cfe9616dd832fb48eb8b6e8e86bff890e414eab26cc7` |
| qoder2api-hub `qoder_proxy.py` | `84a0429448e4e079989c6122769a48ffd7254835519b883df3ff780a1fd5a3bd` |
| cli2api `internal/providers/qoder/direct.go` | `6210b7452725cf85ecbf81b19b0e3a79314e81e848f8446952c0a318bb647dbe` |
