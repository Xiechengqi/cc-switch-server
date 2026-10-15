# Qoder COSY 单账号反代

本文定义 cc-switch-server 的 `qoder_cosy` Provider 边界。一个 Qoder Provider 固定绑定一个 Qoder Account；Server 不实现账号池、轮询、权重、按配额/并发选号、自动切站，或 OAuth/PAT/其他 Provider 之间的 fallback。

当前 `special.qoder_cosy` Driver contract revision 为 **2**。`assets/contract/qoder-reference-delta.json` 把该 revision 与三个 Registry Profile、生产入口、官方 CLI oracle、生成 coverage、三 rail conformance 和真实验收状态映射在一起；`scripts/audit/audit-qoder-reference-delta.mjs` 对这些来源做交叉校验。`supported` 只表示实现入口存在，`fixture_verified` 与 `live_pending` 仍分别表达离线合同证据和缺失的真实证据，三者不得互相替代。

Provider 生命周期边界已收敛到 `src/proxy/providers/qoder/`：它持有精确 `qoder_cosy` Account 绑定、Share/用户/session conversation scope、live catalog model、payload preparation、generation fence 与 Qoder throttle 分类。COSY codec/terminal 仍由 `proxy::qoder` 持有，runtime cache/payload primitive 仍由 `proxy::qoder_runtime` 持有；Share/Account lease、usage、共享 attempt budget、终态和是否执行同账号 pre-commit 401 replay 继续由 forwarder 持有。该结构迁移不改变 endpoint、header、签名、wire、恢复次数或跨边界 fail-closed 规则。

## 站点与凭据 rail

- Global 与 China 是账号身份的一部分，分别固定到经过审计的 origin。请求中的模型、403、quota 或网络错误不能改变站点。
- Global/CN OAuth 通过受限 device flow 建立账号；Global 另支持显式导入 `pt-*` PAT。PAT 每次只为原绑定账号交换短期 job token，不进入 OAuth refresh rail。
- Provider binding 固定 Account id 与 `authIdentityGeneration`。session、catalog、quota、conversation 与短期 job token scope 同时包含 App、Provider revision/runtime fingerprint、Account、site、credential rail、auth generation 和 token generation。
- 第一个 eligible 401 只允许刷新原 OAuth Account，或为原 PAT 重新交换一次 job token；仅在下游提交前重放一次。第二个 401、提交后错误、generation drift 与站点漂移均为终态。

Device lifecycle 由官方 Global/CN `1.1.32` bundle 的固定摘要与 `assets/contract/qoder-cli-oracle.json` 共同冻结。两站都使用 UUID v4 nonce、S256 PKCE、1 秒 poll、300 秒 flow TTL，`GET /api/v1/deviceToken/poll` 的 404 表示 pending。Global machine ID 是 36 位小写 hex；CN machine ID 是 UUID v4，这两个格式不能互换。

Device Flow 是控制面短生命周期状态，不是账号池：同一管理员 principal 对 `qoder_cosy` 同时只保留一个 active flow，新建时在 managed-auth guard 内原子替换旧 flow 与 owner binding；全局最多 64 条，超限按创建时间淘汰最旧项。成功结果只保留 60 秒用于幂等轮询，之后删除；poll lease 仍保证同一 code 不会并发兑换。

账号 profile 读取、refresh/quota 与 session 创建共用同一个 site-aware machine-ID validator：Global 必须恰好 36 位且只能是小写 hex，CN 必须是 RFC 4122 variant 的 UUID v4；前后空白、错站格式、Global 大写 hex、错误 UUID version/variant 都在发网前拒绝。生成器仍分别生成对应格式，持久化数据不能借 `trim()` 或另一站格式蒙混通过。

