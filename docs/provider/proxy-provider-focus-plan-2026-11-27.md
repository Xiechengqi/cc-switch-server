# 五类供应商反代深度对比与增强计划 R6F

> 当前实施跟踪：[增强总账 §24](../../cc-switch-server-enhance.md#enhance-r6-implementation-20261127)。用户已授权逐类实施；下方“未实施”和 planned 均记录原研究交付时点，当前进度与验证结果以总账为准。

> 状态：**研究计划，全部 planned，未实施**。日期：2026-11-27。承接 [R6 总体研究](proxy-comparison-plan-2026-11-27.md)，聚焦 Claude、OpenAI/Codex、Grok、Antigravity/Agy、CodeBuddy/WorkBuddy。
>
> 目标基线：`11bfa2f134db1afc9e6da4626cbf6e693a82dc6a`。开始本次聚焦研究时，工作区已有上一轮三个计划文档的修改，均保留。本次仅追加研究文档和入口，不修改代码、测试、Registry、协议证据或生成资产；不运行参考项目、不访问真实上游、不部署、不 commit/push。
>
> 下文 `confirmed_code_gap` 表示已沿当前代码定位的边界缺口，**本次没有执行产品失败用例**；例子是后续应实现的独立合成反例。`conditional_protocol_gap` 表示适用通道、版本或厂商语义仍须取证。既有 fixture/真实回执不计为本次执行成绩。当前实现权威仍为 [架构总览](../architecture/overview.md)、Rust Provider Registry、[覆盖表](coverage.md) 和 [协议证据](../../PROTOCOL_EVIDENCE.md)。

## 1. 聚焦结论与建议顺序

最值得先做的三项是：**Grok 清洗限定到协议路径、Codex 按最终握手身份复用连接、Buddy 为准备好的目录增加撤权世代围栏**。它们分别保护请求语义、连接协商一致性和目录准入。随后处理 Gemini→Claude 的合成响应 ID。Claude 的基础签名/CCH、Codex interrupt、Buddy 默认推理及官方用量扫描均已有实现，不重复建设。

| 建议顺序 | 条目 | 优先级 / 分类 | 当前判定 |
| --- | --- | --- | --- |
| 1 | R6F-GR-01：Grok 按协议路径清洗 | P1 / `confirmed_code_gap` | 三个全树递归函数会触及 schema/业务值；`encrypted_content` 还可沿数组过滤删除整项工具 |
| 2 | R6F-OA-01：Codex Responses WS 连接缓存的身份一致 | P1 / `confirmed_code_gap` | key 在合入客户端协商头及 metadata 清洗前生成；缓存取得时未再比较最终握手身份 |
| 3 | R6F-BD-01：Buddy 目录替换与撤权围栏 | P1 / `confirmed_code_gap` | 发包复核覆盖身份/token，未覆盖 prepared catalog 的同 scope 替换或撤销 |
| 4 | R6F-AG-01：缺失/空响应 ID 的唯一稳定回退 | P2 / `confirmed_code_gap` | Gemini→Claude 流/非流存在固定 `gemini` 回退，空字符串也会直接进入结果 |
| 5 | R6F-CL-01：Claude Advisor 密文历史归属 | P2 / `conditional_protocol_gap` | beta 和 server tool 已支持；未找到独立 Advisor owner 记录，跨凭据可移植性尚未证明 |
| 6 | R6F-AG-02：工具参数续帧与缺名保护 | P2 / `conditional_protocol_gap` | args-only、无 ID/name 续帧会落入 `tool` 默认名；是否允许该 wire 形状须独立取证 |
| 7 | R6F-BD-02：Buddy 分层请求身份 | P2 / `conditional_protocol_gap` | 五个会话/请求头共用一次物理请求 ID，与新版参考不同；旧受审 wire 仍是权威 |
| 8 | R6F-GR-02：Grok 客户端身份整体复核 | P2 / `conditional_protocol_gap` | 参考默认 shell 1.0.60、Build 主端点与目标生成端点不同；不能只更新版本字符串 |
| 9 | R6F-GR-03：Grok 模型限流与账号限流分开 | P2 / `conditional_protocol_gap` | 当前 Grok 429 进入通用账号限流；只有同 rail 的明确模型级证据才可缩小作用域 |
| 10 | R6F-OA-02：Codex 响应裁剪协商 | P2 / `conditional_protocol_gap` | 参考新增 `X-Codex-Exclude`，目标未接入；先核对对 usage、响应桥接和计量的影响 |
| 11 | R6F-OA-03：Codex 传输/错误反例补证 | P2 / `evidence_gap` | 活性、interrupt、容量错误已有；补新参考的组合反例，不预设当前错误重放坏图 |

上述四项确认缺口仍须先在正式路径复现。条件项可以以“当前边界正确、继续保留”为结论关闭，无须为了追平参考功能数而实施。P1 表示本轮优先级，不等于已证明线上故障或安全事件。R6F 不替换 R6 的十个条目；若安排同一实施批次，先解决本节 P1，再与 R6-ZEN-01 / TR-01 的语义修复一起排期，证据受阻的能力扩展后置。

## 2. 研究对象与边界

已按各目录 `AGENTS.md` 选择推荐项目。下表路径以 `/data/projects/proxy` 为根，符号链接按真实仓去重。研究仅使用本地提交，不拉取远端；sub2api 的六项未提交 Codex 修改、cli2api 的两个未跟踪文件均排除。结束时须复核 HEAD 与本表一致。

| 参考仓 | 完整提交 | 本轮主要用途 | 工作树 |
| --- | --- | --- | --- |
| `CLIProxyAPI` | `47f18e4d82bd72f7d2b5ecf5ede93e4837a4625e` | Claude Advisor/签名边界、Codex WS、Antigravity→Claude | 干净 |
| `Codex/codex2api` | `c6aecea48d43f625dd4f7d6838f3fd8b3f3d0d47` | 协商头与 WS 复用、确定性输入失败 | 干净 |
| `Grok/grok2api` | `8535932b0783db2033657f36e2e1a84196c4e151` | typed 清洗、Build 身份、reasoning 恢复、语义 idle | 干净 |
| `sub2api` | `ab99d56e9626e6cd731592dae8553c9758a0efa2` | 模型级与账号级限流分离 | 6 项变化，仅读提交态 |
| `Antigravity/Antigravity-Manager` | `148838519e12b2f27aec227a4e789e9bacde2f49` | pipeline/终态/预算设计及不采纳边界 | 干净 |
| `CodeBuddy/workbuddy2api-hub` | `99fa0f5707554d2d7520307d3e03cbf9779cd878` | 单账号 CLI 身份、会话/turn/物理请求分层 | 干净 |
| `cli2api` | `ccd4744555060a9b78882f246263c7060d8cb6bb` | CLI 数据面、会话 ID、目录/工具/推理 | `proxy.md`、`proxy.html` 未跟踪，排除 |
| `CodeBuddy/workbuddy-switch` | `22b318c1f44b335c2222e444abcc2d0f74e160f7` | 原生登录、套餐/官方用量控制面 | 干净 |

外仓文件在本文中只用代码路径/提交描述；后续可用 `git show <提交>:<文件>` 复核，**不能成为构建、CI、测试或运行时输入**。本仓链接使用相对路径。没有检查远端 URL、导出账号数据或记录私人请求。

实际产品范围必须区分：

- Claude：`claude_oauth` 与 API key / bearer relay 是不同 Profile；官方 OAuth 的特定修复不能无条件套到兼容服务。
- OpenAI：`codex_oauth` 是 ChatGPT/Codex 协议，普通 `codex` 的 OpenAI-compatible Chat/Responses 不等于同一后端。图片独立端点、Responses 图片工具、compact、models、alpha search 与 WS 各自有合同；不得用聊天成功替代。
- Grok：OAuth 推理、CLI 目录/用量、direct-XAI 媒体和隐藏 Web SSO 不是同一 rail。目标生成固定 `api.x.ai/v1`；参考 Build 默认 `cli-chat-proxy.grok.com/v1`，并有自己的 fallback 规则。
- Antigravity：`antigravity_oauth` 和 `agy_oauth` 是两条独立绑定；当前登记 Claude/Gemini App。共享转换代码存在 OpenAI bridge 不表示新增获准 App Surface。旧 Gemini CLI 已退役。
- Buddy：CodeBuddy/WorkBuddy 是两种产品，各自有 CN/Intl 站点；原生登录身份与生成所用 CLI 身份分别审核。目录默认推理是当前明确产品政策。

## 3. 逐供应商分析

### 3.1 Claude：基础 wire 已较完整，新增重点是有凭据归属的 opaque 历史

推荐 CLIProxyAPI 的 `internal/runtime/executor/claude_executor.go` 将官方签名清理、server tools、工具 ID 和最终发送分层；最新 `91a3f455e` 增加 `helps/claude_advisor_state.go`，专门记录 Advisor state 的 credential owner。不能把它的全部清理策略作为正确性标准。

| 能力 | Server 当前实现 | 对比判断 |
| --- | --- | --- |
| 原生/第三方识别、beta、工具别名、严格 schema、forced tool choice | [claude_oauth.rs](../../src/proxy/claude_oauth.rs) `:200–365`、`:767`，及 [claude_compat.rs](../../src/proxy/claude_compat.rs) | 已覆盖。原生历史也经过窄范围空白 thinking 修复；最终 CCH 在所有正文改写之后计算 |
| 签名历史与 Thread | [Thread profile](../../src/proxy/claude_thread_profile.rs) 及 [Claude 合同](claude-oauth.md) | 已有绑定/模型/调用者/会话隔离与受限持久化，不应另建通用会话数据库 |
| unsigned blank thinking | [协议证据](../../PROTOCOL_EVIDENCE.md) 的 2026-11-24 条目 | 已实施，保留签名和业务 JSON 不透明性；不是本轮缺口 |
| Advisor 支持 | `claude_oauth.rs:2430` 精确识别 `advisor_20260301`，`:4533`、`:4641` 有 beta/CountTokens 测试 | 不能称完全不支持 Advisor；待研究的是返回历史的凭据归属及重新绑定行为 |
| 未知流事件 | [anthropic_semantics.rs](../../src/proxy/anthropic_semantics.rs) `:368` 在 message_start 后作为 Business | 前向兼容与提交边界取舍。未取得合法新控制事件样本，暂不立“未知事件必须忽略”的修复项 |

Advisor 参考实现只识别特定消息层级中的 `server_tool_use(name=advisor)`、`advisor_tool_result`、`advisor_redacted_result` 及特定 `tool_result.content`。owner 缓存最多 4096 项、24h；**已知 foreign 才删除，unknown 首次不删，待上游接受后登记**。它服务于可换凭据的网关；Server 固定绑定仍可能遇到显式重新绑定或客户端携带旧历史，但这不证明厂商一定将 Advisor 密文绑定账号，也不证明应该自动删块。

R6F-CL-01 应先给出 accepted / known-foreign / unknown 三种状态的证据与行为设计。若需新增归属信息，仅记录受作用域约束的块 ID 摘要和身份代次，不存密文/正文、不解码签名、不在未知来源时猜 owner。Thread 省略历史路径应复用现有隔离；客户端完整历史则要独立证明，不能宣称 Thread 已保护所有 opaque 块。

跨 Responses 的 hosted `tool_search` 当前被 [adapters.rs](../../src/proxy/adapters.rs) `:7363` 的正式边界测试拒绝。参考支持这一功能属于产品扩展，不能借 Claude 修复悄悄启用。

### 3.2 OpenAI / Codex：主要新缺口在握手准备顺序，不在 interrupt 或基础 WS

CLIProxyAPI 的 `46c3467a2` 在 active turn 的 Ping/Pong 上续 liveness deadline，并用并发 control write 避免长 payload 挤压 Pong。codex2api 的 `8ecd1860` 将 `X-Codex-Exclude` 纳入 client identity 和 continuation 匹配；`253fc478` 禁止确定性坏图在 catch-all 下继续重放。

Server 已有 [Responses 解码](../../src/proxy/responses_transport.rs) 的严格活性 allowlist、[容量分类](../../src/proxy/openai_capacity_shed.rs)、[interrupt 状态机](../../src/proxy/providers/codex/control.rs)，以及提交后结果不确定性保护。`forwarder.rs:607` 的大 payload 分片写入让 reader 消费 Ping；`:54149` 有双向 Ping lane 回归。CLI 版本也有 [codex_identity.rs](../../src/codex_identity.rs) 的受限后台 release 同步和缓存，不能仅按默认常量 0.169.0 判断版本停滞。

**术语与生效范围：**此项指 Codex Responses 专用的上游 WebSocket 连接缓存。`forwarder.rs:638` 的 `CachedResponsesWebSocket` 保存实际 socket；`:681` 的类型虽名为 `ResponsesWebSocketPool`，其作用是按 key 暂存闲置 socket，供后续下游 WS 连接复用。`:15947` 仅在没有进行中的响应、没有待确认 interrupt 时归还，`:16025` 从缓存取出。当前源码默认最多缓存 64 个连接、闲置 5 分钟、总寿命 55 分钟；`CC_SWITCH_CODEX_WS_CACHE_MAX_CONNECTIONS=0` 禁止留存。`:13690` 只为 Codex mode 且存在 session 的目标生成缓存 key。这里没有研究运行进程的实际配置；该缺口的缓存复用影响以相应路径和留存条件成立为前提，不能泛指所有 OpenAI HTTP 请求、通用 HTTP 连接管理或账号调度。

**R6F-OA-01 的完整静态链：**

1. [forwarder.rs](../../src/proxy/forwarder.rs) `:13659–13700` 的 `prepare_responses_websocket_target` 构建账号/身份头，并在 `:13690` 计算 pool key。
2. 正式入口 `:7584–7604` 在返回后才合入客户端 allowlist 头、清洗 turn metadata 和删除 routing hint。
3. `:13730` 的 key 函数本身会 hash 传入的所有头；问题是**调用时尚不是最终头集合**。
4. `:14424–14439` 的重建路径、`:16495–16515` 的 401 refresh 路径也在取得 key 后才合并客户端头。
5. `:16025` 按 key 取缓存；`CachedResponsesWebSocket` 不另存最终协商头摘要供复核。`:57405` 的既有 key 测试只直接传入凭据/workspace 差异，不能覆盖正式调用顺序。

因此，同一绑定、session、账号/版本下，客户端 `x-codex-beta-features` 从 A 变 B 或从有值变省略，key 仍可能相同；命中的物理连接继续使用建立时的协商值。这是静态确认的 prepared-target 一致性缺口，**不是已证明跨账号串用**，也不能归咎于 hash 算法没有覆盖头。

实施设计应把“最终 handshake 语义快照”作为连接身份来源。哪些头是连接级、哪些只是逐轮诊断，须显式分类；不能为了回归通过把所有高频 request ID 都纳入 key 导致复用退化。相同协商可复用，改变协商必须隔离；带 `previous_response_id` 的续接遇原连接身份不匹配应报受控冲突，不能新建连接偷偷重放历史。

`X-Codex-Exclude` 尚不在目标 `CODEX_ALLOWED_CLIENT_REQUEST_HEADERS:29924`，属于 R6F-OA-02 的条件能力。参考例子是 `tools,usage.attribution`，不等于可以允许客户端裁掉计量必需信息。若接入，应同时覆盖 HTTP、WS、401、WS→HTTP，先解决 OA-01，且保持代理管理头的优先级。

新坏图规则则先列 OA-03 证据补充：目前没有定位目标对该错误重放的代码事实；不能因为参考刚修复就宣称目标也存在同一 bug。11-26 图片/直连已有修复，继续保留。

### 3.3 Grok：请求树清洗越界最明确；新版 Build wire 不能混入现有 XAI rail

grok2api 的 `responses_reasoning_recovery.go:315` 仅在已识别的 `input[]`、`type=reasoning` 上操作密文；测试专门保留 compaction 与非 reasoning 的同名字段。其 `responses_tool_types.go:93` 在 web-search 声明层处理 `external_web_access`，`false` 不被当作省略。Server 不应照搬其将 readable reasoning 改成普通 assistant 文本的有损恢复，但**路径与类型限制值得吸收**。

目标 [grok.rs](../../src/proxy/grok.rs) 的正式准备链为 `apply_forward_contract_with_reasoning_capability:929` → `patch_grok_request_body_with_reasoning_capability:1226` → `patch_grok_request_value:1273`；HTTP 与 WS 准备均使用。前面的 tool/protocol normalization 保留普通 function 的 parameters，并不能挡住后面的递归。

| 清洗函数 | 当前范围 | 可静态推导的反例 |
| --- | --- | --- |
| `remove_recursive:2432` | 在整个请求树删除 `external_web_access` | 同名 schema property 被删除，`required` 字符串仍在；约束变得不一致 |
| `strip_server_item_ids:2052` | `store=false` 时递归移除所有形如 `rs_` / `fc_` / `resp_` / `msg_` 的 `id` | 工具参数 schema 的 `default` / `examples` 中合法业务 ID 也被删除；不能只按值前缀认定是协议 item ID |
| `strip_invalid_encrypted_content_inner:2453` | 检查任何 object 的同名字段；错误标记向父 object 传播，array 用 `retain_mut` 丢弃标记项 | schema 中 `properties.encrypted_content` 是 object，判为非法密文；错误沿父 object 传播到 `tools[]`，整项工具被删 |

后续第一个正式路径用例应使用普通 function，避免未知 input 类型等其他校验提前挡住反例：

```json
{
  "model": "grok-4.6",
  "input": "use the declared tool",
  "tools": [{
    "type": "function", "name": "save_record",
    "parameters": {
      "type": "object",
      "properties": {"encrypted_content": {"type": "string"}},
      "required": ["encrypted_content"], "additionalProperties": false
    }
  }],
  "tool_choice": {"type": "function", "name": "save_record"}
}
```

静态预期：现有清洗会使 `tools` 变空；清理发生在 `sanitize_tools` 之后，不能依赖它再次修正 `tool_choice`。这里尚未执行上游抓包或本仓测试。另做独立 schema default 用例 `{"id":"msg_invoice_42"}`、`external_web_access` property 用例；不要用最终会整体丢弃的 metadata 作为保真证明，也不要把序列化成字符串的 tool output 声称为已被递归改写。

R6F-GR-01 先限制清洗位置和类型，再审查合法 opaque reasoning/compaction 的保留规则。当前对密文长度、base64url、entropy 的启发式也不能单独作为“厂商密文非法”的证据；真实格式变化归条件证据，不借窄修复增加解码/重签或大范围删历史。

以下能力已存在：精确身份目录、reasoning 菜单/默认及显式支持 false、空目录/304/stale 不扩权、受限 reasoning proof 恢复、媒体任务所有权和 Web/CLI 分界。位置见 [grok_models.rs](../../src/clients/oauth/grok_models.rs) `:48` / `:569`、[Grok 合同](grok-oauth.md)。不能再列“缺少动态模型/推理能力”为待办。

另外两项需要补证：

- grok2api 提交态 `config.go:32` 默认是 **1.0.60**；其测试/注释中的“1.0.40 era”是旧模型样本背景。目标 [grok_cli.rs](../../src/domain/grok_cli.rs) 为 shell 1.0.13；但生成/目录/媒体的端点、UA、client identifier 必须逐操作对照。不能把版本数字较大当作升级依据，不能照搬参考 403→另一 upstream plane 的 fallback。
- sub2api `grok_model_quota_block.go` 在账号内对某个模型分别处理 free usage 与 transient capacity。目标 `forwarder.rs:20172` 的通用 429 路径给账号写 cooldown；`:20424` 的 Grok helper 刻意跳过 429，后者不能误读为“无账号限流”。收到明确模型级错误时是否过度阻断其他模型，需用目标实际 rail 的独立错误样本验证。

语义 idle 也不能误报缺失：`forwarder.rs:6231` / `:31993` 已按语义观察更新文本 deadline，WS `:15883` 也按业务观察推进。参考 `semantic_streamidle.go` 暂停未读取上游期间的计时，属于背压政策差异；可在既有 R6-EVID-01 补“心跳持续、业务停滞、慢下游、长推理”对照，先冻结当前正确行为，不直接替换全局 timeout。

### 3.4 Antigravity / Agy：补身份边角，保留严格终态与签名合同

CLIProxyAPI 的 `antigravity_claude_response.go` 在 `:111` 为 missing/blank response ID 生成唯一 ID；`5d2835c30` 另改工具 fallback，`36c176545` 处理零参数与空对象，args-only continuation 保存当前工具身份。Manager 的 pipeline/terminal 拆分可供职责设计参考，但其自动修复、继续生成、内容清洗及换号策略不适合直接迁入。

Server 已有原生签名配对/隔离、明确 ID 跨分片、schema 有界投影、grounding、缓存 input 排除、独立 countTokens/TOS 门禁和双 rail 目录身份。见 [Antigravity 合同](antigravity-oauth.md)。`cpaUsageMetadata` 是 CLIProxyAPI `helps/usage_helpers.go:4114` 起自己产生的中间字段，不是已确认厂商字段；目标不识别它不构成缺口。

**响应 ID：**[stream_transforms.rs](../../src/proxy/stream_transforms.rs) `GeminiAnthropicState::ensure_message_start:2058` 缺失时固定用 `gemini`；空字符串不触发回退。[transforms.rs](../../src/proxy/transforms.rs) `gemini_response_to_anthropic:2194` 的正常/blocked 分支同样固定，非流分支还未同流式一样读取 snake_case ID。grounding 合成 server tool 在 `:2267` 缺 ID 时固定 `gemini_search`。正式 adapter `:1508` 的 unwrap 只合并 wrapper 字段，不会补唯一 ID，随后进入上述转换。

R6F-AG-01 应为一次响应铸造一次 fallback，同流始终稳定、不同响应不同，保留合法上游值。合成 ID 是桥接关联值，不代表上游可 retrieve、可续接或有资源访问权限。共享 Gemini bridge 的修改需检查其他已登记调用方；不能仅因代码共享就扩展 Antigravity App。

**续帧：**`stream_transforms.rs:2030` / `:2281` 将缺名设为 `tool`，匹配 slot 时又要求 name 相同。例如独立合成两帧：

```json
{"functionCall":{"name":"lookup","args":{"q":"a"}}}
{"functionCall":{"args":{"limit":2}}}
```

同 candidate/part 的第二帧没有 ID/name：不能命中第一帧的 `lookup`，将新建一个默认名工具状态。`:2365` 关闭时会分别输出工具块。已有同显式 ID 的分片测试不能证明这个场景。参考可按 current tool 续写，但并行/同名/part 重用时可能歧义；R6F-AG-02 必须把“不要输出伪造工具名”的保护与“允许哪些续帧”分开设计，证据不足时明确失败，不用猜测合并。

### 3.5 WorkBuddy / CodeBuddy：目录已较严格，prepared snapshot 仍缺世代

[Buddy facade](../../src/proxy/providers/codebuddy/mod.rs) 已统一双产品的固定账号、目录、canonical request、生成与身份检查；[控制面](../../src/clients/oauth/codebuddy.rs) 已有持久化刷新、现代套餐与 3000 条截断下的连续时间扫描。workbuddy-switch 的用量扫描建议已被吸收，本轮不再立“接入官方用量”条目；Hub 的正常 EOF 补成功也不采纳。

**目录边界的静态链：**

1. [codebuddy_runtime.rs](../../src/proxy/codebuddy_runtime.rs) `:193` 的 catalog 有 fetched/expiry/stale 和 Arc 快照；`:347` 的 `PreparedCodeBuddyRuntime` 保存该快照。
2. [resident.rs](../../src/proxy/codebuddy_runtime/resident.rs) `insert_catalog:62` 会权威替换同 scope，包括空目录；`invalidate_scope:124` 删除缓存，`mark_catalog_stale:129` 只改变 cache 中的 clone。旧 prepared Arc 仍存在。
3. facade `prepare_generation:142` 按旧 catalog 选模型、工具、图片和默认 effort；`runtime_is_current:240` 只把身份/scope 传给 state。
4. [state.rs](../../src/state.rs) `codebuddy_runtime_generation_matches:18914` 检查 Provider revision/runtime、账号 incarnation、auth/token generation、profile/scope；没有 catalog snapshot/generation/currentness 参数。
5. `forwarder.rs:21567` 等发送/读取/成功检查使用这一函数，因此重复身份检查不会发现同身份下的目录替换。

这证明的是**目录世代未进入围栏**，不是“Buddy 没有身份保护”，也不是已发生收费或越权。相同身份下，准备后刷新为 disabled/empty 的请求是否还能出站，应先用确定性同步的正式路径测试验证。Qoder 在 [qoder resident](../../src/proxy/qoder_runtime/resident.rs) `CatalogEpoch:83` 已有 generation lease，可用作本仓设计参考；不能为了等价测试而引入 Qoder 依赖。

设计还必须保留当前“内存拒绝缓存时，本次新鲜目录仍可服务请求”的合同。简单要求 prepared Arc 必须等于 cache 中 Arc，会误伤无缓存结果与普通 eviction。应把权威替换/撤权与可选缓存留存分离，并明确 TTL、管理刷新失败和已发请求的行为。禁止通过重发生成来解决失效快照。

**请求身份：**[buddy_wire.rs](../../src/proxy/buddy_wire.rs) `:25–39` 五个头共用 `request_id`；`forwarder.rs:21492` 每次物理尝试随机创建。参考对比为：

| 头语义 | 当前 Server | Hub CLI | cli2api CLI |
| --- | --- | --- | --- |
| conversation | 每次物理请求 ID | 按 uid/session 稳定 | 按 uid/session 稳定 |
| conversation-request / root | 同一物理请求 ID | 同一逻辑 turn 稳定 | conversation-request 等于 conversation；不能据此推断 root |
| request / conversation-message | 同一物理请求 ID | 每次物理请求新建 | 每次物理请求新建 |

Hub `wb_identity.py:472` 自述的观察是上游**不靠 conversation ID 存历史**；该观察属于参考项目声明。因此不能由差异推断 Server 丢上下文、必然缓存失效或延迟变差。Hub 与 cli2api 的 turn 关系本身也不同，不能拼头。R6F-BD-02 应核对受审 2.142.0 与新版本的同一 CLI 身份；采用 Server 已有产品/站点/账号代次/Share/用户/session 隔离，不能信任客户端 raw ID。

**默认推理：**始终使用绑定目录默认值是明确政策，已有 requested/effective 记录。[reasoning.rs](../../src/proxy/codebuddy_runtime/reasoning.rs) 不应改成“客户端可以提高档位”。同理，Hub 图片/视频、桌面/VSCode 身份及新 alias 只作条件能力研究；现有受目录授权的图片理解已实现，不能误写为所有图片能力都缺失。

## 4. 可执行增强卡片

所有条目当前均 `planned`；以下是后续实施的完成条件，不是本次已执行测试。实施前重新核对目标 HEAD、[协议证据](../../PROTOCOL_EVIDENCE.md) 与 coverage 生成源；不手改生成文件。先写能在原正式路径失败的有意义用例，再窄修复。现有 state 域方法、锁序、单绑定恢复预算、计量/取消/内存所有权保持。

### R6F-GR-01 · P1 · confirmed_code_gap

- **目标落点：**Grok request patch、协议 item 访问与 schema 保真边界；复用现有 TransformPlan 记录必要兼容动作。
- **实施范围：**将 external-web 控制、服务端 item ID、reasoning 密文处理限定到经审计的路径/type。先验证 properties/default/examples/嵌套 JSON 不透明性；不能通过全量 JSON key 遍历选择业务改写对象。
- **前置/风险：**审查 native reasoning/compaction 和历史恢复合同。去掉启发式过滤可能暴露上游拒绝，必须明确本地拒绝或既有一次受限恢复规则，不能新加宽泛重放。
- **验收：**上述 function 的声明、forced choice、schema 和 default 完整；允许的工具回执内容保持；真实协议 item 按旧窄规则处理；HTTP、WS 及已登记的跨协议入口一致。非法项若清理后失去必要语义，应返回定位错误，不能静默删整轮。业务输出后错误不得重放；不改变模型/账号。

### R6F-OA-01 · P1 · confirmed_code_gap

- **目标落点：**Codex Responses 的 prepared WS target、最终握手语义快照、闲置上游 WS 缓存 acquire/release 与 refresh 后的重建。
- **实施范围：**在账号覆盖、客户端协商合并、metadata 清洗和代理独占头处理完成后确定连接身份。用于 key、建连与复用核对的数据必须同源；显式列出影响协商的字段，避免逐轮诊断值无意造成连接碎片。
- **前置/风险：**冻结现有凭据/workspace/runtime/session 隔离与 interrupt 物理连接 epoch。不要借此更换传输库或扩大客户端 header allowlist。
- **验收：**同一受控入口先完成 A turn 并释放上游缓存，再以相同绑定/session 发 B，覆盖 beta A→B、A→省略、省略→A；上游观察到匹配的握手。相同值保留可复用。401 refresh、缓存未命中、WS→HTTP、取消/关闭都保持身份；已绑定 previous_response_id 时身份变化受控失败，零隐式生成重放。测试必须经过正式入口，不能只直接调用 key 函数。

### R6F-BD-01 · P1 · confirmed_code_gap

- **目标落点：**Buddy catalog resident、prepared runtime lease、state/currentness 域方法与正式发送围栏。
- **实施范围：**将同 scope 的权威目录替换/撤权传播到未发送的 prepared 请求。先定义准入快照政策：TTL 到期、refresh 失败、内容相同刷新、已提交生成各自如何处理；至少已知 disabled/empty 撤权不能给旧准备请求继续授权。
- **前置/风险：**复用父内存账本和有界 flight；世代不依赖可选 cache entry 留存。状态写入只经 state 域方法；不可持锁等待整个网络调用。
- **验收：**prepare→暂停→同 scope disabled/empty 刷新→继续发送，确认生成 POST 为零；替换菜单/工具/图片权限不能沿用旧授权。无缓存、缓存拒绝、eviction、相同内容刷新和过期政策各有明确结果；跨产品/站点/Share/账号代次隔离。已发请求撤权后若终止，已观察 usage 仍结算，不补成功、不重发，不保留无界旧快照。

### R6F-AG-01 · P2 · confirmed_code_gap

- **目标落点：**共享 Gemini→Claude 响应身份工厂、流式 state、非流 blocked/正常/grounding 分支。
- **实施范围：**missing/null/空/空白统一 fallback；camelCase/snake_case 读取一致。一次响应铸造一次，后到 ID 不重铸已提交身份；合法上游 ID 不随意裁剪/改写。检查合成 server-tool ID 与回执配对。
- **前置/风险：**保持显式工具 ID、签名 proof、历史配对不变；如需改变合成 function ID，应另验多轮与碰撞，而非套用新随机值到所有 ID。
- **验收：**连续/并发两次无 ID 得到不同有效消息 ID；一个流跨分片恒定。缺/空/空白/snake_case、blocked refusal、late grounding、带合法 ID 各覆盖；工具 use/result 成对，未新增上游持久资源权限或续接承诺。

### R6F-CL-01 · P2 · conditional_protocol_gap

- **第一步：**取得 Advisor 请求/响应/续轮的独立脱敏形状，区分普通 client tool 名 advisor 与真实 server tool；核对同凭据、受控 token refresh、显式重新绑定、重启后未知来源的厂商接受性。
- **候选设计：**若确有 owner 约束，复用现有 binding/Thread 元数据，必要时增加有界 ID provenance；scope 含产品/Profile、Provider/runtime、账号 durable owner/身份代次、Share/调用者/session。token refresh 是否保留 owner 要按身份合同决定，不能简单以 token 字符串变化判 foreign。
- **验收/退出：**同 owner 保留 opaque 内容；已知 foreign 在发送前给出明确可恢复错误或经批准的精确兼容策略；unknown 不猜测、不静默删除。JSON/SSE/CountTokens/Thread 各确认；不修改 CCH 最终顺序，不记录密文、不自动换号。无法证明约束时可关闭研究项并保留当前透传。

### R6F-AG-02 · P2 · conditional_protocol_gap

- **第一步：**收集两 rail 上真实 args-only continuation 的 candidate/part/id/name 规则，区别参数 delta、完整 snapshot、零参数调用和签名 carrier。先将 §3.4 的两帧反例加到正式转换路径，标出当前是否输出默认 `tool`。
- **候选设计：**明确 ID 优先；无 ID/name 只在有证明的唯一活动调用中续接。两个候选、同名并行或 part 重用且无法唯一判定时明确协议失败；不能猜“最近一个”。完全无身份的首帧不能变成可执行伪造工具名。
- **验收/风险：**零参数 `{}`、显式 ID 分片、名字省略、并行同名/异名、part 复用、迟到签名、text/thinking 转换保持；结果参数等于证据定义的合并，未丢字段、未多生调用。已提交后失败不续写或重放。未证实的形状可以保持拒绝，不强求接受。

### R6F-BD-02 · P2 · conditional_protocol_gap

- **第一步：**对照受审 2.142.0 与参考新版 CLI 的 identity bundle，分别确定 conversation、logical turn、physical message ID 的格式/生命周期；解决 Hub 与 cli2api 对 conversation-request 的冲突。
- **候选设计：**从既有隔离 scope 派生会话，逻辑 turn 跨一次允许的 401 retry 保持、物理尝试另铸 ID；是否采用该关系以证据为准。显式固定 CLI 通道，不混入 desktop/VSCode UA 或登录 platform。
- **验收/风险：**双产品×双站点、同 session 多轮、不同 Share/用户、账号删除重建、token refresh、重启与无 session 请求。既有默认 effort、企业字段、origin、端点保持；不宣称历史由 header 自动存储，也不无测量承诺缓存/延迟收益。

### R6F-GR-02 · P2 · conditional_protocol_gap

- **第一步：**按 OAuth discovery/refresh、models、quota、HTTP/WS inference、direct image/video、任务 poll/cancel 建身份矩阵，核对 endpoint/client identifier/UA/version/session/group/agent 及验证日期。参考 1.0.60 与旧样本分开记录。
- **候选设计：**只有同 endpoint/操作的证据才更新相应身份 bundle；新 group/session 头从 Server scope 派生。若影响目录权限，缓存键与实际出站捕获同一版本快照，不允许 A→B→A 复活旧观察。
- **验收/风险：**按操作记录真实接受/拒绝，旧身份回滚有界；Web SSO 与 direct-XAI 不借 CLI 接受回执。禁止照搬参考自动转 upstream plane、换号或正文清洗；版本变更不自动授予模型/媒体权限。

### R6F-GR-03 · P2 · conditional_protocol_gap

- **第一步：**在现有固定 XAI rail 上辨明结构化模型限流、账号额度、通用速率、容量过载及未知 429。sub2api 的“free usage for model”文本及 TTL 是参考行为，不能直接当成官方错误规范。
- **候选设计：**有充分证据时将模型级 cooldown 写入现有精确模型作用域，账号级仍阻断全账号；未知证据保守处理。不增加账号池、替代模型或另起一套限流存储，不能把估算免费 token 作为硬额度。
- **验收/风险：**模型 A 限流后，同一绑定 B 按既有权限仍可准入；账号额度错误同时阻断 A/B。测试最终映射模型、Share、auth generation、HTTP/流内/WS 握手、reset/Retry-After 和迟到旧响应；错误分类变化不能新增重放或绕过限额。

### R6F-OA-02 · P2 · conditional_protocol_gap

- **依赖：**OA-01 完成；先核对原生客户端是否发送该头、受审后端接受哪些 token，以及被裁剪字段对响应桥接/授权/计量的依赖。
- **候选设计：**有限且可诊断的协商规则，明确定义空/空白/多值头与大小上限。不得裁掉 Server 记账必需的 usage 后按零计量；不能把原生 Codex 头转发给任意 OpenAI-compatible Provider。
- **验收/退出：**HTTP/WS/401/fallback 中有值→省略不残留，变化不复用旧协商；aggregate/stream 工具结果和 usage 仍正确。若只能保证保留计量而无法保证裁剪语义，应明确限制或拒绝，不承诺全量 passthrough；证据不够时维持未支持。

### R6F-OA-03 · P2 · evidence_gap

- **实施范围：**先盘点现有用例，再补少量组合反例：大 payload 上传期间双向 Ping/Pong、active turn 长心跳却无首语义、ACK 与终态相邻/跨连接、确定性坏图嵌套错误、容量过载与坏输入同时出现。
- **验收：**活性不满足首语义、不延长既有业务 deadline；interrupt 不连接/重放、不确认另一 response；坏输入同字节最多一次生成提交，capacity 的既有恢复预算不被诊断文本误触发。JSON/SSE/WS/同绑定 HTTP fallback 只覆盖实际登记路径；失败/取消的 usage 与最后 owner 仍收敛。
- **退出/风险：**如果新反例当前已通过，只登记 evidence，不改代码。CLIProxyAPI 的无限网络等待、codex2api 的 catch-all/buffering/跨账号选择均不采纳，不能用参考 timeout 替代当前产品政策。

## 5. 分阶段落地与证据矩阵

| 阶段 | 内容 | 进入下一阶段的条件 |
| --- | --- | --- |
| A：确认正式路径 | 为 GR-01、OA-01、BD-01 各写最小反例，核对原代码失败原因；AG-01 补 ID 样本 | 失败确实来自所述边界，未被无关校验/测试 helper 掩盖；更新协议研究记录 |
| B：独立窄修复 | 每项独立 review/回退，不同时换传输、IR 或目录架构；AG-01 随后处理 | 语义、固定身份、终态、计量和资源所有权不回归 |
| C：条件研究 | CL-01、AG-02、BD-02、GR-02/03、OA-02；优先选已有真实失败需求的一项 | 同通道证据成立才实施；冲突/未知保持关闭，不能为满足排期猜 wire |
| D：证据收口 | OA-03 与既有 R6-EVID-01；补五类 operation 视图 | fixture、厂商接受性、部署状态可独立判读；不把部分通过升级整个 driver |

建议用既有 real-receipt/reference-delta 体系，按下表补粒度；不新建平行验证框架、不重新宣称完成历史 R5。

| 类别 | 分别记录的 operation / scope | 不可替代关系 |
| --- | --- | --- |
| Claude | Messages JSON/SSE、CountTokens、工具续轮、Advisor、Thread/重启；最终模型、CLI/beta profile、账号/调用者 scope | 普通工具/5.1 成功不证明 Advisor、新模型或 Thread 成功 |
| OpenAI/Codex | Responses/Chat、WS 多轮/interrupt、HTTP fallback、compact、models/search、独立 Images 与 Responses 图片工具 | 纯文本成功不证明坏图分类、图片费用或续接协商 |
| Grok | OAuth HTTP/WS、models、reasoning proof/compaction、每种媒体 create/poll/cancel；另列 Web rail | Build 参考或 Web SSO 成功不证明 direct-XAI 权限；chat 不证明媒体 |
| Antigravity/Agy | 两 rail 各自 Claude/Gemini 入口、签名工具、grounding、目录版本、countTokens、TOS | 一条 rail/一种入口成功不升级另一条；共享 bridge 不授予新 App |
| Buddy | product×site×CLI identity；原生 login→refresh→catalog→生成、工具、图片理解、套餐/官方用量 | 同 UID、另一站/产品、目录可见或套餐成功不证明生成；默认 effort 政策独立保留 |

每份可接受证据至少关联源/binary 摘要、版本、操作、输入形状摘要、模型/身份 scope、生成尝试次数、终态/usage、取消与采集日期。公开只存脱敏结论；凭据/正文/签名及私有回执留仓外。缺输入保持 `blocked_inputs/live_pending`，静态审计和合成 loopback 不提升为 live_verified。

未来代码实施按根 AGENTS 完成 include 源格式、cargo check/test、相关正式路径回归、writer/Phase 0、coverage/UI matrix、本地 smoke 和离线 readiness；按变更选择检查，不为本次文档规划运行产品全套或 UI 自动化。

## 6. 本次实际验证与限制

本次实际进行：推荐关系/目录指令读取、八个参考提交及脏状态复核、本仓请求准备/缓存/发包/转换链静态追踪、已有 fixture 与产品合同核对、提交态关键文件 SHA-256 计算。没有执行文中拟议的失败用例、Rust/前端测试、真实 OAuth/推理/媒体或部署；没有更改 protocol/reference-delta 契约来把计划伪装成已实现。

实际文档验证：docs index/相对链接检查通过（66 个索引文档），coverage `--check` 通过，conformance 通过（11 个 governed drivers，全部粗粒度 live gate 仍 pending）。11 个条目编号唯一、18 个提交态文件摘要与本文一致；tracked/untracked 文档空白检查和变更范围检查通过，工作区变更仅四个 Markdown 路径，其中三个是上一轮已有计划及索引。最终修改后再次核对这些文档门禁及参考 HEAD。原 R6 的十项编号保留，五类深入分析以本文为聚焦补充入口。

### 6.1 关键提交态文件摘要

以下摘要仅供研究复核。完整仓库 SHA 见 §2；这些文件不被默认构建或测试读取。

| 参考仓 / 文件 | SHA-256 |
| --- | --- |
| `CLIProxyAPI/internal/runtime/executor/claude_executor.go` | `5cc836125d01d215b10d64fb4c95f96ce2dd0e45d1033c13d3915c57f728082f` |
| `CLIProxyAPI/internal/runtime/executor/helps/claude_advisor_state.go` | `b7a88c8d3aececfa6fd679dee3daf7521080ee5f85faba47aa006aa94c36d5a3` |
| `CLIProxyAPI/internal/runtime/executor/codex_websockets_session.go` | `aa40d66edc972a18093d0142b0969dde18c87b36576ba752c2bdb29ca16e8717` |
| `CLIProxyAPI/internal/translator/antigravity/claude/antigravity_claude_response.go` | `91b574d33d60093f95748ab5c6cfc0c247c7772fdd3e26483e8a8180649f4733` |
| `Codex/codex2api/proxy/executor.go` | `d8a767ba2c8230c4b1b5c7c81485763ffc05c8c96342d1e3eab1751cd5cbeb25` |
| `Codex/codex2api/proxy/wsrelay/client_identity.go` | `cff7eb0181dea05ce886f712d7966b9d706c4db20b5d18da6093e0cc18776eee` |
| `Codex/codex2api/proxy/invalid_input_image.go` | `0025a790158ef09ccefd1126787479596f4df80d1ade2851519a32b180bf0bd5` |
| `Grok/grok2api/backend/internal/infra/config/config.go` | `219cf703228b7a4532efbd36f6a022511455f7faf91a1ac133ec69fc61478fe1` |
| `Grok/grok2api/backend/internal/infra/provider/cli/responses_reasoning_recovery.go` | `5379850b4a929a89d7b4f839d658579d31f91a6f4f54c3c2bd27f8559546709c` |
| `Grok/grok2api/backend/internal/infra/provider/cli/responses_tool_types.go` | `a76e83fb39868f836d155bdddf1978ab25369d596286e0347a99902a700cc90a` |
| `Grok/grok2api/backend/internal/infra/provider/cli/semantic_streamidle.go` | `6c798b00f949278cf28940431f78a31b1f13de4eea32ca31826606c87c7c597e` |
| `Grok/grok2api/backend/internal/infra/provider/cli/adapter.go` | `4bffffb891d2c022533b8241fb72b32dbe92cb82c94131014aebf0bd12838fc6` |
| `sub2api/backend/internal/service/grok_model_quota_block.go` | `e8975bcee3fd2040415aaee82940cd29bbbd48d14472773a0239c8a37a1aa8a1` |
| `Antigravity/Antigravity-Manager/src-tauri/src/proxy/pipeline/terminal.rs` | `ebfdc4ff69a99719e76714adc29ac297d8a40f605a1e692e95be03020dbf7723` |
| `Antigravity/Antigravity-Manager/src-tauri/src/proxy/pipeline/stream_lifecycle.rs` | `2b33f702f1dcaaa8f03a5239444a0b66baf269ad37d12049ddc6f3a0d221923b` |
| `CodeBuddy/workbuddy2api-hub/wb_identity.py` | `d5dd5cf6882dfa7ffe0155621461fa70f254e55ac0caddf70dbb159356fce7d2` |
| `cli2api/internal/providers/workbuddy/headers.go` | `3fae3e39bd29dec666bdab3c1a8bbbcf35581cd1ec50f3abb61d0e5679f026e6` |
| `CodeBuddy/workbuddy-switch/crates/wb-switch-core/src/modules/official_usage.rs` | `40e4d45f138310d95821b6f3336699fbb6a0d7f2d528b9d169b0a6def2aed902` |
