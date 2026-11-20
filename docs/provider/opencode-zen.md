# OpenCode Zen 免 Key 反代

状态：单条公开免费车道；2026-11-18 已在 MY/HK/US 三节点的隔离 Server 上验证 HY4、DeepSeek 两种免费模型的 Claude/Codex 流式与非流式推理，以及 HY4 的三入口工具往返。Codex 客户端封装修复另在 US 隔离 Server 的签名 Share 路径上验证：完整 Codex 0.169 字段形状配合合成内容，两款模型流/非流 4/4 成功，HY4 工具往返 1/1 成功；这不是实际 CLI 会话或生产部署验收。验收仅覆盖这些操作；真实公网 Router 端到端链路、其他模型与可选能力仍为 `live_pending`。

不使用 OpenCode Go、付费 Zen API Key、账号池或出口轮换；不注册第四个 App。Claude/Codex 的 Profile 使用 `special.opencode_zen`，无用户凭据，固定 `https://opencode.ai`，配置不可覆盖端点。Gemini Surface 不开放。

## 线格式与协议转换

- 现代目录的已知协议仅映射到固定路径：Chat `/inference/openai/v1/chat/completions`、Responses `/inference/openai/v1/responses`、Messages `/inference/anthropic/v1/messages`。旧目录继续使用 `/inference/v1/*` 与既有 Muse Spark/union-alpha 协议映射。未知协议、不同 origin 或未受审路径不可选；绝不直接跟随目录 URL。
- `Bearer public`；请求规范 `opencode-1.19.5-bun-1.4.4-v1` 使用成功捕获的 CLI 1.19.5 UA、CLI/session/affinity 头，不发未经需要证明的 `x-opencode-request`。冻结的本地系统提示及环境标签提供兼容前缀，后续明确交由调用方提供实际身份、指令与工具；调用方 system/history/tool schema 保留。前缀不包含捕获时的临时目录、内置 skill 或用户对话，会占用部分输入上下文；实际计量以上游 usage 为准。
- Session 是可解码时间戳前缀加随机尾部，6 天重铸。缓存键包含 Provider、Share、用户和下游会话；缺少会话时每次请求独立，不把全部用户折叠到 Provider ID。4096 条、哈希键、单条最旧铸造记录淘汰，不接受下游直接指定上游 session。
- 官方 `read`、`shell` 原文指纹；冲突工具改名并恢复，Chat 名称增量支持别名分片。缓存提示不注入上游，工具 schema 中的同名属性不删除。 Zen 专用工具适配对受影响的 `spawn_agent` / `wait_agent` 函数名（包括已转换的 namespace 与冲突后缀）使用稳定、可逆且避免碰撞的出站别名，不依赖另一工具是否同时存在。声明、强制选择和已完成调用历史使用同一映射；返回时恢复原名与 Responses 工具身份。工具描述、schema、调用 ID、参数和结果正文保持，历史工具不因改名获得当前声明权限。原生 Responses 的 namespace 容器、选择器和历史仍按其原有身份处理。
- Chat JSON 关闭 HTML 转义并保持 CLI 顶层字段顺序；缺省输出上限取 32000 与当前模型声明上限的较小值；显式超出声明的上限会在发送前拒绝。`tool_choice:none` 在出站前拒绝。

入口协议与模型的上游协议独立。使用 Server 自有 transforms 与 stateful stream bridge，覆盖 Messages/Chat/Responses 三入口 × 三上游协议 × 流/非流的 18 个基础组合。请求转换保留 system/instructions、工具调用 ID、工具结果与历史，响应转换遵循入口格式，Chat 的 created 使用统一兼容边界。

本 Driver 不实现 Responses 持久化资源；`store:true` 和 `previous_response_id` 显式拒绝，调用方须提供完整历史。目录仅公布受审的文本/工具能力，不开放未经实测的图像/音频能力。

Codex CLI 的 Responses 客户端封装与生成控制分别处理。`client_metadata` 只接受字符串值对象或 null，作为会话和窗口记账信息在出站前移除。`reasoning.summary=auto` 与 `text.verbosity=medium` 兼容上游默认行为。`include=["reasoning.encrypted_content"]` 是可选输出投影，仅在原生 Responses 保留。原生 Responses 在新鲜推理声明下保留 summary。显式推理摘要、JSON Schema 与 strict 工具按实际协议和模型能力校验。

