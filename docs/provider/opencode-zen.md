# OpenCode Zen 免 Key 反代

状态：单条公开免费车道；协议离线/本地 loopback 验证与真实供应商验收分开记录。目录已做只读在线格式核对；真实推理及 Router 链路仍为 `live_pending`。

不使用 OpenCode Go、付费 Zen API Key、账号池或出口轮换；不注册第四个 App。Claude/Codex 的 Profile 使用 `special.opencode_zen`，无用户凭据，固定 `https://opencode.ai`，配置不可覆盖端点。Gemini Surface 不开放。

## 线格式与协议转换

- 现代目录的已知协议仅映射到固定路径：Chat `/inference/openai/v1/chat/completions`、Responses `/inference/openai/v1/responses`、Messages `/inference/anthropic/v1/messages`。旧目录继续使用 `/inference/v1/*` 与既有 Muse Spark/union-alpha 协议映射。未知协议、不同 origin 或未受审路径不可选；绝不直接跟随目录 URL。
- `Bearer public`；UA `opencode/latest/2.1.3/cli`；官方 CLI/session/affinity 头，不发 `x-opencode-request`。
- Session 是可解码时间戳前缀加随机尾部，6 天重铸。缓存键包含 Provider、Share、用户和下游会话；缺少会话时每次请求独立，不把全部用户折叠到 Provider ID。4096 条、哈希键、单条最旧铸造记录淘汰，不接受下游直接指定上游 session。
- 官方 `read`、`shell` 原文指纹；冲突工具改名并恢复，Chat 名称增量支持别名分片。缓存提示不注入上游，工具 schema 中的同名属性不删除。
- Chat JSON 关闭 HTML 转义并保持 CLI 顶层字段顺序；缺省输出上限取 32000 与当前模型声明上限的较小值；显式超出声明的上限会在发送前拒绝。`tool_choice:none` 在出站前拒绝。

入口协议与模型的上游协议独立。使用 Server 自有 transforms 与 stateful stream bridge，覆盖 Messages/Chat/Responses 三入口 × 三上游协议 × 流/非流的 18 个基础组合。请求转换保留 system/instructions、工具调用 ID、工具结果与历史，响应转换遵循入口格式，Chat 的 created 使用统一兼容边界。

本 Driver 不实现 Responses 持久化资源；`store:true` 和 `previous_response_id` 显式拒绝，调用方须提供完整历史。目录仅公布受审的文本/工具能力，不开放未经实测的图像/音频能力。

## 流式、终态与资源

SSE 增量解析，支持网络任意分片、UTF-8 分片、多行 data 与 CRLF。单事件及终态尾部各最多 1 MiB；JSON 响应最多 8 MiB，并受剩余请求内存约束。解析/转换临时空间预留预算，保留状态及输出 Bytes 跟随生命周期释放。

Chat 必须有合法 finish_reason 与 `[DONE]`；Responses/Messages 使用公共严格检查器。非 Chat 终态后不接受数据；错误事件、非法 JSON、残缺帧或缺失终态不能成为成功。成功终态输出保留至干净 EOF，语义内容在 EOF 前增量交付；length/incomplete/max_tokens 保留不完整状态。

总 deadline 覆盖目录刷新、并发准入、上游头和完整响应；首语义 deadline 不因 role/usage/心跳重置。空闲时限只由上游字节推进，下游每 15 秒的 SSE 心跳不延长它。慢下游采用拉取背压；丢弃响应体时取消上游并记录 client_cancelled。账户/Share in-flight、请求预算和计量证据随响应体持有；响应从未被读取时也有取消收尾。

## 一次非流续写与计量

仅对 Chat 上游的纯推理中断、仅推理正常结束及输出上限截断评估一次续写，不在已提交 SSE 中发起第二次生成。显式结构化输出、stop 或强制工具选择不做拼接续写，保留原有不完整结果。续写须有实际 output usage、至少 512 剩余 token、上下文检查点符合本地保守上限且总时间尚有余量；第二段最多 8192 token，不重置请求总预算。没有 usage 时不猜测剩余预算。