两站 Device OAuth refresh 都固定为 OpenAPI `POST /api/v1/deviceToken/refresh`，body 只有 `refresh_token`，header 只有 JSON Accept/Content-Type 与 `User-Agent: qoder/1.1.32`，不得携带 Authorization、COSY identity、Proxy Authorization 或路由账号头。响应主字段是 `device_token`、轮换后的 `refresh_token` 与 `expires_at`；旧 Global center `/algo/api/v3/user/jobToken` 不是 fallback。轮换 receipt 必须先 durable commit，随后才允许 userinfo/CN auth-status 完成 identity closure；失败结果未知或 identity drift 时 fail closed。

CN 的 catalog、quota、auth-status 与 generation 都必须发送非空规范单播 IPv4 `cosy-clientip`。默认按目标路由询问内核选择本机出站 IPv4；remote-DNS proxy 令目标解析不可用时，仅用 UDP connect 的默认路由探测兜底（不发送数据报）；`CC_SWITCH_QODER_CN_CLIENT_IP` 可显式覆盖。Server 永不采信下游 `Forwarded`/`X-Forwarded-For`。Global 继续使用原 machine identity 规则，不进入 CN resolver。

## 动态模型目录与 capability

COSY model list 是当前绑定账号的权威 entitlement。成功空目录保持为空，不与 Provider 配置、静态 alias 或其他账号目录合并。目录条目必须是 object，包含非空且无首尾空白的原样 key；`enable` 若存在必须是 boolean。坏结构、超限响应、缺失字段和身份 scope 漂移均失败关闭。未知厂商 key 保持大小写原样，只有已审 alias 的匹配不区分 ASCII 大小写。

Global/CN alias 表集中维护，但只有对应 live-enabled route 才会发布公开 alias。若上游同时给出与 alias 同名的原始 key，该原始条目不能冒充 alias 的 route/capability；未启用 alias route 时公开目录也不发布这个不可请求的 ID。上游出现未知 live route 时保留其 exact id，便于观察新 entitlement，同时不猜测 reasoning、context、图片或其他能力。`/v1/models` 可返回：

- `reasoningEfforts`：按 site × route 的 reviewed matrix，仅发布 `none`/`low`/`high`/`max` 中已证明的组合；
- `contextWindow`：只在 route 有明确协议证据时发布；
- `inputModalities: ["text"]` 与 `supportsTools: true`；图片能力保持不发布；
- `source: qoder_live_model_catalog`、fresh `fetchedAtMs`，以及权威空结果。

推理 payload 与目录使用同一 capability resolver。fixed-context route 只写 `model_config.max_input_tokens`；runtime-selectable route 还同时写 `parameters.context_length` 和 `chat_context.extra.ideModelConfigOverride.max_input_tokens`。未知 route 不注入 context，未知 effort 不注入 reasoning 字段。

## COSY 签名、session 与流