本通道没有托管 Web 搜索；Codex CLI 若启用 `web_search` 工具，会明确提示关闭该项，可为该供应商使用 `codex -c 'web_search="disabled"'`。普通客户端 function 工具及既有 namespace/custom 映射不受影响。卡片的精简 curl 检测不代表已覆盖 CLI 全部字段；本仓另用完整 Codex 0.169 请求形状覆盖三上游协议和流/非流路径，CI 显式运行 Zen 模块回归。

## Codex CLI 连接与 Responses Lite

Router Share 的「连接」弹窗提供新连接使用的 Codex CLI 命令。固定模式显示实际绑定模型，透传模式列出有效目录中具备兼容 CLI 推理档位的免费模型。用 Share API Key 设置 `TOKENSWITCH_API_KEY` 后即可运行启动或验证命令。

固定上游模型的 Responses 请求可以继续使用原有 Codex 模型别名、默认参数和会话。Server 校验客户端传入的档位类型、值与冲突。绑定后缀固定可用档位，其余请求优先使用模型声明的同名档位，或按 `minimal → low → medium → high → xhigh → max` 选最近的档位，等距取较低一档。HY4 把 `low` 和 `medium` 映射为 `high`。省略档位时采用绑定后缀或上游默认。显式 `none` 仅在原生 Messages 有新鲜的禁用能力声明时映射到 `thinking.type=disabled`。透传模式按目标模型的实际档位校验。

固定模式对 `low`、`medium`、`high` verbosity 使用上游默认，并独立校验 `text.format`。Codex 0.170.0 的 Responses Lite 可能携带 `reasoning.context` 和 `additional_tools`。Chat 在新鲜目录声明 `reasoning_content` 时，Server 验证明文推理出现在所需回合。`current_turn` 仅保留最近一轮的推理，`all_turns`、`auto` 或缺省允许回放完整明文历史。HY4 等缺少回放声明的 Chat 目标仅省略经过结构校验的明文推理项，并保留对话消息、工具调用标识和结果。作用域要求回放的密文、压缩历史和远端历史引用收到明确错误。依赖目录声明的适配在目录过期或刷新失败时返回可重试错误。

请求日志分别记录客户端请求的档位和实际发出的档位，Router 可显示 `low → high`。verbosity 回退与明文推理省略在调试日志中使用固定原因和计数。

当前审计过的 Codex 0.170.0 内置 `functions.exec` 声明可桥接到 Chat 目标。完整参数包含唯一的非空 `input` 字符串，源码和可选 pragma 由 CLI 在原有权限下处理。Server 在收到干净的 SSE 终态后交付完整工具调用，保留跨分片的普通工具名和 Code Mode 调用标识。遗失、冲突或无法验证的 Code Mode 参数和结果收到明确错误。

2026-11-20 使用实际 Codex CLI 0.170.0，在隔离的实际 Router Share 隧道、完整 Server 与固定 US IPv4 出口的真实 Zen 之间，通过 HY4 / DeepSeek 的 8 项 CLI 场景、10 次真实生成。原有 `gpt-6.2-sol` / `gpt-6.2-astra` 别名、默认档位和 verbosity 保持，覆盖 Code Mode 工具结果回传与会话恢复。另两组签名 Share API 的真实工具往返（4 次生成）验证了别名后的工具被正确选择、恢复为原始 namespace/函数身份，并接收模拟结果继续回答；没有启动实际子代理。该验收不代表已经更新生产服务。

Chat 向 Messages 或 Responses 转换时，跨增量的工具名和参数先完成有界组装与身份校验，确认干净终态后交付，避免工具名分片时提前出现占位名 `tool`。普通文本内容仍增量交付。

## Claude Code 连接与工具发现

在 Router Share 的「连接」弹窗展开 Claude Code 配置。Router 读取签名绑定上的 Zen 目录，固定模式显示绑定模型，透传模式可以选择当前可用模型。用 Share API Key 设置 `TOKENSWITCH_API_KEY`，再复制启动或 `claude -p` 验证命令。命令根据新鲜目录的模型声明选择可用推理档位，输出上限取声明值与 32,000 中较小的一个。已有 curl 继续用于测试模型连通性。