截断回答保留第一段并追加续写，不以第二段替换第一段。不同生成的 usage 相加，同次 SSE 累计 usage 由公共累积器去重；Messages 采用 Exclusive 输入语义。续写失败保留有效首段及原有截断状态，无有效回答时返回失败。工具生成不会触发续写；续写意外调用工具会失败，不执行工具。

每次上游生成使用独立 USD attempt。证据沿用公开请求 ID 并递增 attempt；公开日志及 token 分享用量只汇总一次，`value_generation_usage` 单独保存当前生成值，不能把累计 usage 写入最后一个 USD attempt。未知发送/未知用量保持保守状态，不将网络错误冒充未发送。

## 错误与模型目录

429、403、401 不自动重放，分别保留对应 HTTP 状态；其他供应商错误映射为网关错误。客户端和日志不拼接上游原始正文/HTML，只保留固定类别、原状态、受限请求 ID 及合法数字或 HTTP-date Retry-After 提示，并通过公共错误元数据向客户端透传。已识别的参数/上下文错误映射为 400；不能识别的错误仍按网关错误处理。重铸 session 不会创造新额度。

公开目录、管理发现与生成准入共用缓存：8 分钟 fresh，失败后旧成功目录最多保留 24 小时；冷启动无可用目录则拒绝生成，不使用写死清单。单次刷新最多 15 秒、响应 1 MiB、512 个模型、每 ID 256 字节；并发 single-flight，失败退避 30 秒。模型类型、免费分类和协议共同决定准入；decision/image/未知类型及决策模型 `jev*` 不作为 Chat 模型开放。手动刷新可绕过 fresh，但受 30 秒最短间隔、并发合并和失败退避限制。

成功目录原子写入数据目录 `opencode-zen-catalog.json`；重启时仅恢复有效时间窗内、大小受限的版本化完整模型快照，兼容旧版免费 ID 快照，未来时间或过期数据无效。缓存保存失败不撤销上游成功结果。目录规范化 name/type、旧式 cost.input/output 与现代 cost.text.*、limit.context/output 及旧式容量声明。显式零价、收费、未知分开；正价（含阶梯、缓存和模式）优先于 free 名称，缺价不补零。Legacy ID 来源单独标注。成功空名单撤销旧候选，失败不清空；管理发现与生成只从同一快照派生可用子集。目录保留合法 context_window/max_output_tokens 声明；公开接口可投影这些声明，管理端明确列为 upstreamDeclared，不冒称实测能力。降级返回旧目录时管理发现标记 stale。

## 证据与验证

参考身份和逐文件 SHA-256 见根 `PROTOCOL_EVIDENCE.md`，只读参考 `zen-gate` HEAD `58fa1b7`。外部仓库不参与构建、测试、发布或运行。

本仓回归覆盖协议矩阵、工具往返、逐字节分片、错误/终态、缓存恢复与冷启动并发退避；forwarder loopback 验证增量交付、未读取响应取消、续写拼接/用量及限流无重放。完整验证与 review 结果由实施回执记录，离线通过不代表厂商真实通过。

整体 review 修正了无凭据 Profile 的创建与运行时准入、Responses 终态后数据校验、慢下游对空闲超时的影响、分片事件解析前的内存预留，以及续写准入失败时的 attempt 编号。SQLite loopback 同时核对一条公开日志的汇总用量与两个独立 USD attempt，防止最后一个 attempt 重复计入首段。Chat 错误帧按公共协议携带错误后以 `[DONE]` 结束；该分隔符本身不是成功证据，成功还须有效终态、干净 EOF 和完成日志。

2026-11-17 最终验证：Zen 相关 28/28、API 集成 138/138、Node 审计 2205/2205；严格 Clippy、check、build、完整格式检查与本地 HTTP smoke 通过。全量 Rust library 为 4479 passed / 2 failed / 3 ignored，两项为既有额度基线失败，完整套件未全绿；具体名称与最终源码摘要见 `PROTOCOL_EVIDENCE.md` 的 Zen 条目。离线 readiness 为 blocked_inputs，真实目录/推理及 Router/Share 验收继续 live_pending。