- 请求使用 reviewed COSY body encoding、AES/WAF/signature 和固定 signature path。签名 timestamp 必须位于验证时钟正负五分钟内；请求 id 必须是规范 UUID v4 nonce。
- oracle schema v2 独立冻结三 rail 的精确 origin/path/profile、完整 required/forbidden signed-header 集、encoding/signature vectors、cli2api/server projection 与 accepted-difference 原因。canonical synthetic Chat 保存去随机 UUID 后的完整 server body；Global/CN 都由生产 builder 生成后做整棵 JSON exact equality，任意额外、缺失、改型或移位字段都会失败。
- session single-flight 创建并按 runtime、Account 代次、进程内账号实例和请求身份摘要隔离缓存；摘要覆盖实际凭据、profile、机器 token、有效期和测试端点，不保留明文。返回缓存结果及发起 generation 前复核当前绑定，防止同 ID 重建或未涨代次的凭据/机器身份变化复用旧 session 与目录。conversation id 还包含 Share、签名用户与模型，复用的外部 session id 不能跨 scope 命中。
- Claude Messages、OpenAI Responses/Chat 和 Gemini 三类输入归一到同一 COSY conversation contract，再按原 Surface 输出。声明工具、reasoning、usage 和 terminal 事件使用同一个 model capability 与严格 SSE 状态机。
- Codex Responses 的 `previous_response_id` 在转换为 Chat 前从原始请求校验：缺失或 `null` 沿用普通请求；畸形类型、空值、首尾空白或超过 512 字节返回 400；未知、过期、已领取或身份不符的精确 ID 返回 409 `response_context_unavailable`，且不发送 Qoder 生成请求。已缓存 ID 只在同一 runtime、Share、用户、上游模型键及原始下游模型名内续接原 COSY session；单模型映射不能让不同客户端模型名串接。缓存限 2 小时、1024 项、8 MiB 总量、64 KiB 单项，先只读现有 session/catalog，再原子领取；不跨账号、站点或凭据 rail 回退。
- Codex Responses 的下游响应 ID 由每次请求独立生成，流式和非流式均不复用上游可能重复的 Chat ID。省略的 system/tools 继承缓存指纹与工具名，显式漂移或无法与上一响应调用配对的工具结果在发网前拒绝。非流完成后发布下一别名；流式在唯一终态、上游 EOF、身份和输出转换全部通过后，于最终完成事件交付前发布，客户端收到 `response.completed` 后可立即续接。发布失败先发送失败事件，不交付成功终态；终态前取消不发布。离线回环已覆盖三 rail 的两轮续接、工具结果、取消及作用域漂移；真实账号验收仍独立 `live_pending`。
- Canonical Chat 的 `tool_choice` 在 COSY builder 中转换为工具可见性：`none` 不发送工具，`auto`/有工具的 `required` 保留声明，指定函数只保留精确匹配的声明。指定名称不存在、函数选择畸形、未知模式或无工具却要求 `required` 时在发网前拒绝，不能把强制选择静默放宽成全部工具。Claude/Codex/Gemini 共用此转换；它不新增 COSY `tool_choice` wire 字段。
- bounded response compatibility 接受 OpenAI `choices[].delta/message`、content block 数组、reasoning/usage 别名，以及 `content_block_delta`、`tool_use_start/delta`、`tool_call_delta`、`message_delta/stop` envelope，再统一成 Chat chunk。空 placeholder tool call 会丢弃；非字符串 arguments 规范编码为 JSON。该兼容面来自 TokenRouter `5f94cbcf2d1f4e74badf449c192c1431dc4e5c8e` 与 cli2api `b67278960df9c160d45a7520ee3110b5ccb84126` 的只读交叉证据，不改变官方 wire/header/terminal oracle。
- Chat 思考文本先逐项校验 `reasoning_content`、`reasoning_text`、`reasoning` 的显式类型，再按此顺序选首个非 `null` 值。高优先级别名为 `null` 时不遮住后续有效文本；非流聚合和流式规范化使用同一选择边界。显式空字符串仍是合法值，不会被较低优先级别名擅自替换。
- 明确提供的 reasoning 文本、`content_block_delta.data.text` 和 `type=text` 内容块 `text` 若为非 null 的非字符串值，decoder 在转发前按本地协议错误拒绝；缺失、null 及合法字符串沿用原有兼容行为。这防止错误类型被当作空文本丢弃，不表示厂商新增了这些形状。