连接命令设置 `CLAUDE_CODE_DISABLE_THINKING=1` 并显式选择 Zen 的推理档位。Claude 的自适应思考与原生签名显示要求按实际协议校验。显式上限和档位保持严格校验，例如 Sonnet 默认的 128,000 对输出上限较小的固定模型会得到含实际模型和可用上限的错误。只识别 `clear_thinking_20251015` 的 `keep:"all"`，并检查历史在目标协议上能否保留。Chat 在新鲜目录明确声明 `reasoning_content` 时保留客户端返回的无签名文本思考历史，原生 Messages 保留实际的原生块。

连接命令默认立即加载普通工具。需要 Claude 本地 `ToolSearch` 时可移除 `ENABLE_TOOL_SEARCH=false`。Server 验证搜索调用、`tool_reference` 与当前工具声明的配对，再转发已选工具及结果。搜索和工具执行由客户端完成。托管搜索与原生签名显示仍按各自的能力校验返回明确错误。

2026-11-19 使用 Claude Code 2.1.327，经隔离 Router Share 隧道和模拟 Zen 上游通过 8 项验收，覆盖 HY4 与 DeepSeek 的固定模型、透传、普通工具和延迟工具。另用固定 US 出口向真实 Zen 发起的两个模型请求均返回 HTTP 429 `free_usage_limit`，真实生成继续记为 `live_pending`。测试版本与细节见根目录 `PROTOCOL_EVIDENCE.md`。

## 专用出站传输

Zen 推理使用固定 Bun 1.4.4 的 `fetch`，不再通过共享 reqwest/rustls 推理。系统消息与传输是同机同模型差分确认的两个兼容条件；强制 TLS 1.3、换 UA 或回退旧端点均不能独立证明有效。目录请求仍使用共享 Rust HTTP client。

Server 在首个真实生成的计量之前准备运行时：从官方固定版本下载，分别验证 archive 与解压 binary 的 SHA-256，原子安装在 `<config-dir>/runtime/opencode-zen/bun-1.4.4/<platform>/bun`。Linux amd64 使用 baseline，覆盖 glibc/musl 与 arm64。运行时不会追随 latest，也不依赖安装 OpenCode CLI。离线部署可预置校验清单对应的 binary，并通过绝对路径 `CC_SWITCH_ZEN_BUN_PATH` 指定；不匹配的 binary 会拒绝执行。下载失败或超时属于请求前检查失败，不产生已发送的生成计量。单个进程合并准备，后续请求复用已验证的 runtime。

每个上游请求使用独立子进程和有界 stdio 协议，不监听推理 HTTP 端口，不将请求写入磁盘。子进程环境只保留所需的信任库配置；出站只使用显式 `CC_SWITCH_OUTBOUND_PROXY`，不继承环境中的其他代理或 OpenCode 凭据。上游地址与六条路径固定受审，不跟随重定向。Rust 校验 16KiB metadata，按 32KiB 增量读取，正常 EOF 还须子进程成功退出；失败不能伪装干净 EOF。取消任一阶段或丢弃未读响应都会杀掉子进程；stdout 背压、总 deadline、首语义/空闲超时与既有 Rust 解析和计量生命周期共用。

## 流式、终态与资源

SSE 增量解析，支持网络任意分片、UTF-8 分片、多行 data 与 CRLF。单事件及终态尾部各最多 1 MiB；非流式调用也向上游请求 SSE，校验终态及干净 EOF 后聚合为入口协议的 JSON；累计 SSE 最多 8 MiB，并受剩余请求内存约束。解析/转换临时空间预留预算，保留状态及输出 Bytes 跟随生命周期释放。

Chat 必须有合法 finish_reason 与 `[DONE]`；HY4 已确认会在完整用量尾帧中重复同一终态，仅允许一次无新增内容的相同终态用量尾帧并归一为 choices=[]，冲突终态仍拒绝；Responses/Messages 使用公共严格检查器。非 Chat 终态后不接受数据；错误事件、非法 JSON、残缺帧或缺失终态不能成为成功。成功终态输出保留至干净 EOF，语义内容在 EOF 前增量交付；length/incomplete/max_tokens 保留不完整状态。

总 deadline 覆盖目录刷新、并发准入、上游头和完整响应；首语义 deadline 不因 role/usage/心跳重置。空闲时限只由上游字节推进，下游每 15 秒的 SSE 心跳不延长它。慢下游采用拉取背压；丢弃响应体时取消上游并记录 client_cancelled。账户/Share in-flight、请求预算和计量证据随响应体持有；响应从未被读取时也有取消收尾。