## Bundle 管理与固定模型

Zen 的 `authScheme:none`、空凭据槽表示认证条件已满足，无须配置 API Key。该 Profile 不生成 S2 密文凭据或共享密文别名；管理视图 `credentialConfigured=true` 表示认证条件就绪，`credentialSlots=[]` 仍明确没有凭据。Share 的公开来源身份限定于同一显式 Zen Bundle，不能把所有免凭据供应商或不同 Bundle 当成同一账号。

Claude/Codex 均支持透传与固定上游模型，默认保持透传；允许全局或按应用配置。固定策略在目录准入和协议选择前由公共运行时策略执行：下游模型名保留为 requested_model，实际模型用于出站路径、免费目录校验、续写与 USD 计量。保存配置不调用真实目录；生成时仍拒绝付费或不在有效目录中的模型。

保存遇到服务端异常或传输中断时，保留输入并回读，但不自动重放。提示展示脱敏的保存/回读状态及受限错误码，不回显响应正文、凭据或任意异常 URL；回读 404 不再被直接解释为断网。

OpenCode 图标取自官方 `https://opencode.ai/favicon.svg` 的图形，内联 SVG 只保留静态几何，并使用 currentColor 适配主题。旧 Zen 预设持久化的 `custom` 占位在 Web 和服务端身份投影中解析为 OpenCode；其他明确自定义图标保持不变，其他供应商不受影响。

## 动态固定模型选择

切换到固定上游模型后，Web 自动调用受认证的 `fetch_opencode_zen_models`，新建与编辑均无需先保存 Provider。首次加载和编辑期间每 8 分钟刷新、手动刷新共用后端缓存；可搜索名称或精确 ID，也可直接输入自定义模型。默认仅列出可选免费模型；“显示全部模型”可查看付费、未知价格或未支持类型/协议，相关项禁用并说明原因。免费条目显示 Free 标记；声明不等于真实可用性或已验证能力。

全局与 Claude/Codex 按应用固定策略使用同一目录。读取、刷新、失败、下架和晚到结果不改写用户输入，不自动选择第一项，不自动保存。目录失败可保留手动模型并保存；实际调用仍必须通过后端准入。`requiresExplicitUpstreamModel` 表示该 Profile 没有静态默认模型，固定策略需显式填入模型；旧 chat-free 配置保留原值并提示，不自动替换。

2026-11-18 动态模型选择实施与整体 review 已完成。发布隔离树的 Rust 源码和重新构建的 Web 资产与已通过全量验证的冻结树一致：Zen 专项 35/35、库 4521 通过 / 0 失败 / 3 忽略、API 契约 139/139、两个独立集成目标各 1/1；完整静态检查、check/build 与本地 HTTP smoke 通过。本次另行复测 Web 类型检查、414/414 测试和生产构建，均通过。具体摘要、命令及真实验收边界见根 PROTOCOL_EVIDENCE.md 的动态模型选择条目。

## 逐模型能力、控制参数与检测

模型目录保留 reasoning/toggle/effort、结构化输出、采样、工具 choice/strict、输入模态与容量声明；管理接口的 `capabilities` 是声明，`effectiveCapabilities` 是当前驱动允许请求的子集，真实接受性仍为 `live_pending`。公开 `/v1/models`、管理发现及单次请求的准入、协议、容量全部从同一目录快照投影。过期回退目录如实标记 stale，保留基础免费模型准入，不开启可选推理/采样/schema/strict 能力；旧版缓存可恢复但不补造能力。

Messages、Chat、Responses 入口的推理强度、JSON Schema、temperature/top_p/top_k、stop 和并行工具控制按模型与上游协议校验、转换，再进入请求整形。无法表示的控制项、冲突的输出上限/推理强度、未声明的 strict 工具要求等在发送前明确拒绝，不经提示词模拟或静默降级。模型 effort 后缀与显式 effort 必须一致。图片等媒体仍未开放，目录中的 image 声明不构成公开免 Key 通道的验收证据。