目标 Server 的 Qoder wrapper decoder 与非流 Chat 聚合器现在按最早的完整 SSE 空行切分事件，CR、LF、CRLF 及混合结束符在任意网络分块位置都不会把两条 `data:` 并成一条；聚合器也拒绝第二个 `[DONE]`。这仅修本地解析一致性，不引入新的 COSY wire 假设。真实 Global OAuth、Global PAT、CN OAuth receipt 仍分别缺失。
- usage 兼容转换保留已有的非负数字、数字字符串与有限小数截断行为，但任何显式提供的 token 计数字段若为负数、非数值、非有限值或超出 `u64` 范围，整次响应按协议错误失败；即使同组别名的优先字段有效，备用字段无效也不能被忽略后伪装为零。直接进入聚合器的 Chat chunk 使用同一校验。此项只防止本地伪计量，不声明厂商新增 usage wire。
- 规范化 usage 后还要求 `prompt_tokens + completion_tokens` 可用 `u64` 精确表示；显式 `total_tokens` 不得小于该和。跨 chunk 合并后的两个桶若求和溢出也失败关闭，不再静默饱和。上游 total 大于两桶之和仍保留，缺 total 时继续以精确和补齐；这仅是本地计量一致性门槛，不证明厂商真实计费口径。
- SSE wrapper 的 `statusCodeValue`、`statusCode` 若出现，必须能解析为 100～599 的 HTTP 状态；两字段同时出现须一致。显式负数、空值、未知状态名、越界值或矛盾状态不能退回默认 200。缺失两字段时沿用原有成功默认；数值 200、数字字符串 200 和 `OK` 等已兼容成功写法保持可用。这是本地失败关闭规则，不推断新的 COSY 状态格式。
- HTTP 200 wrapper 的内层 JSON 若显式带非 `null` 的 `error`，decoder 立即按上游错误失败；已识别的 `112`/`115` 仍分别归类为权限/限额，无法分类且无限额时间线索的形状为 502，不能作为心跳忽略，也不能让同帧 `choices` 或后续成功终态遮住错误。`error:null` 的合法业务 chunk 保持可用。这是本地错误保真，不凭离线反例推断三条 rail 的真实错误形状。
- SSE wrapper 本身必须是 JSON 对象；`body` 缺失仍可表示原有空心跳，但只要显式提供就必须是字符串。`null`、数组、标量外层，以及 `body: null`、对象或数值均在 `[DONE]` 之前按本地协议错误拒绝，不能被当作空业务帧洗成成功。三条 credential rail 经 Claude/Codex/Gemini 的回环确认不触发 refresh；这仍是本地形状门槛，不证明厂商真实发送或接受性。
- Chat `choices[]` 在 decoder 转发前和非流聚合时共用单选项边界：一个 chunk 至多一个 choice，缺失 index 沿用 0，显式值必须可解析且等于 0；`finish_reason` 若非 null，必须是非空白字符串。重复 index 0、非法 index 或伪终止值直接按本地协议错误拒绝，不能被拼成一条回答、转发给下游或作为成功终态。空 choices 的 usage-only chunk 与跨 chunk 的单选项续接保持。该检查不改变唯一终态与 EOF 门槛，也不推断新的上游 choice 格式。
- 工具调用 `index` 另有独立校验：OpenAI Chat `delta.tool_calls[]`、直接进入非流聚合器的工具分片，以及兼容的工具事件外层索引，缺失时沿用原有位置/0；显式提供时必须是可解析的非负整数且小于 128。`null`、非法字符串、负数或越界值不能被回退为 0 或数组位置，以免把不同工具分片合并。数值及数字字符串的合法索引仍可用；此项不推断新的 COSY wire。
- 工具调用 ID 是不透明身份：非空 ID 在 Chat/兼容工具事件规范化时保留原始字符串，包括首尾空白；只有全空白 ID 按原有缺失规则处理。非流聚合器对同一工具索引的后续 ID 仍做精确比较，不能把 `" call-a "` 与 `"call-a"` 合并。工具名称的既有兼容裁剪不变；这只修本地身份保真，不证明上游会发送带空白 ID。
- 同一工具调用的兼容别名也必须一致：`id`、`tool_call_id`、`call_id` 是同一不透明身份，`function.name`、`name`、`tool_name` 是同一工具名。并存的非空值若冲突，或显式值不是字符串/null，在转发前固定以本地协议错误拒绝；相同别名及单一别名继续可用，ID 不因名称裁剪规则而去掉首尾空白。此项不扩展 COSY wire，仅防止兼容归一化静默改写工具身份。
- 历史工具调用只在当前 assistant batch 内关联：missing/duplicate call id 确定性修复，未知历史工具及其结果成对过滤，orphan/重复 result 丢弃，parallel result 按调用顺序输出；result 缺 id 时仅在唯一未消费 call 场景推断，多义输入直接 400，不猜测也不跨 batch 绑定。
- authoritative finish reason 或上游 `[DONE]` 只标记候选终态，不能单靠 `[DONE]` 把空心跳、空 delta 或内容缺结束理由的流洗成成功。decoder 仅转发有文本、reasoning、工具、usage 或明确结束理由的规范 chunk；已有有意义内容的 Anthropic `message_stop` 可生成对应 `stop`/`tool_calls`，原生 Chat 缺结束理由仍失败。decoder 必须继续读取到上游 EOF，期间第二个 terminal、任何业务数据或 auth error 都失败；只有有效输出、明确结束理由和 EOF 全部验证通过后才向下游输出唯一一次完成事件。非流聚合也不把仅 `[DONE]` 补成空 `stop`。
- 流格式转换器可能从上游的候选 `finish_reason` 提前生成下游完成事件，因此 Qoder 专属流会暂存候选终态开始的输出；之前的正文仍可流出，暂存计入同一请求内存预算。唯一终态、EOF、身份和转换均通过后才交付最终事件，并在最终 chunk 交付前记录成功账务。客户端取得最终 chunk 后立即断连不反记为取消；Codex Responses 别名在同一交付边界已发布，可直接续接。候选终态后错误不能先泄漏成功事件，终态前取消也不发布别名；不据此开启断连后的后台上游收集。
- decoder 和非流 Chat 聚合器一旦在 `push` 拒绝任何数据，后续空块、其他数据与 `finish` 都保持失败；decoder 的完成状态也不再可见。第一次错误仍保留原有上游/本地协议分类，之后只返回固定的本地协议错误，避免已拒绝的终态后数据被最终聚合洗白。
- 下游业务输出提交前的 401 可按上述一次预算恢复；提交后的 401、断流、重复或畸形 terminal 只能结束当前流，不能重放。
- OAuth 与数据面错误区分 invalid grant、authentication、permission、rate limit、temporary 与 protocol。只有明确 invalid-grant 证据或 401 触发重新登录；普通 400 与默认 403 不伪装成凭据失效。`retry-after-ms`、`Retry-After` 秒数/HTTP-date 统一解析并限制在 24 小时。上游 JSON 错误按深度/节点上限递归脱敏 token、Authorization/Cookie、COSY key、machine token、uid/aid/org id，日志和指标只记录有限分类，不记录敏感值或 IP。
- 上游错误的 `code` 是不可信输入，只有已用于本地分类的 `112`（无权限）和 `115`（限额）会进入下游错误 JSON 与文本；其他文本、数字（包括六位验证码形状）、超长值及非整数一律不回显，仍保留上游 HTTP 状态与经过脱敏的 message。该约束不改变 401 恢复预算，也不推断新的厂商错误码。
- 错误体的顶层、`error` 和 `data` 别名按优先级读取，最多深入 8 层；外层显式 `null` 可回退，非 null 值仍优先，即使是未知错误码也不能被内层 `115` 覆盖。首个 message 若是至多 64 KiB 的 JSON 字符串，也可作为缺失错误码和限额重置时间的后备证据；其文本仍走脱敏，未知错误码仍不回显。此项修复本地嵌套分类，不改变 401 重试预算或推断新的厂商错误码。