## 一次非流续写与计量

仅对 Chat 上游的纯推理中断、仅推理正常结束及输出上限截断评估一次非流式续写。显式结构化输出、stop 或强制工具选择保留原有不完整结果。续写复用首轮的 Chat 历史、已映射的控制和目录快照，并要求实际 output usage、至少 512 剩余 token、上下文检查点符合本地保守上限且总时间尚有余量。第二段最多使用 8192 token，并受请求总预算约束。

截断回答保留第一段并追加续写，不以第二段替换第一段。不同生成的 usage 相加，同次 SSE 累计 usage 由公共累积器去重；Messages 采用 Exclusive 输入语义。续写失败保留有效首段及原有截断状态，无有效回答时返回失败。工具生成不会触发续写；续写意外调用工具会失败，不执行工具。

每次上游生成使用独立 USD attempt。证据沿用公开请求 ID 并递增 attempt；公开日志及 token 分享用量只汇总一次，`value_generation_usage` 单独保存当前生成值，不能把累计 usage 写入最后一个 USD attempt。未知发送/未知用量保持保守状态，不将网络错误冒充未发送。

## 错误与模型目录

`FreeUsageLimitError`、`FreeTierError` 与地区拒绝保留受限类别，不把所有 429 解释为付费套餐额度耗尽。已审计的结构化 `FreeUsageLimitError` 若明确提示免费模型须在 OpenCode 中使用，归为 `free_lane_gate`，其 Retry-After 只冷却实际发出的模型和请求 profile。profile 根据出站协议、工具、控制和 system/developer 指令计算摘要，不包含用户对话正文或 Share/用户身份。相同 profile 的后续请求遵守冷却，其他 profile 可继续准入。普通或未知 429 保留模型级限流，401/403 保留公开车道级保护；不会因为改名或重铸会话忽略这些限制。缓存最多 512 条；请求 profile 缓存饱和不会将其拒绝提升为全模型或全车道冷却。

调试日志记录受限的原始触发请求 ID、上游请求 ID、profile 摘要及到期时间。本地冷却错误保留作用域、固定原因与不含业务词的关联摘要，避免诊断 ID 中的 `quota` 等词干扰健康分类。冷却只存于当前进程内存；修复发布后的正常进程替换会丢弃旧版本的宽作用域冷却，没有新增远程清除接口。模型测试将合法 Retry-After 单独传给 Web；本地冷却使用独立类别，明确本次未发送上游请求。429、403、401 不自动重放，分别保留对应 HTTP 状态；其他供应商错误映射为网关错误。客户端和日志不拼接上游原始正文/HTML，只保留固定类别、原状态、受限请求 ID 及合法数字或 HTTP-date Retry-After 提示，并通过公共错误元数据向客户端透传。已识别的参数/上下文错误映射为 400；不能识别的错误仍按网关错误处理。重铸 session 不会创造新额度。

公开目录、管理发现与生成准入共用缓存：8 分钟 fresh，失败后旧成功目录最多保留 24 小时；冷启动无可用目录则拒绝生成，不使用写死清单。单次刷新最多 15 秒、响应 1 MiB、512 个模型、每 ID 256 字节；并发 single-flight，失败退避 30 秒。模型类型、免费分类和协议共同决定准入；decision/image/未知类型及决策模型 `jev*` 不作为 Chat 模型开放。手动刷新可绕过 fresh，但受 30 秒最短间隔、并发合并和失败退避限制。

成功目录原子写入数据目录 `opencode-zen-catalog.json`；重启时仅恢复有效时间窗内、大小受限的版本化完整模型快照，兼容旧版免费 ID 快照，未来时间或过期数据无效。缓存保存失败不撤销上游成功结果。目录规范化 name/type、旧式 cost.input/output 与现代 cost.text.*、limit.context/output 及旧式容量声明。显式零价、收费、未知分开；正价（含阶梯、缓存和模式）优先于 free 名称，缺价不补零。Legacy ID 来源单独标注。成功空名单撤销旧候选，失败不清空；管理发现与生成只从同一快照派生可用子集。目录保留合法 context_window/max_output_tokens 声明；公开接口可投影这些声明，管理端明确列为 upstreamDeclared，不冒称实测能力。降级返回旧目录时管理发现标记 stale。

