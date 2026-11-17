# OpenCode Zen 免 Key 反代

状态：单条公开免费车道；协议离线/本地 loopback 验证与真实供应商验收分开记录。真实目录、推理及 Router 链路仍为 `live_pending`。

不使用 OpenCode Go、付费 Zen API Key、账号池或出口轮换；不注册第四个 App。Claude/Codex 的 Profile 使用 `special.opencode_zen`，无用户凭据，固定 `https://opencode.ai`，配置不可覆盖端点。Gemini Surface 不开放。

## 线格式与协议转换

- Chat：`/inference/v1/chat/completions`；Muse Spark：`/inference/v1/responses`；`union-alpha`：`/inference/v1/messages`。
- `Bearer public`；UA `opencode/latest/2.1.3/cli`；官方 CLI/session/affinity 头，不发 `x-opencode-request`。
- Session 是可解码时间戳前缀加随机尾部，6 天重铸。缓存键包含 Provider、Share、用户和下游会话；缺少会话时每次请求独立，不把全部用户折叠到 Provider ID。4096 条、哈希键、单条最旧铸造记录淘汰，不接受下游直接指定上游 session。
- 官方 `read`、`shell` 原文指纹；冲突工具改名并恢复，Chat 名称增量支持别名分片。缓存提示不注入上游，工具 schema 中的同名属性不删除。
- Chat JSON 关闭 HTML 转义并保持 CLI 顶层字段顺序；默认输出上限 32000。`tool_choice:none` 在出站前拒绝。

入口协议与模型的上游协议独立。使用 Server 自有 transforms 与 stateful stream bridge，覆盖 Messages/Chat/Responses 三入口 × 三上游协议 × 流/非流的 18 个基础组合。请求转换保留 system/instructions、工具调用 ID、工具结果与历史，响应转换遵循入口格式，Chat 的 created 使用统一兼容边界。

本 Driver 不实现 Responses 持久化资源；`store:true` 和 `previous_response_id` 显式拒绝，调用方须提供完整历史。目录仅公布受审的文本/工具能力，不开放未经实测的图像/音频能力。

## 流式、终态与资源

SSE 增量解析，支持网络任意分片、UTF-8 分片、多行 data 与 CRLF。单事件及终态尾部各最多 1 MiB；JSON 响应最多 8 MiB，并受剩余请求内存约束。解析/转换临时空间预留预算，保留状态及输出 Bytes 跟随生命周期释放。

Chat 必须有合法 finish_reason 与 `[DONE]`；Responses/Messages 使用公共严格检查器。非 Chat 终态后不接受数据；错误事件、非法 JSON、残缺帧或缺失终态不能成为成功。成功终态输出保留至干净 EOF，语义内容在 EOF 前增量交付；length/incomplete/max_tokens 保留不完整状态。

总 deadline 覆盖整个请求；首语义 deadline 不因 role/usage/心跳重置。空闲时限只由上游字节推进，下游每 15 秒的 SSE 心跳不延长它。慢下游采用拉取背压；丢弃响应体时取消上游并记录 client_cancelled。账户/Share in-flight、请求预算和计量证据随响应体持有；响应从未被读取时也有取消收尾。

## 一次非流续写与计量

仅对 Chat 上游的纯推理中断、仅推理正常结束及输出上限截断评估一次续写，不在已提交 SSE 中发起第二次生成。须有实际 output usage、至少 512 剩余 token、上下文检查点符合本地保守上限且总时间尚有余量；第二段最多 8192 token，不重置请求总预算。没有 usage 时不猜测剩余预算。

截断回答保留第一段并追加续写，不以第二段替换第一段。不同生成的 usage 相加，同次 SSE 累计 usage 由公共累积器去重；Messages 采用 Exclusive 输入语义。续写失败保留有效首段及原有截断状态，无有效回答时返回失败。工具生成不会触发续写；续写意外调用工具会失败，不执行工具。

每次上游生成使用独立 USD attempt。证据沿用公开请求 ID 并递增 attempt；公开日志及 token 分享用量只汇总一次，`value_generation_usage` 单独保存当前生成值，不能把累计 usage 写入最后一个 USD attempt。未知发送/未知用量保持保守状态，不将网络错误冒充未发送。

## 错误与模型目录

429、403、401 不自动重放，分别保留对应 HTTP 状态；其他供应商错误映射为网关错误。客户端和日志不拼接上游原始正文/HTML，只保留固定类别、原状态、受限请求 ID 及合法数字 Retry-After 提示。重铸 session 不会创造新额度。

公开目录、管理发现与生成准入共用缓存：8 分钟 fresh，失败后旧成功目录最多保留 24 小时；冷启动无可用目录则拒绝生成，不使用写死清单。单次刷新最多 15 秒、响应 1 MiB、512 个模型、每 ID 256 字节；并发 single-flight，失败退避 30 秒。决策模型 `jev*` 不作为 Chat 模型开放。

成功目录原子写入数据目录 `opencode-zen-catalog.json`；重启时仅恢复有效时间窗内、大小受限的免费模型快照，未来时间或过期数据无效。缓存保存失败不撤销上游成功结果。目录保留合法 context_window/max_output_tokens 声明；公开接口可投影这些声明，管理端明确列为 upstreamDeclared，不冒称实测能力。降级返回旧目录时管理发现标记 stale。

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