## 验收边界

oracle `verification` 是既有验证计数单源：65 项离线 Rust Qoder 聚焦测试覆盖 Global/CN lifecycle exact HTTP、refresh rotation/receipt/error taxonomy、capability、公开 alias、权威空目录、坏目录、quota oracle、完整 payload/header、context/reasoning、clock skew、nonce/machine identity、三 credential rails × 三 Surfaces、EOF/唯一终态、响应 envelope、工具历史、Device Flow 容量、session/provider/account generation fencing、Provider facade scope、pre/post-commit 401，以及连续两次 401 只恢复原绑定账号一次；9 项 Node mutation 还冻结 bounded compatibility、安全边界、coherent projection、accepted reason、actual/signature path、header set、encoding/signature vector 与计数漂移；7 项 loopback real-harness fixture 验证 harness 合同。生成 coverage 直接读取 65/9/7，不再维护手写副本。CORE-N2 的独立容量 fixture 使当前 Qoder 关键词测试为 74/74、request-memory 关键词测试为 24/24，不改写 oracle 的历史 65/9/7 元数据。

`scripts/smoke/qoder-real.mjs` 按 `global_oauth`、`global_pat`、`cn_oauth` 一次只运行一条 rail。控制面仅对通过生产 `QoderAccountProfile::parse` 验证的 `qoder_cosy` Account 发布脱敏 `qoderCredentialScope.site/credentialRail`；验收器要求它与所选站点及 rail 精确相等，缺失或不匹配时在数据面请求前失败，账号 ID 前缀和 token 形状不能代替 profile 证据。schema-v2 receipt 绑定当前 target commit/harness、Account auth/token generation、三个 Surface Provider revision/runtime digest、Share revision、site/rail、exact model、三份 fresh catalog snapshot 和六个请求 body hash，再验收 quota、三 Surface non-stream/stream/tool/usage/唯一终态；文件写在仓库外并强制为 0600。`acceptanceChecks` 对每项分别写 `pass`、`binding_only` 或 `not_observed`；当前 harness 没有实际执行登录/轮换、权威空目录和受控两段 401，因此即使连接真实服务也只能写 `partial_live_verified/live_pending`，不能单独提升整条 rail。`scripts/audit/qoder-real.test.mjs` 的 loopback 结果固定为 `contract_verified/live_pending`，同样不能替代真实 receipt。