## 证据与验证

参考身份和逐文件 SHA-256 见根 `PROTOCOL_EVIDENCE.md`，只读参考 `zen-gate` HEAD `58fa1b7`。外部仓库不参与构建、测试、发布或运行。

本仓回归覆盖协议矩阵、工具往返、逐字节分片、错误/终态、缓存恢复与冷启动并发退避；forwarder loopback 验证增量交付、未读取响应取消、续写拼接/用量及限流无重放。完整验证与 review 结果由实施回执记录，离线通过不代表厂商真实通过。

2026-11-18 本轮真实验收使用固定请求规范及 Bun 传输的完整 Server 二进制（SHA-256 `825386de4666283687c97007aabad4a6f8324eb62e311b46fd0eedeecc82c235`）。每个节点均通过 `hy4-flash-free` / `deepseek-v4.1-flash-free` × Claude/Codex × 流/非流的 8 项管理 HTTP 模型检测，合计 24/24；HY4 经签名 Share 入口完成 Chat/Responses/Messages 的工具调用与结果回传，每节点 3 组，合计 9/9。Share 验证使用独立临时身份和签名，不能冒称实际公网 Router 端到端验收。US 另通过显式认证 CONNECT 代理验证自动下载与推理，三节点均启用 IPv4 约束。临时服务和配置已清理，既有 Server 进程未被本轮验收重启；MY 的独立进程核对说明见根证据。本轮未替换生产二进制。该结果不扩展为 arm64/musl、其他免费模型、媒体或可选控制参数的实测证明。

2026-11-18 的整体 review 与验证快照：Rust 库 4574 通过 / 0 失败 / 5 忽略、API 契约 139/139、两个独立集成目标各 1/1；Zen 相关回归 66/66，另两项固定 Bun 进程测试显式运行 2/2 通过。完整静态检查通过，包含严格 Clippy、全部 src/include 格式、Node 审计 2205/2205 与 smoke 脚本回归 13/13、Web 类型检查及 61 文件 / 451 项测试。最终 check/build、本地 10 步 HTTP smoke、生成契约与文档索引通过；源码及传输资产摘要与真实验收版本一致。离线 readiness 的 failures=0，仍因未提供全产品真实输入及部署验收而为 `blocked_inputs`。完整命令、忽略项和验收边界见根 `PROTOCOL_EVIDENCE.md` 的 CLI request/transport rejection repair 条目。

2026-11-18 的整体 review 修正了无凭据 Profile 的创建与运行时准入、Responses 终态后数据校验、慢下游对空闲超时的影响、分片事件解析前的内存预留，以及续写准入失败时的 attempt 编号。SQLite loopback 同时核对一条公开日志的汇总用量与两个独立 USD attempt，防止最后一个 attempt 重复计入首段。Chat 错误帧按公共协议携带错误后以 `[DONE]` 结束；该分隔符本身不是成功证据，成功还须有效终态、干净 EOF 和完成日志。

2026-11-17 最终验证：Zen 相关 28/28、API 集成 138/138、Node 审计 2205/2205；严格 Clippy、check、build、完整格式检查与本地 HTTP smoke 通过。全量 Rust library 为 4479 passed / 2 failed / 3 ignored，两项为既有额度基线失败，完整套件未全绿；具体名称与最终源码摘要见 `PROTOCOL_EVIDENCE.md` 的 Zen 条目。离线 readiness 为 blocked_inputs，真实目录/推理及 Router/Share 验收继续 live_pending。

2026-11-20 工具准入修复完成整体 review：Rust 库 4607/4607、API 契约 139/139、两个独立集成测试各 1/1，默认保留 5 项 opt-in 忽略；其中两项固定 Bun 测试另行显式执行 2/2 通过。严格 Clippy、全量格式与 check、Node 审计 2205/2205、smoke 脚本测试 13/13、Web 测试 448/448 和 10 步本地 HTTP smoke 全部通过。实际 Router Share 的负向回归验证同一 profile 连续 3 次 429 后，其他 profile 仍可成功，且上游拒绝仅发生一次。离线 readiness 的 failures=0，缺全产品真实输入和生产部署验收时仍为 `blocked_inputs`，不替代上述定向真实 Zen 验收。生产更新后应通过一次正式模型检测确认恢复，并核对 Router 的最新健康结果。

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