工具别名只恢复协议位置，工具 input/arguments/结果保持原样；原生 Messages 的历史 tool_use 同步改名。若上游调用为兼容指纹而注入、但客户端没有声明的 read/shell 工具，返回明确失败。非流成功响应校验 envelope、assistant、终态、工具参数和 usage；Chat → Messages 等待独立 usage 尾帧，避免提前结束丢失用量。Responses 流只公开已审核事件，并启用公共重复输出保护。

每个进程只有一条 public lane，所有 Provider/Share 共用并发上限；`CC_SWITCH_ZEN_MAX_CONCURRENCY` 启动时读取，1..32，默认 4，最多排队 32。队列服从总 deadline，取消释放排队资格；SSE 响应体持有许可直到终结或取消。429 按模型冷却，401/403 按通道冷却，使用受限 Retry-After 或默认 30 秒；不自动重放，不轮换出口。一次非流续写沿用首次准入快照、控制参数和剩余输出预算，续写失败也记录冷却。

仅有 HTTP 429 时，检测结果归类为 `rateLimit`，表示请求受限；不能据此断言免费额度耗尽或具体重置周期。本地冷却也使用同一分类。Zen 的手动和自动模型检测均只尝试一次，不受全局检测重试次数影响，保留首次拒绝的原因。Web 的限流和额度提示适用于实际供应商，并保留脱敏错误详情，不套用其他服务的 Coding Plan 说明。

Registry 已开放正式 test/connectivity。固定模型策略是检测模型权威；透传策略必须显式配置或在检测时选择模型，不借用全局 Claude/Codex 默认值。配置检查不访问目录或生成；网络检测复用正式 driver，使用 256 token 上限，完整消费 JSON/SSE 并检查结构化终态，单个超时覆盖全部响应。Router modelProbe 使用同一模型与预算；没有检测模型时不发布虚假 probe。

网络检测的转发与响应读取分为可取消任务，超时或调用取消时中止未完成任务并收尾用量。调试构建的公共分发器单次 poll 栈帧超过 Tokio 默认 2 MiB，因此本地 debug 服务使用至少 8 MiB 工作线程栈，并保留显式配置的更大 RUST_MIN_STACK；release 的栈配置保持原行为。专项回归独立使用 8 MiB 工作线程，覆盖完整 JSON/SSE 与超时取消，避免仅依赖全量 fixture 的 64 MiB 测试栈掩盖问题。

2026-11-18 真实公开通道观察：最终 debug 二进制使用独立临时配置，通过完整管理员 HTTP 检测入口调用 `deepseek-v4.1-flash-free`，上游返回 429；随后的 Codex 检测命中同一模型的本地冷却，没有重复生成请求。默认启动的服务保持正常，生产配置未修改。该结果仅验证真实拒绝及冷却路径，成功生成、可选控制参数和 Router/Share 的真实验收仍为 live_pending。

2026-11-18 本轮实施及整体 review 已完成：合入后的 Rust 库 4566 通过 / 0 失败 / 3 项既有可选忽略，API 集成 139/139、独立集成 2/2；Zen 名称/模块匹配的 58 项回归通过。Node 审计与 smoke 脚本测试 2218/2218，Web 60 文件 / 429 项测试、类型检查与生产构建通过。严格 Clippy、完整格式检查、check/build、10 步本地 HTTP smoke、生成清单和文档索引均通过。review 同时修正免密钥运行时鉴权、受约束请求的续写条件和 debug 检测入口栈溢出；没有遗留本轮实现阻断项。源码摘要、二进制身份及离线 readiness 的真实输入边界见根 PROTOCOL_EVIDENCE.md 最新 Zen 条目。

提交前已纳入主分支 `907a178` 的分享 UI 更新并重建 Web 产物：类型检查、60 文件 / 436 项测试、生产构建和更新后嵌入资源的 cargo check 全部通过。Rust 源码与上述全量验证摘要一致；此前 429 项 Web 测试记录对应更新前的 UI 快照。