三条 rail 的私有验收器直接核对 Share 原始 `bindings`：Claude、Codex、Gemini 各须恰好一条精确 `qoder_cosy` Provider 绑定，主字段须与对应的唯一主绑定一致，且 `enabled=true/status=active`。重复、冲突、停用或缺少明确启用状态的 Share 在目录请求前拒绝；主字段不会合成为额外 binding。控制面复核仍不是跨 API 原子快照。

三个 Surface 的 Provider 均按 Server 真实投影校验：顶层 `revision` 与 runtime `providerRevision` 必须明确存在、是非负安全整数且相等，runtime `runtimeFingerprint` 必须为非空白字符串；Share `configRevision` 也必须明确为非负安全整数。缺失、不一致、旧 mock 使用的虚构顶层 `providerRevision` 或空白 fingerprint，都在三份目录请求前失败且不会发布 receipt；不以 `0` 回退补齐作用域。此门槛只约束本仓库私有证据，不构成真实 COSY 接受性或跨 API 原子快照。

三条 rail 的 Account 凭据布尔投影也必须明确匹配：Global PAT 要求 `hasApiKey=true`、`hasAccessToken=false`、`hasRefreshToken=false`；Global/CN OAuth 要求 `hasApiKey=false`、`hasAccessToken=true`、`hasRefreshToken=true`；全部要求 `needsRelogin=false`。缺字段、`null` 或字符串 `"false"` 不能代替布尔值，错误在 Provider 与三份目录请求前拒绝且不发布 receipt。此门槛不改变生产认证路由或三 rail 的 `live_pending` 状态。

三份 fresh catalog 分别以 `app`、`expectedShareId` 和对应 App 的 `expectedProviderId` 请求签名 Router ingress 的 `/v1/models`。普通 `providerId` 查询参数不决定该入口选择的 Provider；服务端先按签名 Share 与 App 选出绑定，再拒绝与预期 Share/Provider 不符的目录。此检查只证明目录请求的路由绑定，不证明厂商 entitlement 或后续数据面与目录是跨 API 原子快照。

三份目录还必须在 receipt 发布前分别满足两小时新鲜窗口和最多五分钟的未来时钟偏差；validator 在模型获取、准备写文件及临时文件完成后排他发布之前使用同一时间谓词。若三 Surface 请求或临时文件写入期间任一目录过期，即使其获取时有效，也不发布 receipt。这是私有 happy-path 证据门槛，不修改生产 COSY 目录 TTL、wire 或 rail/site 选择；末次检查并非跨 API 原子事务，也不消除检查与硬链接发布之间的极短时钟窗口。

三条 rail 在末次 Account、三个 Provider 与 Share 绑定复核后，还会通过同一签名 Share 和各 App 的 Provider 栅栏重读 Claude、Codex、Gemini 三份目录。每份完整目录的模型 ID 都须为非空字符串、无首尾空白且唯一；目录按 ID 排序后计算完整 `data` 摘要，末次摘要与 receipt 绑定的初次摘要各自一致。纯行顺序变化可接受，能力字段等内容变化仍拒绝。当前目录也必须保持合法来源、非 stale、包含选定模型；初次及末次共六份时间戳在发布前同一时刻复核新鲜性。模型撤下、添加、目录失效、来源漂移或超前时钟都会拒绝发布。分次读取不是跨控制面与上游的原子快照，离线 fixture 不证明真实 Qoder 账号 entitlement。

私有 receipt 发布需要 Linux `/proc/self/fd` 支持；缺失时在任何控制面请求前失败关闭。发布器打开已解析的仓库外父目录，并用该目录描述符锚定临时文件与排他硬链接。写入前、链接前及链接后都比对当前配置路径与所持目录 inode，并复核真实位置仍在仓库外；若链接后发现父目录漂移，仅撤销本次创建的链接。这样即使配置路径的符号链接在写入或链接时改指仓库内，也不会把 receipt 写进仓库。目录 inode 本身被并发移动、元数据被恶意复原或末次检查之后再次发生漂移，不由此获得原子保证。

发布器还将临时文件及最终硬链接分别与待写出的完整 receipt 字节比较，检查普通文件类型、链接数、大小、`0600` 权限、dev/inode 和纳秒级 mtime/ctime。临时文件写入后被等长改写或替换、链接后被改写，均拒绝成功报告；失败时只撤销仍指向本次文件 inode 的最终链接，避免删除后来占据该路径的其他文件。检查不是文件系统原子事务，恶意元数据复原、检查后的变化及目录本身的并发移动仍不由此保证。

临时文件也在排他创建时保留读写描述符，receipt 字节通过该描述符写入并复核；失败清理只删除仍指向本次创建 inode 的临时路径。这样临时路径若在写入后被另一文件替换，验证会失败，但不会误删替换者的文件。路径比较与 unlink 之间仍存在竞态，不能据此声称文件系统原子发布或真实 Qoder 接受性。

真实验收仍需分别提供 Global OAuth、CN OAuth 与 Global PAT 的脱敏 receipt，覆盖 login/import、refresh/job-token exchange、catalog、non-stream/stream、tools、reasoning、quota、首个/第二个 401 和 generation rotation。当前没有真实凭据或 receipt，只能标记 `fixture_verified` / `live_pending`，不能标记 live verified。

QD-N2 已把只读 TokenRouter 交叉核对刷新到 `7faf9469bc6957716923b5b4a98665c0fb9715e0`。已提交 Qoder 文档没有变化，handler 差异只适配共享错误 helper 的签名，没有新增 origin、header、签名、payload、terminal 或恢复 wire，因此本轮不修改生产实现；官方 CLI oracle 继续是第一来源。参考仓库的其他 Provider、调度、计费和 UI 增量不属于 Qoder wire 证据。

Qoder reference delta 已迁为 append-only schema v2：原 schema-v1 文件及 11 个内容字段分别由不可变摘要固定；TokenRouter snapshot 绑定 `7faf9469bc6957716923b5b4a98665c0fb9715e0` 与 tree `b205061c4f0d9b53fbffb0242f34796e10b18251`，并声明只读取干净工作树的 committed Git objects。11 条 observation 把 QD-01～03、QD-N1/N2、CORE-N1/N2、LIVE-N1 和 QD-R1～R3 映射到 source path/symbol/digest、处置理由及本仓库 committed baseline/implementation object。QD-OBS-0011 只把参考的 4 MiB 控制面读取、500 MiB 默认 SSE line ceiling 与非流事件数组作为 retained-state 差分信号；`CORE-N2-QODER=fixture_verified` 由本仓库独立 sticky budget 与 committed fixture 支持。QD-R1～R3 明确拒绝账号池/轮换/affinity 或跨边界 fallback、商业控制面，以及外仓运行时依赖、伪 live 与宽松 terminal；默认 audit 自包含，`--check-sources` 才读取 TokenRouter 冻结对象。

2026-09-28 的 TokenRouter 最新 HEAD `6d676f1d100e11f2f068949f5779e960c862079c` / tree `4b830127433aa21c8534891526b3617bf90d2b80` 要求当时会话使用不可用的 `project-doc` skill；工作树中的 `UD AGENTS.md` 也被排除。`QODER-SKILL-GATE-2026-09-28` 仅冻结当时的仓库元数据，并明确标记 `businessFilesRead=false`、`businessAnalysisPerformed=false`、`productionCodeChanged=false`，不能冒充业务补审。该历史记录的最近可信 snapshot 为 `tokenrouter-2026-09-19`，不描述当前工作树的门禁状态。

2026-10-14，TokenRouter 工作树已删除根 `AGENTS.md`（HEAD 仍追踪，恢复时须重核），因此旧文件的 `project-doc` 前置门禁不再限制当前工作树的只读研究。`Qoder/AGENTS.md` 仍指定它为首选参考；目标仓的外仓只读证据边界保持。对已提交 `c25dbf0cfa5a806460ed0fbc5c43a55797148537` 的 Qoder 网关、认证/刷新、会话、模型、输入输出和失败结算路径逐项补审见增强计划 §18.302～18.320；可移植的嵌套错误分类和 Responses 续接等缺口已按目标单账号边界实施，商业计费、多 Key 切换、断连后后台推理不迁入。reference-delta 现追加 `QODER-CURRENT-HEAD-READ-ONLY-2026-10-14`，不改写 9 月的门禁记录；显式 `--check-sources` 会复核旧冻结文件和这次补审的三个提交树身份，但路径级分析仍以计划中的逐项记录为证，不能仅靠树身份校验替代。Global OAuth、Global PAT、CN OAuth 的真实账号验收在当前实施 goal 外，三条 rail 均保持 `live_pending`。

最终整体 review 补齐用量别名边界：TokenRouter 当前已提交 Qoder 解析器在 `prompt_tokens`/`completion_tokens` 为零时取非零 `input_tokens`/`output_tokens`，目标原把已出现的零主字段固定为最终值。目标现在对所有出现的计数字段保持严格校验，再仅为零主字段使用非零别名；主字段非零、全部为零及显式总数一致性规则不变。Chat choice 和 `message_delta` 共用规范化入口，流式/非流式均受益；离线回归不改变三条真实 rail 的 `live_pending`。

CORE-N2 精确启用 `qoder_cosy`：Global OAuth、Global PAT、CN OAuth 和 Claude/Codex/Gemini 三 Surface 共享请求生命周期预算，覆盖 canonical/runtime/catalog/prepared payload、COSY 编码签名、错误响应、SSE decoder/canonical event、非流聚合与下游 retained state。耗尽为稳定 503/同码流终态和 `CapacityShed`，不 replay、refresh 或记为网络故障；真实 rail 状态不变，仍分别为 `live_pending`。

## CLI 升级门禁

升级 Global 或 CN CLI 时必须保持以下顺序：先冻结包名、版本、npm integrity 与 bundle digest；再从不可变官方包独立提取稳定 wire，并用只读第二来源交叉核对；随后先更新 oracle 和使旧合同失败的 mutation，再改 Rust；最后重新生成合同映射与 coverage，运行离线和 loopback 门禁。任何 rail 都只能由自身独立 receipt 从 `live_pending` 提升。

外部仓库和 npm bundle 永远不是构建或运行时依赖。禁止在缺少独立 digest 的情况下让 fixture 与 Rust 同步“自洽”更新，禁止跨站复用包证据，也禁止用一条 rail 的真实成功外推另一条 rail。精确阶段与禁止项由 `qoder-reference-delta.json` 固定并由审计脚本校验。
